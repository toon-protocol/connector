//! The **settlement networks** this node settles on, as one collection keyed
//! by CAIP-2 network (spec #1684, issue #1714).
//!
//! A settlement network is one network the node settles on, built from one
//! settlement table (ADR 0086), and the unit a chain adapter plugs in at: a
//! **binding** implements [`SettlementNetwork`] over that chain's batch
//! settlement port, and `connector-cli` builds one per table at startup (ADR
//! 0001 keeps construction there). Callers look a network up in
//! [`SettlementNetworks`] and use the handle it returns; none of them
//! matches on the chain, and cardinality -- one Solana network, any number
//! of EVM and XRPL networks -- is decided by the config and the bindings,
//! never at a call site.
//!
//! Here, beside the ports the runtime already holds, because both the client
//! edge and `connector-cli` depend on this crate and the batch port's own
//! crate knows nothing of CAIP-2 networks.

use std::fmt;
use std::sync::Arc;

use connector_domain::x402::X402BatchSettlementTerms;
use connector_domain::{Network, VoucherSignerFact};
use connector_settlement::batch::{BatchSettlementBackend, BatchSettlementPayer, HeldVouchers};
use thiserror::Error;

/// The carriage a voucher arrives over: the client edge, or a peer carriage
/// (BTP or ILP-over-HTTP from a proven peer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherCarriage {
    ClientEdge,
    Peer,
}

/// Which carriages accept a settlement network's vouchers. Every chain this
/// node settles on today answers [`VoucherCarriages::Every`]; ADR 0089's Mina
/// binding will answer [`VoucherCarriages::PeerOnly`], so "Mina only on a peer
/// carriage" is one fact on that binding rather than a match at the client
/// edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherCarriages {
    Every,
    PeerOnly,
}

impl VoucherCarriages {
    /// Whether a voucher arriving over `carriage` is accepted.
    pub fn accept(self, carriage: VoucherCarriage) -> bool {
        match self {
            VoucherCarriages::Every => true,
            VoucherCarriages::PeerOnly => carriage == VoucherCarriage::Peer,
        }
    }
}

/// One settlement network's handle: everything chain-specific about the
/// network, behind one interface. Implemented once per chain, by that
/// chain's binding.
///
/// The batch port's two halves are how a caller presents a channel to the
/// network and reads its state ([`BatchSettlementBackend::admit`],
/// [`BatchSettlementBackend::restore`],
/// [`BatchSettlementBackend::channel_state`]); a channel's presentation is
/// the port's [`connector_settlement::batch::ChannelPresentation`], whose
/// spelling the chain fixes.
pub trait SettlementNetwork: Send + Sync + fmt::Debug {
    /// The CAIP-2 network this handle settles on, exactly as its greeting
    /// publishes it: the collection's key.
    fn network(&self) -> &Network;

    /// The batch port's receiving half on this network: admits, reads and
    /// lands the channels payers open toward this node.
    fn receiver(&self) -> Arc<dyn BatchSettlementBackend>;

    /// The batch port's paying half on this network: opens, funds and signs
    /// on this node's own channels.
    fn payer(&self) -> Arc<dyn BatchSettlementPayer>;

    /// The key this node signs its outbound vouchers with on this network,
    /// published so a peer can bind this node's channel toward it (ADR 0075
    /// decisions 3 and 10).
    fn voucher_signer(&self) -> VoucherSignerFact;

    /// The carriages that accept this network's vouchers.
    fn carriages(&self) -> VoucherCarriages {
        VoucherCarriages::Every
    }

    /// The batch-settlement terms this network's entry in the greeting's
    /// `accepts[]` publishes (ADR 0074 decision 8): proved at connect, never
    /// re-declared. A wire shape, so it stays a per-chain enum; only who
    /// assembles it moved behind the handle (spec #1684).
    fn terms(&self) -> X402BatchSettlementTerms;

    /// Start this network's watcher in the background, handed `held`: the
    /// vouchers held on this network's channels and no other's (ADR 0086).
    /// The watcher keeps its own run loop; only starting it is the handle's.
    /// Called once, from inside a Tokio runtime, when the node starts
    /// serving.
    fn start_watcher(&self, held: Arc<dyn HeldVouchers>);
}

/// Two handles for one network: a node settles on a network through one
/// settlement table, so a second is a wiring fault, never a choice.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("this node already settles on {0}: one settlement table per network (ADR 0086)")]
pub struct NetworkAlreadySettled(pub Network);

