//! EVM's voucher scheme: x402's `batch-settlement` voucher on
//! `x402BatchSettlement` (ADR 0074 decision 4, ADR 0075), the first
//! [`VoucherScheme`].
//!
//! ```text
//! wire       = { blockchain: "evm", channelId: 0x‖64 hex, maxClaimableAmount: uint128 decimal,
//!                signature: 0x‖130 hex (r ‖ s ‖ v, v of 27 or 28), channelConfig?: {...} }
//! channelId  = getChannelId(ChannelConfig) under the x402 EIP-712 domain
//! signed     = getVoucherDigest(channelId, maxClaimableAmount)
//! signer     = payerAuthorizer, else payer, of the config the chain verified
//! challenge  = ClaimStateChallenge(bytes32 channelId, uint256 expires) under the same domain
//! ```
//!
//! The digests, the channel id and the signature rules are
//! [`crate::voucher_signature`]'s and [`crate::claim_state_challenge`]'s,
//! each pinned there against the deployed contract. This module is the one
//! place every caller reaches them through.

use connector_domain::client_claim::{EvmVoucher, EvmVoucherChannelConfig};
use connector_domain::Network;

use super::{VoucherRefusal, VoucherScheme};
use crate::address::to_hex;
use crate::claim_state_challenge::{
    evm_voucher_claim_state_challenge_digest, verify_evm_voucher_claim_state_challenge,
};
use crate::voucher_signature::{
    evm_batch_channel_id, evm_voucher_digest, evm_voucher_signer, verify_evm_voucher,
    BatchChannelConfig, BatchSettlementDomain,
};
use crate::{Address, Signer, SignerError};

/// EVM's voucher scheme. Stateless: the domain and the recorded config are
/// passed in, from this node's configuration and from the chain.
#[derive(Debug, Clone, Copy, Default)]
pub struct EvmVoucherScheme;

/// An EVM voucher with every field decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedEvmVoucher {
    /// The channel the voucher signs.
    pub channel_id: [u8; 32],
    /// The cumulative amount it authorises, `uint128` as the contract's.
    pub max_claimable_amount: u128,
    /// `r ‖ s ‖ v`, `v` as the wallet wrote it.
    pub signature: [u8; 65],
    /// The `ChannelConfig` the voucher presents, if it carries one. Only
    /// ever a candidate: it is re-hashed to the channel id before anything
    /// is read from the chain with it.
    pub channel_config: Option<BatchChannelConfig>,
}

impl EvmVoucherScheme {
    /// The domain vouchers on `network` are verified under: its chain id
    /// and `x402BatchSettlement`'s one address (ADR 0075). `None` for a
    /// network that is not an `eip155` one.
    pub fn domain_on(network: &Network) -> Option<BatchSettlementDomain> {
        network.eip155_chain_id().map(BatchSettlementDomain::x402)
    }

    /// A `channelConfig` as the domain parsed it, decoded into the config
    /// the channel id is the hash of. The one decoder of that wire shape,
    /// for a voucher's config and a claim-state entry's alike.
    pub fn decode_config(
        config: &EvmVoucherChannelConfig,
    ) -> Result<BatchChannelConfig, VoucherRefusal> {
        let address = |value: &str| {
            decode_hex::<20>(value).ok_or_else(|| {
                VoucherRefusal::Malformed(format!("'{value}' is not a 20-byte address"))
            })
        };
        Ok(BatchChannelConfig {
            payer: address(&config.payer)?,
            payer_authorizer: address(&config.payer_authorizer)?,
            receiver: address(&config.receiver)?,
            receiver_authorizer: address(&config.receiver_authorizer)?,
            token: address(&config.token)?,
            withdraw_delay: config.withdraw_delay,
            salt: decode_hex::<32>(&config.salt).ok_or_else(|| {
                VoucherRefusal::Malformed("'channelConfig.salt' is not 32 bytes".to_string())
            })?,
        })
    }

    /// A channel id from its wire spelling, `0x` and 64 hex, or `None`.
    pub fn decode_channel_id(text: &str) -> Option<[u8; 32]> {
        decode_hex::<32>(text)
    }

    /// The config as it is spelled on the wire.
    pub fn spell_config(config: &BatchChannelConfig) -> serde_json::Value {
        serde_json::json!({
            "payer": to_hex(&config.payer),
            "payerAuthorizer": to_hex(&config.payer_authorizer),
            "receiver": to_hex(&config.receiver),
            "receiverAuthorizer": to_hex(&config.receiver_authorizer),
            "token": to_hex(&config.token),
            "withdrawDelay": config.withdraw_delay,
            "salt": hex(&config.salt),
        })
    }

    /// The 32-byte digest a voucher for `amount` on `channel_id` signs.
    pub fn voucher_digest(
        domain: &BatchSettlementDomain,
        channel_id: &[u8; 32],
        amount: u128,
    ) -> [u8; 32] {
        evm_voucher_digest(domain, channel_id, amount)
    }
}

