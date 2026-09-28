//! The ILP-over-HTTP peer carriage end to end
//! (`docs/protocol/peer-carriage-spec.md`, issue #728): an
//! [`HttpPeerTransport`] **dials**, a [`PeerHttpState`] **accepts**, and the
//! requests and responses between them are the ones §3's table names.
//!
//! # What proves a peering here (ADR 0075, #1380)
//!
//! A peering is two one-way x402 `batch-settlement` channels (ADR 0075), and
//! what proves the peer role on a request is **a voucher on the inbound
//! one** -- a channel whose voucher signer, as the receiving half reads it
//! off the chain, this node has bound to that peering -- or, on a PREPARE
//! that moves no value, a peer-role challenge signed by that same signer
//! (decision 5). Nothing else does. In particular a `toon-channel` claim,
//! however genuinely signed and by whatever key, never decides the role any
//! more (#1380): a request carrying one is a client's, and gets no
//! `Toon-Claim-Ack`. The FLUSH, the `Toon-Flush-Requested` prompt, nonce
//! watermarks and a Solana claim's `programId` went with it -- a voucher
//! rides the PREPARE it covers, and its watermark is one cumulative amount.
//!
//! # What is real here, and the one fake
//!
//! The two sides are joined by an in-process client standing in for the
//! socket and *only* for the socket: every header is the one the shared name
//! table declares, every role decision is `role_gate::decide_frame`'s over a
//! real `Connector`'s voucher-signer bindings, every coverage decision is
//! `price_gate`'s, and the payer reaches the payee only through the
//! `PeerTransport` port. Every voucher and challenge is genuinely signed
//! (EIP-712, secp256k1) and genuinely verified.
//!
//! The receiving half -- the thing that resolves a channel on chain and holds
//! its watermark -- is [`VoucherBook`], a **fake** upholding the
//! `VoucherEvidence` port's contract (ADR 0007): it knows its channels the
//! way the chain would, verifies every signature for real, and holds one
//! watermark per channel by the port's rules. The real one is
//! `connector-client-edge`'s claim gate, which needs a chain; its own suite
//! holds it to the same rules. What is not exercised is TLS and the socket.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_btp::{
    ACCUMULATED_COST_HEADER, CLAIM_ACK_HEADER, CLAIM_HEADER, PAYMENT_REQUIRED_HEADER,
    PEER_CHALLENGE_HEADER,
};
use connector_config::StaticRoute;
use connector_domain::client_claim::{ClientClaim, EvmVoucherChannelConfig};
use connector_domain::x402::parse_greeting;
use connector_domain::{PacketResponse, Prepare};
use connector_peer_btp::challenge_json::{self, PeerRoleChallenge};
use connector_peer_btp::role_gate::PeerVoucherVerdict;
use connector_peer_btp::{ClaimEnforcementPolicy, VoucherCheck, VoucherEvidence};
use connector_peer_http::accept::{PeerHttpPolicy, PeerHttpState};
use connector_peer_http::dial::{HttpDialError, PeerHttpClient, PeerRelation};
use connector_peer_http::headers::{Headers, PeerRequest, PeerResponse};
use connector_peer_http::{HttpPeerTransport, NAT_NOTE};
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
use url::Url;

// ─── keys, channels and vouchers ───

const CHAIN_ID: u64 = 84_532;
const PEER_ID: &str = "peer-b";

/// The x402 `batch-settlement` domain every voucher here is signed under.
fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(CHAIN_ID)
}

/// The peer's EVM settlement key: its inbound channel's `payerAuthorizer`
/// (ADR 0075 decision 3), and the key this node binds to [`PEER_ID`].
const PEER_SECRET: [u8; 32] = [0x0a; 32];

fn peer_key() -> SecretKey {
    SecretKey::parse(&PEER_SECRET).expect("valid secret")
}

/// Somebody else, with a channel of their own toward this node that this
/// node admits -- a client, as far as any binding is concerned.
fn stranger_key() -> SecretKey {
    SecretKey::parse(&[0x0b; 32]).expect("valid secret")
}

/// A key whose channel this node has no record of at all.
fn unknown_key() -> SecretKey {
    SecretKey::parse(&[0x0c; 32]).expect("valid secret")
}

fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

/// `owner`'s channel toward this node: `payer == payerAuthorizer == owner`.
fn config_of(owner: &SecretKey) -> BatchChannelConfig {
    let payer = address_of(owner);
    BatchChannelConfig {
        payer,
        payer_authorizer: payer,
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [0x66; 32],
    }
}

fn channel_of(owner: &SecretKey) -> [u8; 32] {
    evm_batch_channel_id(&domain(), &config_of(owner))
}

fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> [u8; 65] {
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(digest), secret);
    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&signature.serialize());
    bytes[64] = recovery.serialize() + 27;
    bytes
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `0x`-prefixed hex of exactly `N` bytes, or `None`.
fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.strip_prefix("0x")?;
    if digits.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(digits.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn config_json(config: &BatchChannelConfig) -> serde_json::Value {
    serde_json::json!({
        "payer": hex(&config.payer),
        "payerAuthorizer": hex(&config.payer_authorizer),
        "receiver": hex(&config.receiver),
        "receiverAuthorizer": hex(&config.receiver_authorizer),
        "token": hex(&config.token),
        "withdrawDelay": config.withdraw_delay,
        "salt": hex(&config.salt),
    })
}

/// A voucher on `owner`'s channel for cumulative `amount`, carrying
/// `signature` -- the x402 `batch-settlement` JSON a peer puts in the claim
/// header, with the channel's config presented beside it.
fn evm_voucher(owner: &SecretKey, amount: u64, signature: &[u8; 65]) -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": "peer",
        "channelId": hex(&channel_of(owner)),
        "maxClaimableAmount": amount.to_string(),
        "signature": hex(signature),
        "channelConfig": config_json(&config_of(owner)),
    })
    .to_string()
}

/// A voucher on `owner`'s channel, signed by `signer`: genuine when they are
/// the same key, a forgery when they are not.
fn voucher_signed_by(owner: &SecretKey, signer: &SecretKey, amount: u64) -> String {
    let digest = evm_voucher_digest(&domain(), &channel_of(owner), u128::from(amount));
    evm_voucher(owner, amount, &sign_evm(signer, &digest))
}

/// `owner`'s genuine voucher on its own channel.
fn signed_voucher(owner: &SecretKey, amount: u64) -> String {
    voucher_signed_by(owner, owner, amount)
}

/// The peer's genuine voucher on its inbound channel.
fn peer_voucher(amount: u64) -> String {
    signed_voucher(&peer_key(), amount)
}

/// A peer-role challenge on `owner`'s channel, signed by `signer`, lapsing
/// at `expires` (ADR 0075 decision 5).
fn evm_challenge(owner: &SecretKey, signer: &SecretKey, expires: u64) -> String {
    let channel_id = channel_of(owner);
    let digest = evm_voucher_claim_state_challenge_digest(&domain(), &channel_id, expires);
    challenge_json::encode(&PeerRoleChallenge::Evm {
        channel_id,
        expires,
        signature: sign_evm(signer, &digest),
        channel_config: Some(
            connector_domain::client_claim::parse_evm_channel_config(&config_json(&config_of(
                owner,
            )))
            .expect("a well-formed config"),
        ),
    })
}

/// [`clock`]'s reading in unix seconds, which is what a challenge's
/// `expires` is judged against.
const CLOCK_UNIX: u64 = 1_893_456_000;

/// A challenge `expires` comfortably inside the role gate's window.
fn fresh_expiry() -> u64 {
    CLOCK_UNIX + 60
}

// ─── the receiving half: a fake upholding `VoucherEvidence` ───

/// One channel the receiving half has admitted, as the chain records it.
struct AdmittedChannel {
    config: BatchChannelConfig,
    max_cumulative: u64,
}

/// What the book has accepted on one channel: the watermark, and the
/// signature that put it there -- so a byte-identical resend can be told
/// from a different voucher at the same amount.
#[derive(Clone, Copy)]
struct Accepted {
    amount: u64,
    signature: [u8; 65],
}

