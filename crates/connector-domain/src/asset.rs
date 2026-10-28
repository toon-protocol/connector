//! Which token an amount is denominated in
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md),
//! issue #1288), spelled in CAIP-19
//! ([ADR 0086](../../../docs/adr/0086-a-node-settles-on-more-than-one-evm-network.md)
//! decision 2, issue #1598).
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
//! That identity is a **network** and a token on it, and nothing else. In
//! particular it carries **no decimals**: ADR 0071 decision 4 folds the scale
//! difference into the declared ratio, and a settlement table's `decimals`
//! stays what it is -- a boot-time assertion against the chain, never an input
//! to value arithmetic.
//!
//! # Why the network, not the family
//!
//! Every anvil, like Arc mainnet and Arc testnet, puts USDC at one address, and
//! x402 deploys its contract beside Circle USDC on many EVM networks. An asset
//! that said only `evm` could not tell USDC on Base from USDC on Arbitrum, so a
//! node settling on both would file two different tokens under one rate key.
//! So an [`AssetId`] carries a [`Network`] (CAIP-2), and its family
//! ([`AssetChain`]) is read off that network.
//!
//! # One spelling: CAIP-19
//!
//! `<caip2 network>/<asset namespace>:<asset reference>`, and this connector
//! reads exactly three asset namespaces:
//!
//! * `eip155:<chainId>/erc20:0x<40 hex>` -- an ERC-20, by contract. The hex is
//!   case-insensitive on the way in and printed lowercase, so the checksummed
//!   spelling an explorer shows and the lowercase one a config was pasted with
//!   are one asset. Canonicalisation only merges spellings of one token.
//! * `solana:<genesis prefix>/token:<base58 mint>` -- an SPL mint, taken byte
//!   for byte: base58 has exactly one spelling already, and lowercasing it
//!   would name a different account.
//! * `<network>/slip44:<coin type>` -- a network's native coin, by its SLIP-44
//!   coin type (owner, 2026-10-28, on #1595). Native coins are CAIP-19 too.
//!
//! # Legacy spellings are refused by name
//!
//! `evm:0x…`, `solana:<mint>`, `evm:<chainId>:0x…` and `<chain>:native` are
//! recognised **only to be refused**, each with its CAIP-19 rewrite in the
//! message. `evm:<chainId>:0x…` matters most: the parser this replaced split at
//! the first `:` and would have read it as a token called `<chainId>:0x…`
//! rather than refusing it. Nothing is guessed (ADR 0009).
//!
//! # What this module does not check
//!
//! Whether the contract exists, whether the node has a settlement table for
//! its network, whether it is the token a peering actually holds. Those are
//! load- and boot-time questions, and this crate has no I/O (ADR 0001).

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::client_claim::{EVM_NAMESPACE, SOLANA_NAMESPACE};
use crate::network::{Network, NetworkError};

/// How many hex characters an EVM contract address is, unprefixed.
const EVM_ADDRESS_HEX_LEN: usize = 40;

/// The base58 alphabet (Bitcoin's): no `0`, `O`, `I` or `l`.
const BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// The shortest and longest base58 spelling of a 32-byte Solana account.
const SOLANA_MINT_LEN: std::ops::RangeInclusive<usize> = 32..=44;

/// CAIP-19's asset namespace for an ERC-20 on an `eip155` network.
pub const ERC20_NAMESPACE: &str = "erc20";
/// CAIP-19's asset namespace for an SPL mint on a `solana` network.
pub const SOLANA_TOKEN_NAMESPACE: &str = "token";
/// CAIP-19's asset namespace for a network's native coin, by SLIP-44 coin
/// type.
pub const SLIP44_NAMESPACE: &str = "slip44";

/// The settlement family a token lives in -- the two this connector has a
/// backend type for, named with the same two strings the rest of the workspace
/// names them with ([`EVM_NAMESPACE`], [`SOLANA_NAMESPACE`]).
///
/// The family says which claim parser and which backend type; the
/// [`Network`] says which instance. An asset's family is always read off its
/// network ([`AssetId::chain`]), never stated beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetChain {
    /// An `eip155` network: one `[settlement.eip155.<chainId>]` table each.
    Evm,
    /// The cluster `[settlement.solana]` names.
    Solana,
}

