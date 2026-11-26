//! Cross-table config rules: one module that takes the parsed tables and returns the first
//! named refusal (issue #1727, spec #1690).
//!
//! A rule that reads one table lives beside that table. A rule that reads several lives here,
//! so [`Config::from_toml_str`](crate::Config) only parses, resolves and asks [`check`]:
//!
//! 1. a route whose next hop can never originate to (peers x routes),
//! 2. every peering rule, which is `connector_domain::peering_admission`'s (ADR 0058, issues
//!    #1691, #1692) -- the same code the runtime's writes run -- with its refusal mapped to the
//!    named one operators already know ([`PeeringAdmissionRefusal`]),
//! 3. an XRPL settlement table on a node that declares no tokens,
//! 4. the zk edge's declared rate into every settlement asset,
//! 5. `state_dir` against every table that needs a journal.

use std::path::{Path, PathBuf};

use connector_domain::peering_admission::{
    admit as admit_peerings, check_bindings, ChannelFact, PeerRouteFacts, PeeringFacts,
    PeeringRefusal, PeeringSet,
};
use connector_domain::AssetId;
use thiserror::Error;

use crate::denomination::{DenominationConfig, DenominationRefusal};
use crate::error::ConfigError;
use crate::operator::OperatorConfig;
use crate::pay_channel::PayChannelConfig;
use crate::peer::{PeerConfig, PeerRefusal};
use crate::peer_channel::PeerChannelConfig;
use crate::route::PeerRouteConfig;
use crate::settlement::{SettlementConfig, SettlementTables};
use crate::zk_note::ZkNoteConfig;

/// The parsed, per-table-validated tables the cross-table rules read.
pub(crate) struct Tables<'a> {
    pub peers: &'a [PeerConfig],
    pub peer_routes: &'a [PeerRouteConfig],
    pub peer_channels: &'a [PeerChannelConfig],
    pub pay_channels: &'a [PayChannelConfig],
    pub settlements: &'a [SettlementConfig],
    pub settlement_tables: &'a SettlementTables,
    pub denomination: &'a DenominationConfig,
    pub zk_note: Option<&'a ZkNoteConfig>,
    pub operator: Option<&'a OperatorConfig>,
    pub state_dir: Option<&'a Path>,
}

/// The first cross-table refusal of `tables`, or `Ok`.
pub(crate) fn check(tables: &Tables<'_>) -> Result<(), ConfigError> {
    check_peer_routes_deliverable(tables)?;
    check_peering_admission(tables)?;
    check_xrpl_declares_tokens(tables)?;
    check_zk_note_rates(tables)?;
    check_state_dir(tables)
}

fn check_peer_routes_deliverable(tables: &Tables<'_>) -> Result<(), ConfigError> {
    for peer_route in tables.peer_routes {
        // A route naming no peer is `peering_admission`'s refusal, made below with the other
        // binding rules.
        let Some(peer) = tables
            .peers
            .iter()
            .find(|peer| peer.id() == peer_route.peer_id())
        else {
            continue;
        };
        // The intersection rule, in the one direction this connector's own file can decide
        // (`peer-carriage-spec.md` section 2.2, 6.4(1)): a route whose next hop is a peering
        // this connector can never originate to is a route that could only ever answer `T01`.
        // What the *far* side exposes is not knowable from here and stays a runtime dial
        // failure.
        if !peer.can_originate() {
            return Err(ConfigError::Peer(PeerRefusal::PeerRouteUndeliverable {
                prefix: peer_route.prefix().to_string(),
                peer_id: peer_route.peer_id().to_string(),
            }));
        }
    }
    Ok(())
}