/// **The receiving half, as a fake upholding the `VoucherEvidence` port's
/// contract** (ADR 0007) -- not a stub with expectations. Nothing here
/// asserts what it was asked; it answers the way the port's contract says
/// the real receiving half (`connector-client-edge`'s claim gate) does:
///
/// * a channel is **found, never believed**: a presented `channelConfig`
///   must re-hash to the channel id it arrived beside (ADR 0074 decision
///   2), and the id must be one this book admitted -- otherwise
///   [`VoucherCheck::Unresolved`];
/// * the signer is the channel's `payerAuthorizer` **as admitted**, never
///   one the voucher declares, and every EIP-712 signature -- voucher and
///   challenge alike -- is genuinely recovered against it;
/// * [`VoucherEvidence::judge_peer_voucher`] holds **one watermark per
///   channel**: an amount above it is accepted and advances it; a
///   byte-identical resend at it is accepted and advances nothing; anything
///   else -- below it, or at it under different bytes, or past the
///   channel's collateral -- is refused `amount_not_advancing`. `prior` is
///   always the watermark before.
///
/// Shared by [`Arc`], so a node "restarted" over the same book is a node
/// whose durable watermarks survived -- the property issue #1104 needs.
struct VoucherBook {
    channels: Vec<AdmittedChannel>,
    watermarks: Mutex<HashMap<[u8; 32], Accepted>>,
}

impl VoucherBook {
    /// The peer's channel and a stranger's, each with ample collateral.
    fn new() -> Arc<VoucherBook> {
        Arc::new(VoucherBook {
            channels: [peer_key(), stranger_key()]
                .iter()
                .map(|owner| AdmittedChannel {
                    config: config_of(owner),
                    max_cumulative: 10_000_000,
                })
                .collect(),
            watermarks: Mutex::new(HashMap::new()),
        })
    }

    /// Where `channel` stands, if anything was ever accepted on it.
    fn watermark(&self, channel: &[u8; 32]) -> Option<u64> {
        self.watermarks
            .lock()
            .expect("watermark lock")
            .get(channel)
            .map(|accepted| accepted.amount)
    }

    /// The admitted channel `channel_id` names. A presented config that does
    /// not hash to that id resolves nothing.
    fn resolve(
        &self,
        channel_id: &[u8; 32],
        presented: Option<&EvmVoucherChannelConfig>,
    ) -> Option<&AdmittedChannel> {
        if let Some(presented) = presented {
            let config = batch_config(presented)?;
            if evm_batch_channel_id(&domain(), &config) != *channel_id {
                return None;
            }
        }
        self.channels
            .iter()
            .find(|channel| evm_batch_channel_id(&domain(), &channel.config) == *channel_id)
    }

    /// The verdict on a voucher's signature, and -- when it verified -- the
    /// channel it is on and the signature that verified.
    fn check(&self, voucher: &ClientClaim) -> (VoucherCheck, Option<VerifiedVoucher>) {
        let ClientClaim::EvmVoucher(voucher) = voucher else {
            return (VoucherCheck::Unresolved, None);
        };
        let Some(channel_id) = unhex::<32>(&voucher.channel_id) else {
            return (VoucherCheck::Unresolved, None);
        };
        let Some(channel) = self.resolve(&channel_id, voucher.channel_config.as_ref()) else {
            return (VoucherCheck::Unresolved, None);
        };
        let signer_address = evm_voucher_signer(&channel.config);
        let signer = VoucherSigner::Evm(signer_address);
        let Some(signature) = unhex::<65>(&voucher.signature) else {
            return (VoucherCheck::SignatureInvalid(signer), None);
        };
        if verify_evm_voucher(
            &domain(),
            &channel_id,
            u128::from(voucher.max_claimable_amount),
            &signature,
            &signer_address,
        ) {
            (
                VoucherCheck::Verified(signer),
                Some((channel_id, signature)),
            )
        } else {
            (VoucherCheck::SignatureInvalid(signer), None)
        }
    }
}

/// A voucher that verified: the channel it is on, and its signature.
type VerifiedVoucher = ([u8; 32], [u8; 65]);

/// A presented `channelConfig`'s strings, as the addresses and salt they
/// spell -- or `None` for one that spells nothing.
fn batch_config(presented: &EvmVoucherChannelConfig) -> Option<BatchChannelConfig> {
    Some(BatchChannelConfig {
        payer: unhex(&presented.payer)?,
        payer_authorizer: unhex(&presented.payer_authorizer)?,
        receiver: unhex(&presented.receiver)?,
        receiver_authorizer: unhex(&presented.receiver_authorizer)?,
        token: unhex(&presented.token)?,
        withdraw_delay: presented.withdraw_delay,
        salt: unhex(&presented.salt)?,
    })
}

#[async_trait]
impl VoucherEvidence for VoucherBook {
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck {
        self.check(voucher).0
    }

    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck {
        let PeerRoleChallenge::Evm {
            channel_id,
            expires,
            signature,
            channel_config,
        } = challenge
        else {
            return VoucherCheck::Unresolved;
        };
        let Some(channel) = self.resolve(channel_id, channel_config.as_ref()) else {
            return VoucherCheck::Unresolved;
        };
        let signer_address = evm_voucher_signer(&channel.config);
        let signer = VoucherSigner::Evm(signer_address);
        if verify_evm_voucher_claim_state_challenge(
            &domain(),
            channel_id,
            *expires,
            signature,
            &signer_address,
        ) {
            VoucherCheck::Verified(signer)
        } else {
            VoucherCheck::SignatureInvalid(signer)
        }
    }

    async fn judge_peer_voucher(&self, voucher: &ClientClaim) -> PeerVoucherVerdict {
        let (check, verified) = self.check(voucher);
        let mut watermarks = self.watermarks.lock().expect("watermark lock");
        let Some((channel_id, signature)) = verified else {
            let ack = match check {
                VoucherCheck::Unresolved => {
                    ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
                }
                _ => ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid),
            };
            return PeerVoucherVerdict { ack, prior: 0 };
        };
        let amount = voucher.transferred_amount();
        let current = watermarks.get(&channel_id).copied();
        let prior = current.map_or(0, |accepted| accepted.amount);
        let max_cumulative = self
            .resolve(&channel_id, None)
            .map_or(0, |channel| channel.max_cumulative);
        let ack = match current {
            // The byte-identical resend a lost ack forces: accepted again,
            // advancing nothing (§6.3).
            Some(accepted) if accepted.amount == amount && accepted.signature == signature => {
                ClaimAckOutcome::Accepted
            }
            _ if amount > prior && amount <= max_cumulative => {
                watermarks.insert(channel_id, Accepted { amount, signature });
                ClaimAckOutcome::Accepted
            }
            _ => ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing),
        };
        PeerVoucherVerdict { ack, prior }
    }
}

// ─── the payee ───

fn clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

/// `connector` with [`PEER_ID`] configured and the peer's settlement key
/// bound to it as a voucher signer -- what a `[[peer_channels]]` row with
/// `voucher_signer` does at boot (#1380).
fn bound(connector: Connector) -> Arc<Connector> {
    let connector = Arc::new(connector.with_config_peer_ids([PEER_ID.to_string()]));
    connector
        .bind_voucher_signer(PEER_ID, VoucherSigner::Evm(address_of(&peer_key())))
        .expect("a configured peer");
    connector
}

/// A connector over `routes`, terminating through `app_client`.
fn node(routes: Vec<StaticRoute>, app_client: Arc<FakeAppClient>) -> Connector {
    Connector::new(
        routes,
        vec![],
        app_client,
        Arc::new(InProcessPeerTransport::new()),
        clock(),
    )
}

/// The payee: no routes -- so every packet it is handed answers `F02` and
/// the *voucher*'s verdict is visibly independent of the packet's (§6.2) --
/// and the peer's voucher signer bound.
fn payee() -> Arc<Connector> {
    bound(node(vec![], Arc::new(FakeAppClient::new())))
}

