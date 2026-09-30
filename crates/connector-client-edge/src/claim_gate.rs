//! Claim ingest gate for the client edge (`docs/protocol/client-edge-spec.md`
//! §1.3): turns the `ILP-Payment-Channel-Claim` (`-Wrapped`) header's
//! already-decoded JSON into a structurally valid, fresh, value-covering,
//! cryptographically verified, collateral-backed [`ClientClaim`], or a
//! documented refusal -- structure, then freshness, then value binding
//! against the matched route's price, then (only once all three have
//! passed) the voucher's signature against its channel's voucher signer as
//! the chain records it, then collateral. A replay or an underpayment is
//! refused before this ingress ever spends a channel lookup or a signature
//! check on it (issue #544).
//!
//! **Every claim is a voucher** (ADR 0074, ADR 0075 decision 8, issue
//! #1384): an x402 `batch-settlement` claim. A voucher on a chain this node
//! does not settle on is refused by name ([`ClaimIngestRejection::BatchSettlementNotAccepted`]);
//! its freshness is `connector_domain::validate_voucher`'s amount-only
//! watermark, under the canonical (blockchain, channel) key; a
//! byte-identical resend at its watermark is answered without advancing or
//! journaling anything; its signer is read from the backend's verified
//! channel ([`BatchSettlementChannels`]), never from the voucher; and its
//! acceptance is journaled. A channel's first accepted voucher also
//! journals the channel itself ([`JournalEntry::BatchChannelAdmitted`]),
//! because on EVM the config it carries is the only way to land any voucher
//! on it after a restart. A lookup for a channel with no record is metered
//! by issue #613's unresolvable-lookup budget. See
//! `ClientClaimGate::admit_voucher`.
//!
//! **A `toon-channel` claim is refused by name** -- a claim with no
//! `scheme`, or with `scheme: "toon-channel"` ([`ClaimIngestRejection::ToonChannel`]),
//! the way a `mina` claim is ([`ClaimIngestRejection::Mina`]). Its whole
//! path -- the per-channel counterparty registry and its chain sources, the
//! balance-proof verification, the deposit floor, the nonce rule and the
//! sweep that reset a settled `TokenNetwork` channel's watermark -- is
//! deleted with it (issue #1384).
//!
//! **What survives a restart** (issue #605): every watermark this gate
//! advances is written to a [`Journal`] -- the same ADR 0005 port, and the
//! same [`JournalEntry::InboundClaimAccepted`] alphabet, as every other
//! book. A gate can only be built by [`ClientClaimGate::restore`], which
//! replays that journal before serving anything, so a process that restarts
//! resumes at the watermark it left off at instead of at `None` -- and an
//! empty watermark admits any voucher naming more than zero, which would
//! make every voucher the client already spent free service again. Three
//! consequences are deliberate and load-bearing:
//!
//! * A journal that cannot be read, or that has a line this build cannot
//!   decode, is an error out of [`ClientClaimGate::restore`] -- the node
//!   refuses to start rather than starting from zero, since starting from
//!   zero is precisely the defect.
//! * Every watermark -- live and journaled -- is filed under the
//!   *canonical* channel key `connector_domain::client_claim::canonical_channel_key`
//!   produces, never the literal text a voucher happened to arrive with
//!   (issue #643). One channel has many spellings (`channelId` is
//!   case-insensitive hex), and a watermark that did not treat them as one
//!   would hand a client a fresh empty watermark per spelling.
//!   [`replay_watermarks`] canonicalises on the way *out* of the journal
//!   too.
//! * A voucher whose acceptance cannot be made durable is **refused**
//!   ([`ClaimIngestRejection::NotDurable`]) and advances nothing. Since
//!   issue #686 the journal append is **group-committed**: the write lock
//!   covers only the authoritative re-check, the watermark advance and
//!   enqueueing the entry with a dedicated committer thread -- microseconds,
//!   no I/O -- and the committer batches everything queued into one journal
//!   write and one fsync ([`Journal::append_batch`]). Enqueueing under the
//!   lock keeps journal order identical to watermark order, and a voucher is
//!   only handed back for its packet to be routed once the committer reports
//!   its batch durable, so ADR 0005's "journal written before value is
//!   considered moved" still holds. A batch that cannot be made durable is
//!   rolled back under the same write lock and every waiting voucher refused
//!   as [`ClaimIngestRejection::NotDurable`], so the same voucher resubmitted
//!   once the journal is writable again is still good.
//!
//! **The watermark key deliberately does not include the client-edge
//! sender identity `resolve_identity` produces (issue #502).** The channel
//! already names its one payer; folding a self-declared HTTP identity into
//! the key would let one channel hold a distinct watermark per identity
//! that presented it, which *reopens* the replay this watermark exists to
//! close.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, RwLock};

use connector_domain::client_claim::{
    canonical_channel_key, parse_client_claim, ClientClaim, ClientClaimError,
    EvmVoucherChannelConfig, EVM_NAMESPACE,
};
use connector_domain::{
    advance_voucher_watermark, validate_voucher, ClaimError, JournalEntry, VoucherAdmission,
    VoucherWatermark, Watermark, VOUCHER_WATERMARK_NONCE,
};
use connector_runtime::{Journal, JournalError};
use connector_signer::{
    evm_batch_channel_id, evm_voucher_signer, verify_evm_voucher, verify_solana_voucher,
    BatchChannelConfig, BatchSettlementDomain, VoucherSignature,
};

use crate::batch_settlement::{
    journaled_batch_channels, AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel,
    BatchSettlementChannels, JournaledBatchChannel,
};
use crate::channels::{decode_base58_bytes, decode_hex_bytes, ChannelResolutionError};
use crate::lookup_budget::{
    LookupBudgetBound, LookupReservation, UnresolvableLookupBudget, UnresolvableLookupBudgetPolicy,
};
use crate::outbound_ledger::{ClientPayoutLedger, PayoutVoucher};
use connector_settlement::batch::VoucherSigner;

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
    /// A claim with no `scheme`, or `scheme: "toon-channel"` (ADR 0075
    /// decision 8, issue #1384): the retired `toon-channel` claim, refused by
    /// name the way [`ClaimIngestRejection::Mina`] is -- this connector no
    /// longer takes it, which is not the same as it making no sense.
    ToonChannel,
    AmountNotAdvancing,
    Underpayment {
        advanced: u128,
        price: u64,
    },
    /// The voucher names a channel the settlement backend does not admit
    /// (issue #558's rule: an unverifiable claim is never accepted), so there
    /// is no signer its signature could be checked against.
    UnknownChannel,
    /// The voucher names a channel the backend knows to be done -- sealed,
    /// or withdrawn -- kept distinct from [`ClaimIngestRejection::UnknownChannel`]
    /// because "this channel is done" is a stronger, more actionable fact
    /// than "this connector has no record of it".
    ChannelTerminal(String),
    /// This connector could not find out who the voucher's channel belongs
    /// to (issue #556) -- its settlement backend's lookup failed, e.g. an
    /// unreachable RPC endpoint. Distinct from
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
    /// The voucher is fresh, well-formed, correctly signed and covers the
    /// route's price -- and names a cumulative amount larger than its
    /// channel can pay (issue #646, ADR 0074 decision 5), so it could never
    /// be landed. Accepting it would be doing work this connector can
    /// provably never be paid for.
    ///
    /// Kept distinct from [`ClaimIngestRejection::Underpayment`] for
    /// exactly the reason every other variant here is kept distinct: this
    /// claim *does* cover the price, and telling a payer it underpaid
    /// would send them to fix the wrong thing. The remedy is the one both
    /// channels share -- deposit more and resubmit the same voucher, which
    /// nothing here has consumed.
    ///
    /// The ceiling is the deposit alone: a payout this connector owes the
    /// same client rides a channel of its own and nets against nothing
    /// (ADR 0075 decision 7, retiring issue #700's netting).
    Undercollateralized {
        claimed: u128,
        deposited: u128,
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
    /// The voucher is on a chain this connector does not settle on -- it has
    /// no `[settlement.<chain>]` table, so no backend to verify the voucher's
    /// channel against. Refused by name, before anything about the voucher is
    /// judged -- distinct from [`ClaimIngestRejection::Malformed`] because
    /// nothing is wrong with it; the greeting's `accepts[]` lists the chains
    /// this connector does take.
    BatchSettlementNotAccepted,
    /// An EVM voucher's `channelConfig` does not hash to the `channelId` it
    /// signs (ADR 0074 decision 2: the connector recomputes `getChannelId`
    /// and refuses a mismatch). The config is where the voucher's signer is
    /// read from, so one that is not the channel's names nobody.
    VoucherChannelConfigMismatch,
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
            ClaimIngestRejection::ToonChannel => format!(
                "claim rejected: {}",
                ClientClaimError::ToonChannel
            ),
            ClaimIngestRejection::AmountNotAdvancing => "claim rejected: cumulative amount does \
                 not strictly exceed this channel's watermark (replay)"
                .to_string(),
            ClaimIngestRejection::Underpayment { advanced, price } => format!(
                "claim rejected: advances value by {advanced}, less than this route's price of {price}"
            ),
            ClaimIngestRejection::UnknownChannel => "claim rejected: names a channel this \
                 connector does not admit, so there is no voucher signer to verify its \
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
                 verify against this channel's voucher signer"
                .to_string(),
            ClaimIngestRejection::Undercollateralized { claimed, deposited } => format!(
                "claim rejected: claims a cumulative {claimed}, more than the {deposited} this \
                 channel can pay, so it could never be landed -- deposit at least {claimed} and \
                 resubmit this same claim"
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
            ClaimIngestRejection::BatchSettlementNotAccepted => "claim rejected: it is a \
                 'batch-settlement' voucher on a chain this connector does not settle on -- pay \
                 on a chain its greeting's 'accepts' lists"
                .to_string(),
            ClaimIngestRejection::VoucherChannelConfigMismatch => "claim rejected: the \
                 voucher's 'channelConfig' does not hash to the 'channelId' it signs"
                .to_string(),
        }
    }
}