/// Every cross-table rule of a peering -- unknown peer, orphaned and unbound channel rows, a
/// channel in both directions, differing networks, a pay channel on an unsettled chain, a route
/// with no channel to pay from -- is `connector_domain::peering_admission`'s, the same code the
/// runtime's writes run. This only states the facts and maps its refusal to the named one.
///
/// Keyed on ROUTES, not on peerings, for the pay-channel rule (ADR 0042): a peering this node
/// only ever receives from owes nothing and needs no `[[pay_channels]]` row.
fn check_peering_admission(tables: &Tables<'_>) -> Result<(), ConfigError> {
    let channel_fact = |peer_id: &str,
                        channel: Option<String>,
                        chain: &'static str,
                        network: Option<String>| ChannelFact {
        peer_id: peer_id.to_string(),
        channel,
        chain: Some(chain),
        network,
    };
    let set = PeeringSet {
        peerings: tables
            .peers
            .iter()
            .map(|peer| PeeringFacts::new(peer.id()))
            .collect(),
        inbound: tables
            .peer_channels
            .iter()
            .map(|channel| {
                channel_fact(
                    channel.peer_id(),
                    channel.inbound_channel().map(ToString::to_string),
                    channel.chain().name(),
                    channel.network().map(ToString::to_string),
                )
            })
            .collect(),
        pay: tables
            .pay_channels
            .iter()
            .map(|channel| {
                channel_fact(
                    channel.peer_id(),
                    Some(channel.outbound_channel().to_string()),
                    channel.chain().name(),
                    channel.network().map(ToString::to_string),
                )
            })
            .collect(),
        routes: tables
            .peer_routes
            .iter()
            .map(|route| PeerRouteFacts::new(route.prefix(), route.peer_id()))
            .collect(),
        settled_chains: tables.settlement_tables.settled_chains(),
        ..PeeringSet::default()
    };
    // Unknown peer, orphaned `[[peer_channels]]` rows and unbound peerings (peer-carriage-spec
    // section 1.2) need no pay row, so they are refused ahead of the whole-set check.
    check_bindings(&set).map_err(ConfigError::from)?;
    admit_peerings(&set).map_err(ConfigError::from)
}

/// ADR 0087 decision 6 (#1636): every XRPL leg is a dealing boundary, and a node that declares
/// no token resolves no peering, so it would cross that boundary unconverted. Which tokens it
/// must declare is `resolve_client_channel_assets`'s rule, once it declares any.
fn check_xrpl_declares_tokens(tables: &Tables<'_>) -> Result<(), ConfigError> {
    if tables.denomination.declares_tokens() {
        return Ok(());
    }
    let xrpl = tables
        .settlements
        .iter()
        .find_map(|settlement| match settlement {
            SettlementConfig::Xrpl(xrpl) => Some(xrpl),
            _ => None,
        });
    match xrpl {
        Some(xrpl) => Err(ConfigError::Denomination(
            DenominationRefusal::XrplSettlementWithoutTokens {
                table: format!("[settlement.{}]", xrpl.table_name()),
                asset: xrpl.asset().to_string(),
            },
        )),
        None => Ok(()),
    }
}

/// ADR 0080 decision 7, as amended: a zk edge prices in MINA, and a forward out of it is paid
/// in a settlement asset, so it crosses a denomination at a rate the operator declared -- no
/// TWAP source quotes MINA. Every settlement table this node pays a next hop in therefore needs
/// a `[[rates]]` row from the vault's asset to the token that table holds. A zk edge with no
/// settlement table forwards nothing.
fn check_zk_note_rates(tables: &Tables<'_>) -> Result<(), ConfigError> {
    let Some(zk_note) = tables.zk_note else {
        return Ok(());
    };
    let from = zk_note.asset();
    for settlement in tables.settlements {
        let declared = tables.denomination.rates().iter().any(|row| {
            row.from() == &from
                && settlement.holds(row.to())
                && tables.denomination.rate(row.from(), row.to()).is_some()
        });
        if !declared {
            return Err(CrossTableRefusal::ZkNoteRateMissing {
                from,
                to: settlement.asset_spelling(),
            }
            .into());
        }
    }
    Ok(())
}

/// What a node must be able to remember across a restart (ADR 0078 decision 8, issue #605).
fn check_state_dir(tables: &Tables<'_>) -> Result<(), ConfigError> {
    let Some(state_dir) = tables.state_dir else {
        return check_state_dir_unset(tables);
    };
    if state_dir.exists() && !state_dir.is_dir() {
        return Err(CrossTableRefusal::StateDirNotADirectory {
            path: PathBuf::from(state_dir),
        }
        .into());
    }
    Ok(())
}