/// This payee's identity key, so a sender can seal a packet it can fulfil.
fn payee_identity() -> Arc<dyn Signer> {
    Arc::new(LocalSigner::from_secret_bytes("payee-identity", [0x5c; 32]).expect("identity signer"))
}

/// As [`payee`], with one terminated route answered by `app_client` and the
/// identity [`sealed_prepare`] seals to -- the fixture issue #880's
/// price-coverage gate needs, since a payee with no routes never reaches it.
fn payee_with_route(route: StaticRoute, app_client: Arc<FakeAppClient>) -> Arc<Connector> {
    bound(node(vec![route], app_client).with_identity_signer(payee_identity()))
}

/// The one priced, terminated route issue #880's gate and issue #1104's
/// restart tests both need.
fn priced_route() -> StaticRoute {
    StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap()
}

/// An app that actually answers `route`'s handler, so a packet the gate
/// admits visibly **fulfils** rather than merely getting past one check.
fn serving_app(route: &StaticRoute, body: &[u8]) -> Arc<FakeAppClient> {
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: connector_domain::EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: body.to_vec(),
            },
        },
    );
    app_client
}

fn accepting(connector: Arc<Connector>, book: &Arc<VoucherBook>) -> Arc<PeerHttpState> {
    accepting_with_enforcement(connector, book, Arc::new(ClaimEnforcementPolicy::default()))
}

/// [`accepting`], with an explicit [`ClaimEnforcementPolicy`] rather than
/// the default (empty, so every peer reads
/// `ForwardedClaimEnforcement::Observe`). Only ADR 0042's forwarded rule
/// answers to this policy; the terminated rule refuses unconditionally
/// (issue #1077).
fn accepting_with_enforcement(
    connector: Arc<Connector>,
    book: &Arc<VoucherBook>,
    enforcement: Arc<ClaimEnforcementPolicy>,
) -> Arc<PeerHttpState> {
    Arc::new(
        PeerHttpState::new(connector, enforcement, PeerHttpPolicy::default())
            .with_voucher_evidence(Arc::clone(book) as Arc<dyn VoucherEvidence>),
    )
}

/// §1.10's dedicated peer listener over [`payee`].
fn dedicated(book: &Arc<VoucherBook>) -> PeerHttpState {
    PeerHttpState::new(
        payee(),
        Arc::new(ClaimEnforcementPolicy::default()),
        PeerHttpPolicy {
            mandatory_auth: true,
        },
    )
    .with_voucher_evidence(Arc::clone(book) as Arc<dyn VoucherEvidence>)
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

/// The amount every forwarded-arrival test sends.
const ARRIVING_AMOUNT: u64 = 100;

/// As [`payee`], but **forwarding**: one `peer_id` route over which
/// [`FORWARDED_DESTINATION`] reaches a real second connector that terminates
/// it, paid for on an outbound x402 channel of this node's own
/// ([`covering`], issue #1145). The BTP twin of the same fixture.
///
/// Returns the next hop's own app client and identity signer too, so a test
/// can seal a packet the far end can actually fulfil and then prove the
/// packet really was carried rather than merely not refused.
fn forwarding_payee() -> (Arc<Connector>, Arc<FakeAppClient>, Arc<dyn Signer>) {
    let next_hop_route = StaticRoute::new(FORWARDED_DESTINATION, "http://localhost:4100").unwrap();
    let app_client = serving_app(&next_hop_route, b"delivered by the next hop");
    let identity: Arc<dyn Signer> = Arc::new(LocalSigner::generate("next-hop-identity"));
    let next_hop = Arc::new(
        node(vec![next_hop_route], Arc::clone(&app_client))
            .with_identity_signer(Arc::clone(&identity)),
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
    let envelope = connector_domain::EnvelopeRequest {
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

/// [`sealed_prepare_to`] at [`payee_identity`]'s priced termination.
fn sealed_prepare(amount: u64) -> (Prepare, [u8; 32]) {
    sealed_prepare_to(payee_identity().as_ref(), "g.example.app", amount)
}

/// A policy in which `PEER_ID` enforces ADR 0042's forwarded rule.
fn forwarded_enforcing() -> Arc<ClaimEnforcementPolicy> {
    Arc::new(ClaimEnforcementPolicy::of(vec![(
        PEER_ID,
        connector_config::ForwardedClaimEnforcement::Enforce,
    )]))
}

fn prepare(destination: &str) -> Prepare {
    Prepare {
        amount: 100,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data: b"sealed to whoever terminates this route".to_vec(),
    }
}

/// A PREPARE that moves no value: the only packet a peer-role challenge
/// proves the role for (ADR 0075 decision 5).
fn zero_value_prepare(destination: &str) -> Prepare {
    Prepare {
        amount: 0,
        ..prepare(destination)
    }
}

// ─── the in-process client standing in for the socket ───

/// Hands each request straight to an accepting [`PeerHttpState`], recording
/// what actually went on the wire so a test can assert §3's table rather than
/// only what came back.
struct Loopback {
    peer: Arc<PeerHttpState>,
    sent: Mutex<Vec<(Url, PeerRequest)>>,
    /// §7.2: the high-water mark of voucher-bearing requests in flight at
    /// once.
    concurrent_vouchers: AtomicUsize,
    peak_concurrent_vouchers: AtomicUsize,
    /// How long each request dwells inside the peer, so overlapping requests
    /// would actually overlap if the in-flight rule were not enforced.
    dwell: Duration,
}

impl Loopback {
    fn new(peer: Arc<PeerHttpState>) -> Arc<Loopback> {
        Loopback::with_dwell(peer, Duration::ZERO)
    }

    fn with_dwell(peer: Arc<PeerHttpState>, dwell: Duration) -> Arc<Loopback> {
        Arc::new(Loopback {
            peer,
            sent: Mutex::new(Vec::new()),
            concurrent_vouchers: AtomicUsize::new(0),
            peak_concurrent_vouchers: AtomicUsize::new(0),
            dwell,
        })
    }

    fn sent(&self) -> Vec<PeerRequest> {
        self.sent
            .lock()
            .expect("sent lock")
            .iter()
            .map(|(_, request)| request.clone())
            .collect()
    }

    fn last(&self) -> PeerRequest {
        self.sent().pop().expect("a request went out")
    }

    fn last_endpoint(&self) -> Url {
        self.sent
            .lock()
            .expect("sent lock")
            .last()
            .expect("a request went out")
            .0
            .clone()
    }
}

#[async_trait]
impl PeerHttpClient for Loopback {
    async fn post(
        &self,
        endpoint: &Url,
        request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        self.sent
            .lock()
            .expect("sent lock")
            .push((endpoint.clone(), request.clone()));
        let carries_voucher = request.headers.get(CLAIM_HEADER).is_some();
        if carries_voucher {
            let in_flight = self.concurrent_vouchers.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_concurrent_vouchers
                .fetch_max(in_flight, Ordering::SeqCst);
        }
        if !self.dwell.is_zero() {
            tokio::time::sleep(self.dwell).await;
        }
        let response = self.peer.handle(request).await;
        if carries_voucher {
            self.concurrent_vouchers.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(response)
    }
}

/// A client that never reaches anybody -- the "the remote does not expose
/// what we dial" case §2.2 says is not locally detectable and must surface as
/// an ordinary dial failure.
struct Unreachable;

#[async_trait]
impl PeerHttpClient for Unreachable {
    async fn post(
        &self,
        endpoint: &Url,
        _request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        Err(HttpDialError {
            peer_id: PEER_ID.to_string(),
            endpoint: endpoint.to_string(),
            reason: "connection refused".to_string(),
        })
    }
}

/// A client that answers a status carrying no ILP body at all (§6.2's
/// `4xx`/`5xx`).
struct Status(u16);

#[async_trait]
impl PeerHttpClient for Status {
    async fn post(
        &self,
        _endpoint: &Url,
        _request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        Ok(PeerResponse::refused(self.0))
    }
}

fn relation() -> PeerRelation {
    PeerRelation::new(
        PEER_ID,
        Url::parse("https://peer.example:443/ilp").unwrap(),
        Duration::from_millis(30_000),
    )
}

fn transport(client: Arc<dyn PeerHttpClient>) -> HttpPeerTransport {
    let transport = HttpPeerTransport::new(client);
    transport.add_peer(relation());
    transport
}

fn voucher(json: String) -> Option<Covering> {
    Some(Covering::Voucher(json))
}

/// One request, as a peer would send it by hand -- for the accept-side tests
/// that have no dialing transport in front of them.
fn request(voucher_json: Option<&str>, body: Vec<u8>) -> PeerRequest {
    let mut headers = Headers::new();
    if let Some(json) = voucher_json {
        headers.push(
            CLAIM_HEADER,
            connector_peer_http::headers::claim_header_value(json),
        );
    }
    PeerRequest { headers, body }
}

/// One request carrying a peer-role challenge in its own header.
fn challenged(challenge_json: &str, body: Vec<u8>) -> PeerRequest {
    let mut headers = Headers::new();
    headers.push(
        PEER_CHALLENGE_HEADER,
        connector_peer_http::headers::peer_challenge_header_value(challenge_json),
    );
    PeerRequest { headers, body }
}

fn ack_on(response: &PeerResponse) -> Option<ClaimAckOutcome> {
    connector_peer_http::headers::claim_ack(&response.headers)
}

fn reject_code(response: &PeerResponse) -> String {
    connector_domain::Reject::decode(&response.body)
        .expect("a reject")
        .code
        .as_str()
        .to_string()
}

// ─── §3, §6: a voucher rides a PREPARE and is acknowledged ───

/// §6.2, the property whose loss would silently destroy ADR 0024's
/// semantics: **the body answers the packet, the header answers the
/// voucher, and the status is `200` regardless**. The packet is rejected
/// (the payee has no route for it) and the voucher that rode it is
/// accepted, on the one response.
#[tokio::test]
async fn a_voucher_riding_a_prepare_is_judged_independently_of_the_packet() {
    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected the payee's own reject, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(reached, "the peer answered, so this hop forwarded");
    assert_eq!(book.watermark(&channel_of(&peer_key())), Some(500));
}

/// §3's table, as bytes: the voucher is a `base64(JSON)` header carrying
/// exactly the JSON handed to the port, and the body is the OER PREPARE
/// unchanged (§8.1).
#[tokio::test]
async fn the_request_a_dialed_peering_puts_on_the_wire_is_the_one_section_3_names() {
    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);
    let json = peer_voucher(500);
    let prepare = prepare("g.nowhere");

    let _ = transport
        .forward(PEER_ID, prepare.clone(), voucher(json.clone()))
        .await;

    let sent = client.last();
    // §1.4: nothing authenticates the peering but the voucher. A dialer
    // sends no credential, because there is none to send (ADR 0060) -- and
    // the header it used to ride in must not reappear under any spelling.
    assert!(
        sent.headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("toon-peer-auth")),
        "a dialed peering put a peer credential on the wire"
    );
    // §4: `base64(JSON)` over exactly the JSON the BTP entry carries raw.
    assert_eq!(
        base64_decode(sent.headers.get(CLAIM_HEADER).expect("the voucher rode")),
        json.as_bytes()
    );
    // §8.1: `data` rides byte-for-byte unchanged, in the same OER encoding
    // every other carriage puts on a wire.
    assert_eq!(sent.body, prepare.encode());
    // §3: a peer connector MUST NOT invent additional headers. One: the
    // voucher.
    assert_eq!(sent.headers.len(), 1, "got {:?}", sent.headers);
    assert_eq!(client.last_endpoint(), relation_endpoint());
}

fn relation_endpoint() -> Url {
    Url::parse("https://peer.example:443/ilp").unwrap()
}

/// ADR 0075 decision 5: a PREPARE that moves no value is covered by a
/// **peer-role challenge**, which rides its own header -- never the claim
/// header -- and proves the peer role at the far end. Proved on a dedicated
/// listener, which answers anything that is not a peer's `401`.
#[tokio::test]
async fn a_challenge_rides_its_own_header_and_proves_the_peer_role() {
    let book = VoucherBook::new();
    let client = Loopback::new(Arc::new(dedicated(&book)));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);
    let json = evm_challenge(&peer_key(), &peer_key(), fresh_expiry());

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            PEER_ID,
            zero_value_prepare("g.nowhere"),
            Some(Covering::Challenge(json.clone())),
        )
        .await;

    let sent = client.last();
    assert_eq!(
        sent.headers.get(CLAIM_HEADER),
        None,
        "a challenge is no voucher"
    );
    assert_eq!(
        base64_decode(
            sent.headers
                .get(PEER_CHALLENGE_HEADER)
                .expect("the challenge rode")
        ),
        json.as_bytes()
    );
    assert_eq!(sent.headers.len(), 1, "got {:?}", sent.headers);
    // Past the dedicated listener's 401: the challenge made it a peer's.
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected the payee's own reject, got {other:?}"),
    }
    assert!(reached);
    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "a challenge pays nothing, so there is nothing to acknowledge"
    );
}

