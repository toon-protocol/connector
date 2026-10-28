//! `[[pay_channels]]`: the **outbound** half of a config-declared peering
//! (ADR 0042, as ADR 0075 decisions 4, 6 and 9 amend it; issue #1380) -- this
//! node's own x402 channel toward a next hop, which every PREPARE it forwards
//! to that hop is covered by a voucher on.
//!
//! ```toml
//! [[pay_channels]]
//! peer_id          = "store"
//! outbound_channel = "0x…"                     # or a base58 Solana channel account
//! client_edge_url  = "https://store.example/ilp"
//! ```
//!
//! # Where each part of a voucher comes from
//!
//! * the **channel** is this node's own outbound x402 channel toward the hop,
//!   opened with `POST /channels` (or by an earlier `POST /peers`) and so
//!   already in this node's outbound-channel journal, which holds the
//!   `ChannelConfig` (EVM) or the channel account (Solana) a voucher
//!   presents. It is named here because a node may hold several channels
//!   toward one receiver and only the operator can say which one pays this
//!   peering (ADR 0075 decision 4). The binary refuses to boot on a row
//!   naming a channel that journal does not hold;
//! * the **signing key** is the chain's settlement key: every channel this
//!   node opens names it as payer and voucher signer (ADR 0075 decision 3).
//!   There is no second key and none is configured;
//! * the **cumulative amount** is the channel's signed watermark plus what
//!   the forward carries. After a restart, or a journal that lost the latest
//!   vouchers, the **next hop's** `POST /ilp/claim-state` (`scheme:
//!   "batch-settlement"`) is asked where the channel stands, and the answer
//!   sets the watermark, higher or lower (decision 6, issue #1446). That is what
//!   `client_edge_url` is for.
//!
//! The inbound half -- the channel the peer pays this node on -- is
//! `[[peer_channels]]`'s (`crate::peer_channel`).
//!
//! # What a `toon-channel` row wrote, and why each field is refused by name
//!
//! This table used to name one TOON channel "in both roles at once" with the
//! peering's `[[peer_channels]]` row: an EVM `channel_id` with the
//! `chain_id`/`token_network` domain its claims were signed under, or a
//! Solana `channel_account` of TOON's own program. ADR 0075 retires that
//! shape, and each of those fields is still parsed so a file that writes it
//! is refused naming it ([`ConfigError::PayChannelToonFieldRemoved`]).

use std::collections::HashSet;

use serde::Deserialize;
use url::Url;

use crate::error::ConfigError;
use crate::peer::plaintext_permitted;
use crate::settlement::{SettlementChain, SettlementTables};
use crate::x402_row::{parse_x402_channel, RemovedToonFields};

/// One `[[pay_channels]]` entry as written in the config file.
///
/// `deny_unknown_fields` for the reason every money-shaped table here has
/// it. The `toon-channel` fields are `toml::Value` so a value of any type is
/// named as the removed key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPayChannel {
    peer_id: String,
    #[serde(default)]
    outbound_channel: Option<String>,
    client_edge_url: String,
    #[serde(default)]
    channel_id: Option<toml::Value>,
    #[serde(default)]
    channel_account: Option<toml::Value>,
    #[serde(default)]
    chain_id: Option<toml::Value>,
    #[serde(default)]
    token_network: Option<toml::Value>,
    #[serde(default)]
    counterparty_key: Option<toml::Value>,
    #[serde(default)]
    program_id: Option<toml::Value>,
}

impl RawPayChannel {
    fn removed(&self) -> RemovedToonFields {
        RemovedToonFields {
            channel_id: self.channel_id.is_some(),
            channel_account: self.channel_account.is_some(),
            chain_id: self.chain_id.is_some(),
            token_network: self.token_network.is_some(),
            counterparty_key: self.counterparty_key.is_some(),
            program_id: self.program_id.is_some(),
        }
    }
}

/// A fully validated `[[pay_channels]]` entry. Constructed only by
/// [`resolve_pay_channels`] (plus [`Config::load`]'s own cross-table
/// checks), so a value that exists names a configured peering exactly once,
/// an x402 channel on a chain this node pays x402 on, and a
/// `client_edge_url` this node is allowed to dial.
///
/// [`Config::load`]: crate::Config::load
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayChannelConfig {
    peer_id: String,
    chain: SettlementChain,
    outbound_channel: String,
    client_edge_url: Url,
}

impl PayChannelConfig {
    /// The next hop this channel pays -- a `[[peers]]` entry's `id`. A row
    /// naming an id no `[[peers]]` entry configures is
    /// [`ConfigError::PayChannelOrphaned`].
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The chain the channel is on, read off its own spelling.
    pub fn chain(&self) -> SettlementChain {
        self.chain
    }

