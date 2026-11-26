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
//! raises the watermark when it is ahead of this node's, and LOWERS it when
//! a forward that rode a voucher ended in a reject and the receiver holds
//! less than this node signed: a packet never carried is not paid for
//! (ADR 0075, issue #1446; [`crate::OutboundChannels::set_watermark`]).

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_domain::Network;
use connector_settlement::batch::{ChannelPresentation, EvmChannelConfig, Voucher};
use connector_settlement::ChannelId;
use connector_signer::{
    BatchChannelConfig, EvmVoucherScheme, ParsedEvmVoucher, ParsedSolanaVoucher, ParsedXrplVoucher,
    SolanaVoucherScheme, VoucherScheme, XrplVoucherScheme,
};

use crate::batch_channels::hex;
use crate::connector::hex_lower;

/// How long a challenge this node signs is valid for, in seconds. Short on
/// purpose: within it a challenge is a bearer proof for zero-value traffic
/// (ADR 0075, Consequences), and it is signed for the one request that
/// carries it. Well inside the 300 seconds a receiver accepts
/// (`peer-carriage-spec.md` §1.2), which absorbs clock skew between two
/// operators' hosts.
pub const PEER_CHALLENGE_TTL_SECS: u64 = 60;

/// The port's config as EVM's voucher scheme carries it: the same seven
/// fields.
fn scheme_config(config: &EvmChannelConfig) -> BatchChannelConfig {
    BatchChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

fn config_json(config: &EvmChannelConfig) -> serde_json::Value {
    EvmVoucherScheme::spell_config(&scheme_config(config))
}

/// An outbound EVM voucher's own fields, spelled by EVM's voucher scheme
/// (spec #1685): `blockchain`, `channelId`, `maxClaimableAmount`,
/// `signature` and, always, `channelConfig` -- a receiver that restarted,
/// or never saw the channel's first voucher, admits the channel from any of
/// them. The envelope is the caller's.
///
/// `channel` and `voucher` are what this node's EVM payer produced: `0x` and
/// 64 hex, and 65 signature bytes. Anything else is not a voucher the payer
/// signs; it is spelled as it stands, for the receiver to refuse.
#[must_use]
pub fn evm_voucher_fields(
    channel: &ChannelId,
    config: &EvmChannelConfig,
    voucher: &Voucher,
) -> serde_json::Map<String, serde_json::Value> {
    let channel_id = EvmVoucherScheme::decode_channel_id(&channel.0);
    let signature = <[u8; 65]>::try_from(voucher.signature.as_slice()).ok();
    let parsed = ParsedEvmVoucher {
        channel_id: channel_id.unwrap_or_default(),
        max_claimable_amount: voucher.cumulative_amount,
        signature: signature.unwrap_or([0; 65]),
        channel_config: Some(scheme_config(config)),
    };
    let mut fields = EvmVoucherScheme::spell(&parsed);
    if channel_id.is_none() {
        fields.insert("channelId".into(), channel.0.clone().into());
    }
    if signature.is_none() {
        fields.insert("signature".into(), hex(&voucher.signature).into());
    }
    fields
}

/// An outbound Solana voucher's own fields, spelled by Solana's voucher
/// scheme (spec #1685): `blockchain`, `channelId`, `maxClaimableAmount`,
/// `expiresAt` (always zero, and part of the signed bytes) and `signature`.
/// The envelope is the caller's.
///
/// `channel` and `voucher` are what this node's Solana payer produced: a
/// base58 channel account, a `u64` amount and 64 signature bytes. Anything
/// else is not a voucher the payer signs; it is spelled as it stands, for
/// the receiver to refuse.
#[must_use]
pub fn solana_voucher_fields(
    channel: &ChannelId,
    voucher: &Voucher,
) -> serde_json::Map<String, serde_json::Value> {
    let channel_account = SolanaVoucherScheme::decode_channel_account(&channel.0);
    let amount = u64::try_from(voucher.cumulative_amount).ok();
    let signature = <[u8; 64]>::try_from(voucher.signature.as_slice()).ok();
    let parsed = ParsedSolanaVoucher {
        channel_account: channel_account.unwrap_or_default(),
        max_claimable_amount: amount.unwrap_or_default(),
        signature: signature.unwrap_or([0; 64]),
    };
    let mut fields = SolanaVoucherScheme::spell(&parsed);
    if channel_account.is_none() {
        fields.insert("channelId".into(), channel.0.clone().into());
    }
    if amount.is_none() {
        fields.insert(
            "maxClaimableAmount".into(),
            voucher.cumulative_amount.to_string().into(),
        );
    }
    if signature.is_none() {
        fields.insert(
            "signature".into(),
            bs58::encode(&voucher.signature).into_string().into(),
        );
    }
    fields
}

/// An outbound XRPL voucher's own fields, spelled by XRPL's voucher scheme
/// (spec #1685, ADR 0087 decision 2): `blockchain`, `channelId` in bare hex,
/// `maxClaimableAmount` in drops and `signature` in hex (DER for a secp256k1
/// key, raw for an Ed25519 one). The envelope is the caller's.
///
/// `channel` is what this node's XRPL payer named: 64 lowercase hex, which
/// is exactly what the scheme spells. Anything else is spelled as it stands,
/// so this never changes what the payer named, and a channel id that is not
/// one is the receiver's to refuse.
#[must_use]
pub fn xrpl_voucher_fields(
    channel: &ChannelId,
    voucher: &Voucher,
) -> serde_json::Map<String, serde_json::Value> {
    let parsed = ParsedXrplVoucher {
        channel_id: XrplVoucherScheme::decode_channel_id(&channel.0).unwrap_or_default(),
        max_claimable_amount: voucher.cumulative_amount,
        signature: voucher.signature.clone(),
    };
    let mut fields = XrplVoucherScheme::spell(&parsed);
    if fields.get("channelId").and_then(|id| id.as_str()) != Some(channel.0.as_str()) {
        fields.insert("channelId".into(), channel.0.clone().into());
    }
    fields
}

/// A voucher on `presentation`'s channel, on `network` -- the CAIP-2 network
/// the channel is on, which every voucher names (ADR 0086 decision 1) -- as
/// the claim slot carries it.
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
    network: &Network,
    voucher: &Voucher,
    sender_id: &str,
    timestamp: &str,
) -> String {
    let amount = voucher.cumulative_amount.to_string();
    let message_id = format!("{}:{amount}", presentation.channel().0);
    match presentation {
        ChannelPresentation::Evm { channel, config } => {
            let mut fields = evm_voucher_fields(channel, config, voucher);
            fields.insert("version".into(), "1.0".into());
            fields.insert("network".into(), network.to_string().into());
            fields.insert("scheme".into(), "batch-settlement".into());
            fields.insert("messageId".into(), message_id.into());
            fields.insert("timestamp".into(), timestamp.into());
            fields.insert("senderId".into(), sender_id.into());
            serde_json::Value::Object(fields)
        }
        ChannelPresentation::Solana { channel } => {
            let mut fields = solana_voucher_fields(channel, voucher);
            fields.insert("version".into(), "1.0".into());
            fields.insert("network".into(), network.to_string().into());
            fields.insert("scheme".into(), "batch-settlement".into());
            fields.insert("messageId".into(), message_id.into());
            fields.insert("timestamp".into(), timestamp.into());
            fields.insert("senderId".into(), sender_id.into());
            serde_json::Value::Object(fields)
        }
        ChannelPresentation::Xrpl { channel } => {
            let mut fields = xrpl_voucher_fields(channel, voucher);
            fields.insert("version".into(), "1.0".into());
            fields.insert("network".into(), network.to_string().into());
            fields.insert("scheme".into(), "batch-settlement".into());
            fields.insert("messageId".into(), message_id.into());
            fields.insert("timestamp".into(), timestamp.into());
            fields.insert("senderId".into(), sender_id.into());
            serde_json::Value::Object(fields)
        }
    }
    .to_string()
}