/// ADR 0075 decision 5: a challenge proves the role **only** for a packet
/// that moves no value, and only when its signer is the channel's.
#[tokio::test]
async fn a_challenge_proves_nothing_on_a_packet_that_moves_value_or_under_a_foreign_signature() {
    let book = VoucherBook::new();
    let peer = dedicated(&book);

    let moves_value = peer
        .handle(challenged(
            &evm_challenge(&peer_key(), &peer_key(), fresh_expiry()),
            prepare("g.nowhere").encode(),
        ))
        .await;
    let forged = peer
        .handle(challenged(
            &evm_challenge(&peer_key(), &stranger_key(), fresh_expiry()),
            zero_value_prepare("g.nowhere").encode(),
        ))
        .await;
    let lapsed = peer
        .handle(challenged(
            &evm_challenge(&peer_key(), &peer_key(), CLOCK_UNIX - 1),
            zero_value_prepare("g.nowhere").encode(),
        ))
        .await;
    let genuine = peer
        .handle(challenged(
            &evm_challenge(&peer_key(), &peer_key(), fresh_expiry()),
            zero_value_prepare("g.nowhere").encode(),
        ))
        .await;

    assert_eq!(moves_value.status, 401, "a challenge never pays for value");
    assert_eq!(
        forged.status, 401,
        "signed by a key that is not the channel's"
    );
    assert_eq!(lapsed.status, 401, "a challenge past its `expires`");
    assert_eq!(genuine.status, 200);
}

/// §10.2 item 6: an uncovered PREPARE is legal on the wire, and carries no
/// claim header rather than an empty one.
#[tokio::test]
async fn an_uncovered_prepare_carries_no_claim_header() {
    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);

    let _ = transport.forward(PEER_ID, prepare("g.nowhere"), None).await;

    let sent = client.last();
    assert_eq!(sent.headers.get(CLAIM_HEADER), None);
    assert_eq!(sent.headers.get(PEER_CHALLENGE_HEADER), None);
}

/// Dialing from a **loaded config** rather than a hand-built relation:
/// [`HttpPeerTransport::add_peers_from_config`] registers the `https://`
/// peering an ADR 0075 config names -- a `[[peer_channels]]` row binding
/// the peer's `voucher_signer`, `[settlement.evm.batch_settlement]` beside
/// it -- at the endpoint the file gives, and a voucher rides it.
#[tokio::test]
async fn a_peering_dialed_from_a_loaded_config_carries_its_voucher_to_the_configured_endpoint() {
    use std::io::Write;

    let state_dir = tempfile::tempdir().expect("temp state dir");
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file.write_all(b"not a real key").expect("write key");
    let toml = format!(
        r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "{PEER_ID}"
endpoint = "https://configured.example:8443/ilp"

[[peer_channels]]
peer_id = "{PEER_ID}"
voucher_signer = "{signer}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{key_file}"

"#,
        state_dir = state_dir.path().display(),
        key_file = key_file.path().display(),
        signer = hex(&address_of(&peer_key())),
    );
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("temp config file");
    config_file
        .write_all(toml.as_bytes())
        .expect("write config");
    let config = connector_config::Config::load(config_file.path()).expect("load");

    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = HttpPeerTransport::new(Arc::clone(&client) as Arc<dyn PeerHttpClient>);
    transport.add_peers_from_config(config.peers());

    let PeerForward { ack, .. } = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert_eq!(
        client.last_endpoint(),
        Url::parse("https://configured.example:8443/ilp").unwrap(),
        "the relation dials the endpoint the file names (§2.1)"
    );
}

