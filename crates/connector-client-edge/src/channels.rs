//! Per-channel counterparty registry for the client edge (issues #558,
//! #556): which key this connector accepts a claim's signature from, for
//! each channel it has a record of.
//!
//! This is what turns `client-edge-spec.md` §1.3 step 4 from a
//! self-referential check into a real one. A claim carries its own
//! `signerAddress`/`signerPublicKey`, but a forger can put anything there
//! -- signing correctly with a freshly generated key and declaring
//! themself the payer costs nothing. The only party whose signature means
//! anything on a channel is that channel's counterparty, and a
//! counterparty is a property of the channel, not of the claim. So it is
//! recorded here, keyed by the channel, and a claim gets no say in it:
//! [`crate::ClientClaimGate`] reads the signer -- and, for EVM, the EIP-712
//! domain the digest is computed under (ADR 0024) -- out of this registry
//! and never out of the claim.
//!
//! Deliberately the same shape the peer semantics already settled on:
//! `connector_runtime::ClaimBook` keeps a `channel_id -> Address` map plus
//! a per-channel `ChannelDomain` for exactly this reason (issue #575), and
//! refuses a claim naming a channel it has no record of as
//! `ClaimRejectReason::UnknownChannel`. This is that rule at the other
//! edge, over the client edge's own claim shapes, since a client-edge
//! claim's channel is never a peer channel.
//!
//! # Where a record comes from
//!
//! Two sources, and they compose rather than replace each other:
//!
//! 1. **Declared** -- [`ClientChannelRegistry::record_evm`] /
//!    [`record_solana`](ClientChannelRegistry::record_solana), which
//!    `connector-cli` fills from the `[[client_channels]]` config section.
//!    A declared channel is authoritative: it is answered from memory,
//!    never resolved, and needs **no chain connection** -- this registry
//!    holds no reference to a settlement backend and never asks one
//!    anything about a declared row.
//!
//!    That is a fact about this registry, not a licence for the node
//!    around it. Since issue #1138 a `[[client_channels]]` row is refused
//!    at config load unless this node declares the `[settlement.<chain>]`
//!    table of the row's own chain -- because that table is where the
//!    node's on-chain identity comes from, and without it the node is not
//!    a participant of the channel and could never redeem a claim on it,
//!    however correctly declared. See
//!    `connector_config`'s `SettlementTables` and
//!    `docs/protocol/peer-carriage-spec.md` §11.1.
//! 2. **Resolved from chain** -- a [`ClientChannelSource`] registered per
//!    chain ([`ClientChannelRegistry::with_source`] for EVM,
//!    [`ClientChannelRegistry::with_solana_source`] for Solana), asked only
//!    about a channel nothing was declared for. `connector-cli` builds one
//!    over the `[settlement.evm]` section's own `TokenNetwork` (issue
//!    #611) and one over the `[settlement.solana]` section's own deployed
//!    payment-channel program (issue #631), so a client that has opened a
//!    channel with this connector on chain can pay without the operator
//!    hand-editing config and restarting. The source is keyed by chain
//!    (issue #629), so an EVM source is never consulted for a Solana
//!    lookup or vice versa.
//!
//! The second is what makes issue #502's *"anonymity is a first-class
//! path, not a fallback: it is how an unaffiliated buyer pays for a
//! terminated route without registering with the operator first"* true
//! rather than aspirational. An unaffiliated buyer registers with the
//! *chain* -- a public fact this connector can read for itself -- instead
//! of with the operator.
//!
//! **Nothing falls back to the claim's own self-declared signer.** A
//! registry with neither a record nor a source refuses every claim
//! ([`crate::ClaimIngestRejection::UnknownChannel`]); a source that cannot
//! answer -- an unreachable RPC endpoint, say -- refuses the claim it was
//! asked about ([`crate::ClaimIngestRejection::ChannelLookupFailed`]),
//! distinguishably and never silently. "Unverifiable" is never "accepted",
//! by configuration, flag or build profile.
//!
//! # What a resolution reports, and what a claim may spend against
//!
//! A record is not only "whose signature", it is also **how much that
//! signature can be good for** (issue #646). A resolved channel carries
//! the counterparty's on-chain deposit as a [`DepositFloor`], and the claim
//! gate refuses a claim whose cumulative amount exceeds it. That is not a
//! credit policy this connector invents: both settlement contracts already
//! refuse an over-deposit claim at redemption
//! (`TokenNetwork.sol`'s `InsufficientChannelBalance`,
//! `packages/solana-program/src/processor.rs`'s
//! `TransferredAmountExceedsDeposit`), so a claim above the deposit is not
//! value at risk -- it is work this connector could never be paid for, and
//! accepting it is giving the app away. Evaluating it here makes the
//! accept rule agree with the redeem rule.
//!
//! A **declared** channel has no deposit to report: `[[client_channels]]`
//! names a counterparty and a domain and never an amount, and a node with
//! no settlement backend has no chain to ask. Its floor is
//! [`DepositFloor::Unknown`], which covers everything -- the deliberate
//! exemption of issue #646, and the ADR 0006 split done properly: an
//! operator hand-declaring a channel *is* the policy decision, correctly
//! located in config and theirs to make. An anonymous buyer resolved from
//! chain never made any such deal, and gets the mechanism.
//!
//! **What that exemption is latitude over** (issue #1138, and #1136 for
//! the domain half): how much a counterparty may spend on a channel this
//! node *is* a participant of. It presupposes redeemability rather than
//! conferring it, so it does not extend to the EIP-712 domain -- a fact
//! about which contract verifies a signature -- nor to whether this node
//! has an address on the chain at all. Both of those have exactly one
//! right answer and are checked; only the amount is the operator's.
//!
//! # Caching, and how it is refreshed
//!
//! A resolution happens on the packet path, so it must not become an RPC
//! round trip per packet. Every resolved channel is therefore memoised.
//! What is memoised divides into three kinds of fact, and the cache treats
//! each on its own terms:
//!
//! * **Immutable.** The participants and the EIP-712 domain.
//!   `TokenNetwork.openChannel`
//!   (`packages/contracts/src/TokenNetwork.sol:206-213`) assigns
//!   `participant1`/`participant2` once, when the channel is created, and
//!   no other function in that contract ever assigns either field again --
//!   `setTotalDeposit`, `claimFromChannel`, `closeChannel` and
//!   `settleChannel` mutate deposits, claimed amounts and `state` only. The
//!   EIP-712 domain is immutable for the same reason one layer up:
//!   OpenZeppelin's `EIP712("TokenNetwork", "1")` derives it from
//!   `block.chainid` and `address(this)`, and a deployed `TokenNetwork`'s
//!   address does not move. These need no invalidation at all.
//! * **Monotone.** The deposit. Its only writer on EVM is
//!   `setTotalDeposit`, which reverts on a decrease
//!   (`TokenNetwork.sol:238`); its only writer on Solana is the `Deposit`
//!   handler's `checked_add` (`processor.rs:382-388`). Neither chain has a
//!   withdraw-while-open. A cached deposit is therefore a permanent
//!   **lower bound**: it can only ever produce a false *refusal* (the payer
//!   topped up since it was read), never a false accept. So it is not
//!   re-read on the hot path at all -- only when a claim actually breaches
//!   it, via [`ClientChannelRegistry::refresh_evm`] /
//!   [`refresh_solana`](ClientChannelRegistry::refresh_solana), which
//!   raises the floor and lets the same claim through. Steady state (a
//!   client that funded once and is spending down) costs zero refreshes.
//! * **Mutable in both directions.** That the channel is not `Settled`, and
//!   that its mint is the one this node settles in. A memoised *positive*
//!   answer encodes both, and neither is immutable (issue #649): a channel
//!   resolved while `Opened` that later settles would otherwise keep
//!   resolving from cache forever, and this connector would go on accepting
//!   claims that can never be redeemed -- precisely what the resolving
//!   backends' settled-channel branches exist to refuse. So a resolved
//!   entry's liveness ages out, on the schedule
//!   [`ChannelLivenessPolicy`] sets: past `refresh_after` the next lookup
//!   re-asks the chain through the same refresh path the deposit floor
//!   uses, and a channel that has since settled (or changed mint) is
//!   dropped from the cache and refused.
//!
//! # Ageing out without a thundering herd
//!
//! An expiry is only as good as what it does when the chain is *not*
//! answering, and the naive version of it -- refuse the claim, leave the
//! entry expired -- is strictly worse than no expiry at all: the next
//! packet finds the same expired entry and re-runs the same failing lookup,
//! so a resolved channel that cost one read per minute costs one read *per
//! packet* for as long as the outage lasts, which on a rate-limited
//! endpoint is a loop that sustains its own 429s. [`ChannelLivenessPolicy`]
//! is therefore three durations rather than one, and every one of them
//! exists to bound work rather than to bound staleness:
//!
//! * `refresh_after` -- when a memoised entry stops being served without
//!   asking the chain. The staleness bound in the happy case.
//! * `serve_stale_until` -- how long past that an entry may still be
//!   *served* when the chain cannot be reached. Serve-stale-while-
//!   revalidate: a lookup failure falls back to the last reading this
//!   connector actually got, loudly, instead of refusing a paying client
//!   because an RPC endpoint blipped. Outage behaviour is then no worse
//!   than a memo with no expiry at all, which is what shipped before.
//!   Past this bound there is no fallback and the claim is refused.
//! * `min_reattempt_interval` -- the floor on how often one channel may
//!   provoke a lookup, applied to *both* triggers (an aged-out entry and a
//!   deposit-floor breach). This is what turns "per packet" into "per
//!   interval" for an outage, and what stops a client re-presenting one
//!   undercollateralized claim from re-provoking a chain read every time:
//!   that refusal deliberately consumes nothing, so without this the same
//!   claim is an unlimited free amplifier.
//!
//! The interval binds **past `serve_stale_until` as well**, where there is
//! nothing left to serve and the claim is refused either way. That case is
//! the easy one to get wrong: it looks like the moment to try hardest, and
//! it is in fact the moment a storm is least affordable, because reaching
//! it means the chain has already been failing for the whole stale window.
//! A refusal there costs an RPC or does not; the claim is refused
//! regardless, and at worst a channel that has just come back stays refused
//! for one more interval.
//!
//! A resolution in flight is also marked as such, so N packets arriving on
//! one aged-out channel cost one lookup and not N. The marker is cleared by
//! a `Drop` guard rather than on the success path, because an HTTP handler's
//! future is dropped outright when its client disconnects, and a marker that
//! survived that would wedge the channel until its stale window ran out.
//!
//! What is *not* memoised at all is any answer that could change to a
//! *better* one: a lookup failure, and a "no such channel". A buyer who
//! opens a channel and pays a second later has to be payable on their next
//! attempt rather than after a TTL, which is exactly the registration-free
//! path #502 asks for.
//!
//! # Bounding the unresolvable lookup (issue #613)
//!
//! The cost of not memoising a negative is that a sender naming channels
//! that do not exist can make this connector perform one `eth_call` each --
//! the one case [`ChannelLivenessPolicy`] cannot bound, since **there is no
//! entry to hang it on**. Every one of its protections (`refresh_after`,
//! `serve_stale_until`, `min_reattempt_interval`, the in-flight marker)
//! reads a [`Resolved`] entry, and a channel that never resolved never gets
//! one: [`ClientChannelRegistry::resolve_evm`] inserts on `Some` and
//! `remove`s on `None`. So an unresolvable lookup takes [`plan`]'s very
//! first branch -- `Plan::Ask { fallback: None, unseen: true }` -- and goes
//! to the chain, every time, however recently the last one went. That is
//! not an oversight in #654 either: an interval keyed by *channel* could
//! never bind an attack whose shape is a fresh channel id per request.
//! `unresolvable_lookups_are_not_bounded_by_the_liveness_policy` below
//! measures the gap rather than asserting it.
//!
//! Two things already made the attack less attractive than it sounds: a
//! resolution is a single `TokenNetwork.channels(id)` read plus the one
//! `participants(id, counterparty)` read the deposit needs, rather than the
//! three-call `SettlementBackend::channel_state` path; and the lookup is
//! the claim gate's *last* stage, so a claim must be structurally valid,
//! fresh and value-covering to reach it at all (issue #544's ordering).
//! Neither is a bound. The bound is [`UnresolvableLookupBudget`]: a lookup
//! for a channel with no entry is charged against the claim's declared
//! signer *and* against a node-wide ceiling before the chain is touched,
//! and refunded if the channel resolves -- so a legitimate buyer's
//! onboarding costs nothing and a walk of the id space stops. See that
//! module's own doc for what the identity is worth and what it is not.
//!
//! Exhaustion is its own answer, [`ChannelResolutionError::Budgeted`],
//! never a [`ChannelLookupFailed`] and never an absent channel: "the chain
//! said no", "the chain did not answer" and "I declined to ask" are three
//! different things an operator has to act on differently.
//!
//! # The fast path: a local channel index (issue #661)
//!
//! Everything above bounds the *cost* of an `eth_call`-per-lookup; it does
//! not remove the call. `connector-cli` wires a [`ClientChannelSource`] over
//! `connector-settlement-evm`'s `EvmChannelIndex` -- a durable local index of
//! `TokenNetwork`'s own `ChannelOpened`/`ChannelNewDeposit`/`ChannelSettled`
//! logs -- as its EVM source, so that a channel the index has caught up to
//! resolves from a `HashMap` probe with no RPC at all, and reports a settled
//! channel as [`ChannelResolutionError::Terminal`] the same way,
//! distinguishably from a channel this registry has simply never heard of.
//! The chain-reading path this module implements is the **fall-through**,
//! not the primary path: a channel the index has not caught up to (never
//! opened, opened inside its confirmation window, or its sync lagging or
//! down) is a plain [`ClientChannelSource::evm_channel`] miss, resolved
//! exactly as before -- so a node whose index has never once caught up
//! behaves byte-identically to a node built before issue #661, and every
//! mitigation in this module (liveness, the lookup budget) still governs
//! every lookup the index cannot yet answer.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use connector_signer::Address;

use crate::lookup_budget::{
    LookupBudgetExhausted, LookupReservation, UnresolvableLookupBudget,
    UnresolvableLookupBudgetPolicy,
};

/// How long a resolved channel's *mutable* facts -- that it has not
/// `Settled`, and that its mint is still the one this node settles in --
/// may be believed without re-reading them from the chain (issue #649).
///
/// A minute rather than a process lifetime: settlement is a deliberate,
/// slow, on-chain act (a close, a challenge period, then a settle), so a
/// window this size cannot be raced into by an attacker, while one read per
/// actively-paying channel per minute is not a hot path cost. It is not a
/// TTL on the *record* -- the counterparty and domain never expire, and a
/// refresh that succeeds re-uses the same entry.
pub const DEFAULT_LIVENESS_TTL: Duration = Duration::from_secs(60);

/// How long past [`DEFAULT_LIVENESS_TTL`] a memoised channel may still be
/// *served* while the chain cannot be reached at all.
///
/// Ten minutes, and the number is chosen against the threat model rather
/// than for comfort: what the expiry defends is a channel that settles on
/// chain, and settling one takes a close, a challenge period and then a
/// settle. A worst-case ten minutes of staleness -- reached only while this
/// connector's RPC endpoint is down, and logged at `warn` every time it is
/// used -- sits far inside that, while the alternative (refusing) turns
/// somebody else's outage into this node's own refusal to serve paying
/// clients.
pub const DEFAULT_SERVE_STALE_UNTIL: Duration = Duration::from_secs(600);

/// The floor on how often one channel may provoke a chain lookup.
///
/// Two seconds: long enough that a client re-presenting one claim, or a
/// packet stream on an aged-out channel, cannot turn into a per-packet read
/// on an endpoint with a request budget; short enough that a channel which
/// has genuinely settled stops being paid on within a beat of
/// `refresh_after`, and that a counterparty who deposits to clear a refusal
/// is not left waiting.
pub const DEFAULT_MIN_REATTEMPT_INTERVAL: Duration = Duration::from_secs(2);

