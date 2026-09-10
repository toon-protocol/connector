//! Which token an amount is denominated in
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md),
//! issue #1288).
//!
//! A packet's amount has no unit of its own: it is denominated by the channel
//! it rides, which is RFC 0027's own definition. That was enough while every
//! path was one **segment** -- one denomination end to end -- because the one
//! unit never had to be named. A hop holding a USDC channel on one side and an
//! ANYONE channel on the other sits on a **denomination boundary**, and there
//! the unit does have to be named: the rate it crosses at is keyed by the
//! ordered pair of tokens, and an ordered pair needs a token identity to be a
//! pair of.
//!
//! That identity is a chain and a contract, and nothing else. In particular it
//! carries **no decimals**: ADR 0071 decision 4 folds the scale difference into
//! the declared ratio, and `[settlement] decimals` stays what it is today -- a
//! boot-time assertion against the chain, never an input to value arithmetic.
//! An [`AssetId`] that carried a scale would be a second place for one to be
//! declared, and two declarations of one fact is how they disagree.
//!
//! # Two spellings, one asset
//!
//! An EVM contract address is hex, and hex has no case: `0xA0b8...` and
//! `0xa0b8...` name the same 20 bytes, and an EIP-55 checksummed spelling of an
//! address is exactly the mixed-case one. A rate table keyed by the literal text
//! an operator happened to type would file `X -> Y` under a key a forward
//! looking for `X -> Y` never finds, and ADR 0071 decision 2 turns a missed
//! lookup into a refused forward. So the EVM half is canonicalised here, once,
//! on the same rule and for the same reason
//! [`crate::client_claim::canonical_channel_key`] canonicalises a channel key:
//! lowercase, `0x`-prefixed, whenever the text has the shape of an address, and
//! left exactly as found when it has not. Canonicalisation only ever merges
//! spellings of one token; it never merges two tokens.
//!
//! Solana is identity, byte for byte, for the reason that module also states:
//! base58 of an exact 32-byte decode has exactly one spelling already, and
//! normalising anyway could only risk merging two mints that are not the same
//! mint.
//!
//! # What this module does not check
//!
//! Whether the contract exists, whether the node has RPC for its chain, whether
//! it is the token a peering actually holds. All three are boot-time questions
//! against a live chain, and this crate has no I/O (ADR 0001). Config load owns
//! them, and refuses by name.

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::client_claim::{EVM_NAMESPACE, SOLANA_NAMESPACE};

/// The separator between an [`AssetId`]'s chain and its token, in the one
/// text form an asset has. The same `<chain>:<id>` shape a channel key
/// already uses, so an operator reading a journal line and an operator
/// reading a rate row are reading the same convention.
const CHAIN_SEPARATOR: char = ':';

/// How many hex characters an EVM contract address is, unprefixed.
const EVM_ADDRESS_HEX_LEN: usize = 40;

/// The settlement chain a token lives on -- the two this connector has a
/// backend for, named with the same two strings the rest of the workspace
/// names them with ([`EVM_NAMESPACE`], [`SOLANA_NAMESPACE`]).
///
/// A closed enum rather than a free string, because ADR 0071 decision 3 puts
/// a token's quote on **its own settlement chain** -- the one chain the
/// peering already guarantees RPC for -- and a chain this node has no backend
/// for is a boot refusal, not a value to carry around. `connector-config`'s
/// `SettlementChain` is the same two chains at the config boundary; the
/// mapping between them belongs there, since this crate must not depend on
/// config to say what a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetChain {
    /// An ERC-20 on the chain `[settlement.evm]` names.
    Evm,
    /// An SPL mint on the cluster `[settlement.solana]` names.
    Solana,
}

impl AssetChain {
    /// The chain's one spelling: `"evm"` or `"solana"`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            AssetChain::Evm => EVM_NAMESPACE,
            AssetChain::Solana => SOLANA_NAMESPACE,
        }
    }
}

impl fmt::Display for AssetChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AssetChain {
    type Err = AssetIdError;

    fn from_str(s: &str) -> Result<AssetChain, AssetIdError> {
        match s {
            EVM_NAMESPACE => Ok(AssetChain::Evm),
            SOLANA_NAMESPACE => Ok(AssetChain::Solana),
            other => Err(AssetIdError::UnknownChain(other.to_string())),
        }
    }
}

/// One token, by the chain it settles on and its contract or mint identity.
///
/// The key half of a rate: ADR 0071 declares a rate per **ordered** pair of
/// these, and direction is the trade -- `X -> Y` and `Y -> X` are different
/// prices from the same mid, so the pair is ordered and never sorted.
///
/// [`Ord`] is derived, on chain then token, purely so a pair of these can be
/// put in a `BTreeMap` and an error can list assets in a stable order. It says
/// nothing about value; no ordering of tokens exists.
///
/// Cloning copies one short string, and every consumer keys a map by one, so
/// this is [`Clone`] rather than [`Copy`] and that is the whole cost.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId {
    chain: AssetChain,
    token: String,
}