/// The voucher claim-state challenge for `presentation`'s channel on
/// `network`, valid until `expires`, as a `POST /ilp/claim-state` entry:
/// what the receiver
/// is asked with, and -- serialized -- what the peer-role challenge slot
/// carries. An EVM entry carries the channel's config, so a receiver with
/// no record of the channel yet can resolve it.
#[must_use]
pub fn challenge_entry(
    presentation: &ChannelPresentation,
    network: &Network,
    expires: u64,
    signature: &[u8],
) -> serde_json::Value {
    match presentation {
        ChannelPresentation::Evm { channel, config } => serde_json::json!({
            "blockchain": "evm",
            "network": network.to_string(),
            "scheme": "batch-settlement",
            "channelId": channel.0,
            "expires": expires,
            "signature": hex(signature),
            "channelConfig": config_json(config),
        }),
        ChannelPresentation::Solana { channel } => serde_json::json!({
            "blockchain": "solana",
            "network": network.to_string(),
            "scheme": "batch-settlement",
            "channelAccount": channel.0,
            "expires": expires,
            "signature": BASE64.encode(signature),
        }),
        ChannelPresentation::Xrpl { channel } => serde_json::json!({
            "blockchain": "xrpl",
            "network": network.to_string(),
            "scheme": "batch-settlement",
            "channelId": channel.0,
            "expires": expires,
            "signature": hex_lower(signature),
        }),
    }
}

