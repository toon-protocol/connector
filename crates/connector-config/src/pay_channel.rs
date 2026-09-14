//! `[[pay_channels]]` (ADR 0042, item 2; issue #881): the channel this node
//! **pays a next hop from, as an ordinary client of that hop**.
//!
//! # Why this is a third table and not a row in either of the other two
//!
//! A node already names channels in two places, and this is neither of
//! them:
//!
//! | table | direction | who is the authority on the watermark |
//! | --- | --- | --- |
//! | `[[client_channels]]` | claims this node **receives** at its client edge | this node |
//! | `[[peer_channels]]` | a peering's claims, both directions, judged against `ClaimBook` | this node |
//! | **`[[pay_channels]]`** | claims this node **signs and hands to a next hop** | **the next hop** |
//!
//! [ADR 0030](../../../docs/adr/0030-an-operator-announces-a-node-the-node-still-does-not.md)
//! already made this exact distinction for `[announce] pay_channel`, which
//! is the one-shot form of the same thing: *"that table is channels this
//! node receives on, and this is one it pays from. One channel in two roles
//! is the same collision `Config::load` already refuses between the peer and
//! client books."* [`ConfigError::PayChannelIsAlsoAClientChannel`] refuses
//! it here, by name, for the same reason.
//!
//! It is deliberately **not** refused against `[[peer_channels]]`. Holding
//! both roles on one channel with one hop is the deployed shape -- the peer
//! role for what arrives, the client role for what this node sends -- and
//! `connector_runtime`'s own `forward_via_peer_route` is built for it: a
//! covered packet is not owed a second time on the peer ledger, so exactly
//! one book signs per packet.
//!
//! A **Solana** row goes one step further and *requires* the peer-channel
//! row ([`ConfigError::PayChannelSolanaWithoutPeerChannel`], issue #1146).
//! `programId` is a required field of the Solana claim wire, where an EVM
//! claim's EIP-712 domain fields are optional and simply ride absent, and
//! both peer carriages render it from that peering's Solana
//! `[[peer_channels]]` row
//! (`connector_peer_http::dial::PeerRelation::solana_program_ids`). Without
//! one, a covering claim minted here would reach `claim_json::encode` with
//! nothing to write there -- a caller bug it panics on, on the packet path,
//! with the money already committed. Since the deployed shape already holds
//! both roles on one channel, this costs a real config nothing.
//!
//! # Where each part of the claim comes from
//!
//! ADR 0030's table is normative and this row is written to it. Only the
//! facts nothing can derive are configured:
//!
//! * the **signing key** is `[settlement.evm]`'s -- the channel's on-chain
//!   participant *is* this node's settlement address, and no second key is
//!   introduced. A row with no `[settlement.evm]` table to sign under is
//!   [`ConfigError::PayChannelWithoutEvmSettlement`] at load (a Solana row's
//!   key is `[settlement.solana]`'s, on the other curve and under the same
//!   rule: [`ConfigError::PayChannelWithoutSolanaSettlement`]);
//! * the **nonce and cumulative amount** come from the receiver, asked over
//!   `POST /ilp/claim-state` (issue #693) on every packet -- never
//!   remembered, never guessed. That is what `client_edge_url` is for;
//! * the **channel id** is configured, because neither side can derive it;
//! * the **EIP-712 domain** (`chain_id`/`token_network`) is configured too,
//!   and this is the one place this table departs from the announce path.
//!   An announce reads the domain off the target's own greeting because it
//!   has one in hand; the forwarding path covers a packet *before* any
//!   greeting exists to read (that is the whole of issue #881). The domain
//!   is not a second source of truth: it is the same chain id and
//!   `TokenNetwork` the channel's peer-role domain carries, since both roles
//!   sign against the very same on-chain channel.
//!
//! A Solana row's counterpart of that last line is not written at all: the
//! binding ADR 0053 signs into a Solana claim is the **settlement program
//! id**, and since issue #1128 that is read from `[settlement.solana]` and
//! from nowhere else. See [`RawSolanaPayChannel`].

use std::collections::HashSet;

use serde::Deserialize;
use url::Url;

use crate::client_channel::{is_base58_32_bytes, parse_evm_address, parse_hex_bytes, to_hex};
use crate::error::ConfigError;
use crate::peer::plaintext_permitted;
use crate::settlement::{SettlementChain, SettlementTables};

