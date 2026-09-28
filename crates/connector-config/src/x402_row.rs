//! What `[[peer_channels]]` and `[[pay_channels]]` share now that both name
//! **x402 channels** (ADR 0075 decisions 4, 6 and 9, issue #1380): how an x402
//! channel and a voucher signer are spelled on each chain, and the removed
//! `toon-channel` row fields both tables still parse, purely so that a file
//! which writes one is refused **by name** (ADR 0009).
//!
//! # Chain by spelling
//!
//! Neither row has a `chain` key, because the two chains' spellings are
//! disjoint and each value already says which chain it is on: an EVM voucher
//! signer is a `0x`-prefixed 20-byte address and an EVM channel a `0x`-prefixed
//! 32-byte id, while a Solana signer and a Solana channel account are both
//! base58 of exactly 32 bytes. A `0x` string is never valid base58 (`0` is
//! outside the alphabet), so no value can be read as both.

use crate::client_channel::{is_base58_32_bytes, parse_hex_bytes, to_hex};
use crate::settlement::SettlementChain;

/// A value written in one chain's spelling, canonicalized: EVM hex lower-cased
/// with its `0x`, Solana base58 exactly as written (base58 is case-sensitive,
/// so lower-casing one names an account nobody holds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChainSpelled {
    pub(crate) chain: SettlementChain,
    pub(crate) value: String,
}

/// `value` as a voucher signer: an EVM address (`0x` + 40 hex) or a Solana
/// public key (base58 of 32 bytes). `None` for anything else.
pub(crate) fn parse_voucher_signer(value: &str) -> Option<ChainSpelled> {
    if value.starts_with("0x") || value.starts_with("0X") {
        return parse_hex_bytes::<20>(value).map(|bytes| ChainSpelled {
            chain: SettlementChain::Evm,
            value: to_hex(&bytes),
        });
    }
    is_base58_32_bytes(value).then(|| ChainSpelled {
        chain: SettlementChain::Solana,
        value: value.to_string(),
    })
}

/// `value` as an x402 channel: an EVM `x402BatchSettlement` channel id
/// (`0x` and 64 hex) or a Solana `payment-channels` channel account (base58
/// of 32 bytes). `None` for anything else.
pub(crate) fn parse_x402_channel(value: &str) -> Option<ChainSpelled> {
    if value.starts_with("0x") || value.starts_with("0X") {
        return parse_hex_bytes::<32>(value).map(|bytes| ChainSpelled {
            chain: SettlementChain::Evm,
            value: to_hex(&bytes),
        });
    }
    is_base58_32_bytes(value).then(|| ChainSpelled {
        chain: SettlementChain::Solana,
        value: value.to_string(),
    })
}

/// The fields a `toon-channel` row wrote, in the order they are reported:
/// each is parsed as a `toml::Value` so that a value of any type is still
/// named as the removed key rather than failing the row's shape.
///
/// * `channel_id` -- the EVM `TokenNetwork` channel, derived from its two
///   participants (ADR 0059, retired by ADR 0075);
/// * `channel_account` -- the Solana channel **in its TOON meaning**: a PDA of
///   TOON's own payment-channel program;
/// * `chain_id` and `token_network` -- the EIP-712 domain a `toon-channel`
///   claim was signed under (ADR 0024, retired by ADR 0075);
/// * `counterparty_key` -- the key a `toon-channel` claim was verified
///   against, which a voucher signer replaces;
/// * `program_id` -- refused since #1128 and still refused.
///
/// Written out on each row rather than `#[serde(flatten)]`ed in, because
/// serde's `flatten` and `deny_unknown_fields` do not compose, and a money
/// row that silently accepted an unknown key would be worse than either.
#[derive(Debug, Default)]
pub(crate) struct RemovedToonFields {
    pub(crate) channel_id: bool,
    pub(crate) channel_account: bool,
    pub(crate) chain_id: bool,
    pub(crate) token_network: bool,
    pub(crate) counterparty_key: bool,
    pub(crate) program_id: bool,
}

impl RemovedToonFields {
    /// The first removed field the row writes, if any.
    pub(crate) fn first(&self) -> Option<&'static str> {
        [
            ("channel_id", self.channel_id),
            ("channel_account", self.channel_account),
            ("chain_id", self.chain_id),
            ("token_network", self.token_network),
            ("counterparty_key", self.counterparty_key),
            ("program_id", self.program_id),
        ]
        .into_iter()
        .find_map(|(field, written)| written.then_some(field))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLANA: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";

    #[test]
    fn a_voucher_signer_is_read_in_either_chains_spelling() {
        let evm = parse_voucher_signer("0xAAAABBBBCCCCDDDDEEEEFFFF0000111122223333").expect("evm");
        assert_eq!(evm.chain, SettlementChain::Evm);
        assert_eq!(evm.value, "0xaaaabbbbccccddddeeeeffff0000111122223333");

        let solana = parse_voucher_signer(SOLANA).expect("solana");
        assert_eq!(solana.chain, SettlementChain::Solana);
        assert_eq!(solana.value, SOLANA);

        assert_eq!(parse_voucher_signer("0x12"), None);
        assert_eq!(parse_voucher_signer("not-base58!!!"), None);
        assert_eq!(
            parse_voucher_signer(&format!("0x{}", "ab".repeat(32))),
            None,
            "a channel id is not a signer"
        );
    }

    #[test]
    fn an_x402_channel_is_read_in_either_chains_spelling() {
        let evm = parse_x402_channel(&format!("0x{}", "AB".repeat(32))).expect("evm");
        assert_eq!(evm.chain, SettlementChain::Evm);
        assert_eq!(evm.value, format!("0x{}", "ab".repeat(32)));

        let solana = parse_x402_channel(SOLANA).expect("solana");
        assert_eq!(solana.chain, SettlementChain::Solana);

        assert_eq!(
            parse_x402_channel("0x2222222222222222222222222222222222222222"),
            None,
            "an address is not a channel"
        );
    }

    #[test]
    fn the_first_removed_field_is_named() {
        let fields = RemovedToonFields {
            chain_id: true,
            token_network: true,
            ..RemovedToonFields::default()
        };
        assert_eq!(fields.first(), Some("chain_id"));
        assert_eq!(RemovedToonFields::default().first(), None);
    }
}
