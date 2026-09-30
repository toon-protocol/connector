//! Shared fixtures for this crate's own unit tests: a fake
//! [`BatchSettlementChannels`] holding one EVM x402 channel and one Solana
//! `payment-channels` channel, and the voucher and claim-state challenge
//! signers a client would use on them (ADR 0074, ADR 0075: every claim is a
//! voucher).
//!
//! The fake is not a mock: it asserts no call sequence, and holds the one
//! fact a backend owns -- which channels exist, who signs on them, and what
//! they can pay. It counts lookups so a test can show a refusal cost
//! nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use connector_runtime::{InMemoryJournal, Journal};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_claim_state_challenge_digest,
    evm_voucher_digest, solana_voucher_claim_state_challenge_message, solana_voucher_message,
    BatchChannelConfig, BatchSettlementDomain,
};
use ed25519_dalek::Signer as _;
use libsecp256k1::{Message, PublicKey, SecretKey};

use crate::batch_settlement::{
    AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
};
use crate::channels::{ChannelLookupFailed, ChannelResolutionError};
use crate::ClientClaimGate;

/// The EVM chain every channel below is on.
pub(crate) const CHAIN_ID: u64 = 84_532;

/// The Solana channel account the fake holds.
pub(crate) const SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];

pub(crate) fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(CHAIN_ID)
}

/// The payer's voucher signer on the EVM channel: its `payerAuthorizer`.
pub(crate) fn authorizer() -> SecretKey {
    SecretKey::parse(&[0x0a; 32]).expect("valid secret")
}

/// A key that signs on no channel the fake holds.
pub(crate) fn stranger() -> SecretKey {
    SecretKey::parse(&[0x5a; 32]).expect("valid secret")
}

pub(crate) fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

/// The EVM channel's config, salted by `salt` so a test can name a second,
/// independent channel.
pub(crate) fn config_salted(salt: u8) -> BatchChannelConfig {
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: address_of(&authorizer()),
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [salt; 32],
    }
}

pub(crate) fn config() -> BatchChannelConfig {
    config_salted(0x66)
}

pub(crate) fn channel_id_of(config: &BatchChannelConfig) -> [u8; 32] {
    evm_batch_channel_id(&domain(), config)
}

pub(crate) fn channel_id() -> [u8; 32] {
    channel_id_of(&config())
}

/// The canonical watermark key of the EVM channel.
pub(crate) fn channel_key() -> String {
    format!("evm:0x{}", hex::encode(channel_id()))
}

/// Sign `digest` as a wallet does: `r ‖ s ‖ v`, `v` in `{27, 28}`.
pub(crate) fn sign_digest(secret: &SecretKey, digest: &[u8; 32]) -> String {
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(digest), secret);
    let mut bytes = signature.serialize().to_vec();
    bytes.push(recovery.serialize() + 27);
    format!("0x{}", hex::encode(bytes))
}

pub(crate) fn config_json(config: &BatchChannelConfig) -> serde_json::Value {
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

/// An EVM voucher's JSON on `config`'s channel, carrying `signature` verbatim
/// and the config itself, declaring `sender_id`.
pub(crate) fn evm_voucher_with(
    config: &BatchChannelConfig,
    amount: u128,
    signature: &str,
    sender_id: &str,
) -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-25T12:00:00.000Z",
        "senderId": sender_id,
        "channelId": format!("0x{}", hex::encode(channel_id_of(config))),
        "maxClaimableAmount": amount.to_string(),
        "signature": signature,
        "channelConfig": config_json(config),
    })
    .to_string()
}

/// The payer's genuine voucher for cumulative `amount` on `config`'s
/// channel.
pub(crate) fn signed_voucher_on(config: &BatchChannelConfig, amount: u128) -> String {
    let digest = evm_voucher_digest(&domain(), &channel_id_of(config), amount);
    evm_voucher_with(
        config,
        amount,
        &sign_digest(&authorizer(), &digest),
        "client-1",
    )
}

/// The payer's genuine voucher for cumulative `amount` on the EVM channel.
pub(crate) fn signed_voucher(amount: u128) -> String {
    signed_voucher_on(&config(), amount)
}

/// A voucher on the EVM channel, correctly signed by a key that is not the
/// channel's voucher signer -- the forger of issue #558.
pub(crate) fn forged_voucher(amount: u128) -> String {
    let digest = evm_voucher_digest(&domain(), &channel_id(), amount);
    evm_voucher_with(
        &config(),
        amount,
        &sign_digest(&stranger(), &digest),
        "client-1",
    )
}

pub(crate) fn solana_signer() -> ed25519_dalek::Keypair {
    let secret = ed25519_dalek::SecretKey::from_bytes(&[0x21; 32]).expect("seed");
    let public = (&secret).into();
    ed25519_dalek::Keypair { secret, public }
}

/// A Solana voucher on [`SOLANA_CHANNEL`], signed by `signer`.
pub(crate) fn solana_voucher(amount: u64, signer: &ed25519_dalek::Keypair) -> String {
    let signature = signer.sign(&solana_voucher_message(&SOLANA_CHANNEL, amount, 0));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-25T12:00:00Z",
        "senderId": "client-2",
        "channelId": bs58::encode(SOLANA_CHANNEL).into_string(),
        "maxClaimableAmount": amount.to_string(),
        "expiresAt": 0,
        "signature": bs58::encode(signature.to_bytes()).into_string(),
    })
    .to_string()
}

