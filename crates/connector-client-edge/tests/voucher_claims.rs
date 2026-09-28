//! ADR 0074, issue #1341: an x402 `batch-settlement` voucher through the
//! client edge's real claim gate -- opt-in, freshness, retransmission,
//! signature, collateral and the journal -- over a fake backend that
//! answers the seam (`BatchSettlementChannels`) the way #1342/#1343's real
//! ones must: with admitted channels only, and the signer read from the
//! chain.
//!
//! The fake is not a mock: it asserts no call sequence, and it holds the
//! one fact a backend owns -- which channels exist and what they can pay.
//! It counts lookups so a test can show a refusal that should cost nothing
//! (a replay, an underpayment, a config that hashes wrong) really asked the
//! backend nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use connector_client_edge::{
    journaled_batch_channels, AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel,
    BatchSettlementChannels, ChannelResolutionError, ClaimIngestRejection, ClientClaimGate,
    JournaledBatchChannel, UnresolvableLookupBudgetPolicy,
};
use connector_domain::{JournalEntry, Watermark, VOUCHER_WATERMARK_NONCE};
use connector_runtime::{FileJournal, InMemoryJournal, Journal};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_digest, solana_voucher_message,
    BatchChannelConfig, BatchSettlementDomain,
};
use ed25519_dalek::Signer as _;
use libsecp256k1::{Message, PublicKey, SecretKey};

const CHAIN_ID: u64 = 84_532;

fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(CHAIN_ID)
}

/// The payer's session key: the channel's `payerAuthorizer`.
fn authorizer() -> SecretKey {
    SecretKey::parse(&[0x0a; 32]).expect("valid secret")
}

fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

fn config() -> BatchChannelConfig {
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: address_of(&authorizer()),
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [0x66; 32],
    }
}

fn channel_id() -> [u8; 32] {
    evm_batch_channel_id(&domain(), &config())
}

fn channel_key() -> String {
    format!("evm:0x{}", hex::encode(channel_id()))
}

fn sign_voucher(secret: &SecretKey, amount: u64) -> String {
    let digest = evm_voucher_digest(&domain(), &channel_id(), u128::from(amount));
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(&digest), secret);
    let mut bytes = signature.serialize().to_vec();
    bytes.push(recovery.serialize() + 27);
    format!("0x{}", hex::encode(bytes))
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

fn evm_voucher(amount: u64, signature: &str, config: Option<&BatchChannelConfig>) -> String {
    let mut voucher = serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-25T12:00:00.000Z",
        "senderId": "client-1",
        "channelId": format!("0x{}", hex::encode(channel_id())),
        "maxClaimableAmount": amount.to_string(),
        "signature": signature,
    });
    if let Some(config) = config {
        voucher["channelConfig"] = config_json(config);
    }
    voucher.to_string()
}

fn signed_evm_voucher(amount: u64) -> String {
    evm_voucher(
        amount,
        &sign_voucher(&authorizer(), amount),
        Some(&config()),
    )
}

const SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];

fn solana_signer() -> ed25519_dalek::Keypair {
    let secret = ed25519_dalek::SecretKey::from_bytes(&[0x21; 32]).expect("seed");
    let public = (&secret).into();
    ed25519_dalek::Keypair { secret, public }
}

fn solana_voucher(amount: u64, signer: &ed25519_dalek::Keypair, expires_at: i64) -> String {
    let signature = signer.sign(&solana_voucher_message(&SOLANA_CHANNEL, amount, expires_at));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-25T12:00:00Z",
        "senderId": "client-2",
        "channelId": bs58::encode(SOLANA_CHANNEL).into_string(),
        "maxClaimableAmount": amount.to_string(),
        "expiresAt": expires_at,
        "signature": bs58::encode(signature.to_bytes()).into_string(),
    })
    .to_string()
}

/// A backend holding one EVM and one Solana channel, both admissible.
///
/// It keeps the seam's one EVM rule a real backend cannot escape: an EVM
/// channel is found only from its config, so an instance that has never
/// been shown the config -- a fresh process, after a restart -- admits
/// nothing when handed `None`.
#[derive(Debug)]
struct FakeBatchSettlement {
    evm_config: BatchChannelConfig,
    max_cumulative: u64,
    lookups: AtomicUsize,
    admitted: Mutex<HashSet<[u8; 32]>>,
}