/// Every settlement network this node settles on, keyed by CAIP-2 network,
/// in the order they were added (table order). Empty on a node that settles
/// on nothing, which then accepts no voucher.
#[derive(Clone, Default)]
pub struct SettlementNetworks {
    networks: Vec<Arc<dyn SettlementNetwork>>,
}

impl SettlementNetworks {
    /// A node that settles on nothing.
    pub fn new() -> SettlementNetworks {
        SettlementNetworks::default()
    }

    /// Add `network`'s handle.
    ///
    /// # Errors
    ///
    /// [`NetworkAlreadySettled`] when the collection already holds a handle
    /// for the same network.
    pub fn insert(
        &mut self,
        network: Arc<dyn SettlementNetwork>,
    ) -> Result<(), NetworkAlreadySettled> {
        if self.settles_on(network.network()) {
            return Err(NetworkAlreadySettled(network.network().clone()));
        }
        self.networks.push(network);
        Ok(())
    }

    /// [`Self::insert`], by value: for building a collection in one
    /// expression.
    ///
    /// # Errors
    ///
    /// As [`Self::insert`].
    pub fn with(
        mut self,
        network: Arc<dyn SettlementNetwork>,
    ) -> Result<SettlementNetworks, NetworkAlreadySettled> {
        self.insert(network)?;
        Ok(self)
    }

    /// `network`'s handle, if this node settles on it.
    pub fn get(&self, network: &Network) -> Option<&Arc<dyn SettlementNetwork>> {
        self.networks.iter().find(|held| held.network() == network)
    }

    /// Whether this node settles on `network`: whether a settlement table
    /// names it (ADR 0086 decision 1). A voucher, claim-state entry or
    /// peer-role challenge naming any other network is refused by name.
    pub fn settles_on(&self, network: &Network) -> bool {
        self.get(network).is_some()
    }

    /// Every network, in the order added.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn SettlementNetwork>> {
        self.networks.iter()
    }

    /// Whether this node settles on no network at all.
    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    /// Every network this node settles on, in table order.
    pub fn networks(&self) -> Vec<Network> {
        self.iter().map(|held| held.network().clone()).collect()
    }

    /// The greeting's whole `accepts[]`: each network's terms, in table order
    /// (ADR 0075 decision 10).
    pub fn terms(&self) -> Vec<X402BatchSettlementTerms> {
        self.iter().map(|held| held.terms()).collect()
    }

    /// Each network's outbound voucher signer, in table order: what the
    /// self-description publishes so a peer can bind this node's channels.
    pub fn voucher_signers(&self) -> Vec<VoucherSignerFact> {
        self.iter().map(|held| held.voucher_signer()).collect()
    }

    /// Each network's receiving half, keyed by its network.
    pub fn receivers(&self) -> Vec<(Network, Arc<dyn BatchSettlementBackend>)> {
        self.iter()
            .map(|held| (held.network().clone(), held.receiver()))
            .collect()
    }

    /// Each network's paying half, keyed by its network.
    pub fn payers(&self) -> Vec<(Network, Arc<dyn BatchSettlementPayer>)> {
        self.iter()
            .map(|held| (held.network().clone(), held.payer()))
            .collect()
    }
}

impl fmt::Debug for SettlementNetworks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.networks.iter().map(|held| held.network()))
            .finish()
    }
}

