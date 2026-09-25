use async_trait::async_trait;
use chrono::Duration;
use thiserror::Error;

/// A payment channel's identifier, opaque to everything above this port --
/// assigned by whichever backend opened the channel (a contract address
/// plus a nonce for EVM, a PDA for Solana, an in-process counter for
/// [`crate::InMemorySettlementBackend`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelId(pub String);

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Whether a channel can still be funded, is running its challenge period,
/// or is permanently done.
///
/// `Closed` and `Settled` are deliberately distinct (issue #574): closing a
/// channel starts a challenge period (`settlement_timeout`, given to
/// [`SettlementBackend::open`]) during which [`SettlementBackend::redeem`]
/// still works -- refusing to redeem in that window hands the whole
/// outstanding balance back to whichever party closed the channel, which
/// `TokenNetwork.claimFromChannel` deliberately does not do
/// (`packages/contracts/src/TokenNetwork.sol:262-263`, `:273`). Only
/// `Settled`, reached by a successful [`SettlementBackend::settle`] once
/// that timeout has elapsed, is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Open,
    /// Closed: its challenge period is running (or, once the timeout has
    /// elapsed, is simply unclaimed). `fund` and a second `close` are
    /// refused, but `redeem` still succeeds.
    Closed,
    /// Settled: `settle` has run to completion. Terminal -- no further
    /// `fund` or `redeem` is possible.
    Settled,
}

/// A snapshot of a channel's state, as any [`SettlementBackend`] must be
/// able to report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelState {
    pub id: ChannelId,
    pub counterparty: Vec<u8>,
    pub status: ChannelStatus,
    /// What the *counterparty* has deposited on their own side of the
    /// channel -- the collateral backing claims **this** node can
    /// [`redeem`](SettlementBackend::redeem), and the ceiling every real
    /// chain bounds such a claim by (`TokenNetwork.claimFromChannel`'s
    /// `counterpartyState.deposit < newTransferred`,
    /// `packages/solana-program`'s `TransferredAmountExceedsDeposit`).
    ///
    /// Named for whose side it is, not merely "deposited" (issue #1118):
    /// a payment channel is two-sided on every chain this port settles
    /// on, and reading one side's number as "the channel's balance" is
    /// exactly what made [`SettlementBackend::fund`] mean the wrong thing.
    /// **[`fund`](SettlementBackend::fund) does not move this** -- only
    /// the counterparty, signing for themselves, can.
    pub counterparty_deposited: u128,
    /// What *this* node has deposited on its own side -- the collateral
    /// backing claims this node signs and its counterparty redeems, and
    /// what [`SettlementBackend::fund`] raises (issue #1118). Zero on a
    /// channel this node has never put its own money behind.
    pub own_deposited: u128,
    /// The highest cumulative amount honored by [`SettlementBackend::redeem`] so far.
    pub redeemed: u128,
}

/// A cumulative, superseding claim to a channel's funds (ADR 0004, ADR
/// 0005): `cumulative_amount` is the total ever owed to the redeemer as of
/// this claim, not an increment over the last one. `nonce` is the
/// strictly-increasing counter inside the signed material every chain this
/// port settles on hashes and enforces (`TokenNetwork.claimFromChannel`'s
/// `balanceProof.nonce > counterpartyState.nonce`, the deployed Solana
/// program's per-participant nonce ratchet, issue #573) -- carried through
/// unchanged from `connector_runtime::WireClaim`, whose own `nonce` is also
/// `u64` (a value signed at one width and hashed at another does not
/// recover, so this port settles on the wire's own width rather than
/// widening it the way `cumulative_amount` already widens to a chain's
/// `uint256`). `signature` is whatever proof a backend's chain requires
/// that the channel's counterparty actually signed it -- opaque bytes here
/// since the signature scheme is chain-specific (recoverable ECDSA for EVM,
/// ed25519 for Solana) and this port does not verify it; only the on-chain
/// (or in-memory) settlement logic that a real backend enforces does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub nonce: u64,
    pub cumulative_amount: u128,
    pub signature: Vec<u8>,
}

