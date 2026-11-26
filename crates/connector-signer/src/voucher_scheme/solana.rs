//! Solana's voucher scheme: x402's `batch-settlement` voucher on
//! solana-foundation's `payment-channels` program (ADR 0074 decisions 3 and
//! 4, ADR 0075), a [`VoucherScheme`].
//!
//! ```text
//! wire       = { blockchain: "solana", channelId: base58 of 32 bytes,
//!                maxClaimableAmount: u64 decimal, expiresAt: 0, signature: base58 of 64 bytes }
//! channelId  = the channel account: the PDA of [b"channel", payer, payee, mint,
//!                authorized_signer, salt LE, open_slot LE] under the program
//! signed     = 0x56 0x01 ‖ channel account ‖ amount u64 LE ‖ expiresAt i64 LE (zero)
//! signer     = the channel account's own `authorized_signer`, read from the chain
//! challenge  = "toon-voucher-claim-state-challenge-v1" ‖ channel account ‖ expires u64 LE
//! ```
//!
//! A channel is found by its account's address, so the channel the chain
//! records is the account it answered at -- the account the voucher names.
//! What binds a voucher to that channel is its signature: it signs the
//! account, and is checked against the `authorized_signer` the chain
//! records there. `check_channel_id` adds nothing beyond that for a channel
//! looked up by the voucher's own account. `expiresAt` is always
//! zero (ADR 0074 decision 3): the domain's parser refuses any other value
//! by name, and the zero is still part of the signed bytes.
//!
//! The message layouts are [`crate::voucher_signature`]'s and
//! [`crate::claim_state_challenge`]'s, each pinned there against the
//! program. This module is the one place every caller reaches them through.

use connector_domain::client_claim::SolanaVoucher;
use sha2::{Digest, Sha256};

use super::{VoucherRefusal, VoucherScheme};
use crate::claim_state_challenge::{
    solana_voucher_claim_state_challenge_message, verify_solana_voucher_claim_state_challenge,
};
use crate::voucher_signature::{solana_voucher_message, verify_solana_voucher};
use crate::{Ed25519Signer, SignerError};

/// solana-foundation's `payment-channels` program at its canonical id: the
/// one program every Solana voucher channel lives under (ADR 0074).
const PAYMENT_CHANNELS_PROGRAM_ID: &str = "CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX";

/// The PDA seed naming a channel account (PC `state/channel.rs`).
const CHANNEL_SEED: &[u8] = b"channel";

/// Solana's voucher scheme. Stateless: the domain and the recorded channel
/// are passed in, from this node's configuration and from the chain.
#[derive(Debug, Clone, Copy, Default)]
pub struct SolanaVoucherScheme;

/// What a Solana channel id is derived under: the program that owns the
/// channel account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaVoucherDomain {
    /// The program id, raw bytes.
    pub program_id: [u8; 32],
}

impl SolanaVoucherDomain {
    /// `payment-channels` at its canonical id, a constant of the binary.
    #[must_use]
    pub fn payment_channels() -> SolanaVoucherDomain {
        SolanaVoucherDomain {
            program_id: SolanaVoucherScheme::decode_channel_account(PAYMENT_CHANNELS_PROGRAM_ID)
                .expect("the canonical program id is base58 of 32 bytes"),
        }
    }
}

/// The seeds a channel account's address is derived from (PC
/// `Channel::find_pda`). `open_slot` is one of them, so an address is per
/// incarnation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaChannelSeeds {
    pub payer: [u8; 32],
    pub payee: [u8; 32],
    pub mint: [u8; 32],
    pub authorized_signer: [u8; 32],
    pub salt: u64,
    pub open_slot: u64,
}

/// What the chain records for a channel: the account it answered at and
/// that account's `authorized_signer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedSolanaChannel {
    /// The address of the account the chain answered.
    pub channel_account: [u8; 32],
    /// Its `authorized_signer` field.
    pub authorized_signer: [u8; 32],
}

/// A Solana voucher with every field decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedSolanaVoucher {
    /// The channel account the voucher signs.
    pub channel_account: [u8; 32],
    /// The cumulative amount it authorises: a `u64`, as the program's.
    pub max_claimable_amount: u64,
    /// Ed25519 `R ‖ S`.
    pub signature: [u8; 64],
}

impl SolanaVoucherScheme {
    /// A channel account from its wire spelling, base58 of 32 bytes, or
    /// `None`.
    #[must_use]
    pub fn decode_channel_account(text: &str) -> Option<[u8; 32]> {
        decode_base58(text)
    }

    /// The 50 bytes a voucher for `amount` on `channel_account` signs.
    #[must_use]
    pub fn voucher_message(channel_account: &[u8; 32], amount: u64) -> [u8; 50] {
        solana_voucher_message(channel_account, amount, 0)
    }
}

impl VoucherScheme for SolanaVoucherScheme {
    type Wire = SolanaVoucher;
    type Voucher = ParsedSolanaVoucher;
    type Signature = [u8; 64];
    type Domain = SolanaVoucherDomain;
    type ChannelParts = SolanaChannelSeeds;
    type Recorded = RecordedSolanaChannel;
    type Signer = [u8; 32];
    type KeyHolder = dyn Ed25519Signer;

    fn parse(wire: &SolanaVoucher) -> Result<ParsedSolanaVoucher, VoucherRefusal> {
        let signature = Self::decode_signature(&wire.signature).ok_or_else(|| {
            VoucherRefusal::Malformed(
                "a Solana voucher's 'signature' must be base58 of 64 bytes".to_string(),
            )
        })?;
        let channel_account = Self::decode_channel_account(&wire.channel_id)
            .ok_or(VoucherRefusal::UnreadableChannel)?;
        Ok(ParsedSolanaVoucher {
            channel_account,
            max_claimable_amount: wire.max_claimable_amount,
            signature,
        })
    }

