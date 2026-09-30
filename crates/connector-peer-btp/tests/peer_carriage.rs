//! The BTP peer carriage end to end (`docs/protocol/peer-carriage-spec.md`,
//! issue #727): a [`BtpPeerTransport`] **dials**, a [`PeerSession`]
//! **accepts**, and the frames between them are the ones §3's table names.
//!
//! # A peering is two x402 channels (ADR 0075, #1380)
//!
//! Every packet a peer sends is covered by an x402 `batch-settlement`
//! **voucher** riding the claim slot, or -- for a PREPARE that moves no value
//! -- by the voucher claim-state **challenge** riding a slot of its own (ADR
//! 0075 decision 5). Either proves the peer role only when the channel it
//! names is resolved by the receiving half, its signature recovers to that
//! channel's voucher signer *as the chain records it*, and that signer is
//! bound to a peering ([`Connector::bind_voucher_signer`]). A `toon-channel`
//! claim proves nothing any more: a frame carrying one is a client's, however
//! genuinely it is signed. There is no FLUSH either -- a voucher rides the
//! PREPARE it covers, and a TRANSFER judges nothing.
//!
//! Once a voucher has proved the role it is judged as payment against the
//! channel's **one** amount watermark (decision 6): an advancing amount is
//! accepted, a byte-identical resend at the watermark is accepted again and
//! pays nothing new, and anything else below it is `amount_not_advancing`.
//! The price gate measures coverage by the advance the voucher made past the
//! watermark as it stood before it (issue #1104's rule).
//!
//! # What is real and what is a fake
//!
//! The two sides are joined by an in-memory duplex standing in for the
//! websocket and *only* for the websocket: every frame is encoded and decoded
//! by `connector-btp`, every role decision is
//! `connector_peer_btp::role_gate::decide_frame`'s over a real `Connector`'s
//! voucher-signer bindings, every voucher is signed with a real secp256k1 key
//! over the real EIP-712 digest, and the payer reaches the payee only through
//! the `PeerTransport` port.
//!
//! The receiving half -- which channels exist on chain, and the durable
//! watermark on each -- is [`ChannelBook`], a fake upholding
//! [`VoucherEvidence`]'s contract (ADR 0007): it resolves a channel from what
//! "the chain" holds, never from the voucher, verifies every signature with
//! `connector_signer`'s own verification, and keeps one watermark per
//! channel. The real implementation is `connector-client-edge`'s claim gate,
//! which this crate cannot depend on. What is not exercised is TLS and the
//! socket itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_btp::{
    decode_frame, encode_message, encode_response, BtpFrame, BtpSessionHandle, OutboundRequests,
    ProtocolData, AUTH_PROTOCOL, BTP_ERROR, BTP_RESPONSE, CLAIM_PROTOCOL, CONTENT_TYPE_TEXT,
    PEER_CHALLENGE_PROTOCOL,
};
use connector_config::StaticRoute;
use connector_domain::client_claim::{ClientClaim, EvmVoucherChannelConfig};
use connector_domain::{EnvelopeRequest, EnvelopeResponse, PacketResponse, Prepare};
use connector_peer_btp::accept::{PeerAcceptPolicy, PeerSession, SessionEnd};
use connector_peer_btp::challenge_json::{self, PeerRoleChallenge};
use connector_peer_btp::dial::{DialError, PeerDialer, PeerRelation};
use connector_peer_btp::role_gate::PeerVoucherVerdict;
use connector_peer_btp::{
    ack, decode_answer, BtpPeerTransport, ClaimEnforcementPolicy, PeerCarriageState, VoucherCheck,
    VoucherEvidence,
};
use connector_runtime::covering_fake::covering;
use connector_runtime::{
    ClaimAckOutcome, ClaimRejectReason, Connector, Covering, FakeAppClient, InProcessPeerTransport,
    PeerForward, PeerRoute, PeerTransport, TestClock, VoucherSigner,
};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_claim_state_challenge_digest,
    evm_voucher_digest, evm_voucher_signer, verify_evm_voucher,
    verify_evm_voucher_claim_state_challenge, BatchChannelConfig, BatchSettlementDomain,
    LocalSigner, Signer,
};
use libsecp256k1::{Message, PublicKey, SecretKey};
use tokio::sync::{mpsc, oneshot};
use url::Url;

// ─── fixtures: keys, channels, vouchers ───

const CHAIN_ID: u64 = 84_532;
const PEER_ID: &str = "peer-b";

fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(CHAIN_ID)
}

/// The dialing peer's EVM settlement key: its channel's `payerAuthorizer`
/// (ADR 0075 decision 3), and the signer this node binds to [`PEER_ID`].
fn payer_key() -> SecretKey {
    SecretKey::parse(&[0x0a; 32]).expect("valid secret")
}

/// Somebody else: a key with a real channel toward this node that is bound
/// to no peering, and the key a forger signs with.
fn stranger_key() -> SecretKey {
    SecretKey::parse(&[0x0b; 32]).expect("valid secret")
}

fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

/// The binding every payee here carries: the payer's settlement address
/// proves [`PEER_ID`] -- what a `[[peer_channels]]` row, or a runtime
/// peering's self-description, feeds `Connector::bind_voucher_signer`.
fn payer_signer() -> VoucherSigner {
    VoucherSigner::Evm(address_of(&payer_key()))
}

/// `owner`'s channel toward this node. `salt` tells two channels of one
/// owner apart, so a fixture can name one the chain does not hold.
fn config_of(owner: &SecretKey, salt: u8) -> BatchChannelConfig {
    let payer = address_of(owner);
    BatchChannelConfig {
        payer,
        payer_authorizer: payer,
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [salt; 32],
    }
}

/// The payer's one channel toward this payee.
fn payer_channel() -> BatchChannelConfig {
    config_of(&payer_key(), 0x66)
}

/// The stranger's own channel toward this payee: real, admitted, and
/// signed on by a key no peering is bound to.
fn stranger_channel() -> BatchChannelConfig {
    config_of(&stranger_key(), 0x77)
}

/// A channel of the payer's that the chain holds no record of.
fn unopened_channel() -> BatchChannelConfig {
    config_of(&payer_key(), 0x99)
}

fn channel_id_of(config: &BatchChannelConfig) -> [u8; 32] {
    evm_batch_channel_id(&domain(), config)
}

/// `r ‖ s ‖ v` with `v` in `{27, 28}`, as `x402BatchSettlement` recovers it.
fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> [u8; 65] {
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(digest), secret);
    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&signature.serialize());
    bytes[64] = recovery.serialize() + 27;
    bytes
}

fn hex0x(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn config_json(config: &BatchChannelConfig) -> serde_json::Value {
    serde_json::json!({
        "payer": hex0x(&config.payer),
        "payerAuthorizer": hex0x(&config.payer_authorizer),
        "receiver": hex0x(&config.receiver),
        "receiverAuthorizer": hex0x(&config.receiver_authorizer),
        "token": hex0x(&config.token),
        "withdrawDelay": config.withdraw_delay,
        "salt": hex0x(&config.salt),
    })
}

/// A voucher on `channel` for cumulative `amount`, signed by `signer` over
/// the real EIP-712 voucher digest. The JSON is x402's own voucher payload,
/// its `channelConfig` presented so the receiver can re-hash it to the id.
fn voucher_on(channel: &BatchChannelConfig, signer: &SecretKey, amount: u64) -> String {
    let channel_id = channel_id_of(channel);
    let digest = evm_voucher_digest(&domain(), &channel_id, u128::from(amount));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": "peer",
        "channelId": hex0x(&channel_id),
        "maxClaimableAmount": amount.to_string(),
        "signature": hex0x(&sign_evm(signer, &digest)),
        "channelConfig": config_json(channel),
    })
    .to_string()
}

/// The payer's genuine voucher on its own channel.
fn payer_voucher(amount: u64) -> String {
    voucher_on(&payer_channel(), &payer_key(), amount)
}

/// A voucher on the payer's channel signed by somebody who is not its
/// `payerAuthorizer`: it decodes, and it can declare any amount it likes.
fn forged_voucher(amount: u64) -> String {
    voucher_on(&payer_channel(), &stranger_key(), amount)
}

fn paid_with(voucher: String) -> Option<Covering> {
    Some(Covering::Voucher(voucher))
}

/// The voucher claim-state challenge on `channel`, valid until `expires`,
/// signed by `signer` (ADR 0075 decision 5).
fn challenge_on(channel: &BatchChannelConfig, signer: &SecretKey, expires: u64) -> String {
    let channel_id = channel_id_of(channel);
    let digest = evm_voucher_claim_state_challenge_digest(&domain(), &channel_id, expires);
    challenge_json::encode(&PeerRoleChallenge::Evm {
        channel_id,
        expires,
        signature: sign_evm(signer, &digest),
        channel_config: Some(
            connector_domain::client_claim::parse_evm_channel_config(&config_json(channel))
                .expect("a well-formed config"),
        ),
    })
}

// ─── the receiving half: a fake upholding `VoucherEvidence`'s contract ───

