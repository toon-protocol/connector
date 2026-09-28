//! The claim on the wire (`peer-carriage-spec.md` §4): **the client edge's
//! claim JSON, verbatim** -- which since ADR 0075 is always a voucher.
//!
//! One claim shape, one JSON encoding, two transfer encodings. A peer claim
//! is the same JSON object `client-edge-spec.md` §1.3 defines for a client
//! claim -- an x402 `batch-settlement` voucher, `version: "1.0"`,
//! discriminated by `blockchain` -- and on BTP the `payment-channel-claim`
//! protocolData entry carries `JSON.stringify(voucher)` as raw UTF-8, no
//! base64 layer (that is a header artifact and nothing else).
//!
//! This is not a convenience. It is spec I4: one claim codec, one
//! structural validator, one signature verifier serve both edges, so a
//! change to the claim shape cannot land on one and not the other. [`parse`]
//! therefore calls [`connector_domain::client_claim::parse_client_claim`] --
//! the client edge's own validator -- rather than reading fields itself.
//!
//! **A `toon-channel` claim is refused by name** (ADR 0075 decision 8, issue
//! #1384): a claim with no `scheme`, or `scheme: "toon-channel"`, is
//! [`ClaimDecodeError::ToonChannel`], and the carriages refuse the frame or
//! request that presents one, naming the retirement
//! ([`crate::role_gate::RefusedEvidence::ToonChannelClaim`]). The emitter of
//! the retired claim (`encode`) and its in-process reading are deleted with
//! it.

use connector_btp::{ProtocolData, CLAIM_PROTOCOL, CONTENT_TYPE_TEXT};
use connector_domain::client_claim::{parse_client_claim, ClientClaim, ClientClaimError};

/// Why a `payment-channel-claim` entry could not be read as a voucher.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimDecodeError {
    /// Not valid UTF-8, so not the raw-UTF-8 JSON §4 requires.
    NotUtf8,
    /// Structurally invalid per the client edge's own validator.
    Structural(String),
    /// A claim on a chain this connector cannot judge. Only `mina` reaches
    /// this (ADR 0002 drops Mina from the Rust connector, and `ClientClaim`
    /// has no Mina variant at all).
    UnsupportedChain(&'static str),
    /// A claim with no `scheme`, or `scheme: "toon-channel"`: the retired
    /// `toon-channel` claim (ADR 0075 decision 8). Refused by name rather
    /// than as [`ClaimDecodeError::Structural`], so a straggling peer learns
    /// why.
    ToonChannel,
}

impl std::fmt::Display for ClaimDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimDecodeError::NotUtf8 => f.write_str("claim protocolData is not valid UTF-8 JSON"),
            ClaimDecodeError::Structural(reason) => write!(f, "{reason}"),
            ClaimDecodeError::UnsupportedChain(chain) => {
                write!(
                    f,
                    "'{chain}' peer claims are not verifiable by this connector"
                )
            }
            ClaimDecodeError::ToonChannel => write!(f, "{}", ClientClaimError::ToonChannel),
        }
    }
}

/// The `payment-channel-claim` protocolData entry carrying `json` as **raw
/// UTF-8** (§4) -- verbatim the client edge's existing convention
/// (`client-edge-spec.md` §1.9 step 2), with no base64 layer.
pub fn protocol_data(json: &str) -> ProtocolData {
    ProtocolData {
        name: CLAIM_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: json.as_bytes().to_vec(),
    }
}

/// More than one claim was presented on one frame or one request.
///
/// Refused, never resolved (§1.5). The connector MUST NOT pick the first,
/// the last, or a concatenation: this is the smuggling defence, and its
/// absence is how "which claim did we verify?" becomes unanswerable. The
/// carriage maps it -- BTP: an ERROR frame (`code F00`, `name
/// NotAcceptedError`); HTTP: `400` with no ILP body.
#[derive(Debug, PartialEq, Eq)]
pub struct AmbiguousClaim {
    /// How many were presented. Two or more, by construction.
    pub presented: usize,
}

impl std::fmt::Display for AmbiguousClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "more than one claim presented on one interaction ({}): ambiguous claims are \
             refused, not resolved (peer-carriage-spec.md §1.5)",
            self.presented
        )
    }
}

impl std::error::Error for AmbiguousClaim {}

/// The raw JSON of a frame's `payment-channel-claim` entry, if it carried
/// one. A frame with no claim is legal on both carriages (§10.2 item 6),
/// so `None` is an ordinary outcome and not a refusal.
///
/// **First-wins, and only safe behind [`present_from_protocol_data`].**
pub fn from_protocol_data(protocol_data: &[ProtocolData]) -> Option<&[u8]> {
    protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .map(|pd| pd.data.as_slice())
}