impl FakeBatchSettlement {
    fn new(max_cumulative: u64) -> FakeBatchSettlement {
        FakeBatchSettlement::holding(config(), max_cumulative)
    }

    fn holding(evm_config: BatchChannelConfig, max_cumulative: u64) -> FakeBatchSettlement {
        FakeBatchSettlement {
            evm_config,
            max_cumulative,
            lookups: AtomicUsize::new(0),
            admitted: Mutex::new(HashSet::new()),
        }
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BatchSettlementChannels for FakeBatchSettlement {
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
        self.lookups.fetch_add(1, Ordering::SeqCst);
        if *channel_id != evm_batch_channel_id(&domain(), &self.evm_config) {
            return Ok(None);
        }
        let mut admitted = self.admitted.lock().unwrap();
        if presented_config.is_none() && !admitted.contains(channel_id) {
            return Ok(None);
        }
        admitted.insert(*channel_id);
        Ok(Some(AdmittedEvmVoucherChannel {
            config: self.evm_config,
            max_cumulative: self.max_cumulative,
        }))
    }

    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Ok(
            (*channel_account == SOLANA_CHANNEL).then_some(AdmittedSolanaVoucherChannel {
                authorized_signer: solana_signer().public.to_bytes(),
                max_cumulative: self.max_cumulative,
            }),
        )
    }
}

fn gate_over(journal: Arc<dyn Journal>, backend: &Arc<FakeBatchSettlement>) -> ClientClaimGate {
    ClientClaimGate::restore(journal)
        .expect("the journal replays")
        .with_batch_settlement(Arc::clone(backend) as Arc<dyn BatchSettlementChannels>)
}

fn gate_with(backend: &Arc<FakeBatchSettlement>) -> (ClientClaimGate, Arc<InMemoryJournal>) {
    let journal = Arc::new(InMemoryJournal::new());
    (
        gate_over(Arc::clone(&journal) as Arc<dyn Journal>, backend),
        journal,
    )
}

// -- Opt-in (ADR 0074 decision 1) --

#[tokio::test]
async fn a_node_that_has_not_opted_in_refuses_a_voucher_by_name() {
    let gate =
        ClientClaimGate::restore(Arc::new(InMemoryJournal::new())).expect("an empty journal");
    for voucher in [
        signed_evm_voucher(100),
        solana_voucher(100, &solana_signer(), 0),
    ] {
        assert_eq!(
            gate.ingest(&voucher, 0).await.unwrap_err(),
            ClaimIngestRejection::BatchSettlementNotAccepted
        );
    }
    assert!(ClaimIngestRejection::BatchSettlementNotAccepted
        .message()
        .contains("does not settle on"));
}

// -- EVM --

#[tokio::test]
async fn a_genuine_evm_voucher_is_accepted_and_journaled_like_a_claim() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, journal) = gate_with(&backend);

    gate.ingest(&signed_evm_voucher(100), 100)
        .await
        .expect("accepted");

    assert_eq!(
        gate.watermark(&channel_key()),
        Some(Watermark {
            cumulative_amount: 100
        })
    );
    // ADR 0005, as amended by ADR 0074 decision 3: the signed bytes and the
    // watermark they set, in the same entry a claim is journaled in -- after
    // the record of the channel itself, which a first voucher adds so the
    // channel can be landed on after a restart.
    let entries = journal.read_all().expect("readable");
    let [JournalEntry::BatchChannelAdmitted { .. }, JournalEntry::InboundClaimAccepted {
        channel_id,
        nonce,
        cumulative_amount,
        signature,
    }] = entries.as_slice()
    else {
        panic!("expected the channel's record and one acceptance, got {entries:?}");
    };
    assert_eq!(
        journaled_batch_channels(&entries).expect("readable"),
        vec![JournaledBatchChannel::Evm {
            channel_id: crate::channel_id(),
            config: config(),
        }]
    );
    assert_eq!(channel_id, &channel_key());
    assert_eq!((*nonce, *cumulative_amount), (VOUCHER_WATERMARK_NONCE, 100));
    assert_eq!(
        format!("0x{}", hex::encode(signature)),
        sign_voucher(&authorizer(), 100)
    );
}

#[tokio::test]
async fn a_later_voucher_needs_no_channel_config() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    gate.ingest(&signed_evm_voucher(100), 0)
        .await
        .expect("first");

    gate.ingest(
        &evm_voucher(250, &sign_voucher(&authorizer(), 250), None),
        150,
    )
    .await
    .expect("a later voucher names its channel and nothing else");
    assert_eq!(
        gate.watermark(&channel_key()).map(|w| w.cumulative_amount),
        Some(250)
    );
}

