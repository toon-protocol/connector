//! What the client edge's claim gate needs from an x402 `batch-settlement`
//! backend to admit a **voucher** (ADR 0074, issue #1341): the seam between
//! the gate, which owns freshness, signature verification and the journal,
//! and the receive-only settlement port (#1340) whose EVM and Solana
//! implementations (#1342, #1343) own admission and collateral.
//!
//! # The seam
//!
//! **Whether a node accepts vouchers at all is whether its gate was given one
//! of these** ([`crate::ClientClaimGate::with_batch_settlement`]). A gate
//! without one refuses every voucher by name
//! ([`crate::ClaimIngestRejection::BatchSettlementNotAccepted`]) -- ADR 0074
//! decision 1's "off unless configured". Nothing in the runtime passes one
//! yet: the config that opts a chain in and the backends that answer these
//! questions are #1340/#1342/#1343, and wiring them is a
//! `with_batch_settlement` call in `connector-cli`'s runtime once they exist.
//!
//! # What an implementation owes
//!
//! Only channels it would **admit** (ADR 0074 decisions 2 and 5): on EVM,
//! `receiver` and `receiverAuthorizer` both this connector's settlement
//! address, a token it settles in, a `withdrawDelay` at or above its
//! published minimum, and no zero `payerAuthorizer` over a contract-wallet
//! `payer`; on Solana, a channel account whose PDA re-derives from its own
//! seeds, Open, with `payee` and `rent_payer` this connector's sponsor key,
//! a mint it settles in, a one-recipient `distribution_hash` and a
//! `grace_period` at or above its minimum. Anything else is `Ok(None)`,
//! which the gate reports as an unknown channel.
//!
//! The gate takes the signer from what this returns and nothing else: the
//! verified `ChannelConfig` on EVM, `authorized_signer` on Solana (ADR 0074
//! decision 4). And it takes `max_cumulative` as the collateral bound
//! without caching it, because on EVM that figure can fall (decision 5):
//! keeping it current -- dropping a cache on `WithdrawInitiated` -- is the
//! implementation's job.

use async_trait::async_trait;
use connector_signer::{BatchChannelConfig, BatchSettlementDomain};

use crate::channels::ChannelResolutionError;

/// An EVM batch-settlement channel this connector admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedEvmVoucherChannel {
    /// The channel's `ChannelConfig`. The gate re-hashes it and refuses a
    /// config that does not hash to the channel the voucher names, so an
    /// implementation that got this wrong fails closed.
    pub config: BatchChannelConfig,
    /// The highest cumulative amount a voucher on this channel may name and
    /// still be claimable -- the backend's current reading of the channel's
    /// escrow, net of any pending withdrawal (ADR 0074 decision 5).
    pub max_cumulative: u64,
}

/// A Solana batch-settlement channel this connector admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedSolanaVoucherChannel {
    /// The channel account's `authorized_signer`, fixed at open.
    pub authorized_signer: [u8; 32],
    /// As [`AdmittedEvmVoucherChannel::max_cumulative`]: on Solana, the
    /// channel's `deposit`, which only `top_up` moves while it is Open.
    pub max_cumulative: u64,
}

/// A batch-settlement backend, as the claim gate sees it. See this module's
/// doc for the seam it is and what an implementation owes.
#[async_trait]
pub trait BatchSettlementChannels: Send + Sync + std::fmt::Debug {
    /// The EIP-712 domain EVM vouchers are verified under -- this node's
    /// settlement chain and `x402BatchSettlement`'s address -- or `None` if
    /// this node has not opted in to EVM vouchers.
    fn evm_domain(&self) -> Option<BatchSettlementDomain>;

    /// Whether this node has opted in to Solana vouchers.
    fn accepts_solana(&self) -> bool;

    /// The EVM channel `channel_id`, if this connector admits it.
    /// `presented_config` is the client's `ChannelConfig` when the voucher
    /// carried one, already checked by the gate to hash to `channel_id`; a
    /// channel's first voucher carries one, and an implementation that has
    /// never seen the channel and is given `None` has nothing to admit.
    async fn evm(
        &self,
        channel_id: &[u8; 32],
        presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError>;

    /// The Solana channel account `channel_account`, if this connector
    /// admits it.
    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError>;
}