/// When a memoised resolution stops being believed, how long it may still
/// be leaned on while the chain is unreachable, and how often one channel
/// may ask (issue #649, and the availability review of #654).
///
/// See this module's own doc for what each duration is for. The defaults
/// are [`DEFAULT_LIVENESS_TTL`], [`DEFAULT_SERVE_STALE_UNTIL`] and
/// [`DEFAULT_MIN_REATTEMPT_INTERVAL`]; a node overrides `refresh_after`
/// from its config file (`channel_liveness_ttl_secs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelLivenessPolicy {
    /// Past this, a memoised entry is re-verified before it is trusted.
    pub refresh_after: Duration,
    /// Past `refresh_after`, how long an entry may still be served when
    /// the re-verification itself fails. Measured from the last *successful*
    /// reading, so it is an absolute staleness ceiling and not a rolling
    /// one.
    pub serve_stale_until: Duration,
    /// The minimum gap between two lookups provoked by the same channel,
    /// however they were provoked.
    pub min_reattempt_interval: Duration,
}

impl Default for ChannelLivenessPolicy {
    fn default() -> ChannelLivenessPolicy {
        ChannelLivenessPolicy {
            refresh_after: DEFAULT_LIVENESS_TTL,
            serve_stale_until: DEFAULT_SERVE_STALE_UNTIL,
            min_reattempt_interval: DEFAULT_MIN_REATTEMPT_INTERVAL,
        }
    }
}

impl ChannelLivenessPolicy {
    /// Re-verify on every lookup, never serve a stale reading, never
    /// suppress an attempt. Correct, and one chain read per packet, so it
    /// is for a test that needs every re-verification to be observable --
    /// never for a production node.
    pub fn reverify_every_lookup() -> ChannelLivenessPolicy {
        ChannelLivenessPolicy {
            refresh_after: Duration::ZERO,
            serve_stale_until: Duration::ZERO,
            min_reattempt_interval: Duration::ZERO,
        }
    }
}

/// What a channel's counterparty has demonstrably put on chain, as far as
/// this connector knows: the ceiling a claim's cumulative amount may not
/// exceed (issue #646).
///
/// [`AtLeast`](DepositFloor::AtLeast) is a *lower bound*, not a reading:
/// deposits are monotonically non-decreasing while a channel is
/// `Opened`/`Closed` on both chains, so a value read at any point in the
/// past can only understate the deposit today. That is what makes it safe
/// to cache -- it can only cause a false refusal, never a false accept, and
/// a false refusal self-heals with one re-read (see
/// [`ClientChannelRegistry::refresh_evm`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepositFloor {
    /// No deposit is knowable for this channel: an operator-declared
    /// `[[client_channels]]` record names a counterparty and a domain and
    /// never an amount. Covers every claim -- the deliberate exemption
    /// described in this module's own doc.
    Unknown,
    /// The counterparty's on-chain deposit as of the last read, saturated
    /// into `u64` from whatever width the chain holds it in. Saturation is
    /// sound in the safe direction: a deposit above `u64::MAX` can never be
    /// exceeded by a `u64` cumulative amount anyway.
    AtLeast(u64),
}

impl DepositFloor {
    /// Whether a claim whose cumulative transferred amount is `amount`
    /// could be redeemed against this floor -- `amount <= deposit`, the
    /// same comparison `TokenNetwork.claimFromChannel` and
    /// `packages/solana-program`'s `Claim` handler make on redemption.
    pub fn covers(&self, amount: u64) -> bool {
        match self {
            DepositFloor::Unknown => true,
            DepositFloor::AtLeast(deposit) => amount <= *deposit,
        }
    }

    /// The floor as a number, for a refusal that has to say what the
    /// channel actually holds. `None` for [`DepositFloor::Unknown`], which
    /// never refuses anything.
    pub fn deposit(&self) -> Option<u64> {
        match self {
            DepositFloor::Unknown => None,
            DepositFloor::AtLeast(deposit) => Some(*deposit),
        }
    }
}

/// A channel identifier that is not the on-chain value its chain's claims
/// are signed over -- a `channelId` that is not a 32-byte `bytes32`, or a
/// `channelAccount` that is not a 32-byte Solana account. Refused at
/// registration rather than hashed or truncated into shape, matching
/// `connector_runtime::InvalidChannelId`'s rule on the peer semantics (issue
/// #575).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidChannelIdentifier(pub String);

impl std::fmt::Display for InvalidChannelIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "channel identifier {:?} is not a 32-byte on-chain identifier",
            self.0
        )
    }
}

impl std::error::Error for InvalidChannelIdentifier {}

/// A [`ClientChannelSource`] could not answer whether a channel exists or
/// who its counterparty is -- an unreachable RPC endpoint, a node that
/// answered with garbage, a timeout. Deliberately distinct from "this
/// channel does not exist": the first is a failure of *this connector's*,
/// the second is a fact about the world, and conflating them would let an
/// RPC outage read as a definitive "no such channel".
///
/// Either way the claim is refused. This type exists so a refusal can say
/// which of the two happened, never so anything can recover from it by
/// believing the claim instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelLookupFailed(pub String);

impl std::fmt::Display for ChannelLookupFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChannelLookupFailed {}

/// A [`ClientChannelSource`] has a definitive, known-without-a-chain-read
/// answer that a channel can never be paid on again -- it settled (issue
/// #661). A *closed* channel is deliberately not this: `claimFromChannel`
/// accepts a channel in `Closed` as readily as one in `Opened`, so closing
/// -- however it was closed -- ends nothing this connector can still be
/// paid for. Kept distinct from [`ChannelLookupFailed`] (this connector
/// could not find out) and from a plain `Ok(None)` (this connector has no
/// information either way): a source that can report this reliably -- the
/// local `TokenNetwork` event index, which has itself seen the terminal log
/// -- lets a refusal say "this channel is done" rather than the weaker "I
/// have no record of it", without spending a chain read to upgrade the
/// answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelTerminal(pub String);

impl std::fmt::Display for ChannelTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChannelTerminal {}

/// Why a channel could not be resolved -- the refusals a lookup can produce,
/// kept apart because they are not the same event (issue #613, extended by
/// #661).
///
/// [`ChannelResolutionError::LookupFailed`] is a failure: this connector
/// asked and did not get an answer. [`ChannelResolutionError::Budgeted`] is
/// a decision: this connector declined to ask, because the sender (or the
/// node as a whole) has already spent its allowance of lookups for channels
/// that turn out not to exist. [`ChannelResolutionError::Terminal`] is a
/// known fact reported without asking anything at all: a source that keeps
/// its own durable record of settlement (issue #661's local channel index)
/// can say a channel is done without either touching the chain or waiting
/// to be asked twice. All three refuse the claim; conflating them would
/// tell an operator whose endpoint is down to go looking for an attacker, an
/// operator being walked to go looking at their endpoint, and an operator
/// whose buyer's channel genuinely settled to go looking for either.
///
/// All three are also distinct from `Ok(None)` -- "there is no such
/// channel" -- which is a fact about the world rather than about this
/// connector, and the answer for a channel this connector has never heard
/// of at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelResolutionError {
    LookupFailed(ChannelLookupFailed),
    Budgeted(LookupBudgetExhausted),
    Terminal(ChannelTerminal),
}

impl From<ChannelLookupFailed> for ChannelResolutionError {
    fn from(failure: ChannelLookupFailed) -> ChannelResolutionError {
        ChannelResolutionError::LookupFailed(failure)
    }
}

impl From<LookupBudgetExhausted> for ChannelResolutionError {
    fn from(exhausted: LookupBudgetExhausted) -> ChannelResolutionError {
        ChannelResolutionError::Budgeted(exhausted)
    }
}

impl From<ChannelTerminal> for ChannelResolutionError {
    fn from(terminal: ChannelTerminal) -> ChannelResolutionError {
        ChannelResolutionError::Terminal(terminal)
    }
}

impl std::fmt::Display for ChannelResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelResolutionError::LookupFailed(failure) => write!(f, "{failure}"),
            ChannelResolutionError::Budgeted(exhausted) => write!(f, "{exhausted}"),
            ChannelResolutionError::Terminal(terminal) => write!(f, "{terminal}"),
        }
    }
}

impl std::error::Error for ChannelResolutionError {}

/// Where a channel nothing was declared for is looked up -- in production
/// the deployed `TokenNetwork` the `[settlement]` section already names
/// (`connector-cli`'s `SettlementChannelSource`), or the deployed Solana
/// payment-channel program `[settlement.solana]` names (issue #631). Kept a
/// port rather than a direct dependency on `connector-settlement-evm` or
/// `connector-settlement-solana` so this crate stays chain-agnostic, and so
/// a test can substitute a source without a chain.
///
/// An implementation MUST report the counterparty **as the chain itself
/// holds it**, never anything derived from a claim: this trait exists
/// precisely so that a claim has no say in what it is checked against.
///
/// Both methods default to answering `Ok(None)` -- "this source knows
/// nothing about that chain's channels" -- rather than being required,
/// since [`ClientChannelRegistry`] already keeps EVM and Solana sources in
/// separate [`ClaimChain`]-keyed slots and only ever calls a source's
/// method for the chain it was registered under: an EVM-only source (say,
/// `connector-cli`'s `SettlementChannelSource`) never has `solana_channel`
/// invoked, so it has nothing useful to say there and the default is never
/// exercised in practice, only spared from being restated by every
/// implementation.
#[async_trait]
pub trait ClientChannelSource: Send + Sync + std::fmt::Debug {
    /// The record for `channel_id`, or `Ok(None)` if that is not a channel
    /// this connector can be paid on -- it was never opened, it has
    /// already settled (a claim on a settled channel can never be
    /// redeemed, so accepting one would be giving the app's work away), or
    /// neither of its participants is this connector.
    ///
    /// `Err` means the lookup itself failed and the answer is unknown. It
    /// must never be reported for a channel that is merely absent.
    async fn evm_channel(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
        let _ = channel_id;
        Ok(None)
    }

    /// The record for `channel_id`, read from this source's own authority
    /// -- for a caching source, the chain its cache is built from -- never
    /// from a cache it keeps. The registry calls this instead of
    /// [`evm_channel`](Self::evm_channel) when a claim has **breached** the
    /// memoised deposit floor: the floor is a lower bound, and a cache can
    /// legitimately hold a stale or partial one (a deposit top-up whose log
    /// is not yet confirmation-deep), so re-serving the cache would refuse
    /// a claim the chain would honour. A source that already answers from
    /// the chain (the default) has nothing fresher to say than
    /// [`evm_channel`](Self::evm_channel).
    async fn evm_channel_fresh(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
        self.evm_channel(channel_id).await
    }

    /// Whether this source has a durable, definitive record that
    /// `channel_id` has settled -- without touching a chain to find out
    /// (issue #661; a merely closed channel is not terminal, see
    /// [`ChannelTerminal`]). Only ever consulted after
    /// [`evm_channel`](Self::evm_channel) has already answered `Ok(None)`
    /// for the same lookup, and only to decide how to *report* that
    /// refusal: it never overrides a positive answer, and a source with no
    /// such record (the default, and every source that predates issue #661)
    /// simply answers `false`, which reproduces today's behaviour -- the
    /// refusal reports as [`crate::ClaimIngestRejection::UnknownChannel`]
    /// exactly as it always has.
    async fn evm_channel_terminal(&self, channel_id: &[u8; 32]) -> bool {
        let _ = channel_id;
        false
    }

    /// The Solana twin of [`evm_channel`](Self::evm_channel) (issue #631):
    /// the counterparty's raw Ed25519 public key for the channel at
    /// `channel_account`, and their on-chain deposit, or `Ok(None)`/`Err`
    /// under exactly the same rules. The domain it reports alongside them
    /// is the settlement program: since ADR 0053 a Solana balance proof is
    /// signed over the program id as well as the channel account, nonce and
    /// amount (`connector_signer::solana_balance_proof_message`), which is
    /// this chain's answer to EIP-712's verifying contract. The mint is
    /// still not in the signed bytes: binding a channel to the mint this node
    /// settles in is the resolving backend's job (a chain-resolved channel
    /// on any other mint must come back `Ok(None)`), not the signature's.
    async fn solana_channel(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<SolanaChannel>, ChannelLookupFailed> {
        let _ = channel_account;
        Ok(None)
    }
}

/// Everything this connector needs to verify an EVM claim on one channel
/// without believing anything the claim says about itself: whose signature
/// it accepts, and the EIP-712 domain (ADR 0024) that signature must have
/// been produced under. `chain_id` and `token_network_address` are
/// per-channel rather than node-wide for the same reason the peer semantics's
/// `ChannelDomain` is (issue #566): each token gets its own `TokenNetwork`,
/// and therefore its own `verifyingContract`, so there is no single domain
/// a node could default to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmChannel {
    /// The address whose signature this connector accepts on a claim for
    /// this channel -- recovered from the signature, never read from the
    /// claim's own `signerAddress`.
    pub counterparty: Address,
    pub chain_id: u64,
    pub token_network_address: Address,
    /// What that counterparty has actually deposited on chain, and
    /// therefore the most a claim on this channel can ever redeem for
    /// (issue #646). [`DepositFloor::Unknown`] for a declared channel --
    /// see this module's own doc for why that exemption is deliberate.
    pub deposit_floor: DepositFloor,
}

/// The Solana twin of [`EvmChannel`]: who this connector accepts a claim's
/// signature from on one Solana channel, and how much that signature can be
/// good for. A separate type rather than a bare `[u8; 32]` counterparty
/// (which is all this was before issue #646) so that the deposit travels
/// with the counterparty through the same seam on both chains, instead of
/// being parsed out of the chain's own bytes and then thrown away.
///
/// The signing domain is `program_id` below: since ADR 0053 a Solana
/// balance proof is signed over the settlement program as well as the
/// channel account, nonce and amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaChannel {
    /// The settlement program this channel lives under, raw 32 bytes.
    ///
    /// Signed into every balance proof for this channel (ADR 0053, issue
    /// #1082), so a signature made against one deployment does not verify
    /// against another. Resolved from the same source as `counterparty` --
    /// a configured row, or the chain -- and never from the claim, which
    /// declares a `cluster` that nothing signs.
    pub program_id: [u8; 32],
    /// The raw Ed25519 public key whose signature this connector accepts on
    /// a claim for this channel -- never the claim's own
    /// `signerPublicKey`.
    pub counterparty: [u8; 32],
    /// See [`EvmChannel::deposit_floor`].
    pub deposit_floor: DepositFloor,
}

/// Which chain a claim's channel lives on -- the key a
/// [`ClientChannelSource`] is registered under in
/// [`ClientChannelRegistry`], so resolving an undeclared channel dispatches
/// on the claim's own chain through a registry rather than a single
/// hardcoded slot. EVM was the first chain with a registered source (issue
/// #611); Solana composes as a second entry under `ClaimChain::Solana`
/// (issue #631), exactly as issue #629's prefactor anticipated, rather than
/// by restructuring this type again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClaimChain {
    Evm,
    Solana,
}