/// ADR 0074 decision 7's watermark case: an equal amount is refused, a
/// higher one accepted -- and the refusal costs no lookup and no signature.
#[tokio::test]
async fn an_equal_amount_is_refused_before_any_lookup_and_a_higher_one_accepted() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    gate.ingest(&signed_evm_voucher(100), 0)
        .await
        .expect("first");
    let lookups = backend.lookups();

    // The same amount under different bytes (a different, even invalid,
    // signature): not strictly greater, so not advancing.
    let other = evm_voucher(
        100,
        &sign_voucher(&SecretKey::parse(&[7; 32]).unwrap(), 100),
        None,
    );
    assert_eq!(
        gate.ingest(&other, 0).await.unwrap_err(),
        ClaimIngestRejection::AmountNotAdvancing
    );
    assert_eq!(
        backend.lookups(),
        lookups,
        "freshness before cryptography: a stale voucher asks the backend nothing"
    );

    gate.ingest(&signed_evm_voucher(101), 1)
        .await
        .expect("strictly greater");
}

#[tokio::test]
async fn a_lower_amount_is_refused() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    gate.ingest(&signed_evm_voucher(100), 0)
        .await
        .expect("first");
    assert_eq!(
        gate.ingest(&signed_evm_voucher(99), 0).await.unwrap_err(),
        ClaimIngestRejection::AmountNotAdvancing
    );
}

/// The `peer_claim_retransmit` rule: a byte-identical voucher at the
/// watermark is not an error, and records nothing.
#[tokio::test]
async fn a_byte_identical_resend_is_accepted_again_and_records_nothing() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, journal) = gate_with(&backend);
    let voucher = signed_evm_voucher(100);
    gate.ingest(&voucher, 100).await.expect("first");
    let lookups = backend.lookups();
    let journaled = journal.read_all().expect("readable").len();

    gate.ingest(&voucher, 0)
        .await
        .expect("a resend at no charge is not an error");

    assert_eq!(journal.read_all().expect("readable").len(), journaled);
    assert_eq!(backend.lookups(), lookups, "nothing new to verify");
    assert_eq!(
        gate.watermark(&channel_key()).map(|w| w.cumulative_amount),
        Some(100)
    );
}

/// ... and buys nothing: resent to pay for a priced packet it is an
/// underpayment of the whole charge, or one voucher would pay forever.
#[tokio::test]
async fn a_byte_identical_resend_pays_for_nothing() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    let voucher = signed_evm_voucher(100);
    gate.ingest(&voucher, 100).await.expect("first");

    assert_eq!(
        gate.ingest(&voucher, 100).await.unwrap_err(),
        ClaimIngestRejection::Underpayment {
            advanced: 0,
            price: 100
        }
    );
}

#[tokio::test]
async fn value_binding_applies_to_a_voucher() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    assert_eq!(
        gate.ingest(&signed_evm_voucher(40), 50).await.unwrap_err(),
        ClaimIngestRejection::Underpayment {
            advanced: 40,
            price: 50
        }
    );
    assert_eq!(backend.lookups(), 0);
    assert_eq!(gate.watermark(&channel_key()), None);
}

/// ADR 0074 decision 2: the connector recomputes `getChannelId` and refuses
/// a config that is not the channel the voucher signs -- before asking the
/// backend about it.
#[tokio::test]
async fn a_config_that_does_not_hash_to_the_channel_is_refused() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    let forged = BatchChannelConfig {
        payer_authorizer: [0x77; 20],
        ..config()
    };
    assert_eq!(
        gate.ingest(
            &evm_voucher(100, &sign_voucher(&authorizer(), 100), Some(&forged)),
            0
        )
        .await
        .unwrap_err(),
        ClaimIngestRejection::VoucherChannelConfigMismatch
    );
    assert_eq!(backend.lookups(), 0);
}

