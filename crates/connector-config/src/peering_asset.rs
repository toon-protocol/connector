use connector_domain::AssetId;

use crate::denomination::DenominationConfig;
use crate::error::ConfigError;
use crate::pay_channel::PayChannelConfig;
use crate::peer::PeerConfig;
use crate::peer_channel::PeerChannelConfig;
use crate::settlement::{SettlementChain, SettlementConfig};

/// Which declared token each peering's channels are denominated in (ADR
/// 0071 decision 1, issue #1292).
///
/// A packet's amount has no unit of its own -- it is denominated by the
/// channel it rides -- so the question "is this forward a conversion" is
/// really "do these two peerings hold different tokens". Nothing could
/// answer it before this table existed: an EVM `[[peer_channels]]` row
/// names a per-token `TokenNetwork` and a Solana row names no token at
/// all, and neither surfaces as an asset anything can compare.
///
/// **Empty is the default and is a whole answer.** A node that declares no
/// `[[tokens]]` resolves no peering, holds the default value here, and
/// forwards exactly as it did before ADR 0071 -- no new required key, no
/// new refusal, nothing on the packet path that was not there before. A
/// node that *does* declare tokens is held to the rule instead: every one
/// of its peerings resolves to exactly one declared token, or
/// [`Config::load`](crate::Config::load) refuses by name.
///
/// A pure function of loaded config, and deliberately so: this is read on
/// the forwarding path, for every forward, where there is no I/O to do and
/// no chain to ask.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeeringAssets {
    /// In `[[peers]]` declaration order, and small -- a node has a handful
    /// of peerings, so a linear scan beats a map an operator would have to
    /// be told about.
    peerings: Vec<(String, AssetId)>,
}

impl PeeringAssets {
    /// Whether this node resolved no peering at all -- `true` for every
    /// node that declares no `[[tokens]]`, which is every config this
    /// repository ships. The question a caller asks before any other,
    /// since `true` means no forward this node makes can be a conversion.
    pub fn is_empty(&self) -> bool {
        self.peerings.is_empty()
    }

    /// Every resolved peering as `(peer_id, token)`, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AssetId)> {
        self.peerings
            .iter()
            .map(|(peer_id, asset)| (peer_id.as_str(), asset))
    }

    /// The declared token `peer_id`'s channels hold, or `None` when this
    /// node resolved none for it.
    ///
    /// `None` has two causes and neither is "this peering holds an
    /// undeclared token" -- that is a boot refusal
    /// ([`ConfigError::PeeringTokenNotDeclared`]), so it never reaches a
    /// reader. It is either a node that declares no tokens at all, or a
    /// peering with no `[[peers]]` row for this table to have been
    /// resolved from: one established at runtime over the operator surface
    /// (ADR 0058), whose channels this file never named.
    pub fn asset(&self, peer_id: &str) -> Option<&AssetId> {
        self.peerings
            .iter()
            .find(|(id, _)| id == peer_id)
            .map(|(_, asset)| asset)
    }

    /// The **denomination boundary** between two peerings, if there is one:
    /// `Some((incoming, outgoing))` when the two hold different tokens, and
    /// `None` when they hold the same one or when either does not resolve.
    ///
    /// The ordered pair, in the order a conversion reads it -- the same
    /// order [`DenominationConfig::rate`] and
    /// [`DenominationConfig::guards_for`] take their arguments in, because
    /// direction is the trade and the reverse pair is a different price.
    ///
    /// This is the whole question ADR 0071 decision 1 turns on, asked of
    /// loaded config alone: a forward whose two legs hold one token takes
    /// the flat fee and nothing else (`None` here), and one whose legs hold
    /// two crosses a boundary at a declared rate (`Some`, and the pair is
    /// what the rate is looked up by).
    pub fn boundary_between(
        &self,
        incoming_peer_id: &str,
        outgoing_peer_id: &str,
    ) -> Option<(&AssetId, &AssetId)> {
        let incoming = self.asset(incoming_peer_id)?;
        let outgoing = self.asset(outgoing_peer_id)?;
        (incoming != outgoing).then_some((incoming, outgoing))
    }
}

/// Collect `(peer_id, token)` pairs into a table, in the order given.
///
/// [`Config::load`](crate::Config::load) builds the real one through
/// `resolve_peering_assets`, which is where every boot refusal lives; this
/// validates nothing and is for a caller that already holds the resolved
/// pairs -- `connector-runtime`'s forwarding tests, which exercise a
/// denomination crossing without a whole config file around it (issue
/// #1295). Declaration order is preserved, because [`PeeringAssets::iter`]
/// promises it.
impl FromIterator<(String, AssetId)> for PeeringAssets {
    fn from_iter<I: IntoIterator<Item = (String, AssetId)>>(peerings: I) -> PeeringAssets {
        PeeringAssets {
            peerings: peerings.into_iter().collect(),
        }
    }
}

