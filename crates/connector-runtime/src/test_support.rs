//! Envelope-shaped test fixtures shared by every `mod tests` in this crate
//! that drives a [`crate::connector::Connector`] through a
//! [`crate::app_client::FakeAppClient`]. Issue #521 moved delivery onto the
//! structured envelope (ADR 0018/issue #519); issue #524 seals it -- a
//! `Prepare.data` is a gift wrap around a request envelope, and a
//! `Fulfill.data` (or a termination `Reject.data`) is a gift wrap around a
//! response envelope, sealed back with the same shared secret -- so every
//! test file that exercises delivery needs the same handful of builders
//! around sealing/opening and [`EnvelopeRequest`]/[`EnvelopeResponse`],
//! built once here instead of once per file.

use std::sync::{Arc, OnceLock};

use connector_domain::{EnvelopeRequest, EnvelopeResponse};
use connector_signer::giftwrap::{derive_fulfillment, open_response, seal_request};
use connector_signer::{evm_balance_proof_digest, Address, LocalSigner, Signer};

use crate::app_client::AppOutcome;
use crate::claim::{evm_proof, parse_channel_id, ChannelDomain, WireClaim};
use crate::connector::Connector;
use crate::outbound_client::{
    ClaimStateDomain, ClaimStateSource, ClaimWatermark, EvmDomain, OutboundClientError,
    OutboundClientLedger,
};

/// This crate's one shared "this connector's own identity" fixture: every
/// [`envelope_request_data`]/[`sealed_envelope_request_data`] call seals to
/// this key, and a `Connector` under test opens it by configuring
/// `.with_identity_signer(identity_signer())`. A single process-lifetime
/// key rather than a fresh one per call, so unrelated tests share it
/// without each needing to thread a key through by hand; a test that
/// specifically needs a *different* identity (issue #524 AC3: a forwarding
/// hop cannot open a wrap addressed elsewhere) constructs its own
/// `LocalSigner` instead of calling this.
pub(crate) fn identity_signer() -> Arc<dyn Signer> {
    static IDENTITY: OnceLock<Arc<dyn Signer>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| Arc::new(LocalSigner::generate("test-support-identity")))
        .clone()
}

/// What a `Prepare`'s `data` carries per ADR 0018/issue #524 -- a gift wrap
/// sealed to [`identity_signer`]'s public key, around a minimal `POST /`
/// envelope carrying `body`. Returns the wire bytes and the shared secret
/// the wrap carries, for a caller that also wants to open the sealed
/// `Fulfill`/`Reject` this `Prepare` produces (see
/// [`open_sealed_envelope`]).
pub(crate) fn sealed_envelope_request_data(body: &[u8]) -> (Vec<u8>, [u8; 32]) {
    sealed_envelope_request_data_with_target("/", body)
}

/// Like [`sealed_envelope_request_data`], but with a caller-chosen
/// `target` -- for a test that exercises how a `Connector` resolves (or
/// refuses) it against a route's handler path (issue #596), rather than
/// the default minimal `POST /`.
pub(crate) fn sealed_envelope_request_data_with_target(
    target: &str,
    body: &[u8],
) -> (Vec<u8>, [u8; 32]) {
    sealed_envelope_request_data_with_headers(target, vec![], body)
}

/// Like [`sealed_envelope_request_data_with_target`], but with
/// caller-chosen request headers inside the sealed envelope -- for a test
/// that asserts what a delivery carries beyond what the sender wrote,
/// including a sender who wrote the attribution headers a terminating
/// connector states for itself (ADR 0040, `crate::attribution`).
pub(crate) fn sealed_envelope_request_data_with_headers(
    target: &str,
    headers: Vec<(String, String)>,
    body: &[u8],
) -> (Vec<u8>, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: target.to_string(),
        headers,
        body: body.to_vec(),
    }
    .encode();
    seal_request(
        &plaintext,
        &identity_signer().public_key().expect("public key"),
    )
    .expect("seal")
}

/// Open `data` (a `Fulfill.data`, or a termination `Reject.data`) with
/// `shared_secret` and decode it as a response envelope -- the inverse of
/// what `Connector::deliver_to_app` seals with, so a test can assert on
/// what a sealed response actually carries rather than on its
/// non-deterministic ciphertext bytes.
pub(crate) fn open_sealed_envelope(shared_secret: &[u8; 32], data: &[u8]) -> EnvelopeResponse {
    let opened = open_response(shared_secret, data).expect("open sealed response");
    EnvelopeResponse::decode(&opened).expect("decode response envelope")
}

/// The fulfilment a terminating `Connector` derives for a packet sealed
/// with `shared_secret` (ADR 0019, issue #525) -- what a genuine sender
/// mints its execution condition from before sealing, and what a test
/// compares a `Fulfill`'s own `fulfillment` field against.
pub(crate) fn expected_fulfillment(shared_secret: &[u8; 32]) -> [u8; 32] {
    derive_fulfillment(shared_secret)
}

/// The `AppOutcome` a `FakeAppClient` produces for an app that answers
/// `200` with `body`. The app supplies nothing toward fulfilment (issue
/// #525): a termination derives its own answer's fulfilment from the
/// packet's sealed secret, never from anything in this response.
pub(crate) fn answered(body: &[u8]) -> AppOutcome {
    answered_with_status(200, body)
}

pub(crate) fn answered_with_status(status: u16, body: &[u8]) -> AppOutcome {
    AppOutcome::Answered {
        response: EnvelopeResponse {
            status,
            headers: vec![],
            body: body.to_vec(),
        },
    }
}