/// The signer is the chain's `payerAuthorizer`, never whoever signed: a
/// voucher signed by the `payer` when an authorizer is set, or by a stranger,
/// is refused and advances nothing.
#[tokio::test]
async fn a_voucher_not_signed_by_the_channels_signer_is_refused() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    let stranger = SecretKey::parse(&[0x0b; 32]).unwrap();
    assert_eq!(
        gate.ingest(
            &evm_voucher(100, &sign_voucher(&stranger, 100), Some(&config())),
            0
        )
        .await
        .unwrap_err(),
        ClaimIngestRejection::SignatureInvalid
    );
    assert_eq!(gate.watermark(&channel_key()), None);
}

#[tokio::test]
async fn a_voucher_above_the_channels_collateral_is_refused() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    assert_eq!(
        gate.ingest(&signed_evm_voucher(1_001), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::Undercollateralized {
            claimed: 1_001,
            deposited: 1_000
        }
    );
    assert_eq!(gate.watermark(&channel_key()), None);
}

#[tokio::test]
async fn a_voucher_on_a_channel_the_backend_does_not_admit_is_unknown() {
    let backend = Arc::new(FakeBatchSettlement::holding(
        BatchChannelConfig {
            salt: [0x01; 32],
            ..config()
        },
        1_000,
    ));
    let (gate, _journal) = gate_with(&backend);
    assert_eq!(
        gate.ingest(&signed_evm_voucher(100), 0).await.unwrap_err(),
        ClaimIngestRejection::UnknownChannel
    );
}

#[tokio::test]
async fn a_voucher_amount_above_u64_is_refused_not_truncated() {
    let backend = Arc::new(FakeBatchSettlement::new(u64::MAX));
    let (gate, _journal) = gate_with(&backend);
    let wide = signed_evm_voucher(100).replace(
        r#""maxClaimableAmount":"100""#,
        &format!(r#""maxClaimableAmount":"{}""#, u128::from(u64::MAX) + 100),
    );
    assert!(matches!(
        gate.ingest(&wide, 0).await.unwrap_err(),
        ClaimIngestRejection::Malformed(reason) if reason.contains("refused, not truncated")
    ));
}

// -- Solana --

#[tokio::test]
async fn a_genuine_solana_voucher_is_accepted_and_a_strangers_refused() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    let key = format!("solana:{}", bs58::encode(SOLANA_CHANNEL).into_string());

    let stranger = {
        let secret = ed25519_dalek::SecretKey::from_bytes(&[0x22; 32]).unwrap();
        let public = (&secret).into();
        ed25519_dalek::Keypair { secret, public }
    };
    assert_eq!(
        gate.ingest(&solana_voucher(100, &stranger, 0), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::SignatureInvalid
    );

    gate.ingest(&solana_voucher(100, &solana_signer(), 0), 100)
        .await
        .expect("signed by the channel's authorized_signer");
    assert_eq!(
        gate.watermark(&key),
        Some(Watermark {
            cumulative_amount: 100
        })
    );
}

#[tokio::test]
async fn a_solana_voucher_that_can_expire_is_refused_structurally() {
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let (gate, _journal) = gate_with(&backend);
    assert!(matches!(
        gate.ingest(&solana_voucher(100, &solana_signer(), 1_900_000_000), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::Malformed(reason) if reason.contains("expiresAt")
    ));
    assert_eq!(backend.lookups(), 0);
}

// -- Restart (ADR 0005) --

/// A voucher's watermark survives a restart the way a claim's does, and so
/// does the retransmission rule: the replayed watermark still carries the
/// signed bytes a resend is compared against.
#[tokio::test]
async fn after_a_restart_a_resend_is_still_a_resend_and_a_replay_still_stale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client-claims.journal");
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let voucher = signed_evm_voucher(100);
    {
        let gate = gate_over(Arc::new(FileJournal::open(&path).expect("opens")), &backend);
        gate.ingest(&voucher, 100).await.expect("accepted");
    }

    let gate = gate_over(
        Arc::new(FileJournal::open(&path).expect("reopens")),
        &backend,
    );
    assert_eq!(
        gate.watermark(&channel_key()).map(|w| w.cumulative_amount),
        Some(100)
    );
    gate.ingest(&voucher, 0)
        .await
        .expect("the same bytes, recognised from the journal");
    assert_eq!(
        gate.ingest(&voucher, 100).await.unwrap_err(),
        ClaimIngestRejection::Underpayment {
            advanced: 0,
            price: 100
        },
        "and still buying nothing"
    );
}

/// A voucher signs only its channel's id, and a fresh process's backend has
/// never been shown the config that id hashes. The journal has: the gate
/// hands the recorded config over, so a client whose later vouchers carry no
/// `channelConfig` -- as x402 lets them -- is still paid after a restart.
#[tokio::test]
async fn after_a_restart_a_voucher_without_its_config_is_still_admitted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client-claims.journal");
    {
        let gate = gate_over(
            Arc::new(FileJournal::open(&path).expect("opens")),
            &Arc::new(FakeBatchSettlement::new(1_000)),
        );
        gate.ingest(&signed_evm_voucher(100), 100)
            .await
            .expect("the channel's first voucher, carrying its config");
    }

    // A new process: a backend that has admitted nothing yet.
    let restarted = Arc::new(FakeBatchSettlement::new(1_000));
    let gate = gate_over(
        Arc::new(FileJournal::open(&path).expect("reopens")),
        &restarted,
    );
    gate.ingest(
        &evm_voucher(250, &sign_voucher(&authorizer(), 250), None),
        150,
    )
    .await
    .expect("the journal remembers the channel's config");
    assert_eq!(
        gate.watermark(&channel_key()).map(|w| w.cumulative_amount),
        Some(250)
    );
}