/// One channel as "the chain" holds it: its config and the collateral
/// behind it.
struct OnChain {
    config: BatchChannelConfig,
    max_cumulative: u128,
}

/// The voucher a channel's watermark stands at: its amount, and its
/// signature, so a byte-identical resend can be told from a rival.
#[derive(Clone)]
struct Accepted {
    amount: u128,
    signature: String,
}

/// **The receiving half, as a fake that upholds [`VoucherEvidence`]'s
/// contract** (ADR 0007) -- not a stub that expects calls.
///
/// It knows a fixed set of EVM `x402BatchSettlement` channels (what the
/// chain holds: config and collateral), and it answers each of the port's
/// three questions by the port's own rules:
///
/// * [`VoucherEvidence::check_voucher`] and
///   [`VoucherEvidence::check_challenge`] resolve the channel **from what
///   the chain holds**, re-hashing a presented `channelConfig` to the id
///   before trusting it, take the signer from that config
///   (`evm_voucher_signer`, never from the evidence), and verify the
///   signature with `connector_signer`'s real EIP-712 verification. A
///   channel it cannot resolve is `Unresolved`; nothing is advanced.
/// * [`VoucherEvidence::judge_peer_voucher`] holds **one watermark per
///   channel**: an amount above it (and within collateral) is accepted and
///   advances it; a byte-identical resend of the voucher at the watermark is
///   accepted and advances nothing; anything else at or below it is
///   `amount_not_advancing`. `prior` is the watermark before the voucher.
///
/// Shared behind an [`Arc`] so a payee "restarted" over the same book --
/// new `Connector`, new `PeerCarriageState` -- finds the watermarks where
/// the last one left them. That is the durable journal's role in the real
/// claim gate, and what issue #1104's property is measured against.
struct ChannelBook {
    channels: HashMap<[u8; 32], OnChain>,
    watermarks: Mutex<HashMap<[u8; 32], Accepted>>,
}

impl ChannelBook {
    /// A chain holding the payer's channel and the stranger's, each with
    /// more collateral than any test here spends.
    fn new() -> Arc<ChannelBook> {
        let channels = [payer_channel(), stranger_channel()]
            .into_iter()
            .map(|config| {
                (
                    channel_id_of(&config),
                    OnChain {
                        config,
                        max_cumulative: 1_000_000_000,
                    },
                )
            })
            .collect();
        Arc::new(ChannelBook {
            channels,
            watermarks: Mutex::new(HashMap::new()),
        })
    }

    /// Where `channel`'s watermark stands, `None` before anything was
    /// accepted on it.
    fn watermark(&self, channel: &BatchChannelConfig) -> Option<u128> {
        self.watermarks
            .lock()
            .expect("watermarks lock")
            .get(&channel_id_of(channel))
            .map(|accepted| accepted.amount)
    }

    /// The channel `channel_id` names, as the chain holds it. A presented
    /// config is trusted only when it hashes to the id and is the one the
    /// chain holds.
    fn resolve(
        &self,
        channel_id: &[u8; 32],
        presented: Option<&EvmVoucherChannelConfig>,
    ) -> Option<&OnChain> {
        let on_chain = self.channels.get(channel_id)?;
        match presented {
            None => Some(on_chain),
            Some(presented) => {
                let presented = decode_config(presented)?;
                (presented == on_chain.config
                    && evm_batch_channel_id(&domain(), &presented) == *channel_id)
                    .then_some(on_chain)
            }
        }
    }

    /// The channel an EVM voucher names and whether its signature recovers
    /// to that channel's signer; `None` for a channel this chain lacks or a
    /// voucher whose fields do not decode.
    fn verify(&self, voucher: &ClientClaim) -> Option<([u8; 32], &OnChain, VoucherSigner, bool)> {
        let ClientClaim::EvmVoucher(voucher) = voucher else {
            return None;
        };
        let channel_id = decode_hex::<32>(&voucher.channel_id)?;
        let signature = decode_hex::<65>(&voucher.signature)?;
        let on_chain = self.resolve(&channel_id, voucher.channel_config.as_ref())?;
        let signer = evm_voucher_signer(&on_chain.config);
        let verified = verify_evm_voucher(
            &domain(),
            &channel_id,
            voucher.max_claimable_amount,
            &signature,
            &signer,
        );
        Some((channel_id, on_chain, VoucherSigner::Evm(signer), verified))
    }
}

#[async_trait]
impl VoucherEvidence for ChannelBook {
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck {
        match self.verify(voucher) {
            Some((_, _, signer, true)) => VoucherCheck::Verified(signer),
            Some((_, _, signer, false)) => VoucherCheck::SignatureInvalid(signer),
            None => VoucherCheck::Unresolved,
        }
    }

    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck {
        let PeerRoleChallenge::Evm {
            channel_id,
            expires,
            signature,
            channel_config,
        } = challenge
        else {
            // This chain holds EVM channels only.
            return VoucherCheck::Unresolved;
        };
        let Some(on_chain) = self.resolve(channel_id, channel_config.as_ref()) else {
            return VoucherCheck::Unresolved;
        };
        let signer = evm_voucher_signer(&on_chain.config);
        if verify_evm_voucher_claim_state_challenge(
            &domain(),
            channel_id,
            *expires,
            signature,
            &signer,
        ) {
            VoucherCheck::Verified(VoucherSigner::Evm(signer))
        } else {
            VoucherCheck::SignatureInvalid(VoucherSigner::Evm(signer))
        }
    }

    async fn judge_peer_voucher(&self, voucher: &ClientClaim) -> PeerVoucherVerdict {
        let rejected = |reason, prior| PeerVoucherVerdict {
            ack: ClaimAckOutcome::Rejected(reason),
            prior,
        };
        let Some((channel_id, on_chain, _, verified)) = self.verify(voucher) else {
            return rejected(ClaimRejectReason::UnknownChannel, 0);
        };
        let mut watermarks = self.watermarks.lock().expect("watermarks lock");
        let standing = watermarks.get(&channel_id).cloned();
        let prior = standing.as_ref().map_or(0, |accepted| accepted.amount);
        if !verified {
            return rejected(ClaimRejectReason::SignatureInvalid, prior);
        }
        let ClientClaim::EvmVoucher(voucher) = voucher else {
            unreachable!("verify resolved an EVM voucher");
        };
        let amount = voucher.max_claimable_amount;
        let resend = standing.as_ref().is_some_and(|accepted| {
            accepted.amount == amount && accepted.signature.eq_ignore_ascii_case(&voucher.signature)
        });
        let ack = if resend {
            // A lost ack must not wedge the peering: the voucher the
            // watermark already holds is accepted again, and pays nothing.
            ClaimAckOutcome::Accepted
        } else if amount <= prior || amount > on_chain.max_cumulative {
            // Not advancing, or past the collateral -- which the real gate
            // answers the same way.
            ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
        } else {
            watermarks.insert(
                channel_id,
                Accepted {
                    amount,
                    signature: voucher.signature.clone(),
                },
            );
            ClaimAckOutcome::Accepted
        };
        PeerVoucherVerdict { ack, prior }
    }
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    let digits = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(digits).ok()?.try_into().ok()
}

fn decode_config(config: &EvmVoucherChannelConfig) -> Option<BatchChannelConfig> {
    Some(BatchChannelConfig {
        payer: decode_hex(&config.payer)?,
        payer_authorizer: decode_hex(&config.payer_authorizer)?,
        receiver: decode_hex(&config.receiver)?,
        receiver_authorizer: decode_hex(&config.receiver_authorizer)?,
        token: decode_hex(&config.token)?,
        withdraw_delay: config.withdraw_delay,
        salt: decode_hex(&config.salt)?,
    })
}

// ─── fixtures: nodes and carriages ───

fn clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

/// This node's clock, in unix seconds: what a challenge's `expires` is
/// judged against.
fn now_unix() -> u64 {
    u64::try_from(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp(),
    )
    .expect("after the epoch")
}

fn node(routes: Vec<StaticRoute>, app_client: Arc<FakeAppClient>) -> Connector {
    Connector::new(
        routes,
        vec![],
        app_client,
        Arc::new(InProcessPeerTransport::new()),
        clock(),
    )
}

/// `connector`, with [`PEER_ID`] configured and the payer's settlement
/// address bound to it: the one fact that makes the payer's vouchers a
/// peer's (ADR 0075 decision 5).
fn bound(connector: Connector) -> Arc<Connector> {
    let connector = Arc::new(connector.with_config_peer_ids([PEER_ID.to_string()]));
    connector
        .bind_voucher_signer(PEER_ID, payer_signer())
        .expect("peer-b is a configured peering");
    connector
}

/// The payee: a connector with no routes -- so every packet it is handed
/// answers `F02` and the *voucher*'s verdict is visibly independent of the
/// packet's (§6.2).
fn payee() -> Arc<Connector> {
    bound(node(vec![], Arc::new(FakeAppClient::new())))
}