impl AssetChain {
    /// The family's one spelling: `"evm"` or `"solana"`.
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

/// Which kind of thing on a network an asset is: CAIP-19's asset namespace,
/// restricted to the three this connector reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetNamespace {
    /// An ERC-20 contract on an `eip155` network.
    Erc20,
    /// An SPL mint on a `solana` network.
    Token,
    /// A network's native coin, by SLIP-44 coin type.
    Slip44,
}

impl AssetNamespace {
    /// The namespace's CAIP-19 spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            AssetNamespace::Erc20 => ERC20_NAMESPACE,
            AssetNamespace::Token => SOLANA_TOKEN_NAMESPACE,
            AssetNamespace::Slip44 => SLIP44_NAMESPACE,
        }
    }
}

impl fmt::Display for AssetNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One token, by the network it is on and its identity there.
///
/// The key half of a rate: ADR 0071 declares a rate per **ordered** pair of
/// these, and direction is the trade -- `X -> Y` and `Y -> X` are different
/// prices from the same mid, so the pair is ordered and never sorted. Because
/// the network is part of the key, a rate-table lookup keys on network as well
/// as token: USDC on Base and USDC on Arbitrum are two assets.
///
/// [`Ord`] is derived purely so a pair of these can be put in a `BTreeMap` and
/// an error can list assets in a stable order. It says nothing about value.
///
/// Boxed, so an asset is one pointer wide: config errors and rate-source
/// errors carry two or three of these, and a `Result` is as large as its
/// largest error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(Box<AssetParts>);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct AssetParts {
    network: Network,
    namespace: AssetNamespace,
    reference: String,
}

impl AssetId {
    /// A token by its contract or mint on `network`: an `erc20` on an
    /// `eip155` network, a `token` on a `solana` one. An EVM contract is
    /// canonicalised to `0x` plus lowercase hex whenever it has an address's
    /// shape; a Solana mint is taken byte for byte.
    ///
    /// Infallible, for a caller holding a value it already validated (a
    /// settlement table's parsed `token_address`). Text from an operator goes
    /// through [`FromStr`], which accepts exactly the CAIP-19 grammar.
    pub fn token_on(network: Network, token: impl AsRef<str>) -> AssetId {
        let token = token.as_ref();
        match network {
            Network::Eip155(_) => AssetId(Box::new(AssetParts {
                network,
                namespace: AssetNamespace::Erc20,
                reference: canonical_evm_contract(token),
            })),
            Network::Solana(_) => AssetId(Box::new(AssetParts {
                network,
                namespace: AssetNamespace::Token,
                reference: token.to_string(),
            })),
        }
    }

    /// An ERC-20 on the `eip155` network `chain_id`.
    pub fn erc20(chain_id: u64, contract: impl AsRef<str>) -> AssetId {
        AssetId::token_on(Network::Eip155(chain_id), contract)
    }

    /// `network`'s native coin, by its SLIP-44 coin type.
    pub fn slip44(network: Network, coin_type: u32) -> AssetId {
        AssetId(Box::new(AssetParts {
            network,
            namespace: AssetNamespace::Slip44,
            reference: coin_type.to_string(),
        }))
    }

    /// The CAIP-2 network this token is on.
    pub const fn network(&self) -> &Network {
        &self.0.network
    }

    /// Which settlement family this token lives in, read off its network.
    pub const fn chain(&self) -> AssetChain {
        self.0.network.chain()
    }

    /// CAIP-19's asset namespace.
    pub const fn namespace(&self) -> AssetNamespace {
        self.0.namespace
    }

    /// The asset reference, canonicalised: the contract, the mint, or the
    /// SLIP-44 coin type in decimal.
    pub fn token(&self) -> &str {
        &self.0.reference
    }
}

impl fmt::Display for AssetId {
    /// `eip155:8453/erc20:0x8335…` / `solana:<prefix>/token:EPjF…` /
    /// `eip155:1/slip44:60` -- the one spelling, and the one this type reads
    /// back through [`FromStr`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}:{}",
            self.0.network, self.0.namespace, self.0.reference
        )
    }
}

impl FromStr for AssetId {
    type Err = AssetIdError;

