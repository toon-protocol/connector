//! **The peer-role challenge on the wire** (ADR 0075 decision 5,
//! `peer-carriage-spec.md` §1.4, issue #1377).
//!
//! Under ADR 0075 an interaction takes the peer role from a voucher on a
//! channel bound to that peer -- or, for a packet that moves no value, from
//! the **voucher claim-state challenge** (#1364) signed by that channel's
//! voucher signer. A zero-value peer packet carries no voucher (ADR 0074
//! decision 3, extended to the peer wire), so this is how it is attributed.
//!
//! # The message is #1364's, unchanged
//!
//! What is signed is exactly what `POST /ilp/claim-state` verifies for a
//! `batch-settlement` channel: on EVM, `ClaimStateChallenge(bytes32
//! channelId,uint256 expires)` under `x402BatchSettlement`'s EIP-712 domain;
//! on Solana, Ed25519 over `"toon-voucher-claim-state-challenge-v1" ‖
//! channelAccount ‖ expires (u64 LE)`. Both say "I control this channel's
//! voucher signer, until `expires`", to the channel's receiver, and neither
//! moves value -- ADR 0075's reason one message may serve both purposes.
//! It stays apart from a voucher: `connector_signer`'s separation tests show
//! neither verifies as the other.
//!
//! # So is the JSON
//!
//! The object is a `POST /ilp/claim-state` entry's (`client-edge-spec.md`
//! §1.10), so a peer that can prove a channel to the claim-state endpoint
//! can prove it here with the same code:
//!
//! ```json
//! {"blockchain": "evm", "scheme": "batch-settlement", "channelId": "0x…",
//!  "expires": 1800000000, "signature": "0x…", "channelConfig": {…}}
//! {"blockchain": "solana", "scheme": "batch-settlement", "channelAccount": "…",
//!  "expires": 1800000000, "signature": "<base64>"}
//! ```
//!
//! One difference, and it is a narrowing: `scheme` is **required** and must
//! be `"batch-settlement"`. A claim-state entry without it asks about a
//! `toon-channel` channel, and a `toon-channel` challenge never proves the
//! peer role -- so it is refused here by name rather than defaulted.
//!
//! # Where it rides
//!
//! In its own slot -- the `peer-role-challenge` protocolData entry on BTP,
//! the `Toon-Peer-Role-Challenge` header (base64 of the JSON) on HTTP --
//! never in the claim's. A challenge is not a claim: it moves nothing and
//! advances no watermark, and a slot that could hold either would make
//! "was this a payment?" a question about the bytes rather than about where
//! they rode.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_btp::{ProtocolData, PEER_CHALLENGE_PROTOCOL};
use connector_domain::client_claim::{
    parse_evm_channel_config, EvmVoucherChannelConfig, SCHEME_BATCH_SETTLEMENT,
};
use serde_json::Value;

use crate::claim_json::AmbiguousClaim;

/// A decoded peer-role challenge: which x402 channel it names, until when,
/// and the signature over #1364's message. Nothing here says whose key
/// signed it -- that is read from the chain, by whoever verifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerRoleChallenge {
    /// An `x402BatchSettlement` channel.
    Evm {
        channel_id: [u8; 32],
        expires: u64,
        /// 65 bytes, `r ‖ s ‖ v`.
        signature: [u8; 65],
        /// The channel's `ChannelConfig`, for a channel the receiver has no
        /// record of yet. Parsed by the voucher's own parser; whoever
        /// verifies re-hashes it to `channel_id` before trusting it.
        channel_config: Option<EvmVoucherChannelConfig>,
    },
    /// A `payment-channels` channel account.
    Solana {
        channel_account: [u8; 32],
        expires: u64,
        signature: [u8; 64],
    },
}

impl PeerRoleChallenge {
    /// The unix second after which this challenge proves nothing.
    #[must_use]
    pub fn expires(&self) -> u64 {
        match self {
            PeerRoleChallenge::Evm { expires, .. } | PeerRoleChallenge::Solana { expires, .. } => {
                *expires
            }
        }
    }
}

/// Why a `peer-role-challenge` entry could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeDecodeError {
    /// Not UTF-8 JSON.
    NotJson,
    /// `scheme` is absent or is not `"batch-settlement"`: a `toon-channel`
    /// challenge proves no peer role (ADR 0075 decision 5).
    NotBatchSettlement,
    /// A field is missing or not its shape.
    Malformed(String),
}

