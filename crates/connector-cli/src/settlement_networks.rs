//! The settlement-network bindings (spec #1684, issue #1714): one per chain,
//! each turning that chain's connected batch settlement backend into a
//! [`SettlementNetwork`] handle. Startup builds one per settlement table
//! (ADR 0086), once, and the node finds each network through the one
//! [`connector_runtime::SettlementNetworks`] collection afterwards.
//!
//! Here because ADR 0001 makes `connector-cli` the only place that wires the
//! node: these are the only types that name both the collection's interface
//! and a chain's backend. Adding a chain adds a binding here, its adapter,
//! and its config table.

use std::fmt;
use std::sync::Arc;

use connector_domain::{Network, VoucherSignerFact};
use connector_runtime::SettlementNetwork;
use connector_settlement::batch::{BatchSettlementBackend, BatchSettlementPayer};
use connector_settlement_evm::EvmBatchSettlementBackend;
use connector_settlement_solana::batch::SolanaBatchSettlement;
use connector_settlement_xrpl::XrplBatchSettlement;

/// One `[settlement.eip155.<chainId>]` table's network.
pub(crate) struct EvmSettlementNetwork {
    network: Network,
    backend: Arc<EvmBatchSettlementBackend>,
}

impl EvmSettlementNetwork {
    /// The network the backend proved at connect: the chain id it read off
    /// the chain, which boot has already checked is the table's.
    pub(crate) fn new(backend: Arc<EvmBatchSettlementBackend>) -> EvmSettlementNetwork {
        EvmSettlementNetwork {
            network: Network::Eip155(backend.domain().chain_id),
            backend,
        }
    }
}

impl fmt::Debug for EvmSettlementNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmSettlementNetwork")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl SettlementNetwork for EvmSettlementNetwork {
    fn network(&self) -> &Network {
        &self.network
    }

    fn receiver(&self) -> Arc<dyn BatchSettlementBackend> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementBackend>
    }

    fn payer(&self) -> Arc<dyn BatchSettlementPayer> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementPayer>
    }

    /// ADR 0075 decisions 3 and 10: the settlement key signs this node's
    /// vouchers (`payerAuthorizer == payer`), so the settlement address is
    /// the voucher signer a peer binds this node's channel toward it by.
    fn voucher_signer(&self) -> VoucherSignerFact {
        VoucherSignerFact {
            network: self.network.to_string(),
            signer: format!("{:#x}", self.backend.own_address()),
        }
    }
}

/// The one `[settlement.solana]` table's network.
pub(crate) struct SolanaSettlementNetwork {
    network: Network,
    backend: Arc<SolanaBatchSettlement>,
}

impl SolanaSettlementNetwork {
    /// The network the backend read off its cluster's genesis hash at
    /// connect, never guessed from the RPC URL.
    pub(crate) fn new(backend: Arc<SolanaBatchSettlement>) -> SolanaSettlementNetwork {
        SolanaSettlementNetwork {
            network: backend
                .caip2_network()
                .parse()
                .expect("a connected Solana backend names a CAIP-2 network"),
            backend,
        }
    }
}

impl fmt::Debug for SolanaSettlementNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SolanaSettlementNetwork")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl SettlementNetwork for SolanaSettlementNetwork {
    fn network(&self) -> &Network {
        &self.network
    }

    fn receiver(&self) -> Arc<dyn BatchSettlementBackend> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementBackend>
    }

    fn payer(&self) -> Arc<dyn BatchSettlementPayer> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementPayer>
    }

    /// ADR 0075 decisions 3 and 10: the Solana settlement key is
    /// `authorized_signer` on every channel this node opens, and what a peer
    /// binds that channel by (#1379).
    fn voucher_signer(&self) -> VoucherSignerFact {
        VoucherSignerFact {
            network: self.backend.caip2_network(),
            signer: self.backend.sponsor().to_string(),
        }
    }
}

/// One `[settlement.xrpl.<id>]` table's network (ADR 0087).
pub(crate) struct XrplSettlementNetwork {
    network: Network,
    backend: Arc<XrplBatchSettlement>,
}

impl XrplSettlementNetwork {
    /// `network` is the table's own, which boot checked against the ledger's
    /// `network_id` before binding the backend (ADR 0087 decision 5).
    pub(crate) fn new(
        network: Network,
        backend: Arc<XrplBatchSettlement>,
    ) -> XrplSettlementNetwork {
        XrplSettlementNetwork { network, backend }
    }
}

impl fmt::Debug for XrplSettlementNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XrplSettlementNetwork")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl SettlementNetwork for XrplSettlementNetwork {
    fn network(&self) -> &Network {
        &self.network
    }

    fn receiver(&self) -> Arc<dyn BatchSettlementBackend> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementBackend>
    }

    fn payer(&self) -> Arc<dyn BatchSettlementPayer> {
        Arc::clone(&self.backend) as Arc<dyn BatchSettlementPayer>
    }

    /// ADR 0087 decision 9: the channel key is the settlement account's own
    /// public key -- the one a payer's channel names as its destination is
    /// the address derived from it.
    fn voucher_signer(&self) -> VoucherSignerFact {
        VoucherSignerFact {
            network: self.network.to_string(),
            signer: self.backend.public_key_hex(),
        }
    }
}