/// The channels this connector has a record of, and the counterparty it
/// accepts a claim's signature from on each. EVM and Solana are separate
/// namespaces -- a `channelId` and a `channelAccount` are different kinds
/// of thing and can never satisfy each other, the same way
/// `connector_domain::ClientClaim::channel_key` namespaces the watermark
/// map.
///
/// See this module's own doc for the two sources a record comes from, and
/// for why the resolution cache is never invalidated.
#[derive(Debug)]
pub struct ClientChannelRegistry {
    evm: HashMap<[u8; 32], EvmChannel>,
    solana: HashMap<[u8; 32], SolanaChannel>,
    /// Consulted only for a channel nothing was declared for, keyed by
    /// [`ClaimChain`] so each chain's source answers for that chain alone --
    /// never, say, an EVM source consulted for a Solana lookup. Empty is a
    /// node with no settlement backend: it accepts claims on exactly what
    /// its config file declares, and on nothing else -- which since issue
    /// #1138 is nothing at all, because a declared row needs its own
    /// chain's `[settlement.<chain>]` table to load. A node with one
    /// backend and not the other still lands here for the missing chain.
    sources: HashMap<ClaimChain, Arc<dyn ClientChannelSource>>,
    /// Memoised answers from [`Self::sources`], each stamped with when the
    /// chain last confirmed it -- see this module's doc for which of the
    /// facts in one of these entries expire and which never do.
    resolved: RwLock<HashMap<[u8; 32], Resolved<EvmChannel>>>,
    /// The Solana twin of [`Self::resolved`] (issue #631) -- a separate map
    /// since a resolved Solana answer is a [`SolanaChannel`], not an
    /// [`EvmChannel`].
    resolved_solana: RwLock<HashMap<[u8; 32], Resolved<SolanaChannel>>>,
    /// When a memoised entry stops being believed, how long it may be
    /// leaned on anyway while the chain is unreachable, and how often one
    /// channel may ask (issue #649).
    liveness: ChannelLivenessPolicy,
    /// How many lookups for channels this registry has never resolved it
    /// will perform, per declared signer and in total (issue #613). The
    /// bound `liveness` structurally cannot provide, since a channel that
    /// never resolved leaves no entry for any of its intervals to hang on.
    lookup_budget: UnresolvableLookupBudget,
    /// How the most recently *completed* lookup went, **per chain** -- an
    /// entry if that chain's last lookup failed, none if it succeeded or
    /// none has completed yet (issue #613). Read only when the shaper
    /// refuses, to keep an outage reportable as an outage: an unreachable
    /// endpoint saturates the drain within seconds, and a refusal reported
    /// as a budget would send an operator hunting an attacker who is not
    /// there.
    ///
    /// Keyed by chain rather than kept as one slot, because a node with
    /// both `[settlement.evm]` and `[settlement.solana]` has two endpoints
    /// that fail independently: a single slot would make an EVM refusal
    /// quote the Solana endpoint's error, which is a worse diagnosis than
    /// the one it replaced -- it names a real outage that has nothing to do
    /// with the claim being refused.
    last_failure: RwLock<HashMap<ClaimChain, ChannelLookupFailed>>,
    /// The Solana cluster this node's own `[settlement.solana] rpc_url`
    /// names, when that URL names one this connector recognises (issue
    /// #975) -- what a Solana claim's self-declared `cluster` is
    /// cross-checked against.
    ///
    /// `None` covers two different nodes and refuses to distinguish them,
    /// because neither can perform the check: one with no
    /// `[settlement.solana]` table at all, and one whose `rpc_url` names no
    /// cluster this connector can recognise (a third-party RPC provider's).
    /// See [`Self::with_solana_cluster`].
    ///
    /// Only the cluster lives here, and deliberately. The **program id** is
    /// not a node-wide fact a claim is checked against -- it is a
    /// *per-channel* fact, carried on [`SolanaChannel::program_id`], and it
    /// is what a claim's signature is actually verified under (ADR 0053).
    /// Keeping a second, node-wide copy of it here would be a second source
    /// of truth for a value the resolved channel already holds.
    solana_cluster: Option<&'static str>,
}

/// A memoised resolution, when the chain last *confirmed* it, and when this
/// connector last *tried* to -- two different clocks, and the difference
/// between them is the whole availability story. `confirmed_at` bounds
/// staleness (issue #649); `attempted_at` bounds work, so a chain that is
/// refusing to answer cannot be asked once per packet.
#[derive(Debug, Clone, Copy)]
struct Resolved<T> {
    channel: T,
    confirmed_at: Instant,
    attempted_at: Instant,
    /// A lookup for this channel is running right now, so another packet
    /// arriving must lean on `channel` rather than start a second one.
    /// Cleared by [`InFlight`]'s `Drop`, so a handler future that is
    /// dropped mid-lookup -- what axum does when a client disconnects --
    /// cannot wedge the channel.
    in_flight: bool,
}

/// What a lookup should do about a channel, decided under the memo's own
/// lock and before any I/O.
enum Plan<T> {
    /// Answer from the memo and touch no chain.
    Serve(T),
    /// Ask the chain. `fallback` is the memoised reading to fall back on if
    /// the lookup fails -- `None` once staleness has passed
    /// `serve_stale_until`, or when there was nothing memoised to begin
    /// with, in which case a failure is a refusal.
    ///
    /// `unseen` is the second of those two cases specifically: **this
    /// registry holds no entry for the channel at all**, so none of
    /// [`ChannelLivenessPolicy`]'s bounds could apply to it and the
    /// unresolvable-lookup budget is the only thing that can (issue #613).
    /// It is not the same as `fallback.is_none()`: an entry aged past
    /// `serve_stale_until` also has no fallback, and that one is already
    /// bounded by `min_reattempt_interval` because there is an entry to
    /// record the attempt on.
    Ask { fallback: Option<T>, unseen: bool },
    /// Refuse without touching the chain: there is nothing safe left to
    /// serve, *and* this channel has already provoked a lookup too
    /// recently (or has one in flight) to be allowed another. The refusal
    /// this connector would have produced anyway, minus the RPC.
    Refuse(ChannelLookupFailed),
}

/// Clears a [`Resolved::in_flight`] marker however the attempt it belongs
/// to ends: a return, an error, a panic, or the whole future being dropped.
/// The last is not hypothetical -- a client disconnecting is enough.
struct InFlight<'a, T> {
    memo: &'a RwLock<HashMap<[u8; 32], Resolved<T>>>,
    key: [u8; 32],
}

impl<T> Drop for InFlight<'_, T> {
    fn drop(&mut self) {
        if let Ok(mut memo) = self.memo.write() {
            if let Some(entry) = memo.get_mut(&self.key) {
                entry.in_flight = false;
            }
        }
    }
}

/// Why a lookup is being considered -- the one thing the two triggers
/// disagree about is whether a *young* entry is good enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// A packet arrived and the entry may have aged out. A young entry is
    /// exactly what the memo is for, so it is served untouched.
    Age,
    /// A claim breached the memoised deposit floor. Age is beside the
    /// point: the floor is a lower bound and the reason to look again is
    /// that it is too low, not that it is old.
    Breach,
}

/// Decide what to do about `key`, marking the entry as being worked on if
/// the answer is [`Plan::Ask`]. Shared by both chains and both triggers:
/// the policy is about an entry's age and how recently it asked, and
/// neither chain's entry ages differently.
///
/// Takes the write lock because deciding to ask *is* a mutation -- the
/// attempt is recorded before it happens, which is what makes the interval
/// and the in-flight marker bind concurrent callers rather than merely
/// describe them. The guard is released before the caller's `.await`.
fn plan<T: Copy>(
    memo: &RwLock<HashMap<[u8; 32], Resolved<T>>>,
    key: &[u8; 32],
    liveness: ChannelLivenessPolicy,
    trigger: Trigger,
) -> Plan<T> {
    let mut memo = memo
        .write()
        .expect("resolved client channels lock poisoned");
    let Some(entry) = memo.get_mut(key) else {
        return Plan::Ask {
            fallback: None,
            unseen: true,
        };
    };
    if trigger == Trigger::Age && entry.confirmed_at.elapsed() < liveness.refresh_after {
        return Plan::Serve(entry.channel);
    }
    let fallback =
        (entry.confirmed_at.elapsed() < liveness.serve_stale_until).then_some(entry.channel);
    // Someone else is already asking, or somebody asked a moment ago: do
    // not add another read. Leaning on the last good reading is right here
    // for the same reason it is right on a failed lookup -- it is the most
    // recent thing the chain actually said.
    if entry.in_flight || entry.attempted_at.elapsed() < liveness.min_reattempt_interval {
        return match fallback {
            Some(channel) => Plan::Serve(channel),
            // Past the stale window there is nothing safe to serve -- and
            // that is precisely why the interval must still bind here
            // rather than be waived. Waiving it was this function's one
            // remaining hole: a channel whose chain had been failing for
            // longer than `serve_stale_until` fell through to `Ask` on
            // *every* packet, so the per-packet storm the interval exists
            // to prevent came back, ten minutes late, against an endpoint
            // that had been failing for ten minutes.
            //
            // Refusing on a timer here costs nothing that was not already
            // lost: with no fallback, a lookup that fails refuses this
            // claim anyway, so the only difference is whether the refusal
            // costs an RPC. At worst a channel that has just come back
            // stays refused for one more `min_reattempt_interval`.
            None => Plan::Refuse(ChannelLookupFailed(format!(
                "this connector's last reading of the channel is older than its \
                 serve-stale window and it is backing off from re-reading -- its chain \
                 endpoint has been failing; retry in up to {} ms",
                liveness.min_reattempt_interval.as_millis()
            ))),
        };
    }
    entry.attempted_at = Instant::now();
    entry.in_flight = true;
    Plan::Ask {
        fallback,
        unseen: false,
    }
}

impl Default for ClientChannelRegistry {
    fn default() -> ClientChannelRegistry {
        ClientChannelRegistry {
            evm: HashMap::new(),
            solana: HashMap::new(),
            sources: HashMap::new(),
            resolved: RwLock::new(HashMap::new()),
            resolved_solana: RwLock::new(HashMap::new()),
            // Hand-written rather than derived precisely for this field: a
            // derived `Default` would give all-zero durations, i.e. re-read
            // the chain on every packet.
            liveness: ChannelLivenessPolicy::default(),
            // And for this one, where a derived `Default` would give an
            // allowance of zero -- a node that refuses to resolve any
            // channel at all, i.e. #611 switched off.
            lookup_budget: UnresolvableLookupBudget::default(),
            last_failure: RwLock::new(HashMap::new()),
            solana_cluster: None,
        }
    }
}

impl ClientChannelRegistry {
    /// An empty registry -- one that refuses every claim, since it has a
    /// record of no channel at all and no source to resolve one from. See
    /// this module's own doc comment.
    pub fn new() -> ClientChannelRegistry {
        ClientChannelRegistry::default()
    }

    /// Re-verify a resolved channel after `ttl` rather than after
    /// [`DEFAULT_LIVENESS_TTL`] (issue #649), leaving the rest of
    /// [`ChannelLivenessPolicy`] at its defaults. This is the knob a node's
    /// config file turns (`channel_liveness_ttl_secs`): an operator whose
    /// RPC endpoint is expensive or rate-limited can lengthen it, and one
    /// who wants a settled channel noticed sooner can shorten it.
    pub fn with_liveness_ttl(self, ttl: Duration) -> ClientChannelRegistry {
        self.with_liveness_policy(ChannelLivenessPolicy {
            refresh_after: ttl,
            ..ChannelLivenessPolicy::default()
        })
    }

    /// Set the whole [`ChannelLivenessPolicy`], including how long a stale
    /// reading may be leaned on during an outage and how often one channel
    /// may provoke a lookup. `with_liveness_ttl` is the only part of it a
    /// config file exposes; this exists for a test that needs to observe
    /// every re-verification, or to suppress none.
    pub fn with_liveness_policy(
        mut self,
        liveness: ChannelLivenessPolicy,
    ) -> ClientChannelRegistry {
        self.liveness = liveness;
        self
    }

    /// Set how many lookups for channels this registry has never resolved
    /// it will perform per window, per declared signer and in total (issue
    /// #613). The knobs a node's config file turns
    /// (`unresolvable_lookup_budget_per_signer`,
    /// `unresolvable_lookup_budget_total`,
    /// `unresolvable_lookup_budget_window_secs`): an operator whose
    /// settlement endpoint is metered wants them tighter, and one running a
    /// busy public edge where new anonymous buyers arrive in bursts wants
    /// them looser.
    pub fn with_lookup_budget(
        mut self,
        budget: UnresolvableLookupBudgetPolicy,
    ) -> ClientChannelRegistry {
        self.lookup_budget = UnresolvableLookupBudget::new(budget);
        self
    }

    /// This registry's shaper, for the one caller outside it: `GET /ilp`'s
    /// self-description is metered through the **same** bucket rather than a
    /// second mechanism (ADR 0050). Sharing is deliberate -- one bucket is one
    /// thing for an operator to size -- and it shapes rather than drops, so
    /// the coupling costs latency and never availability.
    pub(crate) fn lookup_budget(&self) -> &UnresolvableLookupBudget {
        &self.lookup_budget
    }

    /// How long a lookup for a channel this registry has never resolved
    /// would currently wait for its slot -- zero on a node whose discovery
    /// drain is not saturated. For a log line or a test, never for a
    /// decision.
    pub fn unresolvable_lookups_queued_for(&self) -> Duration {
        self.lookup_budget.queued_for()
    }

    /// Consult `source` for any EVM channel this registry has no declared
    /// record of. Additive: everything already recorded stays
    /// authoritative and is still answered without a lookup, so
    /// `[[client_channels]]` keeps working exactly as it did -- and keeps
    /// working when the chain is unreachable.
    ///
    /// Registers `source` under [`ClaimChain::Evm`], so it is consulted for
    /// an EVM lookup and never a Solana one -- see
    /// [`with_solana_source`](Self::with_solana_source) for that twin.
    pub fn with_source(mut self, source: Arc<dyn ClientChannelSource>) -> ClientChannelRegistry {
        self.sources.insert(ClaimChain::Evm, source);
        self
    }

    /// The Solana twin of [`with_source`](Self::with_source) (issue #631):
    /// consult `source` for any Solana channel this registry has no
    /// declared record of, registering it under [`ClaimChain::Solana`] so
    /// an EVM source never answers a Solana lookup or vice versa (issue
    /// #629). Additive in exactly the same way: everything
    /// `[[client_channels]]` already declared stays authoritative and
    /// answered from memory without a lookup.
    pub fn with_solana_source(
        mut self,
        source: Arc<dyn ClientChannelSource>,
    ) -> ClientChannelRegistry {
        self.sources.insert(ClaimChain::Solana, source);
        self
    }

    /// Record the Solana cluster this node settles on (issue #975), so that
    /// a claim declaring a *different* one is refused rather than endorsed.
    ///
    /// Called only when `[settlement.solana] rpc_url` names a cluster this
    /// connector recognises -- `SolanaSettlementConfig::cluster_hint`. A
    /// node that never calls it leaves [`Self::solana_cluster`] at `None`
    /// and checks nothing, which is the honest answer for a node that
    /// genuinely does not know which cluster it is on: a guess would refuse
    /// every genuine claim it ever received.
    pub fn with_solana_cluster(mut self, cluster: &'static str) -> ClientChannelRegistry {
        self.solana_cluster = Some(cluster);
        self
    }