    /// Reads exactly the CAIP-19 grammar this connector settles: refuses a
    /// legacy spelling by name with its rewrite, and refuses everything else
    /// without guessing at it.
    fn from_str(s: &str) -> Result<AssetId, AssetIdError> {
        if let Some(legacy) = legacy_refusal(s) {
            return Err(legacy);
        }
        let not_caip19 = || AssetIdError::NotCaip19(s.to_string());
        let (network, asset) = s.split_once('/').ok_or_else(not_caip19)?;
        let network = network
            .parse::<Network>()
            .map_err(|source| AssetIdError::Network {
                value: s.to_string(),
                source,
            })?;
        let (namespace, reference) = asset.split_once(':').ok_or_else(not_caip19)?;
        let namespace = match (namespace, &network) {
            (ERC20_NAMESPACE, Network::Eip155(_)) => AssetNamespace::Erc20,
            (SOLANA_TOKEN_NAMESPACE, Network::Solana(_)) => AssetNamespace::Token,
            (SLIP44_NAMESPACE, _) => AssetNamespace::Slip44,
            _ => {
                return Err(AssetIdError::UnknownAssetNamespace {
                    value: s.to_string(),
                    namespace: namespace.to_string(),
                    network,
                })
            }
        };
        let invalid = || AssetIdError::InvalidReference {
            value: s.to_string(),
            namespace,
        };
        let reference = match namespace {
            AssetNamespace::Erc20 => {
                let body = reference.strip_prefix("0x").ok_or_else(invalid)?;
                if !is_evm_address_body(body) {
                    return Err(invalid());
                }
                format!("0x{}", body.to_ascii_lowercase())
            }
            AssetNamespace::Token => {
                if !SOLANA_MINT_LEN.contains(&reference.len()) || !is_base58(reference) {
                    return Err(invalid());
                }
                reference.to_string()
            }
            AssetNamespace::Slip44 => {
                if reference.is_empty()
                    || !reference.bytes().all(|b| b.is_ascii_digit())
                    || (reference.len() > 1 && reference.starts_with('0'))
                {
                    return Err(invalid());
                }
                reference.parse::<u32>().map_err(|_| invalid())?.to_string()
            }
        };
        Ok(AssetId(Box::new(AssetParts {
            network,
            namespace,
            reference,
        })))
    }
}

/// The CAIP-19 rewrite of a legacy spelling, when `s` is one -- recognised
/// only so the refusal can name it and say what to write instead.
fn legacy_refusal(s: &str) -> Option<AssetIdError> {
    let (family, rest) = s.split_once(':')?;
    if rest == "native" {
        return Some(AssetIdError::LegacyNative {
            value: s.to_string(),
        });
    }
    match family {
        EVM_NAMESPACE => {
            // `evm:<chainId>:0x…`: the shape that the old parser read as a
            // different token. The chain id is right there, so the rewrite is
            // exact.
            if let Some((chain_id, contract)) = rest.split_once(':') {
                if let Ok(chain_id) = Network::parse_eip155_chain_id(chain_id) {
                    return Some(AssetIdError::LegacyEvmWithChainId {
                        value: s.to_string(),
                        rewrite: format!(
                            "{}/{ERC20_NAMESPACE}:{}",
                            Network::Eip155(chain_id),
                            canonical_evm_contract(contract)
                        ),
                    });
                }
            }
            Some(AssetIdError::LegacyEvm {
                value: s.to_string(),
                contract: canonical_evm_contract(rest),
            })
        }
        // A `solana:` with no `/` is the legacy mint spelling -- or a CAIP-2
        // network with its asset missing, which is the same mistake to fix.
        SOLANA_NAMESPACE if !rest.contains('/') => Some(AssetIdError::LegacySolana {
            value: s.to_string(),
            mint: rest.to_string(),
        }),
        _ => None,
    }
}