/// The one priced, terminated route issue #880's gate and issue #1104's
/// restart tests both need.
fn priced_route() -> StaticRoute {
    StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap()
}

/// This payee's identity key, deterministic so that a node built before a
/// restart and the one built after it are the same node to a sender that
/// sealed to it (issue #1104).
fn payee_identity() -> Arc<dyn Signer> {
    Arc::new(LocalSigner::from_secret_bytes("payee-identity", [0x5c; 32]).expect("identity signer"))
}

/// As [`payee`], but terminating [`priced_route`] and delivering to
/// `app_client` -- the fixture issue #880's price-coverage gate needs,
/// since a payee with no routes never reaches that gate.
fn priced_payee(app_client: Arc<FakeAppClient>) -> Arc<Connector> {
    bound(node(vec![priced_route()], app_client).with_identity_signer(payee_identity()))
}

fn carriage(connector: Arc<Connector>, book: &Arc<ChannelBook>) -> Arc<PeerCarriageState> {
    carriage_with(
        connector,
        book,
        Arc::new(ClaimEnforcementPolicy::default()),
        PeerAcceptPolicy::default(),
    )
}

/// [`carriage`], with an explicit [`ClaimEnforcementPolicy`] and accept
/// policy. Only ADR 0042's forwarded rule answers to the enforcement
/// policy: the terminated rule's own knob was deleted with its escape hatch
/// (issue #1077) and refuses unconditionally.
fn carriage_with(
    connector: Arc<Connector>,
    book: &Arc<ChannelBook>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    policy: PeerAcceptPolicy,
) -> Arc<PeerCarriageState> {
    Arc::new(
        PeerCarriageState::new(connector, enforcement, policy)
            .with_voucher_evidence(Arc::clone(book) as Arc<dyn VoucherEvidence>),
    )
}

/// The next hop a forwarded arrival is carried to (ADR 0042's item 3), and
/// the destination that resolves to it.
const NEXT_HOP_ID: &str = "next-hop";
const FORWARDED_DESTINATION: &str = "g.example.onward";

/// This peering's flat fee, and the client-edge `price` its forwarded route
/// carries. Both are deliberately non-zero and deliberately *not* what a
/// forwarded arrival must cover -- ADR 0042 requires the packet's own
/// `amount`, so a voucher advancing either of these figures is short.
const FORWARD_FEE: u64 = 3;
const FORWARD_ROUTE_PRICE: u64 = 5;

/// The amount every forwarded-arrival test sends, matching [`prepare`].
const ARRIVING_AMOUNT: u64 = 100;

/// As [`payee`], but **forwarding**: one `peer_id` route over which
/// [`FORWARDED_DESTINATION`] reaches a real second connector that terminates
/// it, paid on this node's own outbound x402 channel
/// ([`connector_runtime::covering_fake::covering`]) -- since issue #1145 a
/// connector covers every PREPARE it sends, and a fixture that forwards
/// without that is one no configuration can produce.
///
/// Returns the next hop's own app client and identity signer too, so a test
/// can seal a packet the far end can actually fulfil and then prove the
/// packet really was carried rather than merely not refused.
fn forwarding_payee() -> (Arc<Connector>, Arc<FakeAppClient>, Arc<dyn Signer>) {
    let next_hop_route = StaticRoute::new(FORWARDED_DESTINATION, "http://localhost:4100").unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        next_hop_route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"delivered by the next hop".to_vec(),
            },
        },
    );
    let identity: Arc<dyn Signer> = Arc::new(LocalSigner::generate("next-hop-identity"));
    let next_hop = Arc::new(
        node(vec![next_hop_route], app_client.clone()).with_identity_signer(Arc::clone(&identity)),
    );
    let mut onward = InProcessPeerTransport::new();
    onward.add_peer(NEXT_HOP_ID, next_hop);

    let connector = bound(covering(
        Connector::new(
            vec![],
            vec![PeerRoute::new_priced(
                FORWARDED_DESTINATION,
                NEXT_HOP_ID,
                FORWARD_ROUTE_PRICE,
            )],
            Arc::new(FakeAppClient::new()),
            Arc::new(onward),
            clock(),
        )
        .with_peer_fees([(NEXT_HOP_ID.to_string(), FORWARD_FEE)]),
        NEXT_HOP_ID,
    ));
    (connector, app_client, identity)
}

/// A PREPARE sealed to `identity`'s public key (ADR 0018/0019) so the hop
/// that finally terminates it can fulfil, plus the shared secret needed to
/// open the answer. Sealing is orthogonal to every gate here and is what
/// makes "the packet was carried" provable rather than inferred.
fn sealed_prepare_to(identity: &dyn Signer, destination: &str, amount: u64) -> (Prepare, [u8; 32]) {
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello".to_vec(),
    };
    let identity_public = identity.public_key().expect("identity public key");
    let (data, shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope.encode(), &identity_public)
            .expect("seal");
    (
        Prepare {
            amount,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data,
        },
        shared_secret,
    )
}

/// A PREPARE sealed to [`payee_identity`] at this node's own priced
/// termination.
fn sealed_prepare(amount: u64) -> (Prepare, [u8; 32]) {
    sealed_prepare_to(payee_identity().as_ref(), "g.example.app", amount)
}

/// An app that actually answers `route`'s handler, so a packet the gate
/// admits visibly **fulfils**.
fn serving_app(route: &StaticRoute, body: &[u8]) -> Arc<FakeAppClient> {
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: body.to_vec(),
            },
        },
    );
    app_client
}

/// The body of a fulfil sealed under `shared_secret`.
fn opened_body(response: PacketResponse, shared_secret: &[u8; 32]) -> Vec<u8> {
    let fulfill = match response {
        PacketResponse::Fulfill(fulfill) => fulfill,
        other => panic!("expected a fulfil, got {other:?}"),
    };
    let opened = connector_signer::giftwrap::open_response(shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    EnvelopeResponse::decode(&opened)
        .expect("decode envelope")
        .body
}

/// A policy in which `PEER_ID` enforces ADR 0042's forwarded rule. There is
/// no terminated setting to leave alone: ADR 0029's rule always enforces
/// (issue #1077 deleted `claim_enforcement`).
fn forwarded_enforcing() -> Arc<ClaimEnforcementPolicy> {
    Arc::new(ClaimEnforcementPolicy::of(vec![(
        PEER_ID,
        connector_config::ForwardedClaimEnforcement::Enforce,
    )]))
}

fn prepare(destination: &str) -> Prepare {
    prepare_of(destination, ARRIVING_AMOUNT)
}

fn prepare_of(destination: &str, amount: u64) -> Prepare {
    Prepare {
        amount,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data: b"sealed to whoever terminates this route".to_vec(),
    }
}

// ─── the in-memory duplex standing in for the websocket ───

/// Runs one accepting [`PeerSession`] per dial, joined to the dialing side
/// by two channels. Every byte between them goes through the real codec.
struct LoopbackDialer {
    state: Arc<PeerCarriageState>,
    /// Every frame the dialing side wrote, in order -- so a test can assert
    /// what actually went on the wire (§3's table) and not merely what came
    /// back.
    sent: Arc<Mutex<Vec<BtpFrame>>>,
    /// How many sockets were opened. Counted here because there is nothing
    /// else left to count: session reuse used to be provable from the one
    /// `auth` frame a session sent, and ADR 0060 deleted it.
    dials: Arc<AtomicUsize>,
    /// One session per dial: the switch that kills it, and the signal it
    /// sends back once it is provably dead. Killing one closes the reply
    /// channel the dialing side holds -- what `ws`'s real dialer does when
    /// the socket's read loop stops, and therefore what a payee's restart
    /// leaves behind.
    live: Mutex<Vec<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl LoopbackDialer {
    fn new(state: Arc<PeerCarriageState>) -> Arc<LoopbackDialer> {
        Arc::new(LoopbackDialer {
            state,
            sent: Arc::new(Mutex::new(Vec::new())),
            dials: Arc::new(AtomicUsize::new(0)),
            live: Mutex::new(Vec::new()),
        })
    }

    /// The far side restarts (issue #1240): every session opened so far
    /// dies where it stands, and this returns only once each one is dead.
    /// The accepting carriage behind it is deliberately the same one --
    /// what restarted is the socket, not the payee's book, so a voucher's
    /// watermark survives exactly as a restarted node's durable one does.
    async fn restart_the_far_side(&self) {
        let sessions = std::mem::take(&mut *self.live.lock().expect("live sessions lock"));
        for (kill, dead) in sessions {
            drop(kill);
            // Resolves (as an error) when that session drops its end,
            // which it does only after dropping the receiver whose closure
            // is what the dialing side reads.
            let _ = dead.await;
        }
    }

    fn sent(&self) -> Vec<BtpFrame> {
        self.sent.lock().expect("sent frames lock").clone()
    }
}

#[async_trait]
impl PeerDialer for LoopbackDialer {
    async fn dial(&self, _peer_id: &str, _endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let (to_peer, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let (from_peer, mut from_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(to_peer, Arc::clone(&outbound));

        // The accepting side, reading exactly the bytes the dialing side
        // wrote.
        let (tap, sent) = (mpsc::channel::<Vec<u8>>(32), Arc::clone(&self.sent));
        let (tapped, tapped_rx) = tap;
        let (kill, mut killed) = oneshot::channel::<()>();
        let (dead, buried) = oneshot::channel::<()>();
        self.live
            .lock()
            .expect("live sessions lock")
            .push((kill, buried));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    queued = to_peer_rx.recv() => {
                        let Some(bytes) = queued else { break };
                        sent.lock()
                            .expect("sent frames lock")
                            .push(decode_frame(&bytes).expect("our own encoder"));
                        if tapped.send(bytes).await.is_err() {
                            break;
                        }
                    }
                    // The far side went away. Ending here drops
                    // `to_peer_rx`, so the handle the dialing side still
                    // holds reports itself gone rather than swallowing a
                    // frame nobody will ever read.
                    _ = &mut killed => break,
                }
            }
            // Dropped in this order on purpose: the dialing side must find
            // the channel already closed when the burial is announced.
            drop(to_peer_rx);
            drop(dead);
        });
        let session = PeerSession::new(Arc::clone(&self.state), from_peer);
        tokio::spawn(session.run(tapped_rx));

        // The answer path: a RESPONSE/ERROR resolves whichever outbound
        // request it names (§7.3), which is the only correlation either
        // carriage has or needs.
        tokio::spawn(async move {
            while let Some(bytes) = from_peer_rx.recv().await {
                if let Ok(frame) = decode_frame(&bytes) {
                    outbound.resolve(frame);
                }
            }
        });
        Ok(handle)
    }
}