    /// The cluster this registry knows it is on, if it knows -- see
    /// [`Self::with_solana_cluster`].
    pub(crate) fn solana_cluster(&self) -> Option<&'static str> {
        self.solana_cluster
    }

    /// Record `channel_id`'s counterparty and EIP-712 domain. `channel_id`
    /// is the wire shape a claim names it by -- `0x`-prefixed (or bare)
    /// 64-character hex -- and is refused as
    /// [`InvalidChannelIdentifier`], never coerced, if it is not.
    pub fn record_evm(
        &mut self,
        channel_id: &str,
        channel: EvmChannel,
    ) -> Result<(), InvalidChannelIdentifier> {
        let key = decode_hex_bytes::<32>(channel_id)
            .ok_or_else(|| InvalidChannelIdentifier(channel_id.to_string()))?;
        self.evm.insert(key, channel);
        Ok(())
    }

    /// Record `channel_account`'s counterparty: the Ed25519 public key
    /// whose signature this connector accepts on a Solana claim for that
    /// channel, never the claim's own `signerPublicKey`. Both are base58,
    /// the shape they ride the wire in.
    pub fn record_solana(
        &mut self,
        channel_account: &str,
        counterparty: &str,
        program_id: &str,
    ) -> Result<(), InvalidChannelIdentifier> {
        let key = decode_base58_bytes::<32>(channel_account)
            .ok_or_else(|| InvalidChannelIdentifier(channel_account.to_string()))?;
        let counterparty = decode_base58_bytes::<32>(counterparty)
            .ok_or_else(|| InvalidChannelIdentifier(counterparty.to_string()))?;
        let program_id = decode_base58_bytes::<32>(program_id)
            .ok_or_else(|| InvalidChannelIdentifier(program_id.to_string()))?;
        self.solana.insert(
            key,
            SolanaChannel {
                program_id,
                counterparty,
                // Config declares a counterparty, never an amount -- see
                // this module's doc on the declared-channel exemption.
                deposit_floor: DepositFloor::Unknown,
            },
        );
        Ok(())
    }

    /// Whether this registry can vouch for no channel at all -- nothing
    /// declared and no source to resolve one from -- so that every claim
    /// presented to a gate holding it is refused as
    /// [`crate::ClaimIngestRejection::UnknownChannel`]. A registry with a
    /// source is not empty however little it was told at startup: the
    /// channels it can answer for live on a chain, not in this map.
    pub fn is_empty(&self) -> bool {
        self.evm.is_empty() && self.solana.is_empty() && self.sources.is_empty()
    }

    /// The record for an EVM channel: declared first, then already
    /// resolved, then -- once per channel -- the [`ClaimChain::Evm`] entry
    /// of [`Self::sources`], if one is registered. A claim on a chain with
    /// no registered entry resolves nothing here, the same
    /// [`ClaimIngestRejection::UnknownChannel`] outcome as a registry with
    /// no source at all (issue #629).
    ///
    /// `Ok(None)` is "no such channel this connector can be paid on";
    /// `Err` is "the lookup failed, or this connector declined to make it".
    /// All three refuse the claim; they are kept apart so the refusal can
    /// say which.
    ///
    /// `requester` is the identity a lookup for a channel with no record at
    /// all is budgeted against (issue #613) -- the claim's declared signer,
    /// `ClientClaim::signer_key`. It is never consulted for a declared or
    /// already-resolved channel, and never for anything but the budget: it
    /// is not authority for who signed anything.
    pub(crate) async fn evm(
        &self,
        channel_id: &[u8; 32],
        requester: &str,
    ) -> Result<Option<EvmChannel>, ChannelResolutionError> {
        if let Some(channel) = self.evm.get(channel_id) {
            return Ok(Some(*channel));
        }
        // The guard `plan` takes is released before the `.await` below: a
        // `std::sync::RwLock` guard held across a suspension point is both
        // non-`Send` and a way to stall every other packet in flight.
        match plan(&self.resolved, channel_id, self.liveness, Trigger::Age) {
            Plan::Serve(channel) => Ok(Some(channel)),
            Plan::Refuse(failure) => Err(failure.into()),
            Plan::Ask { fallback, unseen } => {
                self.resolve_evm(channel_id, fallback, unseen, requester, Trigger::Age)
                    .await
            }
        }
    }

    /// Ask the chain about `channel_id` again, whatever is memoised for it
    /// (issues #646, #649), and answer from that fresh reading.
    ///
    /// The claim gate calls this when a claim **breaches** the memoised
    /// deposit floor: the floor is a lower bound, so a breach is not yet a
    /// refusal, it is a reason to look again -- and a counterparty who
    /// topped up gets their claim honoured on the same submission rather
    /// than after a restart.
    ///
    /// It is the same [`plan`] the ageing path uses, so a breach obeys the
    /// same re-attempt interval: without that, a client re-presenting one
    /// undercollateralized claim would provoke a chain read every single
    /// time, since refusing it deliberately consumes no nonce. Suppressed
    /// or not, the answer is the memoised floor, which is exactly what the
    /// gate needs to refuse against.
    ///
    /// A channel the chain no longer vouches for -- settled, wrong mint,
    /// gone -- is **removed** from the memo and answered `Ok(None)`, so the
    /// stale positive cannot be served again by the next packet either.
    /// A declared channel has nothing to refresh: config, not the chain, is
    /// its authority, and it is answered from config exactly as
    /// [`Self::evm`] would.
    pub(crate) async fn refresh_evm(
        &self,
        channel_id: &[u8; 32],
        requester: &str,
    ) -> Result<Option<EvmChannel>, ChannelResolutionError> {
        if let Some(channel) = self.evm.get(channel_id) {
            return Ok(Some(*channel));
        }
        match plan(&self.resolved, channel_id, self.liveness, Trigger::Breach) {
            Plan::Serve(channel) => Ok(Some(channel)),
            Plan::Refuse(failure) => Err(failure.into()),
            Plan::Ask { fallback, unseen } => {
                self.resolve_evm(channel_id, fallback, unseen, requester, Trigger::Breach)
                    .await
            }
        }
    }

    /// The one place [`Self::sources`]'s EVM entry is consulted, and the
    /// one place [`Self::resolved`] is written. `fallback` is the memoised
    /// reading to answer with if the lookup fails, and `unseen` says this
    /// registry held no entry for the channel at all -- see [`Plan::Ask`]
    /// for why the two are not the same thing. `trigger` decides which of
    /// the source's two reads this is: a [`Trigger::Breach`] must reach the
    /// source's authority ([`ClientChannelSource::evm_channel_fresh`]) --
    /// the whole reason a breach re-reads is that a cached floor may be
    /// stale -- while an ageing re-read is content with whatever the source
    /// considers current.
    async fn resolve_evm(
        &self,
        channel_id: &[u8; 32],
        fallback: Option<EvmChannel>,
        unseen: bool,
        requester: &str,
        trigger: Trigger,
    ) -> Result<Option<EvmChannel>, ChannelResolutionError> {
        let Some(source) = self.sources.get(&ClaimChain::Evm) else {
            return Ok(None);
        };
        // Before the chain is touched, not after: the point is to prevent
        // the read (issue #613). Charged under the budget's own lock, so a
        // burst arriving at once is bound by the same number a sequence is
        // -- which matters here precisely because an unseen channel has no
        // memo entry for the in-flight marker to be written on.
        let reservation = self
            .reserve_lookup(ClaimChain::Evm, unseen, requester, || {
                hex::encode(channel_id)
            })
            .await?;
        let _in_flight = InFlight {
            memo: &self.resolved,
            key: *channel_id,
        };
        let lookup = match trigger {
            Trigger::Age => source.evm_channel(channel_id).await,
            Trigger::Breach => source.evm_channel_fresh(channel_id).await,
        };
        let resolved = match lookup {
            Ok(resolved) => {
                self.record_lookup_outcome(ClaimChain::Evm, None);
                resolved
            }
            // A failed lookup says nothing about the channel, so it must
            // not be allowed to say anything about the memo either --
            // neither evicting the entry nor, crucially, refusing a client
            // whose channel this connector read perfectly well a minute
            // ago. Serving the last good reading makes an outage no worse
            // than the memo-with-no-expiry this replaced; past
            // `serve_stale_until` there is no fallback and the claim is
            // refused for what it is.
            Err(failure) => {
                self.record_lookup_outcome(ClaimChain::Evm, Some(&failure));
                return match fallback {
                    Some(channel) => {
                        tracing::warn!(
                            channel_id = %hex::encode(channel_id),
                            error = %failure,
                            "serving a client channel from a stale resolution: the chain could \
                             not be re-read, so its liveness and deposit are older than this \
                             node's refresh interval"
                        );
                        Ok(Some(channel))
                    }
                    None => Err(failure.into()),
                };
            }
        };
        // A source that itself keeps a durable record of settlement (issue
        // #661's local channel index) can say more than `Ok(None)` --
        // "no such channel" -- normally means: it can say the channel is
        // *known and done*, without a chain read either way. Checked here,
        // once, rather than folded into `evm_channel` itself, so every
        // existing source (which answers `false` by default) is completely
        // unaffected and this stays additive.
        if resolved.is_none() && source.evm_channel_terminal(channel_id).await {
            // A known fact, not an unresolvable walk (issue #613): refund
            // rather than charge, and drop any stale positive memo entry so
            // the next lookup is refused the same way instead of served
            // from a reading that predates the settlement.
            if let Some(reservation) = reservation {
                self.lookup_budget.refund(reservation);
            }
            self.resolved
                .write()
                .expect("resolved client channels lock poisoned")
                .remove(channel_id);
            return Err(ChannelTerminal(format!(
                "channel {} has settled and can never be redeemed again",
                hex::encode(channel_id)
            ))
            .into());
        }
        // The lookup found a channel, so it was not an unresolvable one and
        // must not be charged for -- otherwise a node onboarding real
        // anonymous buyers throttles itself for doing exactly what #611
        // built this path to do. Given back before the memo's own lock is
        // taken, so the two locks are never held at once.
        if let (Some(reservation), Some(_)) = (reservation, &resolved) {
            self.lookup_budget.refund(reservation);
        }
        let mut memo = self
            .resolved
            .write()
            .expect("resolved client channels lock poisoned");
        match resolved {
            Some(channel) => {
                let now = Instant::now();
                memo.insert(
                    *channel_id,
                    Resolved {
                        channel,
                        confirmed_at: now,
                        attempted_at: now,
                        in_flight: false,
                    },
                );
                Ok(Some(channel))
            }
            None => {
                // "No such channel this connector can be paid on" is
                // deliberately not memoised as a negative -- a channel
                // opened a second from now must be payable on that
                // sender's next attempt -- but a previously *positive*
                // answer that has become this one is dropped (issue #649).
                memo.remove(channel_id);
                Ok(None)
            }
        }
    }

    /// The counterparty for a Solana channel: declared first, then already
    /// resolved, then -- once per channel -- the [`ClaimChain::Solana`]
    /// entry of [`Self::sources`], if one is registered (issue #631, the
    /// Solana twin of [`Self::evm`]). A claim on a chain with no registered
    /// entry resolves nothing here, same as [`Self::evm`].
    ///
    /// `Ok(None)` is "no such channel this connector can be paid on"; `Err`
    /// is "the lookup failed, so the answer is unknown". Both refuse the
    /// claim; they are kept apart so the refusal can say which.
    pub(crate) async fn solana(
        &self,
        channel_account: &[u8; 32],
        requester: &str,
    ) -> Result<Option<SolanaChannel>, ChannelResolutionError> {
        if let Some(channel) = self.solana.get(channel_account) {
            return Ok(Some(*channel));
        }
        // The guard `plan` takes is released before the `.await` below --
        // see `Self::evm`'s own comment on the same shape.
        match plan(
            &self.resolved_solana,
            channel_account,
            self.liveness,
            Trigger::Age,
        ) {
            Plan::Serve(channel) => Ok(Some(channel)),
            Plan::Refuse(failure) => Err(failure.into()),
            Plan::Ask { fallback, unseen } => {
                self.resolve_solana(channel_account, fallback, unseen, requester)
                    .await
            }
        }
    }

    /// The Solana twin of [`Self::refresh_evm`] (issues #646, #649), with
    /// the same caller and the same rules.
    pub(crate) async fn refresh_solana(
        &self,
        channel_account: &[u8; 32],
        requester: &str,
    ) -> Result<Option<SolanaChannel>, ChannelResolutionError> {
        if let Some(channel) = self.solana.get(channel_account) {
            return Ok(Some(*channel));
        }
        match plan(
            &self.resolved_solana,
            channel_account,
            self.liveness,
            Trigger::Breach,
        ) {
            Plan::Serve(channel) => Ok(Some(channel)),
            Plan::Refuse(failure) => Err(failure.into()),
            Plan::Ask { fallback, unseen } => {
                self.resolve_solana(channel_account, fallback, unseen, requester)
                    .await
            }
        }
    }

    /// Claim a slot for a lookup on a channel this registry has never
    /// resolved, waiting for it if the drain is in arrears (issue #613).
    /// `Ok(None)` -- no reservation, nothing to refund -- for a lookup that
    /// is *not* the unbounded kind: there is an entry for the channel, so
    /// [`ChannelLivenessPolicy`] already bounds it, and shaping it as well
    /// would only make an already-paying client's re-verification queue
    /// behind a stranger's first one.
    ///
    /// `channel` is a closure rather than a string because it is only ever
    /// formatted for the refusal's log line, and a refusal is the rare
    /// case: hex-encoding 32 bytes on every admitted lookup to describe the
    /// ones that are not admitted would be paying the cost of the defence
    /// on the path it defends.
    async fn reserve_lookup(
        &self,
        chain: ClaimChain,
        unseen: bool,
        requester: &str,
        channel: impl FnOnce() -> String,
    ) -> Result<Option<LookupReservation>, ChannelResolutionError> {
        if !unseen {
            return Ok(None);
        }
        let exhausted = match self.lookup_budget.reserve(requester).await {
            Ok(reservation) => return Ok(Some(reservation)),
            Err(exhausted) => exhausted,
        };

        // An outage outranks a shaper. A node whose settlement endpoint is
        // unreachable fails every lookup it makes, and a failed lookup
        // consumes its slot, so the drain saturates within seconds of the
        // endpoint going down -- at which point a refusal reported as a
        // budget would tell an operator they are being walked when in fact
        // their RPC is dead. That is exactly the diagnosis issue #613 asks
        // to keep possible ("degrades loudly rather than looking like an
        // attack"), so while the last lookup this node actually completed
        // came back a failure, that failure is what a refusal reports.
        // Cleared by the next lookup that succeeds, so it cannot outlive
        // the outage that set it.
        if let Some(failure) = self.last_lookup_failure(chain) {
            tracing::warn!(
                signer = %requester,
                channel = %channel(),
                error = %failure,
                "refusing a client claim without a lookup: this node's discovery drain is \
                 saturated, and every lookup it has completed recently has failed -- its chain \
                 endpoint is down"
            );
            return Err(ChannelResolutionError::LookupFailed(failure));
        }

        // `warn`, and every field an operator needs to act on it: which
        // axis was saturated (an attributable sender, or the node-wide
        // drain that says nothing about who), who declared themselves the
        // payer, and which channel they were asking about. Deliberately
        // not `error` -- the node is doing what it was configured to do.
        tracing::warn!(
            bound = exhausted.bound.as_str(),
            allowance = exhausted.allowance,
            window_secs = exhausted.window.as_secs(),
            max_wait_ms = exhausted.max_wait.as_millis(),
            signer = %requester,
            channel = %channel(),
            "declining to resolve an unknown channel from chain: this node's discovery drain is \
             saturated and its queue is full"
        );
        Err(ChannelResolutionError::Budgeted(exhausted))
    }

    /// The failure `chain`'s most recently *completed* lookup came back
    /// with, if it came back with one. `None` once one of that chain's
    /// lookups has succeeded since -- and never affected by the other
    /// chain's endpoint, which fails on its own schedule.
    fn last_lookup_failure(&self, chain: ClaimChain) -> Option<ChannelLookupFailed> {
        self.last_failure
            .read()
            .expect("last lookup outcome lock poisoned")
            .get(&chain)
            .cloned()
    }

    /// Record how one of `chain`'s completed lookups went, for
    /// [`Self::last_lookup_failure`]. Called on every lookup that reaches an
    /// answer, and only there: a lookup that was never made says nothing
    /// about the endpoint.
    fn record_lookup_outcome(&self, chain: ClaimChain, failure: Option<&ChannelLookupFailed>) {
        let mut last = self
            .last_failure
            .write()
            .expect("last lookup outcome lock poisoned");
        match failure {
            Some(failure) => last.insert(chain, failure.clone()),
            None => last.remove(&chain),
        };
    }

    /// The Solana twin of [`Self::resolve_evm`].
    async fn resolve_solana(
        &self,
        channel_account: &[u8; 32],
        fallback: Option<SolanaChannel>,
        unseen: bool,
        requester: &str,
    ) -> Result<Option<SolanaChannel>, ChannelResolutionError> {
        let Some(source) = self.sources.get(&ClaimChain::Solana) else {
            return Ok(None);
        };
        // See `Self::resolve_evm` for why this is charged before the chain
        // is touched (issue #613). Solana's own resolution is one account
        // read rather than two, which makes the attack cheaper for this
        // connector to absorb and not one bit less unbounded.
        let reservation = self
            .reserve_lookup(ClaimChain::Solana, unseen, requester, || {
                bs58::encode(channel_account).into_string()
            })
            .await?;
        let _in_flight = InFlight {
            memo: &self.resolved_solana,
            key: *channel_account,
        };
        let resolved = match source.solana_channel(channel_account).await {
            Ok(resolved) => {
                self.record_lookup_outcome(ClaimChain::Solana, None);
                resolved
            }
            // Serve-stale-while-revalidate -- see `Self::resolve_evm`'s own
            // comment for why an outage must not become this node's refusal.
            Err(failure) => {
                self.record_lookup_outcome(ClaimChain::Solana, Some(&failure));
                return match fallback {
                    Some(channel) => {
                        tracing::warn!(
                            channel_account = %bs58::encode(channel_account).into_string(),
                            error = %failure,
                            "serving a client channel from a stale resolution: the chain could \
                             not be re-read, so its liveness and deposit are older than this \
                             node's refresh interval"
                        );
                        Ok(Some(channel))
                    }
                    None => Err(failure.into()),
                };
            }
        };
        // See `Self::resolve_evm`: a resolution that succeeded is not an
        // unresolvable lookup and gives its charge back.
        if let (Some(reservation), Some(_)) = (reservation, &resolved) {
            self.lookup_budget.refund(reservation);
        }
        let mut memo = self
            .resolved_solana
            .write()
            .expect("resolved client channels lock poisoned");
        match resolved {
            Some(channel) => {
                let now = Instant::now();
                memo.insert(
                    *channel_account,
                    Resolved {
                        channel,
                        confirmed_at: now,
                        attempted_at: now,
                        in_flight: false,
                    },
                );
                Ok(Some(channel))
            }
            None => {
                memo.remove(channel_account);
                Ok(None)
            }
        }
    }
}

