//! Which network a value is on, in CAIP-2 form
//! ([ADR 0086](../../../docs/adr/0086-a-node-settles-on-more-than-one-evm-network.md),
//! issue #1597).
//!
//! [`AssetChain`] says which *family* a value lives in -- which parser reads a
//! claim, which backend type settles it -- and there are two. A [`Network`]
//! says which *instance* of that family: `eip155:8453` is Base and
//! `eip155:42161` Arbitrum, and both project to [`AssetChain::Evm`]. Once a
//! node may hold one settlement table per EVM network, the family no longer
//! picks a backend instance; the network does.
//!
//! # One spelling
//!
//! A network has exactly one text form, the CAIP-2 identifier x402 itself
//! carries (`accepted.network`) and the greeting already publishes:
//!
//! * `eip155:<chainId>` -- the chain id in decimal, with no sign, no leading
//!   zero and no zero, so two spellings can never name one chain;
//! * `solana:<genesis prefix>` -- the first 32 base58 characters of the
//!   cluster's genesis hash;
//! * `mina:mainnet` or `mina:devnet` -- the network a zk-note vault lives on
//!   (ADR 0080 decision 7). Mina has no settlement backend: the zk-note table
//!   is the one place a `mina` network may be named, and the config refuses it
//!   everywhere else.
//!
//! [`FromStr`], [`Display`](fmt::Display) and serde all read and write that one
//! form. Any other namespace -- including the family names `evm` and `solana`
//! on their own, and a namespace CAIP-2 allows but this connector has no
//! backend for -- is refused **by name**, never guessed at (ADR 0009).

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::asset::AssetChain;

/// CAIP-2's namespace for EVM chains identified by their EIP-155 chain id.
pub const EIP155_NAMESPACE: &str = "eip155";

/// CAIP-2's namespace for Solana clusters identified by their genesis hash.
pub const SOLANA_NETWORK_NAMESPACE: &str = "solana";

/// The CAIP-2 namespace a zk-note vault's Mina network is named in.
pub const MINA_NETWORK_NAMESPACE: &str = "mina";

/// How many base58 characters of the genesis hash CAIP-2 keeps for Solana.
pub const SOLANA_GENESIS_PREFIX_LEN: usize = 32;

/// The base58 alphabet (Bitcoin's): no `0`, `O`, `I` or `l`.
const BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// One settlement network, as CAIP-2 names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Network {
    /// An EVM chain, by its EIP-155 chain id.
    Eip155(u64),
    /// A Solana cluster, by the first 32 base58 characters of its genesis
    /// hash.
    Solana(SolanaGenesisPrefix),
    /// The Mina network a zk-note vault lives on.
    Mina(MinaNetwork),
}

/// The two Mina networks a zk-note vault may live on, as CAIP-2 names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MinaNetwork {
    /// `mina:mainnet`.
    Mainnet,
    /// `mina:devnet`.
    Devnet,
}

impl MinaNetwork {
    /// The CAIP-2 network id: what the greeting's `network` and the app's
    /// `X-TOON-Chain` name.
    pub const fn caip2(self) -> &'static str {
        match self {
            MinaNetwork::Mainnet => "mina:mainnet",
            MinaNetwork::Devnet => "mina:devnet",
        }
    }

    /// The reference half: `mainnet` or `devnet`.
    pub const fn reference(self) -> &'static str {
        match self {
            MinaNetwork::Mainnet => "mainnet",
            MinaNetwork::Devnet => "devnet",
        }
    }
}

impl FromStr for MinaNetwork {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, NetworkError> {
        match s {
            "mainnet" => Ok(MinaNetwork::Mainnet),
            "devnet" => Ok(MinaNetwork::Devnet),
            other => Err(NetworkError::InvalidMinaNetwork(other.to_string())),
        }
    }
}

/// The reference half of a `solana:` network: exactly
/// [`SOLANA_GENESIS_PREFIX_LEN`] base58 characters. Only constructible by
/// parsing, so a value that exists is one CAIP-2 accepts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SolanaGenesisPrefix(String);