/// A channel's live watermark together with the exact claim bytes that
/// produced it (issue #1218): a watermark alone says what was spent, but
/// only the signature is redeemable, so it is retained alongside the
/// watermark rather than discarded once the acceptance decision is made.
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
    /// How many lookups for channels this gate has no record of it will
    /// perform per window, per declared sender and in total (issue #613):
    /// a voucher naming a channel nobody opened is a free chain read for
    /// whoever sends it, so discovery is shaped here.
    lookup_budget: UnresolvableLookupBudget,
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
    /// What this connector pays clients back, as vouchers on its outbound
    /// channels toward them (ADR 0075 decision 7, issue #1381). `None` --
    /// every constructor's default -- pays nothing. Never consulted on the
    /// admission path: a payout nets against nothing.
    payout_ledger: Option<Arc<ClientPayoutLedger>>,
    /// Each live client session's payee, keyed by the **authenticated
    /// session** -- the generation `SessionRegistry::bind` issued it -- never
    /// by the ILP address alone (issues #787, #1381, #1396). A session's
    /// `peerId` is its own say-so; nothing verifies it, and two sockets can
    /// declare the same one. Keyed by address, whatever key the latest
    /// socket to declare an address proved decided where every session at
    /// that address was paid; keyed by generation, what a session proves is
    /// visible to that session alone.
    ///
    /// [`Self::open_session`] creates a session's slot, empty, when it
    /// binds; [`Self::record_session_payee`] fills it, and each of its
    /// callers has verified a signature by the key first; and
    /// [`Self::close_session`] removes it when the session unbinds. A record
    /// for a session that is not open -- one that has already closed, whose
    /// voucher finished its durability wait afterwards, say -- is dropped,
    /// so nothing here outlives its session. Best-effort and non-durable,
    /// like [`Self::last_claim_seen`]: a reconnecting client proves its key
    /// again, by voucher or by `channelChallenge`, on its new session.
    session_payees: RwLock<HashMap<u64, SessionPayee>>,
    /// The voucher signer of every batch-settlement channel this gate has
    /// accepted a voucher on since it started, by canonical key: read from
    /// the chain when the voucher was verified, never from the voucher. How
    /// an accepted voucher teaches a session its payee.
    voucher_signers: RwLock<HashMap<String, VoucherSigner>>,
    /// The x402 `batch-settlement` backend vouchers are admitted through
    /// (ADR 0074, issue #1341) -- `None`, every constructor's default, on a
    /// node that settles on no chain, and then every voucher is refused by
    /// name. See [`crate::batch_settlement`].
    batch_settlement: Option<Arc<dyn BatchSettlementChannels>>,
    /// Every batch-settlement channel this gate has accepted a voucher on,
    /// by canonical key (ADR 0074): replayed from the journal's
    /// [`JournalEntry::BatchChannelAdmitted`] records, and added to under
    /// the watermark write lock as a channel's first voucher is enqueued.
    /// It is what tells a voucher channel's watermark apart from one a
    /// pre-ADR 0075 journal left behind, and it holds the EVM config a
    /// voucher without one is admitted from.
    ///
    /// Never shrinks: a record outlives a failed batch or a rollback, which
    /// costs nothing, because the next first voucher on the channel records
    /// it again and the config never changes.
    ///
    /// Wherever both are held, this lock is taken after [`Self::watermarks`].
    batch_channels: RwLock<HashMap<String, JournaledBatchChannel>>,
}

/// One client session's slot in [`ClientClaimGate`]'s payee map (issue
/// #1396): the address it bound at, and the payee it has proved, if any.
#[derive(Debug)]
struct SessionPayee {
    address: String,
    payee: Option<VoucherSigner>,
}

impl ClientClaimGate {
    /// A gate resuming from the watermarks `journal` already records (issue
    /// #605), accepting no voucher until [`Self::with_batch_settlement`]
    /// gives it a backend.
    ///
    /// This is the only way to build a gate, and it always replays: there
    /// is deliberately no constructor that starts a gate at no watermarks
    /// without saying where its watermarks came from, because a gate that
    /// silently starts at `None` accepts every voucher a client has already
    /// spent.
    ///
    /// # Errors
    ///
    /// A journal that cannot be read, or that carries a line this build
    /// cannot decode ([`JournalError::Corrupt`]). The caller must fail --
    /// per ADR 0009, before anything else starts -- rather than fall back
    /// to an empty set of watermarks.
    pub fn restore(journal: Arc<dyn Journal>) -> Result<ClientClaimGate, JournalError> {
        let entries = journal.read_all()?;
        let watermarks = Arc::new(RwLock::new(replay_watermarks(&entries)));
        let batch_channels = journaled_batch_channels(&entries)?
            .into_iter()
            .map(|channel| (channel.channel_key(), channel))
            .collect();
        let committer = GroupCommitter::spawn(journal, Arc::clone(&watermarks));
        Ok(ClientClaimGate {
            lookup_budget: UnresolvableLookupBudget::default(),
            watermarks,
            previous_watermarks: RwLock::new(HashMap::new()),
            committer,
            last_claim_seen: RwLock::new(HashMap::new()),
            payout_ledger: None,
            session_payees: RwLock::new(HashMap::new()),
            voucher_signers: RwLock::new(HashMap::new()),
            batch_settlement: None,
            batch_channels: RwLock::new(batch_channels),
        })
    }

    /// Accept x402 `batch-settlement` vouchers through `backend`. Without
    /// this, a voucher is refused as
    /// [`ClaimIngestRejection::BatchSettlementNotAccepted`]. `connector-cli`'s
    /// runtime calls it for every chain this node settles on.
    pub fn with_batch_settlement(
        mut self,
        backend: Arc<dyn BatchSettlementChannels>,
    ) -> ClientClaimGate {
        self.batch_settlement = Some(backend);
        self
    }

    /// Shape discovery lookups -- for channels this gate has no record of --
    /// by `policy` (issue #613): the knobs a node's config file turns
    /// (`unresolvable_lookup_budget_*`).
    pub fn with_lookup_budget(mut self, policy: UnresolvableLookupBudgetPolicy) -> ClientClaimGate {
        self.lookup_budget = UnresolvableLookupBudget::new(policy);
        self
    }

    /// Pay clients back through `ledger` (ADR 0075 decision 7, issue
    /// #1381): a client session's earnings are signed as vouchers on this
    /// node's outbound channel toward the session's payee key
    /// ([`Self::record_session_payee`]). Nothing a payout signs touches
    /// this gate's own admission: a client's inbound collateral and its
    /// payout channel are independent, and a payout no longer raises what
    /// the client may spend (ADR 0026's #700 netting is retired).
    pub fn with_payout_ledger(mut self, ledger: Arc<ClientPayoutLedger>) -> ClientClaimGate {
        self.payout_ledger = Some(ledger);
        self
    }

