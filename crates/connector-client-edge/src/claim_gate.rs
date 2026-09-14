//! Claim ingest gate for the client edge (`docs/protocol/client-edge-spec.md`
//! §1.3, issues #504, #522, #506/#544, #558): turns the
//! `ILP-Payment-Channel-Claim` (`-Wrapped`) header's already-decoded JSON
//! into a structurally valid, fresh, value-covering, cryptographically
//! verified [`ClientClaim`], or a documented refusal -- structure, then
//! freshness/watermark, then value binding against the matched route's
//! price, then (last, and only once all three have passed) the claim's
//! signature against its channel's counterparty: a replay or an
//! underpayment is refused before this ingress ever spends a signature
//! check on it.
//!
//! One step follows the signature rather than preceding it (issue #646,
//! spec §1.3 step 5): **collateral binding**, the rule that a claim may not
//! name more than its channel's counterparty has deposited on chain. It
//! sits last because it is the only check that can cost a chain read, and
//! #544's ordering promise is about what a *bad* claim costs -- so only a
//! claim already fresh, value-covering and correctly signed can provoke
//! one. See [`check_collateral`].
//!
//! Reuses `connector_domain`'s pure nonce/watermark/value rules
//! ([`connector_domain::validate_claim`], [`connector_domain::validate_price`],
//! [`connector_domain::advance_watermark`]) exactly as the peer semantics's own
//! `connector_runtime::ClaimBook` does for the first two -- this is a
//! second *state* around the same rules, not a second set of rules. The
//! state is deliberately separate from `ClaimBook`: a client-edge claim's
//! channel is never a peer channel, and (unlike `ClaimBook::accept_inbound`)
//! a watermark advance here is gated behind a signature verification, on the
//! `ClientClaimGate`'s own claim-native scheme (EIP-712 for EVM, Ed25519 for
//! Solana -- `connector_signer::claim_signature`), not `ClaimBook`'s
//! chain-agnostic internal digest.
//!
//! **What "verified" means here** (issue #558): a claim's signature must
//! recover to the counterparty this connector has recorded for the channel
//! the claim names -- client-edge-spec.md §1.3 step 4 in full -- looked up
//! in the [`ClientChannelRegistry`] this gate is built with. A claim's own
//! `signerAddress`/`signerPublicKey` is not consulted, and neither is the
//! EIP-712 domain it declares for itself: a claim gets no say in what it is
//! checked against, or a forger would simply sign their own bytes with
//! their own key and declare themself the payer. A claim naming a channel
//! this connector has no record of is refused as
//! [`ClaimIngestRejection::UnknownChannel`], distinguishably from a bad
//! signature and from an underpayment -- there is nothing to verify it
//! against, and "unverifiable" is never "accepted". No configuration, flag
//! or build profile falls back to the claim's self-declared signer.
//!
//! **Where that record comes from** (issue #556): the registry answers
//! from what the config file declared, or -- for a channel nothing
//! declared -- from the chain, via a [`crate::ClientChannelSource`]. That
//! resolution is the one part of this gate that can do I/O, which is why
//! [`ClientClaimGate::ingest`] is `async`. A resolution that *fails* is
//! [`ClaimIngestRejection::ChannelLookupFailed`]: the claim is refused,
//! loudly and distinguishably from a channel that genuinely does not
//! exist, and under no circumstance falls back to trusting it.
//!
//! **What survives a restart** (issue #605): every watermark this gate
//! advances is written to a [`Journal`] -- the same ADR 0005 port the peer
//! wire's own `connector_runtime::ClaimBook` persists its watermarks
//! through, and the same [`JournalEntry::InboundClaimAccepted`] alphabet,
//! rather than a second persistence mechanism invented for this edge. A
//! gate can only be built by [`ClientClaimGate::restore`], which replays
//! that journal before serving anything, so a process that restarts
//! resumes at the watermark it left off at instead of at `None` -- and
//! `validate_claim(None, ..)` accepts any nonce, which makes every claim
//! the client already spent free service again. Two consequences are
//! deliberate and load-bearing:
//!
//! * A journal that cannot be read, or that has a line this build cannot
//!   decode, is an error out of [`ClientClaimGate::restore`] -- the node
//!   refuses to start rather than starting from zero, since starting from
//!   zero is precisely the defect.
//! * Every watermark -- live and journaled -- is filed under the
//!   *canonical* channel key `connector_domain::client_claim::canonical_channel_key`
//!   produces, never the literal text a claim happened to arrive with
//!   (issue #643). One channel has many spellings (`channelId` is
//!   case-insensitive hex), every other stage of this gate already treats
//!   them as one channel, and a watermark that did not would hand a
//!   client a fresh empty watermark per spelling -- one signed claim,
//!   accepted once per casing. [`replay_watermarks`] canonicalises on the
//!   way *out* of the journal too, so a node upgrading onto this build
//!   recovers the watermarks its existing journal already holds instead
//!   of orphaning them at a key nothing looks up any more.
//! * A claim whose acceptance cannot be made durable is **refused**
//!   ([`ClaimIngestRejection::NotDurable`]) and advances nothing, rather
//!   than accepted against an in-memory watermark a crash would erase.
//!   Since issue #686 the journal append is **group-committed**: the
//!   write lock covers only the authoritative re-check, the watermark
//!   advance and enqueueing the entry with a dedicated committer thread
//!   -- microseconds, no I/O -- and the committer batches everything
//!   queued into one journal write and one fsync
//!   ([`Journal::append_batch`]). Enqueueing under the lock is what keeps
//!   journal order identical to watermark order, so a replay still
//!   reconstructs exactly the state the live gate held; and a claim is
//!   only handed back for its packet to be routed once the committer
//!   reports its batch durable, so ADR 0005's "journal written before
//!   value is considered moved" still holds at the only boundary it ever
//!   protected -- no service is rendered against an unfsync'd watermark.
//!   A batch that cannot be made durable is rolled back: under the same
//!   write lock every admission is decided under, every channel a failed
//!   entry touched is restored to its watermark before the earliest
//!   failed claim, and every waiting claim is refused as
//!   [`ClaimIngestRejection::NotDurable`] -- so the refusal's contract is
//!   unchanged, and the same claim resubmitted once the journal is
//!   writable again is still good.
//!
//! **The watermark key deliberately does not include the client-edge
//! sender identity `resolve_identity` produces (issue #502).**
//! client-edge-spec.md §1.3 describes the freshness rule as keyed by a
//! "(peer, blockchain, channel) tuple", and this gate keys it by
//! `(blockchain, channel)` alone (the *canonical* [`ClientClaim::channel_key`]
//! discussed above) -- reading "peer" there as the channel's own recorded
//! counterparty ([`crate::ClientChannelRegistry`], issue #558), not as the
//! HTTP-layer identity #502 resolves. The channel already names its one
//! counterparty; there is no second "peer" dimension a channel's watermark
//! could vary over. Folding the resolved `SenderIdentity` into this key
//! instead would let one channel hold a distinct watermark per identity
//! that happened to present it, which is not a second layer of safety --
//! it *reopens* the replay this watermark exists to close: a nonce this
//! gate already accepted under one presented `ILP-Peer-Id` (or anonymously)
//! would read as fresh again under a different one, since a self-declared
//! HTTP header, unlike the channel a claim cryptographically names, proves
//! nothing about who is presenting it.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, RwLock};

use chrono::{DateTime, Utc};

use connector_domain::client_claim::{
    canonical_channel_key, parse_client_claim, ClientClaim, ClientClaimError, EvmClientClaim,
    SolanaClientClaim, EVM_NAMESPACE, SOLANA_NAMESPACE,
};
use connector_domain::{
    advance_watermark, validate_claim, validate_price, ClaimError, JournalEntry, Watermark,
};
use connector_runtime::{ChannelDomain, Journal, JournalError, WireClaim};
use connector_signer::{verify_evm_balance_proof, verify_solana_balance_proof, EvmBalanceProof};

use crate::channels::{
    decode_base58_bytes, decode_hex_bytes, ChannelResolutionError, ClientChannelRegistry,
    DepositFloor,
};
use crate::lookup_budget::LookupBudgetBound;
use crate::outbound_ledger::ClientPayoutLedger;

/// Why the gate refused a claim. [`ClaimIngestRejection::Mina`] and
/// [`ClaimIngestRejection::Malformed`] are kept distinct on purpose: the
/// acceptance criteria requires a Mina claim's refusal to be distinguishable
/// from a merely malformed one; [`ClaimIngestRejection::Underpayment`] is
/// kept distinct from both for the same reason (issue #522);
/// [`ClaimIngestRejection::SignatureInvalid`] is kept distinct from all of
/// them for the same reason again (issue #506/#544) -- a claim that fails
/// cryptographic verification is neither stale, malformed nor underpaying;
/// and [`ClaimIngestRejection::UnknownChannel`] is kept distinct from
/// *those* for the same reason once more (issue #558) -- a claim naming a
/// channel this connector has no record of has not failed verification, it
/// could not be verified at all, and the two must not be reported as the
/// same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimIngestRejection {
    Malformed(String),
    Mina,
    NonceNotAdvancing,
    AmountNotAdvancing,
    Underpayment {
        advanced: u64,
        price: u64,
    },
    /// The claim names a channel this connector has no counterparty
    /// recorded for (issue #558), so there is no key its signature could
    /// be checked against. Matches the peer semantics's own
    /// `connector_runtime::ClaimRejectReason::UnknownChannel`.
    UnknownChannel,
    /// The claim names a channel a [`crate::ClientChannelSource`] has a
    /// durable, definitive record of having settled (issue #661's local
    /// channel index) -- reported without a chain read, and kept distinct
    /// from [`ClaimIngestRejection::UnknownChannel`] for the same reason
    /// every variant here is kept distinct from its neighbours: "this
    /// channel is done" is a stronger, more actionable fact than "this
    /// connector has no record of it", and conflating the two would send an
    /// operator investigating a buyer's genuinely spent channel to go
    /// looking for a registration problem instead.
    ChannelTerminal(String),
    /// This connector could not find out who the claim's channel belongs
    /// to (issue #556) -- its [`crate::ClientChannelSource`] failed, e.g.
    /// an unreachable RPC endpoint. Distinct from
    /// [`ClaimIngestRejection::UnknownChannel`] on purpose: that one is a
    /// fact about the channel, this one is a failure of this connector's,
    /// and reporting an outage as "no such channel" would tell a
    /// legitimate payer to go away for a reason that is not true. Both
    /// refuse the claim -- an unverifiable claim is never accepted.
    ChannelLookupFailed(String),
    /// The claim names a channel this connector has no record of, and it
    /// **declined to ask the chain** about it, because its budget for
    /// lookups that do not resolve is spent (issue #613).
    ///
    /// Kept distinct from both of the refusals above it, and the reason is
    /// the same one that keeps those two apart from each other: they lead
    /// an operator to three different actions.
    /// [`ClaimIngestRejection::UnknownChannel`] is a fact about the
    /// channel and needs nothing done;
    /// [`ClaimIngestRejection::ChannelLookupFailed`] says this node's
    /// settlement endpoint is not answering and needs fixing; this one says
    /// this node is deliberately withholding a chain read, because an
    /// unaffiliated sender can ask for one for free and something has been
    /// asking a great deal. Reporting any of the three as another would
    /// send an operator, or a payer, to fix the wrong thing.
    ///
    /// The only *temporary* refusal here besides
    /// [`ClaimIngestRejection::NotDurable`], and for the same reason:
    /// nothing is wrong with the claim. A buyer caught by a node-wide
    /// window somebody else spent is told to wait rather than told their
    /// channel does not exist.
    LookupBudgetExhausted {
        /// Which axis was saturated -- the enum rather than its `&str`
        /// spelling, so a caller matching on this cannot mistype a bound
        /// that does not exist.
        bound: LookupBudgetBound,
        allowance: u32,
        window_secs: u64,
        max_wait_ms: u64,
    },
    SignatureInvalid,
    /// The claim is fresh, well-formed, correctly signed and covers the
    /// route's price -- and names a cumulative amount larger than its
    /// channel's counterparty has actually deposited on chain (issue
    /// #646), so it could never be redeemed: `TokenNetwork.claimFromChannel`
    /// reverts `InsufficientChannelBalance` and
    /// `packages/solana-program`'s claim handler returns
    /// `TransferredAmountExceedsDeposit`. Accepting it would not be taking
    /// a credit risk this connector might win; it would be doing work it
    /// can provably never be paid for.
    ///
    /// Kept distinct from [`ClaimIngestRejection::Underpayment`] for
    /// exactly the reason every other variant here is kept distinct: this
    /// claim *does* cover the price, and telling a payer it underpaid
    /// would send them to fix the wrong thing. The remedy is the one both
    /// contracts already document -- deposit more and resubmit the same
    /// claim, which nothing here has consumed.
    ///
    /// As of issue #700, the ceiling this compares against is `deposited`
    /// **plus** whatever this connector has separately credited the same
    /// counterparty (a signed, unredeemed payout claim on this channel --
    /// see `ClientPayoutLedger`), so a claim reaching this variant has
    /// already failed against that raised ceiling too. `deposited` still
    /// reports only the on-chain figure -- an honest fact about the
    /// channel, unlike a combined number this connector alone vouches
    /// for -- so "deposit at least `claimed`" remains true remedial
    /// advice regardless of how much credit was already netted in.
    Undercollateralized {
        claimed: u64,
        deposited: u64,
    },
    /// The claim was structurally valid, fresh, value-covering and
    /// correctly signed -- and this connector could not durably record
    /// having accepted it (issue #605). Kept distinct from every refusal
    /// above for the same reason they are kept distinct from each other:
    /// nothing is wrong with the claim, so a sender must not be told its
    /// claim was invalid, and the same claim resubmitted once this
    /// connector's journal is writable again is still good. This is the
    /// only refusal here that is this connector's own fault, and the only
    /// one answered as a temporary (`T00`) rather than a final error.
    NotDurable,
    WrapUnsupported,
    WrapFailed(String),
    /// A Solana claim declares a `cluster` this node is not on (issue
    /// #975). Checked before the claim's channel is resolved -- like
    /// [`ClaimIngestRejection::UnknownChannel`], it is a fact this
    /// connector already knows without spending a chain read.
    ///
    /// This is a refusal about the **record**, not about the money, and
    /// that distinction is the whole of why it exists. ADR 0053 already
    /// closed the money half: a Solana balance proof signs the settlement
    /// program, taken from the resolved channel and never from the claim,
    /// so a signature made for one deployment cannot be replayed against
    /// another. What no signature can close is `cluster`, because a Solana
    /// program cannot know which cluster it runs on and so could never
    /// rebuild a message containing one. The field is therefore
    /// unsignable-by-construction, declared by the payer, and -- until this
    /// refusal -- never read. A node that accepts a claim labelled with a
    /// chain it is not on has endorsed that label, and the payer keeps the
    /// endorsed artifact.
    ///
    /// A struct variant rather than a formatted string so the two sides are
    /// separately countable: "which cluster am I being told I am on" is a
    /// dimension an operator wants to group by, and a metric derived from a
    /// string is a metric derived from prose.
    SolanaClusterMismatch {
        declared: String,
        configured: &'static str,
    },
}

impl ClaimIngestRejection {
    /// A human-readable reason, carried in the REJECT packet's `message`
    /// (RFC-0027) so a client can tell what went wrong without access to
    /// this connector's logs.
    pub fn message(&self) -> String {
        match self {
            ClaimIngestRejection::Malformed(reason) => {
                format!("claim rejected: structurally invalid: {reason}")
            }
            ClaimIngestRejection::Mina => "claim rejected: mina claims are refused -- ADR 0002 \
                 drops Mina support from the Rust connector; stay on the TypeScript fleet for \
                 Mina channels"
                .to_string(),
            ClaimIngestRejection::NonceNotAdvancing => {
                "claim rejected: nonce does not advance this channel's watermark (replay)"
                    .to_string()
            }
            ClaimIngestRejection::AmountNotAdvancing => "claim rejected: cumulative amount goes \
                 backwards relative to this channel's watermark"
                .to_string(),
            ClaimIngestRejection::Underpayment { advanced, price } => format!(
                "claim rejected: advances value by {advanced}, less than this route's price of {price}"
            ),
            ClaimIngestRejection::UnknownChannel => "claim rejected: names a channel this \
                 connector has no record of, so there is no counterparty to verify its \
                 signature against"
                .to_string(),
            // The reason is already the whole sentence -- it names the
            // channel and says what became of it -- so it is quoted rather
            // than prefaced with a second copy of itself.
            ClaimIngestRejection::ChannelTerminal(reason) => {
                format!("claim rejected: {reason}")
            }
            ClaimIngestRejection::ChannelLookupFailed(reason) => format!(
                "claim rejected: this connector could not look up the channel's counterparty, \
                 so the claim cannot be verified -- retry once the lookup succeeds: {reason}"
            ),
            ClaimIngestRejection::LookupBudgetExhausted {
                bound,
                allowance,
                window_secs,
                max_wait_ms,
            } => format!(
                "claim rejected: this connector has no record of the channel and could not look \
                 it up in time -- its {} discovery drain of {allowance} lookups per \
                 {window_secs} s for channels that do not resolve is saturated, and the queue for \
                 it is longer than the {max_wait_ms} ms it will hold a lookup for. Nothing is \
                 wrong with the claim; retry",
                bound.as_str()
            ),
            ClaimIngestRejection::SignatureInvalid => "claim rejected: signature does not \
                 verify against this channel's recorded counterparty"
                .to_string(),
            ClaimIngestRejection::Undercollateralized { claimed, deposited } => format!(
                "claim rejected: claims a cumulative {claimed}, more than the {deposited} this \
                 channel's counterparty has deposited on chain, so it could never be redeemed -- \
                 deposit at least {claimed} and resubmit this same claim"
            ),
            ClaimIngestRejection::NotDurable => "claim rejected: this connector could not \
                 durably record having accepted this claim, and will not accept a claim it \
                 could not remember spending -- retry"
                .to_string(),
            ClaimIngestRejection::WrapUnsupported => "claim rejected: this connector is not \
                 configured to unwrap a privacy-wrapped claim"
                .to_string(),
            ClaimIngestRejection::WrapFailed(reason) => {
                format!("claim rejected: failed to unwrap claim: {reason}")
            }
            ClaimIngestRejection::SolanaClusterMismatch {
                declared,
                configured,
            } => format!(
                "claim rejected: it declares cluster '{declared}', but this connector settles \
                 on '{configured}' -- a claim naming a chain this connector is not on is wrong, \
                 not merely unverifiable"
            ),
        }
    }
}

/// A channel's live watermark together with the exact claim bytes that
/// produced it (issue #1218): a watermark alone says what was spent, but
/// only the signature is redeemable -- the same reason the peer semantics's
/// own `connector_runtime::ClaimBook` (via `connector_domain::Projection`'s
/// `inbound_claim_signature`) retains one alongside its watermark rather
/// than discarding it once the acceptance decision is made.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveClaim {
    watermark: Watermark,
    signature: Vec<u8>,
}

/// Per-channel watermark state for claims presented at the client edge,
/// over the channels this connector has a record of -- durable across a
/// restart, since a watermark that only lives in this process is not a
/// replay defence at all (issue #605). See this module's own doc.
pub struct ClientClaimGate {
    /// Whose signature this gate accepts, per channel (issue #558). Fixed
    /// at construction rather than mutable behind the lock: a channel's
    /// counterparty is configuration, not something an arriving claim may
    /// teach this connector.
    channels: ClientChannelRegistry,
    /// The live watermarks, each paired with the signature that earned it
    /// (issue #1218). Every acceptance is decided, advanced *and enqueued
    /// for journaling* under this one write lock (issue #605, #686), so the
    /// journal's entry order and the watermark order are the same order --
    /// what a replay reconstructs is exactly the state this gate held.
    /// Shared with the committer thread, which needs the same lock to roll
    /// a failed batch's advances back.
    watermarks: Arc<RwLock<HashMap<String, LiveClaim>>>,
    /// What each channel in [`Self::watermarks`] held immediately before
    /// its *current* entry there -- i.e. exactly what [`Self::roll_back`]
    /// restores when the packet that advanced a channel to its current
    /// watermark turns out never to have been carried (issue #1012).
    /// Written by [`Self::admit`] at the moment it advances a channel,
    /// under the same write lock; consumed (and removed) by
    /// [`Self::roll_back`]. Deliberately **not** durable and **not**
    /// shared with [`GroupCommitter`]: a rollback is only ever attempted
    /// within the same request that admitted the claim being rolled back
    /// (the client edge learns the next hop's answer synchronously, before
    /// replying), so nothing here needs to survive a restart, unlike
    /// `GroupCommitter`'s own `previous` bookkeeping on
    /// [`PendingAcceptance`], which undoes a *failed-durability* advance
    /// rather than a *reported-uncarried* one and is computed fresh for
    /// the batch it is undoing.
    previous_watermarks: RwLock<HashMap<String, Option<LiveClaim>>>,
    /// The group-commit seam between an acceptance and its durability
    /// (issue #686): entries enqueued under the watermark lock, batched
    /// into one journal write + fsync outside it.
    committer: GroupCommitter,
    /// The moment (unix seconds) this gate last accepted a claim on a
    /// channel, keyed the same as [`Self::watermarks`] (issue #693's
    /// claim-state endpoint: a fleet dashboard's liveness signal). Kept
    /// **deliberately non-durable and separate from the watermark**: it is
    /// updated by [`Self::note_claim_time`], called only after
    /// [`Self::ingest`] has already returned -- never from inside `ingest`,
    /// `admit`, or [`GroupCommitter`] -- so it adds no lock contention, no
    /// I/O and no new work to the admission path #686/#688/#690 spent this
    /// gate's whole history keeping cheap. A restart forgets it and the
    /// next accepted claim repopulates it; the watermark (and therefore
    /// every dollar figure the claim-state endpoint reports) is unaffected
    /// either way, since that is still sourced from the durable journal.
    last_claim_seen: RwLock<HashMap<String, u64>>,
    /// This connector's own outbound claim ledger for the same channels
    /// this gate accepts an inbound claim on (issue #700's netting): what
    /// this connector has separately committed to pay a channel's
    /// counterparty, consulted by [`check_collateral`] so that credit
    /// raises spendable headroom directly rather than only after an
    /// on-chain round trip (`toon-meta#262` decision 9). `None` -- the
    /// default every constructor leaves this at absent
    /// [`Self::with_payout_ledger`] -- nets nothing: collateral binding is
    /// exactly [`DepositFloor::covers`], this gate's behaviour before issue
    /// #700.
    payout_ledger: Option<Arc<ClientPayoutLedger>>,
    /// A client session's bound ILP address -> the EVM channel id this
    /// gate has associated it with (issue #787). A BTP session is keyed by
    /// ILP address (issue #736/toon-client#503), not by channel id, so
    /// nothing previously joined "which session earned this fulfilment" to
    /// "which channel to credit". [`Self::record_session_channel`] is the
    /// only writer, and it has two callers, neither of which is a
    /// session's own unverified say-so: `crate::btp::record_accepted_claim`
    /// once a claim has already cleared [`Self::admit`]'s full
    /// verification (issue #787's original path -- a session that both
    /// pays and earns), and `crate::btp::verify_and_record_declared_channel`
    /// once a session's declared channel-control proof at BTP auth has
    /// verified against [`Self::channels`]'s registered counterparty (issue
    /// #790 -- a session that only ever earns and so never presents a
    /// claim of its own). Learning is best-effort and non-durable: a
    /// restart forgets it, same as [`Self::last_claim_seen`], and the next
    /// accepted claim or verified proof from that session teaches it again
    /// before any payout depends on it.
    session_channels: RwLock<HashMap<String, String>>,
}

impl ClientClaimGate {
    /// A gate accepting claims on `channels` and no others, resuming from
    /// the watermarks `journal` already records (issue #605).
    ///
    /// This is the only way to build a gate, and it always replays: there
    /// is deliberately no constructor that starts a gate at no watermarks
    /// without saying where its watermarks came from, because a gate that
    /// silently starts at `None` accepts every nonce a client has already
    /// spent.
    ///
    /// An empty registry refuses every claim as
    /// [`ClaimIngestRejection::UnknownChannel`] -- see
    /// [`crate::ClientChannelRegistry`]'s own doc for why that is the
    /// intended failure mode rather than an oversight.
    ///
    /// # Errors
    ///
    /// A journal that cannot be read, or that carries a line this build
    /// cannot decode ([`JournalError::Corrupt`]). The caller must fail --
    /// per ADR 0009, before anything else starts -- rather than fall back
    /// to an empty set of watermarks.
    pub fn restore(
        channels: ClientChannelRegistry,
        journal: Arc<dyn Journal>,
    ) -> Result<ClientClaimGate, JournalError> {
        let watermarks = Arc::new(RwLock::new(replay_watermarks(&journal.read_all()?)));
        let committer = GroupCommitter::spawn(journal, Arc::clone(&watermarks));
        Ok(ClientClaimGate {
            channels,
            watermarks,
            previous_watermarks: RwLock::new(HashMap::new()),
            committer,
            last_claim_seen: RwLock::new(HashMap::new()),
            payout_ledger: None,
            session_channels: RwLock::new(HashMap::new()),
        })
    }

    /// Bind `ledger` -- this connector's outbound claim ledger -- to this
    /// gate's channels (issue #700): a channel's inbound collateral check
    /// and the claim-state endpoint's `available` figure both net what
    /// `ledger` has credited that channel's counterparty against what this
    /// gate has already accepted from them. `ledger` and this gate's own
    /// [`ClientChannelRegistry`] MUST be configured with the same channel
    /// ids for netting to mean anything -- `ledger`'s EVM channel id
    /// (`0x` + 64 lower-case hex, [`ClientPayoutLedger::set_channel_domain`])
    /// is looked up by exactly the on-chain bytes a resolved EVM claim's
    /// channel already decoded to, so no separate configuration step is
    /// needed here beyond calling this once at startup.
    pub fn with_payout_ledger(mut self, ledger: Arc<ClientPayoutLedger>) -> ClientClaimGate {
        self.payout_ledger = Some(ledger);
        self
    }