impl SolanaGenesisPrefix {
    /// The prefix as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SolanaGenesisPrefix {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, NetworkError> {
        if s.len() == SOLANA_GENESIS_PREFIX_LEN && s.chars().all(|c| BASE58_ALPHABET.contains(c)) {
            Ok(SolanaGenesisPrefix(s.to_string()))
        } else {
            Err(NetworkError::InvalidSolanaGenesisPrefix(s.to_string()))
        }
    }
}

impl Network {
    /// The chain family this network belongs to -- which claim parser reads
    /// a value on it and which backend type settles it.
    pub const fn chain(&self) -> AssetChain {
        match self {
            Network::Eip155(_) => AssetChain::Evm,
            Network::Solana(_) => AssetChain::Solana,
            Network::Mina(_) => AssetChain::Mina,
        }
    }

    /// The EIP-155 chain id, for an `eip155` network.
    pub const fn eip155_chain_id(&self) -> Option<u64> {
        match self {
            Network::Eip155(chain_id) => Some(*chain_id),
            Network::Solana(_) | Network::Mina(_) => None,
        }
    }

    /// Read an `eip155` reference -- the chain id alone, as a
    /// `[settlement.eip155.<chainId>]` table key writes it -- under the same
    /// rule [`FromStr`] reads one after `eip155:`: decimal digits only, no
    /// leading zero, not zero, and within `u64`.
    pub fn parse_eip155_chain_id(reference: &str) -> Result<u64, NetworkError> {
        let invalid = || NetworkError::InvalidEip155ChainId(reference.to_string());
        if reference.is_empty()
            || !reference.bytes().all(|b| b.is_ascii_digit())
            || reference.starts_with('0')
        {
            return Err(invalid());
        }
        reference.parse::<u64>().map_err(|_| invalid())
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Eip155(chain_id) => write!(f, "{EIP155_NAMESPACE}:{chain_id}"),
            Network::Solana(prefix) => write!(f, "{SOLANA_NETWORK_NAMESPACE}:{}", prefix.0),
            Network::Mina(network) => f.write_str(network.caip2()),
        }
    }
}

impl FromStr for Network {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Network, NetworkError> {
        let (namespace, reference) = s
            .split_once(':')
            .ok_or_else(|| NetworkError::NotCaip2(s.to_string()))?;
        match namespace {
            EIP155_NAMESPACE => Ok(Network::Eip155(Network::parse_eip155_chain_id(reference)?)),
            SOLANA_NETWORK_NAMESPACE => Ok(Network::Solana(reference.parse()?)),
            MINA_NETWORK_NAMESPACE => Ok(Network::Mina(reference.parse()?)),
            other => Err(NetworkError::UnknownNamespace {
                namespace: other.to_string(),
                value: s.to_string(),
            }),
        }
    }
}

