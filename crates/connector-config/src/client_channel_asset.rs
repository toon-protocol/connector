use connector_domain::AssetId;

use crate::denomination::DenominationConfig;
use crate::error::ConfigError;
use crate::peering_asset::settlement_asset;
use crate::settlement::{SettlementChain, SettlementConfig};

/// Which declared token each client channel this node accepts claims on is
/// denominated in (ADR 0071 decision 1, issue #1301) -- the client edge's
/// half of what [`PeeringAssets`](crate::PeeringAssets) answers for a
/// peering.
///
/// A buyer's packet is denominated by the channel its covering claim was
/// written against, exactly as a peer's is, so a forward out of a client
/// arrival crosses a denomination boundary on precisely the same terms: do
/// the arriving channel and the outgoing peering hold different tokens.
/// Nothing could ask that before this table existed, and the answer was
/// hard-coded `None` -- which on a dealing node is the one unconverted
/// crossing ADR 0071 decision 2 exists to make impossible.
///
/// # Keyed by chain, not by declared channel
///
/// A peering resolves from its `[[peers]]` row; a client channel has no
/// equivalent, because the channel that matters most is the one no row
/// names. A settlement backend registers a `ClientChannelSource` (ADR
/// 0052, issue #502), so a node with a `[settlement.<chain>]` table accepts
/// claims on channels it has never been configured for -- and a table built
/// from `[[client_channels]]` rows alone would leave every one of those
/// unresolved, which is the unconverted crossing arriving by the one door
/// the absence rule does not cover.
///
/// So this resolves from the **chain namespace of the channel key**
/// ([`ClientClaim::channel_key`](connector_domain::client_claim::ClientClaim::channel_key)
/// -- `evm:<channel id>`, `solana:<channel account>`), against the
/// `[settlement.<chain>]` table that names exactly one `token_address` per
/// chain. Every chain a claim can be admitted on is a chain this node has a
/// settlement table for -- that is what registers the source, and what
/// `ClientChannelWithoutEvmSettlement`/`ClientChannelWithoutSolanaSettlement`
/// already require of a declared row -- so a resolution keyed that way is
/// total over the channels a dealing node can be paid on. A declared row
/// and a chain-discovered one get the same answer because they are the same
/// channel shape on the same chain, which is the whole reason the key is
/// the right thing to read.
///
/// **Empty is the default and is a whole answer**, as it is for
/// [`PeeringAssets`](crate::PeeringAssets): a node that declares no
/// `[[tokens]]` resolves nothing here, gains no required key and no new
/// refusal, and forwards a client arrival exactly as it did before ADR
/// 0071.
///
/// A pure function of loaded config, read on the forwarding path for every
/// forward, where there is no I/O to do and no chain to ask.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientChannelAssets {
    /// One entry per `[settlement.<chain>]` table, and there are two chains
    /// -- so a linear scan is not a compromise here, it is the whole table.
    chains: Vec<(SettlementChain, AssetId)>,
}

impl ClientChannelAssets {
    /// Whether this node resolved no client channel at all -- `true` for
    /// every node that declares no `[[tokens]]`, which is every config this
    /// repository ships. The question a caller asks before any other, since
    /// `true` means no forward out of a client arrival can be a conversion.
    pub fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }

    /// The declared token a claim on `channel_key` is denominated in, or
    /// `None` when this node resolved none for that key's chain.
    ///
    /// `channel_key` is the chain-namespaced key
    /// [`ClientClaim::channel_key`](connector_domain::client_claim::ClientClaim::channel_key)
    /// produces. The id after the namespace is not read at all and is not
    /// checked against anything: the denomination is a property of the
    /// chain's settlement table, not of which channel on it a buyer holds,
    /// and reading the id would be the difference between a declared
    /// channel and a chain-discovered one that must not exist.
    ///
    /// `None` has two causes and neither is "this channel holds an
    /// undeclared token" -- that is a boot refusal
    /// ([`ConfigError::ClientChannelTokenNotDeclared`]), so it never
    /// reaches a reader. It is either a node that declares no tokens at
    /// all, or a key in a namespace that is not a settlement chain this
    /// node configured, on which no claim of this node's could have been
    /// admitted.
    pub fn asset(&self, channel_key: &str) -> Option<&AssetId> {
        let (namespace, _) = channel_key.split_once(':')?;
        let chain = namespace.parse::<SettlementChain>().ok()?;
        self.chains
            .iter()
            .find(|(resolved, _)| *resolved == chain)
            .map(|(_, asset)| asset)
    }
}