/// Issue #613's bound, applied to vouchers: a lookup for a channel this gate
/// has never accepted a voucher on costs a slot of the unresolvable-lookup
/// budget unless it finds a channel, so naming fresh channel ids cannot make
/// this node read its chain without limit -- and a client paying on a real
/// channel spends none of it.
#[tokio::test]
async fn a_voucher_lookup_that_finds_nothing_is_metered() {
    // One lookup per hour, and no waiting for the next.
    let backend = Arc::new(FakeBatchSettlement::new(1_000));
    let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
        .expect("an empty journal")
        .with_lookup_budget(UnresolvableLookupBudgetPolicy {
            per_signer: 1,
            total: 1,
            window: std::time::Duration::from_secs(3_600),
            max_wait: std::time::Duration::ZERO,
        })
        .with_batch_settlement(Arc::clone(&backend) as Arc<dyn BatchSettlementChannels>);

    // Real channels resolve, so they give their slot back: any number of
    // first vouchers on them costs the budget nothing.
    gate.ingest(&signed_evm_voucher(100), 100)
        .await
        .expect("a real EVM channel");
    gate.ingest(&solana_voucher(100, &solana_signer(), 0), 100)
        .await
        .expect("a real Solana channel");

    // A channel that does not exist spends the one slot...
    let stranger = BatchChannelConfig {
        salt: [0x99; 32],
        ..config()
    };
    let stranger_id = evm_batch_channel_id(&domain(), &stranger);
    let unknown = |amount: u64| {
        let digest = evm_voucher_digest(&domain(), &stranger_id, u128::from(amount));
        let (signature, recovery) = libsecp256k1::sign(&Message::parse(&digest), &authorizer());
        let mut bytes = signature.serialize().to_vec();
        bytes.push(recovery.serialize() + 27);
        evm_voucher(
            amount,
            &format!("0x{}", hex::encode(bytes)),
            Some(&stranger),
        )
        .replace(&hex::encode(channel_id()), &hex::encode(stranger_id))
    };
    assert_eq!(
        gate.ingest(&unknown(100), 0).await.unwrap_err(),
        ClaimIngestRejection::UnknownChannel
    );
    let lookups = backend.lookups();

    // ...and the next is refused without asking the chain at all.
    assert!(matches!(
        gate.ingest(&unknown(200), 0).await.unwrap_err(),
        ClaimIngestRejection::LookupBudgetExhausted { .. }
    ));
    assert_eq!(backend.lookups(), lookups);

    // A channel already paid on is not a discovery, and is never budgeted.
    gate.ingest(
        &evm_voucher(250, &sign_voucher(&authorizer(), 250), None),
        150,
    )
    .await
    .expect("a known channel is looked up whatever the budget says");
}

// -- Claim state (issue #1364) --
//
// A voucher has no nonce, so a client that lost its channel store has only
// one way to learn the amount its next voucher must exceed: ask. These go
// through the real `POST /ilp/claim-state` route over the real gate.

mod claim_state {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use chrono::{TimeZone, Utc};
    use connector_client_edge::router_with_gate;
    use connector_runtime::{
        Connector, FakeAppClient, InProcessPeerTransport, TestClock, VoucherSigner,
    };
    use connector_signer::{
        evm_voucher_claim_state_challenge_digest, solana_voucher_claim_state_challenge_message,
        LocalSigner,
    };
    use tower::ServiceExt;

