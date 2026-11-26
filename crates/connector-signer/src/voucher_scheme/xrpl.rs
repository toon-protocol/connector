//! XRPL's voucher scheme: the ledger's own `PaymentChannelClaim`
//! authorisation on a `PayChannel` (ADR 0087 decisions 2 and 3,
//! `xrpl-paychan-v1`), a [`VoucherScheme`].
//!
//! ```text
//! wire       = { blockchain: "xrpl", channelId: 64 hex (no 0x), maxClaimableAmount: drops decimal,
//!                signature: hex of 64 to 72 bytes (DER secp256k1, or raw Ed25519) }
//! channelId  = SHA512Half(0x0078 ‖ source ‖ destination ‖ sequence)
//! signed     = "CLM\0" ‖ channelId ‖ drops: u64 big-endian      (binds no network)
//! signer     = the channel's PublicKey, as the ledger records it
//! challenge  = tag ‖ len(network) ‖ network ‖ channelId ‖ expires (binds the network)
//! ```
//!
//! The message bytes, the key algorithms and the signature rules are
//! [`crate::xrpl`]'s, each pinned there against xrpl.js and rippled. This
//! module is the one place every caller reaches them through.

use connector_domain::client_claim::XrplVoucher;
use connector_domain::Network;

use super::{VoucherRefusal, VoucherScheme};
use crate::xrpl::{
    verify_xrpl_claim, verify_xrpl_voucher_claim_state_challenge, xrpl_channel_id,
    xrpl_voucher_claim_state_challenge_message, XrplError, XrplSigner,
};
use crate::SignerError;

/// XRPL's voucher scheme. Stateless: the domain and the recorded key are
/// passed in, from this node's configuration and from the ledger.
#[derive(Debug, Clone, Copy, Default)]
pub struct XrplVoucherScheme;

/// The XRPL network a voucher is verified on. A claim binds no network (ADR
/// 0087 decision 2), so only the claim-state challenge reads it. Always a
/// CAIP-2 name short enough for the challenge's one-byte length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrplVoucherDomain {
    network: String,
}

impl XrplVoucherDomain {
    /// The domain on the XRPL network whose `server_info.network_id` is
    /// `network_id`: `xrpl:<id>`.
    pub fn for_network_id(network_id: u32) -> XrplVoucherDomain {
        XrplVoucherDomain {
            network: format!("xrpl:{network_id}"),
        }
    }

    /// The CAIP-2 network, `xrpl:<id>`.
    pub fn network(&self) -> &str {
        &self.network
    }
}

/// What an XRPL channel's id is derived from: its creating
/// `PaymentChannelCreate`'s source account, destination and sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XrplChannelParts {
    /// The paying account's id.
    pub source: [u8; 20],
    /// The receiving account's id.
    pub destination: [u8; 20],
    /// The source's `Sequence` (or ticket sequence) on the create.
    pub sequence: u32,
}

/// An XRPL voucher with every field decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedXrplVoucher {
    /// The channel the voucher signs.
    pub channel_id: [u8; 32],
    /// The cumulative drops it authorises.
    pub max_claimable_amount: u128,
    /// DER for a secp256k1 channel key, 64 raw bytes for Ed25519.
    pub signature: Vec<u8>,
}

impl XrplVoucherScheme {
    /// The domain vouchers on `network` are verified under, or `None` for a
    /// network that is not an XRPL one.
    pub fn domain_on(network: &Network) -> Option<XrplVoucherDomain> {
        match network {
            Network::Xrpl(xrpl) => Some(XrplVoucherDomain {
                network: xrpl.caip2().to_string(),
            }),
            _ => None,
        }
    }

    /// A channel id from its wire spelling, 64 hex (`0x` tolerated, either
    /// case), or `None`.
    pub fn decode_channel_id(text: &str) -> Option<[u8; 32]> {
        let text = text.strip_prefix("0x").unwrap_or(text);
        decode_hex(text)?.try_into().ok()
    }

    /// Sign a claim for `amount` drops on `channel_id` with `key`: the
    /// outbound voucher on a channel known by its id. An amount above the
    /// ledger's ceiling is refused, never truncated.
    ///
    /// # Errors
    /// [`XrplError::AmountExceedsChain`] above [`crate::XRPL_MAX_DROPS`].
    pub fn sign(
        channel_id: &[u8; 32],
        amount: u128,
        key: &XrplSigner,
    ) -> Result<ParsedXrplVoucher, XrplError> {
        Ok(ParsedXrplVoucher {
            channel_id: *channel_id,
            max_claimable_amount: amount,
            signature: key.sign_claim(channel_id, amount)?,
        })
    }

