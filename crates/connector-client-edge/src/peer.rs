//! **The peer carriages, mounted on this node's own listeners** (issue
//! #678's gap 1, `docs/protocol/peer-carriage-spec.md` §1, §3.1).
//!
//! `connector-peer-btp` and `connector-peer-http` answer a frame and a
//! request respectively and open no port between them -- deliberately, so
//! that everything they decide is provable without a socket. This module is
//! the other half: it binds them to the sockets this crate already serves.
//!
//! # There is no second listener, and that is the design
//!
//! `docs/operators/btp-peer-transport-bringup.md` states it as the
//! replacement for the deleted `peer_wire_addr`: peer carriages *"ride this
//! node's own listeners, not a second socket"*. §1.3 is why -- role MUST NOT
//! be inferred from *"the carriage, the listener, the port, or the bind
//! address"* -- and §3.1 is what makes it cheap: a peer PREPARE is *"the
//! same OER encodings `POST /ilp` already carries"*. So peer traffic arrives
//! on the very `POST /ilp` and `GET /ilp/btp` a client uses, and what tells
//! the two apart is [`connector_peer_btp::role_gate::decide_frame`] and
//! nothing else: a voucher on the arrival (or, for a packet that moves no
//! value, a peer-role challenge) whose x402 channel's voucher signer is
//! bound to a peering -- published by a runtime peering's self-description,
//! or named by a `[[peer_channels]]` row -- resolved through this node's own
//! claim gate (ADR 0075 decision 5). A `toon-channel` claim never makes an
//! arrival a peer's (#1380).
//!
//! # What this module does, in order
//!
//! 1. **Decides role from the arrival's own evidence, before anything else
//!    happens** (§1.5) -- before a watermark is consulted, before a packet
//!    is routed, before a fee is taken.
//! 2. **Dispatches a peer-role interaction** into
//!    [`connector_peer_http::PeerHttpState::handle`] or
//!    [`connector_peer_btp::PeerSession`], which own everything downstream.
//! 3. **Leaves a client-role interaction exactly as it was.** Not refused,
//!    not annotated, not routed anywhere new: the client edge's own path,
//!    unchanged, which is §1.6's *"MUST NOT refuse it for the assertion
//!    alone"* and §1.7's *"ignored, not rejected"* in the only form a shared
//!    listener can take.
//!
//! # The two carriages are exposed independently
//!
//! A carriage this node's `peer_expose` does not name is simply not built
//! here, and an arrival on it takes the client path whatever it carries.
//! That is not role inference (§1.3): the role is still decided by the
//! arrival's evidence alone, and what `expose` decides is *whether this node
//! offers peer handling on that wire at all* -- the same axis `peer_expose`
//! has always been (§2.1).

use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use connector_btp::BtpFrame;
use connector_config::{PeerCarriage, PeerConfig, PeerExposure};
use connector_peer_auth::{PeerAuthRefusal, PeerAuthRefusalLog};
use connector_peer_btp::role_gate::{self, FrameEvidence, VoucherEvidence};
use connector_peer_btp::{ClaimEnforcementPolicy, PeerAcceptPolicy, PeerCarriageState};
use connector_peer_http::{Headers, PeerHttpPolicy, PeerHttpState, PeerRequest};
use connector_runtime::Connector;

/// What a peeked frame's evidence says about a BTP session that is not yet
/// a peer session (§1.2, §1.5).
///
/// A verdict, not a decision the session inherits: the frame it was read
/// from is **not** consumed, and the session that takes over is handed that
/// same frame and decides its role from it again. Deciding twice over one
/// pure function is free, and the peer session re-decides on every frame
/// after it in any case -- role is a property of the frame.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BtpClaimVerdict {
    /// The frame proves a peering: hand this session, and this frame, to the
    /// peer carriage.
    Peer,
    /// The frame carries no evidence, or evidence that proves no peering: the
    /// session is a client's and stays one, exactly as before this module
    /// existed.
    Client,
}

/// The peer carriages this node exposes.
///
/// One value per node, built once from configuration and shared by every
/// interaction. A peering relation has one watermark per channel however
/// many paths it has (§2.5, I6): the claim gate's, which both carriages'
/// vouchers are judged by.
pub struct PeerCarriages {
    connector: Arc<Connector>,
    /// The receiving half vouchers and peer-role challenges are resolved
    /// through (ADR 0075 decision 5) -- this node's claim gate, in a wired
    /// node. `None`, and neither proves the peer role here.
    vouchers: Option<Arc<dyn VoucherEvidence>>,
    /// `Some` when `peer_expose` names `http`.
    http: Option<Arc<PeerHttpState>>,
    /// `Some` when `peer_expose` names `btp`.
    btp: Option<Arc<PeerCarriageState>>,
    /// §1.6's loud half, rate limited. Held here rather than inside either
    /// carriage because this module is where the decision that produced the
    /// refusal is made -- a peer-role interaction never reaches it, so the
    /// carriages' own logs stay silent by construction.
    refusals: Mutex<PeerAuthRefusalLog>,
}