/// A dialer that never connects -- the "the remote does not expose what we
/// dial" case §2.2 says is not locally detectable and must surface as an
/// ordinary dial failure.
struct DeadDialer;

#[async_trait]
impl PeerDialer for DeadDialer {
    async fn dial(&self, peer_id: &str, endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        Err(DialError {
            peer_id: peer_id.to_string(),
            endpoint: endpoint.to_string(),
            reason: "connection refused".to_string(),
        })
    }
}

fn relation() -> PeerRelation {
    PeerRelation::new(
        PEER_ID,
        Url::parse("wss://peer.example:443/btp").unwrap(),
        Duration::from_millis(30_000),
    )
}

fn transport(dialer: Arc<dyn PeerDialer>) -> BtpPeerTransport {
    let transport = BtpPeerTransport::new(dialer);
    transport.add_peer(relation());
    transport
}

/// A payer dialing `state` over the loopback: its dialer, to read the wire
/// and count sockets, and its transport.
fn dialing(state: Arc<PeerCarriageState>) -> (Arc<LoopbackDialer>, BtpPeerTransport) {
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>);
    (dialer, transport)
}

// ─── §3, §6: a voucher rides a PREPARE and is acknowledged ───

/// §6.2, the property whose loss would silently destroy ADR 0024's
/// semantics: **one RESPONSE carries two independent answers**. The packet
/// is rejected (the payee has no route for it) and the voucher that rode it
/// is accepted, on the same frame.
#[tokio::test]
async fn a_voucher_riding_a_prepare_is_judged_independently_of_the_packet() {
    let book = ChannelBook::new();
    let (_, transport) = dialing(carriage(payee(), &book));

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(500)))
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected the payee's own reject, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(reached, "the peer answered, so this hop forwarded");
    assert_eq!(book.watermark(&payer_channel()), Some(500));
}

/// §3's table, on the wire: a voucher rides the `payment-channel-claim`
/// entry as **raw UTF-8 JSON**, verbatim, on a MESSAGE whose `ilpPacket` is
/// the OER PREPARE -- the session's **first** frame, because ADR 0060
/// deleted the credential that used to ride ahead of it. A peer-role
/// challenge rides an entry of its own (ADR 0075 decision 5), never the
/// claim slot, and on a PREPARE that moves no value it proves the peer role
/// by itself: the answer is the peer's own routing verdict, not the
/// client-role refusal.
#[tokio::test]
async fn the_frames_a_dialed_peering_puts_on_the_wire_are_the_ones_section_3_names() {
    let book = ChannelBook::new();
    let (dialer, transport) = dialing(carriage(payee(), &book));
    let voucher = payer_voucher(500);

    let _ = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(voucher.clone()))
        .await;

    let challenge = challenge_on(&payer_channel(), &payer_key(), now_unix() + 60);
    let PeerForward { response, ack, .. } = transport
        .forward(
            PEER_ID,
            prepare_of("g.zero-value", 0),
            Some(Covering::Challenge(challenge.clone())),
        )
        .await;

    let sent = dialer.sent();
    assert_eq!(sent.len(), 2);
    assert!(
        sent.iter().all(|frame| frame
            .protocol_data
            .iter()
            .all(|pd| pd.name != AUTH_PROTOCOL)),
        "a dialed peering put a peer credential on the wire"
    );

    let message = &sent[0];
    assert_eq!(message.frame_type, connector_btp::BTP_MESSAGE);
    assert!(
        Prepare::decode(&message.ilp_packet).is_ok(),
        "the OER PREPARE rides ilpPacket"
    );
    let claim = message
        .protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .expect("the voucher rode as protocolData");
    assert_eq!(
        claim.data,
        voucher.as_bytes(),
        "the voucher rides verbatim, no base64 layer"
    );
    let claim_json: serde_json::Value = serde_json::from_slice(&claim.data).expect("raw JSON");
    assert_eq!(claim_json["scheme"], "batch-settlement");
    assert_eq!(claim_json["maxClaimableAmount"], "500");

    let challenged = &sent[1];
    assert!(
        challenged
            .protocol_data
            .iter()
            .all(|pd| pd.name != CLAIM_PROTOCOL),
        "a challenge is not a claim and never rides the claim slot"
    );
    let entry = challenged
        .protocol_data
        .iter()
        .find(|pd| pd.name == PEER_CHALLENGE_PROTOCOL)
        .expect("the challenge rode its own entry");
    assert_eq!(entry.data, challenge.as_bytes());

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "F02");
            assert!(
                reject.message.contains("g.zero-value"),
                "the peer's own routing verdict, so the challenge proved the role: {reject:?}"
            );
        }
        other => panic!("expected the peer's own reject, got {other:?}"),
    }
    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "a challenge pays nothing, so nothing is acknowledged"
    );
}

// ─── §6.3: the idempotent re-ack ───

/// §6.3, the rule that stands between a lost ack and a permanently wedged
/// peering: a byte-identical resend of the voucher already at the watermark
/// is answered **`accepted`**, never `amount_not_advancing` -- and it pays
/// nothing new: the watermark stays where the first put it.
#[tokio::test]
async fn a_byte_identical_voucher_resent_at_the_watermark_is_accepted_again() {
    let book = ChannelBook::new();
    let (dialer, transport) = dialing(carriage(payee(), &book));
    let voucher = payer_voucher(900);

    let first = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(voucher.clone()))
        .await;
    let resent = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(voucher.clone()))
        .await;

    assert_eq!(first.ack, ClaimAckOutcome::Accepted);
    assert_eq!(
        resent.ack,
        ClaimAckOutcome::Accepted,
        "a lost ack must not wedge the peering"
    );
    assert_eq!(
        book.watermark(&payer_channel()),
        Some(900),
        "the resend paid nothing new"
    );

    // And the resend really was byte-identical on the wire.
    let claims: Vec<ProtocolData> = dialer
        .sent()
        .into_iter()
        .filter_map(|frame| {
            frame
                .protocol_data
                .into_iter()
                .find(|pd| pd.name == CLAIM_PROTOCOL)
        })
        .collect();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].data, claims[1].data);
}

/// §6.3's other half: a voucher **below** the watermark is a different,
/// stale voucher, refused `amount_not_advancing` -- a voucher's freshness is
/// its amount (ADR 0074 decision 3). Together with the test above this pins
/// the whole boundary.
#[tokio::test]
async fn a_voucher_below_the_watermark_is_refused_amount_not_advancing() {
    let book = ChannelBook::new();
    let (_, transport) = dialing(carriage(payee(), &book));

    let accepted = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(900)))
        .await;
    let stale = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(500)))
        .await;

    assert_eq!(accepted.ack, ClaimAckOutcome::Accepted);
    assert_eq!(
        stale.ack,
        ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
    );
    assert_eq!(book.watermark(&payer_channel()), Some(900));
}

// ─── §6.3: absence and malformation ───