fn check_state_dir_unset(tables: &Tables<'_>) -> Result<(), ConfigError> {
    // A spending or deposit limit counts in a journal under `state_dir`; a node with nowhere to
    // keep it would hand every key a fresh day on each restart.
    for controller in tables.operator.map_or(&[][..], |o| o.controller_keys()) {
        let limit = if controller.spending_limit().is_some() {
            "spending limit"
        } else if controller.deposit_limit().is_some() {
            "deposit limit"
        } else {
            continue;
        };
        return Err(CrossTableRefusal::LimitWithoutStateDir {
            entry: controller.name().to_string(),
            limit,
        }
        .into());
    }
    // A node that can accept a claim must be able to remember having accepted it (issue #605):
    // one whose watermarks live only in memory hands out free service after every restart. The
    // trigger is "can this node resolve a channel", not "did it declare one" (issue #1186).
    // Most specific diagnosis first; the settlement arm is last so it does not swallow the
    // channel tables, each of which already requires a settlement table of its own chain.
    if tables.zk_note.is_some() {
        // ADR 0080 decision 11: a zk edge journals every spent nullifier and keeps its evidence
        // file under state_dir.
        Err(CrossTableRefusal::ZkNoteWithoutStateDir.into())
    } else if !tables.peer_channels.is_empty() {
        // A peer's voucher watermark is no less a replay defence than a client's (ADR 0075).
        Err(CrossTableRefusal::PeerChannelsWithoutStateDir.into())
    } else if !tables.pay_channels.is_empty() {
        // Unreachable through a loadable file today (a pay row needs a peering, which needs a
        // peer row, which already demands a state_dir) and kept: the outbound-channel journal
        // is the only record of the channel the row names and of every voucher signed on it.
        Err(CrossTableRefusal::PayChannelsWithoutStateDir.into())
    } else if !tables.settlements.is_empty() {
        // Exactly the case the channel books do not cover: a node that declares no channel and
        // is paid by strangers anyway.
        Err(CrossTableRefusal::SettlementWithoutStateDir.into())
    } else {
        Ok(())
    }
}

/// The peering-admission refusals, mapped from the domain's rule set
/// (`connector_domain::peering_admission`, ADR 0058). Config load and the runtime run the
/// same rules, so they cannot disagree; this is the one place the domain's refusal becomes
/// the named one an operator reads at boot (ADR 0009).
#[derive(Debug, Error)]
pub enum PeeringAdmissionRefusal {
    #[error(
        "route '{prefix}' forwards to peer_id '{peer_id}', which no '[[peers]]' entry configures"
    )]
    UnknownPeerId { prefix: String, peer_id: String },

    #[error(
        "peer '{id}' has no '[[peer_channels]]' entry: a peer role is proven by a voucher on a \
         channel whose voucher signer is bound to the peering, or by the claim-state challenge \
         that signer signs (peer-carriage-spec.md §1.2, ADR 0075 decision 5), and a \
         config-declared peering's signer is named by its '[[peer_channels]]' row -- so without \
         one this peering can never take the peer role and its vouchers would be judged as a \
         stranger's. Add:\n\
         \n\
             [[peer_channels]]\n\
             peer_id = \"{id}\"\n\
             voucher_signer = \"<the peer's settlement address (EVM) or key (Solana)>\"\n\
         \n\
         see docs/protocol/configuration-spec.md"
    )]
    PeerChannelUnbound { id: String },

    #[error(
        "'[[peer_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures \
         -- a voucher signer bound to a peering that does not exist binds nothing"
    )]
    PeerChannelOrphaned { peer_id: String },

    /// A config peering's `[[peer_channels]]` row and its `[[pay_channels]]`
    /// row are on two networks of one chain (ADR 0086 decision 11): a
    /// peering is on one network, both of its channels. A node that moves
    /// value between networks does it with two peerings and a forward.
    #[error(
        "peering '{peer_id}' has its '[[peer_channels]]' row on {inbound} and its \
         '[[pay_channels]]' row on {outbound}: both channels of a peering are on one network \
         (ADR 0086 decision 11) -- put both rows on the same network, or make two peerings"
    )]
    PeeringNetworksDiffer {
        peer_id: String,
        inbound: String,
        outbound: String,
    },

    // -- `[[pay_channels]]` (ADR 0042 item 2, as ADR 0075 decision 6 amends
    // it): this node's own outbound x402 channel toward a next hop. See
    // `crate::pay_channel`'s module header.
    #[error(
        "'[[pay_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures -- \
         a channel that pays a peering that does not exist pays nobody. This table is how this \
         node covers every PREPARE it forwards to a hop (ADR 0042); the hop is named by the \
         same '[[peers]]' id a route's peer_id names"
    )]
    PayChannelOrphaned { peer_id: String },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' names an outbound channel on {chain}, but this \
         node has no '{heading}' table, so there is no paying half to sign a voucher \
         with (ADR 0075). Add the table, or pay over a chain this node settles on",
        heading = crate::settlement::settlement_heading(.chain)
    )]
    PayChannelWithoutX402 {
        peer_id: String,
        chain: &'static str,
    },

    #[error(
        "channel '{value}' is both a '[[peer_channels]]' inbound_channel and a \
         '[[pay_channels]]' outbound_channel: an x402 channel moves value one way, so this node \
         is either its payer or its receiver, never both (ADR 0075)"
    )]
    ChannelInBothDirections { value: String },

    /// A runtime write took a peer id or route prefix the config file owns
    /// (ADR 0034). `peering_admission` owns the rule; a file's own rows are
    /// all config, so load cannot reach it -- it is named so that the two
    /// surfaces map every refusal.
    #[error("'{value}' is defined in this node's config file and cannot be changed at runtime")]
    PeeringOwnedByConfig { value: String },

    #[error(
        "peer '{peer_id}' is the next hop of route '{prefix}' but has no '[[pay_channels]]' \
         entry: a connector covers every PREPARE it sends (ADR 0042), so a peering this node \
         FORWARDS to must name the channel it pays that hop on. Open one with 'POST /channels' \
         and add:\n\
         \n\
             [[pay_channels]]\n\
             peer_id = \"{peer_id}\"\n\
             outbound_channel = \"<the x402 channel id (EVM) or account (Solana) it opened>\"\n\
             client_edge_url = \"<that hop's own POST /ilp endpoint>\"\n\
         \n\
         This key is REQUIRED, which by ADR 0009 makes it a breaking deploy: land the config \
         before moving the image tag, never the other way round"
    )]
    PayChannelUnbound { prefix: String, peer_id: String },
}