impl Serialize for Network {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Network {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Network, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// Why a string is not a [`Network`]. Each names what it was given.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetworkError {
    /// No `<namespace>:<reference>` separator at all.
    #[error(
        "'{0}' is not a CAIP-2 network: write eip155:<chainId> or solana:<genesis prefix> \
         (ADR 0086)"
    )]
    NotCaip2(String),
    /// A namespace this connector settles on no network of. Never guessed:
    /// `evm:8453` is not read as `eip155:8453`.
    #[error(
        "'{value}' names the CAIP-2 namespace '{namespace}', which this connector does not \
         settle on: write eip155:<chainId> or solana:<genesis prefix> (ADR 0086)"
    )]
    UnknownNamespace { namespace: String, value: String },
    /// An `eip155` reference that is not a canonical decimal chain id.
    #[error(
        "'{0}' is not an EIP-155 chain id: write it in decimal, with no sign, no leading zero, \
         and not zero (ADR 0086)"
    )]
    InvalidEip155ChainId(String),
    /// A `solana` reference that is not 32 base58 characters.
    #[error(
        "'{0}' is not a Solana genesis prefix: CAIP-2 names a cluster by the first 32 base58 \
         characters of its genesis hash (ADR 0086)"
    )]
    InvalidSolanaGenesisPrefix(String),
    /// A `mina` reference other than `mainnet` or `devnet`.
    #[error(
        "'{0}' is not a Mina network a zk-note vault may live on: write mina:mainnet or \
         mina:devnet (ADR 0080)"
    )]
    InvalidMinaNetwork(String),
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const DEVNET: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

    #[test]
    fn known_spellings_parse_to_the_network_they_name() {
        assert_eq!("eip155:8453".parse(), Ok(Network::Eip155(8453)));
        let solana: Network = format!("solana:{DEVNET}").parse().unwrap();
        assert_eq!(solana.to_string(), format!("solana:{DEVNET}"));
        assert_eq!(solana.chain(), AssetChain::Solana);
        let mina: Network = "mina:devnet".parse().unwrap();
        assert_eq!(mina, Network::Mina(MinaNetwork::Devnet));
        assert_eq!(mina.to_string(), "mina:devnet");
        assert_eq!(mina.chain(), AssetChain::Mina);
        assert_eq!(
            "mina:berkeley".parse::<Network>(),
            Err(NetworkError::InvalidMinaNetwork("berkeley".to_string()))
        );
        assert_eq!(Network::Eip155(1).chain(), AssetChain::Evm);
        assert_eq!(Network::Eip155(42161).eip155_chain_id(), Some(42161));
        assert_eq!(solana.eip155_chain_id(), None);
    }

    #[test]
    fn an_unknown_namespace_is_refused_by_name_never_guessed() {
        for value in [
            "evm:8453",
            "cosmos:cosmoshub-4",
            "EIP155:1",
            "hedera:mainnet",
        ] {
            let namespace = value.split_once(':').unwrap().0.to_string();
            assert_eq!(
                value.parse::<Network>(),
                Err(NetworkError::UnknownNamespace {
                    namespace,
                    value: value.to_string()
                }),
                "{value}"
            );
        }
        assert_eq!(
            "evm".parse::<Network>(),
            Err(NetworkError::NotCaip2("evm".to_string()))
        );
    }

    #[test]
    fn a_non_canonical_chain_id_is_refused() {
        for reference in [
            "",
            "0",
            "08453",
            "+8453",
            "-1",
            "0x2105",
            "base",
            "18446744073709551616",
        ] {
            assert_eq!(
                format!("eip155:{reference}").parse::<Network>(),
                Err(NetworkError::InvalidEip155ChainId(reference.to_string())),
                "{reference:?}"
            );
        }
    }

    #[test]
    fn a_malformed_solana_prefix_is_refused() {
        for reference in [
            "",
            "short",
            &format!("{DEVNET}x"),
            "0tWTRABZaYq6iMfeYKouRu166VU2xqa1",
        ] {
            assert!(
                matches!(
                    format!("solana:{reference}").parse::<Network>(),
                    Err(NetworkError::InvalidSolanaGenesisPrefix(_))
                ),
                "{reference:?}"
            );
        }
    }

    #[test]
    fn serde_uses_the_caip2_string() {
        let json = serde_json::to_string(&Network::Eip155(84532)).unwrap();
        assert_eq!(json, r#""eip155:84532""#);
        assert_eq!(
            serde_json::from_str::<Network>(&json).unwrap(),
            Network::Eip155(84532)
        );
        assert!(serde_json::from_str::<Network>(r#""evm:84532""#).is_err());
    }

    fn any_network() -> impl Strategy<Value = Network> {
        prop_oneof![
            (1u64..=u64::MAX).prop_map(Network::Eip155),
            proptest::string::string_regex(
                "[123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{32}"
            )
            .unwrap()
            .prop_map(|prefix| Network::Solana(prefix.parse().unwrap())),
            prop::sample::select(vec![MinaNetwork::Mainnet, MinaNetwork::Devnet])
                .prop_map(Network::Mina),
        ]
    }

    proptest! {
        #[test]
        fn every_network_round_trips_through_its_caip2_spelling(network in any_network()) {
            let text = network.to_string();
            prop_assert_eq!(text.parse::<Network>(), Ok(network.clone()));
            let json = serde_json::to_string(&network).unwrap();
            prop_assert_eq!(serde_json::from_str::<Network>(&json).unwrap(), network);
        }

        #[test]
        fn every_string_that_parses_prints_back_as_itself(text in "[a-z0-9]{0,8}:[0-9A-Za-z+-]{0,34}") {
            if let Ok(network) = text.parse::<Network>() {
                prop_assert_eq!(network.to_string(), text);
            }
        }
    }
}