/// A payee that answers the voucher-bearing frame but carries **no**
/// `claim-ack`. §6.3: not acknowledged -- never accepted, never rejected,
/// never inferred from the packet's own verdict.
#[tokio::test]
async fn a_response_carrying_no_ack_leaves_the_voucher_not_acknowledged() {
    let transport = transport(Arc::new(SilentPayee { ack: None }) as Arc<dyn PeerDialer>);

    let PeerForward { ack, .. } = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(500)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// §6.3: a malformed ack is likewise not acknowledged, and must not be
/// read as either verdict.
#[tokio::test]
async fn a_malformed_ack_leaves_the_voucher_not_acknowledged() {
    let transport = transport(Arc::new(SilentPayee {
        ack: Some(br#"{"result":"probably"}"#.to_vec()),
    }) as Arc<dyn PeerDialer>);

    let PeerForward { ack, .. } = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(500)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// A payee that answers every request with an empty RESPONSE, optionally
/// carrying `ack` bytes verbatim -- for the absence and malformation cases
/// a well-behaved `PeerSession` will not produce.
struct SilentPayee {
    ack: Option<Vec<u8>>,
}

#[async_trait]
impl PeerDialer for SilentPayee {
    async fn dial(&self, _peer_id: &str, _endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        let (to_peer, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(to_peer, Arc::clone(&outbound));
        let ack = self.ack.clone();
        tokio::spawn(async move {
            while let Some(bytes) = to_peer_rx.recv().await {
                let frame = decode_frame(&bytes).expect("our own encoder");
                let entries: Vec<ProtocolData> = ack
                    .iter()
                    .map(|data| ProtocolData {
                        name: connector_btp::CLAIM_ACK_PROTOCOL.to_string(),
                        content_type: CONTENT_TYPE_TEXT,
                        data: data.clone(),
                    })
                    .collect();
                let answer = encode_response(frame.request_id, &entries, &[]);
                let _ = outbound.resolve(decode_frame(&answer).expect("our own encoder"));
            }
        });
        Ok(handle)
    }
}

// ─── issue #1240: a peer that restarts under a live peering ───

/// **Issue #1240.** A payee restarts; the socket its peering was carried
/// on dies with it, and the dialing side is left holding the handle to a
/// corpse. The next packet must be **redialled and delivered**, not refused
/// `T01` on a session no dial was ever attempted for.
///
/// The assertion is both halves, because "reached the peer" alone would go
/// green on a peering carrying bytes for free: the peer answered, **and**
/// the voucher that rode the redial was accepted.
#[tokio::test]
async fn a_packet_after_the_far_side_restarts_is_redialled_rather_than_refused() {
    let book = ChannelBook::new();
    let (dialer, transport) = dialing(carriage(payee(), &book));

    let first = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(500)))
        .await;
    assert!(first.reached_peer, "the peering was carrying before this");
    assert_eq!(first.ack, ClaimAckOutcome::Accepted);
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 1);

    dialer.restart_the_far_side().await;

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), paid_with(payer_voucher(900)))
        .await;

    assert!(
        reached,
        "a restarted payee costs a redial, not a packet: {response:?}"
    );
    match response {
        PacketResponse::Reject(reject) => assert_eq!(
            reject.code.as_str(),
            "F02",
            "the answer is the payee's own, not this transport's T01"
        ),
        other => panic!("expected the payee's own reject, got {other:?}"),
    }
    assert_eq!(
        ack,
        ClaimAckOutcome::Accepted,
        "the voucher covering the packet reached the payee and was judged, \
         so the redial carries value and not merely bytes"
    );
    assert_eq!(
        dialer.dials.load(Ordering::SeqCst),
        2,
        "the dead session was replaced, and only once"
    );
}

// ─── §2.2: a peer that cannot be dialed ───

/// §2.2: whether the remote exposes what we dial is not locally
/// detectable, so it surfaces as an ordinary dial failure -- and a packet
/// routed there rejects **`T01`**, never `T00` and never a silent drop.
/// `reached` is false, so no fee of this hop's belongs on the reject that
/// goes back (ADR 0011).
#[tokio::test]
async fn a_peer_that_cannot_be_dialed_rejects_t01_and_was_never_reached() {
    let transport = transport(Arc::new(DeadDialer) as Arc<dyn PeerDialer>);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            PEER_ID,
            prepare("g.somewhere"),
            paid_with(payer_voucher(500)),
        )
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(reject.message.contains(PEER_ID));
        }
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

#[tokio::test]
async fn a_peer_id_this_connector_does_not_dial_rejects_t01() {
    let transport = transport(Arc::new(DeadDialer) as Arc<dyn PeerDialer>);

    let PeerForward {
        response,
        reached_peer: reached,
        ..
    } = transport
        .forward("nowhere", prepare("g.somewhere"), None)
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert!(!reached);
}

// ─── §1: role is decided by a bound signer's voucher ───

/// A session driver for the accept side alone: feed frames in, read the
/// answers out.
struct Accepting {
    frames: mpsc::Sender<Vec<u8>>,
    answers: mpsc::Receiver<Vec<u8>>,
    session: tokio::task::JoinHandle<SessionEnd>,
}

fn accepting(state: Arc<PeerCarriageState>) -> Accepting {
    let (frames, frames_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replies, answers) = mpsc::channel::<Vec<u8>>(32);
    let session = tokio::spawn(PeerSession::new(state, replies).run(frames_rx));
    Accepting {
        frames,
        answers,
        session,
    }
}

impl Accepting {
    async fn send(&self, frame: Vec<u8>) {
        self.frames.send(frame).await.expect("the session is live");
    }

    async fn answer(&mut self) -> BtpFrame {
        let bytes = self
            .answers
            .recv()
            .await
            .expect("the session answered the request");
        decode_frame(&bytes).expect("our own encoder")
    }
}

fn entry(name: &str, json: &str) -> ProtocolData {
    ProtocolData {
        name: name.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: json.as_bytes().to_vec(),
    }
}

/// A MESSAGE carrying `voucher` in the claim slot, on a PREPARE to nowhere.
fn voucher_frame(request_id: u32, voucher: &str) -> Vec<u8> {
    encode_message(
        request_id,
        &[entry(CLAIM_PROTOCOL, voucher)],
        &prepare("g.nowhere").encode(),
    )
}

/// **The named regression (§1.9).** `toon-sandbox` admitted an anonymous
/// BTP session with `btp_auth … success:true mode:"no-auth"` and then
/// treated it as a quasi-peer. Each frame below is classified `client` and
/// reaches **no peer handling whatsoever** -- testable, per §1.9, as: no
/// `claim-ack` was emitted, and nothing they carried moved a watermark
/// (proved by the book, and by a subsequent *genuine* peer voucher for a
/// single unit being accepted, which it could not be had any of these
/// advanced the channel).
#[tokio::test]
async fn the_named_regression_no_frame_becomes_a_peer_without_a_bound_verifying_voucher() {
    let book = ChannelBook::new();
    let state = carriage(payee(), &book);
    let fresh = now_unix() + 60;

    let cases: Vec<(&str, Vec<ProtocolData>, u64)> = vec![
        ("no evidence at all", vec![], ARRIVING_AMOUNT),
        (
            "a voucher on a channel the chain holds no record of",
            vec![entry(
                CLAIM_PROTOCOL,
                &voucher_on(&unopened_channel(), &payer_key(), 500),
            )],
            ARRIVING_AMOUNT,
        ),
        (
            "a voucher on the bound channel that does not recover to its signer",
            vec![entry(CLAIM_PROTOCOL, &forged_voucher(500))],
            ARRIVING_AMOUNT,
        ),
        (
            "a genuine voucher from a signer bound to no peering",
            vec![entry(
                CLAIM_PROTOCOL,
                &voucher_on(&stranger_channel(), &stranger_key(), 500),
            )],
            ARRIVING_AMOUNT,
        ),
        (
            "a genuine challenge on a PREPARE that moves value",
            vec![entry(
                PEER_CHALLENGE_PROTOCOL,
                &challenge_on(&payer_channel(), &payer_key(), fresh),
            )],
            ARRIVING_AMOUNT,
        ),
        (
            "a challenge that has expired",
            vec![entry(
                PEER_CHALLENGE_PROTOCOL,
                &challenge_on(&payer_channel(), &payer_key(), now_unix() - 1),
            )],
            0,
        ),
        (
            "a challenge signed by somebody else",
            vec![entry(
                PEER_CHALLENGE_PROTOCOL,
                &challenge_on(&payer_channel(), &stranger_key(), fresh),
            )],
            0,
        ),
    ];

    for (case, evidence, amount) in cases {
        let mut session = accepting(Arc::clone(&state));
        session
            .send(encode_message(
                1,
                &evidence,
                &prepare_of("g.nowhere", amount).encode(),
            ))
            .await;
        let answer = session.answer().await;

        // §1.6: not refused for the assertion alone -- refusing would make
        // the check an oracle for which signers are bound.
        assert_eq!(
            answer.frame_type, BTP_RESPONSE,
            "{case}: an asserted role is not refused on the wire (§1.6)"
        );
        assert!(
            ack::from_protocol_data(&answer.protocol_data).is_none(),
            "{case}: a client frame gets no claim-ack (§1.7)"
        );
        match decode_answer(&answer).map(|answer| answer.into_response()) {
            Some(PacketResponse::Reject(reject)) => assert_eq!(
                reject.message, "no peer route for this interaction",
                "{case}: answered as a client"
            ),
            other => panic!("{case}: expected the client-role F02, got {other:?}"),
        }
    }

    assert_eq!(
        book.watermark(&payer_channel()),
        None,
        "no client frame advanced the bound channel"
    );
    assert_eq!(book.watermark(&stranger_channel()), None);
    let mut peer = accepting(Arc::clone(&state));
    peer.send(voucher_frame(2, &payer_voucher(1))).await;
    let answer = peer.answer().await;
    assert_eq!(
        ack::from_protocol_data(&answer.protocol_data),
        Some(ClaimAckOutcome::Accepted),
        "no client frame had advanced this channel's watermark"
    );
}