/// The receiver of an outbound channel, asked where its watermark stands
/// (ADR 0075 decision 6). A port so a test can stand a real answering node
/// in front of it; [`HttpVoucherState`] is the one every node uses.
#[async_trait]
pub trait VoucherStateSource: Send + Sync {
    /// The highest cumulative amount the receiver has accepted a voucher
    /// for on `presentation`'s channel on `network`, proved by `signature`
    /// over the challenge valid until `expires`.
    async fn watermark(
        &self,
        presentation: &ChannelPresentation,
        network: &Network,
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
        network: &Network,
        expires: u64,
        signature: &[u8],
    ) -> Result<u128, String> {
        let url = format!("{}/claim-state", self.edge_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "channels": [challenge_entry(presentation, network, expires, signature)],
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

/// A receiver this node has no route to ask -- an onion client edge on a
/// node with no usable `socks_proxy` (ADR 0070) -- refused by name without
/// a dial, so an onion name never reaches the local resolver.
pub(crate) struct UnreachableVoucherState(pub(crate) String);

#[async_trait]
impl VoucherStateSource for UnreachableVoucherState {
    async fn watermark(
        &self,
        _presentation: &ChannelPresentation,
        _network: &Network,
        _expires: u64,
        _signature: &[u8],
    ) -> Result<u128, String> {
        Err(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_domain::client_claim::{parse_client_claim, ClientClaim};
    use connector_domain::Network;
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
            &Network::Eip155(84_532),
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
        assert_eq!(voucher.network, Network::Eip155(84_532));
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
            &devnet(),
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
        assert_eq!(voucher.network, devnet());
        assert_eq!(voucher.max_claimable_amount, 7);
    }

    fn devnet() -> Network {
        "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
            .parse()
            .expect("a CAIP-2 network")
    }

    #[test]
    fn a_challenge_entry_names_the_scheme_and_carries_the_config() {
        let entry = challenge_entry(&evm(), &Network::Eip155(84_532), 1_800_000_000, &[0x1c; 65]);
        assert_eq!(entry["scheme"], "batch-settlement");
        assert_eq!(entry["network"], "eip155:84532");
        assert_eq!(entry["expires"], 1_800_000_000u64);
        assert_eq!(entry["channelConfig"]["withdrawDelay"], 86_400);
        assert_eq!(entry["signature"].as_str().expect("hex").len(), 2 + 2 * 65);
    }
}