// ─── §6.3: retransmission, and the idempotent re-ack ───

/// §6.3, the rule standing between a lost ack and a permanently wedged
/// peering: a voucher byte-identical to the one already at the watermark is
/// answered `accepted`, and nothing is advanced. The carriage's half is
/// that it puts the voucher on the wire exactly as it was handed it, so a
/// resend is byte-identical.
#[tokio::test]
async fn a_byte_identical_voucher_resent_at_the_watermark_is_accepted_again() {
    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);
    let json = peer_voucher(500);

    let first = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(json.clone()))
        .await;
    let resent = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(json))
        .await;

    assert_eq!(first.ack, ClaimAckOutcome::Accepted);
    assert_eq!(
        resent.ack,
        ClaimAckOutcome::Accepted,
        "a resend at the watermark is accepted, never amount_not_advancing"
    );
    let sent = client.sent();
    assert_eq!(
        sent[0].headers.get(CLAIM_HEADER),
        sent[1].headers.get(CLAIM_HEADER),
        "the resend was byte-identical (§6.3)"
    );
    assert_eq!(
        book.watermark(&channel_of(&peer_key())),
        Some(500),
        "a resend advances nothing"
    );
}

/// §6.3's other half: a voucher **below** the watermark is refused
/// `amount_not_advancing`. A voucher's watermark is one cumulative amount
/// (ADR 0075 decision 6), so there is no nonce to fall back on.
#[tokio::test]
async fn a_voucher_below_the_watermark_is_refused_amount_not_advancing() {
    let book = VoucherBook::new();
    let client = Loopback::new(accepting(payee(), &book));
    let transport = transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>);

    let higher = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;
    let lower = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(400)))
        .await;

    assert_eq!(higher.ack, ClaimAckOutcome::Accepted);
    assert_eq!(
        lower.ack,
        ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
    );
    assert_eq!(book.watermark(&channel_of(&peer_key())), Some(500));
}

/// §6.3: **absence means NOT ACKNOWLEDGED** -- never accepted, never
/// rejected, never inferred from the packet's verdict.
#[tokio::test]
async fn a_response_carrying_no_ack_header_leaves_the_voucher_not_acknowledged() {
    struct Silent;

    #[async_trait]
    impl PeerHttpClient for Silent {
        async fn post(
            &self,
            _endpoint: &Url,
            _request: PeerRequest,
        ) -> Result<PeerResponse, HttpDialError> {
            let mut response = PeerResponse::ok(
                connector_domain::Reject {
                    code: connector_domain::RejectCode::f02_unreachable(),
                    triggered_by: String::new(),
                    message: String::new(),
                    data: Vec::new(),
                    accumulated_cost: 0,
                }
                .encode(),
            );
            response.headers.push(ACCUMULATED_COST_HEADER, "0");
            Ok(response)
        }
    }

    let transport = transport(Arc::new(Silent));

    let PeerForward { ack, .. } = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// §6.3: a malformed ack -- undecodable base64, undecodable JSON, an unknown
/// `result`, a `rejected` with no `reason` -- is likewise **not
/// acknowledged**, and MUST NOT be read as either verdict.
#[tokio::test]
async fn a_malformed_ack_header_leaves_the_voucher_not_acknowledged() {
    struct Garbled(&'static str);

    #[async_trait]
    impl PeerHttpClient for Garbled {
        async fn post(
            &self,
            _endpoint: &Url,
            _request: PeerRequest,
        ) -> Result<PeerResponse, HttpDialError> {
            let mut response = PeerResponse::ok(
                connector_domain::Reject {
                    code: connector_domain::RejectCode::f02_unreachable(),
                    triggered_by: String::new(),
                    message: String::new(),
                    data: Vec::new(),
                    accumulated_cost: 0,
                }
                .encode(),
            );
            response.headers.push(CLAIM_ACK_HEADER, self.0);
            Ok(response)
        }
    }

    for garbled in [
        "!!! not base64 !!!",
        "bm90IGpzb24=",                 // "not json"
        "eyJyZXN1bHQiOiJtYXliZSJ9",     // {"result":"maybe"}
        "eyJyZXN1bHQiOiJyZWplY3RlZCJ9", // {"result":"rejected"}
    ] {
        let transport = transport(Arc::new(Garbled(garbled)));

        let PeerForward { ack, .. } = transport
            .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
            .await;

        assert_eq!(
            ack,
            ClaimAckOutcome::NotSent,
            "read a verdict from {garbled}"
        );
    }
}

/// §6.2: `4xx`/`5xx` are reserved for a request there is no ILP answer to.
/// A response like that is not an ILP verdict, is not a voucher verdict,
/// and leaves the voucher not acknowledged.
#[tokio::test]
async fn a_non_200_answer_is_no_ilp_answer_at_all() {
    let transport = transport(Arc::new(Status(400)));

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(
        !reached,
        "no fee of ours belongs on a hop that never forwarded"
    );
}

// ─── §2.2, §2.4, §6.4(1): what cannot be reached, and why ───

/// §2.2: whether the remote exposes what we dial is not locally detectable,
/// so it surfaces as an ordinary dial failure and the packet rejects `T01`
/// naming the endpoint -- never `T00`, and never a silent drop.
#[tokio::test]
async fn a_peer_that_cannot_be_reached_rejects_t01_and_was_never_reached() {
    let transport = transport(Arc::new(Unreachable));

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), voucher(peer_voucher(500)))
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(
                reject.message.contains("peer.example"),
                "{}",
                reject.message
            );
        }
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

/// §6.4(1) and §2.4, the two things an operator hits first and diagnoses
/// last. A peer this connector does not dial over HTTP can never be
/// originated to -- packets flow only in the dialing direction -- and the
/// `T01` says so, including that an HTTP-only peer can neither reach nor be
/// reached by a NAT'd peer.
#[tokio::test]
async fn a_peer_this_connector_cannot_originate_to_says_why_in_its_t01() {
    let transport = transport(Arc::new(Unreachable));

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            "accept-only",
            prepare("g.nowhere"),
            voucher(peer_voucher(500)),
        )
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(
                reject.message.contains("§6.4(1)"),
                "an operator must not have to infer unidirectional packet flow: {}",
                reject.message
            );
            assert!(
                reject.message.contains("NAT'd peer"),
                "the NAT consequence is the least obvious thing here: {}",
                reject.message
            );
        }
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
    assert!(NAT_NOTE.contains("must be BTP"));
}

// ─── §1, ADR 0075 decision 5: role is decided by a bound signer's voucher ───

