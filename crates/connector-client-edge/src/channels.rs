//! What a channel lookup can fail with, and the byte decoders every stage
//! of the claim gate shares.
//!
//! This module held the client edge's per-channel counterparty registry --
//! `ClientChannelRegistry`, filled from `[[client_channels]]` and, for a
//! channel nothing declared, from the chain through a `ClientChannelSource`
//! (the EVM `TokenNetwork` channel index, or TOON's Solana program) -- which
//! is what a `toon-channel` claim's signature was verified against. ADR 0075
//! retired that claim scheme (decision 8, issue #1384), so the registry, its
//! sources, its liveness memo and `[[client_channels]]` are deleted with its
//! last caller. A voucher's channel is resolved by the batch-settlement
//! backend ([`crate::BatchSettlementChannels`]), and what survives here is
//! the vocabulary that backend reports a failed lookup in.

use crate::lookup_budget::LookupBudgetExhausted;

/// A lookup could not answer whether a channel exists or who its voucher
/// signer is -- an unreachable RPC endpoint, a node that answered with
/// garbage, a timeout. Deliberately distinct from "this channel does not
/// exist": the first is a failure of *this connector's*, the second is a
/// fact about the world, and conflating them would let an RPC outage read
/// as a definitive "no such channel".
///
/// Either way the claim is refused. This type exists so a refusal can say
/// which of the two happened, never so anything can recover from it by
/// believing the claim instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelLookupFailed(pub String);

impl std::fmt::Display for ChannelLookupFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChannelLookupFailed {}

/// A lookup has a definitive answer that a channel can never be paid on
/// again -- a batch-settlement channel that is sealed, or one whose payer
/// has finished withdrawing. Kept distinct from [`ChannelLookupFailed`]
/// (this connector could not find out) and from a plain `Ok(None)` (this
/// connector has no information either way), so a refusal can say "this
/// channel is done" rather than the weaker "I have no record of it".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelTerminal(pub String);

impl std::fmt::Display for ChannelTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChannelTerminal {}

/// Why a channel could not be resolved -- the refusals a lookup can produce,
/// kept apart because they are not the same event (issues #613, #661).
///
/// [`ChannelResolutionError::LookupFailed`] is a failure: this connector
/// asked and did not get an answer. [`ChannelResolutionError::Budgeted`] is
/// a decision: this connector declined to ask, because the sender (or the
/// node as a whole) has already spent its allowance of lookups for channels
/// that turn out not to exist. [`ChannelResolutionError::Terminal`] is a
/// known fact: the channel is done. All three refuse the claim; conflating
/// them would send an operator to fix the wrong thing.
///
/// All three are also distinct from `Ok(None)` -- "there is no such
/// channel" -- which is a fact about the world rather than about this
/// connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelResolutionError {
    LookupFailed(ChannelLookupFailed),
    Budgeted(LookupBudgetExhausted),
    Terminal(ChannelTerminal),
}

impl From<ChannelLookupFailed> for ChannelResolutionError {
    fn from(failure: ChannelLookupFailed) -> ChannelResolutionError {
        ChannelResolutionError::LookupFailed(failure)
    }
}

impl From<LookupBudgetExhausted> for ChannelResolutionError {
    fn from(exhausted: LookupBudgetExhausted) -> ChannelResolutionError {
        ChannelResolutionError::Budgeted(exhausted)
    }
}

impl From<ChannelTerminal> for ChannelResolutionError {
    fn from(terminal: ChannelTerminal) -> ChannelResolutionError {
        ChannelResolutionError::Terminal(terminal)
    }
}

impl std::fmt::Display for ChannelResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelResolutionError::LookupFailed(failure) => write!(f, "{failure}"),
            ChannelResolutionError::Budgeted(exhausted) => write!(f, "{exhausted}"),
            ChannelResolutionError::Terminal(terminal) => write!(f, "{terminal}"),
        }
    }
}

impl std::error::Error for ChannelResolutionError {}

/// Decode a `0x`-prefixed (or bare) hex string into exactly `N` bytes, or
/// `None` for anything malformed or the wrong length -- never a panic, same
/// as every other step of the claim gate (issue #506's "refused as a
/// validation failure, never as a crash").
pub(crate) fn decode_hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    hex::decode(s.strip_prefix("0x").unwrap_or(s))
        .ok()?
        .try_into()
        .ok()
}

/// Decode a base58 string into exactly `N` bytes, or `None` for anything
/// malformed or the wrong length.
pub(crate) fn decode_base58_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    bs58::decode(s).into_vec().ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes_with_or_without_its_prefix_and_only_at_its_length() {
        assert_eq!(decode_hex_bytes::<2>("0xabcd"), Some([0xab, 0xcd]));
        assert_eq!(decode_hex_bytes::<2>("abcd"), Some([0xab, 0xcd]));
        assert_eq!(decode_hex_bytes::<2>("0xab"), None);
        assert_eq!(decode_hex_bytes::<2>("0xzzzz"), None);
    }

    #[test]
    fn base58_decodes_only_at_its_length() {
        let key = [7u8; 32];
        let text = bs58::encode(key).into_string();
        assert_eq!(decode_base58_bytes::<32>(&text), Some(key));
        assert_eq!(decode_base58_bytes::<31>(&text), None);
        assert_eq!(decode_base58_bytes::<32>("0OIl"), None);
    }
}