/// The token a channel on `chain` settles in, read off the
/// `[settlement.<chain>]` table that already states it
/// ([`SettlementConfig::asset`], issue #1290).
///
/// Derived from that table and never declared a second time, which is the
/// point: an EVM `[[peer_channels]]` row's `token_network` is a per-token
/// contract whose token only a chain read could name, and a Solana row
/// names no token at all -- so the one place a node's answer can come from
/// without asking a chain is the table whose key signs the redemption.
///
/// `None` is a channel on a chain with no settlement table, which no file
/// that reached this point has: `resolve_peer_channels` and
/// `resolve_pay_channels` already refuse such a row by name and more
/// precisely (issue #1138).
fn settlement_asset(settlements: &[SettlementConfig], chain: SettlementChain) -> Option<AssetId> {
    settlements
        .iter()
        .find(|settlement| settlement.chain() == chain)
        .map(SettlementConfig::asset)
}

/// Resolve every peering to the one declared token its channels hold (ADR
/// 0071 decision 1, issue #1292).
///
/// Both channel tables of a peering are read, not just the inbound one: a
/// `[[peer_channels]]` row is what an arriving claim is judged against and
/// a `[[pay_channels]]` row is what a departing one is signed from, so a
/// peering whose two rows sat on different chains would be two units
/// wearing one name -- and the forward out of it would be denominated by
/// whichever table the reader happened to consult.
///
/// Runs only for a node that declares `[[tokens]]`. That is the whole of
/// the rule protecting everyone else: a node that deals nothing resolves
/// nothing, gains no required key and no new refusal, and forwards exactly
/// as it did before this function existed.
pub(crate) fn resolve_peering_assets(
    peers: &[PeerConfig],
    peer_channels: &[PeerChannelConfig],
    pay_channels: &[PayChannelConfig],
    settlements: &[SettlementConfig],
    denomination: &DenominationConfig,
) -> Result<PeeringAssets, ConfigError> {
    if !denomination.declares_tokens() {
        return Ok(PeeringAssets::default());
    }

    let mut peerings = Vec::with_capacity(peers.len());
    for peer in peers {
        let chains = peer_channels
            .iter()
            .filter(|channel| channel.peer_id() == peer.id())
            .map(PeerChannelConfig::chain)
            .chain(
                pay_channels
                    .iter()
                    .filter(|channel| channel.peer_id() == peer.id())
                    .map(PayChannelConfig::chain),
            );

        let mut resolved: Option<AssetId> = None;
        for chain in chains {
            let Some(asset) = settlement_asset(settlements, chain) else {
                // Unreachable from a file that got this far -- a channel
                // row whose chain has no settlement table is refused by
                // name, earlier and more precisely (issue #1138).
                continue;
            };
            match &resolved {
                None => resolved = Some(asset),
                Some(first) if first == &asset => {}
                Some(first) => {
                    return Err(ConfigError::PeeringTokenAmbiguous {
                        peer_id: peer.id().to_string(),
                        first: first.clone(),
                        second: asset,
                    })
                }
            }
        }

        let Some(asset) = resolved else {
            // Also unreachable: `PeerChannelUnbound` has already refused a
            // peering with no channel binding at all, which is the only
            // way to arrive here with nothing resolved.
            continue;
        };
        if denomination.token(&asset).is_none() {
            return Err(ConfigError::PeeringTokenNotDeclared {
                peer_id: peer.id().to_string(),
                asset,
            });
        }
        peerings.push((peer.id().to_string(), asset));
    }

    Ok(PeeringAssets { peerings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("an asset")
    }

    fn assets(peerings: &[(&str, &str)]) -> PeeringAssets {
        PeeringAssets {
            peerings: peerings
                .iter()
                .map(|(peer_id, token)| ((*peer_id).to_string(), asset(token)))
                .collect(),
        }
    }

    const USDC_BASE: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    const ANYONE_BASE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";

    /// The question issue #1295 asks on the forwarding path, both ways
    /// round: direction is the trade, so the pair comes back ordered as it
    /// was asked and never sorted.
    #[test]
    fn two_peerings_holding_different_tokens_are_an_ordered_pair() {
        let resolved = assets(&[("in", USDC_BASE), ("out", ANYONE_BASE)]);

        assert_eq!(
            resolved.boundary_between("in", "out"),
            Some((&asset(USDC_BASE), &asset(ANYONE_BASE)))
        );
        assert_eq!(
            resolved.boundary_between("out", "in"),
            Some((&asset(ANYONE_BASE), &asset(USDC_BASE)))
        );
    }

    /// One token on both legs is not a boundary, and a forward across it
    /// takes the flat fee alone -- the `None` a caller must not read as
    /// "unknown".
    #[test]
    fn two_peerings_holding_one_token_are_no_boundary() {
        let resolved = assets(&[("in", USDC_BASE), ("out", USDC_BASE)]);

        assert_eq!(resolved.boundary_between("in", "out"), None);
    }

    /// A peering this table never resolved -- one established at runtime
    /// (ADR 0058), or every peering of a node that declares no tokens.
    #[test]
    fn an_unresolved_peering_has_no_token_and_no_boundary() {
        let resolved = assets(&[("in", USDC_BASE)]);

        assert_eq!(resolved.asset("runtime-peer"), None);
        assert_eq!(resolved.boundary_between("in", "runtime-peer"), None);
        assert_eq!(resolved.boundary_between("runtime-peer", "in"), None);
        assert!(PeeringAssets::default().is_empty());
    }
}