/// The retired `toon-channel` claim, as a pre-ADR 0075 client sends it: no
/// `scheme`, a nonce, and a balance proof.
pub(crate) fn toon_channel_claim() -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "messageId": "claim-1",
        "timestamp": "2026-02-02T12:00:00.000Z",
        "senderId": "peer-bob",
        "channelId": format!("0x{}", "ab".repeat(32)),
        "nonce": 1,
        "transferredAmount": "100",
        "lockedAmount": "0",
        "locksRoot": format!("0x{}", "00".repeat(32)),
        "signature": format!("0x{}", "11".repeat(65)),
        "signerAddress": format!("0x{}", "44".repeat(20)),
    })
    .to_string()
}

/// A voucher claim-state challenge object (the peer-role challenge's JSON,
/// and a BTP auth entry's `channelChallenge`) on the EVM channel, signed by
/// `secret`, carrying the channel's config.
pub(crate) fn evm_challenge(secret: &SecretKey, expires: u64) -> serde_json::Value {
    let digest = evm_voucher_claim_state_challenge_digest(&domain(), &channel_id(), expires);
    serde_json::json!({
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "channelId": format!("0x{}", hex::encode(channel_id())),
        "expires": expires,
        "signature": sign_digest(secret, &digest),
        "channelConfig": config_json(&config()),
    })
}

/// As [`evm_challenge`], on the Solana channel.
pub(crate) fn solana_challenge(signer: &ed25519_dalek::Keypair, expires: u64) -> serde_json::Value {
    use base64::Engine;
    let signature = signer.sign(&solana_voucher_claim_state_challenge_message(
        &SOLANA_CHANNEL,
        expires,
    ));
    serde_json::json!({
        "blockchain": "solana",
        "scheme": "batch-settlement",
        "channelAccount": bs58::encode(SOLANA_CHANNEL).into_string(),
        "expires": expires,
        "signature": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    })
}

/// A backend holding the EVM channels named by `configs` and the Solana
/// channel, each able to pay `max_cumulative`.
///
/// It keeps the seam's one EVM rule a real backend cannot escape: an EVM
/// channel is found only from its config, so an instance that has never
/// been shown the config admits nothing when handed `None`.
#[derive(Debug)]
pub(crate) struct FakeBatchSettlement {
    configs: Vec<BatchChannelConfig>,
    max_cumulative: u128,
    lookups: AtomicUsize,
    admitted: Mutex<HashSet<[u8; 32]>>,
    /// When set, every lookup fails as an unreachable endpoint would.
    failing: bool,
}

impl FakeBatchSettlement {
    pub(crate) fn new(max_cumulative: u128) -> FakeBatchSettlement {
        FakeBatchSettlement::holding(vec![config(), config_salted(0x77)], max_cumulative)
    }

    pub(crate) fn holding(
        configs: Vec<BatchChannelConfig>,
        max_cumulative: u128,
    ) -> FakeBatchSettlement {
        FakeBatchSettlement {
            configs,
            max_cumulative,
            lookups: AtomicUsize::new(0),
            admitted: Mutex::new(HashSet::new()),
            failing: false,
        }
    }

    /// A backend whose every lookup fails -- an unreachable RPC endpoint.
    pub(crate) fn unreachable() -> FakeBatchSettlement {
        FakeBatchSettlement {
            failing: true,
            ..FakeBatchSettlement::new(u128::from(u64::MAX))
        }
    }

    pub(crate) fn lookups(&self) -> usize {
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
        if self.failing {
            return Err(ChannelResolutionError::LookupFailed(ChannelLookupFailed(
                "rpc unreachable".to_string(),
            )));
        }
        let Some(config) = self
            .configs
            .iter()
            .find(|config| channel_id_of(config) == *channel_id)
        else {
            return Ok(None);
        };
        let mut admitted = self.admitted.lock().unwrap();
        if presented_config.is_none() && !admitted.contains(channel_id) {
            return Ok(None);
        }
        admitted.insert(*channel_id);
        Ok(Some(AdmittedEvmVoucherChannel {
            config: *config,
            max_cumulative: self.max_cumulative,
        }))
    }

    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        if self.failing {
            return Err(ChannelResolutionError::LookupFailed(ChannelLookupFailed(
                "rpc unreachable".to_string(),
            )));
        }
        Ok(
            (*channel_account == SOLANA_CHANNEL).then_some(AdmittedSolanaVoucherChannel {
                authorized_signer: solana_signer().public.to_bytes(),
                max_cumulative: u64::try_from(self.max_cumulative).unwrap_or(u64::MAX),
            }),
        )
    }
}

/// A gate over `journal`, admitting vouchers through `backend`.
pub(crate) fn gate_over(
    journal: Arc<dyn Journal>,
    backend: &Arc<FakeBatchSettlement>,
) -> ClientClaimGate {
    ClientClaimGate::restore(journal)
        .expect("the journal replays")
        .with_batch_settlement(Arc::clone(backend) as Arc<dyn BatchSettlementChannels>)
}

/// A gate over an in-memory journal and a backend that can pay a great
/// deal.
pub(crate) fn voucher_gate() -> ClientClaimGate {
    gate_over(
        Arc::new(InMemoryJournal::new()),
        &Arc::new(FakeBatchSettlement::new(1_000_000_000)),
    )
}