/// The cross-table rules that are not peering rules: state_dir against the tables that
/// need a journal, and the zk edge's declared rates (issue #1727).
#[derive(Debug, Error)]
pub enum CrossTableRefusal {
    #[error(
        "'[[peer_channels]]' is configured but 'state_dir' is not: the vouchers a peer pays this \
         node with are journaled beside a client's, and their watermark is the replay defence -- \
         kept only in memory, every voucher a peer has already spent becomes spendable again the \
         next time this process restarts (issue #605). Set a top-level state_dir to a directory \
         this node can write, and mount it so it outlives the container"
    )]
    PeerChannelsWithoutStateDir,

    #[error(
        "'[[pay_channels]]' is configured but 'state_dir' is not: the channel a row names is \
         held in this node's outbound-channel journal under state_dir, which is also where every \
         voucher signed on it is recorded before it is handed out (ADR 0075 decision 8)"
    )]
    PayChannelsWithoutStateDir,

    /// A settlement table is what lets this node admit a voucher on a
    /// channel it has never been configured for (ADR 0052, CF-27), so a node
    /// configuring one accepts claims from strangers -- and therefore has
    /// watermarks to lose. Issue #1186: this arm did not exist, so the
    /// permissionless shape, which is the one an operator should be running,
    /// was the one shape that could boot with its watermarks in memory.
    #[error(
        "a settlement backend is configured but 'state_dir' is not: this node resolves an \
         undeclared channel from chain and accepts its voucher (ADR 0052), so it takes payment \
         from senders it has never been configured for -- and would keep their replay \
         watermarks only in memory. Every claim any of them has already spent becomes \
         spendable again the next time this process restarts, and nothing in a log shows \
         that it did (issue #605, issue #1186). Set a top-level state_dir to a directory \
         this node can write, and mount it so it outlives the container"
    )]
    SettlementWithoutStateDir,

    /// A spending or deposit limit is counted in a journal under `state_dir` (ADR 0078
    /// decision 8); without one the count would not survive a restart.
    #[error(
        "controller key '{entry}' states a {limit} but 'state_dir' is not set: the day's \
         count is journaled under state_dir, and a count held only in memory would reset the \
         day at every restart. Set a top-level state_dir to a directory this node can write"
    )]
    LimitWithoutStateDir { entry: String, limit: &'static str },

    #[error("state_dir '{path}' exists but is not a directory")]
    StateDirNotADirectory { path: PathBuf },

    /// A zk edge with nowhere to journal its spent nullifiers or keep its evidence.
    #[error(
        "[zk_note] is set but 'state_dir' is not: a zk edge journals every spent nullifier and \
         keeps its evidence file under state_dir, and one held only in memory would let a note \
         open a second allowance from a state it already spent after a restart. Set a \
         top-level state_dir to a directory this node can write"
    )]
    ZkNoteWithoutStateDir,

    /// A zk edge that forwards in a settlement asset has no declared rate out of MINA.
    #[error(
        "[zk_note] is set and this node pays its next hops in '{to}', but no [[rates]] row \
         declares a rate for '{from}' -> '{to}': no TWAP source quotes MINA, so the rate a \
         forward crosses at must be declared (ADR 0080 decision 7, ADR 0071)"
    )]
    ZkNoteRateMissing { from: AssetId, to: String },
}