/// The in-memory batch settlement bound as a settlement network: the binding
/// tests register in a [`SettlementNetworks`] wherever a node needs a network
/// without a chain. [`connector_settlement::batch::InMemoryBatchSettlement`]
/// upholds both of the port's contract suites, so a test over it is a test
/// over something the real adapters are held to (ADR 0007).
#[cfg(any(test, feature = "test-support"))]
pub struct InMemorySettlementNetwork {
    network: Network,
    settlement: Arc<connector_settlement::batch::InMemoryBatchSettlement>,
    voucher_signer: String,
    carriages: VoucherCarriages,
    terms: X402BatchSettlementTerms,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemorySettlementNetwork {
    /// `settlement`, settling on `network`, signing its outbound vouchers as
    /// `voucher_signer`.
    pub fn new(
        network: Network,
        settlement: Arc<connector_settlement::batch::InMemoryBatchSettlement>,
        voucher_signer: impl Into<String>,
    ) -> InMemorySettlementNetwork {
        let voucher_signer = voucher_signer.into();
        InMemorySettlementNetwork {
            terms: placeholder_terms(&network, &voucher_signer),
            network,
            settlement,
            voucher_signer,
            carriages: VoucherCarriages::Every,
        }
    }

    /// Publish `terms` in the greeting instead of the placeholder ones a new
    /// in-memory network names.
    pub fn with_terms(mut self, terms: X402BatchSettlementTerms) -> InMemorySettlementNetwork {
        self.terms = terms;
        self
    }

    /// A network on a fresh in-memory chain shaped like `network`'s -- an
    /// EVM, Solana or XRPL chain -- admitting channels whose delay is at
    /// least a day.
    ///
    /// # Panics
    ///
    /// On a network of a chain the in-memory settlement has no shape for
    /// (Mina).
    pub fn fresh(network: Network) -> InMemorySettlementNetwork {
        use connector_domain::AssetChain;
        use connector_settlement::batch::{InMemoryBatchChain, InMemoryBatchSettlement, PayerExit};
        let chain = match network.chain() {
            AssetChain::Evm => InMemoryBatchChain::new(PayerExit::Withdrawal),
            AssetChain::Solana => InMemoryBatchChain::new(PayerExit::Close),
            AssetChain::Xrpl => InMemoryBatchChain::xrpl(),
            AssetChain::Mina => panic!("the in-memory settlement has no Mina shape"),
        };
        let settlement = Arc::new(InMemoryBatchSettlement::on(chain, 0xaa, 86_400));
        InMemorySettlementNetwork::new(network, settlement, "in-memory-node")
    }

    /// Stand in for a client that opened, toward this node, the EVM channel
    /// `config` names on this network -- its id hashed under this network's
    /// `x402BatchSettlement` domain, its voucher signer `payerAuthorizer` --
    /// with `deposit` behind it.
    ///
    /// # Panics
    ///
    /// On a network that is not an EVM one.
    pub fn open_evm(&self, config: &connector_signer::BatchChannelConfig, deposit: u128) {
        use connector_settlement::batch::{ChannelPresentation, EvmChannelConfig, VoucherSigner};
        let chain_id = self
            .network
            .eip155_chain_id()
            .expect("an EVM channel opens on an EVM network");
        let id = connector_signer::evm_batch_channel_id(
            &connector_signer::BatchSettlementDomain::x402(chain_id),
            config,
        );
        self.settlement.client_opened(
            &ChannelPresentation::Evm {
                channel: connector_settlement::ChannelId(format!("0x{}", hex(&id))),
                config: EvmChannelConfig {
                    payer: config.payer,
                    payer_authorizer: config.payer_authorizer,
                    receiver: config.receiver,
                    receiver_authorizer: config.receiver_authorizer,
                    token: config.token,
                    withdraw_delay: config.withdraw_delay,
                    salt: config.salt,
                },
            },
            VoucherSigner::Evm(config.payer_authorizer),
            deposit,
        );
    }

    /// Stand in for a client that opened the Solana channel account
    /// `account` toward this node, `authorized_signer` signing its vouchers.
    pub fn open_solana(&self, account: [u8; 32], authorized_signer: [u8; 32], deposit: u128) {
        use connector_settlement::batch::{ChannelPresentation, VoucherSigner};
        self.settlement.client_opened(
            &ChannelPresentation::Solana {
                channel: connector_settlement::ChannelId(bs58::encode(account).into_string()),
            },
            VoucherSigner::Solana(authorized_signer),
            deposit,
        );
    }

    /// Stand in for a client that created the XRPL `PayChannel` `channel_id`
    /// toward this node under `public_key`.
    pub fn open_xrpl(&self, channel_id: [u8; 32], public_key: [u8; 33], deposit: u128) {
        use connector_settlement::batch::{ChannelPresentation, VoucherSigner};
        self.settlement.client_opened(
            &ChannelPresentation::Xrpl {
                channel: connector_settlement::ChannelId(hex(&channel_id)),
            },
            VoucherSigner::Xrpl(public_key),
            deposit,
        );
    }

    /// Accept this network's vouchers only over `carriages`.
    pub fn with_carriages(mut self, carriages: VoucherCarriages) -> InMemorySettlementNetwork {
        self.carriages = carriages;
        self
    }

    /// The fake under this network, for a test to open channels on.
    pub fn settlement(&self) -> &Arc<connector_settlement::batch::InMemoryBatchSettlement> {
        &self.settlement
    }
}

/// Terms naming `network`, paid to `voucher_signer`, in the shape of
/// `network`'s chain: what an in-memory network publishes until a test gives
/// it real ones ([`InMemorySettlementNetwork::with_terms`]).
#[cfg(any(test, feature = "test-support"))]
fn placeholder_terms(network: &Network, voucher_signer: &str) -> X402BatchSettlementTerms {
    use connector_domain::x402::{
        X402BatchSettlementEvmTerms, X402BatchSettlementSolanaTerms, X402BatchSettlementXrplTerms,
    };
    use connector_domain::AssetChain;
    match network.chain() {
        AssetChain::Solana => X402BatchSettlementTerms::Solana(X402BatchSettlementSolanaTerms {
            network: network.to_string(),
            asset: "in-memory-asset".to_string(),
            pay_to: voucher_signer.to_string(),
            fee_payer: voucher_signer.to_string(),
            min_grace_period_secs: 86_400,
            token_program: "in-memory-token-program".to_string(),
            min_deposit: "0".to_string(),
            sponsor_endpoint: "/ilp/sponsor".to_string(),
        }),
        AssetChain::Xrpl => X402BatchSettlementTerms::Xrpl(X402BatchSettlementXrplTerms {
            network: network.to_string(),
            asset: "XRP".to_string(),
            pay_to: voucher_signer.to_string(),
            binding: connector_domain::x402::XRPL_PAYCHAN_BINDING.to_string(),
            min_settle_delay_secs: 86_400,
            redemption_safety_secs: 3_600,
        }),
        AssetChain::Evm | AssetChain::Mina => {
            X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
                network: network.to_string(),
                asset: "in-memory-asset".to_string(),
                pay_to: voucher_signer.to_string(),
                receiver_authorizer: voucher_signer.to_string(),
                min_withdraw_delay_secs: 86_400,
                name: "In-Memory Token".to_string(),
                version: "1".to_string(),
                asset_transfer_method: Default::default(),
                facilitator: None,
            })
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(any(test, feature = "test-support"))]
impl fmt::Debug for InMemorySettlementNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemorySettlementNetwork")
            .field("network", &self.network)
            .field("carriages", &self.carriages)
            .finish_non_exhaustive()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SettlementNetwork for InMemorySettlementNetwork {
    fn network(&self) -> &Network {
        &self.network
    }

    fn receiver(&self) -> Arc<dyn BatchSettlementBackend> {
        Arc::clone(&self.settlement) as Arc<dyn BatchSettlementBackend>
    }

    fn payer(&self) -> Arc<dyn BatchSettlementPayer> {
        Arc::clone(&self.settlement) as Arc<dyn BatchSettlementPayer>
    }

    fn voucher_signer(&self) -> VoucherSignerFact {
        VoucherSignerFact {
            network: self.network.to_string(),
            signer: self.voucher_signer.clone(),
        }
    }

    fn carriages(&self) -> VoucherCarriages {
        self.carriages
    }

    fn terms(&self) -> X402BatchSettlementTerms {
        self.terms.clone()
    }

    /// The in-memory chain moves only when a test moves it: there is nothing
    /// to watch.
    fn start_watcher(&self, _held: Arc<dyn HeldVouchers>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_settlement::batch::{InMemoryBatchSettlement, PayerExit};

    fn network(caip2: &str) -> Network {
        caip2.parse().expect("a CAIP-2 network")
    }

    fn in_memory(caip2: &str, exit: PayerExit) -> Arc<dyn SettlementNetwork> {
        Arc::new(InMemorySettlementNetwork::new(
            network(caip2),
            Arc::new(InMemoryBatchSettlement::new(exit, 86_400)),
            format!("signer-on-{caip2}"),
        ))
    }

    fn two_evm_one_solana() -> SettlementNetworks {
        SettlementNetworks::new()
            .with(in_memory("eip155:8453", PayerExit::Withdrawal))
            .and_then(|networks| networks.with(in_memory("eip155:84532", PayerExit::Withdrawal)))
            .and_then(|networks| {
                networks.with(in_memory(
                    "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
                    PayerExit::Close,
                ))
            })
            .expect("three distinct networks")
    }

    /// A network is found by its CAIP-2 network and no other: two EVM
    /// networks are two entries, never one chain's.
    #[test]
    fn each_network_is_found_by_its_own_caip2_network() {
        let networks = two_evm_one_solana();
        for caip2 in [
            "eip155:8453",
            "eip155:84532",
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        ] {
            let held = networks.get(&network(caip2)).expect("settled");
            assert_eq!(held.network(), &network(caip2));
            assert_eq!(held.voucher_signer().signer, format!("signer-on-{caip2}"));
            assert_eq!(held.voucher_signer().network, caip2);
        }
    }

    /// ADR 0086 decision 1: a network no table names is not settled on, on
    /// a chain the node settles on or not.
    #[test]
    fn a_network_no_table_names_is_not_settled_on() {
        let networks = two_evm_one_solana();
        for caip2 in [
            "eip155:1",
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "xrpl:1",
            "mina:devnet",
        ] {
            assert!(!networks.settles_on(&network(caip2)), "{caip2}");
            assert!(networks.get(&network(caip2)).is_none(), "{caip2}");
        }
        assert!(!SettlementNetworks::new().settles_on(&network("eip155:8453")));
    }

    #[test]
    fn every_network_is_iterated_in_the_order_added() {
        let order: Vec<String> = two_evm_one_solana()
            .iter()
            .map(|held| held.network().to_string())
            .collect();
        assert_eq!(
            order,
            [
                "eip155:8453",
                "eip155:84532",
                "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
            ]
        );
    }

    /// The greeting's `accepts[]` is each network's own terms, in table
    /// order: assembly names no chain.
    #[test]
    fn greeting_terms_are_each_networks_own_in_table_order() {
        use connector_domain::x402::{
            X402AssetTransferMethod, X402BatchSettlementEvmTerms, X402BatchSettlementTerms,
            X402BatchSettlementXrplTerms,
        };
        let base = X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: "eip155:8453".to_string(),
            asset: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
            pay_to: "0x00000000000000000000000000000000000000aa".to_string(),
            receiver_authorizer: "0x00000000000000000000000000000000000000aa".to_string(),
            min_withdraw_delay_secs: 86_400,
            name: "USD Coin".to_string(),
            version: "2".to_string(),
            asset_transfer_method: X402AssetTransferMethod::default(),
            facilitator: None,
        });
        let xrpl = X402BatchSettlementTerms::Xrpl(X402BatchSettlementXrplTerms {
            network: "xrpl:1".to_string(),
            asset: "XRP".to_string(),
            pay_to: "rPT1Sjq2YGrBMTttX4GZHjKu9dyfzbpAYe".to_string(),
            binding: "paychan".to_string(),
            min_settle_delay_secs: 86_400,
            redemption_safety_secs: 3_600,
        });
        let networks = SettlementNetworks::new()
            .with(Arc::new(
                InMemorySettlementNetwork::fresh(network("xrpl:1")).with_terms(xrpl.clone()),
            ))
            .and_then(|networks| {
                networks.with(Arc::new(
                    InMemorySettlementNetwork::fresh(network("eip155:8453"))
                        .with_terms(base.clone()),
                ))
            })
            .expect("two distinct networks");
        assert_eq!(networks.terms(), vec![xrpl, base]);
        assert!(SettlementNetworks::new().terms().is_empty());
    }