/// Errors a [`SettlementBackend`] implementation reports. Every variant but
/// [`Backend`] is one the port itself defines the meaning of; [`Backend`]
/// is the one variant a real, chain-backed implementation like
/// `connector-settlement-evm` (issue #459) needs and the in-memory stand-in
/// does not: an I/O-level failure (a reverted transaction the backend's own
/// pre-flight checks did not anticipate, an RPC timeout, a dropped
/// transaction) that is specific to *how* a backend talks to its chain
/// rather than to the port's own channel-lifecycle rules above.
///
/// [`Backend`]: SettlementError::Backend
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SettlementError {
    #[error("channel '{0}' not found")]
    ChannelNotFound(ChannelId),

    /// The channel is `Closed` (its challenge period is running, or has
    /// elapsed but [`SettlementBackend::settle`] has not yet been called)
    /// and the attempted operation requires it still be `Open` -- `fund`,
    /// or a second `close`. Does *not* cover [`SettlementBackend::redeem`],
    /// which still succeeds against a `Closed` channel (issue #574) --
    /// see [`ChannelSettled`] for the error `redeem` does return once the
    /// channel is actually settled.
    ///
    /// [`ChannelSettled`]: SettlementError::ChannelSettled
    #[error("channel '{0}' is already closed")]
    ChannelClosed(ChannelId),

    /// The channel is `Settled` -- [`SettlementBackend::settle`] has run to
    /// completion -- and the attempted operation (`fund`, `redeem`,
    /// `close`, or a second `settle`) requires it not be. Distinct from
    /// [`ChannelClosed`], which still permits `redeem`: nothing is possible
    /// against a settled channel (issue #574).
    ///
    /// [`ChannelClosed`]: SettlementError::ChannelClosed
    #[error("channel '{0}' is already settled")]
    ChannelSettled(ChannelId),

    /// [`SettlementBackend::settle`] was called before its channel's
    /// challenge period -- `settlement_timeout`, given to
    /// [`SettlementBackend::open`], counted from [`SettlementBackend::close`]
    /// -- has elapsed (or before `close` was ever called at all). Named
    /// distinctly rather than folded into [`SettlementError::Backend`], so a
    /// caller can tell "try again once the window has passed" apart from a
    /// genuine I/O failure (issue #574).
    #[error("channel '{0}' is not yet due for settlement")]
    SettlementNotYetDue(ChannelId),

    /// A claim named more than the counterparty has actually deposited on
    /// their own side ([`ChannelState::counterparty_deposited`]) -- the
    /// only side a claim this node redeems is ever drawn from.
    #[error("claim of {requested} exceeds the channel's funded balance of {deposited}")]
    InsufficientChannelBalance { requested: u128, deposited: u128 },

    #[error(
        "claim of {claimed} does not supersede the channel's already-redeemed {already_redeemed}"
    )]
    StaleClaim {
        claimed: u128,
        already_redeemed: u128,
    },

    /// A claim's `nonce` did not strictly exceed the highest one already
    /// redeemed on this channel -- distinct from [`StaleClaim`], which is
    /// about `cumulative_amount`, because the two can diverge: a claim can
    /// name a higher amount than has ever been redeemed while still
    /// carrying a nonce that does not advance (a stale or replayed claim
    /// resent alongside a since-fabricated amount). Real chains enforce
    /// nonce ordering directly (see [`Claim::nonce`]'s own doc); today only
    /// [`crate::InMemorySettlementBackend`] enforces it here too --
    /// `connector-settlement-evm` and `connector-settlement-solana` settle
    /// through contracts with no nonce field of their own yet (issue #566's
    /// retarget), so neither backend can enforce this rule client-side
    /// until that lands.
    ///
    /// [`StaleClaim`]: SettlementError::StaleClaim
    #[error(
        "claim nonce {claimed} does not exceed the channel's already-redeemed nonce {already_redeemed}"
    )]
    StaleNonce { claimed: u64, already_redeemed: u64 },

    /// A claim's `signature` could not be put into the form its chain's
    /// verifier requires (issue #590) -- e.g. an EVM claim whose trailing
    /// recovery-id byte is outside both libsecp256k1's `{0, 1}` and
    /// Ethereum's `{27, 28}` conventions. Named distinctly so a malformed
    /// claim is refused before it is ever submitted, rather than
    /// discovered as a reverted on-chain transaction.
    #[error("claim signature is invalid: {0}")]
    InvalidClaimSignature(String),

    #[error("settlement backend error: {0}")]
    Backend(String),
}

