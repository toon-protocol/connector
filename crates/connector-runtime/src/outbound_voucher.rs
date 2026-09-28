//! What this node puts on a peer carriage when it pays a next hop over one of
//! its own outbound x402 channels (ADR 0075 decisions 5 and 6): a voucher,
//! or -- for a packet that moves no value -- the voucher claim-state
//! challenge, and the receiver's `POST /ilp/claim-state` asked where the
//! channel's watermark stands.
//!
//! # One wire shape each, and it is the receiving half's
//!
//! A voucher rides a peer carriage as the client edge's own voucher JSON
//! (`client-edge-spec.md` §1.3, `peer-carriage-spec.md` §4): the same object
//! `connector_domain::client_claim::parse_client_claim` reads, so the peer
//! wire gains no second voucher codec. The challenge is a `POST
//! /ilp/claim-state` entry's JSON (`client-edge-spec.md` §1.10), which the
//! peer-role challenge slot carries verbatim (`peer-carriage-spec.md` §1.4)
//! -- so asking the receiver for a watermark and proving the peer role are
//! one message, signed once per use.
//!
//! # The receiver is the watermark authority on restore (decision 6)
//!
//! [`crate::OutboundChannels`] journals every voucher before it is handed
//! out, so its signed watermark is never behind what this node signed. The
//! receiver is asked anyway, once per process per channel and again after
//! a voucher it did not accept: a node restored from an older journal
//! would otherwise sign a voucher that fails to advance. What it answers
//! only ever raises the watermark ([`crate::OutboundChannels::raise_watermark`]).

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_settlement::batch::{ChannelPresentation, EvmChannelConfig, Voucher};

use crate::batch_channels::hex;

/// How long a challenge this node signs is valid for, in seconds. Short on
/// purpose: within it a challenge is a bearer proof for zero-value traffic
/// (ADR 0075, Consequences), and it is signed for the one request that
/// carries it. Well inside the 300 seconds a receiver accepts
/// (`peer-carriage-spec.md` §1.2), which absorbs clock skew between two
/// operators' hosts.
pub const PEER_CHALLENGE_TTL_SECS: u64 = 60;

fn config_json(config: &EvmChannelConfig) -> serde_json::Value {
    serde_json::json!({
        "payer": hex(&config.payer),
        "payerAuthorizer": hex(&config.payer_authorizer),
        "receiver": hex(&config.receiver),
        "receiverAuthorizer": hex(&config.receiver_authorizer),
        "token": hex(&config.token),
        "withdrawDelay": config.withdraw_delay,
        "salt": hex(&config.salt),
    })
}

/// A voucher on `presentation`'s channel, as the claim slot carries it.
///
/// `sender_id` is this node's voucher signer in its chain's spelling -- a
/// label, as a voucher's `senderId` always is: the receiver reads the
/// signer from the chain, never from here. An EVM voucher always carries
/// the channel's `channelConfig`, not only on the channel's first voucher:
/// a receiver that restarted, or that never saw the first, admits the
/// channel from any of them, and the config is re-hashed to the id before
/// anything is read from the chain.
#[must_use]
pub fn voucher_json(
    presentation: &ChannelPresentation,
    voucher: &Voucher,
    sender_id: &str,
    timestamp: &str,
) -> String {
    let amount = voucher.cumulative_amount.to_string();
    let message_id = format!("{}:{amount}", presentation.channel().0);
    match presentation {
        ChannelPresentation::Evm { channel, config } => serde_json::json!({
            "version": "1.0",
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "messageId": message_id,
            "timestamp": timestamp,
            "senderId": sender_id,
            "channelId": channel.0,
            "maxClaimableAmount": amount,
            "signature": hex(&voucher.signature),
            "channelConfig": config_json(config),
        }),
        ChannelPresentation::Solana { channel } => serde_json::json!({
            "version": "1.0",
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "messageId": message_id,
            "timestamp": timestamp,
            "senderId": sender_id,
            "channelId": channel.0,
            "maxClaimableAmount": amount,
            "expiresAt": 0,
            "signature": bs58::encode(&voucher.signature).into_string(),
        }),
    }
    .to_string()
}

/// The voucher claim-state challenge for `presentation`'s channel, valid
/// until `expires`, as a `POST /ilp/claim-state` entry: what the receiver
/// is asked with, and -- serialized -- what the peer-role challenge slot
/// carries. An EVM entry carries the channel's config, so a receiver with
/// no record of the channel yet can resolve it.
#[must_use]
pub fn challenge_entry(
    presentation: &ChannelPresentation,
    expires: u64,
    signature: &[u8],
) -> serde_json::Value {
    match presentation {
        ChannelPresentation::Evm { channel, config } => serde_json::json!({
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "channelId": channel.0,
            "expires": expires,
            "signature": hex(signature),
            "channelConfig": config_json(config),
        }),
        ChannelPresentation::Solana { channel } => serde_json::json!({
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "channelAccount": channel.0,
            "expires": expires,
            "signature": BASE64.encode(signature),
        }),
    }
}