/// One `[[pay_channels]]` entry as written in the config file, in either
/// chain shape this connector accepts (issue #1146): EVM
/// ([`RawEvmPayChannel`]) or Solana ([`RawSolanaPayChannel`]).
/// `#[serde(untagged)]` picks whichever shape matches, exactly as
/// [`crate::peer_channel::RawPeerChannel`] and
/// [`crate::client_channel::RawClientChannel`] already do -- the EVM shape
/// requires `channel_id`/`chain_id`/`token_network` and forbids
/// `channel_account`, the Solana shape the reverse, and each variant is
/// `deny_unknown_fields` so the two can never blend.
///
/// **This table had no Solana twin until issue #1146, and the omission was
/// deliberate.** The reason it gave was that "an outbound client claim is
/// an EIP-712 balance proof and `connector_runtime`'s outbound client
/// ledger signs nothing else, so a Solana pay-from channel has nothing to
/// wire". That was true and is no longer: the outbound client ledger signs
/// an ed25519 balance proof too, over
/// `connector_signer::solana_balance_proof_message`, and asks a Solana next
/// hop for its watermark over the same `POST /ilp/claim-state` the EVM leg
/// uses. What the omission cost in the meantime is what ADR 0042 exists to
/// retire: a Solana peering could only ever be paid **postpay**, since
/// `cover_forward` had no arm to mint under.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawPayChannel {
    Evm(RawEvmPayChannel),
    Solana(RawSolanaPayChannel),
}

/// `[[pay_channels]]`'s original (and, before issue #1146, only) shape: an
/// EVM channel id and the EIP-712 domain the covering claim on it is signed
/// under (ADR 0024).
///
/// `deny_unknown_fields` for the reason every money-shaped table here has
/// it: a dropped `token_network` would be a claim signed under a domain
/// nobody wrote, which recovers to a different address and is refused at
/// the far gate with the packet already paid for.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvmPayChannel {
    peer_id: String,
    channel_id: String,
    chain_id: u64,
    token_network: String,
    client_edge_url: String,
}

/// `[[pay_channels]]`'s Solana shape (issue #1146): the deployed
/// `payment-channel` program's channel PDA (`channel_account`, not an
/// EVM-style `channel_id`) this node signs covering claims against, and the
/// next hop's client edge to ask for their watermark.
///
/// No `chain_id` or `token_network`, for the reason
/// [`crate::peer_channel::RawSolanaPeerChannel`] already documents: Solana
/// has neither a numeric chain id nor a per-token verifying contract for a
/// row to name.
///
/// **`program_id` is not one of this row's facts.** It is read from
/// `[settlement.solana]` -- the one program this node can redeem a claim
/// under, and since ADR 0053 part of what every claim on the channel signs
/// -- exactly as `[[peer_channels]]` has read it since #1128 and
/// `[[client_channels]]` since #1082. The field is accepted here only so
/// that a config which writes it is refused **by name**
/// ([`ConfigError::PayChannelProgramIdNotDeclared`]) rather than
/// disappearing into `#[serde(untagged)]`'s "matched no variant", which is
/// the least legible failure this file can produce and the exact reason
/// every other Solana row spells the same key out. `toml::Value` rather
/// than `String` so `program_id = 5` is named too.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSolanaPayChannel {
    peer_id: String,
    channel_account: String,
    client_edge_url: String,
    #[serde(default)]
    program_id: Option<toml::Value>,
}

/// A fully validated `[[pay_channels]]` EVM entry. Constructed only by
/// [`resolve_pay_channels`] (plus [`Config::load`]'s own cross-table
/// checks), so a value that exists names a configured peering exactly once,
/// carries a well-formed on-chain channel id and `TokenNetwork` address, and
/// a `client_edge_url` this node is allowed to dial.
///
/// [`Config::load`]: crate::Config::load
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmPayChannelConfig {
    peer_id: String,
    channel_id: String,
    chain_id: u64,
    token_network: [u8; 20],
    client_edge_url: Url,
}

impl EvmPayChannelConfig {
    /// The next hop this channel pays -- a `[[peers]]` entry's `id`. A row
    /// naming an id no `[[peers]]` entry configures is
    /// [`ConfigError::PayChannelOrphaned`].
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The channel this node's settlement address holds with that hop,
    /// canonicalized to lowercase `0x`-prefixed hex however the operator
    /// wrote it -- the value the covering claim names the channel by.
    ///
    /// It may not also appear in `[[client_channels]]`
    /// ([`ConfigError::PayChannelIsAlsoAClientChannel`]): that table is
    /// channels this node *receives* on, and this is one it *pays* from.
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    /// The chain the channel is deployed on: half of the EIP-712 domain the
    /// covering claim is signed under.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// The `TokenNetwork` that verifies this channel's claims on
    /// redemption -- the EIP-712 `verifyingContract`, and the other half of
    /// the domain.
    pub fn token_network(&self) -> [u8; 20] {
        self.token_network
    }

    /// The next hop's own client edge -- see
    /// [`PayChannelConfig::client_edge_url`].
    pub fn client_edge_url(&self) -> &Url {
        &self.client_edge_url
    }
}