impl std::fmt::Display for ChallengeDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChallengeDecodeError::NotJson => {
                f.write_str("peer-role challenge is not a UTF-8 JSON object")
            }
            ChallengeDecodeError::NotBatchSettlement => f.write_str(
                "a peer-role challenge must carry scheme 'batch-settlement': only an x402 \
                 channel's voucher signer proves the peer role (ADR 0075)",
            ),
            ChallengeDecodeError::Malformed(reason) => write!(f, "peer-role challenge: {reason}"),
        }
    }
}

/// The raw JSON of a frame's `peer-role-challenge` entry, from every such
/// entry on it (`peer-carriage-spec.md` §1.5): zero is `None`, two or more
/// is [`AmbiguousClaim`], counted before anything is parsed -- the same
/// smuggling defence the claim entry has.
pub fn present_from_protocol_data(
    protocol_data: &[ProtocolData],
) -> Result<Option<&[u8]>, AmbiguousClaim> {
    let entries: Vec<&ProtocolData> = protocol_data
        .iter()
        .filter(|pd| pd.name == PEER_CHALLENGE_PROTOCOL)
        .collect();
    if entries.len() > 1 {
        return Err(AmbiguousClaim {
            presented: entries.len(),
        });
    }
    Ok(entries.first().map(|pd| pd.data.as_slice()))
}

/// Parse a peer-role challenge's JSON.
///
/// # Errors
///
/// [`ChallengeDecodeError`], naming what is wrong.
pub fn parse(raw: &[u8]) -> Result<PeerRoleChallenge, ChallengeDecodeError> {
    let json: Value = std::str::from_utf8(raw)
        .ok()
        .and_then(|text| serde_json::from_str(text).ok())
        .ok_or(ChallengeDecodeError::NotJson)?;
    let Value::Object(object) = &json else {
        return Err(ChallengeDecodeError::NotJson);
    };
    if object.get("scheme").and_then(Value::as_str) != Some(SCHEME_BATCH_SETTLEMENT) {
        return Err(ChallengeDecodeError::NotBatchSettlement);
    }
    let text = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| ChallengeDecodeError::Malformed(format!("'{field}' is missing")))
    };
    let expires = object
        .get("expires")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ChallengeDecodeError::Malformed("'expires' must be a unix second".to_string())
        })?;
    match object.get("blockchain").and_then(Value::as_str) {
        Some("evm") => {
            let channel_id = hex_bytes::<32>(text("channelId")?).ok_or_else(|| {
                ChallengeDecodeError::Malformed("'channelId' must be 32 bytes of hex".to_string())
            })?;
            let signature = hex_bytes::<65>(text("signature")?).ok_or_else(|| {
                ChallengeDecodeError::Malformed("'signature' must be 65 bytes of hex".to_string())
            })?;
            let channel_config = match object.get("channelConfig") {
                None | Some(Value::Null) => None,
                Some(config) => Some(
                    parse_evm_channel_config(config)
                        .map_err(|error| ChallengeDecodeError::Malformed(error.to_string()))?,
                ),
            };
            Ok(PeerRoleChallenge::Evm {
                channel_id,
                expires,
                signature,
                channel_config,
            })
        }
        Some("solana") => {
            let channel_account = bs58::decode(text("channelAccount")?)
                .into_vec()
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| {
                    ChallengeDecodeError::Malformed(
                        "'channelAccount' must be base58 of 32 bytes".to_string(),
                    )
                })?;
            let signature = BASE64
                .decode(text("signature")?)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| {
                    ChallengeDecodeError::Malformed(
                        "'signature' must be base64 of 64 bytes".to_string(),
                    )
                })?;
            Ok(PeerRoleChallenge::Solana {
                channel_account,
                expires,
                signature,
            })
        }
        _ => Err(ChallengeDecodeError::Malformed(
            "'blockchain' must be 'evm' or 'solana'".to_string(),
        )),
    }
}

