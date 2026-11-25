//! A chain's **voucher scheme** (spec #1685, issue #1716): everything about
//! one chain's voucher in one module -- how it is spelled on the wire, how
//! its signature bytes decode, how its channel id is derived, how it is
//! verified against the signer the chain records for its channel, and the
//! claim-state challenge that proves control of that signer without moving
//! value.
//!
//! # The interface
//!
//! [`VoucherScheme`] is the same for every chain, so adding a chain is
//! writing one implementation of it. Its associated types name what differs
//! between chains:
//!
//! | Type             | EVM ([`EvmVoucherScheme`])                    | Solana (#1717)            | XRPL (#1718)                 |
//! | ---------------- | --------------------------------------------- | ------------------------- | ---------------------------- |
//! | `Wire`           | `connector_domain`'s `EvmVoucher`             | `SolanaVoucher`           | `XrplVoucher`                |
//! | `Domain`         | [`BatchSettlementDomain`] (chain id, contract) | none                      | the XRPL network             |
//! | `ChannelParts`   | the `ChannelConfig`                           | the PDA seeds             | account, destination, sequence |
//! | `Recorded`       | the `ChannelConfig` the lookup verified       | `authorized_signer`       | the channel's `PublicKey`    |
//! | `KeyHolder`      | `dyn` [`crate::Signer`] (ADR 0012)            | `dyn` [`crate::Ed25519Signer`] | [`crate::XrplSigner`]   |
//!
//! `Domain` and `Recorded` always come from this node: its own configuration
//! and what the chain records for the channel. Nothing a voucher carries is
//! ever what it is verified against.
//!
//! # Where it lives
//!
//! Here, because of ADR 0001's two load-bearing constraints: verification
//! needs no key and outbound encoding takes a key holder, so the scheme
//! belongs in the crate that alone touches key material, and
//! `connector-domain` stays pure. The domain keeps each chain's voucher as
//! its wire shape (strings checked for shape) and hands it to the scheme.
//!
//! [`BatchSettlementDomain`]: crate::BatchSettlementDomain

mod evm;

pub use evm::{EvmVoucherScheme, ParsedEvmVoucher};

use crate::SignerError;

/// Why a scheme refused a voucher or a challenge. Each maps onto one of the
/// client edge's existing refusals, so no refusal name changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoucherRefusal {
    /// A field is not the bytes the scheme defines: a signature of the wrong
    /// length, an address that is not 20 bytes.
    Malformed(String),
    /// The channel id is not the bytes the scheme defines, so it names no
    /// channel anything could be looked up by.
    UnreadableChannel,
    /// The channel's recorded parts do not derive the channel id the voucher
    /// names.
    ChannelIdMismatch,
    /// The signature is not the recorded signer's over what it claims to
    /// sign.
    SignatureInvalid,
}

/// One chain's voucher scheme. See the module doc for what each associated
/// type is on each chain.
pub trait VoucherScheme {
    /// The voucher as `connector-domain` parses it off the wire.
    type Wire;
    /// A voucher with every field decoded into the bytes its scheme signs.
    type Voucher;
    /// A voucher's signature, decoded.
    type Signature;
    /// What the scheme verifies under, from this node's own configuration.
    type Domain;
    /// What a channel id is derived from.
    type ChannelParts;
    /// What the chain records for a channel, which a voucher is verified
    /// against.
    type Recorded;
    /// The recorded signer, as the chain names it.
    type Signer;
    /// What signs an outbound voucher. Never anything but a signer port: the
    /// scheme holds no key.
    type KeyHolder: ?Sized;

    /// Parse an inbound voucher from its wire spelling.
    fn parse(wire: &Self::Wire) -> Result<Self::Voucher, VoucherRefusal>;

    /// Produce an outbound voucher for `amount` on the channel `channel`
    /// derives, signed by `key`.
    fn encode(
        domain: &Self::Domain,
        channel: &Self::ChannelParts,
        amount: u128,
        key: &Self::KeyHolder,
    ) -> Result<Self::Voucher, SignerError>;

    /// The voucher's own fields in their wire spelling: `blockchain`,
    /// `channelId`, `maxClaimableAmount`, `signature` and any the chain
    /// adds. The envelope (`version`, `network`, `scheme`, `messageId`,
    /// `timestamp`, `senderId`) is the caller's.
    fn spell(voucher: &Self::Voucher) -> serde_json::Map<String, serde_json::Value>;

    /// Decode a signature from its wire spelling, or `None`.
    fn decode_signature(text: &str) -> Option<Self::Signature>;

    /// The channel id `parts` derive under `domain`.
    fn channel_id(domain: &Self::Domain, parts: &Self::ChannelParts) -> [u8; 32];

    /// Whether `recorded` is the channel `channel_id` names.
    fn check_channel_id(
        domain: &Self::Domain,
        channel_id: &[u8; 32],
        recorded: &Self::Recorded,
    ) -> Result<(), VoucherRefusal>;

    /// Whose signature a voucher on the channel `recorded` describes must
    /// carry.
    fn signer(recorded: &Self::Recorded) -> Self::Signer;

    /// Verify `voucher` against the channel the chain records.
    fn verify(
        domain: &Self::Domain,
        voucher: &Self::Voucher,
        recorded: &Self::Recorded,
    ) -> Result<(), VoucherRefusal>;

    /// The bytes a claim-state challenge on `channel_id`, valid until
    /// `expires`, signs: never a voucher's, so neither verifies as the other.
    fn challenge(domain: &Self::Domain, channel_id: &[u8; 32], expires: u64) -> Vec<u8>;

    /// Verify a claim-state challenge against the channel the chain records.
    fn verify_challenge(
        domain: &Self::Domain,
        channel_id: &[u8; 32],
        expires: u64,
        signature: &[u8],
        recorded: &Self::Recorded,
    ) -> Result<(), VoucherRefusal>;
}
