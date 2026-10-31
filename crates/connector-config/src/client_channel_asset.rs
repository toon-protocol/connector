use connector_domain::client_claim::ZK_NOTE_NAMESPACE;
use connector_domain::{AssetChain, AssetId, Network};

use crate::denomination::DenominationConfig;
use crate::error::ConfigError;
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
/// names. A node with a `[settlement.<chain>]` table admits vouchers on
/// channels it has never been configured for (ADR 0052, issue #502) -- and
/// since ADR 0075 (issue #1384) no config row names a client channel at all
/// -- so a table built from declared rows would leave every one of those
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
    /// One entry per settlement table -- each EVM network's and the Solana
    /// one (ADR 0086) -- each asset id naming its own network, so a linear
    /// scan is not a compromise here, it is the whole table.
    assets: Vec<AssetId>,
    /// What an allowance claim's packet is denominated in on a zk edge
    /// (ADR 0080 decision 7, as amended): the vault's asset,
    /// [`ZkNoteConfig::asset`](crate::ZkNoteConfig::asset). `None` on a node
    /// that is no zk edge, or that deals nothing.
    zk_note: Option<AssetId>,
}

impl ClientChannelAssets {
    /// This table, on a zk edge whose vault holds `vault_asset`: an
    /// allowance's key, `zk-note:<allowance id>`, resolves to it, so a
    /// forward out of an allowance-paid packet crosses into the outgoing
    /// peering's token at the operator's declared rate.
    #[must_use]
    pub fn with_zk_edge(mut self, vault_asset: AssetId) -> ClientChannelAssets {
        self.zk_note = Some(vault_asset);
        self
    }

    /// Whether this node resolved no client channel at all -- `true` for
    /// every node that declares no `[[tokens]]`, which is every config this
    /// repository ships. The question a caller asks before any other, since
    /// `true` means no forward out of a client arrival can be a conversion.
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
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
    ///
    /// On a chain where this node holds several networks -- several EVM
    /// tables (ADR 0086), or several XRPL ones (ADR 0087) -- the key alone
    /// does not say which, and this answers `None`: ask [`Self::asset_on`]
    /// with the channel's network.
    ///
    /// On a zk edge, an allowance's key (`zk-note:<allowance id>`) resolves
    /// to the vault's asset (ADR 0080 decision 7, as amended), whatever the
    /// allowance id.
    pub fn asset(&self, channel_key: &str) -> Option<&AssetId> {
        self.asset_on(channel_key, None)
    }