/// **#1384: a `toon-channel` claim is refused by name.** ADR 0075 retired
/// the scheme: a frame whose claim slot holds one -- no `scheme`, or
/// `scheme: "toon-channel"` -- is answered with an ERROR frame naming the
/// retirement, before any role is decided, and nothing is judged.
#[tokio::test]
async fn a_toon_channel_claim_is_refused_by_name() {
    let book = ChannelBook::new();
    let state = carriage(payee(), &book);
    // The retired claim exactly as a pre-ADR 0075 peer rendered it: no
    // `scheme`, a nonce and an EIP-712 balance proof. Refused before
    // anything about it is read, so its signature does not matter.
    let json = serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "messageId": "message-1",
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": "peer",
        "channelId": format!("0x{}", "07".repeat(32)),
        "nonce": 1,
        "transferredAmount": "500",
        "lockedAmount": "0",
        "locksRoot": format!("0x{}", "00".repeat(32)),
        "signature": format!("0x{}", "11".repeat(65)),
        "signerAddress": format!("0x{}", "44".repeat(20)),
    })
    .to_string();
    let explicit = json.replace(
        r#""blockchain":"evm""#,
        r#""blockchain":"evm","scheme":"toon-channel""#,
    );
    assert_ne!(explicit, json, "the scheme really was written");

    for (request_id, claim) in [(1, &json), (2, &explicit)] {
        let mut session = accepting(Arc::clone(&state));
        session.send(voucher_frame(request_id, claim)).await;
        // The raw frame: an ERROR's data is what names the refusal, and the
        // decoder keeps only its type and request id.
        let bytes = session.answers.recv().await.expect("answered");
        let answer = decode_frame(&bytes).expect("our own encoder");
        assert_eq!(answer.frame_type, BTP_ERROR, "{claim}");
        let reason = String::from_utf8_lossy(&bytes);
        assert!(reason.contains("toon-channel"), "{reason}");
        assert!(reason.contains("ADR 0075"), "{reason}");
    }
    assert_eq!(book.watermark(&payer_channel()), None);
}

/// §1.4: a receiver **ignores** an arriving `auth` entry rather than
/// refusing it (ADR 0060), so the two ends of a peering may be upgraded in
/// either order without the peering going dark mid-flight. The frame is
/// answered the way any evidence-less MESSAGE is -- an empty RESPONSE --
/// and nothing about the entry is evaluated.
#[tokio::test]
async fn an_arriving_auth_entry_is_ignored_rather_than_refused() {
    let book = ChannelBook::new();
    let mut session = accepting(carriage(payee(), &book));

    session
        .send(encode_message(
            1,
            &[entry(
                AUTH_PROTOCOL,
                r#"{"peerId":"peer-b","secret":"whatever"}"#,
            )],
            &[],
        ))
        .await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert!(answer.protocol_data.is_empty());
    // And it decided nothing: the very next voucher still stands on itself,
    // and a forged one is still a client frame.
    session.send(voucher_frame(2, &forged_voucher(500))).await;
    let answer = session.answer().await;
    assert!(ack::from_protocol_data(&answer.protocol_data).is_none());
}

/// §1.5: more than one claim entry on one frame is **refused, not
/// resolved** -- never the first, never the last, never a concatenation.
/// This is the smuggling defence, and its absence is how "which voucher did
/// we verify?" becomes unanswerable.
#[tokio::test]
async fn two_claim_entries_on_one_frame_are_refused_rather_than_resolved() {
    let book = ChannelBook::new();
    let mut session = accepting(carriage(payee(), &book));
    let first = payer_voucher(500);
    let second = payer_voucher(600);

    session
        .send(encode_message(
            1,
            &[
                entry(CLAIM_PROTOCOL, &first),
                entry(CLAIM_PROTOCOL, &second),
            ],
            &[],
        ))
        .await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_ERROR);
    assert_eq!(book.watermark(&payer_channel()), None);
    // Neither voucher was adopted: the first alone still advances.
    session.send(voucher_frame(2, &first)).await;
    let answer = session.answer().await;
    assert_eq!(
        ack::from_protocol_data(&answer.protocol_data),
        Some(ClaimAckOutcome::Accepted)
    );
}

/// §1.5: role is a property of the **frame**, not of the session. A frame
/// whose voucher does not verify is a client frame, and a verifying frame
/// after it does not reach back and reclassify it -- nor does a later frame
/// inherit anything from the one before.
#[tokio::test]
async fn a_frame_whose_voucher_does_not_verify_stays_a_client_frame() {
    let book = ChannelBook::new();
    let mut session = accepting(carriage(payee(), &book));

    session.send(voucher_frame(1, &forged_voucher(500))).await;
    let before = session.answer().await;
    assert!(
        ack::from_protocol_data(&before.protocol_data).is_none(),
        "a frame whose voucher does not verify is a client frame"
    );

    // The forged voucher was never judged, so the channel is still fresh --
    // which is exactly what "not retroactively reclassified" means here.
    session.send(voucher_frame(2, &payer_voucher(500))).await;
    let after = session.answer().await;
    assert_eq!(
        ack::from_protocol_data(&after.protocol_data),
        Some(ClaimAckOutcome::Accepted)
    );

    // And the peer frame does not make the socket a peer socket: the very
    // next frame carrying nothing is a client frame again.
    session
        .send(encode_message(3, &[], &prepare("g.nowhere").encode()))
        .await;
    let evidence_less = session.answer().await;
    assert!(ack::from_protocol_data(&evidence_less.protocol_data).is_none());
}

/// §1.10's bounded escape hatch: on a **dedicated peer listener with
/// mandatory authentication** a frame that does not prove the peer role is
/// refused outright rather than downgraded -- safe only because such a
/// listener serves no clients. Role is still decided by the voucher; the
/// listener never becomes the decider.
#[tokio::test]
async fn a_dedicated_peer_listener_refuses_rather_than_downgrades() {
    let book = ChannelBook::new();
    let state = carriage_with(
        payee(),
        &book,
        Arc::new(ClaimEnforcementPolicy::default()),
        PeerAcceptPolicy {
            mandatory_auth: true,
            ..PeerAcceptPolicy::default()
        },
    );
    let mut session = accepting(state);

    session.send(voucher_frame(1, &forged_voucher(500))).await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_ERROR);
    assert_eq!(
        session.session.await.expect("the session task"),
        SessionEnd::Refused
    );
}

// ─── issue #880 (owner decision #868): every peer PREPARE to a priced
// terminated route carries a covering voucher, or is refused with the client
// edge's own x402 greeting ───

/// An arrival carrying no evidence reaches **no peer handling at all**
/// (§1.2): it is a client frame, and the greeting a client gets is the
/// client edge's own. What this layer holds is that the app never sees it,
/// and nothing is acknowledged.
#[tokio::test]
async fn a_claimless_arrival_at_a_priced_route_reaches_no_peer_handling() {
    let book = ChannelBook::new();
    let app_client = serving_app(&priced_route(), b"free service");
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));
    let (prepare, _) = sealed_prepare(25);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport.forward(PEER_ID, prepare, None).await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(
            reject.code.as_str(),
            "F02",
            "a client-role packet reaches no peer route"
        ),
        other => panic!("expected an F02 reject, got {other:?}"),
    }
    assert!(payment_required.is_none());
    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "no claim-ack to a client (§1.7)"
    );
    assert!(app_client.deliveries().is_empty());
}

/// A voucher rides the PREPARE, but its advance over the watermark falls
/// short of the route's price: refused `F06` with the greeting (issue
/// #880's second acceptance case). The voucher's own validity is unaffected
/// by this gate -- it is still acknowledged (§6.2).
#[tokio::test]
async fn a_voucher_that_does_not_cover_the_routes_price_is_refused() {
    let book = ChannelBook::new();
    let app_client = serving_app(&priced_route(), b"free service");
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));
    let (prepare, _) = sealed_prepare(25);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        // Advances only 10; the price is 25.
        .forward(PEER_ID, prepare, paid_with(payer_voucher(10)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?}"),
    }
    assert!(payment_required.is_some());
    assert!(app_client.deliveries().is_empty());
}