impl From<PeeringRefusal> for PeeringAdmissionRefusal {
    /// No wildcard arm: a new rule does not compile until it is named here (issue #1692).
    fn from(refusal: PeeringRefusal) -> Self {
        match refusal {
            PeeringRefusal::PeerIdOwnedByConfig(value)
            | PeeringRefusal::PrefixOwnedByConfig(value) => {
                PeeringAdmissionRefusal::PeeringOwnedByConfig { value }
            }
            PeeringRefusal::UnknownPeer { prefix, peer_id } => {
                PeeringAdmissionRefusal::UnknownPeerId { prefix, peer_id }
            }
            PeeringRefusal::PeerChannelOrphaned { peer_id } => {
                PeeringAdmissionRefusal::PeerChannelOrphaned { peer_id }
            }
            PeeringRefusal::PeeringWithoutChannel(id) => {
                PeeringAdmissionRefusal::PeerChannelUnbound { id }
            }
            PeeringRefusal::PayChannelOrphaned { peer_id } => {
                PeeringAdmissionRefusal::PayChannelOrphaned { peer_id }
            }
            PeeringRefusal::PayChannelOnUnsettledChain { peer_id, chain } => {
                PeeringAdmissionRefusal::PayChannelWithoutX402 { peer_id, chain }
            }
            PeeringRefusal::ChannelInBothDirections { channel } => {
                PeeringAdmissionRefusal::ChannelInBothDirections { value: channel }
            }
            PeeringRefusal::PeeringNetworksDiffer {
                peer_id,
                inbound,
                outbound,
            } => PeeringAdmissionRefusal::PeeringNetworksDiffer {
                peer_id,
                inbound,
                outbound,
            },
            PeeringRefusal::RouteWithoutPayChannel { prefix, peer_id } => {
                PeeringAdmissionRefusal::PayChannelUnbound { prefix, peer_id }
            }
        }
    }
}

impl From<PeeringRefusal> for ConfigError {
    fn from(refusal: PeeringRefusal) -> Self {
        ConfigError::PeeringAdmission(refusal.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal of the peering-admission rule set maps to a named config refusal whose
    /// message is the one operators already read (ADR 0058, issue #1727). Exhaustive over
    /// `PeeringRefusal`: a rule added to the domain does not compile here until it is listed.
    fn mapped(refusal: PeeringRefusal) -> ConfigError {
        // The match is the exhaustiveness check; the body only names the variant.
        match &refusal {
            PeeringRefusal::PeerIdOwnedByConfig(_)
            | PeeringRefusal::PrefixOwnedByConfig(_)
            | PeeringRefusal::UnknownPeer { .. }
            | PeeringRefusal::PeerChannelOrphaned { .. }
            | PeeringRefusal::PeeringWithoutChannel(_)
            | PeeringRefusal::PayChannelOrphaned { .. }
            | PeeringRefusal::PayChannelOnUnsettledChain { .. }
            | PeeringRefusal::ChannelInBothDirections { .. }
            | PeeringRefusal::PeeringNetworksDiffer { .. }
            | PeeringRefusal::RouteWithoutPayChannel { .. } => {}
        }
        ConfigError::from(refusal)
    }

    fn s(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn a_peer_id_or_prefix_the_file_owns_is_peering_owned_by_config() {
        for refusal in [
            PeeringRefusal::PeerIdOwnedByConfig(s("hop")),
            PeeringRefusal::PrefixOwnedByConfig(s("hop")),
        ] {
            let error = mapped(refusal);
            assert!(matches!(
                error,
                ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PeeringOwnedByConfig { .. })
            ));
            assert_eq!(
                error.to_string(),
                "'hop' is defined in this node's config file and cannot be changed at runtime"
            );
        }
    }

    #[test]
    fn a_route_to_no_peer_is_unknown_peer_id() {
        let error = mapped(PeeringRefusal::UnknownPeer {
            prefix: s("g.x"),
            peer_id: s("ghost"),
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::UnknownPeerId { .. })
        ));
        assert_eq!(
            error.to_string(),
            "route 'g.x' forwards to peer_id 'ghost', which no '[[peers]]' entry configures"
        );
    }

    #[test]
    fn a_peer_channel_row_for_no_peer_is_peer_channel_orphaned() {
        let error = mapped(PeeringRefusal::PeerChannelOrphaned {
            peer_id: s("ghost"),
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PeerChannelOrphaned { .. })
        ));
        assert_eq!(
            error.to_string(),
            "'[[peer_channels]]' names peer_id 'ghost', which no '[[peers]]' entry configures \
             -- a voucher signer bound to a peering that does not exist binds nothing"
        );
    }