    /// This gate's own outbound payout ledger, if [`Self::with_payout_ledger`]
    /// configured one -- the same instance [`Self::credited_evm`] and the
    /// claim-state endpoint already net against.
    pub(crate) fn payout_ledger(&self) -> Option<&Arc<ClientPayoutLedger>> {
        self.payout_ledger.as_ref()
    }

    /// Credit `channel_id` `amount` against `job_id` (issue #770), the
    /// same as [`ClientPayoutLedger::record_payout_once`], except a channel
    /// this gate's ledger has no domain for yet is first resolved through
    /// this gate's own budgeted [`ClientChannelRegistry`] -- the one the
    /// inbound claim path already uses (`verify_evm_claim_signature`) --
    /// rather than being unpayable forever (issue #780). A self-opened
    /// agent channel carries no `[[client_channels]]` row and so is never
    /// in the ledger's pre-seeded set; without this it could never be
    /// credited, no matter how many jobs it fulfilled.
    ///
    /// A channel the ledger already knows about (pre-seeded or previously
    /// resolved) skips the lookup entirely. Resolution is EVM-only, same as
    /// [`ClientPayoutLedger`]'s existing reach -- a `channel_id` that is not
    /// a 32-byte hex channel id is left unresolved, and a resolution
    /// failure (the channel does not exist, or the chain endpoint is down)
    /// is likewise left unresolved rather than defaulted: the payout that
    /// follows then simply produces nothing, exactly as it always has for
    /// an unknown channel.
    ///
    /// Returns `None` if no payout ledger is configured at all, or under
    /// any of the conditions [`ClientPayoutLedger::record_payout_once`]
    /// itself already returns `None` for -- including its own dedupe, which
    /// this does not affect.
    pub(crate) async fn credit_payout(
        &self,
        channel_id: &str,
        job_id: &[u8; 32],
        amount: u64,
        now: DateTime<Utc>,
    ) -> Option<WireClaim> {
        let ledger = self.payout_ledger()?;
        if !ledger.has_channel_domain(channel_id) {
            self.resolve_payout_domain(ledger, channel_id).await;
        }
        ledger.record_payout_once(channel_id, job_id, amount, now)
    }

    /// [`Self::credit_payout`]'s on-demand resolution step, split out so its
    /// early-exit guards (not a 32-byte EVM channel id; the chain lookup
    /// comes back empty or fails) read as a flat sequence rather than nested
    /// `if let`s. Best-effort and silent either way: an id that cannot be
    /// resolved, or a domain [`ClientPayoutLedger::ensure_channel_domain`]
    /// itself declines, simply leaves `channel_id` unresolved for the
    /// `record_payout_once` call that follows.
    async fn resolve_payout_domain(&self, ledger: &ClientPayoutLedger, channel_id: &str) {
        let Some(on_chain_id) = decode_hex_bytes::<32>(channel_id) else {
            return;
        };
        let requester = format!("payout:{channel_id}");
        let Ok(Some(channel)) = self.channels.evm(&on_chain_id, &requester).await else {
            return;
        };
        let _ = ledger.ensure_channel_domain(
            channel_id,
            ChannelDomain {
                chain_id: channel.chain_id,
                token_network_address: channel.token_network_address,
            },
        );
    }

    /// Learn that `address` -- a client session's own bound ILP address --
    /// speaks for `channel_id` (issue #787). Both callers verify this
    /// before calling: `crate::btp::record_accepted_claim`, once
    /// [`Self::admit`] has fully verified a genuine claim naming
    /// `channel_id`, and `crate::btp::verify_and_record_declared_channel`,
    /// once a session's own channel-control proof at BTP auth has verified
    /// against [`Self::channels`]'s registered counterparty (issue #790) --
    /// so this is never a session's own unverified say-so either way.
    /// Overwrites any previous association for `address` -- this is a
    /// best-current-belief cache, not an append-only ledger like a
    /// watermark, and a session that reconnects or a channel that closes
    /// and reopens is still just taught its current fact the next time it
    /// proves or pays.
    pub(crate) fn record_session_channel(&self, address: &str, channel_id: String) {
        self.session_channels
            .write()
            .expect("session channel map lock poisoned")
            .insert(address.to_string(), channel_id);
    }

    /// The channel id [`Self::record_session_channel`] has associated with
    /// `address`, if any.
    fn session_channel(&self, address: &str) -> Option<String> {
        self.session_channels
            .read()
            .expect("session channel map lock poisoned")
            .get(address)
            .cloned()
    }

    /// `destination`'s associated channel id together with this gate's own
    /// payout ledger (issue #779) -- what `session_route::deliver_pending_claim`
    /// needs to look up `ClientPayoutLedger::pending_claim` and, once a
    /// resend succeeds, acknowledge it. `None` if this gate has no payout
    /// ledger configured at all, or if `destination` has no channel
    /// association yet ([`Self::record_session_channel`]) -- both are
    /// reasons there is nothing to resend, not an error.
    pub(crate) fn payout_channel_for_session(
        &self,
        destination: &str,
    ) -> Option<(String, Arc<ClientPayoutLedger>)> {
        let channel_id = self.session_channel(destination)?;
        let ledger = Arc::clone(self.payout_ledger()?);
        Some((channel_id, ledger))
    }

    /// [`Self::credit_payout`], resolving `destination` -- a client
    /// session's own bound ILP address, exactly what
    /// `crate::session_route::route_prepare` delivers a fulfilled PREPARE
    /// through -- to a channel id via [`Self::record_session_channel`]'s
    /// own map first (issue #787). Production binds a session under its
    /// ILP address, never under a channel id (issue #736/toon-client#503),
    /// so `destination` itself is never a payable key: without this
    /// resolution step every credit attempt silently found nothing, on
    /// every deployed connector.
    ///
    /// `None`, logged rather than left silent (this issue's own AC), for a
    /// destination this gate has never learned a channel for -- an earning
    /// agent that has never itself presented a claim on this session has
    /// no association yet, and crediting nothing is the explicit decision
    /// for that case, not an oversight indistinguishable from any of
    /// [`Self::credit_payout`]'s own reasons to decline.
    pub(crate) async fn credit_session_payout(
        &self,
        destination: &str,
        job_id: &[u8; 32],
        amount: u64,
        now: DateTime<Utc>,
    ) -> Option<WireClaim> {
        let Some(channel_id) = self.session_channel(destination) else {
            tracing::info!(
                destination = %destination,
                "no channel is associated with this session yet -- crediting nothing"
            );
            return None;
        };
        self.credit_payout(&channel_id, job_id, amount, now).await
    }

    /// The watermark this gate currently holds for `channel_key` (the
    /// chain-namespaced key `ClientClaim::channel_key` produces), or `None`
    /// if it has never accepted a claim on that channel. Read-only: the
    /// only thing that advances a watermark is a fully accepted claim.
    ///
    /// Canonicalised on the way in (issue #643), so asking for a channel
    /// in one spelling and having been paid on it in another cannot answer
    /// `None` -- the same rule the write side files under, applied to the
    /// read.
    pub fn watermark(&self, channel_key: &str) -> Option<Watermark> {
        self.watermarks
            .read()
            .expect("client claim watermarks lock poisoned")
            .get(&canonical_channel_key(channel_key))
            .map(|record| record.watermark)
    }

    /// The highest-nonce claim this gate has ever accepted on `channel_key`,
    /// as `(nonce, cumulative_amount, signature)` -- exactly what an
    /// on-chain redemption submits (issue #1218), mirroring
    /// `connector_domain::Projection::latest_inbound_claim`, the peer
    /// semantics's own equivalent read. `None` before any claim has been
    /// accepted on this channel.
    pub fn latest_inbound_claim(&self, channel_key: &str) -> Option<(u64, u64, Vec<u8>)> {
        self.watermarks
            .read()
            .expect("client claim watermarks lock poisoned")
            .get(&canonical_channel_key(channel_key))
            .map(|record| {
                (
                    record.watermark.nonce,
                    record.watermark.cumulative_amount,
                    record.signature.clone(),
                )
            })
    }

    /// Every channel this gate has ever accepted a claim on, and that
    /// claim's watermark (issue #1218): what `GET /claims` and
    /// `GET /channels` need to enumerate the client-edge book, the same way
    /// `connector_runtime::ClaimBook::views` enumerates the peer book.
    /// Carries no signature -- nothing reads this list to redeem;
    /// [`Self::latest_inbound_claim`] is the redeem path.
    pub fn accepted_channels(&self) -> Vec<(String, Watermark)> {
        self.watermarks
            .read()
            .expect("client claim watermarks lock poisoned")
            .iter()
            .map(|(channel_id, record)| (channel_id.clone(), record.watermark))
            .collect()
    }

    /// The registry of channels this gate accepts a claim on -- their
    /// recorded counterparty, deposit floor and (for EVM) signing domain
    /// (issue #693's claim-state endpoint needs all three to verify a
    /// proof-of-control challenge and to report a channel's deposit; this
    /// gate already holds the registry, so it is exposed rather than
    /// threaded through separately).
    pub(crate) fn channels(&self) -> &ClientChannelRegistry {
        &self.channels
    }

    /// The shaper this node's chain lookups are already metered by
    /// ([`crate::lookup_budget`]), for the one other caller ADR 0050 gives it:
    /// `GET /ilp`'s self-description is rate-limited *through this bucket*
    /// rather than through a second mechanism of its own.
    pub(crate) fn lookup_budget(&self) -> &crate::lookup_budget::UnresolvableLookupBudget {
        self.channels.lookup_budget()
    }

    /// What this connector has separately committed to pay EVM channel
    /// `channel_id`'s counterparty back (issue #700's "credited" term --
    /// see [`Self::with_payout_ledger`]). `0` with no payout ledger
    /// configured, or for a channel it has never paid out on -- exactly
    /// this gate's pre-#700 behaviour. Exposed alongside [`Self::channels`]
    /// and [`Self::watermark`] so the claim-state endpoint (§1.10) can net
    /// the same figure [`check_collateral`] admits against.
    pub(crate) fn credited_evm(&self, channel_id: &[u8; 32]) -> u64 {
        self.payout_ledger.as_ref().map_or(0, |ledger| {
            ledger.credited(&format!("0x{}", hex::encode(channel_id)))
        })
    }

    /// [`Self::credited_evm`], dispatched on a [`ResolvedChannelKey`]
    /// [`verify_claim_signature`] already resolved -- the collateral
    /// check's own call site, so it never re-decodes an id it already has
    /// in hand.
    fn credited(&self, channel: &ResolvedChannelKey) -> u64 {
        match channel {
            ResolvedChannelKey::Evm(channel_id) => self.credited_evm(channel_id),
            // `ClientPayoutLedger` wraps `connector_runtime::ClaimBook`,
            // which only ever signs an EVM balance proof (issue #699) --
            // there is no Solana payout to net against yet, so a Solana
            // channel nets nothing rather than guessing at a key format no
            // ledger will ever be registered under. Per the issue's own
            // "do not net across chains" rule, this is the correct answer
            // for a Solana channel forever, not just until support lands:
            // Solana credit, if it ever exists, nets against a Solana
            // channel's own floor, never an EVM one's.
            ResolvedChannelKey::Solana(_) => 0,
        }
    }

    /// The unix-second timestamp [`Self::note_claim_time`] last recorded
    /// for `channel_key`, or `None` if this gate has not accepted a claim
    /// on it since the last restart. See [`Self::last_claim_seen`]'s own
    /// doc for why this is best-effort rather than durable.
    pub fn last_claim_time(&self, channel_key: &str) -> Option<u64> {
        self.last_claim_seen
            .read()
            .expect("last claim time lock poisoned")
            .get(&canonical_channel_key(channel_key))
            .copied()
    }

    /// Record that a claim on `channel_key` was just accepted, at
    /// `now_unix`. Deliberately a separate call a caller makes *after*
    /// [`Self::ingest`] has already returned success, never something
    /// `ingest`/`admit` do themselves -- see [`Self::last_claim_seen`]'s
    /// doc. Every carrier that calls `ingest` (`POST /ilp`, `POST
    /// /ilp/probe`, the BTP session) calls this right after, so the
    /// claim-state endpoint's liveness signal covers every carrier a claim
    /// can arrive on.
    pub fn note_claim_time(&self, channel_key: &str, now_unix: u64) {
        self.last_claim_seen
            .write()
            .expect("last claim time lock poisoned")
            .insert(canonical_channel_key(channel_key), now_unix);
    }

    /// Parse and fully validate a plaintext claim JSON body (already
    /// base64-decoded and, if it arrived wrapped, already unwrapped by the
    /// caller): structure, then freshness/watermark, then value binding
    /// against `price` -- the matched route's price (issue #522), `0` for a
    /// route that charges nothing or that isn't priced at all -- then,
    /// last, the claim's signature against the counterparty recorded for
    /// the channel it names (issue #506/#544, #558).
    /// Advances this claim's channel watermark only when the claim is
    /// fully accepted -- a rejected claim, whether stale, underpaying,
    /// unverifiable or unrecordable, leaves the watermark exactly as it
    /// was, so a corrected resubmission is still judged against the same
    /// baseline.
    ///
    /// `async` because resolving a channel nothing declared is a read
    /// against a chain (issue #556). The watermark lock is deliberately
    /// **not** held across that await -- a `std::sync::RwLock` guard held
    /// across a suspension point would stall every other packet in flight
    /// -- so the freshness and value rules are evaluated twice: once up
    /// front, which is what keeps #544's ordering promise that a replay or
    /// an underpayment never pays for a signature check, and once more
    /// under the write lock immediately before the watermark advances,
    /// which is what makes two concurrent claims on one channel still
    /// serialise. The second evaluation is the authoritative one.
    ///
    /// The advance is made durable before it is made *visible to the
    /// caller* (issue #605): the accepted claim is enqueued for this
    /// gate's journal under the write lock, and the claim only comes back
    /// `Ok` once the committer reports the batch carrying it fsync'd --
    /// group commit (issue #686), one write and one fsync amortized over
    /// every claim that arrived while the previous batch was syncing,
    /// instead of one fsync per claim under the global lock. The write
    /// lock covers only the re-check, the advance and the enqueue --
    /// microseconds, no I/O -- which is what lets concurrent sessions'
    /// claims share an fsync instead of queueing behind each other's. A
    /// batch that cannot be made durable refuses every claim in it as
    /// [`ClaimIngestRejection::NotDurable`] and rolls their advances back
    /// (see [`GroupCommitter`]), so this connector still never renders
    /// service against a watermark a restart would forget, and a refused
    /// claim is still resubmittable unchanged.
    pub async fn ingest(
        &self,
        claim_json: &str,
        price: u64,
    ) -> Result<ClientClaim, ClaimIngestRejection> {
        let (claim, durability) = self.admit(claim_json, price).await?;
        durability.durable().await?;
        Ok(claim)
    }

    /// [`ClientClaimGate::ingest`]'s decision half: everything up to and
    /// including the acceptance -- structure, freshness, value, signature,
    /// collateral, the authoritative re-check, the watermark advance and
    /// the journal enqueue -- but not the wait for durability, which the
    /// returned [`DurabilityTicket`] carries. Callers for whom acceptance
    /// order matters (the BTP carriage: claims on one session must be
    /// judged strictly in arrival order) admit in order and may then
    /// overlap the durability waits; `ingest` itself is simply
    /// `admit(..).await` + `durable().await`, so no second admission
    /// pipeline exists to drift.
    ///
    /// An `Ok` here is an *acceptance, not yet durable*: the watermark has
    /// advanced and the entry is queued in acceptance order, but no
    /// service may be rendered for the claim until the ticket resolves --
    /// that is the boundary ADR 0005 protects.
    pub(crate) async fn admit(
        &self,
        claim_json: &str,
        price: u64,
    ) -> Result<(ClientClaim, DurabilityTicket), ClaimIngestRejection> {
        let claim = parse_client_claim(claim_json).map_err(|error| match error {
            ClientClaimError::Mina => ClaimIngestRejection::Mina,
            other => ClaimIngestRejection::Malformed(other.to_string()),
        })?;

        let key = claim.channel_key();
        {
            let watermarks = self
                .watermarks
                .read()
                .expect("client claim watermarks lock poisoned");
            check_freshness_and_value(
                watermarks.get(&key).map(|record| record.watermark),
                &claim,
                price,
            )?;
        }

        // Who a lookup for a channel this connector has never resolved is
        // budgeted against (issue #613). Read from the claim and used for
        // nothing but that: it is the claim's *self-declared* signer, and
        // step 4 below still reads the key it verifies against out of the
        // registry, exactly as #558 requires. See
        // `crate::lookup_budget` for why this identity, and what it is
        // honestly worth.
        let requester = claim.signer_key();

        // The one await, and the only work that has to happen outside the
        // lock -- so it is also the last thing that happens outside it.
        let verified = verify_claim_signature(&self.channels, &claim, &requester).await?;

        // client-edge-spec.md §1.3 step 5 (issue #646), after cryptographic
        // verification and before the write lock: only a claim that is
        // already fresh, value-covering and correctly signed can reach the
        // chain read this may provoke. It needs no re-check under the lock,
        // unlike freshness and value: the bound is absolute per claim
        // rather than relative to the watermark, and both the deposit and
        // the credited amount (issue #700) only ever grow, so no concurrent
        // claim can turn an amount that fitted into one that does not.
        let credited = self.credited(&verified.channel);
        check_collateral(&self.channels, &claim, &verified, &requester, credited).await?;

        let mut watermarks = self
            .watermarks
            .write()
            .expect("client claim watermarks lock poisoned");
        // Re-read rather than reusing the value from above: a concurrent
        // claim on this same channel may have advanced the watermark while
        // the channel lookup was in flight, and accepting both would be
        // exactly the replay this gate exists to refuse.
        check_freshness_and_value(
            watermarks.get(&key).map(|record| record.watermark),
            &claim,
            price,
        )?;

        // Advance and enqueue under the same write lock the authoritative
        // re-check was decided under (ADR 0005, issue #605, #686): the
        // order entries reach the committer's queue is exactly the order
        // watermarks advanced in, so what the journal records -- and what
        // a replay after a restart reconstructs -- is this state and not
        // some interleaving of it. The fsync itself happens outside the
        // lock, in the committer's batch; the caller's ticket resolves
        // only once it has, so nothing is visible-before-durable at any
        // boundary that renders service.
        // The signature is retained rather than discarded for the same
        // reason the peer semantics retains it (issue #425): a watermark says
        // what was spent, but only the claim itself is redeemable.
        let previous = watermarks.get(&key).cloned();
        watermarks.insert(
            key.clone(),
            LiveClaim {
                watermark: advance_watermark(claim.nonce(), claim.transferred_amount()),
                signature: verified.signature.clone(),
            },
        );
        let ticket = match self.committer.enqueue(PendingAcceptance {
            entry: JournalEntry::InboundClaimAccepted {
                channel_id: key.clone(),
                nonce: claim.nonce(),
                cumulative_amount: claim.transferred_amount(),
                signature: verified.signature,
            },
            channel_key: key.clone(),
            previous: previous.clone(),
        }) {
            Ok(ticket) => {
                // Recorded only now the advance is actually queued for the
                // journal (issue #1012): `Self::roll_back` restores exactly
                // this value, so it must agree with what `previous` on the
                // `PendingAcceptance` above would also restore on a failed
                // batch.
                self.previous_watermarks
                    .write()
                    .expect("client claim previous-watermark lock poisoned")
                    .insert(key.clone(), previous);
                ticket
            }
            Err(CommitterGone) => {
                // The committer thread is gone -- nothing will ever fsync
                // this entry. Undo the advance while still holding the
                // lock (no other claim has seen it) and refuse exactly as
                // a failed append always has.
                restore_watermark(&mut watermarks, &key, previous);
                tracing::error!(
                    channel = %key,
                    "refusing a valid claim: the journal committer is gone, so its \
                     acceptance could not be durably recorded"
                );
                return Err(ClaimIngestRejection::NotDurable);
            }
        };
        drop(watermarks);

        Ok((claim, ticket))
    }

    /// Undo [`Self::admit`]'s watermark advance for the claim that reached
    /// exactly `nonce`/`cumulative_amount` on `channel_key` -- because the
    /// PREPARE it covered is now known never to have been carried across a
    /// forwarded route (issue #1012, ADR 0028): the client edge admits the
    /// client's claim before learning whether the next hop will fulfil it,
    /// and the next hop's own terminal reject (F06 after a covered retry,
    /// T01 unreachable) is discoverable only after that admission --
    /// unlike the cases [`crate::Connector::cover_forward`] (or an
    /// equivalent pre-admission check) can already predict before this
    /// gate is ever consulted, this is the seam for the ones it cannot.
    ///
    /// A no-op -- correctly, not a bug -- in two cases:
    ///
    /// * a later claim has since advanced `channel_key` past
    ///   `nonce`/`cumulative_amount`. That claim's own admission is
    ///   unrelated to this reject, and unwinding it here would erase state
    ///   this call has no business touching, so it is left alone -- this
    ///   call only ever acts while the claim it names is still the
    ///   channel's current watermark.
    /// * this gate holds no [`Self::previous_watermarks`] record for
    ///   `channel_key`. Every call this connector itself makes provides
    ///   one -- `admit` records it at the exact moment it advances the
    ///   channel -- so this can only be reached by a rollback attempted
    ///   outside the request that admitted the claim, which nothing in
    ///   this codebase does (see the field's own doc for why that is safe
    ///   to assume).
    ///
    /// Durable exactly like `admit`'s own advance: the in-memory watermark
    /// moves and the entry is enqueued under the same write lock every
    /// acceptance is decided under, and this call does not resolve until
    /// the committer reports it fsync'd -- a rollback a restart could
    /// forget would leave the client durably charged for a packet this
    /// connector itself decided not to count.
    pub(crate) async fn roll_back(
        &self,
        channel_key: &str,
        nonce: u64,
        cumulative_amount: u64,
    ) -> Result<(), ClaimIngestRejection> {
        let key = canonical_channel_key(channel_key);
        let ticket = {
            let mut watermarks = self
                .watermarks
                .write()
                .expect("client claim watermarks lock poisoned");
            let current = watermarks.get(&key).cloned();
            if current.as_ref().map(|record| record.watermark)
                != Some(Watermark {
                    nonce,
                    cumulative_amount,
                })
            {
                return Ok(());
            }
            let mut previous_watermarks = self
                .previous_watermarks
                .write()
                .expect("client claim previous-watermark lock poisoned");
            let Some(previous) = previous_watermarks.remove(&key) else {
                tracing::warn!(
                    channel = %key,
                    "asked to roll back a claim this gate has no prior watermark recorded \
                     for; leaving it charged rather than guessing"
                );
                return Ok(());
            };
            let entry = match &previous {
                Some(record) => JournalEntry::InboundClaimRolledBack {
                    channel_id: key.clone(),
                    nonce: record.watermark.nonce,
                    cumulative_amount: record.watermark.cumulative_amount,
                },
                None => JournalEntry::InboundClaimWatermarkReset {
                    channel_id: key.clone(),
                },
            };
            restore_watermark(&mut watermarks, &key, previous.clone());
            match self.committer.enqueue(PendingAcceptance {
                entry,
                channel_key: key.clone(),
                previous: current.clone(),
            }) {
                Ok(ticket) => ticket,
                Err(CommitterGone) => {
                    restore_watermark(&mut watermarks, &key, current);
                    previous_watermarks.insert(key.clone(), previous);
                    tracing::error!(
                        channel = %key,
                        "could not durably record a claim rollback: the journal committer \
                         is gone"
                    );
                    return Err(ClaimIngestRejection::NotDurable);
                }
            }
        };
        ticket.durable().await
    }

    /// Reset `channel_key`'s watermark, durably (issue #977): the next
    /// claim on this channel is judged as if this gate had never accepted
    /// one. Only ever called once this gate itself -- never a caller's
    /// guess -- has confirmed the chain no longer vouches for the channel;
    /// see [`Self::reap_unresolvable_channels`], its only caller.
    ///
    /// A no-op, durably nothing written, when this channel has no
    /// watermark to reset -- most channels, most of the time, and not
    /// worth an idle journal entry.
    ///
    /// Durability follows exactly [`Self::admit`]'s own discipline: the
    /// reset is enqueued under the same write lock every acceptance is
    /// decided under, so it can never race a concurrent claim's advance on
    /// the same channel, and this call does not resolve until the
    /// committer reports it fsync'd -- a reset a restart could forget is
    /// worse than no reset at all, since it would silently re-arm the
    /// double-charge this issue exists to close.
    async fn reset_watermark(&self, channel_key: &str) -> Result<(), ClaimIngestRejection> {
        let key = canonical_channel_key(channel_key);
        // Scoped so the write guard provably ends here, before this
        // function's one await below -- `std::sync::RwLockWriteGuard` is
        // not `Send`, and this method is awaited from a spawned,
        // multi-threaded task (`ClientClaimGate::reap_unresolvable_channels`),
        // unlike `Self::admit`'s identical-shaped lock use, which never has
        // an await left in its own body once the guard drops.
        let ticket = {
            let mut watermarks = self
                .watermarks
                .write()
                .expect("client claim watermarks lock poisoned");
            let Some(previous) = watermarks.remove(&key) else {
                return Ok(());
            };
            match self.committer.enqueue(PendingAcceptance {
                entry: JournalEntry::InboundClaimWatermarkReset {
                    channel_id: key.clone(),
                },
                channel_key: key.clone(),
                previous: Some(previous.clone()),
            }) {
                Ok(ticket) => ticket,
                Err(CommitterGone) => {
                    restore_watermark(&mut watermarks, &key, Some(previous));
                    tracing::error!(
                        channel = %key,
                        "could not durably record a watermark reset: the journal committer is gone"
                    );
                    return Err(ClaimIngestRejection::NotDurable);
                }
            }
        };
        ticket.durable().await
    }