    /// The committed cross-repo wire vectors (issue #1408): this module's
    /// own `claim_state_toon_channel_refused` cases, replayed against the
    /// real route below.
    const WIRE_VECTORS: &str = include_str!("../../../vectors/wire-vectors.json");

    /// 2031-01-01, after the test clock's 2030 and any real one's today.
    const EXPIRES: u64 = 1_924_992_000;

    fn evm_challenge(secret: &SecretKey, channel: [u8; 32]) -> String {
        sign_digest(
            secret,
            &evm_voucher_claim_state_challenge_digest(&domain(), &channel, EXPIRES),
        )
    }

    fn sign_digest(secret: &SecretKey, digest: &[u8; 32]) -> String {
        let (signature, recovery) = libsecp256k1::sign(&Message::parse(digest), secret);
        let mut bytes = signature.serialize().to_vec();
        bytes.push(recovery.serialize() + 27);
        format!("0x{}", hex::encode(bytes))
    }

    fn evm_entry(signature: String, config: Option<&BatchChannelConfig>) -> serde_json::Value {
        let mut entry = serde_json::json!({
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "channelId": format!("0x{}", hex::encode(channel_id())),
            "expires": EXPIRES,
            "signature": signature,
        });
        if let Some(config) = config {
            entry["channelConfig"] = config_json(config);
        }
        entry
    }

    fn solana_entry(signer: &ed25519_dalek::Keypair) -> serde_json::Value {
        let message = solana_voucher_claim_state_challenge_message(&SOLANA_CHANNEL, EXPIRES);
        serde_json::json!({
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "channelAccount": bs58::encode(SOLANA_CHANNEL).into_string(),
            "expires": EXPIRES,
            "signature": base64::engine::general_purpose::STANDARD
                .encode(signer.sign(&message).to_bytes()),
        })
    }