/// The response envelope `Connector::handle_prepare` seals into `Fulfill
/// .data` for an app answering `200` with `body`. Compare against
/// [`open_sealed_envelope`]'s result, since sealing makes the raw wire
/// bytes non-deterministic per call.
pub(crate) fn fulfill_envelope(body: &[u8]) -> EnvelopeResponse {
    fulfill_envelope_with_status(200, body)
}

pub(crate) fn fulfill_envelope_with_status(status: u16, body: &[u8]) -> EnvelopeResponse {
    EnvelopeResponse {
        status,
        headers: vec![],
        body: body.to_vec(),
    }
}

/// A fixed EIP-712 domain every peer-role test channel in this crate's own
/// test modules shares (issue #575/#566) -- an arbitrary but consistent
/// chain id and `TokenNetwork` address; nothing in these tests depends on
/// their real-world provenance, only that signing and verifying a claim use
/// the same domain a channel was registered with.
pub(crate) fn test_channel_domain() -> ChannelDomain {
    ChannelDomain {
        chain_id: 84_532,
        token_network_address: [0x1E; 20],
    }
}

/// A valid on-chain `bytes32` peer channel id for tests -- `0x`
/// followed by `n` left-padded to 64 hex characters (issue #575's AC4: a
/// peer claim's channel id must already be a real bytes32, never an
/// arbitrary label like the `"channel-a"` placeholders this crate's tests
/// used before this issue).
pub(crate) fn test_channel_id(n: u8) -> String {
    format!("0x{n:064x}")
}

/// Register `n`'s channel on `connector`: `counterparty`'s address as the
/// key an inbound claim on it must recover to, and [`test_channel_domain`]
/// as its EIP-712 signing domain -- the pairing [`ClaimBook::accept_inbound`]
/// and [`ClaimBook::record_fulfillment`] both require before a channel can
/// accept or produce a claim at all (issue #575's AC3).
pub(crate) fn with_test_channel(connector: Connector, n: u8, counterparty: Address) -> Connector {
    connector
        .with_channel_verification_key(test_channel_id(n), counterparty)
        .with_channel_domain(test_channel_id(n), test_channel_domain())
        .expect("test_channel_id(n) is a valid on-chain channel id")
}

/// Sign a [`WireClaim`] for channel `n`'s `nonce`/`cumulative_amount` under
/// [`test_channel_domain`] -- exactly the digest
/// `ClaimBook::record_fulfillment` would compute for the same inputs.
pub(crate) fn sign_wire_claim(
    signer: &dyn Signer,
    n: u8,
    nonce: u64,
    cumulative_amount: u64,
) -> WireClaim {
    let channel_id = test_channel_id(n);
    let on_chain_id = parse_channel_id(&channel_id).expect("test_channel_id(n) is valid");
    let proof = evm_proof(on_chain_id, test_channel_domain(), nonce, cumulative_amount);
    WireClaim {
        channel_id,
        nonce,
        cumulative_amount,
        signature: crate::claim::ClaimSignature::Evm(
            signer
                .sign(&evm_balance_proof_digest(&proof))
                .expect("sign"),
        ),
    }
}

/// A next hop that answers where this node's claims on a channel stand --
/// the authority the outbound client ledger prices every covering claim off
/// (see [`crate::outbound_client`]'s header). A fake upholding the port's
/// contract, not a stub with expectations (ADR 0007): it reports a
/// watermark, which is the whole of what the port is for.
struct AlwaysReportsAWatermark;

#[async_trait::async_trait]
impl ClaimStateSource for AlwaysReportsAWatermark {
    async fn watermark(
        &self,
        _channel: &[u8; 32],
        _domain: &ClaimStateDomain,
    ) -> Result<ClaimWatermark, OutboundClientError> {
        Ok(ClaimWatermark {
            nonce: 0,
            cumulative: 0,
            available: Some(u64::MAX.into()),
        })
    }
}

/// Give `connector` what ADR 0042 requires of **any** peering it forwards
/// to: a channel to pay `peer_id` from, and a ledger to sign the covering
/// claim out of.
///
/// Every test in this crate that forwards a packet to a peer needs this,
/// and that is the point of issue #1145. Before it, a peering with no
/// client-role config fell through to ADR 0004's postpay convention and
/// the packet went out uncovered; now `Connector::forward_via_peer_route`
/// refuses it outright, and `Config::load` refuses the file that would
/// have produced it (`ConfigError::PayChannelUnbound`). A fixture that
/// forwards without this is not a simpler fixture -- it is one no
/// configuration can produce.
///
/// The channel is [`test_channel_id`]`(1)` and the ledger is in-memory,
/// which is right for a test but never for a serving node: a restart that
/// reissued a nonce would fork its own outbound nonce line, which is why
/// `OutboundClientLedger::open` is what `connector-cli` wires.
pub(crate) fn covering(connector: Connector, peer_id: &str) -> Connector {
    let connector = if connector.claims.signer().is_some() {
        connector
    } else {
        connector.with_signer(Arc::new(LocalSigner::generate("test-support-settlement")))
    };
    connector
        .with_outbound_client_ledger(Arc::new(OutboundClientLedger::in_memory()))
        .with_outbound_client_hop(
            peer_id,
            test_channel_id(1),
            EvmDomain {
                chain_id: test_channel_domain().chain_id,
                token_network: test_channel_domain().token_network_address,
            },
            Arc::new(AlwaysReportsAWatermark),
        )
        .expect("test_channel_id(1) is a valid on-chain channel id")
}