    #[test]
    fn a_peering_with_no_channel_row_is_peer_channel_unbound() {
        let error = mapped(PeeringRefusal::PeeringWithoutChannel(s("hop")));
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PeerChannelUnbound { .. })
        ));
        let message = error.to_string();
        assert!(message.starts_with("peer 'hop' has no '[[peer_channels]]' entry: a peer role"));
        assert!(message.ends_with("see docs/protocol/configuration-spec.md"));
    }

    #[test]
    fn a_pay_channel_row_for_no_peer_is_pay_channel_orphaned() {
        let error = mapped(PeeringRefusal::PayChannelOrphaned {
            peer_id: s("ghost"),
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PayChannelOrphaned { .. })
        ));
        assert!(error.to_string().starts_with(
            "'[[pay_channels]]' names peer_id 'ghost', which no '[[peers]]' entry configures -- "
        ));
    }

    #[test]
    fn a_pay_channel_on_an_unsettled_chain_is_pay_channel_without_x402() {
        let error = mapped(PeeringRefusal::PayChannelOnUnsettledChain {
            peer_id: s("hop"),
            chain: "evm",
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PayChannelWithoutX402 { .. })
        ));
        assert_eq!(
            error.to_string(),
            "'[[pay_channels]]' for peer 'hop' names an outbound channel on evm, but this node \
             has no '[settlement.eip155.<chainId>]' table, so there is no paying half to sign a voucher with \
             (ADR 0075). Add the table, or pay over a chain this node settles on"
        );
    }

    #[test]
    fn a_channel_in_both_directions_keeps_its_name_and_message() {
        let error = mapped(PeeringRefusal::ChannelInBothDirections { channel: s("in") });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::ChannelInBothDirections { .. })
        ));
        assert_eq!(
            error.to_string(),
            "channel 'in' is both a '[[peer_channels]]' inbound_channel and a \
             '[[pay_channels]]' outbound_channel: an x402 channel moves value one way, so this \
             node is either its payer or its receiver, never both (ADR 0075)"
        );
    }

    #[test]
    fn two_networks_in_one_peering_is_peering_networks_differ() {
        let error = mapped(PeeringRefusal::PeeringNetworksDiffer {
            peer_id: s("hop"),
            inbound: s("a"),
            outbound: s("b"),
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PeeringNetworksDiffer { .. })
        ));
        assert_eq!(
            error.to_string(),
            "peering 'hop' has its '[[peer_channels]]' row on a and its '[[pay_channels]]' row \
             on b: both channels of a peering are on one network (ADR 0086 decision 11) -- put \
             both rows on the same network, or make two peerings"
        );
    }

    #[test]
    fn a_route_with_no_pay_channel_is_pay_channel_unbound() {
        let error = mapped(PeeringRefusal::RouteWithoutPayChannel {
            prefix: s("g.x"),
            peer_id: s("hop"),
        });
        assert!(matches!(
            error,
            ConfigError::PeeringAdmission(PeeringAdmissionRefusal::PayChannelUnbound { .. })
        ));
        let message = error.to_string();
        assert!(message.starts_with(
            "peer 'hop' is the next hop of route 'g.x' but has no '[[pay_channels]]' entry: "
        ));
        assert!(message
            .ends_with("land the config before moving the image tag, never the other way round"));
    }

    #[test]
    fn the_refusal_is_transparent_in_the_config_error() {
        let refusal = PeeringAdmissionRefusal::PeeringOwnedByConfig { value: s("hop") };
        let message = refusal.to_string();
        assert_eq!(ConfigError::from(refusal).to_string(), message);
    }
}