impl PeerCarriages {
    /// Build the carriages `expose` names over this node's peerings,
    /// resolving vouchers and peer-role challenges through
    /// `vouchers` (ADR 0075 decision 5), or `None` when there is no peer
    /// handling to mount: `peer_expose = "neither"` (the default, and the
    /// NAT'd operator's case -- §2.1).
    ///
    /// A node with no `[[peers]]` table still mounts them: a peering
    /// established at runtime (`POST /peers`, ADR 0058) proves itself with a
    /// voucher whose signer it binds then (ADR 0075 decision 4), and a
    /// carriage built only for config peerings would never hear it. A
    /// config-declared peering's signer is bound at boot, from its
    /// `[[peer_channels]]` row (#1380). With no peering of either kind every
    /// interaction still decides `client`, and asks the receiving half
    /// nothing (`role_gate::decide_frame`).
    #[must_use]
    pub fn from_config(
        connector: Arc<Connector>,
        peers: &[PeerConfig],
        expose: PeerExposure,
        vouchers: Option<Arc<dyn VoucherEvidence>>,
    ) -> Option<Arc<PeerCarriages>> {
        if expose.is_empty() {
            return None;
        }
        // Issue #883 (B6): one migration state per peering, both carriages,
        // so a peering reachable over both is not `observe` on one and
        // `enforce` on the other depending on which carriage a packet
        // happened to arrive on.
        let enforcement = Arc::new(ClaimEnforcementPolicy::from_peers(peers));
        let http = expose.exposes(PeerCarriage::Http).then(|| {
            let state = PeerHttpState::new(
                Arc::clone(&connector),
                Arc::clone(&enforcement),
                // The shared listener reading of §1.10: this node serves
                // clients on the same socket, so a failed credential is an
                // ordinary client and never a `401` -- which would make the
                // check an oracle for which peer ids are configured (§1.6).
                PeerHttpPolicy {
                    mandatory_auth: false,
                },
            );
            Arc::new(match &vouchers {
                Some(vouchers) => state.with_voucher_evidence(Arc::clone(vouchers)),
                None => state,
            })
        });
        let btp = expose.exposes(PeerCarriage::Btp).then(|| {
            let state = PeerCarriageState::new(
                Arc::clone(&connector),
                enforcement,
                PeerAcceptPolicy {
                    mandatory_auth: false,
                    ..PeerAcceptPolicy::default()
                },
            );
            Arc::new(match &vouchers {
                Some(vouchers) => state.with_voucher_evidence(Arc::clone(vouchers)),
                None => state,
            })
        });
        Some(Arc::new(PeerCarriages {
            connector,
            vouchers,
            http,
            btp,
            refusals: Mutex::new(PeerAuthRefusalLog::default()),
        }))
    }

    /// The inbound BTP peer pipeline, for a caller that needs to serve
    /// frames a peer originates on a session **this node dialed** (§2.3).
    /// `None` when `peer_expose` does not name `btp`.
    #[must_use]
    pub fn btp_state(&self) -> Option<Arc<PeerCarriageState>> {
        self.btp.clone()
    }

    /// Answer one `POST /ilp` if -- and only if -- it is a peer
    /// interaction.
    ///
    /// `None` means "this is a client request": the caller runs its own
    /// path, unchanged and unaware. `Some` is either the peer carriage's
    /// answer or §1.5's `400` for an ambiguous credential.
    pub async fn handle_http(&self, headers: &HeaderMap, body: &[u8]) -> Option<Response> {
        // A carriage this node does not expose is not peer handling that
        // failed -- it is peer handling that is not offered here, so the
        // request is a client's and its claim is never read as a peering's.
        let http = self.http.as_ref()?;

        let request = PeerRequest {
            headers: peer_headers(headers),
            body: body.to_vec(),
        };
        // §1.2: the evidence on this request, resolved and verified. The
        // peer handler decides again from the same evidence, which lets
        // that handler stand alone on its own listener (§1.10). A request
        // whose evidence is ambiguous (§1.5) proves no peering here, and
        // the client path answers it.
        let evidence = connector_peer_http::evidence_on(&request)?;
        let (role, refusal) = self.decide(&evidence).await.into_parts();
        self.log_refusal(refusal.as_ref());
        if !role.is_peer() {
            return None;
        }

        Some(into_axum(http.handle(request).await))
    }

    /// What a BTP frame's voucher or peer-role challenge means for a session
    /// that is still a client (§1.2, §1.5). The frame is peeked,
    /// never consumed: see [`BtpClaimVerdict`].
    pub(crate) async fn btp_claim_verdict(&self, frame: &BtpFrame) -> BtpClaimVerdict {
        if self.btp.is_none() {
            return BtpClaimVerdict::Client;
        }
        // Ambiguous evidence proves no peering here; a peer session would
        // refuse the frame (§1.5), and the client path answers it instead.
        let Ok(evidence) = role_gate::btp_evidence(frame) else {
            return BtpClaimVerdict::Client;
        };
        let (role, refusal) = self.decide(&evidence).await.into_parts();
        self.log_refusal(refusal.as_ref());
        if role.is_peer() {
            BtpClaimVerdict::Peer
        } else {
            BtpClaimVerdict::Client
        }
    }

    /// One arrival's role, from its evidence -- the same call both carriages
    /// make (`role_gate::decide_frame`).
    async fn decide(&self, evidence: &FrameEvidence) -> connector_peer_auth::RoleDecision {
        role_gate::decide_frame(&self.connector, self.vouchers.as_deref(), evidence).await
    }

    /// §1.6: a voucher or challenge from a bound signer's channel that does
    /// not verify is an *assertion*. The arrival is a client's and is **not** refused
    /// for the assertion alone -- but a silent downgrade would present to
    /// an operator as "peering configured, nothing peers, no error
    /// anywhere", so the rate-limited event is what stops that.
    fn log_refusal(&self, refusal: Option<&PeerAuthRefusal>) {
        let Some(refusal) = refusal else {
            return;
        };
        let report = self
            .refusals
            .lock()
            .expect("peer auth refusal log poisoned")
            .observe(refusal, crate::now_unix().saturating_mul(1_000));
        if let Some(report) = report {
            tracing::warn!(
                event = report.event,
                peer_id = %report.peer_id,
                unmet = report.unmet.name(),
                suppressed = report.suppressed,
                "a peer channel's voucher did not verify; the arrival is a client's"
            );
        }
    }
}