    async fn claim_state(gate: ClientClaimGate, entry: serde_json::Value) -> serde_json::Value {
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            )),
        ));
        let app = router_with_gate(
            connector,
            Arc::new(LocalSigner::generate("test-signer")),
            None,
            gate,
        );
        let request = Request::builder()
            .method("POST")
            .uri("/ilp/claim-state")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "channels": [entry] }).to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        body["channels"][0].clone()
    }

    #[tokio::test]
    async fn an_evm_voucher_channel_reports_its_amount_watermark_and_headroom() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        gate.ingest(&signed_evm_voucher(100), 100)
            .await
            .expect("accepted");

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), None),
        )
        .await;

        assert_eq!(
            entry,
            serde_json::json!({
                "blockchain": "evm",
                "channelId": format!("0x{}", hex::encode(channel_id())),
                "ok": true,
                "scheme": "batch-settlement",
                "cumulativeClaimed": "100",
                "maxCumulative": "1000",
                "available": "900",
                "lastClaimTime": null,
            })
        );
    }

    /// The case the endpoint exists for: the connector restarted too, and
    /// only its journal knows the channel's config -- the client presents
    /// none, as a client that lost its store could not.
    #[tokio::test]
    async fn after_a_restart_the_watermark_is_answered_from_the_journal_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("client-claims.journal");
        {
            let gate = gate_over(
                Arc::new(FileJournal::open(&path).expect("opens")),
                &Arc::new(FakeBatchSettlement::new(1_000)),
            );
            gate.ingest(&signed_evm_voucher(250), 250)
                .await
                .expect("accepted");
        }
        let gate = gate_over(
            Arc::new(FileJournal::open(&path).expect("reopens")),
            &Arc::new(FakeBatchSettlement::new(1_000)),
        );

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), None),
        )
        .await;

        assert_eq!(entry["ok"], true);
        assert_eq!(entry["cumulativeClaimed"], "250");
    }

    /// A channel nothing has been paid on yet has no record here, so the
    /// config comes from the request -- checked to hash to the channel, and
    /// the signer still read from what the backend admitted.
    #[tokio::test]
    async fn a_channel_with_no_voucher_yet_is_answered_from_a_presented_config() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), Some(&config())),
        )
        .await;

        assert_eq!(entry["ok"], true);
        assert_eq!(entry["cumulativeClaimed"], "0");
        assert_eq!(entry["available"], "1000");
    }

    #[tokio::test]
    async fn a_channel_with_no_record_and_no_config_is_unverified() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), None),
        )
        .await;

        assert_eq!(entry["ok"], false);
        assert_eq!(entry["error"], "unverified");
    }

    #[tokio::test]
    async fn a_config_that_does_not_hash_to_the_channel_is_unverified() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        let other = BatchChannelConfig {
            salt: [0x77; 32],
            ..config()
        };

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), Some(&other)),
        )
        .await;

        assert_eq!(entry["error"], "unverified");
        assert_eq!(backend.lookups(), 0, "refused before the backend is asked");
    }

    /// Only the channel's voucher signer proves control: here the payer
    /// named a separate `payerAuthorizer`, and the payer's own key is not it.
    #[tokio::test]
    async fn a_challenge_not_signed_by_the_voucher_signer_is_unverified() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        gate.ingest(&signed_evm_voucher(100), 100)
            .await
            .expect("accepted");
        let stranger = SecretKey::parse(&[0x0b; 32]).expect("valid secret");

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&stranger, channel_id()), None),
        )
        .await;

        assert_eq!(entry["error"], "unverified");
    }

    /// ADR 0075 decision 8 (issue #1384): `scheme` is required on a
    /// claim-state entry too. An entry with none, or with `"toon-channel"`,
    /// asks about a retired `toon-channel` channel and is answered
    /// `toon-channel-refused` by name, on both chains, without the backend
    /// ever being asked -- even for a channel the voucher signer really
    /// controls.
    #[tokio::test]
    async fn a_toon_channel_entry_is_refused_by_name_without_a_lookup() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let signature = evm_challenge(&authorizer(), channel_id());
        for scheme in [None, Some("toon-channel")] {
            let mut evm = evm_entry(signature.clone(), Some(&config()));
            let mut solana = solana_entry(&solana_signer());
            for entry in [&mut evm, &mut solana] {
                match scheme {
                    Some(scheme) => entry["scheme"] = scheme.into(),
                    None => {
                        entry.as_object_mut().unwrap().remove("scheme");
                    }
                }
            }
            for entry in [evm, solana] {
                let answer = claim_state(gate_with(&backend).0, entry).await;
                assert_eq!(answer["ok"], false, "scheme {scheme:?}");
                assert_eq!(answer["error"], "toon-channel-refused", "scheme {scheme:?}");
            }
        }
        assert_eq!(
            backend.lookups(),
            0,
            "a refusal by name asks the chain nothing"
        );
    }

    /// Issue #1408: the committed `claim_state_toon_channel_refused`
    /// vectors (`vectors/wire-vectors.json`) replayed through the real
    /// `POST /ilp/claim-state` route, over a backend that admits nothing --
    /// the point of this refusal is that it costs no lookup, so a case with
    /// no admitted channel still answering it correctly is exactly the
    /// proof. Each request entry is the vector's own, unmodified; the
    /// answer must equal the vector's `response_entry_json`, including the
    /// `channelId` field a Solana entry's answer carries its channel
    /// account text under.
    #[tokio::test]
    async fn the_committed_claim_state_toon_channel_refused_vectors_match_the_real_endpoint() {
        let vectors: serde_json::Value =
            serde_json::from_str(WIRE_VECTORS).expect("the committed vectors parse");
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let cases = vectors["claim_state_toon_channel_refused"]["cases"]
            .as_array()
            .expect("claim_state_toon_channel_refused.cases is an array");
        assert!(
            !cases.is_empty(),
            "claim_state_toon_channel_refused has cases"
        );
        for case in cases {
            let name = case["name"].as_str().expect("a name");
            let request_entry: serde_json::Value =
                serde_json::from_str(case["request_entry_json"].as_str().expect("request entry"))
                    .expect("the vector's request entry is JSON");
            let expected: serde_json::Value = serde_json::from_str(
                case["response_entry_json"]
                    .as_str()
                    .expect("response entry"),
            )
            .expect("the vector's response entry is JSON");

            let answer = claim_state(gate_with(&backend).0, request_entry).await;
            assert_eq!(answer, expected, "{name}");
        }
        assert_eq!(
            backend.lookups(),
            0,
            "a refusal by name asks the chain nothing, for any committed case"
        );
    }

    #[tokio::test]
    async fn a_solana_voucher_channel_reports_its_amount_watermark() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        gate.ingest(&solana_voucher(300, &solana_signer(), 0), 300)
            .await
            .expect("accepted");

        let entry = claim_state(gate, solana_entry(&solana_signer())).await;

        assert_eq!(
            entry,
            serde_json::json!({
                "blockchain": "solana",
                "channelId": bs58::encode(SOLANA_CHANNEL).into_string(),
                "ok": true,
                "scheme": "batch-settlement",
                "cumulativeClaimed": "300",
                "maxCumulative": "1000",
                "available": "700",
                "lastClaimTime": null,
            })
        );
    }

    #[tokio::test]
    async fn a_solana_challenge_by_anyone_but_the_authorized_signer_is_unverified() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        let secret = ed25519_dalek::SecretKey::from_bytes(&[0x22; 32]).expect("seed");
        let public = (&secret).into();
        let stranger = ed25519_dalek::Keypair { secret, public };

        let entry = claim_state(gate, solana_entry(&stranger)).await;

        assert_eq!(entry["error"], "unverified");
    }

    /// ADR 0074 decision 1: a node that has not opted in knows no voucher
    /// channel, and says no more than that.
    #[tokio::test]
    async fn a_node_that_has_not_opted_in_answers_unverified() {
        let gate =
            ClientClaimGate::restore(Arc::new(InMemoryJournal::new())).expect("an empty journal");

        let entry = claim_state(
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), Some(&config())),
        )
        .await;

        assert_eq!(entry["error"], "unverified");
    }

    // -- A peer's voucher channel (ADR 0075 decisions 5 and 6, issue #1377) --

    const PEER_ID: &str = "store";

    /// A node holding one peering, `store`, whose voucher signers -- the
    /// channel's `payerAuthorizer` on EVM, its `authorized_signer` on Solana
    /// -- are bound to it: both channels are the peer's, not a client's.
    fn peer_bound_connector() -> Arc<Connector> {
        let connector = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            )),
        )
        .with_config_peer_ids([PEER_ID.to_string()]);
        bind_peer(connector)
    }

    fn bind_peer(connector: Connector) -> Arc<Connector> {
        connector
            .bind_voucher_signer(PEER_ID, VoucherSigner::Evm(address_of(&authorizer())))
            .expect("store is a peering");
        connector
            .bind_voucher_signer(
                PEER_ID,
                VoucherSigner::Solana(solana_signer().public.to_bytes()),
            )
            .expect("store is a peering");
        Arc::new(connector)
    }

    async fn claim_state_on(
        connector: Arc<Connector>,
        gate: ClientClaimGate,
        entry: serde_json::Value,
    ) -> serde_json::Value {
        let app = router_with_gate(
            connector,
            Arc::new(LocalSigner::generate("test-signer")),
            None,
            gate,
        );
        let request = Request::builder()
            .method("POST")
            .uri("/ilp/claim-state")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "channels": [entry] }).to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        body["channels"][0].clone()
    }

    /// The watermark authority a paying peer restores from (ADR 0075
    /// decision 6) answers for the peer's channel on either chain, proved by
    /// the same challenge that proves the peer role (decision 5).
    #[tokio::test]
    async fn a_voucher_challenge_is_answered_for_a_peer_bound_channel() {
        let backend = Arc::new(FakeBatchSettlement::new(1_000));
        let (gate, _journal) = gate_with(&backend);
        gate.ingest(&signed_evm_voucher(100), 100)
            .await
            .expect("accepted");
        let connector = peer_bound_connector();
        assert_eq!(
            connector
                .voucher_signer_peer(&VoucherSigner::Evm(address_of(&authorizer())))
                .as_deref(),
            Some(PEER_ID),
            "the channel is the peer's"
        );

        let evm = claim_state_on(
            Arc::clone(&connector),
            gate,
            evm_entry(evm_challenge(&authorizer(), channel_id()), None),
        )
        .await;
        assert_eq!(evm["ok"], true, "{evm}");
        assert_eq!(evm["scheme"], "batch-settlement");
        assert_eq!(evm["cumulativeClaimed"], "100");

        let (gate, _journal) = gate_with(&backend);
        gate.ingest(&solana_voucher(70, &solana_signer(), 0), 70)
            .await
            .expect("accepted");
        let solana = claim_state_on(connector, gate, solana_entry(&solana_signer())).await;
        assert_eq!(solana["ok"], true, "{solana}");
        assert_eq!(solana["cumulativeClaimed"], "70");
    }
}