/// The settlement backend port (ADR 0002, ADR 0006): what opening, funding,
/// closing and redeeming a payment channel mean, independent of any chain.
/// `connector-settlement-evm` and `connector-settlement-solana` each hold a
/// real implementation to the contract suite in [`crate::contract`] (issue
/// #459 and its Solana counterpart, ADR 0007);
/// [`crate::InMemorySettlementBackend`] is the first implementation to pass
/// it, and stands in for a real chain in this workspace's own tests until
/// one is wired into `connector-runtime`.
///
/// Every method is asynchronous and fallible because a real implementation
/// talks to a chain over RPC -- opening a channel, for instance, is a
/// submitted transaction awaiting confirmation, not a local computation --
/// matching the precedent `connector-runtime`'s `PeerTransport` port
/// already sets for I/O-bound ports in this workspace.
#[async_trait]
pub trait SettlementBackend: Send + Sync {
    /// Open a new channel to `counterparty`, with `settlement_timeout` as
    /// the withdrawal-safety window a real chain enforces once [`close`]
    /// is called on it. Returns the backend-assigned id of the new
    /// channel, open and unfunded.
    ///
    /// [`close`]: SettlementBackend::close
    async fn open(
        &self,
        counterparty: Vec<u8>,
        settlement_timeout: Duration,
    ) -> Result<ChannelId, SettlementError>;