    /// [`Self::asset`], for a channel on `network` when it is known: the
    /// network the channel was verified on picks the table among several
    /// of the key's chain (EVM or XRPL: XRP on one XRPL network is another
    /// asset than XRP on the next). A Solana channel is matched by chain alone, since
    /// a node holds one Solana table and config cannot know its cluster's
    /// genesis prefix. `None` for a network this node resolved nothing on,
    /// and for an unknown network on a chain with several tables.
    pub fn asset_on(&self, channel_key: &str, network: Option<&Network>) -> Option<&AssetId> {
        let (namespace, _) = channel_key.split_once(':')?;
        if namespace == ZK_NOTE_NAMESPACE {
            return self.zk_note.as_ref();
        }
        let chain = namespace.parse::<SettlementChain>().ok()?.asset_chain();
        let on_chain: Vec<&AssetId> = self
            .assets
            .iter()
            .filter(|asset| asset.chain() == chain)
            .collect();
        match (on_chain.as_slice(), chain, network) {
            // One table on the key's chain: it is the channel's, as it
            // always was.
            ([only], _, _) => Some(only),
            (several, AssetChain::Evm | AssetChain::Xrpl, Some(network)) => several
                .iter()
                .find(|asset| asset.network() == network)
                .copied(),
            _ => None,
        }
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
impl FromIterator<AssetId> for ClientChannelAssets {
    fn from_iter<I: IntoIterator<Item = AssetId>>(assets: I) -> ClientChannelAssets {
        ClientChannelAssets {
            assets: assets.into_iter().collect(),
            zk_note: None,
        }
    }
}

/// Resolve every chain this node can be paid on at its client edge to the
/// one declared token a channel there holds (ADR 0071 decision 1, issue
/// #1301).
///
/// Every `[settlement.<chain>]` table is resolved, and that is the point
/// rather than an over-reach: a settlement table is what lets this node
/// admit a voucher on a channel nothing names (ADR 0052, issue #502), so on
/// a dealing node every one of those chains can carry an
/// arrival whose denomination a forward has to know. A chain resolved here
/// and a chain left out are the difference between a crossing that converts
/// and one that silently does not.
///
/// Runs only for a node that declares `[[tokens]]` -- the same guard
/// `resolve_peering_assets` opens with, and the same promise: a node that
/// deals nothing resolves nothing, is held to nothing, and reaches the
/// forwarding path with the empty table it had before this function
/// existed.
///
/// On a zk edge (`zk_vault_asset`), an allowance's key resolves to the
/// vault's asset as well ([`ClientChannelAssets::with_zk_edge`]).
pub(crate) fn resolve_client_channel_assets(
    settlements: &[SettlementConfig],
    denomination: &DenominationConfig,
    zk_vault_asset: Option<AssetId>,
) -> Result<ClientChannelAssets, ConfigError> {
    if !denomination.declares_tokens() {
        return Ok(ClientChannelAssets::default());
    }

    let mut assets = Vec::with_capacity(settlements.len());
    for settlement in settlements {
        // Each table's own token (ADR 0086): two EVM tables are two
        // networks, and each resolves to the declared asset on its own.
        let asset = denomination
            .tokens()
            .iter()
            .map(|token| token.asset())
            .find(|asset| settlement.holds(asset))
            .cloned()
            .ok_or_else(|| ConfigError::ClientChannelTokenNotDeclared {
                chain: settlement.chain(),
                asset: settlement.asset_spelling(),
            })?;
        assets.push(asset);
    }

    let assets = ClientChannelAssets {
        assets,
        zk_note: None,
    };
    Ok(match zk_vault_asset {
        Some(vault_asset) => assets.with_zk_edge(vault_asset),
        None => assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC_BASE: &str = "eip155:8453/erc20:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    const USDC_SOLANA: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("an asset")
    }

    fn assets(chains: &[(SettlementChain, &str)]) -> ClientChannelAssets {
        chains.iter().map(|(_, token)| asset(token)).collect()
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

    /// ADR 0086: on a node with two EVM tables, an `evm:` key alone does not
    /// say which network's token it holds; the network its channel was
    /// verified on does.
    #[test]
    fn two_evm_networks_answer_by_the_channels_network() {
        const USDC_ARBITRUM: &str = "eip155:42161/erc20:0xaf88d065e77c8cc2239327c5edb3a432268e5831";
        let resolved = assets(&[
            (SettlementChain::Evm, USDC_BASE),
            (SettlementChain::Evm, USDC_ARBITRUM),
        ]);
        let key = format!("evm:0x{}", "ab".repeat(32));

        assert_eq!(resolved.asset(&key), None, "never guessed");
        assert_eq!(
            resolved.asset_on(&key, Some(&Network::Eip155(42161))),
            Some(&asset(USDC_ARBITRUM))
        );
        assert_eq!(
            resolved.asset_on(&key, Some(&Network::Eip155(8453))),
            Some(&asset(USDC_BASE))
        );
        assert_eq!(resolved.asset_on(&key, Some(&Network::Eip155(1))), None);
    }

    /// ADR 0087 decision 6.5: two XRPL tables are two networks and two XRP
    /// assets, and the channel's network says which -- the key alone never
    /// does, or a forward between them would read as one asset, which is a
    /// pass-through where a conversion is owed.
    #[test]
    fn two_xrpl_networks_answer_by_the_channels_network() {
        let resolved = assets(&[
            (SettlementChain::Xrpl, "xrpl:1/slip44:144"),
            (SettlementChain::Xrpl, "xrpl:0/slip44:144"),
        ]);
        let key = format!("xrpl:{}", "ab".repeat(32));

        assert_eq!(resolved.asset(&key), None, "never guessed");
        assert_eq!(
            resolved.asset_on(&key, Some(&"xrpl:0".parse().expect("a network"))),
            Some(&asset("xrpl:0/slip44:144"))
        );
        assert_eq!(
            resolved.asset_on(&key, Some(&"xrpl:1".parse().expect("a network"))),
            Some(&asset("xrpl:1/slip44:144"))
        );
        assert_eq!(
            resolved.asset_on(&key, Some(&"xrpl:2".parse().expect("a network"))),
            None
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