/// **The named regression (§1.9)**, over ADR 0075's proof. `toon-sandbox`
/// admitted an anonymous BTP session and then treated it as a quasi-peer.
/// Each interaction below is classified `client` and reaches **no peer
/// handling whatsoever** -- testable, per §1.9, as: no `Toon-Claim-Ack` was
/// emitted, and no watermark moved (proved by the peer's genuine voucher
/// afterwards being judged against an untouched channel).
#[tokio::test]
async fn the_named_regression_no_request_becomes_a_peer_without_a_bound_verifying_voucher() {
    let book = VoucherBook::new();
    let peer = accepting(payee(), &book);

    let cases: Vec<(&str, PeerRequest)> = vec![
        (
            "no evidence at all",
            request(None, prepare("g.nowhere").encode()),
        ),
        (
            "a genuine voucher on a channel whose signer is bound to no peering",
            request(
                Some(&signed_voucher(&stranger_key(), 500)),
                prepare("g.nowhere").encode(),
            ),
        ),
        (
            "a voucher on the peer's channel that does not recover to its signer",
            request(
                Some(&voucher_signed_by(&peer_key(), &stranger_key(), 500)),
                prepare("g.nowhere").encode(),
            ),
        ),
        (
            "a voucher on a channel this node has no record of",
            request(
                Some(&signed_voucher(&unknown_key(), 500)),
                prepare("g.nowhere").encode(),
            ),
        ),
        (
            "a genuine challenge on a packet that moves value",
            challenged(
                &evm_challenge(&peer_key(), &peer_key(), fresh_expiry()),
                prepare("g.nowhere").encode(),
            ),
        ),
    ];

    for (case, request) in cases {
        let response = peer.handle(request).await;

        // §1.6: not refused for the assertion alone -- refusing would make
        // the check an oracle for which peerings are configured.
        assert_eq!(response.status, 200, "{case}");
        assert_eq!(reject_code(&response), "F02", "{case}");
        assert!(
            ack_on(&response).is_none(),
            "{case}: a client interaction gets no claim-ack (§1.7)"
        );
    }
    assert_eq!(book.watermark(&channel_of(&stranger_key())), None);
    assert_eq!(book.watermark(&channel_of(&peer_key())), None);

    let genuine = peer
        .handle(request(Some(&peer_voucher(500)), Vec::new()))
        .await;
    assert_eq!(
        ack_on(&genuine),
        Some(ClaimAckOutcome::Accepted),
        "no client interaction had advanced this channel's watermark"
    );
}

/// ADR 0075 decision 8 and #1384: **a `toon-channel` claim is refused by
/// name** on this carriage -- `400`, with the retirement named in the body,
/// on a shared listener and a dedicated one alike, before any role is
/// decided. No `Toon-Claim-Ack`: nothing was judged.
#[tokio::test]
async fn a_toon_channel_claim_is_refused_by_name() {
    let book = VoucherBook::new();
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

    let shared = accepting(payee(), &book)
        .handle(request(Some(&json), prepare("g.nowhere").encode()))
        .await;
    let on_dedicated = dedicated(&book)
        .handle(request(Some(&json), prepare("g.nowhere").encode()))
        .await;

    for response in [&shared, &on_dedicated] {
        assert_eq!(response.status, 400);
        assert!(ack_on(response).is_none(), "nothing was judged");
        let body = String::from_utf8_lossy(&response.body);
        assert!(body.contains("toon-channel"), "{body}");
        assert!(body.contains("ADR 0075"), "{body}");
    }
}

/// §1.5's header-smuggling defence over the material that decides role:
/// **more than one voucher on one request is refused, not resolved** --
/// `400`, with no ILP body, and never the first, the last or a
/// concatenation. So is a voucher beside a challenge (ADR 0075 decision 5):
/// "which did we verify?" has no answer.
#[tokio::test]
async fn two_pieces_of_evidence_on_one_request_are_refused_rather_than_resolved() {
    let book = VoucherBook::new();
    let peer = accepting(payee(), &book);
    let first = peer_voucher(500);
    let second = peer_voucher(600);

    let mut two_vouchers = Headers::new();
    for json in [&first, &second] {
        two_vouchers.push(
            CLAIM_HEADER,
            connector_peer_http::headers::claim_header_value(json),
        );
    }
    let mut voucher_and_challenge = Headers::new();
    voucher_and_challenge.push(
        CLAIM_HEADER,
        connector_peer_http::headers::claim_header_value(&first),
    );
    voucher_and_challenge.push(
        PEER_CHALLENGE_HEADER,
        connector_peer_http::headers::peer_challenge_header_value(&evm_challenge(
            &peer_key(),
            &peer_key(),
            fresh_expiry(),
        )),
    );

    for headers in [two_vouchers, voucher_and_challenge] {
        let response = peer
            .handle(PeerRequest {
                headers,
                body: prepare("g.nowhere").encode(),
            })
            .await;

        assert_eq!(response.status, 400);
        assert!(response.body.is_empty(), "a 400 carries no ILP body (§1.5)");
        assert!(ack_on(&response).is_none());
    }

    // Neither voucher was adopted: nothing was resolved behind the refusal.
    assert_eq!(book.watermark(&channel_of(&peer_key())), None);
    let fresh = peer.handle(request(Some(&first), Vec::new())).await;
    assert_eq!(ack_on(&fresh), Some(ClaimAckOutcome::Accepted));
}

/// §1.4: because HTTP has no session, a request is judged on its own
/// evidence. One request proving a peering says nothing about the next.
#[tokio::test]
async fn a_request_without_a_voucher_is_a_client_however_the_last_one_was_judged() {
    let book = VoucherBook::new();
    let peer = accepting(payee(), &book);

    let proven = peer
        .handle(request(Some(&peer_voucher(500)), Vec::new()))
        .await;
    let next = peer.handle(request(None, Vec::new())).await;

    assert_eq!(ack_on(&proven), Some(ClaimAckOutcome::Accepted));
    assert!(
        ack_on(&next).is_none(),
        "the previous request's role does not carry over (§1.4)"
    );
}

/// ADR 0060: `Toon-Peer-Auth` is **ignored, not refused**. A request still
/// setting it is read exactly as one that does not -- a peer's voucher is
/// still judged, and the header alone makes nobody a peer -- so the two
/// ends of a peering can be upgraded in either order.
#[tokio::test]
async fn a_lingering_toon_peer_auth_header_is_ignored() {
    let book = VoucherBook::new();
    let peer = accepting(payee(), &book);
    let credential = "eyJwZWVySWQiOiJwZWVyLWIiLCJzZWNyZXQiOiJzM2NyM3QifQ==";

    let mut with_voucher = request(Some(&peer_voucher(500)), prepare("g.nowhere").encode());
    with_voucher.headers.push("Toon-Peer-Auth", credential);
    let mut alone = request(None, prepare("g.nowhere").encode());
    alone.headers.push("Toon-Peer-Auth", credential);

    let peered = peer.handle(with_voucher).await;
    let client = peer.handle(alone).await;

    assert_eq!(peered.status, 200);
    assert_eq!(ack_on(&peered), Some(ClaimAckOutcome::Accepted));
    assert_eq!(client.status, 200, "not refused for the stale header");
    assert!(
        ack_on(&client).is_none(),
        "a credential proves nothing since ADR 0060"
    );
}

/// §5.2: a REJECT this carriage sends always carries the running cost,
/// **even at zero**, so "absent" never has to carry meaning in the
/// direction that matters.
#[tokio::test]
async fn a_peers_reject_always_carries_the_accumulated_cost_even_at_zero() {
    let book = VoucherBook::new();
    let peer = accepting(payee(), &book);

    let response = peer
        .handle(request(
            Some(&peer_voucher(500)),
            prepare("g.nowhere").encode(),
        ))
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(reject_code(&response), "F02");
    assert_eq!(response.headers.get(ACCUMULATED_COST_HEADER), Some("0"));
    // §6.2: the two verdicts are independent, so the voucher that rode the
    // refused packet is still judged and still acknowledged.
    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
}

/// §1.10's bounded escape hatch: on a **dedicated** peer listener a request
/// whose evidence does not prove the peer role is refused outright rather
/// than downgraded, because such a listener serves no clients. Role is
/// still decided by the voucher; the listener never becomes the decider.
#[tokio::test]
async fn a_dedicated_peer_listener_refuses_rather_than_downgrades() {
    let book = VoucherBook::new();
    let peer = dedicated(&book);

    let no_voucher = peer
        .handle(request(None, prepare("g.nowhere").encode()))
        .await;
    let forged = peer
        .handle(request(
            Some(&voucher_signed_by(&peer_key(), &stranger_key(), 500)),
            prepare("g.nowhere").encode(),
        ))
        .await;
    let unbound = peer
        .handle(request(
            Some(&signed_voucher(&stranger_key(), 500)),
            prepare("g.nowhere").encode(),
        ))
        .await;
    let admitted = peer
        .handle(request(
            Some(&peer_voucher(500)),
            prepare("g.nowhere").encode(),
        ))
        .await;

    assert_eq!(no_voucher.status, 401);
    assert!(no_voucher.body.is_empty());
    assert_eq!(forged.status, 401, "a voucher that does not verify");
    assert_eq!(
        unbound.status, 401,
        "a voucher whose signer no peering binds"
    );
    assert_eq!(
        admitted.status, 200,
        "a bound signer's voucher decides the role"
    );
}

