//! Configuration: one typed file, fully validated at boot, held as an
//! immutable value for the process lifetime. See ADR 0001, ADR 0009.
//!
//! There is no environment-variable override layer -- [`Config::load`] reads
//! exactly one file, so there is exactly one place any value comes from.
//! Convenience forms (`[[children]]`) are desugared here, so the rest of the
//! connector only ever sees ordinary [`StaticRoute`]s. Key material is never
//! read into this crate; a [`SecretLocation`] is a pointer (a file path or a
//! KMS identifier), validated for presence but not for content.
//!
//! There is no exception for a peering, and there used to be one: a
//! `[[peers]] credential` held a shared secret this crate read in full. ADR
//! 0060 deleted it -- a peering is proven by a verified claim on one of its
//! `[[peer_channels]]` rows, so there is no bearer string left to compare
//! against and none to keep out of a `Debug` rendering. The key is parsed
//! solely to be refused by name ([`ConfigError::PeerCredentialRemoved`]).

mod batch_settlement;
mod client_channel;
mod client_channel_asset;
mod config;
mod denomination;
mod error;
mod identity;
mod node;
mod operator;
mod pay_channel;
mod peer;
mod peer_channel;
mod peering_asset;
mod route;
mod secret;
mod settlement;

pub use batch_settlement::{
    EvmBatchSettlementConfig, SolanaBatchSettlementConfig, BATCH_SETTLEMENT_DELAY_FLOOR_SECS,
    DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS, EVM_BATCH_SETTLEMENT_MAX_WITHDRAW_DELAY_SECS,
};
pub use client_channel::{ClientChannelConfig, EvmClientChannelConfig, SolanaClientChannelConfig};
pub use client_channel_asset::ClientChannelAssets;
pub use config::{parse_socks_proxy, Config};
pub use denomination::{DeclaredToken, DenominationConfig, QuoteLeg, QuotePath, RateRow};
pub use error::ConfigError;
pub use identity::ClientIdentityConfig;
pub use node::NodeConfig;
pub use operator::OperatorConfig;
pub use pay_channel::{EvmPayChannelConfig, PayChannelConfig, SolanaPayChannelConfig};
pub use peer::{
    is_onion_endpoint, plaintext_permitted, ForwardedClaimEnforcement, PeerCarriage, PeerConfig,
    PeerExposure, DEFAULT_MAX_PACKET_AMOUNT, DEFAULT_PEER_TIMEOUT_MS,
};
pub use peer_channel::{EvmPeerChannelConfig, PeerChannelConfig, SolanaPeerChannelConfig};
pub use peering_asset::PeeringAssets;
pub use route::{PeerRouteConfig, StaticRoute, TransportPolicy};
pub use secret::SecretLocation;
pub use settlement::{
    EvmSettlementConfig, SettlementChain, SettlementConfig, SolanaSettlementConfig,
    UnknownSettlementChain, DEFAULT_CHANNEL_INDEX_CONFIRMATIONS,
};