    /// Deposit `amount` of **this node's own** collateral into `channel`,
    /// raising [`ChannelState::own_deposited`] -- the balance backing
    /// claims this node signs and its counterparty redeems. Returns the
    /// channel's state after the deposit.
    ///
    /// A *self*-deposit, deliberately (issue #1118). Until then this meant
    /// "deposit into the counterparty's side", a delegate deposit only
    /// `TokenNetwork.setTotalDeposit` supports -- it names the participant
    /// to credit separately from the caller whose tokens are pulled --
    /// while `packages/solana-program`'s `Deposit` credits strictly by
    /// signer (`processor.rs:356-360`, `InvalidParticipant` otherwise).
    /// The Solana rule is the correct one: a node paying for its
    /// counterparty's collateral is not a shape production should ever
    /// have, and defining the port around the affordance only one chain
    /// offers left `fund` unconditionally broken on the other. An
    /// implementation whose chain *can* delegate a deposit still must not
    /// do it here; the contract suite reaches that through
    /// [`crate::contract::ContractFixture::fund_counterparty`] instead.
    ///
    /// The amount is the caller's, never a policy of the implementation's
    /// own (ADR 0012): a backend that decides on its own when and how much
    /// to collateralise has grown a treasury.
    async fn fund(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError>;

    /// Raise this node's own deposit in `channel` **to** `own_total`, and
    /// no further: the retry-safe form of [`fund`](SettlementBackend::fund)
    /// (ADR 0073 decision 5).
    ///
    /// `fund` deposits an increment, so running it twice deposits twice.
    /// That is harmless only while every outcome is known, and it is the
    /// one write whose retry after an ambiguous outcome (a timeout, a lost
    /// answer) executes again on both chains: a stale claim, a second
    /// `open` or a second `close` is refused by the chain, while a second
    /// deposit is accepted. A caller that states the total it wants can
    /// repeat the call until it gets an answer. A total at or below the
    /// current [`ChannelState::own_deposited`] deposits nothing and returns
    /// the current state, which is exactly the retry of a call that already
    /// took effect.
    ///
    /// Refuses a channel that is not `Open` exactly as `fund` does.
    ///
    /// Provided in terms of [`channel_state`](SettlementBackend::channel_state)
    /// and `fund`, which is only atomic if nothing else funds the same
    /// channel between the read and the deposit. Every backend in this
    /// workspace overrides it with a version that is: the EVM one hands the
    /// total to `setTotalDeposit` itself, so the chain computes the
    /// difference.
    async fn fund_to(
        &self,
        channel: &ChannelId,
        own_total: u128,
    ) -> Result<ChannelState, SettlementError> {
        let state = self.channel_state(channel).await?;
        match state.status {
            ChannelStatus::Open => {}
            ChannelStatus::Closed => return Err(SettlementError::ChannelClosed(channel.clone())),
            ChannelStatus::Settled => return Err(SettlementError::ChannelSettled(channel.clone())),
        }
        if own_total <= state.own_deposited {
            return Ok(state);
        }
        self.fund(channel, own_total - state.own_deposited).await
    }

    /// Redeem `claim` against `channel`: the redeemer's honored total
    /// becomes `claim.cumulative_amount`, and no more (ADR 0005) -- a claim
    /// that does not supersede the last one redeemed, or exceeds the
    /// channel's funded balance, is rejected rather than silently
    /// truncated or ignored. Returns the channel's state after redemption.
    async fn redeem(
        &self,
        channel: &ChannelId,
        claim: Claim,
    ) -> Result<ChannelState, SettlementError>;

    /// Close `channel`, starting its challenge period (issue #574): no
    /// further funding is possible against it afterward, and it cannot be
    /// closed a second time, but [`redeem`](SettlementBackend::redeem)
    /// still works until [`settle`](SettlementBackend::settle) actually
    /// runs. Returns the channel's state immediately after closing.
    async fn close(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError>;

    /// Settle `channel` once its challenge period -- `settlement_timeout`,
    /// given to [`open`](SettlementBackend::open), counted from
    /// [`close`](SettlementBackend::close) -- has elapsed: pays out its
    /// final remainder and marks it permanently done, after which no
    /// further funding or redemption is possible. Returns
    /// [`SettlementError::SettlementNotYetDue`], not
    /// [`SettlementError::Backend`], if the timeout has not yet elapsed (or
    /// `close` has not yet been called at all).
    ///
    /// Permissionless (issue #574): any caller may invoke this once the
    /// timeout has passed, not only a channel's own participants -- this is
    /// what stops a counterparty stranding a channel's deposit by refusing
    /// to ever settle it, matching `TokenNetwork.settleChannel`'s own
    /// design (`packages/contracts/src/TokenNetwork.sol:366-374`, "Anyone
    /// can call after the grace period"). No implementation of this port
    /// should gate `settle` on the caller being a channel participant.
    async fn settle(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError>;

    /// The current state of `channel`, as last recorded by this backend.
    async fn channel_state(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError>;

    /// **"Do I already have a channel with this counterparty?"** -- ADR
    /// 0059's one question, asked of whichever chain this backend settles
    /// on.
    ///
    /// `Ok(Some(id))` when a live channel exists between this backend's
    /// own on-chain identity and `counterparty`; `Ok(None)` when the pair
    /// has none and [`open`](SettlementBackend::open) is what to do next.
    /// `Err` is reserved for a lookup that genuinely failed, so "there is
    /// no channel" is never confused with "I could not find out" -- a
    /// caller that opens on the second would spend gas on a duplicate.
    ///
    /// **Live is `Open` or `Closed`.** A closed channel is still inside
    /// its challenge window, still holds collateral and still occupies the
    /// pair's identifier, so reporting it absent hands the caller an
    /// `open` that fails. Only once it has `Settled` does the pair report
    /// none again, which is what lets two parties start a fresh channel
    /// after finishing one (`CONTEXT.md`, **Payment channel**).
    ///
    /// The answer must come from the chain's own current state rather than
    /// from anything this process has observed and remembered: an index
    /// replayed from a starting block cannot see a channel opened before
    /// it, and "none exists" out of a half-built index looks exactly like
    /// the true answer while being the expensive wrong one (ADR 0059
    /// rejects a local participant index for this reason).
    ///
    /// `counterparty` is the same identity [`open`](SettlementBackend::open)
    /// takes -- **the settlement address of this backend's own chain**, 20
    /// bytes on EVM and a 32-byte ed25519 public key on Solana, never a
    /// node's edge identity. An identity this backend cannot parse is
    /// [`SettlementError::Backend`], the same refusal `open` gives it.
    async fn live_channel_with(
        &self,
        counterparty: Vec<u8>,
    ) -> Result<Option<ChannelId>, SettlementError>;
}