    /// This node's own outbound x402 channel toward the hop: an EVM
    /// `x402BatchSettlement` channel id as lower-case `0x` hex, or a Solana
    /// `payment-channels` channel account in base58 -- the key this node's
    /// outbound-channel journal holds it under.
    pub fn outbound_channel(&self) -> &str {
        &self.outbound_channel
    }

    /// The next hop's own client edge: its `POST /ilp` endpoint, the URL an
    /// ordinary buyer posts a packet to. `POST /ilp/claim-state` hangs off
    /// it, and that is what this node asks where its vouchers on this
    /// channel stand, on restore and after a voucher the hop refused.
    ///
    /// **Explicit, never derived.** A peering's own `endpoint` is not it: on
    /// a `wss://` peering there is no HTTP URL there at all, and turning one
    /// into the other by swapping scheme and appending a path is exactly the
    /// class of guess ADR 0030 refuses for `btpEndpoint`.
    pub fn client_edge_url(&self) -> &Url {
        &self.client_edge_url
    }
}

/// Parse and check the row's URL.
///
/// `allow_plaintext` is the same top-level `peer_allow_plaintext_endpoints`
/// opt-in `[[peers]]` endpoints take (issue #678, gap 3), and for the same
/// reason: a signed claim-state challenge -- a capability to read a
/// channel's state -- would otherwise travel in the clear. `false` is the
/// default and every production config, and then `https://` is the only
/// scheme this table accepts.
///
/// An **onion** URL is the same exception here that it is there (ADR 0070
/// decision 2): a v3 onion address *is* the ed25519 key the circuit is
/// encrypted and authenticated to, so the challenge does not travel in the
/// clear. Refusing one would leave an onion peering that loads and can
/// never restore its watermark. Asked through
/// [`crate::is_onion_endpoint`] (via [`plaintext_permitted`]) rather than
/// restated, so this and `PeerCarriage::for_endpoint` cannot come to
/// different answers about the same host.
fn resolve_client_edge_url(
    peer_id: &str,
    written: String,
    allow_plaintext: bool,
) -> Result<Url, ConfigError> {
    let url =
        Url::parse(&written).map_err(|source| ConfigError::PayChannelInvalidClientEdgeUrl {
            peer_id: peer_id.to_string(),
            value: written.clone(),
            source,
        })?;
    let scheme_allowed = match url.scheme() {
        "https" => true,
        "http" => plaintext_permitted(&url, allow_plaintext),
        _ => false,
    };
    if !scheme_allowed {
        return Err(ConfigError::PayChannelClientEdgeUrlScheme {
            peer_id: peer_id.to_string(),
            value: written,
            scheme: url.scheme().to_string(),
        });
    }
    Ok(url)
}

fn resolve_pay_channel(
    raw: RawPayChannel,
    tables: SettlementTables,
    allow_plaintext: bool,
) -> Result<PayChannelConfig, ConfigError> {
    // First, for the reason `resolve_peer_channel` gives: a removed key is
    // the clearest thing wrong with a file, and must never fall through to a
    // silent ignore (ADR 0009).
    if let Some(field) = raw.removed().first() {
        return Err(ConfigError::PayChannelToonFieldRemoved {
            peer_id: raw.peer_id,
            field,
        });
    }
    let Some(written) = raw.outbound_channel else {
        return Err(ConfigError::PayChannelOutboundChannelMissing {
            peer_id: raw.peer_id,
        });
    };
    let Some(channel) = parse_x402_channel(&written) else {
        return Err(ConfigError::PayChannelInvalidOutboundChannel {
            peer_id: raw.peer_id,
            value: written,
        });
    };
    if !tables.x402(channel.chain) {
        return Err(ConfigError::PayChannelWithoutX402 {
            peer_id: raw.peer_id,
            chain: channel.chain.name(),
        });
    }
    let client_edge_url =
        resolve_client_edge_url(&raw.peer_id, raw.client_edge_url, allow_plaintext)?;
    Ok(PayChannelConfig {
        peer_id: raw.peer_id,
        chain: channel.chain,
        outbound_channel: channel.value,
        client_edge_url,
    })
}