/// An axum [`HeaderMap`] as the carriage's own [`Headers`], multiplicity
/// intact -- §1.5 refuses a second `Toon-Peer-Auth` rather than resolving
/// it, and §6.4 lets `Toon-Flush-Requested` appear once per channel, so a
/// map keyed by name would answer the first question wrong.
///
/// A header whose bytes are not text is dropped rather than refused: §3
/// requires anything it does not name be ignored on receipt, and every
/// header it *does* name is ASCII.
fn peer_headers(headers: &HeaderMap) -> Headers {
    let mut out = Headers::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            out.push(name.as_str(), value);
        }
    }
    out
}

/// The carriage's answer, as axum sees it. **The status is the carriage's**
/// (§6.2): `200` regardless of a claim's verdict, and `4xx` only where
/// there is no ILP answer at all.
fn into_axum(response: connector_peer_http::PeerResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut out = Response::builder().status(status);
    if response.status == 200 {
        out = out.header(axum::http::header::CONTENT_TYPE, crate::OCTET_STREAM);
    }
    for (name, value) in response.headers.iter() {
        out = out.header(name, value);
    }
    out.body(axum::body::Body::from(response.body))
        .expect("a peer response's headers are carriage-generated ASCII")
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_btp::ProtocolData;
    use connector_runtime::{FakeAppClient, InProcessPeerTransport, SystemClock};

    const PEER_ID: &str = "store";

    /// A real loaded [`connector_config::Config`] carrying one peering in
    /// the shape ADR 0075 gives it (issue #1380), rather than hand-built
    /// values: `PeerConfig` is constructible only by config load precisely
    /// so a value that exists is one the loader would produce.
    fn peering() -> connector_config::Config {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        std::io::Write::write_all(&mut key_file, &[7u8; 32]).expect("write key file");
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
        std::io::Write::write_all(
            &mut config_file,
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "{PEER_ID}"
endpoint = "wss://peer.example:443/ilp/btp"

[[peer_channels]]
peer_id = "{PEER_ID}"
voucher_signer = "0x7777777777777777777777777777777777777777"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[settlement.evm.batch_settlement]
asset_eip712_name = "USDC"
asset_eip712_version = "2"
"#,
                state_dir = state_dir.path().display(),
                key_file = key_file.path().display(),
            )
            .as_bytes(),
        )
        .expect("write config file");
        connector_config::Config::load(config_file.path()).expect("load the peering config")
    }

    /// §2.1: `peer_expose = "neither"` -- the default, and the NAT'd
    /// operator -- mounts no peer handling at all, so every interaction on
    /// this node's listeners is a client's.
    #[test]
    fn a_node_that_exposes_nothing_mounts_no_peer_carriage() {
        let config = peering();
        let connector = Arc::new(Connector::new(
            Vec::new(),
            Vec::new(),
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(SystemClock),
        ));
        assert!(
            PeerCarriages::from_config(connector, config.peers(), PeerExposure::Neither, None)
                .is_none()
        );
    }

    /// A BTP MESSAGE carrying `protocol_data` and `ilp_packet`, as the front
    /// door peeks one.
    fn message(protocol_data: Vec<ProtocolData>, ilp_packet: &[u8]) -> BtpFrame {
        BtpFrame {
            frame_type: connector_btp::BTP_MESSAGE,
            request_id: 1,
            amount: None,
            protocol_data,
            ilp_packet: ilp_packet.to_vec(),
        }
    }

    /// ADR 0075 decision 5 (issue #1377): the peer role proven by an x402
    /// voucher, or -- for a packet that moves no value -- by the voucher
    /// claim-state challenge, from a channel whose voucher signer is bound
    /// to the peering. Over both carriages, through the real claim gate and
    /// a fake of the batch-settlement seam that holds the one fact a backend
    /// owns: which channels exist, and whose key signs on each.
    mod x402 {
        use super::*;

        use async_trait::async_trait;
        use base64::Engine;
        use connector_domain::{Prepare, Reject};
        use connector_peer_btp::challenge_json::{self, PeerRoleChallenge};
        use connector_peer_btp::PeerSession;
        use connector_runtime::{InMemoryJournal, VoucherSigner};
        use connector_signer::{
            derive_evm_address, evm_batch_channel_id, evm_voucher_claim_state_challenge_digest,
            evm_voucher_digest, solana_voucher_claim_state_challenge_message,
            solana_voucher_message, BatchChannelConfig, BatchSettlementDomain,
        };
        use ed25519_dalek::Signer as _;
        use libsecp256k1::{Message, PublicKey, SecretKey};

        use crate::{
            AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
            ChannelResolutionError, ClientChannelRegistry, ClientClaimGate,
        };

        const CHAIN: u64 = 84_532;

        fn domain() -> BatchSettlementDomain {
            BatchSettlementDomain::x402(CHAIN)
        }

        /// The peer's EVM settlement key: its channel's `payerAuthorizer`
        /// (ADR 0075 decision 3, `payerAuthorizer == payer`).
        fn peer_key() -> SecretKey {
            SecretKey::parse(&[0x0a; 32]).expect("valid secret")
        }

        /// Somebody else, with a channel of their own toward this node.
        fn stranger_key() -> SecretKey {
            SecretKey::parse(&[0x0b; 32]).expect("valid secret")
        }

        fn address_of(secret: &SecretKey) -> [u8; 20] {
            derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
        }

        fn config_of(secret: &SecretKey) -> BatchChannelConfig {
            let payer = address_of(secret);
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

        fn evm_channel(secret: &SecretKey) -> [u8; 32] {
            evm_batch_channel_id(&domain(), &config_of(secret))
        }

        fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> [u8; 65] {
            let (signature, recovery) = libsecp256k1::sign(&Message::parse(digest), secret);
            let mut bytes = [0u8; 65];
            bytes[..64].copy_from_slice(&signature.serialize());
            bytes[64] = recovery.serialize() + 27;
            bytes
        }

        fn ed25519(seed: u8) -> ed25519_dalek::Keypair {
            let secret = ed25519_dalek::SecretKey::from_bytes(&[seed; 32]).expect("seed");
            let public = (&secret).into();
            ed25519_dalek::Keypair { secret, public }
        }

        /// The peer's Solana settlement key, and the account of its channel.
        fn peer_ed25519() -> ed25519_dalek::Keypair {
            ed25519(0x21)
        }
        const PEER_ACCOUNT: [u8; 32] = [0xc3; 32];
        const STRANGER_ACCOUNT: [u8; 32] = [0xc4; 32];

        /// Two channels on each chain, one the peer's and one a stranger's.
        #[derive(Debug)]
        struct TwoChannelsEachChain;

        #[async_trait]
        impl BatchSettlementChannels for TwoChannelsEachChain {
            fn evm_domain(&self) -> Option<BatchSettlementDomain> {
                Some(domain())
            }

            fn accepts_solana(&self) -> bool {
                true
            }

            async fn evm(
                &self,
                channel_id: &[u8; 32],
                presented_config: Option<&BatchChannelConfig>,
            ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
                // An EVM channel is found from its config alone: the chain
                // stores it by id.
                let Some(config) = presented_config else {
                    return Ok(None);
                };
                let known = [config_of(&peer_key()), config_of(&stranger_key())];
                Ok(known
                    .into_iter()
                    .find(|known| {
                        known == config && evm_batch_channel_id(&domain(), known) == *channel_id
                    })
                    .map(|config| AdmittedEvmVoucherChannel {
                        config,
                        max_cumulative: 1_000_000,
                    }))
            }

            async fn solana(
                &self,
                channel_account: &[u8; 32],
            ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
                let signer = match *channel_account {
                    PEER_ACCOUNT => peer_ed25519(),
                    STRANGER_ACCOUNT => ed25519(0x22),
                    _ => return Ok(None),
                };
                Ok(Some(AdmittedSolanaVoucherChannel {
                    authorized_signer: signer.public.to_bytes(),
                    max_cumulative: 1_000_000,
                }))
            }
        }

        fn config_json(config: &BatchChannelConfig) -> serde_json::Value {
            serde_json::json!({
                "payer": format!("0x{}", hex::encode(config.payer)),
                "payerAuthorizer": format!("0x{}", hex::encode(config.payer_authorizer)),
                "receiver": format!("0x{}", hex::encode(config.receiver)),
                "receiverAuthorizer": format!("0x{}", hex::encode(config.receiver_authorizer)),
                "token": format!("0x{}", hex::encode(config.token)),
                "withdrawDelay": config.withdraw_delay,
                "salt": format!("0x{}", hex::encode(config.salt)),
            })
        }

        /// An EVM voucher on `owner`'s channel for `amount`, whose signature
        /// is `signature`.
        fn evm_voucher(owner: &SecretKey, amount: u64, signature: &[u8; 65]) -> String {
            serde_json::json!({
                "version": "1.0",
                "blockchain": "evm",
                "scheme": "batch-settlement",
                "messageId": format!("voucher-{amount}"),
                "timestamp": "2026-09-27T12:00:00.000Z",
                "senderId": "peer",
                "channelId": format!("0x{}", hex::encode(evm_channel(owner))),
                "maxClaimableAmount": amount.to_string(),
                "signature": format!("0x{}", hex::encode(signature)),
                "channelConfig": config_json(&config_of(owner)),
            })
            .to_string()
        }

        /// `owner`'s genuine voucher on its own channel.
        fn signed_evm_voucher(owner: &SecretKey, amount: u64) -> String {
            let digest = evm_voucher_digest(&domain(), &evm_channel(owner), u128::from(amount));
            evm_voucher(owner, amount, &sign_evm(owner, &digest))
        }

        fn solana_voucher(account: [u8; 32], amount: u64, signature: &[u8; 64]) -> String {
            serde_json::json!({
                "version": "1.0",
                "blockchain": "solana",
                "scheme": "batch-settlement",
                "messageId": format!("voucher-{amount}"),
                "timestamp": "2026-09-27T12:00:00Z",
                "senderId": "peer",
                "channelId": bs58::encode(account).into_string(),
                "maxClaimableAmount": amount.to_string(),
                "expiresAt": 0,
                "signature": bs58::encode(signature).into_string(),
            })
            .to_string()
        }

        fn signed_solana_voucher(
            signer: &ed25519_dalek::Keypair,
            account: [u8; 32],
            amount: u64,
        ) -> String {
            let signature = signer.sign(&solana_voucher_message(&account, amount, 0));
            solana_voucher(account, amount, &signature.to_bytes())
        }

        /// `owner`'s channel, challenged until `expires`, over a signature
        /// `signer` made of the challenge message.
        fn evm_challenge(owner: &SecretKey, signer: &SecretKey, expires: u64) -> String {
            let channel_id = evm_channel(owner);
            let digest = evm_voucher_claim_state_challenge_digest(&domain(), &channel_id, expires);
            challenge_json::encode(&PeerRoleChallenge::Evm {
                channel_id,
                expires,
                signature: sign_evm(signer, &digest),
                channel_config: Some(evm_challenge_config(owner)),
            })
        }

        fn evm_challenge_config(
            owner: &SecretKey,
        ) -> connector_domain::client_claim::EvmVoucherChannelConfig {
            connector_domain::client_claim::parse_evm_channel_config(&config_json(&config_of(
                owner,
            )))
            .expect("a well-formed config")
        }

        fn solana_challenge(
            signer: &ed25519_dalek::Keypair,
            account: [u8; 32],
            expires: u64,
        ) -> String {
            let message = solana_voucher_claim_state_challenge_message(&account, expires);
            challenge_json::encode(&PeerRoleChallenge::Solana {
                channel_account: account,
                expires,
                signature: signer.sign(&message).to_bytes(),
            })
        }

        fn now() -> u64 {
            crate::now_unix()
        }

        fn prepare(amount: u64) -> Vec<u8> {
            Prepare {
                amount,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
                greeting: false,
                destination: "g.nowhere.app".to_string(),
                data: Vec::new(),
            }
            .encode()
        }

        /// A node with one peering, `store`, and the claim gate as its
        /// receiving half. `bind` names the peer's settlement keys this node
        /// binds to it -- the operation a runtime peering's self-description
        /// (#1378, #1379) and a `[[peer_channels]]` row (#1380) both feed.
        fn node(bind: &[VoucherSigner]) -> (Arc<Connector>, Arc<PeerCarriages>) {
            node_exposing(PeerExposure::Both, bind)
        }

        fn node_exposing(
            expose: PeerExposure,
            bind: &[VoucherSigner],
        ) -> (Arc<Connector>, Arc<PeerCarriages>) {
            let config = peering();
            let connector = Arc::new(
                Connector::new(
                    Vec::new(),
                    Vec::new(),
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    Arc::new(SystemClock),
                )
                .with_config_peer_ids([PEER_ID.to_string()]),
            );
            for signer in bind {
                connector
                    .bind_voucher_signer(PEER_ID, *signer)
                    .expect("store is a configured peering");
            }
            let gate = ClientClaimGate::restore(
                ClientChannelRegistry::new(),
                Arc::new(InMemoryJournal::new()),
            )
            .expect("an empty journal")
            .with_batch_settlement(Arc::new(TwoChannelsEachChain));
            let carriages = PeerCarriages::from_config(
                Arc::clone(&connector),
                config.peers(),
                expose,
                Some(Arc::new(gate) as Arc<dyn VoucherEvidence>),
            )
            .expect("a carriage is exposed");
            (connector, carriages)
        }

        fn bound() -> (Arc<Connector>, Arc<PeerCarriages>) {
            node(&[
                VoucherSigner::Evm(address_of(&peer_key())),
                VoucherSigner::Solana(peer_ed25519().public.to_bytes()),
            ])
        }

        /// What one arrival is, on each carriage: the front door's verdict
        /// **and** the peer handler's own, so a handler that re-decided
        /// differently from its front door would show here.
        #[derive(Debug, PartialEq, Eq)]
        enum Seen {
            Peer,
            Client,
        }

        /// HTTP: `POST /ilp` carrying a claim slot or a challenge slot.
        async fn over_http(
            carriages: &PeerCarriages,
            claim: Option<&str>,
            challenge: Option<&str>,
            body: &[u8],
        ) -> Seen {
            let mut headers = HeaderMap::new();
            if let Some(claim) = claim {
                headers.insert(
                    connector_btp::CLAIM_HEADER,
                    connector_peer_http::headers::claim_header_value(claim)
                        .parse()
                        .unwrap(),
                );
            }
            if let Some(challenge) = challenge {
                headers.insert(
                    connector_btp::PEER_CHALLENGE_HEADER,
                    connector_peer_http::headers::peer_challenge_header_value(challenge)
                        .parse()
                        .unwrap(),
                );
            }
            let Some(response) = carriages.handle_http(&headers, body).await else {
                return Seen::Client;
            };
            let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
            assert!(
                !answered_as_a_client(&body),
                "the front door decided peer and the HTTP peer handler decided client"
            );
            Seen::Peer
        }

        /// BTP: a MESSAGE carrying `entries` and `packet`, through the front
        /// door's verdict and then a peer session that answers it.
        async fn over_btp(
            carriages: &PeerCarriages,
            entries: Vec<ProtocolData>,
            packet: &[u8],
        ) -> Seen {
            let frame = message(entries, packet);
            if carriages.btp_claim_verdict(&frame).await == BtpClaimVerdict::Client {
                return Seen::Client;
            }
            let (replies, mut answers) = tokio::sync::mpsc::channel(4);
            let mut session = PeerSession::new(carriages.btp_state().expect("btp"), replies);
            session
                .handle_frame(&connector_btp::encode_message(
                    frame.request_id,
                    &frame.protocol_data,
                    &frame.ilp_packet,
                ))
                .await
                .expect("the session is live");
            let answer = answers.recv().await.expect("an answer");
            let answer = connector_btp::decode_frame(&answer).expect("a frame");
            assert!(
                !answered_as_a_client(&answer.ilp_packet),
                "the front door decided peer and the BTP peer session decided client"
            );
            Seen::Peer
        }

        /// Whether `packet` is the REJECT a peer carriage gives an
        /// interaction it decided was a client's.
        fn answered_as_a_client(packet: &[u8]) -> bool {
            Reject::decode(packet)
                .is_ok_and(|reject| reject.message == "no peer route for this interaction")
        }

        fn entry(name: &str, json: &str) -> ProtocolData {
            ProtocolData {
                name: name.to_string(),
                content_type: connector_btp::CONTENT_TYPE_TEXT,
                data: json.as_bytes().to_vec(),
            }
        }

        /// A voucher, on both carriages: `(http, btp)`.
        async fn voucher_role(carriages: &PeerCarriages, voucher: &str) -> (Seen, Seen) {
            (
                over_http(carriages, Some(voucher), None, &prepare(500)).await,
                over_btp(
                    carriages,
                    vec![entry(connector_btp::CLAIM_PROTOCOL, voucher)],
                    &prepare(500),
                )
                .await,
            )
        }

        /// A challenge riding a PREPARE for `amount`, on both carriages.
        async fn challenge_role(
            carriages: &PeerCarriages,
            challenge: &str,
            amount: u64,
        ) -> (Seen, Seen) {
            (
                over_http(carriages, None, Some(challenge), &prepare(amount)).await,
                over_btp(
                    carriages,
                    vec![entry(connector_btp::PEER_CHALLENGE_PROTOCOL, challenge)],
                    &prepare(amount),
                )
                .await,
            )
        }

        const PEER: (Seen, Seen) = (Seen::Peer, Seen::Peer);
        const CLIENT: (Seen, Seen) = (Seen::Client, Seen::Client);

        #[tokio::test]
        async fn a_voucher_on_a_bound_channel_decides_peer_on_btp_and_on_http() {
            let (_, carriages) = bound();

            assert_eq!(
                voucher_role(&carriages, &signed_evm_voucher(&peer_key(), 500)).await,
                PEER
            );
            assert_eq!(
                voucher_role(
                    &carriages,
                    &signed_solana_voucher(&peer_ed25519(), PEER_ACCOUNT, 500)
                )
                .await,
                PEER
            );
        }

        /// A genuine voucher, correctly signed, on a channel whose signer is
        /// bound to no peering: an ordinary client paying, on both chains.
        /// Expose and dial are separate axes (§2.1), and so is each carriage
        /// from the other: a node exposing only BTP offers no peer handling
        /// on `POST /ilp`, whatever a request carries -- a voucher that would
        /// prove the peering on BTP included.
        #[tokio::test]
        async fn a_carriage_this_node_does_not_expose_never_reads_a_voucher_as_a_peerings() {
            let (_, carriages) = node_exposing(
                PeerExposure::Btp,
                &[VoucherSigner::Evm(address_of(&peer_key()))],
            );
            let voucher = signed_evm_voucher(&peer_key(), 500);

            assert_eq!(
                over_http(&carriages, Some(&voucher), None, &prepare(500)).await,
                Seen::Client
            );
            assert_eq!(
                over_btp(
                    &carriages,
                    vec![entry(connector_btp::CLAIM_PROTOCOL, &voucher)],
                    &prepare(500)
                )
                .await,
                Seen::Peer
            );
        }

        /// §1.4, ADR 0060: a `Toon-Peer-Auth` header is **ignored**, never
        /// refused. A request still setting one is read exactly as one that
        /// does not -- the voucher decides, and only the voucher.
        #[tokio::test]
        async fn a_lingering_peer_auth_header_is_ignored_and_decides_nothing() {
            let (_, carriages) = bound();
            let stale = "eyJwZWVySWQiOiJzdG9yZSIsInNlY3JldCI6ImFueXRoaW5nIn0=";
            let voucher = signed_evm_voucher(&peer_key(), 500);

            let mut with_voucher = HeaderMap::new();
            with_voucher.insert(
                connector_btp::CLAIM_HEADER,
                connector_peer_http::headers::claim_header_value(&voucher)
                    .parse()
                    .unwrap(),
            );
            with_voucher.insert("toon-peer-auth", stale.parse().unwrap());
            let mut without_voucher = HeaderMap::new();
            without_voucher.insert("toon-peer-auth", stale.parse().unwrap());

            assert!(
                carriages
                    .handle_http(&with_voucher, &prepare(500))
                    .await
                    .is_some(),
                "the header changes nothing about a request whose voucher proves the peering"
            );
            assert!(
                carriages
                    .handle_http(&without_voucher, &prepare(500))
                    .await
                    .is_none(),
                "and nothing about one that carries none: it is a client request"
            );
        }

        /// ADR 0075, issue #1380: **a `toon-channel` claim never decides the
        /// peer role, on either carriage** -- not even one genuinely signed
        /// by the very key this node has bound to the peering, which is the
        /// key a `toon-channel` peer claim used to be verified against. The
        /// frame is a client's, and the client edge answers it.
        #[tokio::test]
        async fn a_toon_channel_claim_never_decides_the_peer_role_on_either_carriage() {
            let (_, carriages) = bound();
            let signer = connector_signer::LocalSigner::from_secret_bytes(
                "bound-peer-settlement-key",
                [0x0a; 32],
            )
            .expect("the peer's key");
            assert_eq!(
                derive_evm_address(&connector_signer::Signer::public_key(&signer).unwrap()),
                address_of(&peer_key()),
                "the claim is signed by the peer's own bound key"
            );
            let channel = [0x11; 32];
            let digest =
                connector_signer::evm_balance_proof_digest(&connector_signer::EvmBalanceProof {
                    channel_id: channel,
                    nonce: 1,
                    transferred_amount: 500,
                    locked_amount: 0,
                    locks_root: [0; 32],
                    chain_id: CHAIN,
                    token_network_address: [0xbb; 20],
                });
            let claim = connector_runtime::WireClaim {
                channel_id: format!("0x{}", hex::encode(channel)),
                nonce: 1,
                cumulative_amount: 500,
                signature: connector_runtime::ClaimSignature::Evm(
                    connector_signer::Signer::sign(&signer, &digest).expect("sign"),
                ),
            };
            let toon_claim = connector_peer_btp::claim_json::encode(
                &claim,
                &address_of(&peer_key()),
                None,
                None,
                Some(connector_peer_btp::PeerClaimDomain {
                    chain_id: CHAIN,
                    token_network: [0xbb; 20],
                }),
                "toon-claim-1",
                "2026-09-28T00:00:00.000Z",
            );

            for packet in [prepare(500), prepare(0), Vec::new()] {
                assert_eq!(
                    over_http(&carriages, Some(&toon_claim), None, &packet).await,
                    Seen::Client,
                    "over HTTP"
                );
                assert_eq!(
                    over_btp(
                        &carriages,
                        vec![entry(connector_btp::CLAIM_PROTOCOL, &toon_claim)],
                        &packet
                    )
                    .await,
                    Seen::Client,
                    "over BTP"
                );
            }
            // Control: the same bound key's voucher does decide the role.
            let voucher = signed_evm_voucher(&peer_key(), 500);
            assert_eq!(
                over_http(&carriages, Some(&voucher), None, &prepare(500)).await,
                Seen::Peer
            );
        }

        /// A `[[peer_channels]]` row that pins its `inbound_channel`
        /// (ADR 0075 decision 9): the bound signer proves the peering on
        /// that channel and on no other, even one whose vouchers it
        /// genuinely signs.
        #[tokio::test]
        async fn a_signer_pinned_to_one_inbound_channel_proves_nothing_on_another() {
            let pinned_to = |channel: String| {
                let config = peering();
                let connector = Arc::new(
                    Connector::new(
                        Vec::new(),
                        Vec::new(),
                        Arc::new(FakeAppClient::new()),
                        Arc::new(InProcessPeerTransport::new()),
                        Arc::new(SystemClock),
                    )
                    .with_config_peer_ids([PEER_ID.to_string()])
                    .with_config_voucher_signer(
                        PEER_ID,
                        connector_runtime::SettlementChain::Evm,
                        &format!("0x{}", hex::encode(address_of(&peer_key()))),
                        Some(&channel),
                    )
                    .expect("store is a configured peering"),
                );
                let gate = ClientClaimGate::restore(
                    ClientChannelRegistry::new(),
                    Arc::new(InMemoryJournal::new()),
                )
                .expect("an empty journal")
                .with_batch_settlement(Arc::new(TwoChannelsEachChain));
                PeerCarriages::from_config(
                    connector,
                    config.peers(),
                    PeerExposure::Both,
                    Some(Arc::new(gate) as Arc<dyn VoucherEvidence>),
                )
                .expect("exposed")
            };
            let voucher = signed_evm_voucher(&peer_key(), 500);

            let on_its_channel = pinned_to(format!("0x{}", hex::encode(evm_channel(&peer_key()))));
            assert_eq!(
                over_http(&on_its_channel, Some(&voucher), None, &prepare(500)).await,
                Seen::Peer
            );
            let elsewhere = pinned_to(format!("0x{}", "99".repeat(32)));
            assert_eq!(
                over_http(&elsewhere, Some(&voucher), None, &prepare(500)).await,
                Seen::Client,
                "a genuine voucher by the bound key, on a channel the row does not name"
            );
            assert_eq!(
                over_btp(
                    &elsewhere,
                    vec![entry(connector_btp::CLAIM_PROTOCOL, &voucher)],
                    &prepare(500)
                )
                .await,
                Seen::Client
            );
        }

        #[tokio::test]
        async fn a_voucher_on_a_channel_not_bound_to_any_peer_never_decides_peer() {
            let (_, carriages) = bound();
            assert_eq!(
                voucher_role(&carriages, &signed_evm_voucher(&stranger_key(), 500)).await,
                CLIENT
            );
            assert_eq!(
                voucher_role(
                    &carriages,
                    &signed_solana_voucher(&ed25519(0x22), STRANGER_ACCOUNT, 500)
                )
                .await,
                CLIENT
            );

            // And the peer's own channel, on a node that has bound nothing.
            let (_, unbound) = node(&[]);
            assert_eq!(
                voucher_role(&unbound, &signed_evm_voucher(&peer_key(), 500)).await,
                CLIENT
            );
        }

        /// The chain's signer decides, never the voucher's: a voucher on the
        /// peer's channel signed by somebody else is a client's.
        #[tokio::test]
        async fn a_voucher_on_a_bound_channel_signed_by_another_key_is_a_client() {
            let (_, carriages) = bound();
            let digest = evm_voucher_digest(&domain(), &evm_channel(&peer_key()), 500);
            let forged = evm_voucher(&peer_key(), 500, &sign_evm(&stranger_key(), &digest));

            assert_eq!(voucher_role(&carriages, &forged).await, CLIENT);
        }

        #[tokio::test]
        async fn a_challenge_from_a_bound_channels_signer_decides_peer_for_a_zero_value_packet() {
            let (_, carriages) = bound();
            let expires = now() + 60;

            assert_eq!(
                challenge_role(
                    &carriages,
                    &evm_challenge(&peer_key(), &peer_key(), expires),
                    0
                )
                .await,
                PEER
            );
            assert_eq!(
                challenge_role(
                    &carriages,
                    &solana_challenge(&peer_ed25519(), PEER_ACCOUNT, expires),
                    0
                )
                .await,
                PEER
            );
        }

        #[tokio::test]
        async fn an_expired_challenge_or_one_from_the_wrong_key_does_not_decide_peer() {
            let (_, carriages) = bound();
            let cases = [
                (
                    "expired",
                    evm_challenge(&peer_key(), &peer_key(), now() - 1),
                ),
                (
                    "further ahead than the node accepts",
                    evm_challenge(
                        &peer_key(),
                        &peer_key(),
                        now()
                            + connector_peer_btp::role_gate::MAX_PEER_CHALLENGE_LIFETIME_SECS
                            + 60,
                    ),
                ),
                (
                    "signed by a key that is not the channel's",
                    evm_challenge(&peer_key(), &stranger_key(), now() + 60),
                ),
                (
                    "on a Solana channel, signed by another key",
                    solana_challenge(&ed25519(0x22), PEER_ACCOUNT, now() + 60),
                ),
                (
                    "expired, on Solana",
                    solana_challenge(&peer_ed25519(), PEER_ACCOUNT, now() - 1),
                ),
                (
                    "from an unbound channel's own signer",
                    evm_challenge(&stranger_key(), &stranger_key(), now() + 60),
                ),
            ];
            for (case, challenge) in cases {
                assert_eq!(
                    challenge_role(&carriages, &challenge, 0).await,
                    CLIENT,
                    "{case}"
                );
            }
        }

        /// A challenge proves the role only for a packet that moves no value
        /// (ADR 0075 decision 5): a paying packet carries a voucher.
        #[tokio::test]
        async fn a_challenge_on_a_packet_that_moves_value_does_not_decide_peer() {
            let (_, carriages) = bound();
            let challenge = evm_challenge(&peer_key(), &peer_key(), now() + 60);

            assert_eq!(challenge_role(&carriages, &challenge, 1).await, CLIENT);
        }

        /// The connector-signer separation, now for the peer-role use: a
        /// challenge signature presented as a voucher, and a voucher
        /// signature presented as a challenge, each with the one number they
        /// sign lined up, prove nothing -- on either chain.
        #[tokio::test]
        async fn a_challenge_is_never_accepted_as_a_voucher_nor_a_voucher_as_a_challenge() {
            let (_, carriages) = bound();
            let expires = now() + 60;
            let channel_id = evm_channel(&peer_key());

            let challenge_signature = sign_evm(
                &peer_key(),
                &evm_voucher_claim_state_challenge_digest(&domain(), &channel_id, expires),
            );
            let challenge_as_voucher = evm_voucher(&peer_key(), expires, &challenge_signature);
            assert_eq!(
                voucher_role(&carriages, &challenge_as_voucher).await,
                CLIENT
            );

            let voucher_signature = sign_evm(
                &peer_key(),
                &evm_voucher_digest(&domain(), &channel_id, u128::from(expires)),
            );
            let voucher_as_challenge = challenge_json::encode(&PeerRoleChallenge::Evm {
                channel_id,
                expires,
                signature: voucher_signature,
                channel_config: Some(evm_challenge_config(&peer_key())),
            });
            assert_eq!(
                challenge_role(&carriages, &voucher_as_challenge, 0).await,
                CLIENT
            );

            let signer = peer_ed25519();
            let solana_challenge_signature = signer
                .sign(&solana_voucher_claim_state_challenge_message(
                    &PEER_ACCOUNT,
                    expires,
                ))
                .to_bytes();
            assert_eq!(
                voucher_role(
                    &carriages,
                    &solana_voucher(PEER_ACCOUNT, expires, &solana_challenge_signature)
                )
                .await,
                CLIENT
            );
            let solana_voucher_signature = signer
                .sign(&solana_voucher_message(&PEER_ACCOUNT, expires, 0))
                .to_bytes();
            let solana_voucher_as_challenge = challenge_json::encode(&PeerRoleChallenge::Solana {
                channel_account: PEER_ACCOUNT,
                expires,
                signature: solana_voucher_signature,
            });
            assert_eq!(
                challenge_role(&carriages, &solana_voucher_as_challenge, 0).await,
                CLIENT
            );
        }

        /// §1.5's smuggling defence, extended: a claim beside a challenge is
        /// two pieces of authentication material, and a peer handler refuses
        /// the request rather than choose one.
        #[tokio::test]
        async fn a_voucher_beside_a_challenge_is_refused_not_resolved() {
            let (_, carriages) = bound();
            let http = carriages.http.clone().expect("http is exposed");
            let mut headers = connector_peer_http::Headers::new();
            headers.push(
                connector_btp::CLAIM_HEADER,
                connector_peer_http::headers::claim_header_value(&signed_evm_voucher(
                    &peer_key(),
                    500,
                )),
            );
            headers.push(
                connector_btp::PEER_CHALLENGE_HEADER,
                connector_peer_http::headers::peer_challenge_header_value(&evm_challenge(
                    &peer_key(),
                    &peer_key(),
                    now() + 60,
                )),
            );
            let response = http
                .handle(connector_peer_http::PeerRequest {
                    headers,
                    body: prepare(0),
                })
                .await;
            assert_eq!(response.status, 400);

            let (replies, mut answers) = tokio::sync::mpsc::channel(4);
            let mut session = PeerSession::new(carriages.btp_state().expect("btp"), replies);
            session
                .handle_frame(&connector_btp::encode_message(
                    7,
                    &[
                        entry(
                            connector_btp::CLAIM_PROTOCOL,
                            &signed_evm_voucher(&peer_key(), 500),
                        ),
                        entry(
                            connector_btp::PEER_CHALLENGE_PROTOCOL,
                            &evm_challenge(&peer_key(), &peer_key(), now() + 60),
                        ),
                    ],
                    &prepare(0),
                ))
                .await
                .expect("the session is live");
            let answer = connector_btp::decode_frame(&answers.recv().await.expect("an answer"))
                .expect("a frame");
            assert_eq!(answer.frame_type, connector_btp::BTP_ERROR);
        }

        /// A binding names a relation this node has: a signer bound to a peer
        /// id no peering holds would decide a role that routes nowhere.
        #[tokio::test]
        async fn a_signer_can_only_be_bound_to_a_peering_that_exists() {
            let (connector, _) = node(&[]);
            assert_eq!(
                connector.bind_voucher_signer("ghost", VoucherSigner::Evm([1; 20])),
                Err(connector_runtime::VoucherBindingError::UnknownPeer(
                    "ghost".to_string()
                ))
            );
        }

        #[test]
        fn the_challenge_rides_its_own_slot_on_both_carriages() {
            let names = connector_btp::CARRIAGE_NAMES
                .iter()
                .find(|names| names.concept == "peer-role-challenge")
                .expect("declared as a pair");
            assert_ne!(names.btp_protocol_entry, connector_btp::CLAIM_PROTOCOL);
            assert_ne!(names.http_header, connector_btp::CLAIM_HEADER);
            // The header carries base64 of the JSON, as the claim's does.
            let json = solana_challenge(&peer_ed25519(), PEER_ACCOUNT, 1);
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(connector_peer_http::headers::peer_challenge_header_value(
                        &json
                    ))
                    .unwrap(),
                json.into_bytes()
            );
        }
    }
}