/// The claim a BTP frame presents, from every `payment-channel-claim` entry
/// on it (§1.5). Zero entries is `None`; two or more is [`AmbiguousClaim`],
/// **counted before anything is parsed**.
pub fn present_from_protocol_data(
    protocol_data: &[ProtocolData],
) -> Result<Option<&[u8]>, AmbiguousClaim> {
    let entries: Vec<&ProtocolData> = protocol_data
        .iter()
        .filter(|pd| pd.name == CLAIM_PROTOCOL)
        .collect();
    if entries.len() > 1 {
        return Err(AmbiguousClaim {
            presented: entries.len(),
        });
    }
    Ok(entries.first().map(|pd| pd.data.as_slice()))
}

/// Parse a claim slot through the client edge's own structural validator
/// (I4): always a voucher ([`ClientClaim::EvmVoucher`] or
/// [`ClientClaim::SolanaVoucher`]) when it succeeds. Its channel, signer and
/// signature are the receiving half's to resolve and verify
/// ([`crate::role_gate::VoucherEvidence`]).
///
/// # Errors
///
/// [`ClaimDecodeError`], naming a `toon-channel` claim and a `mina` one.
pub fn parse(raw: &[u8]) -> Result<ClientClaim, ClaimDecodeError> {
    let json = std::str::from_utf8(raw).map_err(|_| ClaimDecodeError::NotUtf8)?;
    parse_client_claim(json).map_err(|error| match error {
        ClientClaimError::Mina => ClaimDecodeError::UnsupportedChain("mina"),
        ClientClaimError::ToonChannel => ClaimDecodeError::ToonChannel,
        other => ClaimDecodeError::Structural(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base58 32-byte Solana account.
    const CHANNEL_ACCOUNT: &str = "GDDMwNyyx8uB6zrqwBFHjLLG3TBYk2F1Mh6usnNPUsqk";

    fn evm_voucher() -> serde_json::Value {
        serde_json::json!({
            "version": "1.0",
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "channelId": format!("0x{:064x}", 7),
            "maxClaimableAmount": "12500",
            "signature": format!("0x{}1b", "ab".repeat(64)),
        })
    }

    /// A well-formed voucher, on either chain, parses as one: it proves the
    /// peer role on a bound channel (ADR 0075 decision 5).
    #[test]
    fn a_voucher_parses_on_either_chain() {
        let solana = serde_json::json!({
            "version": "1.0",
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "channelId": CHANNEL_ACCOUNT,
            "maxClaimableAmount": "10",
            "expiresAt": 0,
            "signature": "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW",
        });
        assert!(matches!(
            parse(evm_voucher().to_string().as_bytes()),
            Ok(ClientClaim::EvmVoucher(_))
        ));
        assert!(matches!(
            parse(solana.to_string().as_bytes()),
            Ok(ClientClaim::SolanaVoucher(_))
        ));
    }

    /// ADR 0075 decision 8: a claim with no `scheme` -- the retired
    /// `toon-channel` claim, as a pre-ADR 0075 peer sends it -- and one
    /// naming `toon-channel` are both refused by name.
    #[test]
    fn a_toon_channel_claim_is_refused_by_name() {
        let unschemed = serde_json::json!({
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "channelId": format!("0x{:064x}", 7),
            "nonce": 4,
            "transferredAmount": "12500",
            "lockedAmount": "0",
            "locksRoot": format!("0x{}", "0".repeat(64)),
            "signature": format!("0x{}", "11".repeat(65)),
            "signerAddress": format!("0x{}", "44".repeat(20)),
        });
        let mut explicit = evm_voucher();
        explicit["scheme"] = "toon-channel".into();
        for claim in [unschemed, explicit] {
            assert_eq!(
                parse(claim.to_string().as_bytes()),
                Err(ClaimDecodeError::ToonChannel),
                "{claim}"
            );
        }
        let message = ClaimDecodeError::ToonChannel.to_string();
        assert!(message.contains("toon-channel"), "{message}");
        assert!(message.contains("ADR 0075"), "{message}");
    }

    /// §4: the entry is raw UTF-8 JSON, not base64.
    #[test]
    fn the_claim_entry_carries_raw_utf8_json() {
        let json = evm_voucher().to_string();
        let entry = protocol_data(&json);
        assert_eq!(entry.name, CLAIM_PROTOCOL);
        assert_eq!(String::from_utf8(entry.data).expect("utf-8"), json);
    }

    /// ADR 0002: `mina` is refused by chain, distinguishably from a
    /// malformed claim.
    #[test]
    fn a_mina_claim_is_refused_by_chain() {
        let json = serde_json::json!({
            "version": "1.0",
            "blockchain": "mina",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
        })
        .to_string();
        assert_eq!(
            parse(json.as_bytes()),
            Err(ClaimDecodeError::UnsupportedChain("mina"))
        );
    }

    #[test]
    fn a_malformed_claim_is_refused_with_the_validators_own_reason() {
        assert!(matches!(
            parse(b"{\"version\":\"2.0\",\"scheme\":\"batch-settlement\"}"),
            Err(ClaimDecodeError::Structural(_))
        ));
        assert_eq!(parse(&[0xff, 0xfe]), Err(ClaimDecodeError::NotUtf8));
    }
}