/// Validate every `[[pay_channels]]` entry.
///
/// Cross-table checks (the peering exists, the channel is not also a
/// `[[peer_channels]]` inbound channel, a route's hop is paid) live in
/// [`Config::load`], which is the only place that has the other tables in
/// scope.
///
/// [`Config::load`]: crate::Config::load
pub(crate) fn resolve_pay_channels(
    raw: Vec<RawPayChannel>,
    allow_plaintext: bool,
    tables: SettlementTables,
) -> Result<Vec<PayChannelConfig>, ConfigError> {
    let mut seen_peers = HashSet::with_capacity(raw.len());
    let mut seen_channels = HashSet::with_capacity(raw.len());
    let mut channels = Vec::with_capacity(raw.len());

    for entry in raw {
        let entry = resolve_pay_channel(entry, tables, allow_plaintext)?;
        // One outbound channel per next hop: the forwarding path signs every
        // voucher to a hop on the one channel registered for it, so a second
        // row would leave which channel paid to file order. True across
        // chains as much as within one.
        if !seen_peers.insert(entry.peer_id.clone()) {
            return Err(ConfigError::PayChannelDuplicatePeer {
                peer_id: entry.peer_id,
            });
        }
        // And the mirror: one channel paying two hops is one receiver paid
        // for two peerings' traffic on one watermark, which neither hop can
        // tell apart. An x402 channel names exactly one receiver.
        if !seen_channels.insert(entry.outbound_channel.clone()) {
            return Err(ConfigError::PayChannelDuplicate {
                value: entry.outbound_channel,
            });
        }
        channels.push(entry);
    }

    Ok(channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNEL: &str = "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111";
    const OTHER_CHANNEL: &str =
        "0x1111222233334444555566667777888811112222333344445555666677778888";
    const NETWORK: &str = "0x3333333333333333333333333333333333333333";
    const ACCOUNT: &str = "G5mXQzfZb4tXWX7cQvXP9ZJnDBcUo6irWTmGGtX3xpzL";
    const PROGRAM: &str = "HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR";

    fn row(peer_id: &str, channel: &str) -> String {
        format!(
            "peer_id = \"{peer_id}\"\noutbound_channel = \"{channel}\"\nclient_edge_url = \
             \"https://relay.example/ilp\""
        )
    }

    fn parse(text: &str) -> RawPayChannel {
        toml::from_str(text).expect("the row parses")
    }

    fn resolve_with(
        rows: &[&str],
        allow_plaintext: bool,
    ) -> Result<Vec<PayChannelConfig>, ConfigError> {
        resolve_pay_channels(
            rows.iter().map(|text| parse(text)).collect(),
            allow_plaintext,
            SettlementTables::for_tests(true, true),
        )
    }

    fn resolve(rows: &[&str]) -> Result<Vec<PayChannelConfig>, ConfigError> {
        resolve_with(rows, false)
    }

    #[test]
    fn an_evm_row_names_an_outbound_x402_channel_canonicalized() {
        let channels =
            resolve(&[&row("relay", &CHANNEL.to_uppercase().replace("0X", "0x"))]).expect("valid");

        assert_eq!(channels[0].peer_id(), "relay");
        assert_eq!(channels[0].chain(), SettlementChain::Evm);
        assert_eq!(channels[0].outbound_channel(), CHANNEL);
        assert_eq!(
            channels[0].client_edge_url().as_str(),
            "https://relay.example/ilp"
        );
    }

    #[test]
    fn a_solana_row_names_a_channel_account() {
        let channels = resolve(&[&row("store", ACCOUNT)]).expect("valid");

        assert_eq!(channels[0].chain(), SettlementChain::Solana);
        assert_eq!(channels[0].outbound_channel(), ACCOUNT);
    }

    /// ADR 0009 and issue #1380's acceptance criterion: every field of the
    /// `toon-channel` `[[pay_channels]]` shapes is refused by name.
    #[test]
    fn every_toon_channel_field_is_refused_by_name() {
        for (field, value) in [
            ("channel_id", format!("\"{CHANNEL}\"")),
            ("channel_account", format!("\"{ACCOUNT}\"")),
            ("chain_id", "8453".to_string()),
            ("token_network", format!("\"{NETWORK}\"")),
            ("counterparty_key", format!("\"{NETWORK}\"")),
            ("program_id", format!("\"{PROGRAM}\"")),
        ] {
            let text = format!("{}\n{field} = {value}", row("relay", CHANNEL));
            let error = resolve(&[&text]).expect_err(field);
            assert!(
                matches!(
                    &error,
                    ConfigError::PayChannelToonFieldRemoved { peer_id, field: named }
                        if peer_id == "relay" && *named == field
                ),
                "{field}: {error:?}"
            );
            assert!(error.to_string().contains("ADR 0075"), "{error}");
        }
    }

    /// The two whole TOON shapes this table used to take, as a file written
    /// before ADR 0075 holds them.
    #[test]
    fn both_whole_toon_channel_shapes_are_refused_naming_their_channel() {
        let evm = format!(
            "peer_id = \"relay\"\nchannel_id = \"{CHANNEL}\"\nchain_id = 8453\ntoken_network = \
             \"{NETWORK}\"\nclient_edge_url = \"https://relay.example/ilp\""
        );
        let solana = format!(
            "peer_id = \"store\"\nchannel_account = \"{ACCOUNT}\"\nclient_edge_url = \
             \"https://relay.example/ilp\""
        );
        assert!(matches!(
            resolve(&[&evm]),
            Err(ConfigError::PayChannelToonFieldRemoved {
                field: "channel_id",
                ..
            })
        ));
        assert!(matches!(
            resolve(&[&solana]),
            Err(ConfigError::PayChannelToonFieldRemoved {
                field: "channel_account",
                ..
            })
        ));
    }

    #[test]
    fn a_row_without_an_outbound_channel_is_refused_by_name() {
        let error =
            resolve(&["peer_id = \"relay\"\nclient_edge_url = \"https://relay.example/ilp\""])
                .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PayChannelOutboundChannelMissing { ref peer_id } if peer_id == "relay"
        ));
    }

    #[test]
    fn a_channel_in_neither_chains_spelling_is_refused() {
        for bad in ["0xnope", "not-an-account", NETWORK] {
            let error = resolve(&[&row("relay", bad)]).unwrap_err();
            assert!(
                matches!(error, ConfigError::PayChannelInvalidOutboundChannel { .. }),
                "{bad}: {error:?}"
            );
        }
    }

    #[test]
    fn a_row_on_a_chain_without_x402_is_refused_per_chain() {
        let error = resolve_pay_channels(
            vec![parse(&row("relay", CHANNEL))],
            false,
            SettlementTables::for_tests(false, true),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PayChannelWithoutX402 { chain: "evm", .. }
        ));
        assert!(resolve_pay_channels(
            vec![parse(&row("store", ACCOUNT))],
            false,
            SettlementTables::for_tests(false, true),
        )
        .is_ok());
    }

    #[test]
    fn two_rows_for_one_peering_are_refused_by_name() {
        let result = resolve(&[&row("relay", CHANNEL), &row("relay", OTHER_CHANNEL)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicatePeer { ref peer_id }) if peer_id == "relay"
        ));
        let result = resolve(&[&row("relay", CHANNEL), &row("relay", ACCOUNT)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicatePeer { .. })
        ));
    }

    #[test]
    fn one_channel_paying_two_peerings_is_refused_by_name() {
        let result = resolve(&[&row("relay", CHANNEL), &row("store", CHANNEL)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicate { ref value }) if value == CHANNEL
        ));
    }

    /// A signed claim-state challenge is a capability to read a channel's
    /// state, so the ask is TLS-only unless the loopback opt-in is set.
    #[test]
    fn a_plaintext_client_edge_url_is_refused_unless_plaintext_is_allowed() {
        let text = row("relay", CHANNEL).replace("https://relay.example", "http://127.0.0.1:3000");
        assert!(matches!(
            resolve(&[&text]),
            Err(ConfigError::PayChannelClientEdgeUrlScheme { ref scheme, .. }) if scheme == "http"
        ));
        let channels = resolve_with(&[&text], true).expect("plaintext is opted into");
        assert_eq!(channels[0].client_edge_url().scheme(), "http");
    }

    /// ADR 0070 decision 2: an onion client edge needs no plaintext opt-in,
    /// in either spelling `anon` has published, and a host that merely
    /// contains the word is an ordinary clearnet host.
    #[test]
    fn an_onion_client_edge_url_needs_no_plaintext_opt_in() {
        for url in [
            "http://vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion/ilp",
            "http://vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.anyone/ilp",
        ] {
            let text = row("relay", CHANNEL).replace("https://relay.example/ilp", url);
            let channels = resolve(&[&text]).expect("an onion client edge loads");
            assert_eq!(channels[0].client_edge_url().as_str(), url);
        }
        for url in ["http://onion.example/ilp", "http://anyone.example/ilp"] {
            let text = row("relay", CHANNEL).replace("https://relay.example/ilp", url);
            assert!(matches!(
                resolve(&[&text]),
                Err(ConfigError::PayChannelClientEdgeUrlScheme { .. })
            ));
        }
    }

    #[test]
    fn a_client_edge_url_that_is_not_http_is_refused_by_name() {
        for written in ["wss://relay.example/btp", "relay.example/ilp"] {
            let text = row("relay", CHANNEL).replace("https://relay.example/ilp", written);
            assert!(
                matches!(
                    resolve_with(&[&text], true),
                    Err(ConfigError::PayChannelClientEdgeUrlScheme { .. })
                        | Err(ConfigError::PayChannelInvalidClientEdgeUrl { .. })
                ),
                "{written} should be refused"
            );
        }
    }

    #[test]
    fn toml_refuses_an_unknown_field() {
        let text = format!("{}\noutbound_chanel = \"{CHANNEL}\"", row("relay", CHANNEL));
        assert!(toml::from_str::<RawPayChannel>(&text).is_err());
    }
}