/// A fully validated `[[pay_channels]]` Solana entry (issue #1146).
/// Constructed only by [`resolve_pay_channels`] -- `channel_account` has
/// already been checked to be a base58-encoded 32-byte value, and
/// `program_id` copied in from `[settlement.solana]`.
///
/// `program_id` is a field of this value but **not** of the config row it
/// came from, exactly as on [`crate::peer_channel::SolanaPeerChannelConfig`]
/// since issue #1128: there is one program this node can redeem a claim
/// under, ADR 0053 signs it into every claim on the channel, so a row that
/// declared a second one could buy carriage with claims this node could
/// never cash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaPayChannelConfig {
    peer_id: String,
    channel_account: String,
    program_id: String,
    client_edge_url: Url,
}

impl SolanaPayChannelConfig {
    /// The next hop this channel pays -- a `[[peers]]` entry's `id`.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The channel's on-chain PDA, base58-encoded -- the value the covering
    /// claim names its `channelAccount` by, and the account
    /// `connector_signer::solana_balance_proof_message` signs over.
    ///
    /// It may not also appear in `[[client_channels]]`
    /// ([`ConfigError::PayChannelIsAlsoAClientChannel`]), the Solana
    /// counterpart of [`EvmPayChannelConfig::channel_id`]'s own rule.
    pub fn channel_account(&self) -> &str {
        &self.channel_account
    }

    /// The base58 program id of the deployed `payment-channel` program this
    /// channel is settled under -- **always `[settlement.solana]
    /// program_id`**, never a fact of the row (issue #1128's rule, applied
    /// to this table from the day it gained a Solana shape). ADR 0053 binds
    /// it into the signed message, so a covering claim minted here is valid
    /// only under the program this node would itself redeem through.
    pub fn program_id(&self) -> &str {
        &self.program_id
    }

    /// The next hop's own client edge -- see
    /// [`PayChannelConfig::client_edge_url`].
    pub fn client_edge_url(&self) -> &Url {
        &self.client_edge_url
    }
}

/// One `[[pay_channels]]` entry, typed by chain (issue #1146). An enum
/// rather than one struct with optional fields for the same reason
/// [`crate::PeerChannelConfig`] and [`crate::ClientChannelConfig`] are: an
/// EVM `channelId` and a Solana `channelAccount` name genuinely different
/// kinds of on-chain identifier, and a shared shape would either force one
/// to fake fields it does not have or erase which chain a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayChannelConfig {
    Evm(EvmPayChannelConfig),
    Solana(SolanaPayChannelConfig),
}

impl PayChannelConfig {
    /// The next hop this channel pays, whichever chain it is on.
    pub fn peer_id(&self) -> &str {
        match self {
            PayChannelConfig::Evm(evm) => evm.peer_id(),
            PayChannelConfig::Solana(solana) => solana.peer_id(),
        }
    }

    /// The chain this channel lives on, and therefore which arm of
    /// `connector_runtime`'s outbound client ledger signs its claims.
    pub fn chain(&self) -> SettlementChain {
        match self {
            PayChannelConfig::Evm(_) => SettlementChain::Evm,
            PayChannelConfig::Solana(_) => SettlementChain::Solana,
        }
    }

    /// How a claim on this row names its channel on the wire -- an EVM
    /// `channelId` or a Solana `channelAccount`. The one spelling every
    /// cross-table check compares by, and the one a covering claim carries.
    pub fn channel(&self) -> &str {
        match self {
            PayChannelConfig::Evm(evm) => evm.channel_id(),
            PayChannelConfig::Solana(solana) => solana.channel_account(),
        }
    }

    /// The next hop's own client edge: its `POST /ilp` endpoint, the URL an
    /// ordinary buyer posts a packet to. `POST /ilp/claim-state` hangs off
    /// it, and that is what this node asks -- on every covered packet -- for
    /// where its claims on this channel stand.
    ///
    /// **Explicit, never derived.** A peering's own `endpoint` is not it: on
    /// a `wss://` peering there is no HTTP URL there at all, and turning one
    /// into the other by swapping scheme and appending a path is exactly the
    /// class of guess ADR 0030 refuses for `btpEndpoint` -- right on this
    /// fleet, wrong for anyone whose deployment does not mirror it.
    pub fn client_edge_url(&self) -> &Url {
        match self {
            PayChannelConfig::Evm(evm) => evm.client_edge_url(),
            PayChannelConfig::Solana(solana) => solana.client_edge_url(),
        }
    }
}