    fn encode(
        domain: &SolanaVoucherDomain,
        channel: &SolanaChannelSeeds,
        amount: u128,
        key: &dyn Ed25519Signer,
    ) -> Result<ParsedSolanaVoucher, SignerError> {
        // The program's width is a `u64`: wider is refused, never
        // truncated (ADR 0075, "Update (issue #1565)").
        let amount = u64::try_from(amount).map_err(|_| {
            SignerError::SigningFailed(format!(
                "a Solana voucher names at most {} units, not {amount}",
                u64::MAX
            ))
        })?;
        let channel_account = Self::channel_id(domain, channel);
        Ok(ParsedSolanaVoucher {
            channel_account,
            max_claimable_amount: amount,
            signature: key.sign(&Self::voucher_message(&channel_account, amount)),
        })
    }

    fn spell(voucher: &ParsedSolanaVoucher) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("blockchain".into(), "solana".into());
        fields.insert(
            "channelId".into(),
            bs58::encode(voucher.channel_account).into_string().into(),
        );
        fields.insert(
            "maxClaimableAmount".into(),
            voucher.max_claimable_amount.to_string().into(),
        );
        fields.insert("expiresAt".into(), 0.into());
        fields.insert(
            "signature".into(),
            bs58::encode(voucher.signature).into_string().into(),
        );
        fields
    }

    fn decode_signature(text: &str) -> Option<[u8; 64]> {
        decode_base58(text)
    }

    /// The channel account's address: the first off-curve
    /// `SHA-256(seeds ‖ bump ‖ program ‖ "ProgramDerivedAddress")`, trying
    /// the bump from 255 down, as `Pubkey::find_program_address` does.
    fn channel_id(domain: &SolanaVoucherDomain, parts: &SolanaChannelSeeds) -> [u8; 32] {
        let salt = parts.salt.to_le_bytes();
        let open_slot = parts.open_slot.to_le_bytes();
        let seeds: [&[u8]; 7] = [
            CHANNEL_SEED,
            &parts.payer,
            &parts.payee,
            &parts.mint,
            &parts.authorized_signer,
            &salt,
            &open_slot,
        ];
        (1..=u8::MAX)
            .rev()
            .find_map(|bump| {
                let mut hasher = Sha256::new();
                for seed in seeds {
                    hasher.update(seed);
                }
                hasher.update([bump]);
                hasher.update(domain.program_id);
                hasher.update(b"ProgramDerivedAddress");
                let address: [u8; 32] = hasher.finalize().into();
                // On the curve is a key someone could hold; a PDA is not.
                ed25519_dalek::PublicKey::from_bytes(&address)
                    .is_err()
                    .then_some(address)
            })
            .expect("some bump of 255 derives an off-curve address")
    }

    /// Whether `recorded`'s account is `channel_id`, byte for byte. A
    /// channel account derives no id to check -- the PDA is the id -- and
    /// the claim gate records the channel at the account the voucher names,
    /// so there this compares that account with itself and always holds. A
    /// voucher naming another channel fails [`verify`](VoucherScheme::verify)
    /// on its signature instead, which signs the account, against the
    /// signer the chain records at the account it was looked up by.
    fn check_channel_id(
        _domain: &SolanaVoucherDomain,
        channel_id: &[u8; 32],
        recorded: &RecordedSolanaChannel,
    ) -> Result<(), VoucherRefusal> {
        if recorded.channel_account == *channel_id {
            Ok(())
        } else {
            Err(VoucherRefusal::ChannelIdMismatch)
        }
    }

    fn signer(recorded: &RecordedSolanaChannel) -> [u8; 32] {
        recorded.authorized_signer
    }

    fn verify(
        domain: &SolanaVoucherDomain,
        voucher: &ParsedSolanaVoucher,
        recorded: &RecordedSolanaChannel,
    ) -> Result<(), VoucherRefusal> {
        Self::check_channel_id(domain, &voucher.channel_account, recorded)?;
        if verify_solana_voucher(
            &voucher.channel_account,
            voucher.max_claimable_amount,
            0,
            &voucher.signature,
            &Self::signer(recorded),
        ) {
            Ok(())
        } else {
            Err(VoucherRefusal::SignatureInvalid)
        }
    }

    fn challenge(_domain: &SolanaVoucherDomain, channel_id: &[u8; 32], expires: u64) -> Vec<u8> {
        solana_voucher_claim_state_challenge_message(channel_id, expires)
    }

    fn verify_challenge(
        domain: &SolanaVoucherDomain,
        channel_id: &[u8; 32],
        expires: u64,
        signature: &[u8],
        recorded: &RecordedSolanaChannel,
    ) -> Result<(), VoucherRefusal> {
        Self::check_channel_id(domain, channel_id, recorded)?;
        if verify_solana_voucher_claim_state_challenge(
            channel_id,
            expires,
            signature,
            &Self::signer(recorded),
        ) {
            Ok(())
        } else {
            Err(VoucherRefusal::SignatureInvalid)
        }
    }
}

/// Exactly `N` bytes of base58, or `None`.
fn decode_base58<const N: usize>(text: &str) -> Option<[u8; N]> {
    bs58::decode(text).into_vec().ok()?.try_into().ok()
}

#[cfg(test)]
mod tests;