    /// Sign the claim-state challenge on `channel_id`, valid until
    /// `expires`, with `key`.
    pub fn sign_challenge(
        domain: &XrplVoucherDomain,
        channel_id: &[u8; 32],
        expires: u64,
        key: &XrplSigner,
    ) -> Vec<u8> {
        key.sign(&Self::challenge(domain, channel_id, expires))
    }
}

impl VoucherScheme for XrplVoucherScheme {
    type Wire = XrplVoucher;
    type Voucher = ParsedXrplVoucher;
    type Signature = Vec<u8>;
    type Domain = XrplVoucherDomain;
    type ChannelParts = XrplChannelParts;
    type Recorded = [u8; 33];
    type Signer = [u8; 33];
    type KeyHolder = XrplSigner;

    fn parse(wire: &XrplVoucher) -> Result<ParsedXrplVoucher, VoucherRefusal> {
        let signature = Self::decode_signature(&wire.signature).ok_or_else(|| {
            VoucherRefusal::Malformed(
                "an XRPL voucher's 'signature' must be 64 to 72 bytes of hex".to_string(),
            )
        })?;
        let channel_id =
            Self::decode_channel_id(&wire.channel_id).ok_or(VoucherRefusal::UnreadableChannel)?;
        Ok(ParsedXrplVoucher {
            channel_id,
            max_claimable_amount: wire.max_claimable_amount,
            signature,
        })
    }

    fn encode(
        domain: &XrplVoucherDomain,
        channel: &XrplChannelParts,
        amount: u128,
        key: &XrplSigner,
    ) -> Result<ParsedXrplVoucher, SignerError> {
        Self::sign(&Self::channel_id(domain, channel), amount, key)
            .map_err(|error| SignerError::SigningFailed(error.to_string()))
    }

    fn spell(voucher: &ParsedXrplVoucher) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("blockchain".into(), "xrpl".into());
        fields.insert("channelId".into(), hex(&voucher.channel_id).into());
        fields.insert(
            "maxClaimableAmount".into(),
            voucher.max_claimable_amount.to_string().into(),
        );
        fields.insert("signature".into(), hex(&voucher.signature).into());
        fields
    }

    fn decode_signature(text: &str) -> Option<Vec<u8>> {
        decode_hex(text).filter(|bytes| (64..=72).contains(&bytes.len()))
    }

    fn channel_id(_domain: &XrplVoucherDomain, parts: &XrplChannelParts) -> [u8; 32] {
        xrpl_channel_id(&parts.source, &parts.destination, parts.sequence)
    }

    /// Always holds: the ledger entry was read by the id the voucher names,
    /// and a channel's `PublicKey` derives no id. A voucher naming another
    /// channel fails [`verify`](VoucherScheme::verify) instead, since the
    /// claim message signs the id.
    fn check_channel_id(
        _domain: &XrplVoucherDomain,
        _channel_id: &[u8; 32],
        _recorded: &[u8; 33],
    ) -> Result<(), VoucherRefusal> {
        Ok(())
    }

    fn signer(recorded: &[u8; 33]) -> [u8; 33] {
        *recorded
    }

    fn verify(
        _domain: &XrplVoucherDomain,
        voucher: &ParsedXrplVoucher,
        recorded: &[u8; 33],
    ) -> Result<(), VoucherRefusal> {
        if verify_xrpl_claim(
            recorded,
            &voucher.channel_id,
            voucher.max_claimable_amount,
            &voucher.signature,
        ) {
            Ok(())
        } else {
            Err(VoucherRefusal::SignatureInvalid)
        }
    }

    fn challenge(domain: &XrplVoucherDomain, channel_id: &[u8; 32], expires: u64) -> Vec<u8> {
        xrpl_voucher_claim_state_challenge_message(&domain.network, channel_id, expires)
            .expect("an `xrpl:<u32>` network is far shorter than 255 bytes")
    }

    fn verify_challenge(
        domain: &XrplVoucherDomain,
        channel_id: &[u8; 32],
        expires: u64,
        signature: &[u8],
        recorded: &[u8; 33],
    ) -> Result<(), VoucherRefusal> {
        if verify_xrpl_voucher_claim_state_challenge(
            recorded,
            &domain.network,
            channel_id,
            expires,
            signature,
        ) {
            Ok(())
        } else {
            Err(VoucherRefusal::SignatureInvalid)
        }
    }
}

/// Lowercase hex, no prefix: how the ledger's ids and signatures are spelled
/// on the wire.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hex of any even length, either case, no prefix, or `None`.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    text.chunks(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests;