/// Parse and check the one URL both shapes carry.
///
/// `allow_plaintext` is the same top-level `peer_allow_plaintext_endpoints`
/// opt-in `[[peers]]` endpoints take (issue #678, gap 3), and for the same
/// reason: a signed claim-state challenge -- a capability to read a
/// channel's state -- would otherwise travel in the clear. `false` is the
/// default and every production config, and then `https://` is the only
/// scheme this table accepts.
///
/// An **onion** URL is the same exception here that it is there (ADR 0070
/// decision 2), and it has to be, twice over. The reason transfers exactly:
/// a v3 onion address *is* the ed25519 key the circuit is encrypted and
/// authenticated to, so the challenge does not travel in the clear and
/// ADR 0004's requirement is satisfied by a different mechanism rather than
/// waived. And the alternative is worse than an inconsistency -- a peering
/// this node forwards to must carry a `[[pay_channels]]` row
/// (`PayChannelUnbound`), and that row's `client_edge_url` is the *payee's*
/// client edge, so on an onion-only payee it can only be an onion URL.
/// Refusing it here would leave an onion peering that loads and can never
/// cover a forward: every packet refused for want of a covering claim,
/// before the carriage ADR 0070 built was ever asked to carry one.
///
/// Asked through [`is_onion_endpoint`] rather than restated, so this and
/// `PeerCarriage::for_endpoint` cannot come to different answers about the
/// same host.
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

/// `tables.evm()` is whether this node declares `[settlement.evm]`. It is
/// required here for the reason ADR 0030's table gives: the covering claim
/// is signed by the channel's on-chain participant, and that address is
/// `[settlement.evm.key]`'s. A row with no table to sign under would reach
/// the packet path and fail every forward it was configured for.
fn resolve_evm_pay_channel(
    raw: RawEvmPayChannel,
    tables: SettlementTables<'_>,
    allow_plaintext: bool,
) -> Result<EvmPayChannelConfig, ConfigError> {
    if !tables.evm() {
        return Err(ConfigError::PayChannelWithoutEvmSettlement {
            peer_id: raw.peer_id,
        });
    }
    let channel_id =
        parse_hex_bytes::<32>(&raw.channel_id).ok_or_else(|| ConfigError::PayChannelInvalidId {
            peer_id: raw.peer_id.clone(),
            value: raw.channel_id.clone(),
        })?;
    let token_network = parse_evm_address(&raw.token_network).ok_or_else(|| {
        ConfigError::PayChannelInvalidAddress {
            peer_id: raw.peer_id.clone(),
            field: "token_network",
            value: raw.token_network.clone(),
        }
    })?;
    let client_edge_url =
        resolve_client_edge_url(&raw.peer_id, raw.client_edge_url, allow_plaintext)?;
    Ok(EvmPayChannelConfig {
        peer_id: raw.peer_id,
        channel_id: to_hex(&channel_id),
        chain_id: raw.chain_id,
        token_network,
        client_edge_url,
    })
}

/// `tables.solana_program_id()` is `[settlement.solana] program_id`, or
/// `None` for a node with no `[settlement.solana]` table at all. It is the
/// only source of a Solana pay channel's program id; the row is refused
/// outright if it tries to name a second one, and refused again if there is
/// no table to read the first from -- which is also the table holding the
/// ed25519 key that signs the covering claim.
fn resolve_solana_pay_channel(
    raw: RawSolanaPayChannel,
    tables: SettlementTables<'_>,
    allow_plaintext: bool,
) -> Result<SolanaPayChannelConfig, ConfigError> {
    // Before the shape checks, because "you wrote a key this table does not
    // have" explains the file better than "one of your other values is
    // malformed" when both are true -- the same ordering
    // `resolve_solana_peer_channel` takes, for the same ADR 0009 reason.
    if raw.program_id.is_some() {
        return Err(ConfigError::PayChannelProgramIdNotDeclared {
            peer_id: raw.peer_id,
        });
    }
    if !is_base58_32_bytes(&raw.channel_account) {
        return Err(ConfigError::PayChannelInvalidSolanaAccount {
            peer_id: raw.peer_id,
            field: "channel_account",
            value: raw.channel_account,
        });
    }
    let Some(program_id) = tables.solana_program_id() else {
        return Err(ConfigError::PayChannelWithoutSolanaSettlement {
            peer_id: raw.peer_id,
        });
    };
    // `[settlement.solana]`'s own resolver checks this value for
    // non-emptiness only, and the settlement backend does not parse it
    // until it dials a chain. A pay channel needs it to be a real 32-byte
    // address before that, because ADR 0053 puts it inside every claim this
    // row signs -- a claim signed over a program id nobody can decode is a
    // claim the far gate recovers a different signer for.
    if !is_base58_32_bytes(program_id) {
        return Err(ConfigError::PayChannelSolanaSettlementProgramIdInvalid {
            peer_id: raw.peer_id,
            value: program_id.to_string(),
        });
    }
    let client_edge_url =
        resolve_client_edge_url(&raw.peer_id, raw.client_edge_url, allow_plaintext)?;
    Ok(SolanaPayChannelConfig {
        peer_id: raw.peer_id,
        channel_account: raw.channel_account,
        program_id: program_id.to_string(),
        client_edge_url,
    })
}