/// Collect `(chain, token)` pairs into a table, in the order given.
///
/// [`Config::load`](crate::Config::load) builds the real one through
/// `resolve_client_channel_assets`, which is where the boot refusal lives;
/// this validates nothing and is for a caller that already holds the
/// resolved pairs -- `connector-runtime`'s forwarding tests, which exercise
/// a client-edge crossing without a whole config file around it. The twin
/// of [`PeeringAssets`](crate::PeeringAssets)'s own `FromIterator`, for the
/// same reason.
impl FromIterator<(SettlementChain, AssetId)> for ClientChannelAssets {
    fn from_iter<I: IntoIterator<Item = (SettlementChain, AssetId)>>(
        chains: I,
    ) -> ClientChannelAssets {
        ClientChannelAssets {
            chains: chains.into_iter().collect(),
        }
    }
}

/// Resolve every chain this node can be paid on at its client edge to the
/// one declared token a channel there holds (ADR 0071 decision 1, issue
/// #1301).
///
/// Every `[settlement.<chain>]` table is resolved, not only the chains
/// `[[client_channels]]` names rows on, and that is the point rather than
/// an over-reach: a settlement table is what registers the
/// `ClientChannelSource` that admits a channel no row names (ADR 0052,
/// issue #502), so on a dealing node every one of those chains can carry an
/// arrival whose denomination a forward has to know. A chain resolved here
/// and a chain left out are the difference between a crossing that converts
/// and one that silently does not.
///
/// Runs only for a node that declares `[[tokens]]` -- the same guard
/// `resolve_peering_assets` opens with, and the same promise: a node that
/// deals nothing resolves nothing, is held to nothing, and reaches the
/// forwarding path with the empty table it had before this function
/// existed.
pub(crate) fn resolve_client_channel_assets(
    settlements: &[SettlementConfig],
    denomination: &DenominationConfig,
) -> Result<ClientChannelAssets, ConfigError> {
    if !denomination.declares_tokens() {
        return Ok(ClientChannelAssets::default());
    }

    let mut chains = Vec::with_capacity(settlements.len());
    for settlement in settlements {
        let chain = settlement.chain();
        let Some(asset) = settlement_asset(settlements, chain) else {
            // Unreachable: `settlement_asset` is looking up the very table
            // it is being asked about.
            continue;
        };
        if denomination.token(&asset).is_none() {
            return Err(ConfigError::ClientChannelTokenNotDeclared { chain, asset });
        }
        chains.push((chain, asset));
    }

    Ok(ClientChannelAssets { chains })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC_BASE: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    const USDC_SOLANA: &str = "solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("an asset")
    }

    fn assets(chains: &[(SettlementChain, &str)]) -> ClientChannelAssets {
        chains
            .iter()
            .map(|(chain, token)| (*chain, asset(token)))
            .collect()
    }

    /// The claim of issue #1301 this whole shape rests on, made
    /// executable: a channel key carries its chain, so the id after the
    /// namespace never has to be recognized. The two keys below are a
    /// declared channel and one this node has never heard of, and they
    /// resolve to the same token because they are on the same chain.
    #[test]
    fn a_channel_key_resolves_by_its_chain_namespace_and_not_by_its_id() {
        let resolved = assets(&[(SettlementChain::Evm, USDC_BASE)]);

        assert_eq!(
            resolved.asset(&format!("evm:0x{}", "ab".repeat(32))),
            Some(&asset(USDC_BASE))
        );
        assert_eq!(
            resolved.asset(&format!("evm:0x{}", "cd".repeat(32))),
            Some(&asset(USDC_BASE))
        );
    }

    /// The two namespaces are separate answers, as the two settlement
    /// tables are separate declarations.
    #[test]
    fn each_chain_answers_with_its_own_settlement_token() {
        let resolved = assets(&[
            (SettlementChain::Evm, USDC_BASE),
            (SettlementChain::Solana, USDC_SOLANA),
        ]);

        assert_eq!(
            resolved.asset(&format!("evm:0x{}", "ab".repeat(32))),
            Some(&asset(USDC_BASE))
        );
        assert_eq!(
            resolved.asset("solana:AiCAdi9xJ5XhJQ3sRkeCYzbWozd7dbS3gwZ9rneJWk8W"),
            Some(&asset(USDC_SOLANA))
        );
    }

    /// A node that declares no tokens, and a key in a namespace no
    /// settlement table covers: both resolve nothing, and nothing is the
    /// pre-ADR-0071 forward.
    #[test]
    fn an_unresolved_key_has_no_token() {
        assert!(ClientChannelAssets::default().is_empty());
        assert_eq!(
            ClientChannelAssets::default().asset(&format!("evm:0x{}", "ab".repeat(32))),
            None
        );

        let evm_only = assets(&[(SettlementChain::Evm, USDC_BASE)]);
        assert_eq!(
            evm_only.asset("solana:AiCAdi9xJ5XhJQ3sRkeCYzbWozd7dbS3gwZ9rneJWk8W"),
            None
        );
        // A peer channel id, which carries no namespace at all -- the
        // journal's other alphabet, and not a client channel key.
        assert_eq!(evm_only.asset(&"ab".repeat(32)), None);
    }
}