/// The boundary this gate exists to leave open: a voucher whose advance
/// exactly meets the route's price is admitted -- delivered to the app, no
/// greeting.
#[tokio::test]
async fn a_covering_voucher_is_admitted() {
    let book = ChannelBook::new();
    let response_body = b"served".to_vec();
    let app_client = serving_app(&priced_route(), &response_body);
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));
    let (prepare, shared_secret) = sealed_prepare(25);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, prepare, paid_with(payer_voucher(25)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    assert_eq!(opened_body(response, &shared_secret), response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

/// PR #913 review finding, on vouchers: a voucher on the bound channel that
/// its signer never signed still *decodes* and can declare any amount it
/// likes. Its signature does not recover to the channel's voucher signer,
/// so the frame is a **client** frame (ADR 0075 decision 5) and there is no
/// `claim-ack` to carry `signature_invalid` back (§1.7). The declared amount
/// buys nothing, and the app never sees the packet.
#[tokio::test]
async fn a_forged_voucher_declaring_a_large_amount_does_not_buy_coverage() {
    let book = ChannelBook::new();
    let app_client = serving_app(&priced_route(), b"free service");
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));
    let (prepare, _) = sealed_prepare(25);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, prepare, paid_with(forged_voucher(1_000_000)))
        .await;

    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "a voucher that does not verify makes the frame a client's, and a \
         client interaction never carries a claim-ack (§1.7)"
    );
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected an F02 reject, got {other:?}"),
    }
    assert!(
        payment_required.is_none(),
        "the greeting a client gets is the client edge's, not this gate's"
    );
    assert!(
        app_client.deliveries().is_empty(),
        "a forged voucher must never reach the app"
    );
    assert_eq!(book.watermark(&payer_channel()), None);
}

/// PR #913 review finding, second case, on vouchers: a genuine voucher
/// resent byte-identically is **accepted** again (§6.3's re-ack), but it
/// advances nothing -- so it must never buy a second packet. Coverage is the
/// advance past the watermark as it stood before the voucher, and a resend's
/// advance is zero, on every retransmission.
#[tokio::test]
async fn a_resent_voucher_never_buys_coverage_twice() {
    let book = ChannelBook::new();
    let response_body = b"served once".to_vec();
    let app_client = serving_app(&priced_route(), &response_body);
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));
    let voucher = payer_voucher(25);

    let (first, shared_secret) = sealed_prepare(25);
    let paid = transport
        .forward(PEER_ID, first, paid_with(voucher.clone()))
        .await;
    assert_eq!(paid.ack, ClaimAckOutcome::Accepted);
    assert_eq!(opened_body(paid.response, &shared_secret), response_body);

    for attempt in 0..2 {
        let (again, _) = sealed_prepare(25);
        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport
            .forward(PEER_ID, again, paid_with(voucher.clone()))
            .await;

        assert_eq!(
            ack,
            ClaimAckOutcome::Accepted,
            "attempt {attempt}: the resend is re-acked"
        );
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F06", "attempt {attempt}");
            }
            other => panic!("expected an F06 reject on attempt {attempt}, got {other:?}"),
        }
        assert!(payment_required.is_some(), "attempt {attempt}");
    }
    assert_eq!(
        app_client.deliveries().len(),
        1,
        "the resent voucher bought exactly the one packet it paid for"
    );
    assert_eq!(book.watermark(&payer_channel()), Some(25));
}

// ─── issue #1104: coverage is the voucher's advance past the **durable**
// watermark, so a payee restart never credits a voucher with its whole
// cumulative amount ───

/// Carries the payer's channel to cumulative 50 000 on a payee judging
/// through `book`, then drops that whole node -- the state a restarted payee
/// comes back to.
async fn book_at_fifty_thousand(book: &Arc<ChannelBook>) {
    let (_, transport) = dialing(carriage(priced_payee(Arc::new(FakeAppClient::new())), book));
    let PeerForward { ack, .. } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere"),
            paid_with(payer_voucher(50_000)),
        )
        .await;
    assert_eq!(
        ack,
        ClaimAckOutcome::Accepted,
        "the pre-restart voucher is what the book records"
    );
}

/// The bug: after a restart the payee's durable watermark is at 50 000,
/// while anything the carriage kept in memory is gone. A voucher at 50 001
/// is one unit of genuinely new money and cannot buy a packet priced at 25.
/// Measured against an empty per-process record it would be credited with
/// all 50 001 and buy it (issue #1104).
#[tokio::test]
async fn a_restart_does_not_credit_a_voucher_with_the_amount_it_already_paid() {
    let book = ChannelBook::new();
    book_at_fifty_thousand(&book).await;

    // The restart: a new node and a new carriage over the same book.
    let app_client = serving_app(&priced_route(), b"free service");
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));

    let (prepare, _) = sealed_prepare(25);
    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        // Advances 1; the price is 25.
        .forward(PEER_ID, prepare, paid_with(payer_voucher(50_001)))
        .await;

    assert_eq!(
        ack,
        ClaimAckOutcome::Accepted,
        "the voucher itself is good -- it advances the durable watermark, \
         which is why the book's verdict cannot catch this on its own"
    );
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?} -- the app served this for free"),
    }
    assert!(payment_required.is_some(), "the x402 greeting rides it");
    assert!(
        app_client.deliveries().is_empty(),
        "one unit of new money must not buy a packet priced at 25"
    );
}

/// The other side of the same boundary: after the same restart, a voucher
/// that genuinely advances the durable watermark by the price is admitted
/// and reaches the app. The fix must not make a restart refuse real money.
#[tokio::test]
async fn a_restart_still_admits_a_voucher_that_genuinely_advances_by_the_price() {
    let book = ChannelBook::new();
    book_at_fifty_thousand(&book).await;

    let response_body = b"served after the restart".to_vec();
    let app_client = serving_app(&priced_route(), &response_body);
    let (_, transport) = dialing(carriage(priced_payee(Arc::clone(&app_client)), &book));

    let (prepare, shared_secret) = sealed_prepare(25);
    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        // Advances exactly the price.
        .forward(PEER_ID, prepare, paid_with(payer_voucher(50_025)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    assert_eq!(opened_body(response, &shared_secret), response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

// ─── ADR 0042 item 3: a forwarded arrival must cover its own `amount`,
// behind a per-peer knob that defaults to observing ───

/// **The default is still `observe`, and this is what that means.** A
/// peering that configures nothing carries an **under-covered** forwarded
/// arrival -- admitted, logged, and actually forwarded to the next hop. What
/// it guards now is the migration default ADR 0042 item 3 keeps for a
/// counterparty on an older binary. The fixture must ALSO pay its own next
/// hop (issue #1145): admitting an arrival for free says nothing about what
/// this node then sends, and what it sends is covered unconditionally.
///
/// The arrival carries a voucher that verifies but advances too little,
/// rather than none at all: an evidence-less one is a client's and never
/// reaches this gate.
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_admitted_by_default() {
    let book = ChannelBook::new();
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
    // The default policy: no entry for this peering at all, exactly as an
    // unconfigured `[[peers]]` row resolves.
    let (_, transport) = dialing(carriage(connector, &book));
    let (sealed, shared_secret) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(
            PEER_ID,
            sealed,
            paid_with(payer_voucher(ARRIVING_AMOUNT - 1)),
        )
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    assert_eq!(
        opened_body(response, &shared_secret),
        b"delivered by the next hop"
    );
    assert_eq!(
        next_hop_app.deliveries().len(),
        1,
        "the packet was really carried, not merely not refused"
    );
}

/// The same arrival on a peering an operator has flipped: refused `F06`
/// with the x402 greeting, quoting the packet's own `amount` -- and never
/// carried, so the next hop does no work this connector was not paid for.
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_refused_once_this_peering_enforces() {
    let book = ChannelBook::new();
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
    let (_, transport) = dialing(carriage_with(
        connector,
        &book,
        forwarded_enforcing(),
        PeerAcceptPolicy::default(),
    ));
    let (sealed, _) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let PeerForward {
        response,
        payment_required,
        ..
    } = transport
        .forward(
            PEER_ID,
            sealed,
            paid_with(payer_voucher(ARRIVING_AMOUNT - 1)),
        )
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?}"),
    }
    let terms = payment_required.expect("the x402 greeting rode the reject");
    assert_eq!(
        terms.price(),
        Some(ARRIVING_AMOUNT),
        "a forwarded arrival is quoted the packet's own amount, not the route's price"
    );
    assert_eq!(terms.ilp_address(), Some(FORWARDED_DESTINATION));
    assert!(
        next_hop_app.deliveries().is_empty(),
        "a refused arrival is never carried"
    );
}

/// A voucher advancing the full arriving `amount` is admitted under
/// **either** setting: enforcing changes what an uncovered packet gets,
/// never what a covered one gets.
#[tokio::test]
async fn a_voucher_covering_the_arriving_amount_is_admitted_under_either_setting() {
    for enforcement in [
        Arc::new(ClaimEnforcementPolicy::default()),
        forwarded_enforcing(),
    ] {
        let book = ChannelBook::new();
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
        let (_, transport) = dialing(carriage_with(
            connector,
            &book,
            enforcement,
            PeerAcceptPolicy::default(),
        ));
        let (sealed, shared_secret) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );

        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport
            .forward(PEER_ID, sealed, paid_with(payer_voucher(ARRIVING_AMOUNT)))
            .await;

        assert_eq!(ack, ClaimAckOutcome::Accepted);
        assert!(
            payment_required.is_none(),
            "an admitted packet carries no greeting"
        );
        assert_eq!(
            opened_body(response, &shared_secret),
            b"delivered by the next hop"
        );
        assert_eq!(next_hop_app.deliveries().len(), 1);
    }
}

/// **Which figure must be covered**, stated as the three near misses: not
/// the forwarded route's client-edge `price` (ADR 0028 says that is a fact
/// about this node's *client* edge), not the post-fee amount this hop passes
/// on (that is what this hop covers to the next hop, and the difference it
/// keeps is its fee, ADR 0010), and not one unit short. Only the arriving
/// `amount` covers an arriving packet.
#[tokio::test]
async fn a_voucher_advancing_less_than_the_arriving_amount_never_covers_it() {
    for advance in [
        FORWARD_ROUTE_PRICE,
        ARRIVING_AMOUNT - FORWARD_FEE,
        ARRIVING_AMOUNT - 1,
    ] {
        let book = ChannelBook::new();
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
        let (_, transport) = dialing(carriage_with(
            connector,
            &book,
            forwarded_enforcing(),
            PeerAcceptPolicy::default(),
        ));
        let (sealed, _) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );

        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport
            .forward(PEER_ID, sealed, paid_with(payer_voucher(advance)))
            .await;

        // The voucher is perfectly valid and is still acknowledged: the two
        // verdicts stay independent (§6.2).
        assert_eq!(ack, ClaimAckOutcome::Accepted, "advance {advance}");
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F06", "advance {advance}");
            }
            other => panic!("expected an F06 reject for advance {advance}, got {other:?}"),
        }
        assert!(payment_required.is_some(), "advance {advance}");
        assert!(
            next_hop_app.deliveries().is_empty(),
            "advance {advance} was never carried"
        );
    }
}