    /// This gate's payout ledger, if [`Self::with_payout_ledger`] configured
    /// one.
    pub(crate) fn payout_ledger(&self) -> Option<&Arc<ClientPayoutLedger>> {
        self.payout_ledger.as_ref()
    }

    /// Open the payee slot of the client session `generation` just bound at
    /// `address` -- empty: a new session is paid nowhere until it proves a
    /// key on itself (issue #1396), whatever an earlier session at the same
    /// address proved. Called from the BTP auth branch, right after
    /// `SessionRegistry::bind` issued `generation`.
    pub(crate) fn open_session(&self, address: &str, generation: u64) {
        self.session_payees
            .write()
            .expect("session payee map lock poisoned")
            .insert(
                generation,
                SessionPayee {
                    address: address.to_string(),
                    payee: None,
                },
            );
    }

    /// Close the payee slot [`Self::open_session`] opened for `generation`:
    /// called wherever the BTP session calls `SessionRegistry::unbind` for
    /// the same pair. Generations are never reused, so closing a superseded
    /// session touches nothing the session that superseded it learned.
    pub(crate) fn close_session(&self, address: &str, generation: u64) {
        let mut payees = self
            .session_payees
            .write()
            .expect("session payee map lock poisoned");
        if payees
            .get(&generation)
            .is_some_and(|slot| slot.address == address)
        {
            payees.remove(&generation);
        }
    }

    /// Learn that the client session `generation`, bound at `address`, is
    /// paid at `payee`, the key a payout channel toward it names as
    /// receiver. Every caller has verified a signature by `payee` first, on
    /// this same session: `crate::btp::record_accepted_claim`, once
    /// [`Self::admit`] has accepted a voucher whose channel's voucher signer
    /// is `payee`, and `crate::btp::verify_and_record_declared_channel`,
    /// once a voucher claim-state challenge at BTP auth has verified against
    /// `payee`, its channel's voucher signer as the chain records it (issues
    /// #790, #1384).
    ///
    /// Scoped to the session (issue #1396): it never changes any other
    /// session's payee, another session's at the same address included.
    /// Within the session, the latest proof wins. A session that is not
    /// open learns nothing. `true` if this changed the session's payee --
    /// the signal that a payout already owed to `payee` may now be
    /// deliverable to this session.
    pub(crate) fn record_session_payee(
        &self,
        address: &str,
        generation: u64,
        payee: VoucherSigner,
    ) -> bool {
        let mut payees = self
            .session_payees
            .write()
            .expect("session payee map lock poisoned");
        match payees.get_mut(&generation) {
            Some(slot) if slot.address == address => slot.payee.replace(payee) != Some(payee),
            _ => false,
        }
    }

    /// The payee the client session `generation`, bound at `address`, has
    /// proved -- `None` for a session that proved none, one that has closed,
    /// or a generation that was never bound at `address`. There is no
    /// lookup by address alone (issue #1396).
    pub(crate) fn session_payee(&self, address: &str, generation: u64) -> Option<VoucherSigner> {
        self.session_payees
            .read()
            .expect("session payee map lock poisoned")
            .get(&generation)
            .filter(|slot| slot.address == address)
            .and_then(|slot| slot.payee)
    }

    /// The session `generation`'s payee together with this gate's payout
    /// ledger (issue #779): what `session_route::deliver_pending_claim`
    /// needs to resend a stranded payout voucher. `None` if no ledger is
    /// configured or the session has proved no payee -- both reasons there
    /// is nothing to resend, not errors.
    pub(crate) fn payout_for_session(
        &self,
        destination: &str,
        generation: u64,
    ) -> Option<(VoucherSigner, Arc<ClientPayoutLedger>)> {
        let payee = self.session_payee(destination, generation)?;
        let ledger = Arc::clone(self.payout_ledger()?);
        Some((payee, ledger))
    }

    /// Pay the client session `generation`, bound at `destination` (a
    /// session's bound ILP address, never a channel id, issue #787),
    /// `amount` for the job `job_id` it did, through
    /// [`ClientPayoutLedger::record_payout_once`] -- at the payee *that
    /// session* proved (issue #1396), never one another session at the same
    /// address proved.
    ///
    /// `payee_at_dispatch` is the payee the same session had proved when
    /// the job was handed to it, read by the caller before the session
    /// answered: it stands in when the session has since closed (a client
    /// may drop its socket the moment it has fulfilled), so the job is still
    /// paid, and left pending for the client's next session to be resent.
    /// It is never another session's payee.
    ///
    /// `None`, logged rather than left silent, for a session that has
    /// proved no payee -- neither paid a voucher nor proved a channel with a
    /// challenge at auth -- and under every condition `record_payout_once`
    /// itself declines on.
    pub(crate) async fn credit_session_payout(
        &self,
        destination: &str,
        generation: u64,
        payee_at_dispatch: Option<VoucherSigner>,
        job_id: &[u8; 32],
        amount: u64,
    ) -> Option<PayoutVoucher> {
        let ledger = self.payout_ledger()?;
        let Some(payee) = self
            .session_payee(destination, generation)
            .or(payee_at_dispatch)
        else {
            tracing::info!(
                destination = %destination,
                generation,
                "the session that did this job has proved no payee -- paying nothing"
            );
            return None;
        };
        ledger.record_payout_once(payee, job_id, amount).await
    }

    /// The voucher signer of the batch-settlement channel `channel_key`, as
    /// the chain recorded it when this gate last accepted a voucher on it:
    /// EVM `payerAuthorizer`, Solana `authorized_signer`. `None` for a
    /// channel this process has accepted no voucher on.
    pub(crate) fn voucher_signer(&self, channel_key: &str) -> Option<VoucherSigner> {
        self.voucher_signers
            .read()
            .expect("voucher signers lock poisoned")
            .get(&canonical_channel_key(channel_key))
            .copied()
    }

    /// The watermark this gate currently holds for `channel_key` (the
    /// chain-namespaced key `ClientClaim::channel_key` produces), or `None`
    /// if it has never accepted a voucher on that channel. Read-only: the
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

    /// The latest voucher this gate has accepted on `channel_key`, as
    /// `(cumulative_amount, signature)` -- exactly what landing it submits
    /// (issue #1218). `None` before any voucher has been accepted on this
    /// channel.
    pub fn latest_inbound_claim(&self, channel_key: &str) -> Option<(u128, Vec<u8>)> {
        self.watermarks
            .read()
            .expect("client claim watermarks lock poisoned")
            .get(&canonical_channel_key(channel_key))
            .map(|record| (record.watermark.cumulative_amount, record.signature.clone()))
    }

    /// Every x402 `batch-settlement` channel this gate has accepted a
    /// voucher on (ADR 0074), as the journal records it. With
    /// [`Self::latest_inbound_claim`] under each one's
    /// [`channel_key`](JournaledBatchChannel::channel_key), this is what a
    /// watcher or sweep (issue #1344) lands: the channel, and the latest
    /// voucher's amount and signature.
    pub fn batch_channels(&self) -> Vec<JournaledBatchChannel> {
        self.batch_channels
            .read()
            .expect("batch channels lock poisoned")
            .values()
            .copied()
            .collect()
    }

    /// The EVM batch-settlement channel `channel_id`, found for a claim-state
    /// read (issue #1364) the way [`Self::admit_voucher`] finds it for a
    /// voucher, with the domain its challenge is verified under: the config
    /// is `presented` if the request carried one, else this gate's record of
    /// the channel, and either must hash to `channel_id` before the backend
    /// is asked. `Ok(None)` for a node that has not opted in to EVM
    /// vouchers, a channel with neither config, a config that hashes to
    /// another channel, or one the backend does not admit.
    pub(crate) async fn evm_voucher_channel(
        &self,
        channel_id: &[u8; 32],
        presented: Option<BatchChannelConfig>,
        requester: &str,
    ) -> Result<Option<(BatchSettlementDomain, AdmittedEvmVoucherChannel)>, ChannelResolutionError>
    {
        let Some(backend) = self.batch_settlement.as_deref() else {
            return Ok(None);
        };
        let Some(domain) = backend.evm_domain() else {
            return Ok(None);
        };
        let key = format!("{EVM_NAMESPACE}:0x{}", hex::encode(channel_id));
        let recorded = match self.known_batch_channel(&key) {
            Some(JournaledBatchChannel::Evm { config, .. }) => Some(config),
            _ => None,
        };
        let Some(config) = presented.or(recorded) else {
            return Ok(None);
        };
        if evm_batch_channel_id(&domain, &config) != *channel_id {
            return Ok(None);
        }
        let found = self
            .metered_voucher_lookup(
                recorded.is_none(),
                requester,
                backend.evm(channel_id, Some(&config)),
            )
            .await?;
        // As in `verify_voucher`: the signer is read from the backend's
        // config, so it has to be this channel's.
        Ok(found
            .filter(|channel| evm_batch_channel_id(&domain, &channel.config) == *channel_id)
            .map(|channel| (domain, channel)))
    }

