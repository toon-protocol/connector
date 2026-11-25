//! The settlement-network bindings (spec #1684, issue #1714): one per chain,
//! each turning that chain's connected batch settlement backend into a
//! [`SettlementNetwork`] handle. Startup builds one per settlement table
//! (ADR 0086), once, and the node finds each network through the one
//! [`connector_runtime::SettlementNetworks`] collection afterwards: boot
//! restores channels on it, the greeting publishes each network's terms off
//! it, and serving starts each network's watcher through it (issue #1715).
//!
//! Here because ADR 0001 makes `connector-cli` the only place that wires the
//! node: these are the only types that name both the collection's interface
//! and a chain's backend. Adding a chain adds a binding here, its adapter,
//! and its config table.

use std::fmt;
use std::sync::Arc;

use connector_config::{EvmSettlementConfig, XrplSettlementConfig};
use connector_domain::x402::{
    X402BatchSettlementEvmTerms, X402BatchSettlementSolanaTerms, X402BatchSettlementTerms,
    X402BatchSettlementXrplTerms, XRPL_PAYCHAN_BINDING,
};
use connector_domain::{Network, VoucherSignerFact};
use connector_runtime::SettlementNetwork;
use connector_settlement::batch::{BatchSettlementBackend, BatchSettlementPayer, HeldVouchers};
use connector_settlement::watch_interval::watch_interval;
use connector_settlement_evm::{EvmBatchSettlementBackend, EvmBatchWatcher};
use connector_settlement_solana::batch::{SolanaBatchSettlement, SolanaBatchWatcher};
use connector_settlement_xrpl::{XrplBatchSettlement, XrplBatchWatcher};

/// One `[settlement.eip155.<chainId>]` table's network.
pub(crate) struct EvmSettlementNetwork {
    network: Network,
    backend: Arc<EvmBatchSettlementBackend>,
    terms: X402BatchSettlementEvmTerms,
}

impl EvmSettlementNetwork {
    /// The network the backend proved at connect: the chain id it read off
    /// the chain, which boot has already checked is the table's. `table` is
    /// the `[settlement.eip155.<chainId>]` table the backend was bound from.
    pub(crate) fn new(
        backend: Arc<EvmBatchSettlementBackend>,
        table: &EvmSettlementConfig,
    ) -> EvmSettlementNetwork {
        let network = Network::Eip155(backend.domain().chain_id);
        let batch = table.batch_settlement();
        // The connection proved the settlement address and the live chain
        // id; the token comes from the config line `connect` just verified
        // against that chain (issue #564).
        let settlement_address = format!("{:#x}", backend.own_address());
        let terms = X402BatchSettlementEvmTerms {
            network: network.to_string(),
            asset: format!("{:#x}", ethers::types::Address::from(table.token_address())),
            pay_to: settlement_address.clone(),
            receiver_authorizer: settlement_address,
            min_withdraw_delay_secs: batch.min_withdraw_delay_secs(),
            name: batch.asset_eip712_name().to_string(),
            version: batch.asset_eip712_version().to_string(),
            // toon-client#695: configured, like `name` and `version`; this
            // node never calls the facilitator.
            asset_transfer_method: batch.asset_transfer_method(),
            facilitator: batch.facilitator_url().map(str::to_string),
        };
        EvmSettlementNetwork {
            network,
            backend,
            terms,
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

    fn terms(&self) -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Evm(self.terms.clone())
    }

    /// [`EvmBatchWatcher`]: lands the latest voucher on a withdrawing
    /// channel, and sweeps every held voucher into one `claim` and `settle`
    /// every [`connector_settlement_evm::BATCH_SWEEP_INTERVAL`].
    fn start_watcher(&self, held: Arc<dyn HeldVouchers>) {
        let watcher = EvmBatchWatcher::new(Arc::clone(&self.backend), held);
        tokio::spawn(watcher.run(
            watch_interval(self.backend.min_withdraw_delay_secs()),
            connector_settlement_evm::BATCH_SWEEP_INTERVAL,
        ));
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

    /// ADR 0074 decision 8, issue #1345: `payTo` and `feePayer` are both the
    /// sponsor key (decision 5's sponsor-is-the-receiving-operator rule), and
    /// `network` is read off the chain's own genesis hash, never guessed from
    /// the RPC URL. The two minimums are the backend's own: what it admits by
    /// and what its sponsor co-signs above are what is published. Issue #1357
    /// adds x402's required `tokenProgram` and where the sponsor is served.
    fn terms(&self) -> X402BatchSettlementTerms {
        let sponsor = self.backend.sponsor().to_string();
        X402BatchSettlementTerms::Solana(X402BatchSettlementSolanaTerms {
            network: self.backend.caip2_network(),
            asset: self.backend.mint().to_string(),
            pay_to: sponsor.clone(),
            fee_payer: sponsor,
            min_grace_period_secs: self.backend.min_grace_period_secs(),
            token_program: self.backend.token_program().to_string(),
            min_deposit: self.backend.min_sponsored_deposit().to_string(),
            // The path `sponsor::router` mounts, from the one constant, so
            // the greeting can never name a path nothing serves.
            sponsor_endpoint: crate::sponsor::SPONSOR_PATH.to_string(),
        })
    }

    /// [`SolanaBatchWatcher`]: rediscovers every sponsored channel each pass,
    /// sealing, distributing and reclaiming as each one's state asks, and
    /// settles Open ones every
    /// [`connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL`].
    fn start_watcher(&self, held: Arc<dyn HeldVouchers>) {
        let watcher = SolanaBatchWatcher::new(Arc::clone(&self.backend), held);
        tokio::spawn(watcher.run(
            watch_interval(self.backend.min_grace_period_secs()),
            connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL,
        ));
    }
}

/// One `[settlement.xrpl.<id>]` table's network (ADR 0087).
pub(crate) struct XrplSettlementNetwork {
    network: Network,
    backend: Arc<XrplBatchSettlement>,
    terms: X402BatchSettlementXrplTerms,
}

impl XrplSettlementNetwork {
    /// The network is `table`'s own, which boot checked against the ledger's
    /// `network_id` before binding the backend (ADR 0087 decision 5).
    pub(crate) fn new(
        backend: Arc<XrplBatchSettlement>,
        table: &XrplSettlementConfig,
    ) -> XrplSettlementNetwork {
        let network = Network::Xrpl(table.network());
        let terms = X402BatchSettlementXrplTerms {
            network: table.network().caip2().to_string(),
            asset: table.asset().to_string(),
            pay_to: backend.address().to_string(),
            binding: XRPL_PAYCHAN_BINDING.to_string(),
            min_settle_delay_secs: table.min_settle_delay_secs(),
            redemption_safety_secs: table.redemption_safety_secs(),
        };
        XrplSettlementNetwork {
            network,
            backend,
            terms,
        }
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

    fn terms(&self) -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Xrpl(self.terms.clone())
    }

    /// [`XrplBatchWatcher`]: answers a payer's exit by landing the latest
    /// voucher with `tfClose` (ADR 0087 decision 8).
    fn start_watcher(&self, held: Arc<dyn HeldVouchers>) {
        let watcher = XrplBatchWatcher::new(Arc::clone(&self.backend), held);
        tokio::spawn(watcher.run(watch_interval(self.backend.min_settle_delay_secs())));
    }
}