impl AssetId {
    /// An ERC-20, by its contract address. The address is canonicalised to
    /// `0x` plus lowercase hex whenever it has an address's shape, so the
    /// checksummed spelling an explorer shows and the lowercase one a config
    /// file was pasted with are one value.
    pub fn evm(contract: impl AsRef<str>) -> AssetId {
        AssetId {
            chain: AssetChain::Evm,
            token: canonical_evm_contract(contract.as_ref()),
        }
    }

    /// An SPL token, by its mint address. Taken byte for byte: base58 of a
    /// 32-byte decode already has exactly one spelling.
    pub fn solana(mint: impl AsRef<str>) -> AssetId {
        AssetId {
            chain: AssetChain::Solana,
            token: mint.as_ref().to_string(),
        }
    }

    /// The same two constructors, chosen at runtime -- what a config row that
    /// names its chain in a field rather than in a table key needs.
    pub fn new(chain: AssetChain, token: impl AsRef<str>) -> AssetId {
        match chain {
            AssetChain::Evm => AssetId::evm(token),
            AssetChain::Solana => AssetId::solana(token),
        }
    }

    /// Which settlement chain this token lives on.
    pub const fn chain(&self) -> AssetChain {
        self.chain
    }

    /// The contract or mint identity, canonicalised.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Display for AssetId {
    /// `evm:0xa0b8...` / `solana:EPjF...` -- one spelling, and the one this
    /// type reads back through [`FromStr`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{CHAIN_SEPARATOR}{}", self.chain, self.token)
    }
}

impl FromStr for AssetId {
    type Err = AssetIdError;

    /// Reads `<chain>:<token>`, refusing an unnamespaced string and an
    /// unknown chain **by name** rather than guessing at either. A token
    /// containing the separator keeps it: the split is at the first one, so
    /// the chain is unambiguous and the rest is the token exactly as written.
    fn from_str(s: &str) -> Result<AssetId, AssetIdError> {
        let (chain, token) = s
            .split_once(CHAIN_SEPARATOR)
            .ok_or_else(|| AssetIdError::Unnamespaced(s.to_string()))?;
        if token.is_empty() {
            return Err(AssetIdError::EmptyToken(s.to_string()));
        }
        Ok(AssetId::new(chain.parse::<AssetChain>()?, token))
    }
}

impl Serialize for AssetId {
    /// As the one text form, so an asset is a JSON *string* and can therefore
    /// be a JSON object key -- which is what an operator-surface projection of
    /// a table keyed by a pair of these needs.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<AssetId, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<AssetId>().map_err(D::Error::custom)
    }
}

/// What can be wrong with the text form of an asset. Every variant names the
/// text it refused, because the whole point of refusing by name (ADR 0009) is
/// that the operator learns which row was wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssetIdError {
    /// A chain this connector has no settlement backend for.
    #[error(
        "'{0}' is not a chain this connector settles on; an asset is named \
         '{EVM_NAMESPACE}:<contract>' or '{SOLANA_NAMESPACE}:<mint>'"
    )]
    UnknownChain(String),

    /// No `<chain>:` prefix at all. A bare contract address names a token on
    /// no particular chain, and two chains can hold the same address.
    #[error(
        "'{0}' names no chain; an asset is named '{EVM_NAMESPACE}:<contract>' or \
         '{SOLANA_NAMESPACE}:<mint>'"
    )]
    Unnamespaced(String),

    /// A chain and a separator, and nothing after it.
    #[error("'{0}' names a chain but no token")]
    EmptyToken(String),
}