    /// As [`Self::evm_voucher_channel`], for the Solana channel account
    /// `channel_account`: every field is on chain, so nothing is presented.
    pub(crate) async fn solana_voucher_channel(
        &self,
        channel_account: &[u8; 32],
        requester: &str,
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        let Some(backend) = self
            .batch_settlement
            .as_deref()
            .filter(|backend| backend.accepts_solana())
        else {
            return Ok(None);
        };
        let key = JournaledBatchChannel::Solana {
            channel_account: *channel_account,
        }
        .channel_key();
        let unseen = self.known_batch_channel(&key).is_none();
        self.metered_voucher_lookup(unseen, requester, backend.solana(channel_account))
            .await
    }

    fn known_batch_channel(&self, key: &str) -> Option<JournaledBatchChannel> {
        self.batch_channels
            .read()
            .expect("batch channels lock poisoned")
            .get(key)
            .copied()
    }

    /// Run a batch-settlement `lookup` for a read, metered as a voucher's is
    /// (issue #613): a channel this gate has no record of is a discovery,
    /// charged before the backend is asked and given back if it found one.
    async fn metered_voucher_lookup<T>(
        &self,
        unseen: bool,
        requester: &str,
        lookup: impl std::future::Future<Output = Result<Option<T>, ChannelResolutionError>>,
    ) -> Result<Option<T>, ChannelResolutionError> {
        let budget = self.lookup_budget();
        let reservation = if unseen {
            Some(
                budget
                    .reserve(requester)
                    .await
                    .map_err(ChannelResolutionError::Budgeted)?,
            )
        } else {
            None
        };
        let found = lookup.await?;
        refund_if_found(budget, reservation, found.is_some());
        Ok(found)
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

    /// The shaper this node's chain lookups are already metered by
    /// ([`crate::lookup_budget`]), for the one other caller ADR 0050 gives it:
    /// `GET /ilp`'s self-description is rate-limited *through this bucket*
    /// rather than through a second mechanism of its own.
    pub(crate) fn lookup_budget(&self) -> &UnresolvableLookupBudget {
        &self.lookup_budget
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
    /// route that charges nothing or that isn't priced at all -- then the
    /// voucher's signature against its channel's voucher signer, then
    /// collateral.
    /// Advances this claim's channel watermark only when the claim is
    /// fully accepted -- a rejected claim, whether stale, underpaying,
    /// unverifiable or unrecordable, leaves the watermark exactly as it
    /// was, so a corrected resubmission is still judged against the same
    /// baseline.
    ///
    /// `async` because resolving a channel is a read against a chain. The watermark lock is deliberately
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
            ClientClaimError::ToonChannel => ClaimIngestRejection::ToonChannel,
            other => ClaimIngestRejection::Malformed(other.to_string()),
        })?;
        self.admit_voucher(claim, price).await
    }

    /// Advance `key` to `watermark`, retaining `signature`, and enqueue the
    /// [`JournalEntry::InboundClaimAccepted`] that records it -- under the
    /// write lock the caller already holds and has just re-checked freshness
    /// under (ADR 0005, issue #605, #686). The one place an acceptance is
    /// made.
    ///
    /// `channel_record`, when given, is journaled in the same batch and
    /// immediately before the acceptance: a batch-settlement channel's
    /// [`JournalEntry::BatchChannelAdmitted`], on its first voucher.
    fn advance_and_enqueue(
        &self,
        watermarks: &mut HashMap<String, LiveClaim>,
        key: &str,
        watermark: Watermark,
        signature: Vec<u8>,
        channel_record: Option<JournalEntry>,
    ) -> Result<DurabilityTicket, ClaimIngestRejection> {
        let previous = watermarks.get(key).cloned();
        watermarks.insert(
            key.to_string(),
            LiveClaim {
                watermark,
                signature: signature.clone(),
            },
        );
        match self.committer.enqueue(PendingAcceptance {
            channel_record,
            entry: JournalEntry::InboundClaimAccepted {
                channel_id: key.to_string(),
                nonce: VOUCHER_WATERMARK_NONCE,
                cumulative_amount: watermark.cumulative_amount,
                signature,
            },
            channel_key: key.to_string(),
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
                    .insert(key.to_string(), previous);
                Ok(ticket)
            }
            Err(CommitterGone) => {
                // The committer thread is gone -- nothing will ever fsync
                // this entry. Undo the advance while still holding the
                // lock (no other claim has seen it) and refuse exactly as
                // a failed append always has.
                restore_watermark(watermarks, key, previous);
                tracing::error!(
                    channel = %key,
                    "refusing a valid claim: the journal committer is gone, so its \
                     acceptance could not be durably recorded"
                );
                Err(ClaimIngestRejection::NotDurable)
            }
        }
    }

    /// [`Self::admit`]'s stages for a voucher (ADR 0074 decisions 3 and 4,
    /// issue #1341): structure, freshness, value, then the signature, then
    /// collateral, then the authoritative re-check, the advance and the
    /// journal.
    ///
    /// * **A chain this node settles on, first.** A gate with no
    ///   [`BatchSettlementChannels`] for the voucher's chain refuses it by
    ///   name, before anything about it is judged.
    /// * **Freshness** is [`validate_voucher`]'s amount-only watermark, keyed
    ///   by the same canonical (blockchain, channel) tuple as a claim's.
    /// * **A byte-identical resend** of the voucher at the watermark, at no
    ///   charge, is answered `Ok` with nothing advanced and nothing
    ///   journaled -- its ticket is already durable, because there is
    ///   nothing new to make durable.
    /// * **The signer** comes from the backend's verified channel, never from
    ///   the voucher; on EVM the gate re-hashes that channel's config and
    ///   refuses one that is not the channel the voucher signs.
    pub(crate) async fn admit_voucher(
        &self,
        claim: ClientClaim,
        price: u64,
    ) -> Result<(ClientClaim, DurabilityTicket), ClaimIngestRejection> {
        let Some(backend) = self.batch_settlement.as_deref() else {
            return Err(ClaimIngestRejection::BatchSettlementNotAccepted);
        };
        let accepted_here = match &claim {
            ClientClaim::EvmVoucher(_) => backend.evm_domain().is_some(),
            ClientClaim::SolanaVoucher(_) => backend.accepts_solana(),
        };
        if !accepted_here {
            return Err(ClaimIngestRejection::BatchSettlementNotAccepted);
        }

        // Decoded before freshness, not after: a resend is recognised by its
        // signature's *bytes*, and decoding is not cryptography.
        let signature = decode_voucher_signature(&claim)?;
        let signature_bytes = signature.to_bytes();
        let key = claim.channel_key();
        let amount = claim.transferred_amount();
        {
            let watermarks = self
                .watermarks
                .read()
                .expect("client claim watermarks lock poisoned");
            let admission = check_voucher_freshness_and_value(
                watermarks.get(&key),
                amount,
                &signature_bytes,
                price,
            )?;
            drop(watermarks);
            if admission == VoucherAdmission::Retransmission {
                return Ok((claim, self.retransmission(&key)));
            }
        }

        let requester = claim.signer_key();
        // A channel this gate has accepted a voucher on before: its record
        // supplies the EVM config a voucher may omit, and its lookup is not
        // a discovery the unresolvable-lookup budget meters.
        let known = self.known_batch_channel(&key);
        let verified = verify_voucher(
            backend,
            &claim,
            signature,
            &requester,
            known,
            self.lookup_budget(),
        )
        .await?;
        // client-edge-spec.md §1.3 step 5, against the backend's current
        // reading rather than a cached floor: on EVM the figure can fall
        // (ADR 0074 decision 5), so there is no lower bound to cache.
        if amount > verified.max_cumulative {
            return Err(ClaimIngestRejection::Undercollateralized {
                claimed: amount,
                deposited: verified.max_cumulative,
            });
        }

        let mut watermarks = self
            .watermarks
            .write()
            .expect("client claim watermarks lock poisoned");
        // Re-read, as `admit` does: a concurrent voucher on this channel may
        // have advanced it while the backend was being asked.
        let admission = check_voucher_freshness_and_value(
            watermarks.get(&key),
            amount,
            &signature_bytes,
            price,
        )?;
        if admission == VoucherAdmission::Retransmission {
            drop(watermarks);
            return Ok((claim, self.retransmission(&key)));
        }
        // The channel's first voucher -- or its first since a rollback or a
        // failed batch emptied the watermark -- records the channel with it,
        // so it can still be landed on after a restart.
        let channel_record = (!watermarks.contains_key(&key)).then(|| verified.channel.to_entry());
        let ticket = self.advance_and_enqueue(
            &mut watermarks,
            &key,
            advance_voucher_watermark(amount),
            signature_bytes,
            channel_record,
        )?;
        self.voucher_signers
            .write()
            .expect("voucher signers lock poisoned")
            .insert(key.clone(), verified.signer);
        self.batch_channels
            .write()
            .expect("batch channels lock poisoned")
            .insert(key, verified.channel);
        drop(watermarks);
        Ok((claim, ticket))
    }

    /// Answer a voucher resent at its watermark (ADR 0074 decision 3): no
    /// advance, no journal entry, and a ticket that is already durable.
    ///
    /// It also forgets `key`'s rollback record (issue #1012). The resend's
    /// watermark is the one the *original* voucher set, so a rollback of
    /// the resend's packet would name exactly the original's watermark and
    /// undo the original's acceptance -- letting a client that paid once
    /// for a carried packet get that payment back by resending the voucher
    /// with a free forwarded packet it can make fail. Forgetting the record
    /// makes that rollback a logged no-op; the only cost is that an
    /// original still in flight at the same moment can no longer be rolled
    /// back, which leaves its payer charged rather than this connector
    /// unpaid.
    fn retransmission(&self, key: &str) -> DurabilityTicket {
        self.previous_watermarks
            .write()
            .expect("client claim previous-watermark lock poisoned")
            .remove(key);
        DurabilityTicket::already_durable()
    }

    /// Undo [`Self::admit`]'s watermark advance for the voucher that reached
    /// exactly `cumulative_amount` on `channel_key` -- because the
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
    /// * a later voucher has since advanced `channel_key` past
    ///   `cumulative_amount`. That claim's own admission is
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
        cumulative_amount: u128,
    ) -> Result<(), ClaimIngestRejection> {
        let key = canonical_channel_key(channel_key);
        let ticket = {
            let mut watermarks = self
                .watermarks
                .write()
                .expect("client claim watermarks lock poisoned");
            let current = watermarks.get(&key).cloned();
            if current.as_ref().map(|record| record.watermark)
                != Some(Watermark { cumulative_amount })
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
                    nonce: VOUCHER_WATERMARK_NONCE,
                    cumulative_amount: record.watermark.cumulative_amount,
                },
                None => JournalEntry::InboundClaimWatermarkReset {
                    channel_id: key.clone(),
                },
            };
            restore_watermark(&mut watermarks, &key, previous.clone());
            match self.committer.enqueue(PendingAcceptance {
                channel_record: None,
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
    /// Written first, in the same batch: a batch-settlement channel's
    /// record, enqueued with its first voucher (ADR 0074). Undone by
    /// nothing -- see `ClientClaimGate::batch_channels` for why an orphaned
    /// record is harmless.
    channel_record: Option<JournalEntry>,
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
    /// A ticket for an admission that recorded nothing -- a voucher resent at
    /// its watermark (ADR 0074 decision 3) -- and so has nothing to wait
    /// for.
    fn already_durable() -> DurabilityTicket {
        let (durable_tx, durable_rx) = tokio::sync::oneshot::channel();
        let _ = durable_tx.send(Ok(()));
        DurabilityTicket {
            durable: durable_rx,
        }
    }

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
/// place would burn every refused voucher -- the client's perfectly good
/// voucher, resubmitted as [`ClaimIngestRejection::NotDurable`] invites,
/// would bounce off its own ghost as `AmountNotAdvancing`. So the committer
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
            .flat_map(|(pending, _)| {
                pending
                    .channel_record
                    .iter()
                    .chain(std::iter::once(&pending.entry))
                    .cloned()
            })
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

/// Rebuild the per-channel watermarks a journal records, folding every
/// [`JournalEntry::InboundClaimAccepted`] in it -- the client edge's own
/// half of the replay `connector_runtime::ClaimBook::set_journal` does for
/// the peer semantics, over the same entry.
///
/// `max` rather than last-wins: entries are appended in accepted order and
/// each accepted voucher strictly advances, so the two agree on any journal
/// this gate itself wrote. They
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
/// voucher its client already spent.
///
/// The `max` above is what makes the merge sound in the one case where a
/// pre-#643 journal holds *several* spellings of one channel. They collapse
/// into one key at the highest amount either of them ever reached, so the
/// upgrade can only ever tighten what this gate will accept next, never
/// loosen it.
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
/// to, without needing to trust the rollback entry's own amount as
/// anything more than a diagnostic. A reset clears the whole stack: it
/// erases the channel outright rather than restoring an earlier claim.
fn replay_watermarks(entries: &[JournalEntry]) -> HashMap<String, LiveClaim> {
    let mut history: HashMap<String, Vec<LiveClaim>> = HashMap::new();
    for entry in entries {
        match entry {
            JournalEntry::InboundClaimAccepted {
                channel_id,
                cumulative_amount,
                signature,
                ..
            } => {
                let stack = history
                    .entry(canonical_channel_key(channel_id))
                    .or_default();
                let previous = stack
                    .last()
                    .map_or(0, |record| record.watermark.cumulative_amount);
                stack.push(LiveClaim {
                    watermark: Watermark {
                        cumulative_amount: previous.max(*cumulative_amount),
                    },
                    signature: signature.clone(),
                });
            }
            // Issue #977: a reset -- written by the rollback of a channel's
            // first voucher, and by the `TokenNetwork` sweep an older build
            // ran -- must be able to erase what was folded in *before* it in
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

/// client-edge-spec.md §1.3 steps 2 and 3 for a voucher, against the
/// channel's current record: [`validate_voucher`], with its refusals mapped
/// onto this gate's taxonomy. Pure, and cheap enough to run twice -- once up
/// front, and once more under the write lock immediately before the
/// watermark advances, which is the authoritative one.
fn check_voucher_freshness_and_value(
    current: Option<&LiveClaim>,
    amount: u128,
    signature: &[u8],
    price: u64,
) -> Result<VoucherAdmission, ClaimIngestRejection> {
    let watermark = current.map(|record| VoucherWatermark {
        cumulative_amount: record.watermark.cumulative_amount,
        signature: &record.signature,
    });
    validate_voucher(watermark, amount, signature, price).map_err(|error| match error {
        ClaimError::AmountNotAdvancing { .. } => ClaimIngestRejection::AmountNotAdvancing,
        ClaimError::Underpayment { advanced, price } => {
            ClaimIngestRejection::Underpayment { advanced, price }
        }
    })
}

/// A voucher's signature as the bytes its scheme defines: `0x` + 130 hex on
/// EVM (65 bytes, `v` as the wallet wrote it), base58 of 64 bytes on
/// Solana. Anything else is a structural failure -- the parser checked the
/// alphabet, this checks the length.
fn decode_voucher_signature(claim: &ClientClaim) -> Result<VoucherSignature, ClaimIngestRejection> {
    match claim {
        ClientClaim::EvmVoucher(voucher) => decode_hex_bytes::<65>(&voucher.signature)
            .map(VoucherSignature::Evm)
            .ok_or_else(|| {
                ClaimIngestRejection::Malformed(
                    "a voucher's 'signature' must be 65 bytes of hex".to_string(),
                )
            }),
        ClientClaim::SolanaVoucher(voucher) => decode_base58_bytes::<64>(&voucher.signature)
            .map(VoucherSignature::Solana)
            .ok_or_else(|| {
                ClaimIngestRejection::Malformed(
                    "a Solana voucher's 'signature' must be base58 of 64 bytes".to_string(),
                )
            }),
    }
}

/// An EVM voucher's `channelConfig`, decoded into the struct
/// `connector_signer` hashes. The parser has already checked every field's
/// shape, so a decode that fails here is still reported, not unwrapped.
pub(crate) fn decode_evm_channel_config(
    config: &EvmVoucherChannelConfig,
) -> Result<BatchChannelConfig, ClaimIngestRejection> {
    let address = |value: &str| {
        decode_hex_bytes::<20>(value).ok_or_else(|| {
            ClaimIngestRejection::Malformed(format!("'{value}' is not a 20-byte address"))
        })
    };
    Ok(BatchChannelConfig {
        payer: address(&config.payer)?,
        payer_authorizer: address(&config.payer_authorizer)?,
        receiver: address(&config.receiver)?,
        receiver_authorizer: address(&config.receiver_authorizer)?,
        token: address(&config.token)?,
        withdraw_delay: config.withdraw_delay,
        salt: decode_hex_bytes::<32>(&config.salt).ok_or_else(|| {
            ClaimIngestRejection::Malformed("'channelConfig.salt' is not 32 bytes".to_string())
        })?,
    })
}

/// What survives [`verify_voucher`]: the channel's collateral bound for
/// step 5, and the channel as the journal records it.
struct VerifiedVoucher {
    max_cumulative: u128,
    channel: JournaledBatchChannel,
    /// The key the voucher verified against, as the chain records it.
    signer: VoucherSigner,
}

/// client-edge-spec.md §1.3 step 4 for a voucher (ADR 0074 decision 4):
/// resolve its channel through `backend`, take the signer from what the
/// backend verified -- never from the voucher -- and check the signature.
///
/// `known` is this gate's record of the channel, if it has accepted a
/// voucher on it before: on EVM its config is presented for a voucher that
/// carries none. A channel with no record is a discovery, and its lookup is
/// metered against `budget` (issue #613): charged before the backend is
/// asked, and given back if the backend found a channel.
async fn verify_voucher(
    backend: &dyn BatchSettlementChannels,
    claim: &ClientClaim,
    signature: VoucherSignature,
    requester: &str,
    known: Option<JournaledBatchChannel>,
    budget: &UnresolvableLookupBudget,
) -> Result<VerifiedVoucher, ClaimIngestRejection> {
    let refuse_resolution = |error: ChannelResolutionError| {
        if let ChannelResolutionError::LookupFailed(failure) = &error {
            tracing::warn!(
                requester = %requester,
                error = %failure,
                "refusing a voucher: could not resolve its batch-settlement channel"
            );
        }
        resolution_refusal(error)
    };
    match (claim, signature) {
        (ClientClaim::EvmVoucher(voucher), VoucherSignature::Evm(signature)) => {
            let domain = backend
                .evm_domain()
                .ok_or(ClaimIngestRejection::BatchSettlementNotAccepted)?;
            let Some(channel_id) = decode_hex_bytes::<32>(&voucher.channel_id) else {
                return Err(ClaimIngestRejection::UnknownChannel);
            };
            let recorded = match known {
                Some(JournaledBatchChannel::Evm { config, .. }) => Some(config),
                _ => None,
            };
            let presented = voucher
                .channel_config
                .as_ref()
                .map(decode_evm_channel_config)
                .transpose()?
                .or(recorded);
            // ADR 0074 decision 2: recompute `getChannelId` and refuse a
            // mismatch -- before asking the backend anything about it.
            if presented.is_some_and(|config| evm_batch_channel_id(&domain, &config) != channel_id)
            {
                return Err(ClaimIngestRejection::VoucherChannelConfigMismatch);
            }
            let reservation = reserve_voucher_lookup(budget, known.is_none(), requester).await?;
            let found = backend
                .evm(&channel_id, presented.as_ref())
                .await
                .map_err(refuse_resolution)?;
            refund_if_found(budget, reservation, found.is_some());
            let channel = found.ok_or(ClaimIngestRejection::UnknownChannel)?;
            // The backend's config is re-hashed too: the signer is read from
            // it, so it has to be this channel's, whoever supplied it.
            if evm_batch_channel_id(&domain, &channel.config) != channel_id {
                return Err(ClaimIngestRejection::VoucherChannelConfigMismatch);
            }
            let signer = evm_voucher_signer(&channel.config);
            if verify_evm_voucher(
                &domain,
                &channel_id,
                voucher.max_claimable_amount,
                &signature,
                &signer,
            ) {
                Ok(VerifiedVoucher {
                    max_cumulative: channel.max_cumulative,
                    channel: JournaledBatchChannel::Evm {
                        channel_id,
                        config: channel.config,
                    },
                    signer: VoucherSigner::Evm(signer),
                })
            } else {
                Err(ClaimIngestRejection::SignatureInvalid)
            }
        }
        (ClientClaim::SolanaVoucher(voucher), VoucherSignature::Solana(signature)) => {
            let Some(channel_account) = decode_base58_bytes::<32>(&voucher.channel_id) else {
                return Err(ClaimIngestRejection::UnknownChannel);
            };
            let reservation = reserve_voucher_lookup(budget, known.is_none(), requester).await?;
            let found = backend
                .solana(&channel_account)
                .await
                .map_err(refuse_resolution)?;
            refund_if_found(budget, reservation, found.is_some());
            let channel = found.ok_or(ClaimIngestRejection::UnknownChannel)?;
            // `expiresAt` is zero: the parser refused anything else (ADR
            // 0074 decision 3), and it is still part of the signed bytes.
            if verify_solana_voucher(
                &channel_account,
                voucher.max_claimable_amount,
                0,
                &signature,
                &channel.authorized_signer,
            ) {
                Ok(VerifiedVoucher {
                    max_cumulative: u128::from(channel.max_cumulative),
                    channel: JournaledBatchChannel::Solana { channel_account },
                    signer: VoucherSigner::Solana(channel.authorized_signer),
                })
            } else {
                Err(ClaimIngestRejection::SignatureInvalid)
            }
        }
        _ => Err(ClaimIngestRejection::BatchSettlementNotAccepted),
    }
}

/// Charge a voucher lookup for a channel this gate has no record of against
/// the unresolvable-lookup budget (issue #613), waiting for its slot if the
/// drain is in arrears. `Ok(None)` for a lookup on a known channel, which is
/// not a discovery and is never charged.
async fn reserve_voucher_lookup(
    budget: &UnresolvableLookupBudget,
    unseen: bool,
    requester: &str,
) -> Result<Option<LookupReservation>, ClaimIngestRejection> {
    if !unseen {
        return Ok(None);
    }
    match budget.reserve(requester).await {
        Ok(reservation) => Ok(Some(reservation)),
        Err(exhausted) => {
            tracing::warn!(
                bound = exhausted.bound.as_str(),
                allowance = exhausted.allowance,
                signer = %requester,
                "declining to look up an unknown batch-settlement channel: this node's \
                 discovery drain is saturated and its queue is full"
            );
            Err(resolution_refusal(ChannelResolutionError::Budgeted(
                exhausted,
            )))
        }
    }
}

/// Give a voucher lookup's slot back if it found a channel: only lookups
/// that found nothing, or failed, leave a mark (issue #613).
fn refund_if_found(
    budget: &UnresolvableLookupBudget,
    reservation: Option<LookupReservation>,
    found: bool,
) {
    if let (Some(reservation), true) = (reservation, found) {
        budget.refund(reservation);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        self, channel_key, config_salted, gate_over, signed_voucher, signed_voucher_on,
        FakeBatchSettlement,
    };
    use connector_runtime::{FileJournal, InMemoryJournal};

    fn backend() -> Arc<FakeBatchSettlement> {
        Arc::new(FakeBatchSettlement::new(1_000_000))
    }

    fn gate() -> ClientClaimGate {
        gate_over(Arc::new(InMemoryJournal::new()), &backend())
    }

    fn file_gate(path: &std::path::Path) -> ClientClaimGate {
        gate_over(
            Arc::new(FileJournal::open(path).expect("open the journal file")),
            &backend(),
        )
    }

    fn second_channel_key() -> String {
        format!(
            "evm:0x{}",
            hex::encode(test_support::channel_id_of(&config_salted(0x77)))
        )
    }

    fn at(amount: u128) -> Option<Watermark> {
        Some(Watermark {
            cumulative_amount: amount,
        })
    }

    // -- Refused by name --

    /// ADR 0075 decision 8 (issue #1384): the retired `toon-channel` claim
    /// -- no `scheme`, or `scheme: "toon-channel"` -- is refused by name,
    /// never parsed, never looked up, and never reported as malformed.
    #[tokio::test]
    async fn a_toon_channel_claim_is_refused_by_name() {
        let backend = backend();
        let gate = gate_over(Arc::new(InMemoryJournal::new()), &backend);
        let unschemed = test_support::toon_channel_claim();
        let explicit = unschemed.replace(
            r#""blockchain":"evm""#,
            r#""blockchain":"evm","scheme":"toon-channel""#,
        );
        assert_ne!(explicit, unschemed);
        for claim in [unschemed, explicit] {
            assert_eq!(
                gate.ingest(&claim, 0).await,
                Err(ClaimIngestRejection::ToonChannel)
            );
        }
        assert_eq!(
            backend.lookups(),
            0,
            "a refusal by name asks the chain nothing"
        );
        let message = ClaimIngestRejection::ToonChannel.message();
        assert!(message.contains("toon-channel"), "{message}");
        assert!(message.contains("ADR 0075"), "{message}");
        assert!(!message.contains("structurally invalid"), "{message}");
    }

    #[tokio::test]
    async fn a_mina_claim_is_refused_distinguishably_from_malformed() {
        let mina = signed_voucher(100).replace(r#""blockchain":"evm""#, r#""blockchain":"mina""#);
        assert_eq!(
            gate().ingest(&mina, 0).await,
            Err(ClaimIngestRejection::Mina)
        );
    }

    #[tokio::test]
    async fn a_structurally_invalid_claim_is_refused_as_malformed() {
        assert!(matches!(
            gate()
                .ingest(r#"{"version":"1.0","scheme":"batch-settlement"}"#, 0)
                .await,
            Err(ClaimIngestRejection::Malformed(_))
        ));
    }

    // -- Freshness and canonical keys --

    #[tokio::test]
    async fn a_voucher_advances_the_watermark_and_an_older_one_is_refused() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0)
            .await
            .expect("accepted");
        assert_eq!(gate.watermark(&channel_key()), at(100));
        assert_eq!(
            gate.ingest(&signed_voucher(50), 0).await,
            Err(ClaimIngestRejection::AmountNotAdvancing)
        );
        assert_eq!(gate.watermark(&channel_key()), at(100));
    }

    /// Issue #643: hex has no case, so the same voucher with its
    /// `channelId` recased names the same channel and the same watermark.
    #[tokio::test]
    async fn a_recased_channel_id_is_the_same_watermark() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0)
            .await
            .expect("accepted");
        let id = hex::encode(test_support::channel_id());
        let recased = signed_voucher(50).replace(&id, &id.to_ascii_uppercase());
        assert_ne!(recased, signed_voucher(50));
        assert_eq!(
            gate.ingest(&recased, 0).await,
            Err(ClaimIngestRejection::AmountNotAdvancing),
            "a recased channel id must not open a second, empty watermark"
        );
        assert_eq!(
            gate.watermark(
                &channel_key()
                    .to_ascii_uppercase()
                    .replace("EVM:0X", "evm:0x")
            ),
            at(100)
        );
    }

    #[tokio::test]
    async fn two_channels_keep_independent_watermarks() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0).await.expect("first");
        gate.ingest(&signed_voucher_on(&config_salted(0x77), 5), 0)
            .await
            .expect("an independent channel starts from zero");
        assert_eq!(gate.watermark(&channel_key()), at(100));
        assert_eq!(gate.watermark(&second_channel_key()), at(5));
    }

    // -- Durability (issues #605, #686) --

    /// A [`Journal`] whose `append` always fails -- a full or read-only
    /// disk behaves exactly like this.
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

    /// A [`Journal`] whose `read_all` fails.
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

    /// A [`Journal`] that fails a set number of appends and then recovers.
    struct RecoveringJournal {
        failures_left: std::sync::atomic::AtomicU32,
        inner: InMemoryJournal,
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

    /// Issue #605's own failure: a voucher spent before a restart is still
    /// spent after it.
    #[tokio::test]
    async fn a_voucher_accepted_before_a_restart_is_refused_after_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client-edge-claims.log");
        file_gate(&path)
            .ingest(&signed_voucher(100), 0)
            .await
            .expect("accepted before the restart");

        let restarted = file_gate(&path);
        assert_eq!(restarted.watermark(&channel_key()), at(100));
        assert_eq!(
            restarted.ingest(&signed_voucher(100), 5).await,
            Err(ClaimIngestRejection::Underpayment {
                advanced: 0,
                price: 5
            }),
            "resent after a restart, the spent voucher buys nothing"
        );
        assert_eq!(
            restarted.ingest(&signed_voucher(90), 0).await,
            Err(ClaimIngestRejection::AmountNotAdvancing)
        );
    }

    #[tokio::test]
    async fn a_voucher_that_cannot_be_journaled_is_refused_and_advances_nothing() {
        let gate = gate_over(Arc::new(UnwritableJournal), &backend());
        assert_eq!(
            gate.ingest(&signed_voucher(100), 0).await,
            Err(ClaimIngestRejection::NotDurable)
        );
        assert_eq!(gate.watermark(&channel_key()), None);
    }

    /// The half of `NotDurable`'s contract the group commit must not lose:
    /// the same voucher resubmitted once the journal is writable again is
    /// still good.
    #[tokio::test]
    async fn a_failed_batch_rolls_back_so_the_same_voucher_is_good_once_the_journal_recovers() {
        let gate = gate_over(
            Arc::new(RecoveringJournal {
                failures_left: std::sync::atomic::AtomicU32::new(1),
                inner: InMemoryJournal::new(),
            }),
            &backend(),
        );
        assert_eq!(
            gate.ingest(&signed_voucher(100), 0).await,
            Err(ClaimIngestRejection::NotDurable)
        );
        assert_eq!(gate.watermark(&channel_key()), None);
        gate.ingest(&signed_voucher(100), 0)
            .await
            .expect("the identical voucher, resubmitted after recovery, is still good");
        assert_eq!(gate.watermark(&channel_key()), at(100));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn group_committed_acceptances_replay_to_the_watermarks_the_live_gate_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client-edge-claims.log");
        let (live_first, live_second) = {
            let gate = Arc::new(file_gate(&path));
            let mut tasks = Vec::new();
            for config in [test_support::config(), config_salted(0x77)] {
                let gate = gate.clone();
                tasks.push(tokio::spawn(async move {
                    for step in 1..=25u128 {
                        gate.ingest(&signed_voucher_on(&config, step * 10), 0)
                            .await
                            .expect("strictly advancing vouchers are accepted");
                    }
                }));
            }
            for task in tasks {
                task.await.expect("ingest task");
            }
            (
                gate.watermark(&channel_key()),
                gate.watermark(&second_channel_key()),
            )
        };
        let restored = file_gate(&path);
        assert_eq!(restored.watermark(&channel_key()), live_first);
        assert_eq!(restored.watermark(&second_channel_key()), live_second);
        assert_eq!(live_first, at(250));
    }

    /// The journal's entry order is the acceptance order, and a channel's
    /// first voucher journals the channel itself immediately before it.
    #[tokio::test]
    async fn the_journal_records_acceptances_in_acceptance_order() {
        let journal = Arc::new(InMemoryJournal::new());
        let gate = gate_over(journal.clone(), &backend());
        for amount in [100, 200, 300u128] {
            gate.ingest(&signed_voucher(amount), 0)
                .await
                .expect("accepted");
        }
        let entries = journal.read_all().unwrap();
        assert!(matches!(
            entries[0],
            JournalEntry::BatchChannelAdmitted { .. }
        ));
        let amounts: Vec<u128> = entries[1..]
            .iter()
            .map(|entry| match entry {
                JournalEntry::InboundClaimAccepted {
                    cumulative_amount,
                    nonce,
                    ..
                } => {
                    assert_eq!(*nonce, VOUCHER_WATERMARK_NONCE);
                    *cumulative_amount
                }
                other => panic!("unexpected entry {other:?}"),
            })
            .collect();
        assert_eq!(amounts, vec![100, 200, 300]);
    }

    #[test]
    fn a_not_durable_refusal_does_not_blame_the_claim() {
        let message = ClaimIngestRejection::NotDurable.message();
        assert!(message.contains("durably record"), "{message}");
    }

    #[test]
    fn an_unreadable_journal_refuses_to_produce_a_gate() {
        assert!(matches!(
            ClientClaimGate::restore(Arc::new(UnreadableJournal)),
            Err(JournalError::Io(_))
        ));
    }

    #[tokio::test]
    async fn a_corrupt_journal_line_refuses_to_produce_a_gate() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client-edge-claims.log");
        file_gate(&path)
            .ingest(&signed_voucher(100), 0)
            .await
            .expect("one good entry");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "this is not a journal entry").unwrap();
        drop(file);
        assert!(matches!(
            ClientClaimGate::restore(Arc::new(FileJournal::open(&path).expect("open"))),
            Err(JournalError::Corrupt(_))
        ));
    }

    // -- Replay (pure) --

    fn accepted(channel: &str, amount: u128, signature: u8) -> JournalEntry {
        JournalEntry::InboundClaimAccepted {
            channel_id: channel.to_string(),
            nonce: VOUCHER_WATERMARK_NONCE,
            cumulative_amount: amount,
            signature: vec![signature; 65],
        }
    }

    #[test]
    fn replay_never_recovers_a_watermark_lower_than_one_already_recorded() {
        let key = channel_key();
        let replayed = replay_watermarks(&[accepted(&key, 300, 1), accepted(&key, 100, 2)]);
        assert_eq!(
            replayed[&key].watermark,
            Watermark {
                cumulative_amount: 300
            }
        );
    }

    #[test]
    fn replay_ignores_entries_that_are_not_accepted_inbound_claims() {
        let replayed = replay_watermarks(&[JournalEntry::OutboundVoucherSigned {
            channel_id: channel_key(),
            cumulative_amount: 500,
        }]);
        assert!(replayed.is_empty());
    }

    #[test]
    fn replay_clears_prior_accumulation_on_a_reset_entry() {
        let key = channel_key();
        let replayed = replay_watermarks(&[
            accepted(&key, 300, 1),
            JournalEntry::InboundClaimWatermarkReset {
                channel_id: key.clone(),
            },
            accepted(&key, 20, 2),
        ]);
        assert_eq!(
            replayed[&key].watermark,
            Watermark {
                cumulative_amount: 20
            }
        );
        let reset_only = replay_watermarks(&[
            accepted(&key, 300, 1),
            JournalEntry::InboundClaimWatermarkReset { channel_id: key },
        ]);
        assert!(reset_only.is_empty());
    }

    #[test]
    fn replay_puts_a_rolled_back_channel_back_to_its_prior_amount_and_signature() {
        let key = channel_key();
        let replayed = replay_watermarks(&[
            accepted(&key, 100, 1),
            accepted(&key, 200, 2),
            JournalEntry::InboundClaimRolledBack {
                channel_id: key.clone(),
                nonce: VOUCHER_WATERMARK_NONCE,
                cumulative_amount: 100,
            },
        ]);
        assert_eq!(
            replayed[&key].watermark,
            Watermark {
                cumulative_amount: 100
            }
        );
        assert_eq!(replayed[&key].signature, vec![1; 65]);

        let advanced_again = replay_watermarks(&[
            accepted(&key, 100, 1),
            accepted(&key, 200, 2),
            JournalEntry::InboundClaimRolledBack {
                channel_id: key.clone(),
                nonce: VOUCHER_WATERMARK_NONCE,
                cumulative_amount: 100,
            },
            accepted(&key, 150, 3),
        ]);
        assert_eq!(
            advanced_again[&key].watermark,
            Watermark {
                cumulative_amount: 150
            }
        );
    }

    #[test]
    fn a_journal_entry_in_no_known_namespace_folds_under_its_own_key() {
        let replayed = replay_watermarks(&[accepted("channel-a", 7, 1)]);
        assert_eq!(
            replayed["channel-a"].watermark,
            Watermark {
                cumulative_amount: 7
            }
        );
    }

    // -- Rollback (issue #1012) --

    #[tokio::test]
    async fn a_rolled_back_voucher_is_forgotten_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client-edge-claims.log");
        {
            let gate = file_gate(&path);
            gate.ingest(&signed_voucher(100), 0).await.expect("first");
            gate.ingest(&signed_voucher(200), 0).await.expect("second");
            gate.roll_back(&channel_key(), 200)
                .await
                .expect("rolled back");
            assert_eq!(gate.watermark(&channel_key()), at(100));
        }
        let restarted = file_gate(&path);
        assert_eq!(restarted.watermark(&channel_key()), at(100));
        restarted
            .ingest(&signed_voucher(200), 0)
            .await
            .expect("the rolled-back voucher is good again");
    }

    #[tokio::test]
    async fn rolling_back_a_superseded_voucher_leaves_the_later_one_alone() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0).await.expect("first");
        gate.ingest(&signed_voucher(200), 0).await.expect("second");
        gate.roll_back(&channel_key(), 100).await.expect("a no-op");
        assert_eq!(gate.watermark(&channel_key()), at(200));
    }

    #[tokio::test]
    async fn rolling_back_a_channels_first_voucher_empties_it() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0).await.expect("first");
        gate.roll_back(&channel_key(), 100)
            .await
            .expect("rolled back");
        assert_eq!(gate.watermark(&channel_key()), None);
    }

    #[tokio::test]
    async fn an_accepted_voucher_is_readable_back_with_its_signature() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0)
            .await
            .expect("accepted");
        let (amount, signature) = gate
            .latest_inbound_claim(&channel_key())
            .expect("the latest voucher is held");
        assert_eq!(amount, 100);
        assert_eq!(signature.len(), 65);
        assert_eq!(gate.batch_channels().len(), 1);
    }

    // -- Payees --

    #[tokio::test]
    async fn a_destination_with_no_payee_is_paid_nothing() {
        let gate = gate().with_payout_ledger(
            crate::outbound_ledger::test_ledger_paying(test_support::address_of(
                &test_support::authorizer(),
            ))
            .await,
        );
        gate.open_session("g.toon.nobody", 1);
        assert!(gate
            .credit_session_payout("g.toon.nobody", 1, None, &[1; 32], 100)
            .await
            .is_none());
    }

    /// Issue #1396: a payee belongs to the session that proved it. Another
    /// session at the same address -- open before it or after -- neither
    /// sees it nor changes it; closing a session clears only its own; and a
    /// session that is not open learns nothing.
    #[test]
    fn a_session_payee_is_scoped_to_the_session_that_proved_it() {
        let gate = gate();
        let (first, second) = (
            VoucherSigner::Evm([0x0a; 20]),
            VoucherSigner::Evm([0x0b; 20]),
        );

        gate.open_session("g.toon.agent", 1);
        assert!(gate.record_session_payee("g.toon.agent", 1, first));
        assert!(
            !gate.record_session_payee("g.toon.agent", 1, first),
            "proving the same key again changes nothing"
        );

        gate.open_session("g.toon.agent", 2);
        assert_eq!(
            gate.session_payee("g.toon.agent", 2),
            None,
            "a new session at the same address starts with no payee"
        );
        assert!(gate.record_session_payee("g.toon.agent", 2, second));
        assert_eq!(gate.session_payee("g.toon.agent", 1), Some(first));
        assert_eq!(gate.session_payee("g.toon.agent", 2), Some(second));
        assert_eq!(
            gate.session_payee("g.toon.other", 1),
            None,
            "a generation resolves only at the address it was bound at"
        );

        // The superseded session closing leaves the newer one's payee intact.
        gate.close_session("g.toon.agent", 1);
        assert_eq!(gate.session_payee("g.toon.agent", 1), None);
        assert_eq!(gate.session_payee("g.toon.agent", 2), Some(second));

        gate.close_session("g.toon.agent", 2);
        assert_eq!(gate.session_payee("g.toon.agent", 2), None);
        assert!(
            !gate.record_session_payee("g.toon.agent", 2, second),
            "a closed session learns nothing"
        );
        assert_eq!(gate.session_payee("g.toon.agent", 2), None);
    }

    /// An accepted voucher teaches the gate its channel's voucher signer as
    /// the chain records it: the payee a session paying on it is paid at.
    #[tokio::test]
    async fn an_accepted_voucher_records_its_channels_voucher_signer() {
        let gate = gate();
        gate.ingest(&signed_voucher(100), 0)
            .await
            .expect("accepted");
        assert_eq!(
            gate.voucher_signer(&channel_key()),
            Some(VoucherSigner::Evm(test_support::address_of(
                &test_support::authorizer()
            )))
        );
    }
}