impl VoucherScheme for EvmVoucherScheme {
    type Wire = EvmVoucher;
    type Voucher = ParsedEvmVoucher;
    type Signature = [u8; 65];
    type Domain = BatchSettlementDomain;
    type ChannelParts = BatchChannelConfig;
    type Recorded = BatchChannelConfig;
    type Signer = Address;
    type KeyHolder = dyn Signer;

    fn parse(wire: &EvmVoucher) -> Result<ParsedEvmVoucher, VoucherRefusal> {
        let signature = Self::decode_signature(&wire.signature).ok_or_else(|| {
            VoucherRefusal::Malformed("a voucher's 'signature' must be 65 bytes of hex".to_string())
        })?;
        let channel_config = wire
            .channel_config
            .as_ref()
            .map(Self::decode_config)
            .transpose()?;
        let channel_id =
            Self::decode_channel_id(&wire.channel_id).ok_or(VoucherRefusal::UnreadableChannel)?;
        Ok(ParsedEvmVoucher {
            channel_id,
            max_claimable_amount: wire.max_claimable_amount,
            signature,
            channel_config,
        })
    }

    fn encode(
        domain: &BatchSettlementDomain,
        channel: &BatchChannelConfig,
        amount: u128,
        key: &dyn Signer,
    ) -> Result<ParsedEvmVoucher, SignerError> {
        let channel_id = Self::channel_id(domain, channel);
        let signed = key.sign(&Self::voucher_digest(domain, &channel_id, amount))?;
        let mut signature = signed.to_bytes();
        // The port answers libsecp256k1's raw recovery id; the contract's
        // `ecrecover` takes Ethereum's 27/28.
        signature[64] += 27;
        Ok(ParsedEvmVoucher {
            channel_id,
            max_claimable_amount: amount,
            signature,
            channel_config: Some(*channel),
        })
    }

    fn spell(voucher: &ParsedEvmVoucher) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("blockchain".into(), "evm".into());
        fields.insert("channelId".into(), hex(&voucher.channel_id).into());
        fields.insert(
            "maxClaimableAmount".into(),
            voucher.max_claimable_amount.to_string().into(),
        );
        fields.insert("signature".into(), hex(&voucher.signature).into());
        if let Some(config) = &voucher.channel_config {
            fields.insert("channelConfig".into(), Self::spell_config(config));
        }
        fields
    }

    fn decode_signature(text: &str) -> Option<[u8; 65]> {
        decode_hex::<65>(text)
    }

    fn channel_id(domain: &BatchSettlementDomain, parts: &BatchChannelConfig) -> [u8; 32] {
        evm_batch_channel_id(domain, parts)
    }

    fn check_channel_id(
        domain: &BatchSettlementDomain,
        channel_id: &[u8; 32],
        recorded: &BatchChannelConfig,
    ) -> Result<(), VoucherRefusal> {
        if Self::channel_id(domain, recorded) == *channel_id {
            Ok(())
        } else {
            Err(VoucherRefusal::ChannelIdMismatch)
        }
    }

    fn signer(recorded: &BatchChannelConfig) -> Address {
        evm_voucher_signer(recorded)
    }

    fn verify(
        domain: &BatchSettlementDomain,
        voucher: &ParsedEvmVoucher,
        recorded: &BatchChannelConfig,
    ) -> Result<(), VoucherRefusal> {
        Self::check_channel_id(domain, &voucher.channel_id, recorded)?;
        if verify_evm_voucher(
            domain,
            &voucher.channel_id,
            voucher.max_claimable_amount,
            &voucher.signature,
            &Self::signer(recorded),
        ) {
            Ok(())
        } else {
            Err(VoucherRefusal::SignatureInvalid)
        }
    }

    fn challenge(domain: &BatchSettlementDomain, channel_id: &[u8; 32], expires: u64) -> Vec<u8> {
        evm_voucher_claim_state_challenge_digest(domain, channel_id, expires).to_vec()
    }

    fn verify_challenge(
        domain: &BatchSettlementDomain,
        channel_id: &[u8; 32],
        expires: u64,
        signature: &[u8],
        recorded: &BatchChannelConfig,
    ) -> Result<(), VoucherRefusal> {
        Self::check_channel_id(domain, channel_id, recorded)?;
        if verify_evm_voucher_claim_state_challenge(
            domain,
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

/// `0x` and lowercase hex: how every EVM field is spelled on the wire.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + 2 * bytes.len());
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Exactly `N` bytes of hex, `0x` optional and either case, or `None`.
fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.strip_prefix("0x").unwrap_or(text).as_bytes();
    if text.len() != 2 * N {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (digit(text[2 * i])? << 4) | digit(text[2 * i + 1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