/// Decode a `0x`-prefixed (or bare) hex string into exactly `N` bytes, or
/// `None` for anything malformed or the wrong length -- never a panic, same
/// as every other step of the claim gate (issue #506's "refused as a
/// validation failure, never as a crash").
pub(crate) fn decode_hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    hex::decode(s.strip_prefix("0x").unwrap_or(s))
        .ok()?
        .try_into()
        .ok()
}

/// Decode a base58 string into exactly `N` bytes, or `None` for anything
/// malformed or the wrong length.
pub(crate) fn decode_base58_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    bs58::decode(s).into_vec().ok()?.try_into().ok()
}

#[cfg(test)]
pub(crate) mod test_source {
    //! A stand-in for a chain, shared with [`crate::claim_gate`]'s own
    //! tests: answers for exactly the channels it was handed, counts how
    //! often it was asked, and can be made to fail the way an unreachable
    //! RPC endpoint does.

    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Debug)]
    pub(crate) struct FakeChannelSource {
        /// Behind a lock so a test can play the part of a chain that
        /// *changed* -- a counterparty topping up their deposit (issue
        /// #646), or a channel settling (issue #649) -- which is the whole
        /// subject of the refresh path and cannot be expressed by a source
        /// fixed at construction.
        channels: Mutex<HashMap<[u8; 32], EvmChannel>>,
        /// `Some` once the chain has stopped answering -- and settable
        /// after construction, because an outage that a *resolved* channel
        /// lives through is the whole subject of the availability tests and
        /// cannot be expressed by a source that failed from the start.
        failure: Mutex<Option<String>>,
        /// Channels this source has a durable, definitive record of having
        /// settled (issue #661) -- a stand-in for the local channel index's
        /// own terminal record, answered by
        /// [`ClientChannelSource::evm_channel_terminal`] and never counted
        /// against [`Self::lookups`], since the real index answers it from
        /// memory rather than a chain read.
        terminal: Mutex<HashSet<[u8; 32]>>,
        /// How long a lookup takes. Non-zero lets a test put several
        /// lookups genuinely in flight at once, which is what a stampede
        /// is; zero would let each future complete before the next is
        /// polled and prove nothing about single-flight.
        latency: Duration,
        lookups: AtomicUsize,
    }

    impl FakeChannelSource {
        pub(crate) fn knowing(channels: Vec<([u8; 32], EvmChannel)>) -> FakeChannelSource {
            FakeChannelSource {
                channels: Mutex::new(channels.into_iter().collect()),
                failure: Mutex::new(None),
                terminal: Mutex::new(HashSet::new()),
                latency: Duration::ZERO,
                lookups: AtomicUsize::new(0),
            }
        }

        pub(crate) fn unreachable(reason: &str) -> FakeChannelSource {
            FakeChannelSource {
                channels: Mutex::new(HashMap::new()),
                failure: Mutex::new(Some(reason.to_string())),
                terminal: Mutex::new(HashSet::new()),
                latency: Duration::ZERO,
                lookups: AtomicUsize::new(0),
            }
        }

        /// This source now has a durable, definitive record that
        /// `channel_id` has settled (issue #661) -- the stand-in for the
        /// local channel index having observed the terminal log. Also
        /// removes any positive `now_says` entry, the same way a real index
        /// drops a channel's active record once it sees the terminal
        /// event.
        pub(crate) fn now_terminal(&self, channel_id: [u8; 32]) {
            self.terminal
                .lock()
                .expect("fake source lock poisoned")
                .insert(channel_id);
            self.channels
                .lock()
                .expect("fake source lock poisoned")
                .remove(&channel_id);
        }

        /// Every lookup from now on takes `latency` -- see the field's own
        /// doc.
        pub(crate) fn taking(mut self, latency: Duration) -> FakeChannelSource {
            self.latency = latency;
            self
        }

        /// The chain stops answering (`Some`), or starts again (`None`).
        pub(crate) fn now_fails(&self, reason: Option<&str>) {
            *self.failure.lock().expect("fake source lock poisoned") =
                reason.map(|reason| reason.to_string());
        }

        /// What the chain says about `channel_id` from now on: `Some` for a
        /// channel that is (still) payable, `None` for one that has
        /// settled, changed mint or otherwise stopped being one this
        /// connector can be paid on.
        pub(crate) fn now_says(&self, channel_id: [u8; 32], channel: Option<EvmChannel>) {
            let mut channels = self.channels.lock().expect("fake source lock poisoned");
            match channel {
                Some(channel) => channels.insert(channel_id, channel),
                None => channels.remove(&channel_id),
            };
        }

        pub(crate) fn lookups(&self) -> usize {
            self.lookups.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ClientChannelSource for FakeChannelSource {
        async fn evm_channel(
            &self,
            channel_id: &[u8; 32],
        ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            if !self.latency.is_zero() {
                tokio::time::sleep(self.latency).await;
            }
            if let Some(reason) = self
                .failure
                .lock()
                .expect("fake source lock poisoned")
                .clone()
            {
                return Err(ChannelLookupFailed(reason));
            }
            Ok(self
                .channels
                .lock()
                .expect("fake source lock poisoned")
                .get(channel_id)
                .copied())
        }

        async fn evm_channel_terminal(&self, channel_id: &[u8; 32]) -> bool {
            // Deliberately does not touch `self.lookups` -- the whole point
            // of issue #661's terminal check is that it costs no chain read
            // at all, so a test asserting `lookups() == 0` on a terminal
            // channel must see exactly that.
            self.terminal
                .lock()
                .expect("fake source lock poisoned")
                .contains(channel_id)
        }
    }

    /// The Solana twin of [`FakeChannelSource`] (issue #631): a stand-in
    /// for `connector-cli`'s adapter over `SolanaSettlementBackend`, kept a
    /// separate type rather than a second field on [`FakeChannelSource`] so
    /// an EVM-only test's `lookups()` count can never be perturbed by a
    /// Solana lookup or vice versa.
    #[derive(Debug)]
    pub(crate) struct FakeSolanaChannelSource {
        /// Behind a lock for the same reason [`FakeChannelSource`]'s is.
        channels: Mutex<HashMap<[u8; 32], SolanaChannel>>,
        /// See [`FakeChannelSource::failure`].
        failure: Mutex<Option<String>>,
        lookups: AtomicUsize,
    }

    impl FakeSolanaChannelSource {
        pub(crate) fn knowing(channels: Vec<([u8; 32], SolanaChannel)>) -> FakeSolanaChannelSource {
            FakeSolanaChannelSource {
                channels: Mutex::new(channels.into_iter().collect()),
                failure: Mutex::new(None),
                lookups: AtomicUsize::new(0),
            }
        }

        pub(crate) fn unreachable(reason: &str) -> FakeSolanaChannelSource {
            FakeSolanaChannelSource {
                channels: Mutex::new(HashMap::new()),
                failure: Mutex::new(Some(reason.to_string())),
                lookups: AtomicUsize::new(0),
            }
        }

        /// The Solana twin of [`FakeChannelSource::now_fails`].
        pub(crate) fn now_fails(&self, reason: Option<&str>) {
            *self.failure.lock().expect("fake source lock poisoned") =
                reason.map(|reason| reason.to_string());
        }

        /// The Solana twin of [`FakeChannelSource::now_says`].
        pub(crate) fn now_says(&self, account: [u8; 32], channel: Option<SolanaChannel>) {
            let mut channels = self.channels.lock().expect("fake source lock poisoned");
            match channel {
                Some(channel) => channels.insert(account, channel),
                None => channels.remove(&account),
            };
        }

        pub(crate) fn lookups(&self) -> usize {
            self.lookups.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ClientChannelSource for FakeSolanaChannelSource {
        async fn solana_channel(
            &self,
            channel_account: &[u8; 32],
        ) -> Result<Option<SolanaChannel>, ChannelLookupFailed> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            if let Some(reason) = self
                .failure
                .lock()
                .expect("fake source lock poisoned")
                .clone()
            {
                return Err(ChannelLookupFailed(reason));
            }
            Ok(self
                .channels
                .lock()
                .expect("fake source lock poisoned")
                .get(channel_account)
                .copied())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_source::FakeChannelSource;
    use super::*;
    use crate::lookup_budget::LookupBudgetBound;

    /// The identity an unresolvable lookup is budgeted against (issue
    /// #613): the signer a claim declared for itself, as
    /// `ClientClaim::signer_key` spells it. Most of the tests below are
    /// about a channel that resolves, where it is never consulted at all --
    /// the ones it is the subject of are gathered at the end of this
    /// module and name their own senders.
    const A_BUYER: &str = "evm:0x1111111111111111111111111111111111111111";

    fn evm_channel() -> EvmChannel {
        evm_channel_depositing(DepositFloor::AtLeast(1_000))
    }

    fn evm_channel_depositing(deposit_floor: DepositFloor) -> EvmChannel {
        EvmChannel {
            counterparty: [0x11; 20],
            chain_id: 8453,
            token_network_address: [0x42; 20],
            deposit_floor,
        }
    }

    fn solana_channel() -> SolanaChannel {
        SolanaChannel {
            program_id: [7u8; 32],
            counterparty: [0x09; 32],
            deposit_floor: DepositFloor::AtLeast(1_000),
        }
    }

    #[test]
    fn a_deposit_floor_covers_exactly_up_to_the_deposit() {
        let floor = DepositFloor::AtLeast(1_000);
        assert!(floor.covers(999));
        assert!(floor.covers(1_000));
        assert!(!floor.covers(1_001));
        assert!(DepositFloor::Unknown.covers(u64::MAX));
    }

    #[tokio::test]
    async fn a_recorded_evm_channel_is_found_under_the_id_it_was_recorded_by() {
        let mut registry = ClientChannelRegistry::new();
        let channel_id = format!("0x{}", "ab".repeat(32));
        registry
            .record_evm(&channel_id, evm_channel())
            .expect("a 32-byte hex channel id");

        let key = decode_hex_bytes::<32>(&channel_id).unwrap();
        assert_eq!(registry.evm(&key, A_BUYER).await, Ok(Some(evm_channel())));
    }

    #[tokio::test]
    async fn the_0x_prefix_is_not_part_of_a_channels_identity() {
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(&"ab".repeat(32), evm_channel())
            .expect("a bare 32-byte hex channel id");

        // A claim naming the same channel with the `0x` prefix names the
        // same channel -- the prefix is notation, not identity.
        let key = decode_hex_bytes::<32>(&format!("0x{}", "ab".repeat(32))).unwrap();
        assert_eq!(registry.evm(&key, A_BUYER).await, Ok(Some(evm_channel())));
    }

    #[test]
    fn an_id_that_is_not_a_32_byte_channel_is_refused_never_coerced() {
        let mut registry = ClientChannelRegistry::new();
        assert_eq!(
            registry.record_evm("0xdeadbeef", evm_channel()),
            Err(InvalidChannelIdentifier("0xdeadbeef".to_string()))
        );
        assert!(
            registry.is_empty(),
            "nothing was recorded under a coerced id"
        );
    }

    /// Issue #629: the source is stored under the claim's chain
    /// ([`ClaimChain::Evm`]) rather than as a single untyped slot a lookup
    /// for any chain could fall into. An EVM source registered via
    /// `with_source` must never answer a Solana lookup for the very same 32
    /// bytes -- the regression a chain-agnostic "one source" field would
    /// silently permit once a Solana entry is added alongside it.
    #[tokio::test]
    async fn an_evm_source_never_answers_a_solana_lookup_for_the_same_bytes() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x09; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new().with_source(source);

        assert_eq!(
            registry.evm(&[0x09; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(registry.solana(&[0x09; 32], A_BUYER).await, Ok(None));
    }

    #[tokio::test]
    async fn a_recorded_solana_channel_is_found_under_the_account_it_was_recorded_by() {
        let mut registry = ClientChannelRegistry::new();
        let account = bs58::encode([3u8; 32]).into_string();
        let counterparty = bs58::encode([7u8; 32]).into_string();
        registry
            .record_solana(
                &account,
                &counterparty,
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .expect("a 32-byte base58 account");

        assert_eq!(
            registry.solana(&[3u8; 32], A_BUYER).await,
            Ok(Some(SolanaChannel {
                program_id: [7u8; 32],
                counterparty: [7u8; 32],
                deposit_floor: DepositFloor::Unknown,
            }))
        );
    }

    #[tokio::test]
    async fn evm_and_solana_channels_are_separate_namespaces() {
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(&"03".repeat(32), evm_channel())
            .expect("a 32-byte hex channel id");

        // The same 32 bytes, presented as a Solana account, is not that
        // channel: an EVM record can never answer for a Solana claim.
        assert_eq!(registry.solana(&[3u8; 32], A_BUYER).await, Ok(None));
    }

    #[test]
    fn a_fresh_registry_has_a_record_of_no_channel() {
        assert!(ClientChannelRegistry::new().is_empty());
    }

    /// Issues #556/#502: a channel nothing declared, that the chain knows
    /// about, is answered for. This is the whole point of the source --
    /// without it an unaffiliated buyer cannot pay until an operator edits
    /// a config file and restarts the node.
    #[tokio::test]
    async fn a_channel_only_the_source_knows_about_is_resolved() {
        let registry = ClientChannelRegistry::new().with_source(Arc::new(
            FakeChannelSource::knowing(vec![([0x07; 32], evm_channel())]),
        ));

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
    }

    /// The cache: a second claim on the same channel costs no second
    /// lookup, which is what keeps the packet path off the RPC endpoint.
    #[tokio::test]
    async fn a_resolved_channel_is_answered_from_memory_the_second_time() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new().with_source(source.clone());

        for _ in 0..5 {
            assert_eq!(
                registry.evm(&[0x07; 32], A_BUYER).await,
                Ok(Some(evm_channel()))
            );
        }
        assert_eq!(
            source.lookups(),
            1,
            "the chain is asked once per channel, not once per packet"
        );
    }

    /// A declared channel is authoritative: the source is never consulted
    /// for it, so a node whose config names its channels keeps accepting
    /// their claims while the RPC endpoint is down.
    #[tokio::test]
    async fn a_declared_channel_is_never_looked_up() {
        let source = Arc::new(FakeChannelSource::unreachable("connection refused"));
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(&"07".repeat(32), evm_channel())
            .expect("a 32-byte hex channel id");
        let registry = registry.with_source(source.clone());

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(source.lookups(), 0);
    }

    /// A channel the source does not know about is absent, not a failure
    /// -- and "absent" is not memoised, so a buyer who opens their channel
    /// a moment later is not locked out by a stale negative.
    #[tokio::test]
    async fn an_unknown_channel_is_absent_rather_than_a_failure_and_is_not_cached() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new().with_source(source.clone());

        assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));
        assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));
        assert_eq!(
            source.lookups(),
            2,
            "a channel that did not exist yet is asked about again"
        );
    }

    /// Issue #661: a source that keeps its own durable record of settlement
    /// (the local channel index) reports a terminal channel distinguishably
    /// from a channel it has simply never heard of, and does so without a
    /// chain read -- `lookups()` never counts it, unlike the unknown-channel
    /// case above.
    #[tokio::test]
    async fn a_terminal_channel_is_refused_distinguishably_from_an_unknown_one() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        source.now_terminal([0x07; 32]);
        let registry = ClientChannelRegistry::new().with_source(source.clone());

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Err(ChannelTerminal(
                "channel 0707070707070707070707070707070707070707070707070707070707070707 has \
                 settled and can never be redeemed again"
                    .to_string()
            )
            .into())
        );
        assert_ne!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(None),
            "a terminal channel must not be reported the same way as a channel this registry \
             has simply never heard of"
        );
        assert_eq!(
            source.lookups(),
            2,
            "the terminal check itself is not counted as a chain-reading lookup, but \
             evm_channel is still asked (and answers None) each time"
        );
    }

    /// A positive resolution that later turns terminal (issue #649's
    /// settle-while-cached case, now served by the index instead of a
    /// refresh) is dropped from the memo rather than kept alive: the very
    /// next lookup is refused, not served from a reading that predates the
    /// settlement.
    #[tokio::test]
    async fn a_channel_that_turns_terminal_after_being_resolved_stops_being_served_from_cache() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup());

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );

        source.now_terminal([0x07; 32]);
        // A refresh -- what a deposit-floor breach drives -- re-asks the
        // source regardless of age under `reverify_every_lookup`, the same
        // path issue #649's settle-while-cached case exercises.
        assert_eq!(
            registry.refresh_evm(&[0x07; 32], A_BUYER).await,
            Err(ChannelTerminal(
                "channel 0707070707070707070707070707070707070707070707070707070707070707 has \
                 settled and can never be redeemed again"
                    .to_string()
            )
            .into())
        );
    }

    /// A lookup this connector could not complete is a failure of its own,
    /// never silently "no such channel" and never a reason to believe what
    /// the claim says about itself.
    #[tokio::test]
    async fn a_lookup_failure_is_reported_as_a_failure() {
        let registry = ClientChannelRegistry::new().with_source(Arc::new(
            FakeChannelSource::unreachable("connection refused"),
        ));

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Err(ChannelLookupFailed("connection refused".to_string()).into())
        );
    }

    /// A registry with a source can vouch for channels it was never told
    /// about, so it is not "empty" in the sense the gate cares about.
    #[test]
    fn a_registry_with_a_source_is_not_empty() {
        let registry =
            ClientChannelRegistry::new().with_source(Arc::new(FakeChannelSource::knowing(vec![])));
        assert!(!registry.is_empty());
    }

    /// Issue #631: the Solana twin of
    /// `a_channel_only_the_source_knows_about_is_resolved` above -- a
    /// Solana channel nothing declared, that the chain knows about, is
    /// answered for through a registered [`ClaimChain::Solana`] source.
    #[tokio::test]
    async fn a_solana_channel_only_the_source_knows_about_is_resolved() {
        let registry = ClientChannelRegistry::new().with_solana_source(Arc::new(
            super::test_source::FakeSolanaChannelSource::knowing(vec![(
                [0x07; 32],
                solana_channel(),
            )]),
        ));

        assert_eq!(
            registry.solana(&[0x07; 32], A_BUYER).await,
            Ok(Some(solana_channel()))
        );
    }

    /// The Solana twin of `a_resolved_channel_is_answered_from_memory_the_second_time`:
    /// a second claim on the same Solana channel costs no second lookup.
    #[tokio::test]
    async fn a_resolved_solana_channel_is_answered_from_memory_the_second_time() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::knowing(vec![
            ([0x07; 32], solana_channel()),
        ]));
        let registry = ClientChannelRegistry::new().with_solana_source(source.clone());

        for _ in 0..5 {
            assert_eq!(
                registry.solana(&[0x07; 32], A_BUYER).await,
                Ok(Some(solana_channel()))
            );
        }
        assert_eq!(
            source.lookups(),
            1,
            "the chain is asked once per channel, not once per packet"
        );
    }

    /// A declared Solana channel is authoritative: the source is never
    /// consulted for it, so a node whose config names its channels keeps
    /// accepting their claims while the RPC endpoint is down (the Solana
    /// twin of `a_declared_channel_is_never_looked_up`).
    #[tokio::test]
    async fn a_declared_solana_channel_is_never_looked_up() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::unreachable(
            "connection refused",
        ));
        let account = bs58::encode([7u8; 32]).into_string();
        let counterparty = bs58::encode([9u8; 32]).into_string();
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_solana(
                &account,
                &counterparty,
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .expect("a 32-byte base58 account");
        let registry = registry.with_solana_source(source.clone());

        assert_eq!(
            registry.solana(&[7u8; 32], A_BUYER).await,
            Ok(Some(SolanaChannel {
                program_id: [7u8; 32],
                counterparty: [9u8; 32],
                deposit_floor: DepositFloor::Unknown,
            }))
        );
        assert_eq!(source.lookups(), 0);
    }

    /// A lookup failure on the Solana source is reported as a failure, not
    /// silently absorbed into "no such channel" (the Solana twin of
    /// `a_lookup_failure_is_reported_as_a_failure`).
    #[tokio::test]
    async fn a_solana_lookup_failure_is_reported_as_a_failure() {
        let registry = ClientChannelRegistry::new().with_solana_source(Arc::new(
            super::test_source::FakeSolanaChannelSource::unreachable("connection refused"),
        ));

        assert_eq!(
            registry.solana(&[0x07; 32], A_BUYER).await,
            Err(ChannelLookupFailed("connection refused".to_string()).into())
        );
    }

    /// A Solana source registered under `ClaimChain::Solana` must never
    /// answer an EVM lookup for the same bytes -- the Solana-first twin of
    /// `an_evm_source_never_answers_a_solana_lookup_for_the_same_bytes`.
    #[tokio::test]
    async fn a_solana_source_never_answers_an_evm_lookup_for_the_same_bytes() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::knowing(vec![
            ([0x09; 32], solana_channel()),
        ]));
        let registry = ClientChannelRegistry::new().with_solana_source(source);

        assert_eq!(
            registry.solana(&[0x09; 32], A_BUYER).await,
            Ok(Some(solana_channel()))
        );
        assert_eq!(registry.evm(&[0x09; 32], A_BUYER).await, Ok(None));
    }

    // -- The deposit floor and its refresh (issues #646, #649) --

    /// A declared channel reports no deposit at all: config names a
    /// counterparty and a domain and never an amount, so the collateral cap
    /// has nothing to bind against and deliberately does not (issue #646).
    #[tokio::test]
    async fn a_declared_channel_has_no_knowable_deposit() {
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(
                &"ab".repeat(32),
                evm_channel_depositing(DepositFloor::Unknown),
            )
            .expect("a 32-byte hex channel id");
        registry
            .record_solana(
                &bs58::encode([3u8; 32]).into_string(),
                &bs58::encode([7u8; 32]).into_string(),
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .expect("a 32-byte base58 account");

        let evm = registry.evm(&[0xab; 32], A_BUYER).await.unwrap().unwrap();
        assert_eq!(evm.deposit_floor, DepositFloor::Unknown);
        assert!(evm.deposit_floor.covers(u64::MAX));

        let solana = registry.solana(&[3u8; 32], A_BUYER).await.unwrap().unwrap();
        assert_eq!(solana.deposit_floor, DepositFloor::Unknown);
    }

    /// The performance claim of issue #646, as a test rather than a
    /// paragraph: the deposit rides along with the resolution, so a channel
    /// whose claims all fit under the memoised floor never asks the chain
    /// again -- the steady state of a client that funded once and is
    /// spending down.
    #[tokio::test]
    async fn a_memoised_deposit_floor_costs_no_further_lookups() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel_depositing(DepositFloor::AtLeast(1_000)),
        )]));
        let registry = ClientChannelRegistry::new().with_source(source.clone());

        for _ in 0..5 {
            let channel = registry.evm(&[0x07; 32], A_BUYER).await.unwrap().unwrap();
            assert_eq!(channel.deposit_floor, DepositFloor::AtLeast(1_000));
        }
        assert_eq!(source.lookups(), 1);
    }

    /// The floor is a lower bound, not a reading: a counterparty who
    /// deposits more moves it, and one refresh -- exactly one -- is what
    /// finds that out.
    #[tokio::test]
    async fn a_refresh_raises_the_floor_to_what_the_chain_now_says() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel_depositing(DepositFloor::AtLeast(1_000)),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            // The subject here is the refresh itself, so nothing suppresses
            // it; the interval that would is measured on its own below.
            .with_liveness_policy(ChannelLivenessPolicy {
                min_reattempt_interval: Duration::ZERO,
                ..ChannelLivenessPolicy::default()
            });

        assert_eq!(
            registry
                .evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(1_000)
        );
        source.now_says(
            [0x07; 32],
            Some(evm_channel_depositing(DepositFloor::AtLeast(5_000))),
        );

        // Still the memoised floor: nothing has breached it, so nothing
        // re-reads.
        assert_eq!(
            registry
                .evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(1_000)
        );
        assert_eq!(source.lookups(), 1);

        assert_eq!(
            registry
                .refresh_evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(5_000)
        );
        assert_eq!(source.lookups(), 2, "exactly one re-read");

        // And the raised floor is what the next lookup answers from --
        // a refresh writes through the memo rather than around it.
        assert_eq!(
            registry
                .evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(5_000)
        );
        assert_eq!(source.lookups(), 2);
    }

    /// A source that answers `evm_channel` from a cache of its own (the
    /// issue #661 channel index) can hold a deposit floor the chain has
    /// since raised -- a top-up whose log is not yet confirmation-deep. A
    /// breach re-read that consulted the same cache again would refuse a
    /// claim the chain would honour, so the breach path must go through
    /// [`ClientChannelSource::evm_channel_fresh`], never
    /// [`ClientChannelSource::evm_channel`].
    #[tokio::test]
    async fn a_breach_re_read_bypasses_a_source_s_own_cache() {
        /// `evm_channel` plays the cache (stale floor); `evm_channel_fresh`
        /// plays the chain (the floor after the top-up).
        #[derive(Debug)]
        struct CachingSource;

        #[async_trait]
        impl ClientChannelSource for CachingSource {
            async fn evm_channel(
                &self,
                _channel_id: &[u8; 32],
            ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
                Ok(Some(evm_channel_depositing(DepositFloor::AtLeast(1_000))))
            }

            async fn evm_channel_fresh(
                &self,
                _channel_id: &[u8; 32],
            ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
                Ok(Some(evm_channel_depositing(DepositFloor::AtLeast(5_000))))
            }
        }

        // A long refresh window, so the ageing path serves the memo (which
        // is what lets the final assertion below see the write-through
        // rather than another cached read), and no re-attempt suppression,
        // so the breach re-read is observable immediately.
        let registry = ClientChannelRegistry::new()
            .with_source(Arc::new(CachingSource))
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::from_secs(3_600),
                serve_stale_until: Duration::from_secs(7_200),
                min_reattempt_interval: Duration::ZERO,
            });

        // An ordinary resolve is content with the source's cached answer.
        assert_eq!(
            registry
                .evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(1_000)
        );

        // A breach is not: it reaches the source's authority.
        assert_eq!(
            registry
                .refresh_evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(5_000)
        );

        // And the fresh floor writes through the memo, so the next packet
        // is served the corrected figure without another read.
        assert_eq!(
            registry
                .evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::AtLeast(5_000)
        );
    }

    /// Issue #649: a channel resolved while it was payable, which the chain
    /// later stops vouching for -- it settled, or its mint changed -- must
    /// stop being answered for out of the memo. On a cache that is never
    /// invalidated this returns the stale positive forever, and the
    /// connector goes on accepting claims that can never be redeemed.
    #[tokio::test]
    async fn a_channel_the_chain_stops_vouching_for_stops_resolving() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup());

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_says([0x07; 32], None);

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(None),
            "a settled channel is not answered for out of a cache that predates the settlement"
        );
    }

    /// The Solana twin of the above -- the chain #646 was actually observed
    /// on, and where a settled channel's PDA is closed outright.
    #[tokio::test]
    async fn a_solana_channel_the_chain_stops_vouching_for_stops_resolving() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::knowing(vec![
            ([0x07; 32], solana_channel()),
        ]));
        let registry = ClientChannelRegistry::new()
            .with_solana_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup());

        assert_eq!(
            registry.solana(&[0x07; 32], A_BUYER).await,
            Ok(Some(solana_channel()))
        );
        source.now_says([0x07; 32], None);

        assert_eq!(registry.solana(&[0x07; 32], A_BUYER).await, Ok(None));
    }

    /// Liveness expiry is what bounds how long the stale positive above can
    /// survive, and the default is a real cache rather than a per-packet
    /// read: within the window the chain is asked exactly once.
    #[tokio::test]
    async fn within_the_liveness_window_the_chain_is_asked_once() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new().with_source(source.clone());

        for _ in 0..5 {
            assert_eq!(
                registry.evm(&[0x07; 32], A_BUYER).await,
                Ok(Some(evm_channel()))
            );
        }
        assert_eq!(source.lookups(), 1);
        assert!(DEFAULT_LIVENESS_TTL > Duration::ZERO);
    }

    /// A channel that has stopped resolving is *removed* from the memo, not
    /// merely skipped once: the next packet must not find the stale
    /// positive sitting there again.
    #[tokio::test]
    async fn a_dropped_channel_is_not_still_in_the_memo() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                min_reattempt_interval: Duration::ZERO,
                ..ChannelLivenessPolicy::default()
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_says([0x07; 32], None);
        assert_eq!(registry.refresh_evm(&[0x07; 32], A_BUYER).await, Ok(None));

        // The default TTL has not expired, so this reads the memo -- which
        // must no longer hold the channel.
        assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));
    }

    /// A declared channel has nothing to refresh: config is its authority,
    /// and a refresh must not turn into the chain lookup #556 promised
    /// declared channels would never need.
    #[tokio::test]
    async fn refreshing_a_declared_channel_consults_no_chain() {
        let source = Arc::new(FakeChannelSource::unreachable("connection refused"));
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(
                &"07".repeat(32),
                evm_channel_depositing(DepositFloor::Unknown),
            )
            .expect("a 32-byte hex channel id");
        let registry = registry.with_source(source.clone());

        assert_eq!(
            registry
                .refresh_evm(&[0x07; 32], A_BUYER)
                .await
                .unwrap()
                .unwrap()
                .deposit_floor,
            DepositFloor::Unknown
        );
        assert_eq!(source.lookups(), 0);
    }

    /// A refresh whose lookup fails says nothing about the channel, so it
    /// must not evict what was memoised: a node whose RPC endpoint blips
    /// does not lose every channel it had resolved, and the client whose
    /// claim provoked the failed read is answered from the last reading
    /// this connector actually got rather than refused.
    ///
    /// The channel is resolved for real first and the chain fails
    /// afterwards -- the entry under test has to be one that genuinely
    /// came from a lookup, or this proves nothing about the path a real
    /// outage takes.
    #[tokio::test]
    async fn a_failed_refresh_leaves_the_memo_alone() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new().with_source(source.clone());
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );

        source.now_fails(Some("connection refused"));
        assert_eq!(
            registry.refresh_evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel())),
            "the last good reading, not a refusal"
        );
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
    }

    // -- Ageing out without a thundering herd (the availability review of
    // #654; see this module's own doc) --

    /// A resolved entry that has aged out, on a chain that has stopped
    /// answering, must not turn one channel's packet stream into one chain
    /// read per packet. Before the fix this measured 26 lookups for 25
    /// packets -- an outage that *raised* this node's RPC load, on the
    /// endpoint that was already failing.
    ///
    /// Note what is asserted besides the count: the packets are still
    /// **served**. Refusing them would make somebody else's outage into
    /// this node's own refusal to take money, which is strictly worse than
    /// the memo-with-no-expiry that shipped before the expiry existed.
    #[tokio::test]
    async fn an_outage_on_an_aged_out_channel_costs_one_lookup_not_one_per_packet() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                // Aged out immediately, so every one of the packets below
                // is a re-verification attempt...
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::from_secs(600),
                // ...and the interval is the only thing bounding them. Long
                // enough that the whole burst is inside one of them however
                // slow the machine running this is: the assertion below is
                // about work per interval, and it must not become an
                // assertion about how fast a loop ran.
                min_reattempt_interval: Duration::from_secs(600),
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(source.lookups(), 1, "the initial resolution");
        source.now_fails(Some("429 Too Many Requests"));

        for packet in 0..25 {
            assert_eq!(
                registry.evm(&[0x07; 32], A_BUYER).await,
                Ok(Some(evm_channel())),
                "packet {packet} is served from the last good reading"
            );
        }
        assert_eq!(
            source.lookups(),
            1,
            "25 packets inside one interval add no reads at all"
        );
    }

    /// ...and the interval is a bound on work, not a decision to stop
    /// trying: one packet past it re-attempts, exactly once. A single
    /// packet rather than a burst, so this measures the re-attempt and
    /// cannot accidentally measure how long a loop took.
    #[tokio::test]
    async fn a_packet_past_the_interval_re_attempts_once() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::from_secs(600),
                min_reattempt_interval: Duration::from_millis(20),
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_fails(Some("429 Too Many Requests"));

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(source.lookups(), 2, "one re-attempt, and it was made");
    }

    /// The Solana twin: the same measurement on the chain whose public RPC
    /// has the tighter per-method budget.
    #[tokio::test]
    async fn an_outage_on_an_aged_out_solana_channel_costs_one_lookup_not_one_per_packet() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::knowing(vec![
            ([0x07; 32], solana_channel()),
        ]));
        let registry = ClientChannelRegistry::new()
            .with_solana_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::from_secs(600),
                min_reattempt_interval: Duration::from_secs(600),
            });

        assert_eq!(
            registry.solana(&[0x07; 32], A_BUYER).await,
            Ok(Some(solana_channel()))
        );
        source.now_fails(Some("429 Too Many Requests"));
        for _ in 0..25 {
            assert_eq!(
                registry.solana(&[0x07; 32], A_BUYER).await,
                Ok(Some(solana_channel()))
            );
        }
        assert_eq!(source.lookups(), 1);
    }

    /// Past `serve_stale_until` there is nothing safe to lean on, so the
    /// refusal comes back -- the stale window is a bounded grace period,
    /// not a way to never re-verify. This is what keeps #649 true even
    /// through an outage: a channel that settles can be served stale for
    /// the window and no longer.
    #[tokio::test]
    async fn past_the_stale_window_an_unreachable_chain_refuses() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::ZERO,
                // Nothing suppresses the attempt, so the refusal below is
                // the chain's own answer rather than this node backing off
                // -- which is the case the two tests after this one are
                // about.
                min_reattempt_interval: Duration::ZERO,
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_fails(Some("connection refused"));

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Err(ChannelLookupFailed("connection refused".to_string()).into())
        );
    }

    /// Past `serve_stale_until` the interval must **still** bind, and this
    /// is the case it is easiest to talk oneself out of: there is nothing
    /// left to serve, so it looks like the moment to try hardest. It is the
    /// opposite. Reaching this state means the chain has already been
    /// failing for the whole stale window, and waiving the interval here
    /// reinstated exactly the per-packet storm the interval exists to
    /// remove -- measured at 25 lookups for 25 packets, ten minutes into an
    /// outage, against an endpoint that had been failing for ten minutes.
    ///
    /// The claim is refused either way. The only question is whether the
    /// refusal costs an RPC.
    #[tokio::test]
    async fn past_the_stale_window_a_burst_still_costs_one_lookup_not_one_per_packet() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                // Nothing may be served stale...
                serve_stale_until: Duration::ZERO,
                // ...and the whole burst is inside one interval, however
                // slow the machine running it.
                min_reattempt_interval: Duration::from_secs(600),
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_fails(Some("429 Too Many Requests"));

        for packet in 0..25 {
            assert!(
                registry.evm(&[0x07; 32], A_BUYER).await.is_err(),
                "packet {packet} is refused -- there is nothing safe to serve"
            );
        }
        assert_eq!(
            source.lookups(),
            1,
            "the initial resolution and nothing else: a refusal must not cost an RPC each"
        );

        // And it says which refusal it is, so an operator reading a log can
        // tell "my endpoint is down" from "my node is backing off".
        let Err(ChannelResolutionError::LookupFailed(failure)) =
            registry.evm(&[0x07; 32], A_BUYER).await
        else {
            panic!("expected a lookup failure, not a budget refusal");
        };
        assert!(failure.0.contains("backing off"), "{failure}");
    }

    /// The concurrency half of the same hole: 32 packets arriving together
    /// past the stale window cost one lookup between them, not 32. The
    /// in-flight marker has to bind here too -- it was bypassed along with
    /// the interval.
    #[tokio::test]
    async fn past_the_stale_window_concurrent_packets_still_share_one_lookup() {
        let source = Arc::new(
            FakeChannelSource::knowing(vec![([0x07; 32], evm_channel())])
                .taking(Duration::from_millis(300)),
        );
        let registry = Arc::new(
            ClientChannelRegistry::new()
                .with_source(source.clone())
                .with_liveness_policy(ChannelLivenessPolicy {
                    refresh_after: Duration::ZERO,
                    serve_stale_until: Duration::ZERO,
                    // Zero, so the in-flight marker is the only thing
                    // holding the herd back -- the interval is measured on
                    // its own above.
                    min_reattempt_interval: Duration::ZERO,
                }),
        );

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_fails(Some("429 Too Many Requests"));

        let barrier = Arc::new(tokio::sync::Barrier::new(32));
        let mut packets = Vec::new();
        for _ in 0..32 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            packets.push(tokio::spawn(async move {
                // Released only once all 32 are spawned and waiting, so
                // this is a genuine simultaneous arrival rather than a
                // hope about scheduling.
                barrier.wait().await;
                registry.evm(&[0x07; 32], A_BUYER).await
            }));
        }
        for packet in packets {
            assert!(packet.await.unwrap().is_err());
        }

        assert_eq!(
            source.lookups(),
            2,
            "one resolution and one re-read between 32 simultaneous packets"
        );
    }

    /// N packets arriving on one aged-out channel cost one lookup between
    /// them, not N: the entry is marked in-flight under the memo's own lock
    /// before the await, so the other 31 lean on the last good reading
    /// instead of piling onto the endpoint. This stampede is what produces
    /// the first 429 in the first place.
    #[tokio::test]
    async fn concurrent_packets_after_an_expiry_share_one_lookup() {
        let source = Arc::new(
            FakeChannelSource::knowing(vec![([0x07; 32], evm_channel())])
                // The barrier below guarantees the 32 packets *arrive*
                // together; this guarantees they are still in flight
                // together, since an instantaneous source would let each
                // lookup finish -- clearing the in-flight marker -- before
                // the next task is polled.
                .taking(Duration::from_millis(300)),
        );
        let registry = Arc::new(
            ClientChannelRegistry::new()
                .with_source(source.clone())
                .with_liveness_policy(ChannelLivenessPolicy {
                    refresh_after: Duration::ZERO,
                    serve_stale_until: Duration::from_secs(600),
                    // Zero, so nothing but the in-flight marker itself is
                    // holding the herd back.
                    min_reattempt_interval: Duration::ZERO,
                }),
        );

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(source.lookups(), 1);

        // A barrier rather than a slow source: every task is released only
        // once all 32 exist and are waiting, so this is a genuine
        // simultaneous arrival rather than a hope about how a runner
        // schedules them.
        let barrier = Arc::new(tokio::sync::Barrier::new(32));
        let mut packets = Vec::new();
        for _ in 0..32 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            packets.push(tokio::spawn(async move {
                barrier.wait().await;
                registry.evm(&[0x07; 32], A_BUYER).await
            }));
        }
        for packet in packets {
            assert_eq!(packet.await.unwrap(), Ok(Some(evm_channel())));
        }

        assert_eq!(
            source.lookups(),
            2,
            "one resolution and one re-verification between 32 concurrent packets"
        );
    }

    /// The intermediate boundary the ZERO-and-default tests never touch:
    /// an entry is served without a lookup while it is young, and
    /// re-verified once it is not. Real durations, so `refresh_after` is
    /// exercised as a comparison rather than as a degenerate `0`.
    #[tokio::test]
    async fn an_entry_is_re_verified_only_once_it_is_older_than_refresh_after() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::from_millis(500),
                serve_stale_until: Duration::from_secs(600),
                min_reattempt_interval: Duration::ZERO,
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        // Well inside the window: answered from the memo.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        assert_eq!(source.lookups(), 1);

        // Past it: re-verified, and the channel is dropped if the chain has
        // stopped vouching for it in the meantime.
        tokio::time::sleep(Duration::from_millis(600)).await;
        source.now_says([0x07; 32], None);
        assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));
        assert_eq!(source.lookups(), 2);
    }

    /// A lookup that fails does not reset the staleness clock: the entry
    /// keeps ageing towards `serve_stale_until` from the last *successful*
    /// reading, so a chain that is down for longer than the window stops
    /// being papered over rather than being served stale forever.
    #[tokio::test]
    async fn a_failed_attempt_does_not_count_as_a_confirmation() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::from_millis(500),
                min_reattempt_interval: Duration::ZERO,
            });

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        source.now_fails(Some("connection refused"));
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Err(ChannelLookupFailed("connection refused".to_string()).into()),
            "the stale window is measured from the last confirmation, not the last attempt"
        );
    }

    // -- Shaping the unresolvable lookup (issue #613) --

    /// A sender walking the id space: every request names a channel that
    /// has never been seen and never will resolve.
    fn a_nonexistent_channel(request: u32) -> [u8; 32] {
        let mut channel_id = [0xee; 32];
        channel_id[..4].copy_from_slice(&request.to_be_bytes());
        channel_id
    }

    /// A policy that shapes nothing, for a test whose subject is something
    /// else. Deliberately spelled out rather than hidden behind a
    /// constructor: a registry with no bound is exactly what #613 is about,
    /// and it should be conspicuous wherever it appears.
    fn unshaped() -> UnresolvableLookupBudgetPolicy {
        UnresolvableLookupBudgetPolicy {
            per_signer: u32::MAX,
            total: u32::MAX,
            window: Duration::from_secs(600),
            max_wait: Duration::ZERO,
        }
    }

    /// A policy shaped tightly enough to measure, with a wait ceiling of
    /// zero so that every decision is observable as an immediate pass or
    /// refusal rather than as a sleep -- which is what keeps these
    /// measurements schedule-independent.
    ///
    /// A consequence worth naming: with no wait ceiling there is no band in
    /// which a lookup queues, and the per-signer axis only ever bites
    /// *inside* that band (it is consulted only once the node-wide drain is
    /// in arrears, and with a zero ceiling the node refuses at exactly that
    /// point). So what these tests measure is the node-wide rate. The
    /// per-signer split, and the queueing itself, are measured in
    /// `crate::lookup_budget`, which can drive an exact clock.
    fn shaped(per_signer: u32, total: u32) -> UnresolvableLookupBudgetPolicy {
        UnresolvableLookupBudgetPolicy {
            per_signer,
            total,
            window: Duration::from_secs(600),
            max_wait: Duration::ZERO,
        }
    }

    /// The measurement that motivates the whole change, and the evidence
    /// that #654's machinery does **not** already cover it -- in its
    /// hardest form.
    ///
    /// `ChannelLivenessPolicy` is set as tight as it goes: every entry aged
    /// out, nothing servable stale, and a ten-minute `min_reattempt_interval`
    /// -- the configuration that reduces 200 packets on one *resolved*
    /// channel to a single lookup
    /// (`past_the_stale_window_a_burst_still_costs_one_lookup_not_one_per_packet`
    /// measures exactly that). And the sender here does not even vary the
    /// channel: it is **the same nonexistent id, two hundred times**, which
    /// is the case an interval keyed by channel ought to bound if anything
    /// could. It costs two hundred chain reads, because `resolve_evm`
    /// inserts a memo entry on `Some` and `remove`s on `None`, so there is
    /// never an entry for the interval to be recorded on.
    #[tokio::test]
    async fn the_same_nonexistent_channel_is_not_bounded_by_the_liveness_policy() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::ZERO,
                min_reattempt_interval: Duration::from_secs(600),
            })
            .with_lookup_budget(unshaped());

        for _ in 0..200 {
            assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));
        }
        assert_eq!(
            source.lookups(),
            200,
            "the liveness policy hangs every one of its bounds off a memo entry, and a channel \
             that resolves to nothing never gets one -- so it bounds this not at all, even for \
             one id repeated"
        );
    }

    /// ...and the same with a fresh id per request, which is the shape the
    /// attack actually takes and which no per-channel interval could ever
    /// bound.
    #[tokio::test]
    async fn a_walk_of_fresh_ids_is_not_bounded_by_the_liveness_policy_either() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy {
                refresh_after: Duration::ZERO,
                serve_stale_until: Duration::ZERO,
                min_reattempt_interval: Duration::from_secs(600),
            })
            .with_lookup_budget(unshaped());

        for request in 0..200 {
            assert_eq!(
                registry.evm(&a_nonexistent_channel(request), A_BUYER).await,
                Ok(None)
            );
        }
        assert_eq!(source.lookups(), 200);
    }

    /// The same walk with the shaper in place: 200 requests, and the chain
    /// is read for the sender's own rate only.
    ///
    /// The liveness policy is left at ZERO across the board, so nothing but
    /// the shaper is suppressing anything -- and the assertion is against
    /// the configured rate rather than a wall clock, so a slow runner
    /// cannot turn this into a race.
    #[tokio::test]
    async fn a_walk_of_the_channel_id_space_costs_a_bounded_number_of_lookups() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup())
            .with_lookup_budget(shaped(4, 8));

        let mut budgeted = 0;
        for request in 0..200 {
            match registry.evm(&a_nonexistent_channel(request), A_BUYER).await {
                Ok(None) => {}
                Err(ChannelResolutionError::Budgeted(_)) => budgeted += 1,
                other => panic!("unexpected answer to request {request}: {other:?}"),
            }
        }

        assert_eq!(
            source.lookups(),
            8,
            "one sender's walk costs the node-wide rate, not 200 chain reads"
        );
        assert_eq!(budgeted, 192, "every request past it says so");
    }

    /// The same walk by an *adaptive* sender, who declares a fresh signer
    /// on every request. This is the case the per-signer axis cannot touch
    /// -- a keypair is free, and the signer is read rather than
    /// authenticated -- and it is why there is a node-wide drain at all.
    #[tokio::test]
    async fn a_sybil_walk_is_bounded_by_the_node_wide_drain() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup())
            .with_lookup_budget(shaped(4, 8));

        for request in 0..200u32 {
            let _ = registry
                .evm(
                    &a_nonexistent_channel(request),
                    &format!("evm:0x{request:040x}"),
                )
                .await;
        }

        assert_eq!(
            source.lookups(),
            8,
            "200 requests under 200 identities still cost only the node-wide rate"
        );
    }

    /// **The property the first cut of this bound got wrong**, and the one
    /// that matters most: a flood must not switch the feature off.
    ///
    /// A sender saturates the drain completely. Under a bound that refuses
    /// at its ceiling, an honest buyer is then denied for as long as the
    /// flood lasts -- the whole feature turned off by whoever can afford
    /// one request per slot. Under a bound that shapes, the drain keeps
    /// draining, so the honest buyer arriving a few slots later is served.
    ///
    /// The elapsed time below is asserted in the safe direction: a slow
    /// runner drains *more*, never less, so this cannot become a race. The
    /// exact-clock version of the same property, with no wall time at all,
    /// is `crate::lookup_budget::a_flood_reopens_every_interval_rather_than_closing`.
    #[tokio::test]
    async fn a_flood_delays_an_honest_buyer_rather_than_denying_them() {
        let honest_channel = [0x77; 32];
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            honest_channel,
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            // Ten slots per 100 ms, i.e. one every 10 ms, and a 5 ms
            // ceiling -- so nothing here ever actually sleeps and the whole
            // test is decided by which side of the ceiling a slot falls on.
            .with_lookup_budget(UnresolvableLookupBudgetPolicy {
                per_signer: u32::MAX,
                total: 10,
                window: Duration::from_millis(100),
                max_wait: Duration::from_millis(5),
            });

        let mut refused = 0;
        for request in 0..200 {
            if registry
                .evm(&a_nonexistent_channel(request), "evm:0xflood")
                .await
                .is_err()
            {
                refused += 1;
            }
        }
        assert!(refused > 0, "the drain really is saturated");

        // Five slots' worth of drain later -- the queue has moved on, which
        // is the whole difference between shaping and dropping.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            registry.evm(&honest_channel, A_BUYER).await,
            Ok(Some(evm_channel())),
            "a flood must delay this buyer, never deny them -- a bound that refused here would \
             be the feature switched off by whoever can afford one request per slot"
        );
    }

    /// The case a negative cache would have broken, and the reason #611
    /// refused to build one: a buyer opens a channel and writes a second
    /// later, so their channel does not resolve on the first attempt and
    /// does on the second. They are served on that second attempt, with no
    /// TTL to wait out.
    ///
    /// Twenty such buyers in a row, and the rate is what makes the refund
    /// load-bearing rather than incidental. Each buyer costs one genuine
    /// miss -- their channel really was not there yet -- plus one
    /// resolution, which gives its slot back. So twenty buyers cost twenty
    /// slots, not forty, and a burst of twenty-five carries all of them.
    /// Without the refund the same twenty cost forty and the burst runs out
    /// around the twelfth.
    #[tokio::test]
    async fn a_buyer_whose_channel_appears_a_moment_later_is_served_on_their_next_attempt() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(2, 25));

        for buyer in 0..20 {
            let channel_id = a_nonexistent_channel(buyer);
            let signer = format!("evm:0x{buyer:040x}");

            // Attempt one: they opened the channel a moment ago and the
            // chain does not have it yet.
            assert_eq!(
                registry.evm(&channel_id, &signer).await,
                Ok(None),
                "buyer {buyer}'s channel is not on chain yet"
            );

            // ...and now it is. No TTL to wait out: the answer that could
            // change to a better one was never memoised.
            source.now_says(channel_id, Some(evm_channel()));
            assert_eq!(
                registry.evm(&channel_id, &signer).await,
                Ok(Some(evm_channel())),
                "buyer {buyer} is served on their very next attempt"
            );
        }

        assert_eq!(source.lookups(), 40, "one miss and one resolution each");
        assert_eq!(
            registry.unresolvable_lookups_queued_for(),
            Duration::ZERO,
            "and the drain is not in arrears: every resolution gave its own slot back"
        );
    }

    /// A node onboarding real anonymous buyers as fast as they arrive must
    /// not throttle itself, so a lookup that *resolves* costs nothing.
    /// Fifty distinct channels against a rate of one: every one is served,
    /// because every one gives its slot back.
    #[tokio::test]
    async fn resolving_real_channels_never_spends_the_budget() {
        let channels: Vec<([u8; 32], EvmChannel)> = (0..50)
            .map(|buyer| (a_nonexistent_channel(buyer), evm_channel()))
            .collect();
        let source = Arc::new(FakeChannelSource::knowing(channels));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(1, 1));

        for buyer in 0..50 {
            assert_eq!(
                registry
                    .evm(
                        &a_nonexistent_channel(buyer),
                        &format!("evm:0x{buyer:040x}")
                    )
                    .await,
                Ok(Some(evm_channel())),
                "buyer {buyer} resolves, and a resolution is not an unresolvable lookup"
            );
        }
        assert_eq!(source.lookups(), 50);
        assert_eq!(registry.unresolvable_lookups_queued_for(), Duration::ZERO);
    }

    /// Two senders' rates are independent, and the split is measured with
    /// an exact clock in `crate::lookup_budget`'s
    /// `two_signers_have_independent_allowances` rather than here: the
    /// per-signer axis only bites inside the queueing band (see `shaped`),
    /// and reproducing that band against a real clock would be a race
    /// rather than a measurement. What is asserted here is the seam --
    /// that the registry passes each claim's own declared signer through,
    /// so two senders reach two different buckets at all.
    #[tokio::test]
    async fn each_sender_reaches_its_own_bucket() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(1, 2));

        // Two senders, one node-wide slot each.
        assert_eq!(
            registry
                .evm(&a_nonexistent_channel(0), "evm:0xaaaaaaaa")
                .await,
            Ok(None)
        );
        assert_eq!(
            registry
                .evm(&a_nonexistent_channel(1), "evm:0xbbbbbbbb")
                .await,
            Ok(None)
        );
        assert_eq!(source.lookups(), 2);
    }

    /// Exhaustion is its own answer. `Ok(None)`, a `LookupFailed` and a
    /// `Budgeted` all refuse the claim above this, and an operator has to
    /// be able to tell them apart -- "there is no such channel" needs
    /// nothing done, "the chain did not answer" needs an endpoint fixed,
    /// and "I declined to ask" needs somebody looked at.
    #[tokio::test]
    async fn an_exhausted_budget_is_neither_an_absent_channel_nor_a_lookup_failure() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(1, 1));

        // "No such channel", which spends the whole burst.
        assert_eq!(registry.evm(&[0x07; 32], A_BUYER).await, Ok(None));

        let budgeted = registry
            .evm(&[0x08; 32], A_BUYER)
            .await
            .expect_err("the drain is saturated");
        assert!(
            matches!(
                budgeted,
                ChannelResolutionError::Budgeted(LookupBudgetExhausted {
                    bound: LookupBudgetBound::Node,
                    allowance: 1,
                    ..
                })
            ),
            "{budgeted:?}"
        );
        assert_eq!(
            source.lookups(),
            1,
            "the refusal cost no chain read -- that is the whole point of it"
        );
    }

    /// **Issue #613's own acceptance criterion**: a node whose settlement
    /// endpoint is genuinely unreachable must degrade *loudly* rather than
    /// looking like an attack.
    ///
    /// That is not automatic, and getting it wrong is easy: a failed lookup
    /// consumes its slot, so an outage saturates the drain within seconds
    /// and every refusal after that would read as a budget. So while the
    /// last lookup this node actually completed came back a failure, that
    /// failure is what a refusal reports -- for as long as the outage
    /// lasts, however saturated the drain is.
    #[tokio::test]
    async fn an_unreachable_chain_never_reads_as_a_budget() {
        let source = Arc::new(FakeChannelSource::unreachable("connection refused"));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(3, 3));

        for request in 0..3 {
            assert_eq!(
                registry.evm(&a_nonexistent_channel(request), A_BUYER).await,
                Err(ChannelLookupFailed("connection refused".to_string()).into()),
                "request {request} reports the outage, in the endpoint's own words"
            );
        }

        // The drain is saturated now -- and every refusal still names the
        // outage, because the outage is what is actually wrong.
        for request in 3..40 {
            assert_eq!(
                registry.evm(&a_nonexistent_channel(request), A_BUYER).await,
                Err(ChannelLookupFailed("connection refused".to_string()).into()),
                "request {request} must not read as rate-limiting while the chain is down"
            );
        }
        assert_eq!(
            source.lookups(),
            3,
            "and the shaper still did its job: the dead endpoint was not re-asked 40 times"
        );
    }

    /// Issue #613's review, finding F: a node with both `[settlement.evm]`
    /// and `[settlement.solana]` has two endpoints that fail independently,
    /// so the outage precedence above must be keyed by chain. With one slot
    /// shared between them, a Solana outage made an *EVM* refusal quote the
    /// Solana endpoint's error -- a worse diagnosis than the one it
    /// replaced, because it names a real outage that has nothing to do with
    /// the claim being refused.
    #[tokio::test]
    async fn one_chains_outage_does_not_explain_the_other_chains_refusal() {
        let evm = Arc::new(FakeChannelSource::knowing(vec![]));
        let solana = Arc::new(super::test_source::FakeSolanaChannelSource::unreachable(
            "solana rpc: connection refused",
        ));
        let registry = ClientChannelRegistry::new()
            .with_source(evm.clone())
            .with_solana_source(solana.clone())
            .with_lookup_budget(shaped(2, 2));
        let buyer = "evm:0xdddddddddddddddddddddddddddddddddddddddd";

        // Solana's endpoint is down, and says so.
        assert_eq!(
            registry.solana(&[0x01; 32], buyer).await,
            Err(ChannelLookupFailed("solana rpc: connection refused".to_string()).into())
        );
        // EVM's is fine -- it simply has no such channel. That spends the
        // rest of the shared node-wide drain.
        assert_eq!(registry.evm(&[0x02; 32], buyer).await, Ok(None));

        // Now the drain is saturated. The EVM refusal must be a budget: EVM
        // is not the chain that is down.
        assert!(
            matches!(
                registry.evm(&[0x03; 32], buyer).await,
                Err(ChannelResolutionError::Budgeted(_))
            ),
            "an EVM refusal must not borrow the Solana endpoint's outage"
        );
        // ...and the Solana refusal still is the outage, because Solana is.
        assert_eq!(
            registry.solana(&[0x04; 32], buyer).await,
            Err(ChannelLookupFailed("solana rpc: connection refused".to_string()).into())
        );
    }

    /// The hold, end to end through the registry rather than through the
    /// shaper alone: a lookup past the burst is *held* and then reaches the
    /// chain, which is what makes "delayed, not denied" a fact about the
    /// packet path rather than about a number.
    ///
    /// `start_paused`, so the hold is real and instant -- and so this also
    /// stands as the proof that mounting this crate needs a time-enabled
    /// runtime.
    #[tokio::test(start_paused = true)]
    async fn a_held_lookup_still_reaches_the_chain() {
        let honest_channel = [0x77; 32];
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            honest_channel,
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            // One slot a second, and a two-second ceiling, so the arrival
            // after the burst is held rather than refused.
            .with_lookup_budget(UnresolvableLookupBudgetPolicy {
                per_signer: u32::MAX,
                total: 60,
                window: Duration::from_secs(60),
                max_wait: Duration::from_secs(2),
            });

        for request in 0..60 {
            assert_eq!(
                registry
                    .evm(&a_nonexistent_channel(request), "evm:0xflood")
                    .await,
                Ok(None),
                "burst request {request}"
            );
        }

        let started = tokio::time::Instant::now();
        assert_eq!(
            registry.evm(&honest_channel, A_BUYER).await,
            Ok(Some(evm_channel())),
            "the honest buyer is held for a slot and then served"
        );
        let waited = tokio::time::Instant::now().duration_since(started);
        assert!(
            waited >= Duration::from_millis(900),
            "and the hold really happened: {waited:?}"
        );
        assert_eq!(source.lookups(), 61, "the held lookup did reach the chain");
    }

    /// ...and once the endpoint comes back, a refusal is a budget again.
    /// The outage precedence must not outlive the outage, or a node that
    /// recovered would report an attack as an endpoint problem forever.
    #[tokio::test]
    async fn a_recovered_chain_reports_a_budget_again() {
        let source = Arc::new(FakeChannelSource::unreachable("connection refused"));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(2, 2));

        assert!(registry.evm(&[0x01; 32], A_BUYER).await.is_err());
        source.now_fails(None);
        assert_eq!(registry.evm(&[0x02; 32], A_BUYER).await, Ok(None));

        assert!(
            matches!(
                registry.evm(&[0x03; 32], A_BUYER).await,
                Err(ChannelResolutionError::Budgeted(_))
            ),
            "a completed, successful lookup clears the outage the refusal was deferring to"
        );
    }

    /// A declared `[[client_channels]]` channel is answered from config and
    /// never resolved, so it can never spend this drain -- which is what
    /// keeps an operator's own hand-declared buyers working while a walk of
    /// the id space is in progress.
    #[tokio::test]
    async fn a_declared_channel_never_spends_the_lookup_budget() {
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        let mut registry = ClientChannelRegistry::new();
        registry
            .record_evm(&"07".repeat(32), evm_channel())
            .expect("a 32-byte hex channel id");
        let registry = registry
            .with_source(source.clone())
            .with_lookup_budget(shaped(1, 1));

        // Saturate the drain on a channel that does not exist...
        assert_eq!(registry.evm(&[0x09; 32], A_BUYER).await, Ok(None));

        // ...and the declared channel is unaffected, however many times it
        // is asked for.
        for _ in 0..10 {
            assert_eq!(
                registry.evm(&[0x07; 32], A_BUYER).await,
                Ok(Some(evm_channel()))
            );
        }
        assert_eq!(source.lookups(), 1);
    }

    /// An already-resolved channel is served from the memo without
    /// consuming anything here, so a client that is already paying is not
    /// affected by a drain somebody else saturated. This is the bound on
    /// the blast radius.
    #[tokio::test]
    async fn an_already_resolved_channel_is_unaffected_by_a_saturated_drain() {
        let source = Arc::new(FakeChannelSource::knowing(vec![(
            [0x07; 32],
            evm_channel(),
        )]));
        let registry = ClientChannelRegistry::new()
            .with_source(source.clone())
            .with_lookup_budget(shaped(1, 1));

        assert_eq!(
            registry.evm(&[0x07; 32], A_BUYER).await,
            Ok(Some(evm_channel()))
        );
        // A stranger saturates the node's whole drain.
        assert_eq!(
            registry
                .evm(
                    &[0x09; 32],
                    "evm:0xcccccccccccccccccccccccccccccccccccccccc"
                )
                .await,
            Ok(None)
        );

        for _ in 0..10 {
            assert_eq!(
                registry.evm(&[0x07; 32], A_BUYER).await,
                Ok(Some(evm_channel())),
                "a channel already resolved is served from the memo, drain or no drain"
            );
        }
    }

    /// The Solana twin of the bound: the same walk, on the chain whose
    /// public RPC has the tighter per-method budget and where a resolution
    /// is one account read rather than two -- cheaper to absorb, and not
    /// one bit less unbounded without this.
    #[tokio::test]
    async fn a_walk_of_the_solana_account_space_costs_a_bounded_number_of_lookups() {
        let source = Arc::new(super::test_source::FakeSolanaChannelSource::knowing(vec![]));
        let registry = ClientChannelRegistry::new()
            .with_solana_source(source.clone())
            .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup())
            .with_lookup_budget(shaped(4, 8));
        let buyer = "solana:So11111111111111111111111111111111111111112";

        for request in 0..200 {
            let _ = registry
                .solana(&a_nonexistent_channel(request), buyer)
                .await;
        }
        assert_eq!(source.lookups(), 8);
        assert!(matches!(
            registry.solana(&a_nonexistent_channel(200), buyer).await,
            Err(ChannelResolutionError::Budgeted(_))
        ));
    }

    /// Concurrency does not get around the bound. The slot is claimed under
    /// the shaper's own lock *before* the chain is touched, which matters
    /// here more than anywhere else: an unresolvable channel has no memo
    /// entry, so #654's in-flight marker -- the thing that holds back a
    /// stampede on a channel that *has* resolved -- has nothing to be
    /// written on. A hundred requests released together, and the rate still
    /// holds.
    #[tokio::test]
    async fn simultaneous_unresolvable_lookups_do_not_overshoot_the_budget() {
        let source = Arc::new(
            FakeChannelSource::knowing(vec![])
                // Slow, so all hundred are genuinely in flight at once
                // rather than completing one at a time.
                .taking(Duration::from_millis(100)),
        );
        let registry = Arc::new(
            ClientChannelRegistry::new()
                .with_source(source.clone())
                .with_lookup_budget(shaped(8, 8)),
        );

        let barrier = Arc::new(tokio::sync::Barrier::new(100));
        let mut requests = Vec::new();
        for request in 0..100 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            requests.push(tokio::spawn(async move {
                barrier.wait().await;
                registry
                    .evm(&a_nonexistent_channel(request), A_BUYER)
                    .await
                    .is_ok()
            }));
        }
        let mut looked_up = 0;
        for request in requests {
            if request.await.unwrap() {
                looked_up += 1;
            }
        }

        assert_eq!(
            looked_up, 8,
            "the rate binds a burst as it binds a sequence"
        );
        assert_eq!(source.lookups(), 8);
    }
}