// ─── issue #880 (owner decision #868): every peer PREPARE to a priced
// terminated route carries a covering voucher, or is refused with the client
// edge's own x402 greeting ───

/// A voucherless arrival reaches **no peer handling at all** (§1.2): it is
/// a client request, and the greeting a client gets is the client edge's
/// own. What this layer pins is that the app never sees it and nothing is
/// acknowledged.
#[tokio::test]
async fn a_voucherless_arrival_at_a_priced_route_reaches_no_peer_handling() {
    let book = VoucherBook::new();
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, _) = sealed_prepare(25);

    let response = peer.handle(request(None, sealed.encode())).await;

    assert_eq!(
        response.status, 200,
        "a packet verdict, not a transport 4xx (§6.2)"
    );
    assert_eq!(
        reject_code(&response),
        "F02",
        "a client-role packet reaches no peer route"
    );
    assert!(
        ack_on(&response).is_none(),
        "no claim-ack to a client (§1.7)"
    );
    assert!(app_client.deliveries().is_empty());
}

/// A voucher rides the request, but its advance over the watermark falls
/// short of the route's price: refused `F06` with the greeting -- while
/// the voucher itself is perfectly good and is acknowledged, the two
/// verdicts staying independent (§6.2).
#[tokio::test]
async fn a_voucher_that_does_not_cover_the_routes_price_is_refused_with_the_greeting() {
    let book = VoucherBook::new();
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, _) = sealed_prepare(25);

    let response = peer
        .handle(request(Some(&peer_voucher(10)), sealed.encode()))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert_eq!(reject_code(&response), "F06");
    assert!(response.headers.get(PAYMENT_REQUIRED_HEADER).is_some());
    assert!(app_client.deliveries().is_empty());
}

/// ADR 0065 (issue #984) at the peer gate: what a peer arrival must cover is
/// the schedule at THAT packet's payload length, and the greeting it gets
/// back publishes the schedule so the peer can price its next packet.
#[tokio::test]
async fn a_peer_voucher_must_cover_the_schedule_at_this_packets_length() {
    let book = VoucherBook::new();
    let route = StaticRoute::new_scheduled(
        "g.example.app",
        "http://localhost:4000",
        connector_domain::Price::scheduled(25, 1),
    )
    .unwrap();
    let peer = accepting(
        payee_with_route(route, Arc::new(FakeAppClient::new())),
        &book,
    );

    // ~2 KiB of payload, so the slope actually bites: 25 + 1*2 = 27.
    let mut big = prepare("g.example.app");
    big.data = vec![0xab; 2000];
    let expected = connector_domain::Price::scheduled(25, 1).charge(big.data.len());
    assert_eq!(expected, 27);

    // A voucher covering the BASE is no longer enough for this packet.
    let response = peer
        .handle(request(Some(&peer_voucher(25)), big.encode()))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert_eq!(reject_code(&response), "F06");
    let terms = parse_greeting(&base64_decode(
        response
            .headers
            .get(PAYMENT_REQUIRED_HEADER)
            .expect("a refused arrival is greeted"),
    ))
    .expect("well-formed terms");
    assert_eq!(terms.price(), Some(expected));
    assert_eq!(
        terms.schedule(),
        Some(connector_domain::Price::scheduled(25, 1))
    );
}

/// The boundary this gate exists to leave open: a voucher whose advance
/// exactly meets the route's price is admitted -- delivered to the app, no
/// greeting.
#[tokio::test]
async fn a_covering_voucher_is_admitted() {
    let book = VoucherBook::new();
    let route = priced_route();
    let app_client = serving_app(&route, b"served");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, shared_secret) = sealed_prepare(25);

    let response = peer
        .handle(request(Some(&peer_voucher(25)), sealed.encode()))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
        "an admitted packet carries no greeting"
    );
    assert_eq!(opened_fulfil(&response, &shared_secret), b"served");
    assert_eq!(app_client.deliveries().len(), 1);
}

/// The body of the sealed FULFIL `response` carries, opened.
fn opened_fulfil(response: &PeerResponse, shared_secret: &[u8; 32]) -> Vec<u8> {
    let fulfill = connector_domain::Fulfill::decode(&response.body).expect("a fulfil");
    let opened = connector_signer::giftwrap::open_response(shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    connector_domain::EnvelopeResponse::decode(&opened)
        .expect("decode envelope")
        .body
}

/// PR #913 review finding, in the voucher world: a voucher on the peer's
/// channel signed by somebody else still *decodes* and can declare any
/// amount it likes. It fails verification, so the request is a **client**
/// request -- no ack, `F02` -- and the declared amount buys nothing.
#[tokio::test]
async fn a_forged_voucher_declaring_a_large_amount_does_not_buy_coverage() {
    let book = VoucherBook::new();
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, _) = sealed_prepare(25);

    let response = peer
        .handle(request(
            Some(&voucher_signed_by(&peer_key(), &stranger_key(), 1_000_000)),
            sealed.encode(),
        ))
        .await;

    assert!(
        ack_on(&response).is_none(),
        "a voucher that does not verify makes the request a client's (§1.7)"
    );
    assert_eq!(reject_code(&response), "F02");
    assert!(
        app_client.deliveries().is_empty(),
        "a forged voucher must never reach the app"
    );
    assert_eq!(book.watermark(&channel_of(&peer_key())), None);
}

/// PR #913 review finding, second case: a genuinely signed voucher already
/// at the watermark -- resent byte-identically -- is acknowledged, and
/// **advances nothing**, so it covers nothing; one below the watermark is
/// refused outright. Neither buys a packet, on any retransmission, and the
/// watermark never moves off the last genuinely new voucher.
#[tokio::test]
async fn a_voucher_that_advances_nothing_never_buys_coverage() {
    let book = VoucherBook::new();
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);

    // A voucher standing alone (an empty ILP body) takes the channel to 25.
    let paid = peer_voucher(25);
    let standalone = peer.handle(request(Some(&paid), Vec::new())).await;
    assert_eq!(ack_on(&standalone), Some(ClaimAckOutcome::Accepted));

    for attempt in 0..2 {
        let (sealed, _) = sealed_prepare(25);
        let resent = peer.handle(request(Some(&paid), sealed.encode())).await;
        assert_eq!(
            ack_on(&resent),
            Some(ClaimAckOutcome::Accepted),
            "attempt {attempt}: a byte-identical resend is accepted"
        );
        assert_eq!(reject_code(&resent), "F06", "attempt {attempt}");

        let (sealed, _) = sealed_prepare(25);
        let lower = peer
            .handle(request(Some(&peer_voucher(10)), sealed.encode()))
            .await;
        assert_eq!(
            ack_on(&lower),
            Some(ClaimAckOutcome::Rejected(
                ClaimRejectReason::AmountNotAdvancing
            )),
            "attempt {attempt}"
        );
        assert_eq!(reject_code(&lower), "F06", "attempt {attempt}");
        assert!(lower.headers.get(PAYMENT_REQUIRED_HEADER).is_some());
    }
    assert!(
        app_client.deliveries().is_empty(),
        "nothing that advanced nothing reached the app"
    );
    assert_eq!(book.watermark(&channel_of(&peer_key())), Some(25));
}

// ─── issue #1104: coverage is the voucher's advance past the **durable**
// watermark, so a payee restart never credits a voucher with its whole
// cumulative amount. The BTP twin of each lives in `connector-peer-btp`'s
// own `peer_carriage.rs` ───

/// Carries the peer's channel to cumulative 50 000 on a node over `book`,
/// then drops that node -- the state a restarted payee comes back to.
async fn book_at_fifty_thousand(book: &Arc<VoucherBook>) {
    let peer = accepting(
        payee_with_route(priced_route(), Arc::new(FakeAppClient::new())),
        book,
    );
    let response = peer
        .handle(request(Some(&peer_voucher(50_000)), Vec::new()))
        .await;
    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Accepted),
        "the pre-restart voucher is what the book records"
    );
}