    /// The receiving and paying halves, and the voucher signers, are each
    /// network's own, keyed by the network that published them.
    #[test]
    fn each_networks_halves_and_signer_are_keyed_by_its_network() {
        let networks = two_evm_one_solana();
        let order = [
            "eip155:8453",
            "eip155:84532",
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        ];
        let receivers: Vec<String> = networks
            .receivers()
            .into_iter()
            .map(|(held, _)| held.to_string())
            .collect();
        let payers: Vec<String> = networks
            .payers()
            .into_iter()
            .map(|(held, _)| held.to_string())
            .collect();
        let signers: Vec<(String, String)> = networks
            .voucher_signers()
            .into_iter()
            .map(|fact| (fact.network, fact.signer))
            .collect();
        assert_eq!(receivers, order);
        assert_eq!(payers, order);
        assert_eq!(
            signers,
            order
                .iter()
                .map(|caip2| (caip2.to_string(), format!("signer-on-{caip2}")))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            networks.networks(),
            order.iter().map(|caip2| network(caip2)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_second_handle_for_one_network_is_refused_by_name() {
        let refused = two_evm_one_solana()
            .with(in_memory("eip155:84532", PayerExit::Withdrawal))
            .unwrap_err();
        assert_eq!(refused, NetworkAlreadySettled(network("eip155:84532")));
    }

    /// Every existing chain accepts its vouchers on every carriage; a binding
    /// that says otherwise (ADR 0089's Mina) is accepted on a peer carriage
    /// alone.
    #[test]
    fn carriages_accept_what_the_binding_says() {
        let every = two_evm_one_solana();
        for held in every.iter() {
            assert!(held.carriages().accept(VoucherCarriage::ClientEdge));
            assert!(held.carriages().accept(VoucherCarriage::Peer));
        }
        let peer_only = VoucherCarriages::PeerOnly;
        assert!(peer_only.accept(VoucherCarriage::Peer));
        assert!(!peer_only.accept(VoucherCarriage::ClientEdge));
    }
}