    /// Sweep every channel this gate currently holds a watermark for, and
    /// reset any the chain no longer vouches for -- settled, deallocated,
    /// or otherwise gone (issue #977).
    ///
    /// This is the fix's proactive half, and it has to be: a reopened
    /// channel's first claim after settlement fails this gate's freshness
    /// check (its nonce cannot advance the stale watermark left over from
    /// the settled incarnation) *before* [`Self::admit`] ever resolves the
    /// channel again -- [`check_freshness_and_value`] runs first and is
    /// pure, deliberately spending no chain read on a claim that looks
    /// like a replay (issue #544's ordering). So nothing on the claim path
    /// can ever observe a reopen; only a check that runs independently of
    /// claim traffic can.
    ///
    /// Declared channels ([`ClientChannelRegistry::record_evm`]/
    /// [`record_solana`](ClientChannelRegistry::record_solana)) are
    /// untouched: `refresh_evm`/`refresh_solana` answer them from config,
    /// never the chain, by design -- an operator who hand-declares a
    /// channel is expected to also manage its lifecycle by hand, the same
    /// exemption issue #646 already carves out for a declared channel's
    /// collateral cap.
    ///
    /// Every re-read goes through [`ClientChannelRegistry::refresh_evm`]/
    /// [`refresh_solana`](ClientChannelRegistry::refresh_solana) -- the
    /// exact same rate-limited, budgeted path [`check_collateral`] already
    /// uses on a deposit-floor breach -- so a sweep costs at most one chain
    /// read per channel per `min_reattempt_interval`, never a burst, and
    /// never touches a channel this gate has no watermark for at all (an
    /// unpaid or never-resolved channel has nothing here worth protecting).
    ///
    /// Best-effort throughout: a lookup failure (an unreachable endpoint,
    /// say) answers neither `Ok(None)` nor
    /// [`ChannelResolutionError::Terminal`], so it is left alone rather
    /// than treated as gone, and a reset that could not be made durable is
    /// logged and skipped -- one bad channel or one journal hiccup must
    /// not stall every other channel's sweep or crash the loop that calls
    /// this repeatedly.
    pub async fn reap_unresolvable_channels(&self) {
        let keys: Vec<String> = self
            .watermarks
            .read()
            .expect("client claim watermarks lock poisoned")
            .keys()
            .cloned()
            .collect();
        for key in keys {
            if self.channel_is_gone(&key).await {
                tracing::warn!(
                    channel = %key,
                    "resetting this channel's watermark: the chain no longer vouches for it \
                     (settled, deallocated, or otherwise gone) -- a reopened channel starts \
                     clean rather than inheriting its predecessor's spend"
                );
                if let Err(error) = self.reset_watermark(&key).await {
                    tracing::error!(
                        channel = %key,
                        ?error,
                        "could not durably reset this channel's watermark; will retry it on the \
                         next sweep"
                    );
                }
            }
        }
    }

    /// Whether `key` -- an already-canonical channel key -- currently
    /// resolves to nothing this connector can be paid on, per
    /// [`Self::reap_unresolvable_channels`]'s own rules for what counts.
    /// Split out so that function reads as the sweep it is, not the two
    /// chains' decoding.
    ///
    /// A key this cannot take apart -- an unknown namespace, or an
    /// identifier that does not decode to its chain's 32 bytes -- is
    /// answered `false`, the same "leave it alone" a failed lookup gets:
    /// nothing here may reset a watermark on anything but a chain's own
    /// answer.
    async fn channel_is_gone(&self, key: &str) -> bool {
        let requester = format!("sweep:{key}");
        // Split on the namespace exactly as `canonical_channel_key` does,
        // since that is the function whose output this is taking apart.
        match key.split_once(':') {
            Some((EVM_NAMESPACE, channel_id)) => {
                let Some(channel_id) = decode_hex_bytes::<32>(channel_id) else {
                    return false;
                };
                matches!(
                    self.channels.refresh_evm(&channel_id, &requester).await,
                    Ok(None) | Err(ChannelResolutionError::Terminal(_))
                )
            }
            Some((SOLANA_NAMESPACE, channel_account)) => {
                let Some(channel_account) = decode_base58_bytes::<32>(channel_account) else {
                    return false;
                };
                matches!(
                    self.channels
                        .refresh_solana(&channel_account, &requester)
                        .await,
                    Ok(None) | Err(ChannelResolutionError::Terminal(_))
                )
            }
            _ => false,
        }
    }
}

/// The most entries one journal batch carries -- a bound on the buffer a
/// commit builds, not a tuning knob: the committer drains only what is
/// already queued, so a batch is naturally sized by how many claims
/// arrived during the previous batch's fsync. At ~200 bytes a line this
/// caps a batch's buffer under a megabyte.
const GROUP_COMMIT_MAX_BATCH: usize = 4096;

/// An accepted-but-not-yet-durable claim, queued for the committer: the
/// journal entry to write, and what the committer needs to *unwrite* the
/// acceptance -- the channel it advanced and the watermark that channel
/// held before it -- should the batch fail.
struct PendingAcceptance {
    entry: JournalEntry,
    channel_key: String,
    previous: Option<LiveClaim>,
}

/// The committer thread has exited, so nothing will ever journal this
/// entry. Only possible after that thread panicked -- its loop runs until
/// the gate (the sender) is dropped.
struct CommitterGone;

/// A claim's pending durability (issue #686): resolves once the journal
/// batch carrying the claim's entry is fsync'd -- or refuses, if it could
/// not be. [`ClientClaimGate::ingest`] awaits it before returning the
/// claim; no caller may render service before it resolves, because until
/// then the acceptance exists only in memory.
pub struct DurabilityTicket {
    durable: tokio::sync::oneshot::Receiver<Result<(), ()>>,
}

impl DurabilityTicket {
    /// Wait for the batch fsync. Any failure -- the batch could not be
    /// written, or the committer is gone -- is
    /// [`ClaimIngestRejection::NotDurable`]: the watermark advance has
    /// already been rolled back by whoever discovered the failure, so the
    /// same claim resubmitted is still good.
    pub async fn durable(self) -> Result<(), ClaimIngestRejection> {
        match self.durable.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(())) | Err(_) => Err(ClaimIngestRejection::NotDurable),
        }
    }
}

/// The group-commit half of issue #686: a dedicated thread that drains
/// every [`PendingAcceptance`] queued since the last batch, writes them as
/// one [`Journal::append_batch`] -- one write, one fsync -- and only then
/// resolves their tickets. Batching is what moves the fsync out from under
/// the watermark lock without giving up durable-before-visible: claims
/// admitted while a batch is syncing queue up and share the *next* fsync,
/// so sustained throughput is bounded by claims-per-batch times the disk's
/// fsync rate rather than by the fsync rate alone.
///
/// A dedicated OS thread rather than a tokio task because
/// [`Journal::append_batch`] blocks on disk I/O, and this loop exists to
/// do nothing else; it exits when the gate is dropped (the sender goes
/// away) and takes nothing with it.
///
/// **Failure is rolled back, not just reported.** When a batch cannot be
/// made durable, the watermarks its entries advanced are wrong: they
/// promise a durable record that does not exist, and leaving them in
/// place would burn every refused claim's nonce -- the client's perfectly
/// good claim, resubmitted as [`ClaimIngestRejection::NotDurable`] invites,
/// would bounce off its own ghost as `NonceNotAdvancing`. So the committer
/// takes the same write lock every admission is decided under, drains
/// whatever else was admitted against the now-unrecorded state (those
/// entries could only have landed in this or a later batch, and there is
/// no later batch until this loop comes back around), restores every
/// touched channel to its watermark before the *earliest* failed claim,
/// and only then refuses the waiters. Admissions blocked on the lock
/// meanwhile re-check against the restored watermarks once they get it,
/// so nothing is ever judged against an advance that was rolled back.
struct GroupCommitter {
    sender: mpsc::Sender<(
        PendingAcceptance,
        tokio::sync::oneshot::Sender<Result<(), ()>>,
    )>,
}

impl GroupCommitter {
    fn spawn(
        journal: Arc<dyn Journal>,
        watermarks: Arc<RwLock<HashMap<String, LiveClaim>>>,
    ) -> GroupCommitter {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("client-claim-journal-commit".to_string())
            .spawn(move || group_commit_loop(receiver, journal, watermarks))
            .expect("spawning the journal committer thread");
        GroupCommitter { sender }
    }

    /// Queue `pending` for the next batch. Callers hold the watermark
    /// write lock while calling this -- that is the ordering guarantee,
    /// not an accident -- so the queue receives entries in exactly the
    /// order their watermarks advanced.
    fn enqueue(&self, pending: PendingAcceptance) -> Result<DurabilityTicket, CommitterGone> {
        let (durable_tx, durable_rx) = tokio::sync::oneshot::channel();
        self.sender
            .send((pending, durable_tx))
            .map_err(|_| CommitterGone)?;
        Ok(DurabilityTicket {
            durable: durable_rx,
        })
    }
}

type QueuedAcceptance = (
    PendingAcceptance,
    tokio::sync::oneshot::Sender<Result<(), ()>>,
);

fn group_commit_loop(
    receiver: mpsc::Receiver<QueuedAcceptance>,
    journal: Arc<dyn Journal>,
    watermarks: Arc<RwLock<HashMap<String, LiveClaim>>>,
) {
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        while batch.len() < GROUP_COMMIT_MAX_BATCH {
            match receiver.try_recv() {
                Ok(queued) => batch.push(queued),
                Err(_) => break,
            }
        }
        let entries: Vec<JournalEntry> = batch
            .iter()
            .map(|(pending, _)| pending.entry.clone())
            .collect();
        match journal.append_batch(&entries) {
            Ok(()) => {
                for (_, ticket) in batch {
                    // A receiver gone before its fsync means the ingest
                    // future was dropped; the acceptance is durable
                    // regardless, so there is nothing to do about it.
                    let _ = ticket.send(Ok(()));
                }
            }
            Err(err) => {
                tracing::error!(
                    %err,
                    claims = batch.len(),
                    "refusing a batch of valid claims: their acceptance could not be \
                     durably recorded"
                );
                {
                    let mut watermarks = watermarks
                        .write()
                        .expect("client claim watermarks lock poisoned");
                    // Everything still queued was admitted against the
                    // watermarks this failed batch advanced -- it has no
                    // durable batch to land in ahead of the rollback, so
                    // it fails and rolls back with it.
                    while let Ok(queued) = receiver.try_recv() {
                        batch.push(queued);
                    }
                    let mut restored: HashSet<&str> = HashSet::new();
                    for (pending, _) in &batch {
                        // First failed entry per channel wins: entries are
                        // in acceptance order, so its `previous` is the
                        // last watermark with a durable record behind it.
                        if restored.insert(pending.channel_key.as_str()) {
                            restore_watermark(
                                &mut watermarks,
                                &pending.channel_key,
                                pending.previous.clone(),
                            );
                        }
                    }
                }
                for (_, ticket) in batch {
                    let _ = ticket.send(Err(()));
                }
            }
        }
    }
}

/// Put `channel_key` back to `previous` -- the inverse of one watermark
/// advance, used only to unwind acceptances whose durable record failed.
fn restore_watermark(
    watermarks: &mut HashMap<String, LiveClaim>,
    channel_key: &str,
    previous: Option<LiveClaim>,
) {
    match previous {
        Some(record) => {
            watermarks.insert(channel_key.to_string(), record);
        }
        None => {
            watermarks.remove(channel_key);
        }
    }
}

/// client-edge-spec.md §1.3 steps 2 and 3 against `current`: the claim's
/// nonce and cumulative amount must advance this channel's watermark, and
/// the advance must cover `price`. Pure, and cheap enough to run twice --
/// see [`ClientClaimGate::ingest`] for why it is.
fn check_freshness_and_value(
    current: Option<Watermark>,
    claim: &ClientClaim,
    price: u64,
) -> Result<(), ClaimIngestRejection> {
    if let Err(error) = validate_claim(current, claim.nonce(), claim.transferred_amount()) {
        return Err(match error {
            ClaimError::NonceNotAdvancing { .. } => ClaimIngestRejection::NonceNotAdvancing,
            ClaimError::AmountNotAdvancing { .. } => ClaimIngestRejection::AmountNotAdvancing,
            ClaimError::Underpayment { .. } => {
                unreachable!("validate_claim never returns Underpayment")
            }
        });
    }
    if let Err(error) = validate_price(current, claim.transferred_amount(), price) {
        return Err(match error {
            ClaimError::Underpayment { advanced, price } => {
                ClaimIngestRejection::Underpayment { advanced, price }
            }
            other => unreachable!("validate_price only ever returns Underpayment: {other:?}"),
        });
    }
    Ok(())
}

/// Rebuild the per-channel watermarks a journal records, folding every
/// [`JournalEntry::InboundClaimAccepted`] in it -- the client edge's own
/// half of the replay `connector_runtime::ClaimBook::set_journal` does for
/// the peer semantics, over the same entry.
///
/// Componentwise `max` rather than last-wins, unlike the peer semantics's fold:
/// entries are appended in accepted order and each accepted claim strictly
/// advances, so the two agree on any journal this gate itself wrote. They
/// differ only on a journal that has been reordered or spliced, and there
/// the direction of the disagreement matters -- a watermark recovered by
/// `max` can never come back lower than something already accepted, which
/// is the one failure this whole mechanism exists to prevent.
///
/// Entries of other kinds are ignored rather than refused: the entry
/// alphabet is shared with the peer semantics, and this gate is only the
/// authority on the ones it writes.
///
/// **Every key is canonicalised as it is folded** (issue #643), which is
/// what makes that fix safe to deploy onto a node whose journal already
/// has entries in it. Nothing on disk is rewritten and no entry is
/// migrated: the file stays append-only, exactly as ADR 0005 has it, and
/// an old line still decodes. It is the *fold* that normalises, so a
/// watermark written under a pre-#643 build is recovered under the key
/// this build files it by, rather than orphaned at a spelling nothing
/// looks up any more -- and an orphaned watermark is worse than the bug
/// it was meant to fix, since a channel recovered at `None` accepts every
/// nonce its client already spent.
///
/// The componentwise `max` above is what makes the merge sound in the one
/// case where a pre-#643 journal holds *several* spellings of one
/// channel -- i.e. a journal where the defect was actually exercised. They
/// collapse into one key at the highest nonce and amount either of them
/// ever reached, so the upgrade can only ever tighten what this gate will
/// accept next, never loosen it.
///
/// **Signature retention (issue #1218).** Each channel's fold keeps a
/// stack of every [`LiveClaim`] it has pushed, not just the current one:
/// [`JournalEntry::InboundClaimRolledBack`] carries the watermark it
/// restores but -- unlike [`JournalEntry::InboundClaimAccepted`] -- no
/// signature, so the only place that signature still exists once the
/// rollback that undid its claim has been folded is the entry *before* it
/// in this same stack. `Self::roll_back` (this gate's only writer of that
/// entry kind) always journals it immediately after the acceptance it
/// undoes, so popping the stack recovers exactly the claim being restored
/// to, without needing to trust the rollback entry's own nonce/amount as
/// anything more than a diagnostic. A reset clears the whole stack: it
/// erases the channel outright rather than restoring an earlier claim.
fn replay_watermarks(entries: &[JournalEntry]) -> HashMap<String, LiveClaim> {
    let mut history: HashMap<String, Vec<LiveClaim>> = HashMap::new();
    for entry in entries {
        match entry {
            JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                signature,
            } => {
                let stack = history
                    .entry(canonical_channel_key(channel_id))
                    .or_default();
                let previous = stack
                    .last()
                    .map(|record| record.watermark)
                    .unwrap_or(Watermark {
                        nonce: 0,
                        cumulative_amount: 0,
                    });
                stack.push(LiveClaim {
                    watermark: Watermark {
                        nonce: previous.nonce.max(*nonce),
                        cumulative_amount: previous.cumulative_amount.max(*cumulative_amount),
                    },
                    signature: signature.clone(),
                });
            }
            // Issue #977: a channel's deterministic on-chain address means
            // a reopened channel reuses its settled predecessor's key, so a
            // reset must be able to erase what was folded in *before* it in
            // this same replay, not merely refuse to add anything new.
            // Entries are folded in the order they were appended
            // (`Journal::read_all`), so clearing the whole stack here and
            // letting a later `InboundClaimAccepted` start a fresh one is
            // exactly "this channel's watermark starts clean again from
            // this point on" -- the same effect the reset had when it was
            // first accepted, reproduced on every replay.
            JournalEntry::InboundClaimWatermarkReset { channel_id } => {
                history.remove(&canonical_channel_key(channel_id));
            }
            // Issue #1012: a rollback exists precisely to move a
            // watermark DOWN, which componentwise `max` above would
            // silently undo -- so, like the reset above, this SETS the
            // channel's watermark directly rather than folding into it, by
            // popping the accepted claim it undoes off this channel's
            // stack. Sound for the same reason: `Self::roll_back` only
            // ever writes this entry immediately after the
            // `InboundClaimAccepted` it undoes, so replay sees the two in
            // the same order they were decided in, and the entry left on
            // top of the stack afterward is exactly the claim being
            // restored to.
            JournalEntry::InboundClaimRolledBack { channel_id, .. } => {
                let key = canonical_channel_key(channel_id);
                if let Some(stack) = history.get_mut(&key) {
                    stack.pop();
                    if stack.is_empty() {
                        history.remove(&key);
                    }
                }
            }
            _ => continue,
        }
    }
    history
        .into_iter()
        .filter_map(|(key, mut stack)| stack.pop().map(|record| (key, record)))
        .collect()
}

/// Verify a claim's signature against the counterparty `channels` records
/// for the channel it names -- the gate's last stage, run only once
/// structure, freshness and value have all passed (issue #506/#544, #558).
/// The channel lookup belongs to this stage rather than ahead of it
/// precisely because it is the *signature's* missing half: a replay or an
/// underpayment is still refused for what it is, before this connector
/// spends any cryptographic work, exactly as #544 ordered it.
///
/// Returns the verified signature's raw bytes -- decoded here anyway to
/// check it, and what the journal entry recording this claim's acceptance
/// carries (issue #605/#425), so nothing downstream has to re-parse the
/// claim's chain-specific wire encoding to learn them -- together with the
/// resolved channel's [`DepositFloor`], which [`check_collateral`] judges
/// the claim's amount against next (issue #646). Both come out of the one
/// resolution this stage already performs; neither costs a second lookup.
async fn verify_claim_signature(
    channels: &ClientChannelRegistry,
    claim: &ClientClaim,
    requester: &str,
) -> Result<VerifiedClaim, ClaimIngestRejection> {
    match claim {
        ClientClaim::Evm(claim) => verify_evm_claim_signature(channels, claim, requester).await,
        ClientClaim::Solana(claim) => {
            verify_solana_claim_signature(channels, claim, requester).await
        }
    }
}

/// Report a resolution that produced neither a channel nor a definite
/// absence, as the refusal that says which of the two things went wrong
/// (issue #613).
///
/// A failed lookup and a withheld one are separate variants rather than one
/// with a reason string, because the two are separately *countable*: an
/// operator wants "how often is my endpoint failing" and "how often am I
/// budgeting somebody" as different numbers, and a metric derived from a
/// string is a metric derived from prose.
fn resolution_refusal(error: ChannelResolutionError) -> ClaimIngestRejection {
    match error {
        ChannelResolutionError::LookupFailed(failure) => {
            ClaimIngestRejection::ChannelLookupFailed(failure.0)
        }
        ChannelResolutionError::Budgeted(exhausted) => {
            ClaimIngestRejection::LookupBudgetExhausted {
                bound: exhausted.bound,
                allowance: exhausted.allowance,
                window_secs: exhausted.window.as_secs(),
                max_wait_ms: exhausted.max_wait.as_millis() as u64,
            }
        }
        ChannelResolutionError::Terminal(terminal) => {
            ClaimIngestRejection::ChannelTerminal(terminal.0)
        }
    }
}

/// What survives [`verify_claim_signature`]: the signature the journal
/// records, what the channel it was checked against can pay, and the
/// already-decoded on-chain key that channel was found under.
///
/// The key is carried rather than re-derived because it has *already* been
/// decoded, once, to perform the lookup the signature was checked against.
/// Decoding it a second time in [`check_collateral`] would mean writing a
/// failure branch for a case that cannot arise, which is worse than useless:
/// it is untestable, and it invites a reader to believe it can happen.
struct VerifiedClaim {
    signature: Vec<u8>,
    deposit_floor: DepositFloor,
    channel: ResolvedChannelKey,
}

/// The on-chain identifier a verified claim's channel was resolved under,
/// already decoded and already known to name a channel this connector can
/// be paid on.
enum ResolvedChannelKey {
    Evm([u8; 32]),
    Solana([u8; 32]),
}

/// client-edge-spec.md §1.3 step 5, *collateral binding* (issue #646): the
/// claim's cumulative amount must not exceed what its channel's
/// counterparty has deposited on chain -- the same bound
/// `TokenNetwork.claimFromChannel` and `packages/solana-program`'s claim
/// handler enforce at redemption, evaluated here so this connector refuses
/// unpayable work *before* rendering service instead of discovering it
/// after.
///
/// `floor` is a lower bound, never a reading (deposits only grow), so a
/// breach is not yet a refusal: the chain is asked once more through
/// [`ClientChannelRegistry::refresh_evm`]/`refresh_solana`, and a payer who
/// topped up since the channel was first resolved has this very claim
/// honoured rather than being told to retry. A channel the refreshed
/// reading no longer vouches for at all -- settled since, mint changed --
/// is [`ClaimIngestRejection::UnknownChannel`], the same answer it would
/// have got had it never been cached (issue #649).
///
/// Refusing changes nothing: no watermark moves and nothing is journaled,
/// so the identical claim, at the identical nonce, is good again the moment
/// the deposit covers it. That is verbatim the semantics
/// `packages/solana-program/src/processor.rs` documents for its own version
/// of this check -- *"a participant who intends to spend more can deposit
/// first and resubmit the claim, since a rejected claim leaves the stored
/// nonce untouched"*.
///
/// That property is also why the re-read is rate-limited rather than
/// unconditional (`ChannelLivenessPolicy::min_reattempt_interval`): a claim
/// that consumes nothing can be re-presented forever, so an unconditional
/// re-read would make one undercollateralized claim an unlimited free chain
/// read for whoever holds an underfunded channel. Inside the interval the
/// memoised floor answers instead, which refuses exactly the same claim for
/// exactly the same reason; the only cost is that a counterparty who
/// deposits mid-interval waits it out before their resubmission is
/// honoured, seconds rather than a restart.
///
/// `credited` (issue #700) raises the ceiling the same way a deposit does:
/// what this connector has separately committed to pay this channel's
/// counterparty back, from [`ClientClaimGate::credited`]. `0` for a gate
/// with no payout ledger configured, or for a channel nothing has ever been
/// paid out on -- exactly this check's pre-#700 behaviour. Like the
/// deposit, it only ever grows (a payout ledger's cumulative total is
/// monotonic, `connector_runtime::ClaimBook::record_fulfillment`), so the
/// same reasoning that makes a cached deposit safe to compare against
/// applies to it too: it can only produce a false refusal, never a false
/// accept.
async fn check_collateral(
    channels: &ClientChannelRegistry,
    claim: &ClientClaim,
    verified: &VerifiedClaim,
    requester: &str,
    credited: u64,
) -> Result<(), ClaimIngestRejection> {
    let claimed = claim.transferred_amount();
    if verified.deposit_floor.covers_with_credit(claimed, credited) {
        return Ok(());
    }

    let refreshed = match &verified.channel {
        ResolvedChannelKey::Evm(channel_id) => channels
            .refresh_evm(channel_id, requester)
            .await
            .map(|channel| channel.map(|channel| channel.deposit_floor)),
        ResolvedChannelKey::Solana(channel_account) => channels
            .refresh_solana(channel_account, requester)
            .await
            .map(|channel| channel.map(|channel| channel.deposit_floor)),
    };

    match refreshed {
        Ok(Some(floor)) if floor.covers_with_credit(claimed, credited) => Ok(()),
        Ok(Some(floor)) => Err(ClaimIngestRejection::Undercollateralized {
            claimed,
            // `Unknown` covers every amount, so a floor that reached this
            // arm is always a number. Asserted rather than defaulted: a
            // refusal that quoted a deposit of `0` it had not read would be
            // telling a payer something untrue about their own channel, and
            // there is no honest fallback value to print instead.
            deposited: floor
                .deposit()
                .expect("a deposit floor that failed to cover an amount is never Unknown"),
        }),
        Ok(None) => Err(ClaimIngestRejection::UnknownChannel),
        Err(error) => {
            tracing::warn!(
                channel = %claim.channel_key(),
                error = %error,
                "refusing a client claim: could not re-read its channel's on-chain deposit"
            );
            Err(resolution_refusal(error))
        }
    }
}