/// `0x` plus lowercase hex when `contract` has the shape of an EVM address --
/// 40 hex characters, with or without the prefix -- and `contract` untouched
/// otherwise.
///
/// Untouched rather than refused: this crate cannot tell a mistyped address
/// from an address format it has not been taught, and inventing a token by
/// rewriting text it does not recognise is the one outcome worth avoiding.
/// Config load is where a contract meets a chain that can say whether it
/// exists.
fn canonical_evm_contract(contract: &str) -> String {
    let body = contract.strip_prefix("0x").unwrap_or(contract);
    if body.len() == EVM_ADDRESS_HEX_LEN && body.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{}", body.to_ascii_lowercase())
    } else {
        contract.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// USDC on Base, as an explorer shows it.
    const USDC_CHECKSUMMED: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const USDC_LOWERCASE: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    /// USDC on Solana.
    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    #[test]
    fn a_checksummed_address_and_a_lowercase_one_are_the_same_asset() {
        assert_eq!(AssetId::evm(USDC_CHECKSUMMED), AssetId::evm(USDC_LOWERCASE));
        assert_eq!(AssetId::evm(USDC_CHECKSUMMED).token(), USDC_LOWERCASE);
    }

    #[test]
    fn an_unprefixed_address_is_the_same_asset_as_a_prefixed_one() {
        let unprefixed = USDC_CHECKSUMMED.trim_start_matches("0x");
        assert_eq!(AssetId::evm(unprefixed), AssetId::evm(USDC_CHECKSUMMED));
    }

    #[test]
    fn two_chains_holding_the_same_text_are_two_assets() {
        // The one thing canonicalisation must never do.
        assert_ne!(AssetId::evm(USDC_MINT), AssetId::solana(USDC_MINT));
    }

    #[test]
    fn a_solana_mint_is_taken_byte_for_byte() {
        // Base58 is case-sensitive: lowercasing it would name a different
        // account, or none.
        assert_eq!(AssetId::solana(USDC_MINT).token(), USDC_MINT);
        assert_ne!(
            AssetId::solana(USDC_MINT),
            AssetId::solana(USDC_MINT.to_ascii_lowercase())
        );
    }

    #[test]
    fn text_that_is_not_an_address_is_left_exactly_as_found() {
        for odd in ["not-an-address", "0x", "0xdeadbeef", ""] {
            assert_eq!(AssetId::evm(odd).token(), odd);
        }
    }

    #[test]
    fn an_asset_reads_back_from_its_own_spelling() {
        let usdc = AssetId::evm(USDC_CHECKSUMMED);
        assert_eq!(usdc.to_string(), format!("evm:{USDC_LOWERCASE}"));
        assert_eq!(
            usdc.to_string().parse::<AssetId>().expect("reads back"),
            usdc
        );
    }

    #[test]
    fn an_unknown_chain_is_refused_by_name() {
        let error = "mina:B62qFoo"
            .parse::<AssetId>()
            .expect_err("a chain with no backend is not an asset");
        assert_eq!(error, AssetIdError::UnknownChain("mina".to_string()));
        assert!(error.to_string().contains("mina"), "got: {error}");
    }

    #[test]
    fn a_bare_address_names_no_chain_and_is_refused() {
        let error = USDC_LOWERCASE
            .parse::<AssetId>()
            .expect_err("a bare address is on no particular chain");
        assert!(
            matches!(error, AssetIdError::Unnamespaced(_)),
            "got: {error}"
        );
    }

    #[test]
    fn a_chain_with_no_token_is_refused() {
        let error = "evm:".parse::<AssetId>().expect_err("no token");
        assert!(matches!(error, AssetIdError::EmptyToken(_)), "got: {error}");
    }

    #[test]
    fn an_asset_serializes_as_a_string_so_it_can_key_a_map() {
        use std::collections::BTreeMap;

        let mut table = BTreeMap::new();
        table.insert(AssetId::evm(USDC_CHECKSUMMED).to_string(), 1u64);
        assert_eq!(
            serde_json::to_string(&table).expect("serializes"),
            format!(r#"{{"evm:{USDC_LOWERCASE}":1}}"#)
        );
        assert_eq!(
            serde_json::to_string(&AssetId::solana(USDC_MINT)).expect("serializes"),
            format!(r#""solana:{USDC_MINT}""#)
        );
    }

    #[test]
    fn a_deserialized_asset_is_refused_in_this_modules_own_words() {
        let error = serde_json::from_str::<AssetId>(r#""bitcoin:1A1zP1""#)
            .expect_err("a chain with no backend is not an asset");
        assert!(error.to_string().contains("bitcoin"), "got: {error}");
    }

    proptest! {
        #[test]
        fn canonicalising_is_idempotent(token in "\\PC{0,64}") {
            let once = AssetId::evm(&token);
            prop_assert_eq!(AssetId::evm(once.token()), once);
        }

        #[test]
        fn an_asset_round_trips_through_its_text_form(
            chain in prop::sample::select(vec![AssetChain::Evm, AssetChain::Solana]),
            token in "[A-Za-z0-9]{1,64}",
        ) {
            let asset = AssetId::new(chain, &token);
            prop_assert_eq!(asset.to_string().parse::<AssetId>().expect("reads back"), asset);
        }

        #[test]
        fn an_asset_round_trips_through_serde(
            chain in prop::sample::select(vec![AssetChain::Evm, AssetChain::Solana]),
            token in "[A-Za-z0-9]{1,64}",
        ) {
            let asset = AssetId::new(chain, &token);
            let json = serde_json::to_string(&asset).expect("serializes");
            let read: AssetId = serde_json::from_str(&json).expect("reads back");
            prop_assert_eq!(read, asset);
        }

        #[test]
        fn recasing_a_hex_address_never_changes_which_asset_it_is(
            body in "[0-9a-fA-F]{40}",
        ) {
            prop_assert_eq!(
                AssetId::evm(body.to_ascii_uppercase()),
                AssetId::evm(body.to_ascii_lowercase())
            );
        }
    }
}