impl Serialize for AssetId {
    /// As the one text form, so an asset is a JSON *string* and can therefore
    /// be a JSON object key -- which is what an operator-surface projection of
    /// a table keyed by a pair of these needs.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
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
/// that the operator learns which row was wrong -- and every legacy variant
/// carries the CAIP-19 rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssetIdError {
    /// The retired `evm:0x…` spelling, which says no network.
    #[error(
        "'{value}' is the retired 'evm:' asset spelling, which does not say which EVM network \
         the token is on: write it in CAIP-19, eip155:<chainId>/erc20:{contract} (ADR 0086)"
    )]
    LegacyEvm { value: String, contract: String },

    /// `evm:<chainId>:0x…`, refused rather than read as a token called
    /// `<chainId>:0x…`.
    #[error(
        "'{value}' is not an asset spelling this connector reads: write it in CAIP-19, \
         {rewrite} (ADR 0086)"
    )]
    LegacyEvmWithChainId { value: String, rewrite: String },

    /// The retired `solana:<mint>` spelling, which says no cluster.
    #[error(
        "'{value}' is the retired 'solana:<mint>' asset spelling, which does not say which \
         cluster the mint is on: write it in CAIP-19, solana:<genesis prefix>/token:{mint} \
         (ADR 0086)"
    )]
    LegacySolana { value: String, mint: String },

    /// `<chain>:native`, refused: a native coin is CAIP-19 too.
    #[error(
        "'{value}' is not an asset spelling this connector reads: a native coin is CAIP-19 \
         too, <caip2 network>/slip44:<coin type> (e.g. eip155:1/slip44:60) (ADR 0086)"
    )]
    LegacyNative { value: String },

    /// Text with no `<network>/<namespace>:<reference>` shape at all.
    #[error(
        "'{0}' is not a CAIP-19 asset: write eip155:<chainId>/erc20:0x<contract>, \
         solana:<genesis prefix>/token:<mint>, or <network>/slip44:<coin type> (ADR 0086)"
    )]
    NotCaip19(String),

    /// The network half is not a CAIP-2 network this connector settles on.
    #[error("'{value}' does not name a network: {source}")]
    Network {
        value: String,
        #[source]
        source: NetworkError,
    },

    /// An asset namespace this connector does not read on that network.
    #[error(
        "'{value}' names the asset namespace '{namespace}', which this connector does not read \
         on {network}: an eip155 network holds erc20 and slip44 assets, a solana network \
         token and slip44 assets (ADR 0086)"
    )]
    UnknownAssetNamespace {
        value: String,
        namespace: String,
        network: Network,
    },

    /// A reference that is not one in its namespace.
    #[error(
        "'{value}' is not a valid {namespace} asset: an erc20 reference is 0x and 40 hex \
         characters, a token reference a base58 mint, a slip44 reference a decimal coin type"
    )]
    InvalidReference {
        value: String,
        namespace: AssetNamespace,
    },
}