/// The receiver of an outbound channel, asked where its watermark stands
/// (ADR 0075 decision 6). A port so a test can stand a real answering node
/// in front of it; [`HttpVoucherState`] is the one every node uses.
#[async_trait]
pub trait VoucherStateSource: Send + Sync {
    /// The highest cumulative amount the receiver has accepted a voucher
    /// for on `presentation`'s channel, proved by `signature` over the
    /// challenge valid until `expires`.
    async fn watermark(
        &self,
        presentation: &ChannelPresentation,
        expires: u64,
        signature: &[u8],
    ) -> Result<u128, String>;
}

/// `POST <edge>/claim-state` with one `batch-settlement` entry.
pub struct HttpVoucherState {
    client: reqwest::Client,
    /// The receiver's `POST /ilp` endpoint; `claim-state` hangs off it.
    edge_url: String,
}

impl HttpVoucherState {
    #[must_use]
    pub fn new(client: reqwest::Client, edge_url: impl Into<String>) -> HttpVoucherState {
        HttpVoucherState {
            client,
            edge_url: edge_url.into(),
        }
    }
}

#[async_trait]
impl VoucherStateSource for HttpVoucherState {
    async fn watermark(
        &self,
        presentation: &ChannelPresentation,
        expires: u64,
        signature: &[u8],
    ) -> Result<u128, String> {
        let url = format!("{}/claim-state", self.edge_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "channels": [challenge_entry(presentation, expires, signature)],
        });
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("{url}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("{url} answered {}", response.status()));
        }
        let answer: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("{url} answered no JSON: {error}"))?;
        let entry = answer["channels"]
            .get(0)
            .ok_or_else(|| format!("{url} answered no channel entry"))?;
        if entry["ok"] != serde_json::Value::Bool(true) {
            return Err(format!(
                "{url} would not report channel {}: {}",
                presentation.channel().0,
                entry["error"].as_str().unwrap_or("unverified")
            ));
        }
        entry["cumulativeClaimed"]
            .as_str()
            .and_then(|amount| amount.parse().ok())
            .ok_or_else(|| format!("{url} answered no readable cumulativeClaimed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_domain::client_claim::{parse_client_claim, ClientClaim};
    use connector_settlement::ChannelId;

    fn evm() -> ChannelPresentation {
        ChannelPresentation::Evm {
            channel: ChannelId(format!("0x{}", "ab".repeat(32))),
            config: EvmChannelConfig {
                payer: [0x11; 20],
                payer_authorizer: [0x11; 20],
                receiver: [0x22; 20],
                receiver_authorizer: [0x22; 20],
                token: [0x33; 20],
                withdraw_delay: 86_400,
                salt: [0x44; 32],
            },
        }
    }

    /// The voucher this node emits is the one the receiving half parses:
    /// one codec, both directions (`peer-carriage-spec.md` §4).
    #[test]
    fn an_evm_voucher_is_the_client_edges_own_voucher_json() {
        let json = voucher_json(
            &evm(),
            &Voucher {
                cumulative_amount: 1_234,
                signature: vec![0x1b; 65],
            },
            "0x1111111111111111111111111111111111111111",
            "2026-09-27T00:00:00.000Z",
        );
        let ClientClaim::EvmVoucher(voucher) = parse_client_claim(&json).expect("parses") else {
            panic!("an EVM voucher");
        };
        assert_eq!(voucher.max_claimable_amount, 1_234);
        assert_eq!(voucher.channel_id, format!("0x{}", "ab".repeat(32)));
        let config = voucher
            .channel_config
            .expect("the config rides every voucher");
        assert_eq!(config.payer, config.payer_authorizer);
        assert_eq!(config.salt, format!("0x{}", "44".repeat(32)));
    }

    #[test]
    fn a_solana_voucher_is_the_client_edges_own_voucher_json() {
        let channel = bs58::encode([0x5a; 32]).into_string();
        let json = voucher_json(
            &ChannelPresentation::Solana {
                channel: ChannelId(channel.clone()),
            },
            &Voucher {
                cumulative_amount: 7,
                signature: vec![0x01; 64],
            },
            "sender",
            "2026-09-27T00:00:00Z",
        );
        let ClientClaim::SolanaVoucher(voucher) = parse_client_claim(&json).expect("parses") else {
            panic!("a Solana voucher");
        };
        assert_eq!(voucher.channel_id, channel);
        assert_eq!(voucher.max_claimable_amount, 7);
    }

    #[test]
    fn a_challenge_entry_names_the_scheme_and_carries_the_config() {
        let entry = challenge_entry(&evm(), 1_800_000_000, &[0x1c; 65]);
        assert_eq!(entry["scheme"], "batch-settlement");
        assert_eq!(entry["expires"], 1_800_000_000u64);
        assert_eq!(entry["channelConfig"]["withdrawDelay"], 86_400);
        assert_eq!(entry["signature"].as_str().expect("hex").len(), 2 + 2 * 65);
    }
}