/// Validate every `[[pay_channels]]` entry.
///
/// Cross-table checks (the peering exists, the channel is not also a
/// `[[client_channels]]` row, a Solana row's channel is bound as a peer
/// channel too) live in [`Config::load`], which is the only place that has
/// the other tables in scope.
///
/// [`Config::load`]: crate::Config::load
pub(crate) fn resolve_pay_channels(
    raw: Vec<RawPayChannel>,
    allow_plaintext: bool,
    tables: SettlementTables<'_>,
) -> Result<Vec<PayChannelConfig>, ConfigError> {
    let mut seen_peers = HashSet::with_capacity(raw.len());
    let mut seen_channels = HashSet::with_capacity(raw.len());
    let mut channels = Vec::with_capacity(raw.len());

    for entry in raw {
        let entry =
            match entry {
                RawPayChannel::Evm(evm) => {
                    PayChannelConfig::Evm(resolve_evm_pay_channel(evm, tables, allow_plaintext)?)
                }
                RawPayChannel::Solana(solana) => PayChannelConfig::Solana(
                    resolve_solana_pay_channel(solana, tables, allow_plaintext)?,
                ),
            };
        // One nonce line per next hop (see `connector_runtime`'s
        // `outbound_client` header: the ledger is keyed by next-hop peer
        // id, precisely so one hop reached over several routes stays one
        // line). Two rows for one hop would be two channels for one line,
        // and which one signed would depend on file order -- and that is
        // true across chains as much as within one, which is why this is
        // checked on the peer id alone rather than per chain.
        if !seen_peers.insert(entry.peer_id().to_string()) {
            return Err(ConfigError::PayChannelDuplicatePeer {
                peer_id: entry.peer_id().to_string(),
            });
        }
        // The mirror image of the rule above: one channel paid from by two
        // hops is one channel carrying two nonce lines, which forks it at
        // the far gate exactly as a second process would. An EVM
        // `0x`-prefixed hex id and a base58 Solana account are drawn from
        // disjoint spellings, so one set holds both without either chain
        // being able to collide with the other.
        if !seen_channels.insert(entry.channel().to_string()) {
            return Err(ConfigError::PayChannelDuplicate {
                value: entry.channel().to_string(),
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
    /// A real 32-byte base58 account, so nothing here reads as a
    /// placeholder: `local/mixed-chain`'s own b-c channel PDA.
    const ACCOUNT: &str = "G5mXQzfZb4tXWX7cQvXP9ZJnDBcUo6irWTmGGtX3xpzL";
    const OTHER_ACCOUNT: &str = "93mxPHokL6EhVxzVicSyouEhvTVGUcKXKtHH1uTmw2Aw";
    /// The program id `local/mixed-chain`'s `[settlement.solana]` names.
    const PROGRAM: &str = "HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR";

    /// A node with both settlement tables, which is what
    /// `local/mixed-chain`'s middle node is -- so neither chain's row is
    /// refused for want of a table unless a test asks for that.
    fn both_chains() -> SettlementTables<'static> {
        SettlementTables::for_tests(true, Some(PROGRAM))
    }

    fn evm(peer_id: &str, channel_id: &str) -> RawPayChannel {
        RawPayChannel::Evm(RawEvmPayChannel {
            peer_id: peer_id.to_string(),
            channel_id: channel_id.to_string(),
            chain_id: 8453,
            token_network: NETWORK.to_string(),
            client_edge_url: "https://relay.example/ilp".to_string(),
        })
    }

    fn solana(peer_id: &str, channel_account: &str) -> RawPayChannel {
        RawPayChannel::Solana(RawSolanaPayChannel {
            peer_id: peer_id.to_string(),
            channel_account: channel_account.to_string(),
            client_edge_url: "https://relay.example/ilp".to_string(),
            program_id: None,
        })
    }

    fn resolve(raw: Vec<RawPayChannel>) -> Result<Vec<PayChannelConfig>, ConfigError> {
        resolve_pay_channels(raw, false, both_chains())
    }

    fn expect_evm(channel: &PayChannelConfig) -> &EvmPayChannelConfig {
        match channel {
            PayChannelConfig::Evm(evm) => evm,
            PayChannelConfig::Solana(_) => panic!("expected an EVM pay channel"),
        }
    }

    fn expect_solana(channel: &PayChannelConfig) -> &SolanaPayChannelConfig {
        match channel {
            PayChannelConfig::Solana(solana) => solana,
            PayChannelConfig::Evm(_) => panic!("expected a Solana pay channel"),
        }
    }

    /// The round trip: what an operator wrote comes back canonicalized,
    /// with the channel id in the one spelling a claim names it by (a
    /// channel named in two casings is two watermarks at the far gate).
    #[test]
    fn resolves_and_canonicalizes_a_row() {
        let channels = resolve(vec![evm(
            "relay",
            &CHANNEL.to_uppercase().replace("0X", "0x"),
        )])
        .expect("resolve");

        assert_eq!(channels.len(), 1);
        let evm = expect_evm(&channels[0]);
        assert_eq!(evm.peer_id(), "relay");
        assert_eq!(evm.channel_id(), CHANNEL);
        assert_eq!(evm.chain_id(), 8453);
        assert_eq!(evm.token_network(), [0x33u8; 20]);
        assert_eq!(evm.client_edge_url().as_str(), "https://relay.example/ilp");
        assert_eq!(channels[0].chain(), SettlementChain::Evm);
        assert_eq!(channels[0].channel(), CHANNEL);
    }

    /// Issue #1146, the whole point: a Solana row loads, and it is a
    /// genuinely different SHAPE -- a base58 `channel_account`, no
    /// `chain_id`, no `token_network`.
    #[test]
    fn resolves_a_solana_row() {
        let channels = resolve(vec![solana("store", ACCOUNT)]).expect("resolve");

        assert_eq!(channels.len(), 1);
        let solana = expect_solana(&channels[0]);
        assert_eq!(solana.peer_id(), "store");
        assert_eq!(solana.channel_account(), ACCOUNT);
        assert_eq!(
            solana.client_edge_url().as_str(),
            "https://relay.example/ilp"
        );
        assert_eq!(channels[0].chain(), SettlementChain::Solana);
        assert_eq!(channels[0].channel(), ACCOUNT);
    }

    /// The program id is `[settlement.solana]`'s, copied in during
    /// resolution -- never a value the row could have typed differently
    /// (issue #1128's rule, and ADR 0053's reason for it: the program id is
    /// inside every claim's signed message, so the one signed under has to
    /// be the one redeemed under).
    #[test]
    fn a_solana_rows_program_id_comes_from_the_settlement_table() {
        let channels = resolve(vec![solana("store", ACCOUNT)]).expect("resolve");

        assert_eq!(expect_solana(&channels[0]).program_id(), PROGRAM);
    }

    #[test]
    fn rejects_a_malformed_channel_id() {
        let result = resolve(vec![evm("relay", "0xnope")]);

        assert!(matches!(
            result,
            Err(ConfigError::PayChannelInvalidId { ref peer_id, ref value })
                if peer_id == "relay" && value == "0xnope"
        ));
    }

    #[test]
    fn rejects_a_malformed_channel_account() {
        let result = resolve(vec![solana("store", "not-an-account")]);

        assert!(matches!(
            result,
            Err(ConfigError::PayChannelInvalidSolanaAccount {
                ref peer_id,
                field: "channel_account",
                ..
            }) if peer_id == "store"
        ));
    }

    #[test]
    fn rejects_a_malformed_token_network() {
        let RawPayChannel::Evm(mut entry) = evm("relay", CHANNEL) else {
            panic!("built an EVM row");
        };
        entry.token_network = "0x12".to_string();

        assert!(matches!(
            resolve(vec![RawPayChannel::Evm(entry)]),
            Err(ConfigError::PayChannelInvalidAddress {
                field: "token_network",
                ..
            })
        ));
    }

    /// An EVM row on a node with no `[settlement.evm]` has no on-chain
    /// participant to sign as (ADR 0030's table).
    #[test]
    fn an_evm_row_without_evm_settlement_is_refused_by_name() {
        let result = resolve_pay_channels(
            vec![evm("relay", CHANNEL)],
            false,
            SettlementTables::for_tests(false, Some(PROGRAM)),
        );

        assert!(matches!(
            result,
            Err(ConfigError::PayChannelWithoutEvmSettlement { ref peer_id }) if peer_id == "relay"
        ));
    }

    /// The Solana half of the same rule: no `[settlement.solana]` is both
    /// no ed25519 key to sign with and no program id to sign under.
    #[test]
    fn a_solana_row_without_solana_settlement_is_refused_by_name() {
        let result = resolve_pay_channels(
            vec![solana("store", ACCOUNT)],
            false,
            SettlementTables::for_tests(true, None),
        );

        assert!(matches!(
            result,
            Err(ConfigError::PayChannelWithoutSolanaSettlement { ref peer_id })
                if peer_id == "store"
        ));
    }

    /// `[settlement.solana]`'s own resolver only checks the program id for
    /// non-emptiness. A pay channel needs a real address, because ADR 0053
    /// puts it inside the message every claim on this row signs.
    #[test]
    fn a_solana_row_under_an_unparseable_settlement_program_is_refused_by_name() {
        let result = resolve_pay_channels(
            vec![solana("store", ACCOUNT)],
            false,
            SettlementTables::for_tests(true, Some("not-a-program")),
        );

        assert!(matches!(
            result,
            Err(ConfigError::PayChannelSolanaSettlementProgramIdInvalid { ref peer_id, .. })
                if peer_id == "store"
        ));
    }

    /// One next hop, one nonce line: a second row for the same peering
    /// would be a second channel for one line, resolved by file order.
    /// True across chains as much as within one -- the ledger is keyed by
    /// peer id and knows nothing about chains.
    #[test]
    fn two_rows_for_one_peering_are_refused_by_name() {
        let result = resolve(vec![evm("relay", CHANNEL), evm("relay", OTHER_CHANNEL)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicatePeer { ref peer_id }) if peer_id == "relay"
        ));

        let result = resolve(vec![evm("relay", CHANNEL), solana("relay", ACCOUNT)]);
        assert!(
            matches!(
                result,
                Err(ConfigError::PayChannelDuplicatePeer { ref peer_id }) if peer_id == "relay"
            ),
            "one hop paid on two chains is still one nonce line"
        );
    }

    /// And the mirror: one channel paid from by two hops is one channel
    /// carrying two nonce lines.
    #[test]
    fn one_channel_paid_from_by_two_peerings_is_refused_by_name() {
        let result = resolve(vec![evm("relay", CHANNEL), evm("store", CHANNEL)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicate { ref value }) if value == CHANNEL
        ));

        let result = resolve(vec![solana("relay", ACCOUNT), solana("store", ACCOUNT)]);
        assert!(matches!(
            result,
            Err(ConfigError::PayChannelDuplicate { ref value }) if value == ACCOUNT
        ));
    }

    /// The two chains' spellings are disjoint, so one hop's EVM row and
    /// another's Solana row never collide with each other.
    #[test]
    fn an_evm_row_and_a_solana_row_for_different_hops_both_load() {
        let channels = resolve(vec![evm("relay", CHANNEL), solana("store", OTHER_ACCOUNT)])
            .expect("two chains, two hops");

        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].chain(), SettlementChain::Evm);
        assert_eq!(channels[1].chain(), SettlementChain::Solana);
    }

    /// A signed claim-state challenge is a capability to read a channel's
    /// state, so the ask is TLS-only unless the same loopback opt-in
    /// `[[peers]].endpoint` takes is set. Both shapes, since both ask.
    #[test]
    fn a_plaintext_client_edge_url_is_refused_unless_plaintext_is_allowed() {
        for row in [evm("relay", CHANNEL), solana("relay", ACCOUNT)] {
            let row = with_client_edge_url(row, "http://127.0.0.1:3000/ilp");
            assert!(matches!(
                resolve_pay_channels(vec![row], false, both_chains()),
                Err(ConfigError::PayChannelClientEdgeUrlScheme { ref scheme, .. })
                    if scheme == "http"
            ));
        }

        for row in [evm("relay", CHANNEL), solana("relay", ACCOUNT)] {
            let row = with_client_edge_url(row, "http://127.0.0.1:3000/ilp");
            let channels = resolve_pay_channels(vec![row], true, both_chains())
                .expect("plaintext is opted into");
            assert_eq!(channels[0].client_edge_url().scheme(), "http");
        }
    }

    /// ADR 0070 decision 2, on the row that would otherwise make the whole
    /// feature inert. A peering this node forwards to must carry a
    /// `[[pay_channels]]` row, and that row names the *payee's* client
    /// edge -- so on an onion-only payee it can only be an onion URL. If
    /// this table refused one, an onion peering would load and then refuse
    /// every packet for want of a covering claim, before the carriage was
    /// ever asked to carry one.
    ///
    /// Read through the same `is_onion_endpoint` the carriage rule reads,
    /// so the two cannot disagree about a host -- and, as there, the suffix
    /// is a suffix: a host that only looks onion is still plaintext.
    /// Both spellings the `anon` daemon has published (issue #1284), for
    /// the reason the carriage rule takes both: what a payee writes here is
    /// whatever its own daemon generated, and this node has no say in which
    /// release that was.
    #[test]
    fn an_onion_client_edge_url_needs_no_plaintext_opt_in() {
        const HIDDEN_SERVICE_URLS: [&str; 2] = [
            "http://vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion/ilp",
            "http://vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.anyone/ilp",
        ];

        for url in HIDDEN_SERVICE_URLS {
            for row in [evm("relay", CHANNEL), solana("relay", ACCOUNT)] {
                let row = with_client_edge_url(row, url);
                let channels = resolve_pay_channels(vec![row], false, both_chains())
                    .expect("an onion client edge loads on a node that opted into nothing");
                assert_eq!(channels[0].client_edge_url().as_str(), url);
            }
        }

        for url in ["http://onion.example/ilp", "http://anyone.example/ilp"] {
            for row in [evm("relay", CHANNEL), solana("relay", ACCOUNT)] {
                let row = with_client_edge_url(row, url);
                assert!(
                    matches!(
                        resolve_pay_channels(vec![row], false, both_chains()),
                        Err(ConfigError::PayChannelClientEdgeUrlScheme { ref scheme, .. })
                            if scheme == "http"
                    ),
                    "{url}: a host that merely contains the word is an ordinary clearnet host"
                );
            }
        }
    }

    fn with_client_edge_url(row: RawPayChannel, url: &str) -> RawPayChannel {
        match row {
            RawPayChannel::Evm(mut evm) => {
                evm.client_edge_url = url.to_string();
                RawPayChannel::Evm(evm)
            }
            RawPayChannel::Solana(mut solana) => {
                solana.client_edge_url = url.to_string();
                RawPayChannel::Solana(solana)
            }
        }
    }

    /// Neither a `wss://` peer endpoint nor a bare host is a client edge to
    /// post a packet to, and neither is silently coerced into one.
    #[test]
    fn a_client_edge_url_that_is_not_http_is_refused_by_name() {
        for written in ["wss://relay.example/btp", "relay.example/ilp"] {
            let row = with_client_edge_url(evm("relay", CHANNEL), written);

            let result = resolve_pay_channels(vec![row], true, both_chains());
            assert!(
                matches!(
                    result,
                    Err(ConfigError::PayChannelClientEdgeUrlScheme { .. })
                        | Err(ConfigError::PayChannelInvalidClientEdgeUrl { .. })
                ),
                "{written} should be refused"
            );
        }
    }

    /// The TOML shape itself, not just the constructor: `deny_unknown_fields`
    /// means a mistyped field is a load failure rather than a claim signed
    /// under a domain nobody wrote.
    #[test]
    fn toml_refuses_an_unknown_field() {
        let text = format!(
            r#"
peer_id = "relay"
channel_id = "{CHANNEL}"
chain_id = 8453
token_network = "{NETWORK}"
client_edge_url = "https://relay.example/ilp"
token_netwrok = "{NETWORK}"
"#
        );

        let error = toml::from_str::<RawPayChannel>(&text).expect_err("unknown field");
        assert!(error.to_string().contains("did not match"), "{error}");
    }

    /// The two shapes cannot blend: an EVM row that also names a
    /// `channel_account` matches neither variant, so nothing is silently
    /// dropped (ADR 0009).
    #[test]
    fn toml_refuses_a_row_that_mixes_the_two_shapes() {
        let text = format!(
            r#"
peer_id = "relay"
channel_id = "{CHANNEL}"
channel_account = "{ACCOUNT}"
chain_id = 8453
token_network = "{NETWORK}"
client_edge_url = "https://relay.example/ilp"
"#
        );

        assert!(toml::from_str::<RawPayChannel>(&text).is_err());
    }

    /// A Solana row's `program_id` is refused BY NAME rather than being
    /// lost in `#[serde(untagged)]`'s "matched no variant" -- the failure
    /// an operator carrying the key over from an older `[[peer_channels]]`
    /// row would otherwise get.
    #[test]
    fn a_solana_row_naming_a_program_id_is_refused_by_name() {
        let text = format!(
            r#"
peer_id = "store"
channel_account = "{ACCOUNT}"
program_id = "{PROGRAM}"
client_edge_url = "https://relay.example/ilp"
"#
        );

        let raw: RawPayChannel = toml::from_str(&text).expect("the removed key still parses");
        assert!(matches!(
            resolve(vec![raw]),
            Err(ConfigError::PayChannelProgramIdNotDeclared { ref peer_id }) if peer_id == "store"
        ));
    }

    /// And it is named even when it is not a string -- the same
    /// `toml::Value` reason `[[peer_channels]]` gives.
    #[test]
    fn a_solana_row_naming_a_non_string_program_id_is_refused_by_name_too() {
        let text = format!(
            r#"
peer_id = "store"
channel_account = "{ACCOUNT}"
program_id = 5
client_edge_url = "https://relay.example/ilp"
"#
        );

        let raw: RawPayChannel = toml::from_str(&text).expect("the removed key still parses");
        assert!(matches!(
            resolve(vec![raw]),
            Err(ConfigError::PayChannelProgramIdNotDeclared { .. })
        ));
    }
}