async fn verify_evm_claim_signature(
    channels: &ClientChannelRegistry,
    claim: &EvmClientClaim,
    requester: &str,
) -> Result<VerifiedClaim, ClaimIngestRejection> {
    // An id that is not a 32-byte `channelId` cannot be a channel this
    // connector recorded, and cannot be one any chain could resolve either
    // -- so it is unknown rather than merely unverifiable, and is settled
    // here without spending a lookup on it.
    let Some(channel_id) = decode_hex_bytes::<32>(&claim.channel_id) else {
        return Err(ClaimIngestRejection::UnknownChannel);
    };
    let channel = match channels.evm(&channel_id, requester).await {
        Ok(Some(channel)) => channel,
        Ok(None) => return Err(ClaimIngestRejection::UnknownChannel),
        // Loud, per issue #556: an operator has to be able to tell "my
        // chain endpoint is down, so no *new* channel can be recognised"
        // apart from "someone is claiming on channels that do not exist".
        // The claim is refused either way.
        //
        // A lookup this connector *declined* to make (issue #613) is a
        // third thing again, and it has already logged itself, with the
        // signer and the allowance it hit -- so it is not logged twice
        // here, only reported distinguishably.
        Err(error) => {
            if let ChannelResolutionError::LookupFailed(failure) = &error {
                tracing::warn!(
                    channel_id = %claim.channel_id,
                    error = %failure,
                    "refusing a client claim: could not resolve its channel's counterparty"
                );
            }
            return Err(resolution_refusal(error));
        }
    };

    // `lockedAmount`/`locksRoot` are read from the claim because they are
    // material the counterparty signed over (ADR 0004 hashes both, as
    // zeros), not because the claim is trusted about them: a value the
    // signer did not sign simply produces a digest their signature does
    // not recover under. The signer and the EIP-712 domain are the two the
    // claim gets no say in, and both come from `channel` below.
    let Some(locks_root) = decode_hex_bytes::<32>(&claim.locks_root) else {
        return Err(ClaimIngestRejection::SignatureInvalid);
    };
    let Ok(locked_amount) = claim.locked_amount.parse::<u128>() else {
        return Err(ClaimIngestRejection::SignatureInvalid);
    };
    let Some(signature) = decode_hex_bytes::<65>(&claim.signature) else {
        return Err(ClaimIngestRejection::SignatureInvalid);
    };

    let proof = EvmBalanceProof {
        channel_id,
        nonce: claim.nonce,
        transferred_amount: u128::from(claim.transferred_amount),
        locked_amount,
        locks_root,
        chain_id: channel.chain_id,
        token_network_address: channel.token_network_address,
    };
    if verify_evm_balance_proof(&proof, &signature, &channel.counterparty) {
        Ok(VerifiedClaim {
            signature: signature.to_vec(),
            deposit_floor: channel.deposit_floor,
            channel: ResolvedChannelKey::Evm(channel_id),
        })
    } else {
        Err(ClaimIngestRejection::SignatureInvalid)
    }
}

/// Cross-check a Solana claim's self-declared `cluster` against the cluster
/// this node actually settles on (issue #975).
///
/// # Why this field, and only this field
///
/// A Solana claim declares two things about where it lives: `programId` and
/// `cluster`. They are not the same kind of fact, and only one of them
/// needs checking here.
///
/// `programId` is **signed over** since ADR 0053. The verifier rebuilds the
/// balance-proof message from the *resolved channel's* program id, never
/// from the claim, so a claim whose signature verifies is a claim its payer
/// signed for this node's program whatever its `programId` field says. What
/// the payer must write there is pinned (`client-edge-spec.md` §1.3: the
/// settlement program the `channelAccount` lives under), but the field grants
/// nothing and gates nothing, so a disagreement is handled as a warning at
/// the point where the authoritative value is in scope -- see
/// [`verify_solana_claim_signature`], which also records why it is not yet a
/// refusal.
///
/// `cluster` can never be signed over. A Solana program knows its own id
/// but has no way to learn which cluster it is running on, so it could not
/// rebuild a message containing one -- ADR 0053 says exactly this, and it
/// is why the cluster stayed out of the signed bytes. The field is
/// unsignable by construction, supplied by the payer, and this off-chain
/// comparison against the node's own configuration is the only check it can
/// ever get.
///
/// # When it passes without comparing
///
/// `Ok(())` when there is nothing to disagree with, which is two cases:
/// the claim omits the optional `cluster` and so declares nothing, or
/// `configured` is `None` because this node cannot tell which cluster it is
/// on (no `[settlement.solana]` table, or an `rpc_url` naming no cluster
/// this connector recognises -- a third-party RPC provider's, say). Passing
/// in the second case is deliberate: the alternative is guessing, and a
/// wrong guess refuses every genuine claim the node ever receives.
fn check_solana_cluster(
    configured: Option<&'static str>,
    claim: &SolanaClientClaim,
) -> Result<(), ClaimIngestRejection> {
    let (Some(declared), Some(configured)) = (&claim.cluster, configured) else {
        return Ok(());
    };
    if declared != configured {
        return Err(ClaimIngestRejection::SolanaClusterMismatch {
            declared: declared.clone(),
            configured,
        });
    }
    Ok(())
}