/// Whether `body` is 40 hex characters.
fn is_evm_address_body(body: &str) -> bool {
    body.len() == EVM_ADDRESS_HEX_LEN && body.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_base58(text: &str) -> bool {
    text.chars().all(|c| BASE58_ALPHABET.contains(c))
}

/// `0x` plus lowercase hex when `contract` has the shape of an EVM address --
/// 40 hex characters, with or without the prefix -- and `contract` untouched
/// otherwise. Only used where the caller already holds a validated address,
/// or to print a legacy spelling's rewrite.
fn canonical_evm_contract(contract: &str) -> String {
    let body = contract.strip_prefix("0x").unwrap_or(contract);
    if is_evm_address_body(body) {
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
    /// Solana devnet's genesis prefix.
    const DEVNET: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

    fn devnet() -> Network {
        format!("solana:{DEVNET}").parse().unwrap()
    }

    #[test]
    fn an_erc20_reads_from_and_prints_as_caip19_lowercase() {
        let text = format!("eip155:8453/erc20:{USDC_CHECKSUMMED}");
        let usdc: AssetId = text.parse().expect("a CAIP-19 erc20");
        assert_eq!(usdc, AssetId::erc20(8453, USDC_LOWERCASE));
        assert_eq!(
            usdc.to_string(),
            format!("eip155:8453/erc20:{USDC_LOWERCASE}")
        );
        assert_eq!(usdc.chain(), AssetChain::Evm);
        assert_eq!(usdc.network(), &Network::Eip155(8453));
        assert_eq!(usdc.token(), USDC_LOWERCASE);
    }

    #[test]
    fn a_solana_mint_reads_from_and_prints_as_caip19_byte_for_byte() {
        let text = format!("solana:{DEVNET}/token:{USDC_MINT}");
        let usdc: AssetId = text.parse().expect("a CAIP-19 token");
        assert_eq!(usdc.to_string(), text);
        assert_eq!(usdc.chain(), AssetChain::Solana);
        assert_eq!(usdc.token(), USDC_MINT);
        assert_eq!(usdc, AssetId::token_on(devnet(), USDC_MINT));
    }

    #[test]
    fn one_contract_on_two_networks_is_two_assets() {
        // Every anvil seeds USDC at one address; Base and Arbitrum USDC are
        // different tokens, so a rate keyed on them must be too.
        assert_ne!(
            AssetId::erc20(8453, USDC_LOWERCASE),
            AssetId::erc20(42161, USDC_LOWERCASE)
        );
    }

    #[test]
    fn a_slip44_asset_parses_and_prints_in_caip19_form() {
        let eth: AssetId = "eip155:1/slip44:60".parse().expect("ETH");
        assert_eq!(eth, AssetId::slip44(Network::Eip155(1), 60));
        assert_eq!(eth.to_string(), "eip155:1/slip44:60");
        assert_eq!(eth.namespace(), AssetNamespace::Slip44);

        let sol: AssetId = format!("solana:{DEVNET}/slip44:501").parse().expect("SOL");
        assert_eq!(sol.to_string(), format!("solana:{DEVNET}/slip44:501"));
        assert_eq!(sol.chain(), AssetChain::Solana);
    }

    #[test]
    fn the_retired_evm_spelling_is_refused_with_its_rewrite() {
        let error = format!("evm:{USDC_CHECKSUMMED}")
            .parse::<AssetId>()
            .expect_err("retired");
        assert!(matches!(error, AssetIdError::LegacyEvm { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(&format!("eip155:<chainId>/erc20:{USDC_LOWERCASE}")),
            "{error}"
        );
    }

    #[test]
    fn an_evm_spelling_with_a_chain_id_is_refused_with_its_exact_rewrite() {
        let error = format!("evm:8453:{USDC_CHECKSUMMED}")
            .parse::<AssetId>()
            .expect_err("not read as a token called '8453:0x…'");
        assert!(
            error
                .to_string()
                .contains(&format!("eip155:8453/erc20:{USDC_LOWERCASE}")),
            "{error}"
        );
    }

    #[test]
    fn the_retired_solana_spelling_is_refused_with_its_rewrite() {
        let error = format!("solana:{USDC_MINT}")
            .parse::<AssetId>()
            .expect_err("retired");
        assert!(
            error
                .to_string()
                .contains(&format!("solana:<genesis prefix>/token:{USDC_MINT}")),
            "{error}"
        );
    }

    #[test]
    fn a_native_spelling_is_refused_by_name() {
        for value in ["evm:native", "solana:native", "xrpl:native"] {
            let error = value.parse::<AssetId>().expect_err("retired");
            assert!(
                matches!(error, AssetIdError::LegacyNative { .. }),
                "{value}"
            );
            assert!(error.to_string().contains("slip44"), "{error}");
        }
    }

    #[test]
    fn a_namespace_on_the_wrong_network_is_refused() {
        for value in [
            format!("eip155:1/token:{USDC_MINT}"),
            format!("solana:{DEVNET}/erc20:{USDC_LOWERCASE}"),
            format!("eip155:1/erc721:{USDC_LOWERCASE}"),
        ] {
            assert!(
                matches!(
                    value.parse::<AssetId>(),
                    Err(AssetIdError::UnknownAssetNamespace { .. })
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn a_malformed_reference_is_refused() {
        for value in [
            "eip155:1/erc20:0xdeadbeef".to_string(),
            format!("eip155:1/erc20:{}", USDC_LOWERCASE.trim_start_matches("0x")),
            "eip155:1/slip44:060".to_string(),
            "eip155:1/slip44:".to_string(),
            "eip155:1/slip44:4294967296".to_string(),
            format!("solana:{DEVNET}/token:0OIl"),
        ] {
            assert!(
                matches!(
                    value.parse::<AssetId>(),
                    Err(AssetIdError::InvalidReference { .. })
                ),
                "{value}"
            );
        }
    }

    #[test]
    fn an_unknown_network_is_refused_by_name() {
        let error = "cosmos:cosmoshub-4/slip44:118"
            .parse::<AssetId>()
            .expect_err("no backend");
        assert!(matches!(error, AssetIdError::Network { .. }), "{error}");
        assert!(error.to_string().contains("cosmos"), "{error}");
    }

    #[test]
    fn an_asset_serializes_as_a_string_so_it_can_key_a_map() {
        use std::collections::BTreeMap;

        let mut table = BTreeMap::new();
        table.insert(AssetId::erc20(8453, USDC_CHECKSUMMED).to_string(), 1u64);
        assert_eq!(
            serde_json::to_string(&table).expect("serializes"),
            format!(r#"{{"eip155:8453/erc20:{USDC_LOWERCASE}":1}}"#)
        );
        assert!(serde_json::from_str::<AssetId>(r#""evm:0x00""#).is_err());
    }

    fn base58(len: std::ops::RangeInclusive<usize>) -> impl Strategy<Value = String> {
        proptest::string::string_regex(&format!(
            "[123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{{{},{}}}",
            len.start(),
            len.end()
        ))
        .unwrap()
    }

    fn any_asset() -> impl Strategy<Value = AssetId> {
        prop_oneof![
            (1u64..=u64::MAX, "[0-9a-fA-F]{40}")
                .prop_map(|(id, hex)| AssetId::erc20(id, format!("0x{hex}"))),
            (base58(32..=32), base58(32..=44)).prop_map(|(prefix, mint)| {
                AssetId::token_on(Network::Solana(prefix.parse().unwrap()), mint)
            }),
            (1u64..=u64::MAX, any::<u32>())
                .prop_map(|(id, coin)| AssetId::slip44(Network::Eip155(id), coin)),
        ]
    }

    proptest! {
        #[test]
        fn every_asset_round_trips_through_caip19(asset in any_asset()) {
            let text = asset.to_string();
            prop_assert_eq!(text.parse::<AssetId>(), Ok(asset.clone()));
            let json = serde_json::to_string(&asset).unwrap();
            prop_assert_eq!(serde_json::from_str::<AssetId>(&json).unwrap(), asset);
        }

        #[test]
        fn an_erc20_prints_lowercase_whatever_case_it_was_written_in(
            id in 1u64..=u64::MAX,
            hex in "[0-9a-fA-F]{40}",
        ) {
            let asset: AssetId = format!("eip155:{id}/erc20:0x{hex}").parse().unwrap();
            prop_assert_eq!(asset.to_string(), format!("eip155:{id}/erc20:0x{}", hex.to_ascii_lowercase()));
        }

        #[test]
        fn every_legacy_spelling_is_refused_by_name_never_read(
            hex in "[0-9a-fA-F]{40}",
            id in 1u64..=u64::MAX,
            mint in base58(32..=44),
            family in "[a-z]{1,8}",
        ) {
            let refused = |text: String| text.parse::<AssetId>();
            let refusal = refused(format!("evm:0x{hex}"));
            prop_assert!(matches!(refusal, Err(AssetIdError::LegacyEvm { .. })), "{:?}", refusal);
            let refusal = refused(format!("evm:{id}:0x{hex}"));
            prop_assert!(matches!(refusal, Err(AssetIdError::LegacyEvmWithChainId { .. })), "{:?}", refusal);
            let refusal = refused(format!("solana:{mint}"));
            prop_assert!(matches!(refusal, Err(AssetIdError::LegacySolana { .. })), "{:?}", refusal);
            let refusal = refused(format!("{family}:native"));
            prop_assert!(matches!(refusal, Err(AssetIdError::LegacyNative { .. })), "{:?}", refusal);
        }

        #[test]
        fn every_string_that_parses_prints_back_canonically(
            text in "(eip155:[0-9]{1,4}|solana:[1-9A-HJ-NP-Za-km-z]{32})/(erc20|token|slip44):(0x)?[0-9a-zA-Z]{1,44}",
        ) {
            if let Ok(asset) = text.parse::<AssetId>() {
                // Parsing never merges two values: what it read reads back.
                prop_assert_eq!(asset.to_string().parse::<AssetId>(), Ok(asset.clone()));
                prop_assert_eq!(asset.to_string().to_ascii_lowercase(), text.to_ascii_lowercase());
            }
        }
    }
}