/// The bug: a voucher at cumulative 50 001 after a restart is one unit of
/// genuinely new money, and cannot buy a packet priced at 25. Measured
/// against an empty per-process record it would be credited with all
/// 50 001 and buy it (issue #1104). Coverage is measured from the book's
/// own `prior`, which survived.
#[tokio::test]
async fn a_restart_does_not_credit_a_voucher_with_the_amount_it_already_paid() {
    let book = VoucherBook::new();
    book_at_fifty_thousand(&book).await;

    // The restart: a new node and a new carriage over the same book.
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, _) = sealed_prepare(25);

    let response = peer
        .handle(request(Some(&peer_voucher(50_001)), sealed.encode()))
        .await;

    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Accepted),
        "the voucher itself is good -- it advances the durable watermark"
    );
    assert_eq!(
        reject_code(&response),
        "F06",
        "anything else means the app served this for free"
    );
    assert!(response.headers.get(PAYMENT_REQUIRED_HEADER).is_some());
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
    let book = VoucherBook::new();
    book_at_fifty_thousand(&book).await;

    let route = priced_route();
    let app_client = serving_app(&route, b"served after the restart");
    let peer = accepting(payee_with_route(route, Arc::clone(&app_client)), &book);
    let (sealed, shared_secret) = sealed_prepare(25);

    let response = peer
        .handle(request(Some(&peer_voucher(50_025)), sealed.encode()))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert!(response.headers.get(PAYMENT_REQUIRED_HEADER).is_none());
    assert_eq!(
        opened_fulfil(&response, &shared_secret),
        b"served after the restart"
    );
    assert_eq!(app_client.deliveries().len(), 1);
}

// ─── ADR 0042 item 3: a forwarded arrival must cover its own `amount`,
// behind a per-peer knob that defaults to observing. The HTTP twins of the
// BTP carriage's own tests of the same names -- §0.1's one pipeline cannot
// admit over one carriage what it refuses over the other ───

/// **The default is still `observe`.** A peering that configures nothing
/// carries an **under-covered** forwarded arrival -- admitted, logged, and
/// actually forwarded to the next hop, which this node still pays on its own
/// outbound channel ([`covering`], issue #1145).
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_admitted_by_default() {
    let book = VoucherBook::new();
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
    let peer = accepting(connector, &book);
    let (sealed, shared_secret) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let response = peer
        .handle(request(
            Some(&peer_voucher(ARRIVING_AMOUNT - 1)),
            sealed.encode(),
        ))
        .await;

    assert_eq!(response.status, 200);
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
        "an admitted packet carries no greeting"
    );
    assert_eq!(
        opened_fulfil(&response, &shared_secret),
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
    let book = VoucherBook::new();
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
    let peer = accepting_with_enforcement(connector, &book, forwarded_enforcing());
    let (sealed, _) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let response = peer
        .handle(request(
            Some(&peer_voucher(ARRIVING_AMOUNT - 1)),
            sealed.encode(),
        ))
        .await;

    assert_eq!(
        response.status, 200,
        "a packet verdict, not a transport 4xx (§6.2)"
    );
    assert_eq!(reject_code(&response), "F06");
    let terms = parse_greeting(&base64_decode(
        response
            .headers
            .get(PAYMENT_REQUIRED_HEADER)
            .expect("the x402 greeting rode the response"),
    ))
    .expect("readable terms");
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
        let book = VoucherBook::new();
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
        let peer = accepting_with_enforcement(connector, &book, enforcement);
        let (sealed, shared_secret) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );

        let response = peer
            .handle(request(
                Some(&peer_voucher(ARRIVING_AMOUNT)),
                sealed.encode(),
            ))
            .await;

        assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
        assert!(response.headers.get(PAYMENT_REQUIRED_HEADER).is_none());
        assert_eq!(
            opened_fulfil(&response, &shared_secret),
            b"delivered by the next hop"
        );
        assert_eq!(next_hop_app.deliveries().len(), 1);
    }
}

/// **Which figure must be covered**, stated as the three near misses: not
/// the forwarded route's client-edge `price` (ADR 0028), not the post-fee
/// amount this hop passes on (the difference is its fee, ADR 0010), and not
/// one unit short. Only the arriving `amount` covers an arriving packet.
#[tokio::test]
async fn a_voucher_advancing_less_than_the_arriving_amount_never_covers_it() {
    for advance in [
        FORWARD_ROUTE_PRICE,
        ARRIVING_AMOUNT - FORWARD_FEE,
        ARRIVING_AMOUNT - 1,
    ] {
        let book = VoucherBook::new();
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee();
        let peer = accepting_with_enforcement(connector, &book, forwarded_enforcing());
        let (sealed, _) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );

        let response = peer
            .handle(request(Some(&peer_voucher(advance)), sealed.encode()))
            .await;

        // The voucher is perfectly valid and is still acknowledged: the two
        // verdicts stay independent (§6.2).
        assert_eq!(
            ack_on(&response),
            Some(ClaimAckOutcome::Accepted),
            "advance {advance}"
        );
        assert_eq!(reject_code(&response), "F06", "advance {advance}");
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
            "advance {advance}"
        );
        assert!(
            next_hop_app.deliveries().is_empty(),
            "advance {advance} was never carried"
        );
    }
}

/// ADR 0029's rule is **untouched** by ADR 0042, and since issue #1077 it
/// has no escape hatch: an arrival at a priced termination that does not
/// cover the route's price is refused under **every** setting a peering can
/// carry -- `forwarded_claim_enforcement` is the *forwarded* rule's knob.
#[tokio::test]
async fn no_peering_setting_admits_an_uncovered_arrival_at_a_priced_termination() {
    for forwarded in [
        connector_config::ForwardedClaimEnforcement::Observe,
        connector_config::ForwardedClaimEnforcement::Enforce,
    ] {
        let book = VoucherBook::new();
        let route = priced_route();
        let app_client = serving_app(&route, b"terminated here");
        let peer = accepting_with_enforcement(
            payee_with_route(route, Arc::clone(&app_client)),
            &book,
            Arc::new(ClaimEnforcementPolicy::of(vec![(PEER_ID, forwarded)])),
        );
        let (sealed, _) = sealed_prepare(25);

        let response = peer
            .handle(request(Some(&peer_voucher(1)), sealed.encode()))
            .await;

        assert_eq!(
            reject_code(&response),
            "F06",
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(app_client.deliveries().is_empty());
    }
}

// ─── §7.2: the voucher race, and its mitigation ───

/// §7.2: **no more than one voucher-bearing request in flight to a peer per
/// relation.** A peering's vouchers are cumulative on one outbound channel;
/// without this, parallel requests at 100 and 200 reach the payee's
/// watermark in either order and the loser is refused
/// `amount_not_advancing` for nothing.
#[tokio::test]
async fn only_one_voucher_bearing_request_is_in_flight_per_relation() {
    let book = VoucherBook::new();
    let client = Loopback::with_dwell(accepting(payee(), &book), Duration::from_millis(20));
    let transport = Arc::new(transport(Arc::clone(&client) as Arc<dyn PeerHttpClient>));

    let forwards = (1..=4).map(|step| {
        let transport = Arc::clone(&transport);
        let json = peer_voucher(step * 100);
        tokio::spawn(async move {
            transport
                .forward(PEER_ID, prepare("g.nowhere"), voucher(json))
                .await
                .ack
        })
    });
    let acks: Vec<ClaimAckOutcome> = futures_join(forwards).await;

    assert_eq!(
        client.peak_concurrent_vouchers.load(Ordering::SeqCst),
        1,
        "two voucher-bearing requests were in flight to one relation at once (§7.2)"
    );
    assert!(
        acks.iter().all(|ack| *ack == ClaimAckOutcome::Accepted),
        "serialized vouchers cannot race each other into amount_not_advancing: {acks:?}"
    );
}

async fn futures_join<T>(handles: impl IntoIterator<Item = tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("the task did not panic"));
    }
    results
}

fn base64_decode(value: &str) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.decode(value).expect("standard base64")
}