async fn verify_solana_claim_signature(
    channels: &ClientChannelRegistry,
    claim: &SolanaClientClaim,
    requester: &str,
) -> Result<VerifiedClaim, ClaimIngestRejection> {
    // An id that is not a 32-byte Solana account cannot be a channel this
    // connector recorded, and cannot be one any chain could resolve either
    // -- so it is unknown rather than merely unverifiable, and is settled
    // here without spending a lookup on it (mirrors `verify_evm_claim_signature`).
    let Some(channel_account) = decode_base58_bytes::<32>(&claim.channel_account) else {
        return Err(ClaimIngestRejection::UnknownChannel);
    };

    // Before spending a lookup on the claim's channel (issue #975): like
    // the decode above, a disagreement here is a fact this connector
    // already knows without a chain read.
    check_solana_cluster(channels.solana_cluster(), claim)?;

    // Declared, or -- for a channel nothing declared -- resolved from the
    // chain via a registered `ClaimChain::Solana` source (issue #631).
    let channel = match channels.solana(&channel_account, requester).await {
        Ok(Some(channel)) => channel,
        Ok(None) => return Err(ClaimIngestRejection::UnknownChannel),
        // Loud, per issue #556/#631: an operator has to be able to tell
        // "my chain endpoint is down, so no *new* channel can be
        // recognised" apart from "someone is claiming on channels that do
        // not exist". The claim is refused either way -- and a lookup this
        // connector declined to make (issue #613) is a third thing again,
        // already logged where it was declined.
        Err(error) => {
            if let ChannelResolutionError::LookupFailed(failure) = &error {
                tracing::warn!(
                    channel_account = %claim.channel_account,
                    error = %failure,
                    "refusing a client claim: could not resolve its channel's counterparty"
                );
            }
            return Err(resolution_refusal(error));
        }
    };

    // The claim's own `programId`, against the program its channel actually
    // lives under (issue #975). Said out loud, and deliberately *not* a
    // refusal.
    //
    // What the field must carry is now pinned: the settlement program the
    // `channelAccount` lives under (`client-edge-spec.md` §1.3), which is
    // the same 32 bytes ADR 0053 binds into the signed balance proof, and
    // the committed wire vector `peer_carriage.claim_solana` demonstrates it
    // (issue #1127, `schema_version` 2).
    //
    // Still not a refusal, and the reason has moved. It is no longer "the
    // contract does not say"; it is that the contract said something else
    // until now. The vector declared the *system program* here for its whole
    // life, so a payer that conformed to the published contract is sending a
    // value this comparison rejects, and a connector cannot see which payers
    // those are -- the client edge is where buyers it has never heard of
    // arrive. Refusing would also refuse money it can actually collect: the
    // signature below is verified against `channel.program_id`, so a claim
    // that survives this function is one whose payer signed for this node's
    // program whatever they wrote in this field.
    //
    // Promotion is therefore gated on adoption, not on this repository:
    // issue #1127 step 4, once payers are known to emit the pinned value.
    // Landing it before then is the ADR 0041 failure shape -- one change
    // taking the whole devnet Solana paid-write path dark at the moment the
    // tag moves.
    //
    // Warned about because issue #975's real complaint is that a
    // disagreement between a claim's label and the chain it was paid on is
    // invisible to both parties -- "the payer sees a FULFILL, the operator
    // sees nothing at all". This is the operator seeing something, and it is
    // also how an operator learns whether their own payers have adopted.
    if decode_base58_bytes::<32>(&claim.program_id) != Some(channel.program_id) {
        tracing::warn!(
            channel_account = %claim.channel_account,
            declared_program_id = %claim.program_id,
            "a client claim declares a programId that is not the program its channel lives \
             under; accepting it on the strength of its signature, which is checked against \
             the channel's own program (ADR 0053)"
        );
    }

    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    let Ok(signature) = BASE64.decode(&claim.signature) else {
        return Err(ClaimIngestRejection::SignatureInvalid);
    };

    if verify_solana_balance_proof(
        &channel.program_id,
        &channel_account,
        claim.nonce,
        claim.transferred_amount,
        &signature,
        &channel.counterparty,
    ) {
        Ok(VerifiedClaim {
            signature,
            deposit_floor: channel.deposit_floor,
            channel: ResolvedChannelKey::Solana(channel_account),
        })
    } else {
        Err(ClaimIngestRejection::SignatureInvalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::test_source::FakeChannelSource;
    use crate::channels::EvmChannel;
    use connector_runtime::{FileJournal, InMemoryJournal};
    use connector_signer::{derive_evm_address, evm_balance_proof_digest, to_hex, Address};
    use libsecp256k1::{Message, PublicKey, SecretKey};
    use std::sync::Arc;

    const EVM_CHAIN_ID: u64 = 8453;
    const EVM_TOKEN_NETWORK_ADDRESS: [u8; 20] = [0x42; 20];
    const SOLANA_CHANNEL_ACCOUNT: [u8; 32] = [3u8; 32];
    /// The settlement program [`test_channels`]'s Solana channel lives
    /// under, and therefore the program every genuine claim below signs
    /// (ADR 0053). Base58 `US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx`.
    const SOLANA_CHANNEL_PROGRAM_ID: [u8; 32] = [7u8; 32];

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The channels these tests claim against, each recorded with the
    /// fixed test keypair below as its counterparty (issue #558) -- a claim
    /// on any other channel, or signed by any other key, is refused.
    fn test_channels() -> ClientChannelRegistry {
        let (_secret, address) = evm_signer();
        let channel = EvmChannel {
            counterparty: address,
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            // Declared, so no deposit is knowable -- the exemption of
            // issue #646, exactly what `connector-cli` records from
            // `[[client_channels]]`.
            deposit_floor: DepositFloor::Unknown,
        };
        let mut channels = ClientChannelRegistry::new();
        channels
            .record_evm(&channel_id(), channel)
            .expect("a 32-byte hex channel id");
        channels
            .record_evm(&second_channel_id(), channel)
            .expect("a 32-byte hex channel id");
        channels
            .record_solana(
                &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
                &base58_encode(&solana_signer().public.to_bytes()),
                &base58_encode(&SOLANA_CHANNEL_PROGRAM_ID),
            )
            .expect("a 32-byte base58 channel account");
        channels
    }

    /// A gate with a record of [`test_channels`] and nothing else, over a
    /// journal that lives only as long as the gate does. Every test below
    /// that is not about durability uses this; the durability tests build
    /// their own gates over a [`FileJournal`] so that a "restart" is a
    /// second gate on the same path, not a mocked one.
    fn gate() -> ClientClaimGate {
        gate_over(test_channels())
    }

    /// A gate over `channels`, journaling somewhere that lives no longer
    /// than the test does. Tests about *which* claims a gate accepts use
    /// this; that an accepted watermark outlives the process is the
    /// `durability` module's own subject, and it uses a real file.
    fn gate_over(channels: ClientChannelRegistry) -> ClientClaimGate {
        ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
    }

    /// A fixed, deterministic EVM keypair -- deterministic on purpose, since
    /// these tests assert on *whether* a signature verifies, not on which
    /// specific key produced it.
    fn evm_signer() -> (SecretKey, Address) {
        let secret = SecretKey::parse(&[9u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        (secret, derive_evm_address(&public.serialize()))
    }

    /// Sign `digest` exactly the way a real EVM wallet would (a 65-byte
    /// `r || s || v` signature, `v` in the conventional `{27, 28}` range).
    fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
        let message = Message::parse(digest);
        let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
        let mut bytes = signature.serialize().to_vec();
        let recovery_byte: u8 = recovery_id.into();
        bytes.push(recovery_byte + 27);
        bytes
    }

    /// An EVM claim JSON carrying whatever `signature`/`signer_address` hex
    /// strings are given verbatim -- the low-level builder every EVM test
    /// helper below goes through, so a test can substitute a wrong,
    /// corrupted or absent value without hand-writing the whole claim.
    fn evm_claim_json_with(
        channel_id: &str,
        nonce: u64,
        transferred_amount: u64,
        signature_hex: &str,
        signer_address_hex: &str,
        chain_fields: &str,
    ) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "evm",
                "messageId": "msg-{nonce}",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "peer-bob",
                "channelId": "{channel_id}",
                "nonce": {nonce},
                "transferredAmount": "{transferred_amount}",
                "lockedAmount": "0",
                "locksRoot": "0x{zeros}",
                "signature": "{signature_hex}",
                "signerAddress": "{signer_address_hex}"
                {chain_fields}
            }}"#,
            zeros = "0".repeat(64),
        )
    }

    /// An EVM claim JSON with a genuine EIP-712 signature produced by
    /// `secret` and declaring `declared_signer` as its own `signerAddress`
    /// -- the two are separable on purpose (issue #558): a forger signs
    /// perfectly well with a key of their own and declares whatever they
    /// like, so a test needs to be able to build exactly that.
    fn evm_claim_json_signed_by(
        secret: &SecretKey,
        declared_signer: &Address,
        channel_id: &str,
        nonce: u64,
        transferred_amount: u64,
    ) -> String {
        let proof = EvmBalanceProof {
            channel_id: decode_hex_bytes::<32>(channel_id).expect("test channel_id is valid hex"),
            nonce,
            transferred_amount: u128::from(transferred_amount),
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let signature = sign_evm(secret, &evm_balance_proof_digest(&proof));
        evm_claim_json_with(
            channel_id,
            nonce,
            transferred_amount,
            &format!("0x{}", hex_encode(&signature)),
            &to_hex(declared_signer),
            &format!(
                r#", "chainId": {EVM_CHAIN_ID}, "tokenNetworkAddress": "{}""#,
                to_hex(&EVM_TOKEN_NETWORK_ADDRESS)
            ),
        )
    }

    /// An EVM claim JSON with a genuine EIP-712 signature over its own
    /// fields, produced by [`evm_signer`] -- so every test using it exercises
    /// the real verification path (issue #506/#544), not a bypass.
    fn evm_claim_json(channel_id: &str, nonce: u64, transferred_amount: u64) -> String {
        let (secret, address) = evm_signer();
        let proof = EvmBalanceProof {
            channel_id: decode_hex_bytes::<32>(channel_id).expect("test channel_id is valid hex"),
            nonce,
            transferred_amount: u128::from(transferred_amount),
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let signature = sign_evm(&secret, &evm_balance_proof_digest(&proof));
        evm_claim_json_with(
            channel_id,
            nonce,
            transferred_amount,
            &format!("0x{}", hex_encode(&signature)),
            &to_hex(&address),
            &format!(
                r#", "chainId": {EVM_CHAIN_ID}, "tokenNetworkAddress": "{}""#,
                to_hex(&EVM_TOKEN_NETWORK_ADDRESS)
            ),
        )
    }

    fn channel_id() -> String {
        format!("0x{}", "ab".repeat(32))
    }

    /// A second recorded channel, for the tests that need two.
    fn second_channel_id() -> String {
        format!("0x{}", "cd".repeat(32))
    }

    /// A channel this connector has no record of -- well-formed as an id,
    /// simply never recorded.
    fn unrecorded_channel_id() -> String {
        format!("0x{}", "ef".repeat(32))
    }

    #[tokio::test]
    async fn a_fresh_claim_is_accepted() {
        let gate = gate();
        let result = gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await;
        assert!(result.is_ok());
    }

    /// Issue #1218: once a claim is accepted, its nonce, cumulative amount
    /// and signature are readable back off the gate -- exactly what the
    /// operator surface's `GET /claims` and `redeem-latest` need, and what
    /// this gate could not previously answer at all.
    #[tokio::test]
    async fn an_accepted_claims_nonce_amount_and_signature_are_readable_back() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 1, 100), 0)
            .await
            .expect("claim accepted");

        let (nonce, cumulative_amount, signature) = gate
            .latest_inbound_claim(&format!("evm:{channel}"))
            .expect("an accepted claim is redeemable");
        assert_eq!(nonce, 1);
        assert_eq!(cumulative_amount, 100);
        assert_eq!(
            signature.len(),
            65,
            "the raw 65-byte EIP-712 signature, not its hex text"
        );

        let channels = gate.accepted_channels();
        assert_eq!(
            channels,
            vec![(
                format!("evm:{channel}"),
                Watermark {
                    nonce: 1,
                    cumulative_amount: 100
                }
            )]
        );
    }

    /// A channel this gate has never accepted a claim on has nothing to
    /// redeem -- distinguishing "never claimed" from "claimed for zero" is
    /// exactly what `Option::None` here is for.
    #[tokio::test]
    async fn a_channel_never_claimed_on_has_no_latest_inbound_claim() {
        let gate = gate();
        assert_eq!(gate.latest_inbound_claim("evm:0xnotachannel"), None);
        assert!(gate.accepted_channels().is_empty());
    }

    #[tokio::test]
    async fn a_replayed_nonce_is_rejected_without_touching_the_watermark() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 5, 500), 0)
            .await
            .expect("first claim accepted");

        let replay = gate.ingest(&evm_claim_json(&channel, 5, 999), 0).await;
        assert_eq!(replay, Err(ClaimIngestRejection::NonceNotAdvancing));

        // The watermark still holds at nonce 5 -- a genuinely advancing
        // claim after the rejected replay is judged against it, not against
        // whatever the rejected replay tried to claim.
        let next = gate.ingest(&evm_claim_json(&channel, 6, 500), 0).await;
        assert!(next.is_ok());
    }

    #[tokio::test]
    async fn an_amount_going_backwards_is_rejected() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 1, 500), 0)
            .await
            .expect("first claim accepted");

        let result = gate.ingest(&evm_claim_json(&channel, 2, 100), 0).await;
        assert_eq!(result, Err(ClaimIngestRejection::AmountNotAdvancing));
    }

    #[tokio::test]
    async fn the_watermark_never_advances_on_a_rejected_claim() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 5, 500), 0)
            .await
            .expect("first claim accepted");
        gate.ingest(&evm_claim_json(&channel, 5, 999), 0)
            .await
            .unwrap_err(); // replay, rejected
        gate.ingest(&evm_claim_json(&channel, 6, 100), 0)
            .await
            .unwrap_err(); // amount regresses vs. watermark 500

        // Watermark is still exactly (5, 500): a claim of nonce 6 / amount
        // 500 (equal, not less) still advances cleanly.
        assert!(gate
            .ingest(&evm_claim_json(&channel, 6, 500), 0)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn different_channels_have_independent_watermarks() {
        let gate = gate();
        gate.ingest(&evm_claim_json(&channel_id(), 5, 500), 0)
            .await
            .expect("first channel");

        let result = gate
            .ingest(&evm_claim_json(&second_channel_id(), 1, 10), 0)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_mina_claim_is_rejected_distinguishably_from_malformed() {
        let gate = gate();
        let json = r#"{
            "version": "1.0",
            "blockchain": "mina",
            "messageId": "claim-3",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-dave",
            "zkAppAddress": "irrelevant",
            "tokenId": "1",
            "balanceCommitment": "abc",
            "nonce": 1,
            "proof": "AAAA",
            "salt": "salt"
        }"#;

        assert_eq!(gate.ingest(json, 0).await, Err(ClaimIngestRejection::Mina));
    }

    #[tokio::test]
    async fn a_structurally_invalid_claim_is_rejected_as_malformed() {
        let gate = gate();
        let result = gate
            .ingest(r#"{"version": "1.0", "blockchain": "evm"}"#, 0)
            .await;
        assert!(matches!(result, Err(ClaimIngestRejection::Malformed(_))));
    }

    #[tokio::test]
    async fn a_first_claim_advancing_by_at_least_the_price_is_accepted() {
        let gate = gate();
        let result = gate
            .ingest(&evm_claim_json(&channel_id(), 1, 100), 100)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_first_claim_advancing_by_less_than_the_price_is_underpayment() {
        let gate = gate();
        let result = gate
            .ingest(&evm_claim_json(&channel_id(), 1, 99), 100)
            .await;
        assert_eq!(
            result,
            Err(ClaimIngestRejection::Underpayment {
                advanced: 99,
                price: 100
            })
        );
    }

    #[tokio::test]
    async fn an_underpaying_claim_does_not_advance_the_watermark() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 1, 99), 100)
            .await
            .unwrap_err();

        // A corrected resubmission is judged against the same (untouched)
        // baseline -- nonce 1 would otherwise fail as a replay if the
        // rejected claim above had advanced anything.
        let result = gate.ingest(&evm_claim_json(&channel, 1, 100), 100).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_later_claim_only_needs_to_cover_the_price_since_the_watermark() {
        let gate = gate();
        let channel = channel_id();
        gate.ingest(&evm_claim_json(&channel, 1, 100), 100)
            .await
            .expect("first claim covers the price");

        // Advances by only 50 past the watermark of 100 -- underpayment
        // against a price of 100, even though the claim's own cumulative
        // transferredAmount (150) is larger than the price in isolation.
        let result = gate.ingest(&evm_claim_json(&channel, 2, 150), 100).await;
        assert_eq!(
            result,
            Err(ClaimIngestRejection::Underpayment {
                advanced: 50,
                price: 100
            })
        );

        // Advancing by exactly the price is accepted.
        assert!(gate
            .ingest(&evm_claim_json(&channel, 2, 200), 100)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_zero_price_route_charges_nothing() {
        let gate = gate();
        let result = gate.ingest(&evm_claim_json(&channel_id(), 1, 0), 0).await;
        assert!(result.is_ok());
    }

    // -- Signature verification (issue #506/#544) --

    #[tokio::test]
    async fn a_genuine_evm_signature_is_accepted() {
        let gate = gate();
        let result = gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await;
        assert!(result.is_ok());
    }

    /// The forger of issue #558: a well-formed claim, genuinely signed,
    /// self-consistent -- and signed by a key that is not the channel's
    /// counterparty. Before #558 this was *accepted*, because the claim was
    /// checked against the signer it declared for itself.
    #[tokio::test]
    async fn an_evm_claim_signed_by_a_key_that_is_not_the_channels_counterparty_is_rejected() {
        let gate = gate();

        // An attacker's own freshly generated keypair, declared as this
        // claim's signer. The signature genuinely recovers to it; it is
        // simply not a party to the channel being claimed against.
        let forger_secret = SecretKey::parse(&[0x5a; 32]).unwrap();
        let forger_address =
            derive_evm_address(&PublicKey::from_secret_key(&forger_secret).serialize());
        let (_genuine_secret, counterparty) = evm_signer();
        assert_ne!(
            forger_address, counterparty,
            "the forger must not accidentally be the counterparty"
        );

        let claim =
            evm_claim_json_signed_by(&forger_secret, &forger_address, &channel_id(), 1, 100);

        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::SignatureInvalid)
        );
    }

    /// A forged claim is refused *and* leaves nothing behind: the channel's
    /// real counterparty is judged against the same baseline afterwards.
    #[tokio::test]
    async fn a_forged_claim_advances_no_watermark() {
        let gate = gate();
        let forger_secret = SecretKey::parse(&[0x5a; 32]).unwrap();
        let forger_address =
            derive_evm_address(&PublicKey::from_secret_key(&forger_secret).serialize());
        gate.ingest(
            &evm_claim_json_signed_by(&forger_secret, &forger_address, &channel_id(), 9, 900),
            0,
        )
        .await
        .unwrap_err();

        // The counterparty's own first claim, at a far lower nonce and
        // amount than the forgery named, is still a fresh first claim.
        assert!(gate
            .ingest(&evm_claim_json(&channel_id(), 1, 100), 0)
            .await
            .is_ok());
    }

    /// A claim's `signerAddress` is not consulted at all -- the registry
    /// decides. A claim declaring the wrong address, but genuinely signed
    /// by the channel's actual counterparty, is accepted: the field is
    /// unverified decoration, and this connector does not act on it either
    /// way.
    #[tokio::test]
    async fn an_evm_claims_declared_signer_field_carries_no_authority() {
        let gate = gate();
        let (secret, _address) = evm_signer();
        let claim = evm_claim_json_signed_by(
            &secret,
            &[0xde; 20], // a declared signer that is nobody
            &channel_id(),
            1,
            100,
        );

        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    /// A claim naming a channel this connector has no record of is refused
    /// -- distinguishably from a bad signature and from an underpayment
    /// (issue #558's AC2).
    #[tokio::test]
    async fn a_claim_on_an_unrecorded_channel_is_refused_as_unknown_channel() {
        let gate = gate();
        let claim = evm_claim_json(&unrecorded_channel_id(), 1, 100);

        let result = gate.ingest(&claim, 0).await;
        assert_eq!(result, Err(ClaimIngestRejection::UnknownChannel));
        assert_ne!(result, Err(ClaimIngestRejection::SignatureInvalid));
        assert!(result.unwrap_err().message().contains("no record of"));
    }

    /// An empty registry is not an open door: a gate with a record of no
    /// channel at all refuses even a perfectly signed claim, rather than
    /// falling back to the claim's own declared signer (issue #558's AC8).
    #[tokio::test]
    async fn a_gate_with_no_recorded_channels_accepts_nothing() {
        let gate = ClientClaimGate::restore(
            ClientChannelRegistry::new(),
            Arc::new(InMemoryJournal::new()),
        )
        .expect("a fresh in-memory journal has nothing to replay");
        assert_eq!(
            gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await,
            Err(ClaimIngestRejection::UnknownChannel)
        );
    }

    /// Issue #556/#502: a channel **nothing declared**, resolved from the
    /// chain, is accepted -- the unaffiliated buyer's path. On a tree
    /// without this change there is no source to consult and this exact
    /// claim is refused `UnknownChannel`, so an operator has to edit
    /// `[[client_channels]]` and restart before anyone new can pay.
    ///
    /// The claim is signed by [`evm_signer`] and the *source* -- standing
    /// in for `TokenNetwork.channels(id)` -- is what names that address as
    /// the channel's counterparty. Nothing here reads the claim's own
    /// `signerAddress`; the forger tests below still pass unchanged.
    #[tokio::test]
    async fn a_claim_on_a_channel_only_the_chain_knows_about_is_accepted() {
        let (_secret, address) = evm_signer();
        let channel_id = decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap();
        let gate = gate_over(ClientChannelRegistry::new().with_source(Arc::new(
            FakeChannelSource::knowing(vec![(
                channel_id,
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]),
        )));

        let accepted = gate
            .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 100), 100)
            .await
            .expect("a channel the chain knows about is payable without a config edit");
        // The canonical key (issue #643): the namespace, then the id's
        // 32 bytes as `0x` plus lower-case hex -- not the literal text the
        // claim spelled its `channelId` with.
        assert_eq!(accepted.channel_key(), format!("evm:0x{}", "ef".repeat(32)));
    }

    /// Issue #661 at the gate: a source that keeps its own durable record
    /// of settlement (the local channel index) refuses the claim as
    /// [`ClaimIngestRejection::ChannelTerminal`], not as
    /// [`ClaimIngestRejection::UnknownChannel`] -- the whole point of the
    /// separate variant is that an operator can tell a buyer's spent
    /// channel from one this connector has never heard of.
    #[tokio::test]
    async fn a_claim_on_a_channel_the_source_records_as_settled_is_refused_as_terminal() {
        let channel_id = decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap();
        let source = Arc::new(FakeChannelSource::knowing(vec![]));
        source.now_terminal(channel_id);
        let gate = gate_over(ClientChannelRegistry::new().with_source(source));

        assert_eq!(
            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 100), 100)
                .await,
            Err(ClaimIngestRejection::ChannelTerminal(format!(
                "channel {} has settled and can never be redeemed again",
                "ef".repeat(32)
            )))
        );
    }

    /// The forger rule survives the new source: a claim signed by a key
    /// that is not what the *chain* holds as the channel's counterparty is
    /// still refused, even though the channel itself resolves.
    #[tokio::test]
    async fn a_claim_signed_by_someone_other_than_the_chains_counterparty_is_still_refused() {
        let channel_id = decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap();
        let forger = SecretKey::parse(&[13u8; 32]).unwrap();
        let forger_address = derive_evm_address(&PublicKey::from_secret_key(&forger).serialize());
        let (_secret, genuine) = evm_signer();
        let gate = gate_over(ClientChannelRegistry::new().with_source(Arc::new(
            FakeChannelSource::knowing(vec![(
                channel_id,
                EvmChannel {
                    counterparty: genuine,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]),
        )));

        let claim =
            evm_claim_json_signed_by(&forger, &forger_address, &unrecorded_channel_id(), 1, 100);
        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::SignatureInvalid)
        );
    }

    /// A source that cannot answer refuses the claim -- it never degrades
    /// to trusting what the claim says about itself -- and says so
    /// distinguishably from a channel that genuinely does not exist, so an
    /// operator can tell an RPC outage from a sender naming channels at
    /// random.
    #[tokio::test]
    async fn a_claim_whose_channel_lookup_fails_is_refused_distinguishably() {
        let gate = gate_over(ClientChannelRegistry::new().with_source(Arc::new(
            FakeChannelSource::unreachable("connection refused"),
        )));

        let result = gate
            .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 100), 0)
            .await;
        assert_eq!(
            result,
            Err(ClaimIngestRejection::ChannelLookupFailed(
                "connection refused".to_string()
            ))
        );
        assert_ne!(result, Err(ClaimIngestRejection::UnknownChannel));
        let message = result.unwrap_err().message();
        assert!(message.contains("could not look up"), "{message}");
        assert!(message.contains("connection refused"), "{message}");
    }

    /// A node whose config declares its channels keeps working while its
    /// chain endpoint is down: the declared record answers, and the broken
    /// source is never consulted. This is the "still start and serve when
    /// the chain is unreachable" requirement at claim level.
    #[tokio::test]
    async fn a_declared_channel_is_still_payable_while_the_chain_is_unreachable() {
        let mut channels = ClientChannelRegistry::new();
        let (_secret, address) = evm_signer();
        channels
            .record_evm(
                &channel_id(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )
            .expect("a 32-byte hex channel id");
        let gate = gate_over(
            channels.with_source(Arc::new(FakeChannelSource::unreachable(
                "connection refused",
            ))),
        );

        assert!(gate
            .ingest(&evm_claim_json(&channel_id(), 1, 100), 0)
            .await
            .is_ok());
    }

    /// An unrecorded channel is refused *after* freshness and value, not
    /// before: #544's ordering is preserved, so an underpaying claim still
    /// costs this ingress no channel lookup or cryptographic work to
    /// refuse (issue #558's AC4).
    #[tokio::test]
    async fn an_underpaying_claim_on_an_unrecorded_channel_is_still_refused_as_underpayment() {
        let gate = gate();
        let result = gate
            .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 99), 100)
            .await;
        assert_eq!(
            result,
            Err(ClaimIngestRejection::Underpayment {
                advanced: 99,
                price: 100
            })
        );
    }

    #[tokio::test]
    async fn an_evm_claim_with_a_corrupted_signature_is_rejected_not_panicking() {
        let gate = gate();
        let (secret, address) = evm_signer();
        let proof = EvmBalanceProof {
            channel_id: decode_hex_bytes::<32>(&channel_id()).unwrap(),
            nonce: 1,
            transferred_amount: 100,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let mut signature = sign_evm(&secret, &evm_balance_proof_digest(&proof));
        signature[0] ^= 0xff;

        let claim = evm_claim_json_with(
            &channel_id(),
            1,
            100,
            &format!("0x{}", hex_encode(&signature)),
            &to_hex(&address),
            &format!(
                r#", "chainId": {EVM_CHAIN_ID}, "tokenNetworkAddress": "{}""#,
                to_hex(&EVM_TOKEN_NETWORK_ADDRESS)
            ),
        );

        let result = gate.ingest(&claim, 0).await;
        assert_eq!(result, Err(ClaimIngestRejection::SignatureInvalid));
    }

    #[tokio::test]
    async fn an_evm_claim_with_a_truncated_signature_is_rejected_not_panicking() {
        let gate = gate();
        let (_secret, address) = evm_signer();
        let claim = evm_claim_json_with(
            &channel_id(),
            1,
            100,
            "0xabcd",
            &to_hex(&address),
            &format!(
                r#", "chainId": {EVM_CHAIN_ID}, "tokenNetworkAddress": "{}""#,
                to_hex(&EVM_TOKEN_NETWORK_ADDRESS)
            ),
        );

        let result = gate.ingest(&claim, 0).await;
        assert_eq!(result, Err(ClaimIngestRejection::SignatureInvalid));
    }

    /// The EIP-712 domain a claim is verified under comes from the channel's
    /// record, never from the claim (issue #558): a claim declaring no
    /// `chainId`/`tokenNetworkAddress` at all still verifies, and a claim
    /// declaring a *different* domain than the one recorded gains nothing by
    /// it -- both are judged against the recorded domain.
    #[tokio::test]
    async fn an_evm_claims_declared_eip712_domain_carries_no_authority() {
        let gate = gate();
        let (secret, address) = evm_signer();
        let proof = EvmBalanceProof {
            channel_id: decode_hex_bytes::<32>(&channel_id()).unwrap(),
            nonce: 1,
            transferred_amount: 100,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let signature = sign_evm(&secret, &evm_balance_proof_digest(&proof));

        let no_declared_domain = evm_claim_json_with(
            &channel_id(),
            1,
            100,
            &format!("0x{}", hex_encode(&signature)),
            &to_hex(&address),
            "",
        );
        assert!(gate.ingest(&no_declared_domain, 0).await.is_ok());

        // The same signature, now declaring a domain it was not produced
        // under. It is still checked against the recorded one, so it still
        // verifies -- the declared fields simply do not participate.
        let wrong_declared_domain = evm_claim_json_with(
            &channel_id(),
            2,
            200,
            &format!(
                "0x{}",
                hex_encode(&sign_evm(
                    &secret,
                    &evm_balance_proof_digest(&EvmBalanceProof {
                        nonce: 2,
                        transferred_amount: 200,
                        ..proof
                    })
                ))
            ),
            &to_hex(&address),
            r#", "chainId": 1, "tokenNetworkAddress": "0x00000000000000000000000000000000000000ff""#,
        );
        assert!(gate.ingest(&wrong_declared_domain, 0).await.is_ok());
    }

    /// A claim signed under a domain that is *not* the channel's recorded
    /// one does not verify -- the recorded domain is the only one this
    /// connector computes a digest under.
    #[tokio::test]
    async fn an_evm_claim_signed_under_another_domain_is_rejected() {
        let gate = gate();
        let (secret, address) = evm_signer();
        let proof = EvmBalanceProof {
            channel_id: decode_hex_bytes::<32>(&channel_id()).unwrap(),
            nonce: 1,
            transferred_amount: 100,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: 1,
            token_network_address: [0xff; 20],
        };
        let signature = sign_evm(&secret, &evm_balance_proof_digest(&proof));

        let claim = evm_claim_json_with(
            &channel_id(),
            1,
            100,
            &format!("0x{}", hex_encode(&signature)),
            &to_hex(&address),
            r#", "chainId": 1, "tokenNetworkAddress": "0x00000000000000000000000000000000000000ff""#,
        );

        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::SignatureInvalid)
        );
    }

    #[tokio::test]
    async fn a_claim_failing_signature_verification_does_not_advance_the_watermark() {
        let gate = gate();
        let channel = channel_id();
        let (_secret, address) = evm_signer();
        let bad_signature_claim = evm_claim_json_with(
            &channel,
            1,
            100,
            "0xabcd",
            &to_hex(&address),
            &format!(
                r#", "chainId": {EVM_CHAIN_ID}, "tokenNetworkAddress": "{}""#,
                to_hex(&EVM_TOKEN_NETWORK_ADDRESS)
            ),
        );
        gate.ingest(&bad_signature_claim, 0).await.unwrap_err();

        // The watermark was never advanced by the rejected claim -- the
        // same nonce/amount is accepted here as a fresh first claim, not
        // refused as a replay.
        let genuine = gate.ingest(&evm_claim_json(&channel, 1, 100), 0).await;
        assert!(genuine.is_ok());
    }

    fn solana_signer() -> ed25519_dalek::Keypair {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::from_seed([13u8; 32]);
        ed25519_dalek::Keypair::generate(&mut rng)
    }

    fn base58_encode(bytes: &[u8]) -> String {
        bs58::encode(bytes).into_string()
    }

    /// A Solana claim on [`test_channels`]'s channel, declaring the program
    /// that channel really lives under -- what a conforming payer sends
    /// (`client-edge-spec.md` §1.3: `programId` names the settlement program
    /// the `channelAccount` lives under).
    ///
    /// It declared the **system program** until issue #1127, matching the
    /// committed wire vector, so every test built on it quietly exercised a
    /// mismatch. The mismatch is worth exercising -- it is
    /// [`a_solana_claim_declaring_a_foreign_program_is_accepted_on_its_signature`]
    /// -- but a test that means to exercise something else should not be
    /// carrying it by accident, and a fixture is the closest thing this crate
    /// has to a statement of what a real payer's claim looks like.
    fn solana_claim_json_with(
        channel_account: &str,
        nonce: u64,
        transferred_amount: u64,
        signature_base64: &str,
        signer_public_key: &str,
    ) -> String {
        solana_claim_json_declaring(
            channel_account,
            &base58_encode(&SOLANA_CHANNEL_PROGRAM_ID),
            nonce,
            transferred_amount,
            signature_base64,
            signer_public_key,
        )
    }

    /// [`solana_claim_json_with`] with the declared `programId` spelled out
    /// by the caller -- for the one test whose subject *is* a claim declaring
    /// a program its channel does not live under.
    fn solana_claim_json_declaring(
        channel_account: &str,
        program_id: &str,
        nonce: u64,
        transferred_amount: u64,
        signature_base64: &str,
        signer_public_key: &str,
    ) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "solana",
                "messageId": "msg-{nonce}",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "peer-carol",
                "programId": "{program_id}",
                "channelAccount": "{channel_account}",
                "nonce": {nonce},
                "transferredAmount": "{transferred_amount}",
                "signature": "{signature_base64}",
                "signerPublicKey": "{signer_public_key}"
            }}"#
        )
    }

    fn genuine_solana_claim_json(
        channel_account_bytes: &[u8; 32],
        nonce: u64,
        transferred_amount: u64,
    ) -> String {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;

        let keypair = solana_signer();
        let message = connector_signer::solana_balance_proof_message(
            &[7u8; 32],
            channel_account_bytes,
            nonce,
            transferred_amount,
        );
        let signature = keypair.sign(&message);
        solana_claim_json_with(
            &base58_encode(channel_account_bytes),
            nonce,
            transferred_amount,
            &BASE64.encode(signature.to_bytes()),
            &base58_encode(&keypair.public.to_bytes()),
        )
    }

    /// [`genuine_solana_claim_json`], carrying an explicit `cluster` -- the
    /// field issue #975 is about. The signature is produced exactly as the
    /// genuine helper produces it, over
    /// [`SOLANA_CHANNEL_PROGRAM_ID`]/account/nonce/amount, because
    /// `cluster` is not in the signed bytes and cannot be: a claim declaring
    /// the wrong cluster is a *genuine* claim wearing a wrong label, which
    /// is the only case worth testing.
    fn genuine_solana_claim_json_with_cluster(
        channel_account_bytes: &[u8; 32],
        nonce: u64,
        transferred_amount: u64,
        cluster: Option<&str>,
    ) -> String {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;

        let keypair = solana_signer();
        let message = connector_signer::solana_balance_proof_message(
            &SOLANA_CHANNEL_PROGRAM_ID,
            channel_account_bytes,
            nonce,
            transferred_amount,
        );
        let signature = keypair.sign(&message);
        let cluster_field = match cluster {
            Some(cluster) => format!(r#", "cluster": "{cluster}""#),
            None => String::new(),
        };
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "solana",
                "messageId": "msg-{nonce}",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "peer-carol",
                "programId": "{}",
                "channelAccount": "{}",
                "nonce": {nonce},
                "transferredAmount": "{transferred_amount}",
                "signature": "{}",
                "signerPublicKey": "{}"
                {cluster_field}
            }}"#,
            base58_encode(&SOLANA_CHANNEL_PROGRAM_ID),
            base58_encode(channel_account_bytes),
            BASE64.encode(signature.to_bytes()),
            base58_encode(&keypair.public.to_bytes()),
        )
    }

    #[tokio::test]
    async fn a_genuine_solana_signature_is_accepted() {
        let gate = gate();
        let claim = genuine_solana_claim_json(&SOLANA_CHANNEL_ACCOUNT, 1, 100);
        let result = gate.ingest(&claim, 0).await;
        assert!(result.is_ok());
    }

    /// The Solana half of issue #558's forger: a genuine Ed25519 signature
    /// over the right message, produced by a key that is not the channel's
    /// recorded counterparty and declared as the claim's own signer. Both
    /// families verify against the registry, not against themselves.
    #[tokio::test]
    async fn a_solana_claim_signed_by_a_key_that_is_not_the_channels_counterparty_is_rejected() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;
        use rand::SeedableRng;

        let gate = gate();
        let forger =
            ed25519_dalek::Keypair::generate(&mut rand::rngs::StdRng::from_seed([99u8; 32]));
        assert_ne!(
            forger.public.to_bytes(),
            solana_signer().public.to_bytes(),
            "the forger must not accidentally be the counterparty"
        );
        let message = connector_signer::solana_balance_proof_message(
            &[7u8; 32],
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
        );
        let signature = forger.sign(&message);

        let claim = solana_claim_json_with(
            &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
            1,
            100,
            &BASE64.encode(signature.to_bytes()),
            &base58_encode(&forger.public.to_bytes()),
        );

        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::SignatureInvalid)
        );
    }

    /// A Solana claim naming a channel account this connector has no record
    /// of is refused as [`ClaimIngestRejection::UnknownChannel`], the same
    /// as its EVM counterpart.
    #[tokio::test]
    async fn a_solana_claim_on_an_unrecorded_channel_is_refused_as_unknown_channel() {
        let gate = gate();
        let claim = genuine_solana_claim_json(&[8u8; 32], 1, 100);
        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::UnknownChannel)
        );
    }

    /// A Solana claim's `signerPublicKey` carries no authority either: a
    /// claim genuinely signed by the recorded counterparty is accepted
    /// however it declares itself.
    #[tokio::test]
    async fn a_solana_claims_declared_signer_field_carries_no_authority() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;

        let gate = gate();
        let signer = solana_signer();
        let message = connector_signer::solana_balance_proof_message(
            &[7u8; 32],
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
        );
        let signature = signer.sign(&message);

        let claim = solana_claim_json_with(
            &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
            1,
            100,
            &BASE64.encode(signature.to_bytes()),
            &base58_encode(&[7u8; 32]),
        );

        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    #[tokio::test]
    async fn a_solana_claim_with_a_corrupted_signature_is_rejected_not_panicking() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;

        let gate = gate();
        let keypair = solana_signer();
        let message = connector_signer::solana_balance_proof_message(
            &[7u8; 32],
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
        );
        let mut signature_bytes = keypair.sign(&message).to_bytes();
        signature_bytes[0] ^= 0xff;

        let claim = solana_claim_json_with(
            &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
            1,
            100,
            &BASE64.encode(signature_bytes),
            &base58_encode(&keypair.public.to_bytes()),
        );

        let result = gate.ingest(&claim, 0).await;
        assert_eq!(result, Err(ClaimIngestRejection::SignatureInvalid));
    }

    // -- Declared cluster: a claim's `cluster` vs. the one this node
    // -- actually settles on (issue #975) --

    /// A gate over [`test_channels`] that also knows which cluster it is on
    /// -- the shape `connector-cli` builds when `[settlement.solana]
    /// rpc_url` names a cluster it recognises.
    fn gate_on_cluster(cluster: &'static str) -> ClientClaimGate {
        gate_over(test_channels().with_solana_cluster(cluster))
    }

    /// Issue #975's defect, at the layer that can see it: a genuinely
    /// signed claim, on a channel this node vouches for, whose body
    /// declares a cluster this node is not on. Before this check the node
    /// accepted it in full and advanced the watermark -- "the payer sees a
    /// FULFILL, the operator sees nothing at all".
    ///
    /// The signature is genuine and stays genuine: `cluster` is not in the
    /// signed bytes and cannot be (a Solana program cannot know its own
    /// cluster), which is exactly why an off-chain comparison is the only
    /// check this field can ever get.
    #[tokio::test]
    async fn a_solana_claim_declaring_a_cluster_this_node_is_not_on_is_refused() {
        let gate = gate_on_cluster("devnet");
        let claim = genuine_solana_claim_json_with_cluster(
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
            Some("mainnet-beta"),
        );
        assert_eq!(
            gate.ingest(&claim, 0).await,
            Err(ClaimIngestRejection::SolanaClusterMismatch {
                declared: "mainnet-beta".to_string(),
                configured: "devnet",
            })
        );
    }

    /// Refusing changes nothing that outlives the request: the watermark
    /// does not move, so the payer who fixes their label is good again at
    /// the same nonce. Asserted because a refusal that silently consumed a
    /// nonce would turn a mislabelled claim into a lost one.
    #[tokio::test]
    async fn a_cluster_mismatch_consumes_no_nonce() {
        let gate = gate_on_cluster("devnet");
        let mislabelled = genuine_solana_claim_json_with_cluster(
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
            Some("testnet"),
        );
        assert!(gate.ingest(&mislabelled, 0).await.is_err());

        let corrected =
            genuine_solana_claim_json_with_cluster(&SOLANA_CHANNEL_ACCOUNT, 1, 100, Some("devnet"));
        assert!(
            gate.ingest(&corrected, 0).await.is_ok(),
            "the same nonce must still be good once the label is corrected"
        );
    }

    /// The agreeing case: same cluster, accepted exactly as before.
    #[tokio::test]
    async fn a_solana_claim_declaring_the_cluster_this_node_is_on_is_accepted() {
        let gate = gate_on_cluster("devnet");
        let claim =
            genuine_solana_claim_json_with_cluster(&SOLANA_CHANNEL_ACCOUNT, 1, 100, Some("devnet"));
        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    /// `cluster` is optional (client-edge-spec.md §1.3), and the committed
    /// wire vector `peer_carriage.claim_solana` omits it. A claim that
    /// declares nothing disagrees with nothing, so a spec-conformant client
    /// is untouched by this check.
    #[tokio::test]
    async fn a_solana_claim_that_declares_no_cluster_is_accepted() {
        let gate = gate_on_cluster("devnet");
        let claim = genuine_solana_claim_json_with_cluster(&SOLANA_CHANNEL_ACCOUNT, 1, 100, None);
        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    /// A node that cannot tell which cluster it is on -- no
    /// `[settlement.solana]`, or an `rpc_url` naming no cluster this
    /// connector recognises -- compares nothing and accepts whatever is
    /// declared. Deliberate: the alternative is guessing, and a wrong guess
    /// refuses every genuine claim the node ever receives.
    #[tokio::test]
    async fn a_node_that_does_not_know_its_cluster_checks_nothing() {
        let gate = gate();
        let claim = genuine_solana_claim_json_with_cluster(
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
            Some("mainnet-beta"),
        );
        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    /// The `programId` half of issue #975, and the reason it is a warning
    /// rather than a refusal: this claim declares a program that is *not*
    /// the one its channel lives under, and it is accepted -- because its
    /// signature is verified against the channel's program (ADR 0053), so
    /// the claim is cryptographically correct and fully redeemable however
    /// its label reads. Refusing it would refuse money this node can collect.
    ///
    /// The mismatch is built here on purpose. It used to be inherited from
    /// `genuine_solana_claim_json`'s own body, which declared the system
    /// program exactly as the committed wire vector did -- so every Solana
    /// test in this file was silently a mismatch test, and this one proved
    /// nothing the others did not. Issue #1127 pinned what `programId` must
    /// carry and fixed both fixtures; what survives is this, the deliberate
    /// case, with the divergence written down where a reader can see it.
    ///
    /// Note what is *not* asserted: that the claim goes through **silently**.
    /// It does not -- `verify_solana_claim_signature` logs a `warn` naming
    /// the channel and the declared value, which is the whole of issue #975's
    /// "the operator sees nothing at all".
    #[tokio::test]
    async fn a_solana_claim_declaring_a_foreign_program_is_accepted_on_its_signature() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;

        // The system program: 32 zero bytes, owning no channel anywhere, and
        // the value this repository's own contract fixture declared until
        // issue #1127 -- so it is the value a payer built against that
        // contract is most likely to still be sending.
        const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
        assert_ne!(
            decode_base58_bytes::<32>(SYSTEM_PROGRAM),
            Some(SOLANA_CHANNEL_PROGRAM_ID),
            "this test is vacuous unless the declared and actual programs really differ"
        );

        // Signed under the channel's *real* program, as ADR 0053 requires --
        // only the declared label is wrong. A claim signed under the foreign
        // program would be refused as `SignatureInvalid`, which is a
        // different fact and already has its own test.
        let keypair = solana_signer();
        let signature = keypair.sign(&connector_signer::solana_balance_proof_message(
            &SOLANA_CHANNEL_PROGRAM_ID,
            &SOLANA_CHANNEL_ACCOUNT,
            1,
            100,
        ));
        let claim = solana_claim_json_declaring(
            &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
            SYSTEM_PROGRAM,
            1,
            100,
            &BASE64.encode(signature.to_bytes()),
            &base58_encode(&keypair.public.to_bytes()),
        );

        let gate = gate();
        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    /// The conforming case, which nothing pinned before issue #1127: a claim
    /// declaring the program its channel really lives under is accepted, and
    /// [`solana_claim_json_with`] -- the fixture behind most of the Solana
    /// tests in this file -- is that claim.
    ///
    /// This is not a duplicate of `a_genuine_solana_signature_is_accepted`.
    /// That test would pass with any `programId` at all; this one fails if
    /// the fixture ever drifts back to declaring something the channel does
    /// not live under, which is what makes the fixture a statement about the
    /// wire rather than an accident.
    #[tokio::test]
    async fn the_solana_fixture_declares_the_program_its_channel_lives_under() {
        let claim = genuine_solana_claim_json(&SOLANA_CHANNEL_ACCOUNT, 1, 100);
        let declared = serde_json::from_str::<serde_json::Value>(&claim)
            .expect("the fixture is valid JSON")["programId"]
            .as_str()
            .expect("a Solana claim declares a programId")
            .to_string();
        assert_eq!(
            decode_base58_bytes::<32>(&declared),
            Some(SOLANA_CHANNEL_PROGRAM_ID),
            "a conforming claim declares the settlement program its channelAccount lives under \
             (client-edge-spec.md §1.3)"
        );

        let gate = gate();
        assert!(gate.ingest(&claim, 0).await.is_ok());
    }

    // -- Collateral binding: the cap at the on-chain deposit (issue #646) --

    mod collateral {
        use super::*;
        use crate::channels::test_source::FakeSolanaChannelSource;
        use crate::channels::{ChannelLivenessPolicy, DepositFloor, SolanaChannel};
        use std::time::Duration;

        /// A gate over a chain-resolved EVM channel whose counterparty has
        /// `deposit` on chain -- the shape every test here needs, and the
        /// one a declared `[[client_channels]]` record deliberately cannot
        /// express.
        fn chain_resolved(deposit: u64) -> (Arc<FakeChannelSource>, ClientClaimGate) {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(deposit),
                },
            )]));
            // Nothing suppresses a re-read here: what these tests are
            // about is the cap and the refresh, and the interval that
            // bounds how often one may run is measured on its own in
            // `a_replayed_undercollateralized_claim_is_not_a_free_chain_read`
            // below.
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(unsuppressed()),
            );
            (source, gate)
        }

        /// The default policy with the re-attempt interval removed -- see
        /// [`chain_resolved`].
        fn unsuppressed() -> ChannelLivenessPolicy {
            ChannelLivenessPolicy {
                min_reattempt_interval: Duration::ZERO,
                ..ChannelLivenessPolicy::default()
            }
        }

        fn resolved_channel_id() -> [u8; 32] {
            decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap()
        }

        /// The core of issue #646: a claim naming more than its channel's
        /// counterparty has actually deposited could never be redeemed
        /// (`TokenNetwork.claimFromChannel` reverts
        /// `InsufficientChannelBalance`), so serving it is doing work that
        /// can provably never be paid for.
        #[tokio::test]
        async fn a_claim_above_the_on_chain_deposit_is_refused() {
            let (_source, gate) = chain_resolved(1_000);

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_001,
                    deposited: 1_000,
                })
            );
        }

        /// The literal #633 scenario: a channel opened with a zero deposit
        /// -- a real channel, a real counterparty, a genuinely valid
        /// signature -- buys nothing.
        #[tokio::test]
        async fn a_zero_deposit_channel_refuses_its_first_claim() {
            let (_source, gate) = chain_resolved(0);

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 6_000), 100)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 6_000,
                    deposited: 0,
                })
            );
        }

        /// The boundary both contracts draw: `transferred <= deposit`, so a
        /// claim for exactly the deposit is good. This is the case a
        /// well-behaved client that has spent its whole channel ends at,
        /// and an off-by-one here would strand it.
        #[tokio::test]
        async fn a_claim_exactly_equal_to_the_deposit_is_accepted() {
            let (_source, gate) = chain_resolved(1_000);

            assert!(gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_000), 100)
                .await
                .is_ok());
        }

        /// The refusal is distinct from every other one, and says the right
        /// thing: this claim covers the price, so telling its sender they
        /// underpaid would send them to fix the wrong thing.
        #[tokio::test]
        async fn undercollateralized_is_not_underpayment_or_a_bad_signature() {
            let (_source, gate) = chain_resolved(1_000);

            let rejection = gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .unwrap_err();
            assert_ne!(
                rejection,
                ClaimIngestRejection::Underpayment {
                    advanced: 1_001,
                    price: 100
                }
            );
            assert_ne!(rejection, ClaimIngestRejection::SignatureInvalid);
            let message = rejection.message();
            assert!(message.contains("deposited on chain"), "{message}");
            assert!(message.contains("resubmit"), "{message}");
        }

        /// Nothing is consumed by the refusal -- no watermark, no journal
        /// entry -- which is what makes "deposit more and resubmit the same
        /// claim" true rather than aspirational. It is verbatim the
        /// semantics `packages/solana-program`'s own claim handler
        /// documents.
        #[tokio::test]
        async fn a_refused_claim_leaves_the_watermark_and_the_journal_untouched() {
            let (_source, gate) = chain_resolved(1_000);
            let key = format!("evm:{}", unrecorded_channel_id());

            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .unwrap_err();
            assert_eq!(gate.watermark(&key), None);

            // The same nonce is still fresh, so the client can simply pay
            // within their means at it.
            assert!(gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_000), 100)
                .await
                .is_ok());
        }

        /// The re-read-on-breach path, which is the whole reason a cached
        /// deposit is safe: the memoised floor is a *lower bound*, so a
        /// breach is a reason to look again rather than a refusal. A
        /// counterparty who tops up has the very claim that was refused
        /// honoured on resubmission -- no restart, no TTL wait.
        #[tokio::test]
        async fn a_top_up_after_a_refusal_makes_the_same_claim_good() {
            let (source, gate) = chain_resolved(1_000);
            let (_secret, address) = evm_signer();

            assert!(gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .is_err());

            source.now_says(
                resolved_channel_id(),
                Some(EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(2_000),
                }),
            );

            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .expect("the identical claim is good once the deposit covers it");
        }

        /// The cost claim, as a test: a claim inside the floor spends no
        /// chain read at all, and a breaching one spends exactly one.
        #[tokio::test]
        async fn only_a_breaching_claim_costs_a_re_read() {
            let (source, gate) = chain_resolved(1_000);

            for nonce in 1..=3 {
                gate.ingest(
                    &evm_claim_json(&unrecorded_channel_id(), nonce, nonce * 100),
                    100,
                )
                .await
                .expect("well inside the deposit");
            }
            assert_eq!(source.lookups(), 1, "one resolution, no refreshes");

            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 4, 1_001), 100)
                .await
                .unwrap_err();
            assert_eq!(source.lookups(), 2, "exactly one re-read on the breach");
        }

        /// The amplifier this cap would otherwise hand out (the
        /// availability review of #654): refusing an undercollateralized
        /// claim deliberately consumes nothing -- no nonce, no watermark --
        /// which is what makes "deposit and resubmit" true, and also means
        /// the identical claim re-passes freshness and signature every
        /// time. Without a floor on how often one channel may ask, each
        /// resubmission would be a fresh chain read that costs its sender
        /// nothing: on EVM two `eth_call`s per replay, from exactly the
        /// population this check exists to refuse.
        ///
        /// Measured before the fix at 21 lookups for 20 replays. The
        /// refusal itself must be unchanged, and the watermark must still
        /// be untouched -- the bound is on this connector's work, never on
        /// the sender's ability to correct their claim.
        #[tokio::test]
        async fn a_replayed_undercollateralized_claim_is_not_a_free_chain_read() {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                resolved_channel_id(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(ChannelLivenessPolicy {
                        refresh_after: Duration::from_secs(600),
                        serve_stale_until: Duration::from_secs(600),
                        // Long enough that all 20 replays are inside one
                        // interval however slow the machine running this
                        // is: what is measured is work per interval, not
                        // how fast a loop ran.
                        min_reattempt_interval: Duration::from_secs(600),
                    }),
            );

            for replay in 0..20 {
                assert_eq!(
                    gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                        .await,
                    Err(ClaimIngestRejection::Undercollateralized {
                        claimed: 1_001,
                        deposited: 1_000,
                    }),
                    "replay {replay} is refused for exactly what it is"
                );
            }

            assert_eq!(
                source.lookups(),
                1,
                "one resolution across 20 replays, not one each"
            );
            assert_eq!(
                gate.watermark(&format!("evm:{}", unrecorded_channel_id())),
                None,
                "and the sender's nonce is still theirs to correct"
            );
        }

        /// ...and the interval is a bound on work, not a wall: a
        /// counterparty who deposits is honoured on their next attempt past
        /// it, without a restart. One attempt after the interval rather
        /// than a burst, so this measures the re-read and not a loop's
        /// speed.
        #[tokio::test]
        async fn a_deposit_is_honoured_on_the_first_attempt_past_the_interval() {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                resolved_channel_id(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(ChannelLivenessPolicy {
                        refresh_after: Duration::from_secs(600),
                        serve_stale_until: Duration::from_secs(600),
                        min_reattempt_interval: Duration::from_millis(20),
                    }),
            );

            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .unwrap_err();
            source.now_says(
                resolved_channel_id(),
                Some(EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(2_000),
                }),
            );

            tokio::time::sleep(Duration::from_millis(60)).await;
            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                .await
                .expect("the deposit landed, so the same claim is good");
        }

        /// The deliberate exemption: an operator-declared channel names a
        /// counterparty and a domain and never a deposit, and a node with
        /// no settlement backend has no chain to ask. Hand-declaring a
        /// channel is itself the operator's decision, correctly located in
        /// config -- so it keeps today's behaviour exactly.
        #[tokio::test]
        async fn a_declared_channel_is_exempt_from_the_cap() {
            let gate = gate();

            assert!(gate
                .ingest(&evm_claim_json(&channel_id(), 1, u64::MAX), 100)
                .await
                .is_ok());
        }

        /// The Solana half -- the chain #646 was actually observed on,
        /// where the deposit is already parsed out of the channel account
        /// the counterparty comes from.
        #[tokio::test]
        async fn a_solana_claim_above_the_on_chain_deposit_is_refused() {
            let account = [0x44u8; 32];
            let source = Arc::new(FakeSolanaChannelSource::knowing(vec![(
                account,
                SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty: solana_signer().public.to_bytes(),
                    deposit_floor: DepositFloor::AtLeast(0),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_solana_source(source.clone())
                    .with_liveness_policy(unsuppressed()),
            );

            assert_eq!(
                gate.ingest(&genuine_solana_claim_json(&account, 1, 6_000), 0)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 6_000,
                    deposited: 0,
                }),
                "the #633 e2e exactly: nonce 6, 6000 base units, a vault holding nothing"
            );

            // ...and the same claim once a real deposit lands.
            source.now_says(
                account,
                Some(SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty: solana_signer().public.to_bytes(),
                    deposit_floor: DepositFloor::AtLeast(6_000),
                }),
            );
            assert!(gate
                .ingest(&genuine_solana_claim_json(&account, 1, 6_000), 0)
                .await
                .is_ok());
        }

        /// Issue #649, at the gate: a channel resolved while it was
        /// payable, settled on chain afterwards, must stop buying writes.
        /// On a cache that is never invalidated the claim below is accepted
        /// -- the settled-channel branch of the resolving backend is
        /// bypassed for the whole life of the process.
        #[tokio::test]
        async fn a_channel_that_settles_after_it_was_resolved_stops_being_accepted() {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                resolved_channel_id(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(10_000),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(ChannelLivenessPolicy::reverify_every_lookup()),
            );

            gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 100), 100)
                .await
                .expect("payable while the channel is open");

            // Settled: the resolving backend now answers "not a channel
            // this connector can be paid on" -- the same answer it gives
            // for a wrong-mint or nonexistent channel.
            source.now_says(resolved_channel_id(), None);

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 2, 200), 100)
                    .await,
                Err(ClaimIngestRejection::UnknownChannel),
                "a claim on a settled channel can never be redeemed, so it buys nothing"
            );
        }
    }

    // -- Issue #977: a reopened channel must not inherit its settled
    // predecessor's watermark -- neither charging its payer twice for
    // units already settled on chain, nor becoming permanently unusable. --
    mod reopen {
        use super::*;
        use crate::channels::test_source::FakeSolanaChannelSource;
        use crate::channels::{ChannelLivenessPolicy, DepositFloor, SolanaChannel};
        use std::time::Duration;

        /// The default liveness policy with the re-attempt interval
        /// removed, matching `collateral::unsuppressed` -- these tests
        /// drive `reap_unresolvable_channels`'s own chain re-read directly
        /// and must not have it suppressed by the interval a real sweep
        /// relies on to bound its own cost.
        fn unsuppressed() -> ChannelLivenessPolicy {
            ChannelLivenessPolicy {
                min_reattempt_interval: Duration::ZERO,
                ..ChannelLivenessPolicy::default()
            }
        }

        /// A gate over a chain-resolved (never declared) Solana channel --
        /// the shape #977 was observed on, and the one a declared
        /// `[[client_channels]]` record cannot express (it has no chain to
        /// notice a settle on at all, see [`ClientClaimGate::channel_is_gone`]'s
        /// own doc).
        fn chain_resolved_solana(
            account: [u8; 32],
            counterparty: [u8; 32],
            deposit: u64,
        ) -> (Arc<FakeSolanaChannelSource>, ClientClaimGate) {
            let source = Arc::new(FakeSolanaChannelSource::knowing(vec![(
                account,
                SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty,
                    deposit_floor: DepositFloor::AtLeast(deposit),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_solana_source(source.clone())
                    .with_liveness_policy(unsuppressed()),
            );
            (source, gate)
        }

        /// The bug end to end: a channel is spent down, settles (the
        /// resolver now answers "not a channel this connector can be paid
        /// on", exactly [`crate::channels::test_source::FakeSolanaChannelSource::now_says`]
        /// with `None` -- the same answer a genuinely deallocated Solana
        /// account produces), and reopens at the identical, deterministic
        /// address with a fresh deposit. Reopening it before this gate's
        /// sweep has ever caught the settled channel gone reproduces the
        /// issue exactly: the first claim on the reincarnation -- nonce 1,
        /// a small amount -- collides with the stale watermark the settled
        /// incarnation left behind. A sweep that runs *while the chain
        /// still reports the channel gone* -- the realistic case, since
        /// settling and reopening a channel is a slow, deliberate on-chain
        /// act -- resets it before that claim ever arrives.
        #[tokio::test]
        async fn a_sweep_lets_a_reopened_channel_be_paid_on_again() {
            let account = [0x55u8; 32];
            let counterparty = solana_signer().public.to_bytes();
            let (source, gate) = chain_resolved_solana(account, counterparty, 5_000_000);

            gate.ingest(&genuine_solana_claim_json(&account, 1, 1_000), 0)
                .await
                .expect("the original incarnation accepts its first claim");

            // Settled and deallocated: the chain no longer vouches for this
            // address at all. The sweep runs while this is still true --
            // exactly the case `reap_unresolvable_channels`'s own doc names
            // as what a sweep can actually observe, unlike the claim path.
            source.now_says(account, None);
            gate.reap_unresolvable_channels().await;

            // Only now does the channel reopen at the identical address,
            // funded again.
            source.now_says(
                account,
                Some(SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty,
                    deposit_floor: DepositFloor::AtLeast(5_000_000),
                }),
            );

            assert!(
                gate.ingest(&genuine_solana_claim_json(&account, 1, 100), 0)
                    .await
                    .is_ok(),
                "the sweep already reset the stale watermark before the reopen, so the \
                 reincarnation's own first claim is judged as fresh, not as a replay of its \
                 predecessor's"
            );
        }

        /// The failure mode the issue calls unbounded: a channel spent to
        /// exactly its deposit, then reopened with the same deposit, has no
        /// nonce that can ever satisfy both "greater than the watermark"
        /// and "within collateral" -- permanently unusable without a reset.
        #[tokio::test]
        async fn a_channel_spent_to_its_deposit_and_reopened_is_not_permanently_unusable() {
            let account = [0x56u8; 32];
            let counterparty = solana_signer().public.to_bytes();
            let (source, gate) = chain_resolved_solana(account, counterparty, 5_000_000);

            gate.ingest(&genuine_solana_claim_json(&account, 9, 5_000_000), 0)
                .await
                .expect("spent to exactly the deposit");

            // No nonce could ever satisfy both "> the old watermark's
            // 5,000,000" and "<= the reopened channel's own 5,000,000
            // deposit" -- every claim the reincarnation's payer could ever
            // sign would be refused, one way or the other, without a sweep
            // to reset the watermark while the settle is still observable.
            source.now_says(account, None);
            gate.reap_unresolvable_channels().await;

            source.now_says(
                account,
                Some(SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty,
                    deposit_floor: DepositFloor::AtLeast(5_000_000),
                }),
            );

            assert!(
                gate.ingest(&genuine_solana_claim_json(&account, 1, 5_000_000), 0)
                    .await
                    .is_ok(),
                "the reincarnation can spend its own full deposit once the sweep has reset \
                 the predecessor's watermark"
            );
        }

        /// The sweep must not perturb a channel that is still genuinely
        /// live: its watermark is untouched, and a real replay on it is
        /// still refused after the sweep runs.
        #[tokio::test]
        async fn a_sweep_leaves_a_still_live_channels_watermark_alone() {
            let account = [0x57u8; 32];
            let counterparty = solana_signer().public.to_bytes();
            let (_source, gate) = chain_resolved_solana(account, counterparty, 5_000_000);

            gate.ingest(&genuine_solana_claim_json(&account, 3, 3_000), 0)
                .await
                .expect("a live channel accepts its claim");

            gate.reap_unresolvable_channels().await;

            assert_eq!(
                gate.ingest(&genuine_solana_claim_json(&account, 3, 3_000), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing),
                "sweeping a channel the chain still vouches for must not reset it"
            );
        }

        /// A declared channel ([`ClientChannelRegistry::record_solana`]) has
        /// no chain source to notice a settle on at all -- `refresh_solana`
        /// answers it from config, unconditionally -- so the sweep is
        /// always a no-op for one, by construction rather than by the fake
        /// source's own behaviour.
        #[tokio::test]
        async fn a_sweep_never_resets_a_declared_channels_watermark() {
            let gate = gate();
            let channel = channel_id();

            gate.ingest(&evm_claim_json(&channel, 5, 500), 0)
                .await
                .expect("a declared channel accepts its claim");

            gate.reap_unresolvable_channels().await;

            assert_eq!(
                gate.ingest(&evm_claim_json(&channel, 5, 999), 0).await,
                Err(ClaimIngestRejection::NonceNotAdvancing),
                "a declared channel's watermark is config-scoped and must survive a sweep"
            );
        }

        /// [`chain_resolved_solana`]'s EVM twin, over
        /// [`unrecorded_channel_id`] -- a channel resolved from the chain
        /// rather than declared.
        fn chain_resolved_evm(deposit: u64) -> (Arc<FakeChannelSource>, ClientClaimGate) {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                resolved_evm_channel_id(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(deposit),
                },
            )]));
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(unsuppressed()),
            );
            (source, gate)
        }

        fn resolved_evm_channel_id() -> [u8; 32] {
            decode_hex_bytes::<32>(&unrecorded_channel_id()).expect("a valid test channel id")
        }

        /// The issue's own "Note on scope": it was observed on Solana, but
        /// an EVM `channelId` is derived rather than random too, so the
        /// same reopen collides there and the sweep has to cover both
        /// chains rather than the one it was reported on.
        #[tokio::test]
        async fn a_sweep_lets_a_reopened_evm_channel_be_paid_on_again() {
            let channel = unrecorded_channel_id();
            let (source, gate) = chain_resolved_evm(5_000_000);
            let (_secret, counterparty) = evm_signer();

            gate.ingest(&evm_claim_json(&channel, 9, 5_000_000), 0)
                .await
                .expect("the original incarnation spends its whole deposit");

            source.now_says(resolved_evm_channel_id(), None);
            gate.reap_unresolvable_channels().await;

            source.now_says(
                resolved_evm_channel_id(),
                Some(EvmChannel {
                    counterparty,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(5_000_000),
                }),
            );

            assert!(
                gate.ingest(&evm_claim_json(&channel, 1, 100), 0)
                    .await
                    .is_ok(),
                "an EVM channel reopened at its own derived id is judged from a clean \
                 watermark, exactly as the Solana one is"
            );
        }

        /// The reset survives a restart (issue #605's own durability
        /// discipline, extended to this new entry kind): a second gate
        /// replaying the same journal file recovers the reset, not the
        /// stale predecessor watermark it erased.
        #[tokio::test]
        async fn the_reset_a_sweep_makes_survives_a_restart() {
            use connector_runtime::FileJournal;

            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let account = [0x58u8; 32];
            let counterparty = solana_signer().public.to_bytes();
            let source = Arc::new(FakeSolanaChannelSource::knowing(vec![(
                account,
                SolanaChannel {
                    program_id: [7u8; 32],
                    counterparty,
                    deposit_floor: DepositFloor::AtLeast(5_000_000),
                },
            )]));
            let registry = || {
                ClientChannelRegistry::new()
                    .with_solana_source(source.clone())
                    .with_liveness_policy(unsuppressed())
            };

            {
                let gate = ClientClaimGate::restore(
                    registry(),
                    Arc::new(FileJournal::open(&path).expect("open the journal file")),
                )
                .expect("replay the journal");
                gate.ingest(&genuine_solana_claim_json(&account, 1, 1_000), 0)
                    .await
                    .expect("the original incarnation accepts its first claim");
                source.now_says(account, None);
                gate.reap_unresolvable_channels().await;
                source.now_says(
                    account,
                    Some(SolanaChannel {
                        program_id: [7u8; 32],
                        counterparty,
                        deposit_floor: DepositFloor::AtLeast(5_000_000),
                    }),
                );
            }

            // A second gate over the same journal file: a restarted
            // process, reading the same durable state off the same disk.
            let restarted = ClientClaimGate::restore(
                registry(),
                Arc::new(FileJournal::open(&path).expect("open the journal file")),
            )
            .expect("replay the journal");

            assert!(
                restarted
                    .ingest(&genuine_solana_claim_json(&account, 1, 100), 0)
                    .await
                    .is_ok(),
                "the reset the sweep made before the restart must still hold after it -- a \
                 restart must not resurrect the settled predecessor's watermark"
            );
        }
    }

    // -- Netting: spendable headroom nets a channel's outbound payout
    // ledger too (issue #700, `toon-meta#262` decision 9) --
    mod netting {
        use super::*;
        use crate::channels::test_source::FakeChannelSource;
        use crate::channels::{
            ChannelLivenessPolicy, ChannelLookupFailed, ClientChannelSource, DepositFloor,
        };
        use chrono::{DateTime, Utc};
        use connector_runtime::ChannelDomain;
        use connector_signer::LocalSigner;
        use proptest::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        use tokio::sync::Notify;

        fn now() -> DateTime<Utc> {
            "2030-01-01T00:00:00Z".parse().unwrap()
        }

        fn payout_domain() -> ChannelDomain {
            ChannelDomain {
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            }
        }

        /// A ledger with `channel_id` registered and credited `amount` --
        /// signed by its own dedicated key, since this connector's outbound
        /// signer is never a channel's counterparty. `amount` of `0`
        /// registers the channel (so [`ClientClaimGate::credited_evm`] can
        /// find it) without recording a payout.
        fn ledger_crediting(channel_id: &str, amount: u64) -> Arc<ClientPayoutLedger> {
            let mut ledger = ClientPayoutLedger::new();
            ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
            ledger
                .set_channel_domain(channel_id, payout_domain())
                .expect("test channel id is valid");
            let ledger = Arc::new(ledger);
            if amount > 0 {
                ledger
                    .record_payout(channel_id, amount, now())
                    .expect("signer and domain configured");
            }
            ledger
        }

        /// The default liveness policy with the re-attempt interval
        /// removed, matching `collateral::unsuppressed` -- these tests are
        /// about the ceiling and the refresh, not the rate limiter.
        fn unsuppressed() -> ChannelLivenessPolicy {
            ChannelLivenessPolicy {
                min_reattempt_interval: Duration::ZERO,
                ..ChannelLivenessPolicy::default()
            }
        }

        /// A gate over a chain-resolved EVM channel whose counterparty has
        /// `deposit` on chain, with a payout ledger crediting the same
        /// channel `credited` -- the shape every simple test in this module
        /// needs.
        fn chain_resolved_with_credit(
            deposit: u64,
            credited: u64,
        ) -> (
            Arc<FakeChannelSource>,
            Arc<ClientPayoutLedger>,
            ClientClaimGate,
        ) {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(deposit),
                },
            )]));
            let ledger = ledger_crediting(&unrecorded_channel_id(), credited);
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source.clone())
                    .with_liveness_policy(unsuppressed()),
            )
            .with_payout_ledger(Arc::clone(&ledger));
            (source, ledger, gate)
        }

        /// A claim that would be refused against the raw deposit alone
        /// (issue #646) is accepted once the channel's counterparty has
        /// been credited enough to cover the difference -- decision 9 of
        /// `toon-meta#262`: an inbound claim raises spendable headroom
        /// directly.
        #[tokio::test]
        async fn a_claim_above_the_raw_deposit_is_accepted_once_credited_covers_the_rest() {
            let (_source, _ledger, gate) = chain_resolved_with_credit(1_000, 500);

            assert!(gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_500), 100)
                .await
                .is_ok());
        }

        /// The boundary this ceiling draws: one unit past `deposit +
        /// credited` is still refused -- the same off-by-one discipline
        /// `collateral::a_claim_exactly_equal_to_the_deposit_is_accepted`
        /// holds the raw deposit alone to, checked from the other side.
        #[tokio::test]
        async fn one_unit_past_deposit_plus_credited_is_still_refused() {
            let (_source, _ledger, gate) = chain_resolved_with_credit(1_000, 500);

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_501), 100)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_501,
                    deposited: 1_000,
                })
            );
        }

        /// A gate with no payout ledger configured at all behaves exactly
        /// as it did before issue #700 -- the default every constructor
        /// leaves `payout_ledger` at.
        #[tokio::test]
        async fn no_payout_ledger_configured_nets_nothing() {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]));
            // No `.with_payout_ledger(..)` call -- the pre-#700 default.
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source)
                    .with_liveness_policy(unsuppressed()),
            );

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_001,
                    deposited: 1_000,
                })
            );
        }

        /// A payout credited on a *different* channel does not leak
        /// headroom across channels -- issue #700's explicit "do not net
        /// across chains", applied at the channel granularity that rule's
        /// own reasoning already implies.
        #[tokio::test]
        async fn credit_on_a_different_channel_does_not_raise_this_ones_headroom() {
            let (_secret, address) = evm_signer();
            let source = Arc::new(FakeChannelSource::knowing(vec![(
                decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap(),
                EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
            )]));
            let mut ledger = ClientPayoutLedger::new();
            ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
            ledger
                .set_channel_domain(unrecorded_channel_id(), payout_domain())
                .expect("test channel id is valid");
            ledger
                .set_channel_domain(second_channel_id(), payout_domain())
                .expect("test channel id is valid");
            let ledger = Arc::new(ledger);
            ledger
                .record_payout(&second_channel_id(), 10_000, now())
                .expect("a channel this ledger's own domain covers");
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(source)
                    .with_liveness_policy(unsuppressed()),
            )
            .with_payout_ledger(Arc::clone(&ledger));

            assert_eq!(
                gate.ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_001), 100)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_001,
                    deposited: 1_000,
                })
            );
        }

        // -- Interleaved inbound/outbound advances (issue #700's own
        // explicit ask: "at minimum: interleaved inbound/outbound
        // advances") --

        proptest! {
            /// However inbound claims and outbound payouts interleave, an
            /// inbound claim is admitted iff its cumulative amount is at
            /// most `deposit + credited` at the moment it is judged, and
            /// the gate's own watermark and the ledger's own credited
            /// total always agree with what this test tracks by hand.
            #[test]
            fn netting_never_admits_beyond_deposit_plus_credited_however_interleaved(
                ops in proptest::collection::vec((proptest::bool::ANY, 1u64..50_000u64), 1..15)
            ) {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                runtime.block_on(async move {
                    const DEPOSIT: u64 = 500_000;
                    let (_secret, address) = evm_signer();
                    let mut channels = ClientChannelRegistry::new();
                    channels
                        .record_evm(
                            &channel_id(),
                            EvmChannel {
                                counterparty: address,
                                chain_id: EVM_CHAIN_ID,
                                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                                deposit_floor: DepositFloor::AtLeast(DEPOSIT),
                            },
                        )
                        .expect("valid channel id");
                    let ledger = ledger_crediting(&channel_id(), 0);
                    let gate = gate_over(channels).with_payout_ledger(Arc::clone(&ledger));

                    let mut owed: u64 = 0;
                    let mut credited: u64 = 0;
                    let mut nonce: u64 = 0;

                    for (is_payout, amount) in ops {
                        if is_payout {
                            ledger
                                .record_payout(&channel_id(), amount, now())
                                .expect("signer and domain configured");
                            credited += amount;
                        } else {
                            nonce += 1;
                            let new_cumulative = owed + amount;
                            let result = gate
                                .ingest(&evm_claim_json(&channel_id(), nonce, new_cumulative), 0)
                                .await;
                            if new_cumulative <= DEPOSIT + credited {
                                prop_assert!(
                                    result.is_ok(),
                                    "{new_cumulative} <= {DEPOSIT} + {credited} must admit: {result:?}"
                                );
                                owed = new_cumulative;
                            } else {
                                prop_assert!(
                                    matches!(
                                        result,
                                        Err(ClaimIngestRejection::Undercollateralized { .. })
                                    ),
                                    "{new_cumulative} > {DEPOSIT} + {credited} must refuse: {result:?}"
                                );
                            }
                        }
                    }

                    let watermark = gate.watermark(&format!("evm:{}", channel_id()));
                    prop_assert_eq!(
                        watermark.map(|w| w.cumulative_amount).unwrap_or(0),
                        owed
                    );
                    prop_assert_eq!(ledger.credited(&channel_id()), credited);
                    Ok(())
                })?;
            }
        }

        // -- A credit arriving mid-flight during an in-flight admission --

        /// A [`ClientChannelSource`] whose *second* lookup on `channel_id`
        /// -- the collateral-breach refresh, never the first resolution --
        /// rendezvouses with the test: it signals `entered_refresh` the
        /// instant it is called, then waits on `release_refresh` before
        /// answering. This is what lets a test inject a payout at the
        /// exact point between `ClientClaimGate::credited`'s snapshot and
        /// the refresh's own answer, deterministically, without racing
        /// real wall-clock timing.
        #[derive(Debug)]
        struct RendezvousSource {
            channel_id: [u8; 32],
            channel: EvmChannel,
            calls: AtomicUsize,
            entered_refresh: Arc<Notify>,
            release_refresh: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl ClientChannelSource for RendezvousSource {
            async fn evm_channel(
                &self,
                channel_id: &[u8; 32],
            ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
                if *channel_id != self.channel_id {
                    return Ok(None);
                }
                if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    self.entered_refresh.notify_one();
                    self.release_refresh.notified().await;
                }
                Ok(Some(self.channel))
            }
        }

        /// An inbound admission that has already taken its collateral
        /// snapshot (issue #700) must not retroactively benefit from a
        /// payout recorded while it is still awaiting the chain's answer
        /// on refresh: `credited` is read once, before `check_collateral`'s
        /// own await, exactly like the deposit it is added to. This is the
        /// safe direction on purpose -- a race can only produce a false
        /// refusal (which self-heals on resubmission, proven below) and
        /// never lets a single payout be "spent" twice by two admissions
        /// that individually raced past its snapshot.
        #[tokio::test]
        async fn a_payout_recorded_mid_admission_does_not_retroactively_cover_it() {
            let (_secret, address) = evm_signer();
            let entered_refresh = Arc::new(Notify::new());
            let release_refresh = Arc::new(Notify::new());
            let source = Arc::new(RendezvousSource {
                channel_id: decode_hex_bytes::<32>(&unrecorded_channel_id()).unwrap(),
                channel: EvmChannel {
                    counterparty: address,
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                    deposit_floor: DepositFloor::AtLeast(1_000),
                },
                calls: AtomicUsize::new(0),
                entered_refresh: Arc::clone(&entered_refresh),
                release_refresh: Arc::clone(&release_refresh),
            });
            let ledger = ledger_crediting(&unrecorded_channel_id(), 0);
            let gate = Arc::new(
                gate_over(
                    ClientChannelRegistry::new()
                        .with_source(source)
                        .with_liveness_policy(unsuppressed()),
                )
                .with_payout_ledger(Arc::clone(&ledger)),
            );

            // Breaches the raw deposit (1_000 + 0 credited < 1_400), so
            // `check_collateral` re-reads the chain -- the second lookup
            // `RendezvousSource` gates.
            let admitting = Arc::clone(&gate);
            let admission = tokio::spawn(async move {
                admitting
                    .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_400), 100)
                    .await
            });

            // Wait until the admission is inside the refresh read -- its
            // `credited` snapshot (0) is already taken by construction,
            // since that read happens synchronously before this refresh is
            // ever reached.
            entered_refresh.notified().await;
            ledger
                .record_payout(&unrecorded_channel_id(), 1_000, now())
                .expect("signer and domain configured");
            release_refresh.notify_one();

            assert_eq!(
                admission.await.unwrap(),
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_400,
                    deposited: 1_000,
                }),
                "a payout recorded after this admission's credited snapshot must not rescue it"
            );

            // The self-heal: the identical claim, resubmitted now that the
            // payout is visible from the start, succeeds -- nothing about
            // the race left the gate in a state where it can never be
            // paid.
            assert!(gate
                .ingest(&evm_claim_json(&unrecorded_channel_id(), 1, 1_400), 100)
                .await
                .is_ok());
        }

        // -- Reconnect mid-flight: a session reconnect must not lose
        // either watermark --

        fn reconnect_test_channels(address: Address) -> ClientChannelRegistry {
            let mut channels = ClientChannelRegistry::new();
            channels
                .record_evm(
                    &channel_id(),
                    EvmChannel {
                        counterparty: address,
                        chain_id: EVM_CHAIN_ID,
                        token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                        deposit_floor: DepositFloor::AtLeast(1_000),
                    },
                )
                .expect("valid channel id");
            channels
        }

        /// A BTP session reconnect rebuilds only the [`ClientClaimGate`]
        /// (`btp.rs`'s own doc: "the same `ClientClaimGate` instance, the
        /// same watermarks and journal" -- here, a fresh gate over the
        /// *same* journal, standing in for a reconnect within a process
        /// that never restarted) while [`ClientPayoutLedger`] -- owned by
        /// the longer-lived `ClientEdgeState`, not the session -- is
        /// simply reattached via [`ClientClaimGate::with_payout_ledger`].
        /// Both the client's already-accepted spend (owed) and this
        /// connector's already-signed payout (credited) must still net
        /// exactly as they did before the reconnect.
        #[tokio::test]
        async fn a_reconnect_mid_sequence_preserves_both_owed_and_credited() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let channel = channel_id();
            let (_secret, address) = evm_signer();
            let ledger = ledger_crediting(&channel, 0);

            {
                let gate = ClientClaimGate::restore(
                    reconnect_test_channels(address),
                    Arc::new(FileJournal::open(&path).expect("open the journal file")),
                )
                .expect("replay the journal")
                .with_payout_ledger(Arc::clone(&ledger));

                // Owed climbs to 800 against a 1_000 deposit and no credit
                // yet.
                gate.ingest(&evm_claim_json(&channel, 1, 800), 0)
                    .await
                    .expect("within the raw deposit");
                // Mid-sequence, this connector credits the client 500 for
                // earned work -- headroom is now 1_000 + 500 - 800 = 700.
                ledger
                    .record_payout(&channel, 500, now())
                    .expect("signer and domain configured");
            }

            // Reconnect: a fresh gate over the same journal, the same
            // ledger reattached -- the session dropped, the process did
            // not.
            let reconnected = ClientClaimGate::restore(
                reconnect_test_channels(address),
                Arc::new(FileJournal::open(&path).expect("open the journal file")),
            )
            .expect("replay the journal")
            .with_payout_ledger(Arc::clone(&ledger));

            // The client's own already-spent nonce is still spent.
            assert_eq!(
                reconnected
                    .ingest(&evm_claim_json(&channel, 1, 800), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing),
            );

            // Exactly the netted headroom survives the reconnect: 800 +
            // 700 = 1_500 is good, one unit past it is not.
            assert!(reconnected
                .ingest(&evm_claim_json(&channel, 2, 1_500), 0)
                .await
                .is_ok());
            assert_eq!(
                reconnected
                    .ingest(&evm_claim_json(&channel, 3, 1_501), 0)
                    .await,
                Err(ClaimIngestRejection::Undercollateralized {
                    claimed: 1_501,
                    deposited: 1_000,
                })
            );
        }
    }

    // -- Issue #787: a session bound under its ILP address resolves to
    // the channel id an earlier inbound claim taught this gate --
    mod session_channel_association {
        use super::*;
        use connector_runtime::ChannelDomain;
        use connector_signer::LocalSigner;

        fn payout_domain() -> ChannelDomain {
            ChannelDomain {
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            }
        }

        /// The bug this issue fixes, reproduced directly at the gate: a
        /// session is bound under its ILP address, never under a channel
        /// id (issue #736/toon-client#503), so `credit_session_payout`
        /// must resolve the one from the other rather than being handed a
        /// channel id already. Before the fix there was no resolution
        /// step at all -- `credit_payout` was called with the address
        /// itself, which never decodes as a channel id, so nothing was
        /// ever credited on any real deployment.
        #[tokio::test]
        async fn a_session_taught_its_channel_is_credited_by_its_address() {
            let address = "g.toon.provider";
            let channel_id = format!("0x{:064x}", 1);

            let mut ledger = ClientPayoutLedger::new();
            ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
            ledger
                .set_channel_domain(channel_id.clone(), payout_domain())
                .expect("valid channel id");
            let ledger = Arc::new(ledger);

            let gate = gate().with_payout_ledger(Arc::clone(&ledger));
            gate.record_session_channel(address, channel_id.clone());

            let job_id = [3u8; 32];
            let claim = gate
                .credit_session_payout(address, &job_id, 5_000, now())
                .await
                .expect("the session's channel was known, and is payable");
            assert_eq!(claim.channel_id, channel_id);
            assert_eq!(ledger.credited(&channel_id), 5_000);
        }

        /// The issue's own AC3: a destination this gate has never learned a
        /// channel for -- an earning agent that has never itself presented
        /// a claim on this session -- must credit nothing, decided
        /// explicitly rather than by some other path silently finding
        /// nothing.
        #[tokio::test]
        async fn a_destination_with_no_known_channel_credits_nothing() {
            let ledger = Arc::new(ClientPayoutLedger::new());
            let gate = gate().with_payout_ledger(ledger);

            let job_id = [4u8; 32];
            let claim = gate
                .credit_session_payout("g.toon.unpaid", &job_id, 5_000, now())
                .await;
            assert!(
                claim.is_none(),
                "no association was ever taught for this address"
            );
        }

        fn now() -> DateTime<Utc> {
            "2030-01-01T00:00:00Z".parse().unwrap()
        }
    }

    // -- Watermark durability across a restart (issue #605) --

    /// Issue #643: a watermark is filed under the channel, not under the
    /// text a claim spelled the channel with. Every stage of this gate
    /// except the watermark already agreed that `0xAB..` and `0xab..` are
    /// one channel -- the registry resolves the counterparty from the
    /// decoded bytes, and the EIP-712 digest is computed over those same
    /// bytes, so a recased claim carries a signature that still verifies.
    /// The watermark disagreeing was worth one free write per casing.
    mod canonical_channel_ids {
        use super::*;

        /// The same `channel_id()` an accepted claim named, retyped in
        /// upper case. Byte-for-byte a different string; the same 32-byte
        /// channel.
        fn upper_cased_channel_id() -> String {
            format!("0x{}", "AB".repeat(32))
        }

        fn mixed_case_channel_id() -> String {
            format!("0x{}", "aB".repeat(32))
        }

        /// The defect, exactly as issue #643 states it: one signed claim,
        /// presented twice, differing only in the casing of its
        /// `channelId`. On a tree without the fix the second presentation
        /// lands on a fresh, empty watermark and is **accepted** -- the
        /// client is served twice for one claim.
        #[tokio::test]
        async fn the_same_claim_re_presented_in_a_different_hex_casing_is_refused_as_stale() {
            let gate = gate();
            gate.ingest(&evm_claim_json(&channel_id(), 5, 500), 0)
                .await
                .expect("the first presentation is a genuine fresh claim");

            assert_eq!(
                gate.ingest(&evm_claim_json(&upper_cased_channel_id(), 5, 500), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
            assert_eq!(
                gate.ingest(&evm_claim_json(&mixed_case_channel_id(), 5, 500), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
        }

        /// N presentations do not buy N writes: after one claim at nonce
        /// 5, *nothing* at or below 5 is accepted in any spelling, which
        /// is the property "a fresh watermark per casing" destroyed.
        #[tokio::test]
        async fn a_recased_channel_id_does_not_open_a_second_watermark() {
            let gate = gate();
            gate.ingest(&evm_claim_json(&channel_id(), 5, 500), 0)
                .await
                .expect("first claim accepted");

            for nonce in 1..=5 {
                assert_eq!(
                    gate.ingest(&evm_claim_json(&upper_cased_channel_id(), nonce, 500), 0)
                        .await,
                    Err(ClaimIngestRejection::NonceNotAdvancing),
                    "nonce {nonce} in a different casing must not be a fresh channel"
                );
            }
        }

        /// The fix normalises a key; it does not refuse a spelling. A
        /// client that has always sent upper-case hex is still a paying
        /// client -- its claim resolves the same channel record and
        /// verifies under the same digest -- and simply advances the same
        /// watermark a lower-case one would.
        #[tokio::test]
        async fn an_upper_cased_channel_id_is_still_a_perfectly_good_claim() {
            let gate = gate();
            gate.ingest(&evm_claim_json(&upper_cased_channel_id(), 1, 100), 100)
                .await
                .expect("casing is not a validity question");

            // ... and it advanced the one watermark, so the lower-case
            // spelling of the same channel is now judged against it.
            assert_eq!(
                gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
        }

        /// The read side agrees with the write side: a watermark reached
        /// through one spelling is visible through the other, so nothing
        /// downstream can conclude a paid channel has never been paid on.
        #[tokio::test]
        async fn a_watermark_reads_back_the_same_whichever_spelling_asks_for_it() {
            let gate = gate();
            gate.ingest(&evm_claim_json(&channel_id(), 7, 700), 0)
                .await
                .expect("accepted");

            let expected = Some(Watermark {
                nonce: 7,
                cumulative_amount: 700,
            });
            assert_eq!(gate.watermark(&format!("evm:{}", channel_id())), expected);
            assert_eq!(
                gate.watermark(&format!("evm:{}", upper_cased_channel_id())),
                expected
            );
            assert_eq!(
                gate.watermark(&format!("evm:{}", "ab".repeat(32))),
                expected,
                "the bare-hex spelling names the same channel too"
            );
        }

        /// Recasing does not merge two channels either: a genuinely
        /// different channel still gets its own watermark, so the fix
        /// closes a replay hole without inventing a shared one.
        #[tokio::test]
        async fn two_genuinely_different_channels_still_have_independent_watermarks() {
            let gate = gate();
            gate.ingest(&evm_claim_json(&upper_cased_channel_id(), 9, 900), 0)
                .await
                .expect("first channel");

            assert!(gate
                .ingest(&evm_claim_json(&second_channel_id(), 1, 10), 0)
                .await
                .is_ok());
        }

        /// Solana is untouched, and must stay untouched: base58 is
        /// case-*sensitive*, so lower-casing a `channelAccount` would name
        /// a different account. This is here to fail loudly if the EVM
        /// rule is ever generalised across namespaces.
        #[tokio::test]
        async fn a_solana_claim_is_unaffected_by_the_evm_canonicalisation() {
            let gate = gate();
            gate.ingest(
                &genuine_solana_claim_json(&SOLANA_CHANNEL_ACCOUNT, 4, 400),
                0,
            )
            .await
            .expect("accepted");

            assert_eq!(
                gate.watermark(&format!(
                    "solana:{}",
                    base58_encode(&SOLANA_CHANNEL_ACCOUNT)
                )),
                Some(Watermark {
                    nonce: 4,
                    cumulative_amount: 400
                })
            );
        }
    }

    mod durability {
        use super::*;

        /// A [`Journal`] whose `append` always fails -- ADR 0007's fake,
        /// not a mock: a real journal on a full or read-only disk behaves
        /// exactly like this, and the gate must refuse claims rather than
        /// accept ones it cannot remember.
        struct UnwritableJournal;

        impl Journal for UnwritableJournal {
            fn append(&self, _entry: &JournalEntry) -> Result<(), JournalError> {
                Err(JournalError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "read-only journal",
                )))
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                Ok(Vec::new())
            }
        }

        /// A [`Journal`] whose `read_all` fails, standing in for a journal
        /// file that exists but cannot be read back.
        struct UnreadableJournal;

        impl Journal for UnreadableJournal {
            fn append(&self, _entry: &JournalEntry) -> Result<(), JournalError> {
                Ok(())
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                Err(JournalError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unreadable journal",
                )))
            }
        }

        fn file_gate(path: &std::path::Path) -> ClientClaimGate {
            ClientClaimGate::restore(
                test_channels(),
                Arc::new(FileJournal::open(path).expect("open the journal file")),
            )
            .expect("replay the journal")
        }

        /// Issue #605's own failure, end to end: a client spends a claim,
        /// the process restarts, and the client re-presents the very same
        /// claim. Before this fix the restarted gate held no watermark for
        /// the channel and accepted it -- and every claim above it -- as
        /// fresh, so 50 already-spent writes became 50 free ones.
        #[tokio::test]
        async fn a_claim_accepted_before_a_restart_is_refused_after_one() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let channel = channel_id();

            {
                let gate = file_gate(&path);
                gate.ingest(&evm_claim_json(&channel, 50, 50_000), 1000)
                    .await
                    .expect("the first process accepts the claim");
            }

            // A second gate over the same journal file: a restarted
            // process, reading the same durable state off the same disk.
            let restarted = file_gate(&path);

            assert_eq!(
                restarted
                    .ingest(&evm_claim_json(&channel, 50, 50_000), 1000)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing),
                "a claim already spent before the restart must not be spendable after it"
            );
        }

        /// The rest of the replay-attack surface the ticket names: not
        /// just the last claim, but every lower nonce beneath it.
        #[tokio::test]
        async fn no_nonce_at_or_below_the_pre_restart_watermark_is_spendable_after_it() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let channel = channel_id();

            {
                let gate = file_gate(&path);
                for nonce in 1..=5 {
                    gate.ingest(&evm_claim_json(&channel, nonce, nonce * 1000), 1000)
                        .await
                        .expect("a run of claims, each advancing by the price");
                }
            }

            let restarted = file_gate(&path);
            for nonce in 1..=5 {
                assert_eq!(
                    restarted
                        .ingest(&evm_claim_json(&channel, nonce, nonce * 1000), 1000)
                        .await,
                    Err(ClaimIngestRejection::NonceNotAdvancing),
                    "nonce {nonce} was spent before the restart"
                );
            }

            // The next genuinely fresh claim still works: recovery
            // restores the watermark, it does not wedge the channel.
            assert!(restarted
                .ingest(&evm_claim_json(&channel, 6, 6000), 1000)
                .await
                .is_ok());
        }

        /// Recovery is per channel, not a single global high-water mark:
        /// a channel that was never claimed on before the restart still
        /// accepts its first claim afterwards.
        #[tokio::test]
        async fn a_restart_recovers_each_channels_watermark_independently() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");

            {
                let gate = file_gate(&path);
                gate.ingest(&evm_claim_json(&channel_id(), 9, 9000), 0)
                    .await
                    .expect("first channel claimed on");
            }

            let restarted = file_gate(&path);
            assert_eq!(
                restarted
                    .ingest(&evm_claim_json(&channel_id(), 9, 9000), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
            assert!(restarted
                .ingest(&evm_claim_json(&second_channel_id(), 1, 10), 0)
                .await
                .is_ok());
        }

        /// Solana claims recover the same way -- the journal is keyed by
        /// the same chain-namespaced `channel_key` the live watermark map
        /// is, so neither chain's recovery can answer for the other's.
        #[tokio::test]
        async fn a_solana_claim_accepted_before_a_restart_is_refused_after_one() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");

            {
                let gate = file_gate(&path);
                gate.ingest(
                    &genuine_solana_claim_json(&SOLANA_CHANNEL_ACCOUNT, 4, 400),
                    0,
                )
                .await
                .expect("the first process accepts the claim");
            }

            let restarted = file_gate(&path);
            assert_eq!(
                restarted
                    .ingest(
                        &genuine_solana_claim_json(&SOLANA_CHANNEL_ACCOUNT, 4, 400),
                        0
                    )
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
        }

        /// A refused claim leaves nothing durable behind either: the
        /// journal is a record of what was *accepted*, so a corrected
        /// resubmission after a restart is still judged against the same
        /// baseline the live process judged it against.
        #[tokio::test]
        async fn a_refused_claim_writes_nothing_a_restart_could_recover() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let channel = channel_id();

            {
                let gate = file_gate(&path);
                gate.ingest(&evm_claim_json(&channel, 1, 99), 100)
                    .await
                    .unwrap_err(); // underpayment
            }

            let restarted = file_gate(&path);
            assert_eq!(restarted.watermark(&format!("evm:{channel}")), None);
            assert!(restarted
                .ingest(&evm_claim_json(&channel, 1, 100), 100)
                .await
                .is_ok());
        }

        /// Issue #643's *upgrade* case, and the one that decides whether
        /// that fix is safe to ship to the live fleet: a devnet node's
        /// journal already holds watermarks written by a pre-#643 build,
        /// under whatever spelling the paying client happened to use. If
        /// changing the key format orphaned those, every live node's
        /// watermarks would silently come back `None` on the first
        /// restart, and `validate_claim(None, ..)` accepts every nonce the
        /// client already spent -- reopening, fleet-wide and all at once,
        /// exactly the hole #643 closes.
        ///
        /// It does not, because [`replay_watermarks`] canonicalises as it
        /// folds: the legacy line is read exactly as written and lands
        /// under the key this build files by.
        #[tokio::test]
        async fn a_legacy_spelled_watermark_still_refuses_a_replay_after_the_upgrade() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let legacy_key = format!("evm:0x{}", "AB".repeat(32));

            // Exactly what a pre-#643 build wrote: the claim's own literal
            // `channelId`, namespaced. Written through `FileJournal` so
            // this is the real on-disk encoding, not a hand-rolled line.
            {
                let journal = FileJournal::open(&path).expect("open the journal file");
                journal
                    .append(&JournalEntry::InboundClaimAccepted {
                        channel_id: legacy_key.clone(),
                        nonce: 12,
                        cumulative_amount: 1200,
                        signature: vec![0xde, 0xad, 0xbe, 0xef],
                    })
                    .expect("append a legacy entry");
            }

            let upgraded = file_gate(&path);

            // The watermark survived the upgrade, under the canonical key.
            assert_eq!(
                upgraded.watermark(&format!("evm:{}", channel_id())),
                Some(Watermark {
                    nonce: 12,
                    cumulative_amount: 1200
                }),
                "a pre-#643 watermark must not be orphaned by the upgrade"
            );

            // And it is still enforced: the client's already-spent claim
            // is refused in the spelling it was spent in *and* in the one
            // it was recorded in.
            assert_eq!(
                upgraded
                    .ingest(&evm_claim_json(&channel_id(), 12, 1200), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
            assert_eq!(
                upgraded
                    .ingest(
                        &evm_claim_json(&format!("0x{}", "AB".repeat(32)), 12, 1200),
                        0
                    )
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );

            // A genuinely advancing claim still pays, so the upgrade cost
            // the honest client nothing.
            assert!(upgraded
                .ingest(&evm_claim_json(&channel_id(), 13, 1300), 0)
                .await
                .is_ok());
        }

        /// The **rollback** direction, which the upgrade test above says
        /// nothing about: a box running this build writes a journal, and
        /// its image tag is then rolled back to a pre-#643 binary --
        /// routine, since the deploy model is baked image tags. That older
        /// binary folds the journal *without* canonicalising and derives
        /// its lookup key as `format!("evm:{}", claim.channel_id)`. If the
        /// two formats did not coincide it would find `None`, and
        /// `validate_claim(None, ..)` would re-accept the client's entire
        /// spend history -- strictly worse than the bug #643 fixes.
        ///
        /// They coincide because the canonical key keeps the `0x`: for the
        /// lowercase hex `parse_evm` has always required, the key this
        /// build writes is byte-identical to the one the old build reads.
        /// Both halves of the old build are reproduced literally here,
        /// since neither exists in this tree any more.
        #[tokio::test]
        async fn a_journal_this_build_writes_is_still_understood_by_a_rolled_back_binary() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");

            // This build serves a paying client and journals it.
            {
                let gate = file_gate(&path);
                gate.ingest(&evm_claim_json(&channel_id(), 12, 1200), 0)
                    .await
                    .expect("a genuine claim is accepted");
            }

            let entries = FileJournal::open(&path)
                .expect("open the journal file")
                .read_all()
                .expect("a journal this build wrote replays");

            // The pre-#643 replay: every key folded verbatim, no
            // canonicalisation anywhere.
            let mut legacy_watermarks: HashMap<String, Watermark> = HashMap::new();
            for entry in &entries {
                if let JournalEntry::InboundClaimAccepted {
                    channel_id,
                    nonce,
                    cumulative_amount,
                    ..
                } = entry
                {
                    legacy_watermarks.insert(
                        channel_id.clone(),
                        advance_watermark(*nonce, *cumulative_amount),
                    );
                }
            }

            // The pre-#643 key for the client's next claim: its own
            // `channelId`, namespaced, verbatim.
            let legacy_key = format!("evm:{}", channel_id());

            assert_eq!(
                legacy_watermarks.get(&legacy_key),
                Some(&Watermark {
                    nonce: 12,
                    cumulative_amount: 1200
                }),
                "a rolled-back binary must find the watermark this build wrote, \
                 or every spent nonce becomes spendable again"
            );
        }

        /// The other shape a live journal can be in: one written by a
        /// pre-#643 build that was *actually exploited*, so it carries
        /// several spellings of one channel, each with its own watermark.
        /// The fold merges them at the componentwise maximum -- the
        /// highest nonce and amount anyone ever reached on that channel --
        /// so the upgrade strictly tightens what comes next. It never
        /// resumes at the lower of the two, which would hand the client
        /// back the gap between them.
        #[tokio::test]
        async fn several_legacy_spellings_of_one_channel_merge_at_the_highest_watermark() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");

            {
                let journal = FileJournal::open(&path).expect("open the journal file");
                for (spelling, nonce, amount) in [
                    (format!("evm:0x{}", "ab".repeat(32)), 4u64, 400u64),
                    (format!("evm:0x{}", "AB".repeat(32)), 9, 900),
                    (format!("evm:0x{}", "aB".repeat(32)), 6, 600),
                ] {
                    journal
                        .append(&JournalEntry::InboundClaimAccepted {
                            channel_id: spelling,
                            nonce,
                            cumulative_amount: amount,
                            signature: vec![0x01],
                        })
                        .expect("append a legacy entry");
                }
            }

            let upgraded = file_gate(&path);
            assert_eq!(
                upgraded.watermark(&format!("evm:{}", channel_id())),
                Some(Watermark {
                    nonce: 9,
                    cumulative_amount: 900
                })
            );
            assert_eq!(
                upgraded
                    .ingest(&evm_claim_json(&channel_id(), 9, 900), 0)
                    .await,
                Err(ClaimIngestRejection::NonceNotAdvancing)
            );
        }

        /// A journal entry whose channel is in no namespace this build
        /// canonicalises -- the peer role shares the entry alphabet -- is
        /// folded byte for byte. Canonicalisation must never invent a
        /// channel out of a key it does not recognise.
        #[test]
        fn a_journal_entry_in_no_known_namespace_folds_under_its_own_key() {
            let replayed = replay_watermarks(&[JournalEntry::InboundClaimAccepted {
                channel_id: "channel-a".to_string(),
                nonce: 3,
                cumulative_amount: 30,
                signature: Vec::new(),
            }]);

            assert_eq!(
                replayed.get("channel-a").map(|record| record.watermark),
                Some(Watermark {
                    nonce: 3,
                    cumulative_amount: 30
                })
            );
            assert_eq!(replayed.len(), 1);
        }

        /// A claim this connector cannot durably record is refused, not
        /// accepted against a watermark that only exists in this process
        /// -- accepting it would be exactly the defect, one restart later.
        #[tokio::test]
        async fn a_claim_that_cannot_be_journaled_is_refused_and_advances_nothing() {
            let gate = ClientClaimGate::restore(test_channels(), Arc::new(UnwritableJournal))
                .expect("an unwritable journal still reads back empty");

            assert_eq!(
                gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await,
                Err(ClaimIngestRejection::NotDurable)
            );
            assert_eq!(gate.watermark(&format!("evm:{}", channel_id())), None);
        }

        /// A [`Journal`] that fails a set number of appends and then
        /// recovers -- ADR 0007's fake, not a mock: a disk that fills and
        /// is cleared, or a volume remounted writable, behaves exactly
        /// like this, and it is the situation `NotDurable`'s "retry"
        /// contract was written for.
        struct RecoveringJournal {
            failures_left: std::sync::atomic::AtomicU32,
            inner: InMemoryJournal,
        }

        impl RecoveringJournal {
            fn failing_once() -> RecoveringJournal {
                RecoveringJournal {
                    failures_left: std::sync::atomic::AtomicU32::new(1),
                    inner: InMemoryJournal::new(),
                }
            }
        }

        impl Journal for RecoveringJournal {
            fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
                use std::sync::atomic::Ordering;
                let remaining = self.failures_left.load(Ordering::SeqCst);
                if remaining > 0 {
                    self.failures_left.store(remaining - 1, Ordering::SeqCst);
                    return Err(JournalError::Io(std::io::Error::new(
                        std::io::ErrorKind::StorageFull,
                        "disk full",
                    )));
                }
                self.inner.append(entry)
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                self.inner.read_all()
            }
        }

        /// The half of `NotDurable`'s contract the group commit (issue
        /// #686) must not lose: "the same claim resubmitted once this
        /// connector's journal is writable again is still good". The
        /// advance happens before the fsync now, so a failed batch must
        /// roll it back -- a watermark left advanced would bounce the
        /// resubmission off its own ghost as `NonceNotAdvancing`, blaming
        /// a claim nothing was ever wrong with.
        #[tokio::test]
        async fn a_failed_batch_rolls_back_so_the_same_claim_is_good_once_the_journal_recovers() {
            let gate = ClientClaimGate::restore(
                test_channels(),
                Arc::new(RecoveringJournal::failing_once()),
            )
            .expect("an empty journal replays to nothing");

            assert_eq!(
                gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0).await,
                Err(ClaimIngestRejection::NotDurable)
            );
            assert_eq!(
                gate.watermark(&format!("evm:{}", channel_id())),
                None,
                "a refused acceptance must leave no watermark behind"
            );

            gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0)
                .await
                .expect("the identical claim, resubmitted after recovery, is still good");
            assert_eq!(
                gate.watermark(&format!("evm:{}", channel_id())),
                Some(Watermark {
                    nonce: 1,
                    cumulative_amount: 100
                })
            );
        }

        /// Issue #686's own invariant: enqueueing under the watermark
        /// lock keeps journal order identical to acceptance order, so a
        /// replay of what the group commit wrote reconstructs exactly the
        /// watermarks the live gate held -- under concurrency, which is
        /// the only condition group commit actually batches under.
        #[tokio::test(flavor = "multi_thread")]
        async fn group_committed_acceptances_replay_to_the_watermarks_the_live_gate_held() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            let first_key = format!("evm:{}", channel_id());
            let second_key = format!("evm:{}", second_channel_id());

            let (live_first, live_second) = {
                let gate = Arc::new(file_gate(&path));
                let claims_per_channel = 25u64;
                let mut tasks = Vec::new();
                for channel in [channel_id(), second_channel_id()] {
                    let gate = gate.clone();
                    tasks.push(tokio::spawn(async move {
                        for nonce in 1..=claims_per_channel {
                            gate.ingest(&evm_claim_json(&channel, nonce, nonce * 10), 0)
                                .await
                                .expect("strictly advancing claims are accepted");
                        }
                    }));
                }
                for task in tasks {
                    task.await.expect("ingest task");
                }
                (gate.watermark(&first_key), gate.watermark(&second_key))
            };

            // The "restart": a second gate over the same file.
            let restored = file_gate(&path);
            assert_eq!(restored.watermark(&first_key), live_first);
            assert_eq!(restored.watermark(&second_key), live_second);
            assert_eq!(
                live_first,
                Some(Watermark {
                    nonce: 25,
                    cumulative_amount: 250
                })
            );
        }

        /// The journal's entry order is the acceptance order -- the
        /// property the replay's soundness argument leans on, preserved
        /// across the group commit because entries are enqueued under the
        /// same write lock their watermarks advance under.
        #[tokio::test]
        async fn the_journal_records_acceptances_in_acceptance_order() {
            let journal = Arc::new(InMemoryJournal::new());
            let gate = ClientClaimGate::restore(test_channels(), journal.clone())
                .expect("nothing to replay");
            for nonce in 1..=3u64 {
                gate.ingest(&evm_claim_json(&channel_id(), nonce, nonce * 100), 0)
                    .await
                    .expect("accepted");
            }

            let nonces: Vec<u64> = journal
                .read_all()
                .unwrap()
                .iter()
                .map(|entry| match entry {
                    JournalEntry::InboundClaimAccepted { nonce, .. } => *nonce,
                    other => panic!("unexpected entry {other:?}"),
                })
                .collect();
            assert_eq!(nonces, vec![1, 2, 3]);
        }

        /// `NotDurable` is distinguishable from every other refusal, for
        /// the same reason the others are distinguishable from each other:
        /// nothing was wrong with this claim.
        #[test]
        fn a_not_durable_refusal_does_not_blame_the_claim() {
            let message = ClaimIngestRejection::NotDurable.message();
            assert!(message.contains("durably record"), "{message}");
            assert_ne!(
                ClaimIngestRejection::NotDurable,
                ClaimIngestRejection::SignatureInvalid
            );
        }

        /// A journal that cannot be read is a refusal to build a gate at
        /// all, which the caller turns into a refusal to start (ADR 0009)
        /// -- never a gate that quietly starts at no watermarks, since
        /// that is the state that accepts every spent claim.
        #[test]
        fn an_unreadable_journal_refuses_to_produce_a_gate() {
            let result = ClientClaimGate::restore(test_channels(), Arc::new(UnreadableJournal));
            assert!(matches!(result, Err(JournalError::Io(_))));
        }

        /// A corrupt line is the same refusal: this build will not guess
        /// what a line it cannot decode meant, and will not skip it.
        #[tokio::test]
        async fn a_corrupt_journal_line_refuses_to_produce_a_gate() {
            use std::io::Write;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("client-edge-claims.log");
            {
                let gate = file_gate(&path);
                gate.ingest(&evm_claim_json(&channel_id(), 1, 100), 0)
                    .await
                    .expect("one good entry");
            }
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(file, "this is not a journal entry").unwrap();
            drop(file);

            let result = ClientClaimGate::restore(
                test_channels(),
                Arc::new(FileJournal::open(&path).expect("open")),
            );
            assert!(matches!(result, Err(JournalError::Corrupt(_))));
        }

        /// A replayed watermark can only ever move forwards. The fold is
        /// componentwise `max` rather than last-wins precisely so that a
        /// journal whose entries are out of order -- however it got that
        /// way -- cannot hand a restarted node a *lower* watermark than
        /// one already accepted, which is the failure this whole
        /// mechanism exists to prevent.
        #[test]
        fn replay_never_recovers_a_watermark_lower_than_one_already_recorded() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 7,
                    cumulative_amount: 700,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 2,
                    cumulative_amount: 200,
                    signature: vec![2],
                },
            ];

            let watermarks = replay_watermarks(&entries);
            assert_eq!(
                watermarks.get("evm:0xabc").map(|record| record.watermark),
                Some(Watermark {
                    nonce: 7,
                    cumulative_amount: 700
                })
            );
        }

        /// Entries the peer role writes share this journal's alphabet but
        /// not this gate's authority: replaying them must not invent a
        /// client-edge watermark out of an outbound claim or a fulfilment.
        #[test]
        fn replay_ignores_entries_that_are_not_accepted_inbound_claims() {
            let entries = vec![
                JournalEntry::OutboundClaimSigned {
                    peer_id: "peer-b".to_string(),
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                },
                JournalEntry::InboundFulfillmentRecorded {
                    channel_id: "evm:0xabc".to_string(),
                    amount: 50,
                },
            ];

            assert!(replay_watermarks(&entries).is_empty());
        }

        /// Issue #977: a reset entry erases everything folded in for that
        /// channel *before* it in the same replay, and a later accepted
        /// entry re-accumulates from zero rather than from what the reset
        /// erased -- exactly the effect [`ClientClaimGate::reap_unresolvable_channels`]
        /// has live, reproduced on every replay.
        #[test]
        fn replay_watermarks_clears_prior_accumulation_on_a_reset_entry() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "solana:abc".to_string(),
                    nonce: 9,
                    cumulative_amount: 5_000_000,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimWatermarkReset {
                    channel_id: "solana:abc".to_string(),
                },
                JournalEntry::InboundClaimAccepted {
                    channel_id: "solana:abc".to_string(),
                    nonce: 1,
                    cumulative_amount: 100,
                    signature: vec![2],
                },
            ];

            let watermarks = replay_watermarks(&entries);
            assert_eq!(
                watermarks.get("solana:abc").map(|record| record.watermark),
                Some(Watermark {
                    nonce: 1,
                    cumulative_amount: 100
                }),
                "the post-reset claim's own nonce/amount, not a max against the erased \
                 pre-reset accumulation"
            );
        }

        /// A reset with nothing accepted after it leaves the channel with
        /// no watermark at all -- the state a never-before-seen channel is
        /// in, which is exactly what a reopened channel's first claim must
        /// be judged against.
        #[test]
        fn replay_watermarks_a_reset_with_nothing_after_it_leaves_no_watermark() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "solana:abc".to_string(),
                    nonce: 9,
                    cumulative_amount: 5_000_000,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimWatermarkReset {
                    channel_id: "solana:abc".to_string(),
                },
            ];

            assert!(!replay_watermarks(&entries).contains_key("solana:abc"));
        }

        /// Issue #1012: a rollback entry SETS the channel's watermark back
        /// to the value it names rather than folding into it by
        /// componentwise `max`, which -- the fold every accepted claim uses
        /// -- would silently undo the very thing the entry records.
        #[test]
        fn replay_watermarks_puts_a_rolled_back_channel_back_to_the_named_watermark() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 10,
                    cumulative_amount: 1_000,
                    signature: vec![2],
                },
                JournalEntry::InboundClaimRolledBack {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                },
            ];

            assert_eq!(
                replay_watermarks(&entries)
                    .get("evm:0xabc")
                    .map(|record| record.watermark),
                Some(Watermark {
                    nonce: 9,
                    cumulative_amount: 900
                }),
                "the rolled-back value, not the `max` of it and the claim it undoes"
            );
        }

        /// The rollback restores not just the prior watermark but the
        /// prior claim's own signature -- the whole point of #1218 -- since
        /// the entry that undid the second claim carries no signature of
        /// its own to fall back to.
        #[test]
        fn replay_watermarks_puts_a_rolled_back_channel_back_to_the_named_signature() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 10,
                    cumulative_amount: 1_000,
                    signature: vec![2],
                },
                JournalEntry::InboundClaimRolledBack {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                },
            ];

            assert_eq!(
                replay_watermarks(&entries)
                    .get("evm:0xabc")
                    .map(|record| record.signature.clone()),
                Some(vec![1]),
                "the first claim's own signature, not the second claim's -- the second's \
                 acceptance was undone by the rollback"
            );
        }

        /// The other half of the same rule: the claim the client resubmits
        /// after a rollback advances the channel again, from the restored
        /// watermark.
        #[test]
        fn replay_watermarks_advances_again_after_a_rollback() {
            let entries = vec![
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 10,
                    cumulative_amount: 1_000,
                    signature: vec![1],
                },
                JournalEntry::InboundClaimRolledBack {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 9,
                    cumulative_amount: 900,
                },
                JournalEntry::InboundClaimAccepted {
                    channel_id: "evm:0xabc".to_string(),
                    nonce: 10,
                    cumulative_amount: 1_000,
                    signature: vec![2],
                },
            ];

            assert_eq!(
                replay_watermarks(&entries)
                    .get("evm:0xabc")
                    .map(|record| record.watermark),
                Some(Watermark {
                    nonce: 10,
                    cumulative_amount: 1_000
                })
            );
        }

        /// Issue #1012, end to end through the gate rather than through
        /// `replay_watermarks` alone: a rollback is durable, so a restart
        /// recovers the pre-claim watermark and judges the resubmitted
        /// claim as fresh. A rollback a restart could forget would leave
        /// the client durably charged for a packet this connector decided
        /// not to count.
        #[tokio::test]
        async fn a_rolled_back_claim_is_forgotten_across_a_restart() {
            let key = format!("evm:{}", channel_id());
            let journal = Arc::new(InMemoryJournal::new());
            let gate = ClientClaimGate::restore(test_channels(), journal.clone())
                .expect("nothing to replay");
            gate.ingest(&evm_claim_json(&channel_id(), 3, 300), 0)
                .await
                .expect("accepted");
            gate.roll_back(&key, 3, 300)
                .await
                .expect("the rollback is durably recorded");

            assert_eq!(gate.watermark(&key), None);
            let restarted = ClientClaimGate::restore(test_channels(), journal)
                .expect("the journal replays, rollback and all");
            assert_eq!(
                restarted.watermark(&key),
                None,
                "the rollback survived the restart"
            );
            restarted
                .ingest(&evm_claim_json(&channel_id(), 3, 300), 0)
                .await
                .expect("the identical claim is judged as fresh as it was the first time");
        }

        /// Issue #1012's first documented no-op: a rollback naming a claim
        /// a later one has already superseded leaves the channel exactly
        /// where that later claim put it. Unwinding it would erase state
        /// the reject that provoked this call has nothing to do with.
        #[tokio::test]
        async fn rolling_back_a_superseded_claim_leaves_the_later_one_alone() {
            let key = format!("evm:{}", channel_id());
            let journal = Arc::new(InMemoryJournal::new());
            let gate =
                ClientClaimGate::restore(test_channels(), journal).expect("nothing to replay");
            gate.ingest(&evm_claim_json(&channel_id(), 3, 300), 0)
                .await
                .expect("accepted");
            gate.ingest(&evm_claim_json(&channel_id(), 4, 400), 0)
                .await
                .expect("accepted");

            gate.roll_back(&key, 3, 300)
                .await
                .expect("a no-op is not an error");

            assert_eq!(
                gate.watermark(&key),
                Some(Watermark {
                    nonce: 4,
                    cumulative_amount: 400
                })
            );
        }

        /// The journal keeps the claim itself, not merely its watermark:
        /// a watermark says what was spent, but only the signed claim is
        /// redeemable on chain (issue #425), and this edge's claims are
        /// the only ones a client-facing node ever holds.
        #[tokio::test]
        async fn an_accepted_claim_is_journaled_with_the_signature_it_was_verified_by() {
            let journal = Arc::new(InMemoryJournal::new());
            let gate = ClientClaimGate::restore(test_channels(), journal.clone())
                .expect("nothing to replay");
            gate.ingest(&evm_claim_json(&channel_id(), 3, 300), 0)
                .await
                .expect("accepted");

            let entries = journal.read_all().unwrap();
            assert_eq!(entries.len(), 1);
            let JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                signature,
            } = &entries[0]
            else {
                panic!("expected an accepted-claim entry, got {:?}", entries[0]);
            };
            // Journaled under the canonical key (issue #643), so a
            // replay of this file files the watermark exactly where a
            // live acceptance would have -- and, since the canonical key
            // keeps the `0x`, exactly where a *pre*-#643 build would have
            // too, which is what makes an image rollback a no-op.
            assert_eq!(channel_id, &format!("evm:0x{}", "ab".repeat(32)));
            assert_eq!(*nonce, 3);
            assert_eq!(*cumulative_amount, 300);
            assert_eq!(
                signature.len(),
                65,
                "the raw 65-byte EIP-712 signature, not its hex text"
            );
        }
    }

    #[tokio::test]
    async fn a_mina_claim_is_never_routed_into_signature_verification() {
        // Mina is refused at structural parsing (ADR 0002), long before
        // this gate would ever reach a signature check -- there is no Mina
        // arm in `verify_claim_signature` to route into.
        let gate = gate();
        let json = r#"{
            "version": "1.0",
            "blockchain": "mina",
            "messageId": "claim-mina",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-dave",
            "zkAppAddress": "irrelevant",
            "tokenId": "1",
            "balanceCommitment": "abc",
            "nonce": 1,
            "proof": "AAAA",
            "salt": "salt"
        }"#;
        assert_eq!(gate.ingest(json, 0).await, Err(ClaimIngestRejection::Mina));
    }

    /// The gate's half of issue #613: what a sender is *told* when this
    /// connector declines to look their channel up, and that it is told
    /// apart from the two refusals either side of it.
    mod lookup_budget {
        use super::*;
        use crate::lookup_budget::LookupBudgetBound;
        use crate::UnresolvableLookupBudgetPolicy;
        use std::time::Duration;

        /// A gate that resolves from a chain knowing nothing at all, with
        /// `allowance` lookups per (very long) window -- long enough that
        /// every test below runs entirely inside one of them however slow
        /// the machine is, so nothing here is a race against a wall clock.
        fn gate_allowing(allowance: u32) -> ClientClaimGate {
            gate_over(
                ClientChannelRegistry::new()
                    .with_source(Arc::new(FakeChannelSource::knowing(vec![])))
                    .with_lookup_budget(UnresolvableLookupBudgetPolicy {
                        per_signer: allowance,
                        total: allowance,
                        window: Duration::from_secs(600),
                        // Zero, so a refusal is observable immediately
                        // rather than as a sleep -- the waiting is
                        // `crate::lookup_budget`'s own subject, and these
                        // tests are about what a *sender* is told.
                        max_wait: Duration::ZERO,
                    }),
            )
        }

        /// A claim on a never-recorded channel, declaring `signer` as its
        /// own payer. The signature is genuine but produced by the test
        /// keypair rather than by `signer`, which is exactly the point: a
        /// declared signer is not a credential, and this refusal is decided
        /// before any signature is checked anyway.
        fn claim_from(signer: &Address, nonce: u64) -> String {
            let (secret, _) = evm_signer();
            evm_claim_json_signed_by(&secret, signer, &format!("0x{:064x}", nonce), nonce, 1_000)
        }

        /// Three refusals, one gate, all distinct -- and each one sends an
        /// operator somewhere different. Collapsing any two of them fails
        /// here.
        #[tokio::test]
        async fn exhaustion_is_told_apart_from_an_unknown_channel_and_a_failed_lookup() {
            let gate = gate_allowing(1);
            let (_, payer) = evm_signer();

            // The chain answered, and said there is no such channel.
            assert_eq!(
                gate.ingest(&claim_from(&payer, 1), 0).await,
                Err(ClaimIngestRejection::UnknownChannel)
            );

            // The allowance is now spent, so the chain is not asked at all.
            let budgeted = gate
                .ingest(&claim_from(&payer, 2), 0)
                .await
                .expect_err("the allowance is spent");
            assert_eq!(
                budgeted,
                ClaimIngestRejection::LookupBudgetExhausted {
                    bound: LookupBudgetBound::Node,
                    allowance: 1,
                    window_secs: 600,
                    max_wait_ms: 0,
                }
            );
            assert_ne!(budgeted, ClaimIngestRejection::UnknownChannel);
            assert_ne!(
                budgeted,
                ClaimIngestRejection::ChannelLookupFailed("connection refused".to_string())
            );

            // And a sender reading only the message can tell too: it says
            // what was withheld and how long to wait, and never that their
            // channel does not exist.
            let message = budgeted.message();
            assert!(message.contains("discovery drain"), "{message}");
            assert!(
                message.contains("Nothing is wrong with the claim"),
                "{message}"
            );
            assert!(
                !message.contains("no counterparty to verify"),
                "a shaped refusal must not read as an unknown channel: {message}"
            );
        }

        /// A chain that is genuinely unreachable is reported as an outage,
        /// in the endpoint's own words, rather than as a budget -- the
        /// acceptance criterion that a node whose RPC is down must degrade
        /// loudly rather than look like it is under attack.
        #[tokio::test]
        async fn an_unreachable_chain_is_still_reported_as_a_lookup_failure() {
            let gate = gate_over(
                ClientChannelRegistry::new()
                    .with_source(Arc::new(FakeChannelSource::unreachable(
                        "connection refused",
                    )))
                    .with_lookup_budget(UnresolvableLookupBudgetPolicy {
                        per_signer: 4,
                        total: 4,
                        window: Duration::from_secs(600),
                        max_wait: Duration::ZERO,
                    }),
            );
            let (_, payer) = evm_signer();

            assert_eq!(
                gate.ingest(&claim_from(&payer, 1), 0).await,
                Err(ClaimIngestRejection::ChannelLookupFailed(
                    "connection refused".to_string()
                ))
            );
        }

        /// Each sender reaches its own bucket -- i.e. the gate really does
        /// pass the claim's declared signer down to the shaper rather than
        /// budgeting everything as one caller.
        ///
        /// What that buys is deliberately understated here, because the
        /// declared signer is read and not verified: a sender who exhausts
        /// one bucket can simply declare another. The node-wide drain is
        /// what actually bounds them, and the per-signer split is measured
        /// against an exact clock in
        /// `crate::lookup_budget::two_signers_have_independent_allowances`,
        /// where the queueing band it lives in can be reproduced without a
        /// race.
        #[tokio::test]
        async fn each_declared_sender_reaches_its_own_bucket() {
            let gate = gate_allowing(2);
            let first: Address = [0xaa; 20];
            let second: Address = [0xbb; 20];

            assert_eq!(
                gate.ingest(&claim_from(&first, 1), 0).await,
                Err(ClaimIngestRejection::UnknownChannel)
            );
            assert_eq!(
                gate.ingest(&claim_from(&second, 2), 0).await,
                Err(ClaimIngestRejection::UnknownChannel),
                "a second declared sender is looked up for on its own account"
            );
            assert!(
                matches!(
                    gate.ingest(&claim_from(&second, 3), 0).await,
                    Err(ClaimIngestRejection::LookupBudgetExhausted { .. })
                ),
                "and the node-wide drain is what stops them both"
            );
        }
    }
}
