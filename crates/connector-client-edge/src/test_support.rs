//! Shared fixtures for this crate's own unit tests: settlement networks over
//! the in-memory batch settlement, holding one EVM x402 channel (and a
//! second, salted one) and one Solana `payment-channels` channel, and the
//! voucher and claim-state challenge signers a client would use on them (ADR
//! 0074, ADR 0075: every claim is a voucher).
//!
//! The in-memory settlement is not a mock: it asserts no call sequence, and
//! upholds the port's contract suites. It counts chain reads so a test can
//! show a refusal cost nothing.

use std::sync::Arc;

use connector_domain::Network;
use connector_runtime::{
    InMemoryJournal, InMemorySettlementNetwork, Journal, SettlementNetwork, SettlementNetworks,
};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_claim_state_challenge_digest,
    evm_voucher_digest, solana_voucher_claim_state_challenge_message, solana_voucher_message,
    BatchChannelConfig, BatchSettlementDomain,
};
use ed25519_dalek::Signer as _;
use libsecp256k1::{Message, PublicKey, SecretKey};

use crate::ClientClaimGate;

/// The EVM chain every channel below is on.
pub(crate) const CHAIN_ID: u64 = 84_532;

/// The Solana channel account the fake holds.
pub(crate) const SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];

/// The Solana network the fake settles on: devnet's genesis prefix.
pub(crate) const SOLANA_NETWORK: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

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
        "network": format!("eip155:{CHAIN_ID}"),
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
        "network": SOLANA_NETWORK,
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
        "network": format!("eip155:{CHAIN_ID}"),
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
        "network": SOLANA_NETWORK,
        "scheme": "batch-settlement",
        "channelAccount": bs58::encode(SOLANA_CHANNEL).into_string(),
        "expires": expires,
        "signature": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    })
}

/// The settlement networks every unit test's gate settles on: one EVM
/// network (`eip155:84532`) holding the channels named by `configs`, and the
/// Solana network holding [`SOLANA_CHANNEL`], each over the in-memory batch
/// settlement -- the fake both of the port's contract suites hold to the
/// same rules as the real adapters (ADR 0007) -- and each channel able to
/// pay `max_cumulative`.
///
/// It keeps the one EVM rule a real network cannot escape: an EVM channel
/// is found only from its config.
pub(crate) struct InMemoryNetworks {
    evm: Arc<InMemorySettlementNetwork>,
    solana: Arc<InMemorySettlementNetwork>,
}

impl InMemoryNetworks {
    pub(crate) fn new(max_cumulative: u128) -> InMemoryNetworks {
        InMemoryNetworks::holding(vec![config(), config_salted(0x77)], max_cumulative)
    }

    pub(crate) fn holding(
        configs: Vec<BatchChannelConfig>,
        max_cumulative: u128,
    ) -> InMemoryNetworks {
        let evm = InMemorySettlementNetwork::fresh(Network::Eip155(CHAIN_ID));
        for config in &configs {
            evm.open_evm(config, max_cumulative);
        }
        let solana =
            InMemorySettlementNetwork::fresh(SOLANA_NETWORK.parse().expect("a CAIP-2 network"));
        solana.open_solana(
            SOLANA_CHANNEL,
            solana_signer().public.to_bytes(),
            max_cumulative,
        );
        InMemoryNetworks {
            evm: Arc::new(evm),
            solana: Arc::new(solana),
        }
    }

    /// Networks whose every read fails -- an unreachable RPC endpoint.
    pub(crate) fn unreachable() -> InMemoryNetworks {
        let networks = InMemoryNetworks::new(u128::from(u64::MAX));
        networks.evm.settlement().set_unreachable(true);
        networks.solana.settlement().set_unreachable(true);
        networks
    }

    /// Every chain read either network has made.
    pub(crate) fn lookups(&self) -> usize {
        self.evm.settlement().reads() + self.solana.settlement().reads()
    }

    /// The two networks, as a gate is given them.
    pub(crate) fn networks(&self) -> SettlementNetworks {
        SettlementNetworks::new()
            .with(Arc::clone(&self.evm) as Arc<dyn SettlementNetwork>)
            .and_then(|networks| {
                networks.with(Arc::clone(&self.solana) as Arc<dyn SettlementNetwork>)
            })
            .expect("two distinct networks")
    }
}

/// A gate over `journal`, admitting vouchers on `networks`.
pub(crate) fn gate_over(
    journal: Arc<dyn Journal>,
    networks: &Arc<InMemoryNetworks>,
) -> ClientClaimGate {
    ClientClaimGate::restore(journal)
        .expect("the journal replays")
        .with_settlement_networks(networks.networks())
}

/// A gate over an in-memory journal and networks that can pay a great
/// deal.
pub(crate) fn voucher_gate() -> ClientClaimGate {
    gate_over(
        Arc::new(InMemoryJournal::new()),
        &Arc::new(InMemoryNetworks::new(1_000_000_000)),
    )
}