/// Render `challenge` as the JSON [`parse`] reads -- what a dialer puts in
/// the slot, and what a test builds one from.
#[must_use]
pub fn encode(challenge: &PeerRoleChallenge) -> String {
    match challenge {
        PeerRoleChallenge::Evm {
            channel_id,
            expires,
            signature,
            channel_config,
        } => {
            let mut json = serde_json::json!({
                "blockchain": "evm",
                "scheme": SCHEME_BATCH_SETTLEMENT,
                "channelId": format!("0x{}", hex::encode(channel_id)),
                "expires": expires,
                "signature": format!("0x{}", hex::encode(signature)),
            });
            if let Some(config) = channel_config {
                json["channelConfig"] = serde_json::json!({
                    "payer": config.payer,
                    "payerAuthorizer": config.payer_authorizer,
                    "receiver": config.receiver,
                    "receiverAuthorizer": config.receiver_authorizer,
                    "token": config.token,
                    "withdrawDelay": config.withdraw_delay,
                    "salt": config.salt,
                });
            }
            json.to_string()
        }
        PeerRoleChallenge::Solana {
            channel_account,
            expires,
            signature,
        } => serde_json::json!({
            "blockchain": "solana",
            "scheme": SCHEME_BATCH_SETTLEMENT,
            "channelAccount": bs58::encode(channel_account).into_string(),
            "expires": expires,
            "signature": BASE64.encode(signature),
        })
        .to_string(),
    }
}

fn hex_bytes<const N: usize>(text: &str) -> Option<[u8; N]> {
    hex::decode(text.strip_prefix("0x").unwrap_or(text))
        .ok()?
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_btp::CONTENT_TYPE_TEXT;

    fn evm() -> PeerRoleChallenge {
        PeerRoleChallenge::Evm {
            channel_id: [0xab; 32],
            expires: 1_800_000_000,
            signature: [0x11; 65],
            channel_config: Some(EvmVoucherChannelConfig {
                payer: format!("0x{}", "01".repeat(20)),
                payer_authorizer: format!("0x{}", "01".repeat(20)),
                receiver: format!("0x{}", "02".repeat(20)),
                receiver_authorizer: format!("0x{}", "02".repeat(20)),
                token: format!("0x{}", "03".repeat(20)),
                withdraw_delay: 86_400,
                salt: format!("0x{}", "04".repeat(32)),
            }),
        }
    }

    fn solana() -> PeerRoleChallenge {
        PeerRoleChallenge::Solana {
            channel_account: [0xc3; 32],
            expires: 1_800_000_000,
            signature: [0x22; 64],
        }
    }

    #[test]
    fn a_challenge_reads_back_as_itself_on_either_chain() {
        for challenge in [evm(), solana()] {
            assert_eq!(parse(encode(&challenge).as_bytes()), Ok(challenge));
        }
    }

    #[test]
    fn an_evm_challenge_needs_no_channel_config() {
        let PeerRoleChallenge::Evm {
            channel_id,
            expires,
            signature,
            ..
        } = evm()
        else {
            unreachable!()
        };
        let bare = PeerRoleChallenge::Evm {
            channel_id,
            expires,
            signature,
            channel_config: None,
        };
        assert_eq!(parse(encode(&bare).as_bytes()), Ok(bare));
    }

    /// A claim-state entry with no `scheme` asks about a `toon-channel`
    /// channel. On the peer wire that is refused by name, never defaulted.
    #[test]
    fn a_challenge_without_the_batch_settlement_scheme_is_refused_by_name() {
        for scheme in [None, Some("toon-channel")] {
            let mut json: Value = serde_json::from_str(&encode(&solana())).unwrap();
            match scheme {
                None => {
                    json.as_object_mut().unwrap().remove("scheme");
                }
                Some(scheme) => json["scheme"] = Value::from(scheme),
            }
            assert_eq!(
                parse(json.to_string().as_bytes()),
                Err(ChallengeDecodeError::NotBatchSettlement)
            );
        }
    }

    #[test]
    fn a_malformed_challenge_is_refused_not_guessed() {
        for (field, value) in [
            ("expires", Value::from("soon")),
            ("signature", Value::from("0x1234")),
            ("channelId", Value::from("0xnothex")),
            ("blockchain", Value::from("mina")),
        ] {
            let mut json: Value = serde_json::from_str(&encode(&evm())).unwrap();
            json[field] = value;
            assert!(
                matches!(
                    parse(json.to_string().as_bytes()),
                    Err(ChallengeDecodeError::Malformed(_))
                ),
                "{field}"
            );
        }
        assert_eq!(parse(b"not json"), Err(ChallengeDecodeError::NotJson));
    }

    #[test]
    fn two_challenge_entries_on_one_frame_are_ambiguous() {
        let entry = ProtocolData {
            name: PEER_CHALLENGE_PROTOCOL.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: encode(&solana()).into_bytes(),
        };
        assert_eq!(present_from_protocol_data(&[]), Ok(None));
        assert!(present_from_protocol_data(std::slice::from_ref(&entry))
            .unwrap()
            .is_some());
        assert!(present_from_protocol_data(&[entry.clone(), entry]).is_err());
    }
}