/// ADR 0029's rule is **untouched** by ADR 0042, and since issue #1077 it
/// has no escape hatch at all: an arrival at a priced termination that does
/// not cover the route's price is refused under **every** setting a peering
/// can carry. The forwarded knob is the only one left, and neither of its
/// values admits one.
#[tokio::test]
async fn no_peering_setting_admits_an_uncovered_arrival_at_a_priced_termination() {
    for forwarded in [
        connector_config::ForwardedClaimEnforcement::Observe,
        connector_config::ForwardedClaimEnforcement::Enforce,
    ] {
        let book = ChannelBook::new();
        let app_client = serving_app(&priced_route(), b"terminated here");
        let (_, transport) = dialing(carriage_with(
            priced_payee(Arc::clone(&app_client)),
            &book,
            Arc::new(ClaimEnforcementPolicy::of(vec![(PEER_ID, forwarded)])),
            PeerAcceptPolicy::default(),
        ));
        let (sealed, _) = sealed_prepare(25);

        let PeerForward {
            response,
            payment_required,
            ..
        } = transport
            // The price is 25.
            .forward(PEER_ID, sealed, paid_with(payer_voucher(1)))
            .await;

        assert!(
            matches!(&response, PacketResponse::Reject(reject) if reject.code.as_str() == "F06"),
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(
            payment_required.is_some(),
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(app_client.deliveries().is_empty());
    }
}

// ─── §6.1: what a frame that proves nothing is acknowledged with ───

/// A genuine voucher from a signer **bound to no peering** -- a real channel,
/// a real signature -- is **not acknowledged at all**: the frame is a
/// client's (ADR 0075 decision 5), and §1.7 forbids a `claim-ack` on a
/// client interaction. `signature_invalid` and `unknown_channel` are
/// therefore verdicts a peer frame cannot reach, structurally; their JSON is
/// still pinned in `ack.rs`'s own round trip because the client edge still
/// reaches them.
#[tokio::test]
async fn a_voucher_from_an_unbound_signer_is_not_acknowledged_at_all() {
    let book = ChannelBook::new();
    let (_, transport) = dialing(carriage(payee(), &book));

    let PeerForward { ack, .. } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere"),
            paid_with(voucher_on(&stranger_channel(), &stranger_key(), 500)),
        )
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert_eq!(book.watermark(&stranger_channel()), None);
}

/// A voucher on a channel the receiving half cannot resolve is not
/// acknowledged either, even signed by the bound key: there is no channel
/// whose signer the chain records, so nothing to resolve a binding from.
#[tokio::test]
async fn a_voucher_on_a_channel_the_chain_does_not_hold_is_not_acknowledged_at_all() {
    let book = ChannelBook::new();
    let (_, transport) = dialing(carriage(payee(), &book));

    let PeerForward { ack, .. } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere"),
            paid_with(voucher_on(&unopened_channel(), &payer_key(), 500)),
        )
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

// ─── §7.1: ordering ───

/// §7.1: vouchers on one session are judged **strictly sequentially, in
/// arrival order**, so vouchers sent in order on one socket cannot race each
/// other into `amount_not_advancing`. Sixteen advancing vouchers are sent
/// back to back without waiting for any answer; every one is accepted.
#[tokio::test]
async fn vouchers_sent_in_order_on_one_session_never_race_each_other() {
    let book = ChannelBook::new();
    let mut session = accepting(carriage(payee(), &book));

    for step in 1..=16u64 {
        session
            .send(encode_message(
                step as u32 + 1,
                &[entry(CLAIM_PROTOCOL, &payer_voucher(step * 100))],
                &[],
            ))
            .await;
    }

    for _ in 1..=16 {
        let answer = session.answer().await;
        assert_eq!(
            ack::from_protocol_data(&answer.protocol_data),
            Some(ClaimAckOutcome::Accepted)
        );
    }
    assert_eq!(book.watermark(&payer_channel()), Some(1_600));
}

// ─── §2.3: BTP is symmetric once established ───

/// §2.3: either side may originate on the one session -- the whole of the
/// difference between the two carriages. The accepting side's handle
/// originates a MESSAGE and the dialing side answers it.
#[tokio::test]
async fn the_accepting_side_can_originate_on_the_session_it_accepted() {
    let book = ChannelBook::new();
    let state = carriage(payee(), &book);
    let (frames, frames_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replies, mut answers) = mpsc::channel::<Vec<u8>>(32);
    let session = PeerSession::new(state, replies);
    let handle = session.handle();
    let driver = tokio::spawn(session.run(frames_rx));

    let counterparty = tokio::spawn(async move {
        let bytes = answers.recv().await.expect("the originated MESSAGE");
        let frame = decode_frame(&bytes).expect("our own encoder");
        assert_eq!(frame.frame_type, connector_btp::BTP_MESSAGE);
        frames
            .send(encode_response(frame.request_id, &[], b"answered"))
            .await
            .expect("the session is live");
    });

    let answer = handle
        .send_message(&[], &[])
        .await
        .expect("the counterparty answered");

    assert_eq!(answer.ilp_packet, b"answered".to_vec());
    counterparty.await.expect("the counterparty task");
    drop(driver);
}

// ─── the port's own contract (spec I5) ───

/// Establishing that nothing above the port can tell which carriage
/// delivered a packet: the `PeerTransport` contract statements, restated
/// against the BTP carriage. A registered peer's own answer comes back
/// unchanged, and an unregistered peer id produces `T01` with
/// `reached == false`.
///
/// The first forward carries a voucher, because that is what makes it a
/// peer frame at all -- without one the answer would be the client-role
/// `F02` and the peer's *own* routing verdict would never be reached.
#[tokio::test]
async fn the_btp_carriage_upholds_the_peer_transport_contract() {
    let book = ChannelBook::new();
    let (_, transport) = dialing(carriage(payee(), &book));

    let PeerForward {
        response,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere-on-the-peer"),
            paid_with(payer_voucher(500)),
        )
        .await;
    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "F02");
            assert!(reject.message.contains("g.nowhere-on-the-peer"));
        }
        other => panic!("expected the peer's own reject, got {other:?}"),
    }
    assert!(reached);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward("unregistered", prepare("g.anything"), None)
        .await;
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

/// A dialed session is established once and reused: eight concurrent
/// forwards to one peer do not open eight sessions.
#[tokio::test]
async fn concurrent_forwards_share_one_dialed_session() {
    let book = ChannelBook::new();
    let (dialer, transport) = dialing(carriage(payee(), &book));
    let transport = Arc::new(transport);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let transport = Arc::clone(&transport);
        handles.push(tokio::spawn(async move {
            transport.forward(PEER_ID, prepare("g.nowhere"), None).await
        }));
    }
    for handle in handles {
        let PeerForward {
            response,
            reached_peer: reached,
            ..
        } = handle.await.expect("task");
        assert!(matches!(response, PacketResponse::Reject(_)));
        assert!(reached);
    }

    assert_eq!(
        dialer.dials.load(Ordering::SeqCst),
        1,
        "eight concurrent forwards opened one socket"
    );
}
