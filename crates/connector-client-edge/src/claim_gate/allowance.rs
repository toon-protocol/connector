//! The zk edge's half of the claim gate (ADR 0080, issue #1522): admitting
//! an allowance, and admitting allowance claims against it under a voucher's
//! rules.
//!
//! **Admitting an allowance** is cheapest refusal first, and nothing is
//! written until every check has passed (decision 3, as amended): the
//! request's structure, a tier this edge publishes, an expiry within the
//! allowance lifetime, the proof being about this edge's own state key, a
//! request time inside zk_toon's window of this edge's clock, the four
//! refusals a subsidised proof adds (decision 14: not the smallest tier, not
//! the current period, a closed budget, a per-period nullifier already
//! seen), the binding tag -- and then the two refusals this gate's
//! own records can make, a nullifier it already spent and the open-budget
//! cap, before the three that cost a vault read: a known root, a nullifier
//! unused on chain, and the proof. Every check that needs no vault runs
//! before any that does, so a replayed or mis-bound proof costs nothing.
//!
//! A granted allowance is evidence before it is an answer (decision 11): its
//! grant record is appended to the evidence file and fsynced, then
//! [`JournalEntry::AllowanceGranted`] -- the spent nullifier, the record's
//! hash and the terms -- is appended to the journal and fsynced, and only
//! then is the grant returned. A grant the journal refuses is cut back out
//! of the evidence file before the refusal is answered, so a retry writes
//! the only grant record for its nullifier, and the challenger never reads
//! a grant this edge does not remember. A nullifier is reserved, under the ledger's
//! lock, from the moment its local checks pass, so two requests spending one
//! note at once open one allowance.
//!
//! **An allowance claim** is held to [`validate_allowance_claim`]: strictly
//! above the watermark, an advance covering the charge, at most the budget.
//! Its watermark lives beside every voucher's, under the key
//! `zk-note:<allowance id>`, so a reject rolls it back exactly as a
//! voucher's (`ClientClaimGate::roll_back`).
//!
//! **Closing an allowance** (decision 4, #1523) happens on its session
//! key's signed request, at its expiry (the sweep), or when a claim reaches
//! its budget, and stops new claims at once. Its start is journaled then
//! ([`JournalEntry::AllowanceClosing`]), before a signed close is answered,
//! so a close is final: a restart brings the allowance back closing, and
//! the boot sweep finishes it. Every packet admitted on an
//! allowance is counted in flight from before its watermark moves until the
//! carriage drops its [`InFlightForward`] -- after any rollback -- so the
//! charge Δ is read only once nothing can move the watermark again. Then,
//! each durable before the next: Δ is journaled
//! ([`JournalEntry::AllowanceClosed`], once), the close record goes to the
//! evidence file, and the successor -- zk_toon's `charge` and
//! `next_anchor` -- is signed by the state key, which only
//! `connector-signer`'s [`ZkStateSigner`] holds, and journaled
//! ([`JournalEntry::AllowanceSuccessorSigned`]). A successor fetch only
//! reads what that left.
//!
//! **A subsidised allowance** (decision 14, #1581) is admitted under the same
//! verification key, and is paid from the subsidy budget rather than by the
//! note. A grant reserves the full tier against the budget of the period it
//! was granted in; the close replaces that reservation with Δ, and the
//! remainder returns to that period's tally -- which is of no use to a later
//! period, whose tally starts from the whole budget. The successor of a
//! subsidised allowance carries the note's old balance unchanged: the note is
//! charged 0, and Δ is the budget's. The per-period nullifier, the period and
//! the reservation are journaled with the grant, so they survive a restart.
//!
//! Nothing here logs or reports a proof, a nullifier or a session key: a
//! refusal names its rule, and a log line names the allowance by its opaque
//! id.

use std::time::Duration;

use connector_domain::client_claim::{AllowanceClaim, AllowanceId};
use connector_domain::AllowanceSubsidy;
use connector_domain::{allowance_close_charge, validate_allowance_claim};
use connector_runtime::Clock;
use connector_zk_note::evidence::{decode_error_message, evidence_hash, Close, Grant, Record};
use connector_zk_note::note::policy;
use connector_zk_note::note::state::StateMessage;
use connector_zk_note::request::{expected_tag, RequestProof};
use connector_zk_note::session::{
    signature_from_be_bytes, verify_allowance_claim, verify_allowance_close,
};
use connector_zk_note::successor::successor_state;
use connector_zk_note::tiers::Tiers;
use connector_zk_note::{
    commitment_from_le_bytes, decompress, field_from_le_bytes, field_to_le_bytes,
    scalar_from_le_bytes, scalar_to_le_bytes, Commitment, CompressedPubKey, Field, Signature,
};

use super::*;
use connector_signer::ZkStateSigner;

use crate::evidence_file::EvidenceFile;
use crate::zk_note::ZkNoteVault;

/// What makes a gate a zk edge: its vault, its evidence file, its state
/// key, and the terms the `[zk_note]` table declares.
pub struct ZkEdge {
    pub vault: Arc<dyn ZkNoteVault>,
    pub evidence: EvidenceFile,
    /// The key this edge signs successor states with, held by
    /// `connector-signer`. A request proof must be about its public half,
    /// whose x is the `edge` the binding tag is computed with.
    pub state_signer: ZkStateSigner,
    pub tiers: Tiers,
    pub allowance_lifetime: Duration,
    /// The most the budgets of all open allowances may sum to.
    pub open_budget_cap: u64,
    /// The subsidy terms (decision 14): `None` while the `[zk_note]` table's
    /// `subsidy_budget` is zero, which refuses every subsidised proof as
    /// budget-closed.
    pub subsidy: Option<Subsidy>,
    pub clock: Arc<dyn Clock>,
}

/// A zk edge's standing subsidy terms (ADR 0080 decision 14): the budget it
/// pays per subsidy period, and the vault's creation time that the periods
/// are counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subsidy {
    /// The most subsidised allowances may reserve in one period, in MINA base
    /// units. Never zero: a zero budget is no `Subsidy` at all.
    pub budget: u64,
    /// When the vault was created, ms since the epoch.
    pub vault_creation_ms: u64,
}

impl Subsidy {
    /// Read the terms at boot: a zero `budget` reads nothing and offers no
    /// subsidy, and a non-zero one reads the vault's creation time once.
    ///
    /// # Errors
    /// [`crate::zk_note::ZkNoteVaultUnavailable`] when the read fails: a node
    /// that cannot say which period it is in must not boot.
    pub async fn read(
        vault: &dyn ZkNoteVault,
        budget: u64,
    ) -> Result<Option<Subsidy>, crate::zk_note::ZkNoteVaultUnavailable> {
        if budget == 0 {
            return Ok(None);
        }
        Ok(Some(Subsidy {
            budget,
            vault_creation_ms: vault.creation_time_ms().await?,
        }))
    }

    /// The subsidy period `now_ms` falls in: zk_toon's, or `None` before the
    /// vault was created.
    fn period(&self, now_ms: u64) -> Option<u64> {
        policy::subsidy_period(self.vault_creation_ms, now_ms)
    }
}

/// A request to open an allowance: the request proof, and the two terms the
/// proof binds only through its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceRequest {
    /// The request proof, o1js `JsonProof` bytes.
    pub proof: Vec<u8>,
    /// The fresh session key the payer will sign allowance claims with.
    pub session_key: CompressedPubKey,
    /// The expiry the payer asks for, ms since the epoch.
    pub expires_at_ms: u64,
}

/// This connector is no zk edge: what opening, closing and fetching a
/// successor all refuse with, under the one name both endpoints answer
/// ([`NotAZkEdge::NAME`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this connector is not a zk edge")]
pub struct NotAZkEdge;

impl NotAZkEdge {
    /// The rule's name, as both zk-note endpoints answer it.
    pub const NAME: &'static str = "zk_note_not_offered";
}

/// An allowance granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceGrant {
    pub allowance_id: AllowanceId,
    /// The budget `b`: the proof's tier, nanomina.
    pub tier: u64,
    pub expires_at_ms: u64,
}

/// Why an allowance was refused, each by its rule. None names a nullifier,
/// a session key or anything from the proof beyond its tier and times.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AllowanceRefusal {
    #[error("allowance refused: {}", NotAZkEdge)]
    NotAZkEdge,
    #[error("allowance refused: the request is malformed: {0}")]
    Malformed(String),
    #[error("allowance refused: tier {tier} is not one this zk edge publishes")]
    TierNotOffered { tier: u64 },
    #[error(
        "allowance refused: an expiry of {expires_at_ms} is not after now and at most \
         {latest}, this zk edge's allowance lifetime from now"
    )]
    ExpiryOutsideLifetime { expires_at_ms: u64, latest: u64 },
    #[error("allowance refused: the proof is about a state key other than this zk edge's")]
    ForeignStateKey,
    #[error(
        "allowance refused: a request time of {request_time} is more than 10 minutes behind or \
         1 minute ahead of this zk edge's clock ({now})"
    )]
    RequestTimeOutsideWindow { request_time: u64, now: u64 },
    #[error(
        "allowance refused: a subsidised allowance is offered at the smallest tier only, not at \
         {tier}"
    )]
    SubsidyTierNotSmallest { tier: u64 },
    #[error(
        "allowance refused: the proof is for subsidy period {period}, which is not the current \
         one; prove again for the current period"
    )]
    SubsidyPeriodNotCurrent { period: String },
    #[error("allowance refused: this zk edge's subsidy budget for the period is closed")]
    SubsidyBudgetClosed,
    #[error("allowance refused: the note has already used its subsidised allowance this period")]
    SubsidyNullifierSpent,
    #[error(
        "allowance refused: the authorization tag does not bind this zk edge, the session \
         key, the tier and the expiry"
    )]
    TagDoesNotBind,
    #[error("allowance refused: the note's state is already spent")]
    NullifierSpent,
    #[error(
        "allowance refused: open allowances already hold {open} of this zk edge's open budget \
         of {cap}, and a tier of {tier} would exceed it"
    )]
    OpenBudgetExceeded { open: u64, tier: u64, cap: u64 },
    #[error("allowance refused: the proof is against a root the vault has never held")]
    UnknownRoot,
    #[error("allowance refused: the request proof does not verify")]
    ProofInvalid,
    #[error("allowance refused: the vault could not be read; retry: {0}")]
    VaultUnavailable(String),
    #[error(
        "allowance refused: this zk edge could not durably record the grant, and grants \
         nothing it could not remember; retry"
    )]
    NotDurable,
}

impl AllowanceRefusal {
    /// The rule's name, as `POST /ilp/zk-note/allowances` answers it.
    pub fn name(&self) -> &'static str {
        match self {
            AllowanceRefusal::NotAZkEdge => NotAZkEdge::NAME,
            AllowanceRefusal::Malformed(_) => "request_malformed",
            AllowanceRefusal::TierNotOffered { .. } => "tier_not_offered",
            AllowanceRefusal::ExpiryOutsideLifetime { .. } => "expiry_outside_lifetime",
            AllowanceRefusal::ForeignStateKey => "foreign_state_key",
            AllowanceRefusal::RequestTimeOutsideWindow { .. } => "request_time_outside_window",
            AllowanceRefusal::SubsidyTierNotSmallest { .. } => "subsidy_tier_not_smallest",
            AllowanceRefusal::SubsidyPeriodNotCurrent { .. } => "subsidy_period_not_current",
            AllowanceRefusal::SubsidyBudgetClosed => "subsidy_budget_closed",
            AllowanceRefusal::SubsidyNullifierSpent => "subsidy_nullifier_spent",
            AllowanceRefusal::TagDoesNotBind => "tag_does_not_bind",
            AllowanceRefusal::NullifierSpent => "nullifier_spent",
            AllowanceRefusal::OpenBudgetExceeded { .. } => "open_budget_exceeded",
            AllowanceRefusal::UnknownRoot => "unknown_root",
            AllowanceRefusal::ProofInvalid => "proof_invalid",
            AllowanceRefusal::VaultUnavailable(_) => "vault_unavailable",
            AllowanceRefusal::NotDurable => "not_durable",
        }
    }
}

/// The evidence file is missing a grant the journal records: the
/// challenger could never be shown it.
#[derive(Debug, thiserror::Error)]
#[error(
    "the zk-note evidence file holds no record for allowance {allowance_id}, which the journal \
     records as granted: the evidence that would defend its note has been lost"
)]
pub struct ZkEdgeRestoreError {
    pub allowance_id: String,
}

impl ZkEdge {
    /// The public half of [`Self::state_signer`]: what the greeting
    /// publishes and every request proof must be about.
    pub fn state_key(&self) -> CompressedPubKey {
        self.state_signer.public_key()
    }
}

/// Where an allowance is in its life (ADR 0080 decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
enum AllowanceState {
    /// Taking claims.
    Open,
    /// Closed to new claims -- by its session key, its expiry or its
    /// budget -- and waiting for its forwards in flight to resolve.
    Closing,
    /// Δ is journaled; the successor is not yet. Only ever seen mid-close
    /// or after a crash between the two, which the boot sweep finishes.
    Charged { charge: u64 },
    /// Closed, and its successor signed and journaled.
    Signed(JournaledSuccessor),
}

/// A successor as the journal holds it: everything but the key that
/// signed it, which is the edge's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JournaledSuccessor {
    charge: u64,
    commitment: Commitment,
    anchor: Field,
    signature: Signature,
}

#[derive(Debug, Clone)]
struct Allowance {
    tier: u64,
    expires_at_ms: u64,
    session_key: CompressedPubKey,
    /// The nullifier the allowance spent, whose next anchor the successor
    /// takes.
    nullifier: Field,
    /// The proof's re-randomised commitment, which the successor charges.
    commitment: Commitment,
    state: AllowanceState,
    /// The subsidy period a subsidised allowance was granted in: its tier is
    /// reserved against that period's budget, and its note is charged 0.
    subsidy_period: Option<u64>,
    /// Whether the close's start is journaled
    /// ([`JournalEntry::AllowanceClosing`]): once it is, a restart brings
    /// the allowance back closing. A close whose entry could not be written
    /// is retried by the sweep.
    closing_journaled: bool,
    /// Packets admitted on this allowance and not yet resolved: each is an
    /// [`InFlightForward`] somebody holds.
    in_flight: usize,
}

impl Allowance {
    /// What the allowance's final watermark Δ charges the note: Δ, or nothing
    /// when the subsidy budget pays it (decision 14).
    fn note_charge(&self, delta: u64) -> u64 {
        if self.subsidy_period.is_some() {
            0
        } else {
            delta
        }
    }

    /// Open or closing: its budget still rides on its forwards.
    fn is_open(&self) -> bool {
        matches!(self.state, AllowanceState::Open | AllowanceState::Closing)
    }
}

/// Every allowance granted, and every nullifier spent or being spent.
#[derive(Debug, Default)]
pub(super) struct ZkLedger {
    allowances: HashMap<AllowanceId, Allowance>,
    evidence: Vec<(AllowanceId, [u8; 32])>,
    spent: HashSet<[u8; 32]>,
    /// Nullifiers whose request is past its local checks and still being
    /// verified, with the tiers they reserve against the cap.
    pending: HashMap<[u8; 32], PendingGrant>,
    /// What subsidised allowances have reserved or been charged against each
    /// period's budget: a grant adds its tier, a close gives back what Δ did
    /// not use. A period's tally starts from nothing, so a remainder returned
    /// after its period ended raises no later period's budget.
    subsidy_used: HashMap<u64, u64>,
    /// The per-period nullifiers seen in each period. A finished period's are
    /// pruned: a proof for it can no longer be admitted.
    subsidy_nullifiers: HashMap<u64, HashSet<[u8; 32]>>,
    /// How many subsidised allowances each period granted. Pruned to the
    /// current period and the last finished one.
    subsidy_grants: HashMap<u64, u64>,
}

/// A request past its local checks and still being verified.
#[derive(Debug, Clone)]
struct PendingGrant {
    tier: u64,
    /// The period and per-period nullifier of a subsidised request.
    subsidy: Option<(u64, [u8; 32])>,
}

impl ZkLedger {
    /// Fold the journal's grants.
    pub(super) fn replay(entries: &[JournalEntry]) -> Result<ZkLedger, JournalError> {
        let mut ledger = ZkLedger::default();
        for entry in entries {
            match entry {
                JournalEntry::AllowanceGranted {
                    allowance_id,
                    nullifier,
                    evidence_hash,
                    tier,
                    expires_at_ms,
                    session_key_x,
                    session_key_is_odd,
                    commitment_x,
                    commitment_y,
                    subsidy,
                } => {
                    let corrupt =
                        || JournalError::Corrupt(format!("allowance_granted {allowance_id}"));
                    let id: AllowanceId = allowance_id.parse().map_err(|_| corrupt())?;
                    let x = field_from_le_bytes(session_key_x).ok_or_else(corrupt)?;
                    ledger.spent.insert(*nullifier);
                    if let Some(subsidy) = subsidy {
                        ledger.record_subsidised_grant(
                            subsidy.period,
                            subsidy.period_nullifier,
                            *tier,
                        );
                    }
                    ledger.evidence.push((id, *evidence_hash));
                    ledger.allowances.insert(
                        id,
                        Allowance {
                            tier: *tier,
                            expires_at_ms: *expires_at_ms,
                            session_key: CompressedPubKey {
                                x,
                                is_odd: *session_key_is_odd,
                            },
                            nullifier: field_from_le_bytes(nullifier).ok_or_else(corrupt)?,
                            commitment: commitment_from_le_bytes(commitment_x, commitment_y)
                                .ok_or_else(corrupt)?,
                            state: AllowanceState::Open,
                            subsidy_period: subsidy.as_ref().map(|subsidy| subsidy.period),
                            closing_journaled: false,
                            in_flight: 0,
                        },
                    );
                }
                JournalEntry::AllowanceClosing { allowance_id } => {
                    let corrupt =
                        || JournalError::Corrupt(format!("allowance_closing {allowance_id}"));
                    let allowance = ledger.replayed(allowance_id).ok_or_else(corrupt)?;
                    if allowance.state != AllowanceState::Open {
                        // A close starts once; a second is a journal this
                        // build did not write.
                        return Err(corrupt());
                    }
                    allowance.state = AllowanceState::Closing;
                    allowance.closing_journaled = true;
                }
                JournalEntry::AllowanceClosed {
                    allowance_id,
                    charge,
                } => {
                    let corrupt =
                        || JournalError::Corrupt(format!("allowance_closed {allowance_id}"));
                    let allowance = ledger.replayed(allowance_id).ok_or_else(corrupt)?;
                    if allowance.state != AllowanceState::Closing {
                        // Δ is written once, after the close's start; any
                        // other order is a journal this build did not write.
                        return Err(corrupt());
                    }
                    allowance.state = AllowanceState::Charged { charge: *charge };
                    ledger.release_subsidy(allowance_id, *charge);
                }
                JournalEntry::AllowanceSuccessorSigned {
                    allowance_id,
                    commitment_x,
                    commitment_y,
                    anchor,
                    signature_rx,
                    signature_s,
                } => {
                    let corrupt = || {
                        JournalError::Corrupt(format!("allowance_successor_signed {allowance_id}"))
                    };
                    let allowance = ledger.replayed(allowance_id).ok_or_else(corrupt)?;
                    let AllowanceState::Charged { charge } = allowance.state else {
                        return Err(corrupt());
                    };
                    allowance.state = AllowanceState::Signed(JournaledSuccessor {
                        charge: allowance.note_charge(charge),
                        commitment: commitment_from_le_bytes(commitment_x, commitment_y)
                            .ok_or_else(corrupt)?,
                        anchor: field_from_le_bytes(anchor).ok_or_else(corrupt)?,
                        signature: Signature::new(
                            field_from_le_bytes(signature_rx).ok_or_else(corrupt)?,
                            scalar_from_le_bytes(signature_s).ok_or_else(corrupt)?,
                        ),
                    });
                }
                _ => {}
            }
        }
        Ok(ledger)
    }

    fn replayed(&mut self, allowance_id: &str) -> Option<&mut Allowance> {
        let id: AllowanceId = allowance_id.parse().ok()?;
        self.allowances.get_mut(&id)
    }

    /// Reserve `tier` against `period`'s budget for a subsidised grant, and
    /// remember its per-period nullifier.
    fn record_subsidised_grant(&mut self, period: u64, period_nullifier: [u8; 32], tier: u64) {
        let used = self.subsidy_used.entry(period).or_default();
        *used = used.saturating_add(tier);
        self.subsidy_nullifiers
            .entry(period)
            .or_default()
            .insert(period_nullifier);
        *self.subsidy_grants.entry(period).or_default() += 1;
    }

    /// A subsidised allowance's close: its reservation becomes Δ, and the
    /// remainder returns to the period it was reserved in.
    fn release_subsidy(&mut self, allowance_id: &str, delta: u64) {
        let Some(allowance) = self.replayed(allowance_id) else {
            return;
        };
        let (Some(period), tier) = (allowance.subsidy_period, allowance.tier) else {
            return;
        };
        if let Some(used) = self.subsidy_used.get_mut(&period) {
            *used = used.saturating_sub(tier.saturating_sub(delta));
        }
    }

    /// Whether `period`'s budget still covers the smallest tier, counting
    /// what requests being admitted have reserved.
    fn subsidy_open(&self, budget: u64, period: u64) -> bool {
        let pending: u64 = self
            .pending
            .values()
            .filter_map(|pending| match pending.subsidy {
                Some((p, _)) if p == period => Some(pending.tier),
                _ => None,
            })
            .fold(0, u64::saturating_add);
        let used = self.subsidy_used.get(&period).copied().unwrap_or(0);
        budget.saturating_sub(used).saturating_sub(pending) >= policy::TIERS[0]
    }

    /// Whether a per-period nullifier is already seen in `period`, granted or
    /// being admitted.
    fn subsidy_nullifier_seen(&self, period: u64, period_nullifier: &[u8; 32]) -> bool {
        self.subsidy_nullifiers
            .get(&period)
            .is_some_and(|seen| seen.contains(period_nullifier))
            || self.pending.values().any(|pending| {
                pending
                    .subsidy
                    .is_some_and(|(p, n)| p == period && n == *period_nullifier)
            })
    }

    /// The refusal, if any, that `period`'s budget and nullifier make
    /// (decision 14, refusals 3 and 4). Also forgets what finished periods
    /// left behind, keeping only the last one's grant count for the
    /// self-description.
    fn subsidy_refusal(
        &mut self,
        budget: u64,
        period: u64,
        period_nullifier: &[u8; 32],
    ) -> Option<AllowanceRefusal> {
        self.subsidy_nullifiers.retain(|p, _| *p >= period);
        self.subsidy_used.retain(|p, _| *p >= period);
        self.subsidy_grants
            .retain(|p, _| p.saturating_add(1) >= period);
        if !self.subsidy_open(budget, period) {
            return Some(AllowanceRefusal::SubsidyBudgetClosed);
        }
        if self.subsidy_nullifier_seen(period, period_nullifier) {
            return Some(AllowanceRefusal::SubsidyNullifierSpent);
        }
        None
    }

    /// The budgets of open allowances -- every allowance granted and not yet
    /// charged (ADR 0080 decision 5) -- and of those being admitted.
    fn open_budget(&self) -> u64 {
        self.allowances
            .values()
            .filter(|allowance| allowance.is_open())
            .map(|allowance| allowance.tier)
            .chain(self.pending.values().map(|pending| pending.tier))
            .fold(0, u64::saturating_add)
    }
}

/// A nullifier and its tier held against the cap while a request is being
/// verified, released when dropped. A granted request has already moved its
/// nullifier from pending to spent, in the lock hold that records the
/// allowance, so the release then finds nothing to remove.
struct Reservation<'a> {
    ledger: &'a Mutex<ZkLedger>,
    nullifier: [u8; 32],
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .pending
            .remove(&self.nullifier);
    }
}

fn now_ms(clock: &dyn Clock) -> u64 {
    u64::try_from(clock.now().timestamp_millis()).unwrap_or(0)
}

fn mint_allowance_id() -> AllowanceId {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("the OS random source is available");
    AllowanceId::from_bytes(bytes)
}

impl ClientClaimGate {
    /// Make this gate a zk edge (ADR 0080). The journal's grants were
    /// replayed by [`ClientClaimGate::restore`]; this checks that the
    /// evidence file still holds every one of them, then runs the boot sweep
    /// (decision 4): every allowance that expired while the node was down is
    /// closed, and a close a crash interrupted is finished, before the gate
    /// serves anything.
    ///
    /// # Errors
    /// [`ZkEdgeRestoreError`] naming a granted allowance the evidence file
    /// has no record of.
    pub fn with_zk_edge(mut self, edge: ZkEdge) -> Result<ClientClaimGate, ZkEdgeRestoreError> {
        {
            let ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            if let Some((id, _)) = ledger
                .evidence
                .iter()
                .find(|(_, hash)| !edge.evidence.holds(hash))
            {
                return Err(ZkEdgeRestoreError {
                    allowance_id: id.to_string(),
                });
            }
        }
        self.zk_edge = Some(Arc::new(edge));
        self.sweep_allowances();
        Ok(self)
    }

    /// Admit an allowance (ADR 0080 decision 3, as amended): every check,
    /// cheapest first, then the evidence and the journal, then the grant.
    /// The operation `POST /ilp/zk-note/allowances` (#1523) answers with.
    ///
    /// # Errors
    /// [`AllowanceRefusal`], naming the first rule the request breaks.
    pub async fn admit_allowance(
        &self,
        request: AllowanceRequest,
    ) -> Result<AllowanceGrant, AllowanceRefusal> {
        let edge = self.zk_edge.as_ref().ok_or(AllowanceRefusal::NotAZkEdge)?;

        // Structure.
        let proof = RequestProof::parse(&request.proof)
            .map_err(|error| AllowanceRefusal::Malformed(error.to_string()))?;
        // The evidence file frames a proof with a `u32` length, which
        // `zk-evidence` does not check: a longer one is refused here, not
        // truncated there.
        if u32::try_from(proof.json().len()).is_err() {
            return Err(AllowanceRefusal::Malformed(
                "the request proof is too long to record as evidence".into(),
            ));
        }
        if decompress(&request.session_key).is_none() {
            return Err(AllowanceRefusal::Malformed(
                "the session key is not a point on Pallas".into(),
            ));
        }
        let public = proof.public();

        // The terms this edge publishes.
        let tier = edge
            .tiers
            .get(public.tier)
            .ok_or(AllowanceRefusal::TierNotOffered { tier: public.tier })?
            .budget();
        let now = now_ms(edge.clock.as_ref());
        let lifetime_ms = u64::try_from(edge.allowance_lifetime.as_millis()).unwrap_or(u64::MAX);
        let latest = now.saturating_add(lifetime_ms);
        if request.expires_at_ms <= now || request.expires_at_ms > latest {
            return Err(AllowanceRefusal::ExpiryOutsideLifetime {
                expires_at_ms: request.expires_at_ms,
                latest,
            });
        }

        // What the proof leaves to the zk edge (decision 3, as amended).
        let state_key = edge.state_key();
        if proof.vault() != state_key {
            return Err(AllowanceRefusal::ForeignStateKey);
        }
        if !policy::is_request_time_fresh(public.request_time, now) {
            return Err(AllowanceRefusal::RequestTimeOutsideWindow {
                request_time: public.request_time,
                now,
            });
        }
        let subsidy = self.check_subsidy(edge, public, now)?;

        // The binding tag: this edge, this session key, this tier, this expiry.
        let tag = expected_tag(
            public.nullifier,
            &state_key,
            &request.session_key,
            public.tier,
            request.expires_at_ms,
        );
        if tag != public.tag {
            return Err(AllowanceRefusal::TagDoesNotBind);
        }

        // This edge's own records: a spent nullifier, and the cap. The
        // nullifier and the tier are reserved from here on.
        let nullifier = field_to_le_bytes(public.nullifier);
        let reservation = {
            let mut ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            if ledger.spent.contains(&nullifier) || ledger.pending.contains_key(&nullifier) {
                return Err(AllowanceRefusal::NullifierSpent);
            }
            // The authoritative check of what `check_subsidy` read.
            if let (Some(subsidy), Some((period, period_nullifier))) = (&edge.subsidy, &subsidy) {
                if let Some(refusal) =
                    ledger.subsidy_refusal(subsidy.budget, *period, period_nullifier)
                {
                    return Err(refusal);
                }
            }
            let open = ledger.open_budget();
            if open.saturating_add(tier) > edge.open_budget_cap {
                return Err(AllowanceRefusal::OpenBudgetExceeded {
                    open,
                    tier,
                    cap: edge.open_budget_cap,
                });
            }
            ledger
                .pending
                .insert(nullifier, PendingGrant { tier, subsidy });
            Reservation {
                ledger: &self.zk_ledger,
                nullifier,
            }
        };

        // The vault, last.
        let unavailable = |error: crate::zk_note::ZkNoteVaultUnavailable| {
            AllowanceRefusal::VaultUnavailable(error.0)
        };
        if !edge
            .vault
            .root_known(public.root)
            .await
            .map_err(unavailable)?
        {
            return Err(AllowanceRefusal::UnknownRoot);
        }
        if edge
            .vault
            .nullifier_used(public.nullifier)
            .await
            .map_err(unavailable)?
        {
            return Err(AllowanceRefusal::NullifierSpent);
        }
        if !edge
            .vault
            .verify_request_proof(&proof)
            .await
            .map_err(unavailable)?
        {
            return Err(AllowanceRefusal::ProofInvalid);
        }

        // Evidence, then the journal, then the answer.
        let allowance_id = mint_allowance_id();
        let payload = Record::Grant(Grant {
            proof: proof.json().to_vec(),
            allowance_id: allowance_id.as_bytes().to_vec(),
            session_key_x: request.session_key.x,
            session_key_is_odd: request.session_key.is_odd,
            expires_at: request.expires_at_ms,
            granted_at: now,
        })
        .encode()
        .map_err(|error| AllowanceRefusal::Malformed(decode_error_message(&error)))?;
        // Pending until the journal holds the grant: a grant the journal
        // refuses is cut back out of the evidence file, so a retry writes
        // the only grant record for this nullifier. The file stays locked
        // meanwhile, so nothing is appended after it.
        let pending = edge.evidence.append_pending(&payload).map_err(|error| {
            tracing::error!(%error, "refusing an allowance: its evidence could not be written");
            AllowanceRefusal::NotDurable
        })?;
        let evidence_hash = pending.hash();
        self.journal
            .append(&JournalEntry::AllowanceGranted {
                allowance_id: allowance_id.to_string(),
                nullifier,
                evidence_hash,
                tier,
                expires_at_ms: request.expires_at_ms,
                session_key_x: field_to_le_bytes(request.session_key.x),
                session_key_is_odd: request.session_key.is_odd,
                commitment_x: field_to_le_bytes(public.commitment.x),
                commitment_y: field_to_le_bytes(public.commitment.y),
                subsidy: subsidy.map(|(period, period_nullifier)| AllowanceSubsidy {
                    period,
                    period_nullifier,
                }),
            })
            .map_err(|error| {
                tracing::error!(%error, "refusing an allowance: its grant could not be journaled");
                AllowanceRefusal::NotDurable
            })?;
        pending.keep();
        {
            let mut ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            ledger.allowances.insert(
                allowance_id,
                Allowance {
                    tier,
                    expires_at_ms: request.expires_at_ms,
                    session_key: request.session_key,
                    nullifier: public.nullifier,
                    commitment: proof.commitment(),
                    state: AllowanceState::Open,
                    subsidy_period: subsidy.map(|(period, _)| period),
                    closing_journaled: false,
                    in_flight: 0,
                },
            );
            ledger.evidence.push((allowance_id, evidence_hash));
            // Inserted into `spent` in the same lock hold as the allowance,
            // so the open budget never counts it twice or not at all.
            ledger.pending.remove(&nullifier);
            ledger.spent.insert(nullifier);
            if let Some((period, period_nullifier)) = subsidy {
                ledger.record_subsidised_grant(period, period_nullifier, tier);
            }
        }
        drop(reservation);
        tracing::info!(allowance = %allowance_id, tier, "granted a zk-note allowance");
        Ok(AllowanceGrant {
            allowance_id,
            tier,
            expires_at_ms: request.expires_at_ms,
        })
    }

    /// The checks a proof's subsidy adds, in decision 14's order, before the
    /// binding tag. Returns the period and per-period nullifier of a
    /// subsidised proof that passes them; `None` for an unsubsidised one,
    /// whose period and per-period nullifier must be zero.
    fn check_subsidy(
        &self,
        edge: &ZkEdge,
        public: &connector_zk_note::request::RequestPublic,
        now: u64,
    ) -> Result<Option<(u64, [u8; 32])>, AllowanceRefusal> {
        if !public.subsidised {
            // zk_toon's rule for an unsubsidised proof: no period and no
            // per-period nullifier. It reads no creation time.
            if public.check_subsidy_period(0).is_err() {
                return Err(AllowanceRefusal::Malformed(
                    "an unsubsidised proof states a subsidy period or per-period nullifier".into(),
                ));
            }
            return Ok(None);
        }
        // 1. The smallest tier.
        if public.tier != policy::TIERS[0] {
            return Err(AllowanceRefusal::SubsidyTierNotSmallest { tier: public.tier });
        }
        // Without a budget there is no period to measure against, and the
        // budget is closed whatever the period.
        let Some(subsidy) = edge.subsidy.as_ref() else {
            return Err(AllowanceRefusal::SubsidyBudgetClosed);
        };
        // 2. The current period, with no grace at a boundary: the edge's
        // current period, and -- zk_toon's `check_subsidy_period`, which
        // the circuit cannot do -- the period the proof's own request time
        // is in, counted from this vault's creation. Just past a boundary
        // the two differ, and a proof claiming the edge's period for a
        // request made in the last one was proven with another creation time.
        let not_current = || AllowanceRefusal::SubsidyPeriodNotCurrent {
            period: public.period.to_string(),
        };
        let period = subsidy
            .period(now)
            .filter(|current| public.period == *current)
            .ok_or_else(not_current)?;
        public
            .check_subsidy_period(subsidy.vault_creation_ms)
            .map_err(|_| not_current())?;
        // 3. and 4. The budget and the per-period nullifier.
        let period_nullifier = field_to_le_bytes(public.period_nullifier);
        match self
            .zk_ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .subsidy_refusal(subsidy.budget, period, &period_nullifier)
        {
            Some(refusal) => Err(refusal),
            None => Ok(Some((period, period_nullifier))),
        }
    }

    /// Whether a subsidised allowance can be opened right now, as the
    /// greeting says it: `None` when this edge offers no subsidy, or is no zk
    /// edge at all. It is not a promise -- the refusal is authoritative.
    pub fn subsidy_open(&self) -> Option<bool> {
        let edge = self.zk_edge.as_ref()?;
        let subsidy = edge.subsidy.as_ref()?;
        let Some(period) = subsidy.period(now_ms(edge.clock.as_ref())) else {
            return Some(false);
        };
        Some(
            self.zk_ledger
                .lock()
                .expect("zk ledger lock poisoned")
                .subsidy_open(subsidy.budget, period),
        )
    }

    /// How many subsidised allowances the last **finished** subsidy period
    /// granted, for the self-description: never the current period's count.
    /// `None` when this edge offers no subsidy.
    pub fn subsidised_last_period(&self) -> Option<u64> {
        let edge = self.zk_edge.as_ref()?;
        let subsidy = edge.subsidy.as_ref()?;
        let finished = subsidy
            .period(now_ms(edge.clock.as_ref()))
            .and_then(|current| current.checked_sub(1));
        Some(finished.map_or(0, |period| {
            self.zk_ledger
                .lock()
                .expect("zk ledger lock poisoned")
                .subsidy_grants
                .get(&period)
                .copied()
                .unwrap_or(0)
        }))
    }

    /// [`Self::admit`]'s stages for an allowance claim: the allowance is
    /// open and unexpired, and the claim is counted in flight on it from
    /// here on; then the voucher rules and the budget under the read lock,
    /// the session key's signature, the authoritative re-check, the advance
    /// and the journal. A claim that reaches the budget closes the
    /// allowance; the close charges once the returned [`InFlightForward`]
    /// is dropped.
    pub(super) fn admit_allowance_claim(
        &self,
        claim: &AllowanceClaim,
        price: u64,
    ) -> Result<(DurabilityTicket, InFlightForward), ClaimIngestRejection> {
        let parts = self
            .zk_parts()
            .ok_or(ClaimIngestRejection::UnknownAllowance)?;
        let (tier, session_key) = {
            let mut ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            let allowance = ledger
                .allowances
                .get_mut(&claim.allowance_id)
                .ok_or(ClaimIngestRejection::UnknownAllowance)?;
            if allowance.state != AllowanceState::Open {
                return Err(ClaimIngestRejection::AllowanceClosed);
            }
            if now_ms(parts.edge.clock.as_ref()) >= allowance.expires_at_ms {
                return Err(ClaimIngestRejection::AllowanceExpired);
            }
            // Counted before anything moves, and under the same lock a
            // close takes: a close either sees this claim in flight, or
            // this claim sees the allowance closing.
            allowance.in_flight += 1;
            (allowance.tier, allowance.session_key.clone())
        };
        // Declared before any watermark lock, so it is dropped after it: a
        // drop may charge the allowance, which reads the watermarks.
        let in_flight = InFlightForward(Some((parts, claim.allowance_id)));

        let key = claim.allowance_id.claim_key();
        let amount = u128::from(claim.cumulative_amount);
        let check = |current: Option<&LiveClaim>| {
            let watermark = current.map(|record| VoucherWatermark {
                cumulative_amount: record.watermark.cumulative_amount,
                signature: &record.signature,
            });
            validate_allowance_claim(watermark, amount, &claim.signature, price, tier)
                .map_err(claim_refusal)
        };
        {
            let watermarks = self
                .watermarks
                .read()
                .expect("client claim watermarks lock poisoned");
            if check(watermarks.get(&key))? == VoucherAdmission::Retransmission {
                drop(watermarks);
                return Ok((self.retransmission(&key), in_flight));
            }
        }

        let signature = signature_from_be_bytes(&claim.signature)
            .ok_or(ClaimIngestRejection::SignatureInvalid)?;
        if !verify_allowance_claim(
            &signature,
            &session_key,
            claim.allowance_id.as_bytes(),
            claim.cumulative_amount,
        ) {
            return Err(ClaimIngestRejection::SignatureInvalid);
        }

        let mut watermarks = self
            .watermarks
            .write()
            .expect("client claim watermarks lock poisoned");
        if check(watermarks.get(&key))? == VoucherAdmission::Retransmission {
            drop(watermarks);
            return Ok((self.retransmission(&key), in_flight));
        }
        let ticket = self.advance_and_enqueue(
            &mut watermarks,
            &key,
            advance_voucher_watermark(amount),
            claim.signature.to_vec(),
            None,
        )?;
        drop(watermarks);
        tracing::debug!(allowance = %key, cumulative = amount, "accepted an allowance claim");
        if amount == u128::from(tier) {
            // Decision 4: an allowance closes when it reaches its budget. A
            // close start that cannot be journaled is retried by the sweep;
            // the claim that reached the budget is good either way.
            let _ = self.begin_close(claim.allowance_id);
        }
        Ok((ticket, in_flight))
    }

    /// Whether this gate is a zk edge: what decides whether the zk-note
    /// endpoints serve or refuse by name.
    pub fn is_zk_edge(&self) -> bool {
        self.zk_edge.is_some()
    }

    /// The parts a close needs, shared with every [`InFlightForward`]:
    /// `None` on a gate that is no zk edge.
    fn zk_parts(&self) -> Option<ZkParts> {
        Some(ZkParts {
            edge: Arc::clone(self.zk_edge.as_ref()?),
            ledger: Arc::clone(&self.zk_ledger),
            watermarks: Arc::clone(&self.watermarks),
            journal: Arc::clone(&self.journal),
            closing: Arc::clone(&self.zk_closing),
        })
    }

    /// Stop `id` taking claims, durably, and charge it now if nothing is in
    /// flight.
    ///
    /// # Errors
    /// [`JournalError`] when the close's start could not be journaled. The
    /// allowance takes no claims regardless; the sweep retries the entry.
    fn begin_close(&self, id: AllowanceId) -> Result<(), JournalError> {
        let Some(parts) = self.zk_parts() else {
            return Ok(());
        };
        parts.start_close(id)?;
        parts.finish(id);
        Ok(())
    }

    /// Close an allowance on its session key's signed request (ADR 0080
    /// decisions 4 and 8): it stops taking claims at once, and is charged
    /// once every forward in flight on it has resolved. Idempotent: closing
    /// a closed allowance answers its successor again. What
    /// `POST /ilp/zk-note/allowances` answers a close with.
    ///
    /// # Errors
    /// [`AllowanceCloseRefusal`], by its rule.
    pub fn close_allowance(
        &self,
        close: AllowanceClose,
    ) -> Result<SuccessorStatus, AllowanceCloseRefusal> {
        if self.zk_edge.is_none() {
            return Err(AllowanceCloseRefusal::NotAZkEdge);
        }
        let session_key = self
            .zk_ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .allowances
            .get(&close.allowance_id)
            .map(|allowance| allowance.session_key.clone())
            .ok_or(AllowanceCloseRefusal::UnknownAllowance)?;
        let signed = signature_from_be_bytes(&close.signature).is_some_and(|signature| {
            verify_allowance_close(&signature, &session_key, close.allowance_id.as_bytes())
        });
        if !signed {
            return Err(AllowanceCloseRefusal::SignatureInvalid);
        }
        self.begin_close(close.allowance_id)
            .map_err(|_| AllowanceCloseRefusal::NotDurable)?;
        self.successor(close.allowance_id)
    }

    /// An allowance's successor (ADR 0080 decision 4): pending until its
    /// charge is journaled, then the one signed state, every time. Only
    /// reads. What `GET /ilp/zk-note/allowances/:id/successor` answers.
    ///
    /// # Errors
    /// [`AllowanceCloseRefusal::UnknownAllowance`] for an id this edge never
    /// granted, or [`AllowanceCloseRefusal::NotAZkEdge`].
    pub fn successor(&self, id: AllowanceId) -> Result<SuccessorStatus, AllowanceCloseRefusal> {
        let edge = self
            .zk_edge
            .as_ref()
            .ok_or(AllowanceCloseRefusal::NotAZkEdge)?;
        let ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
        let allowance = ledger
            .allowances
            .get(&id)
            .ok_or(AllowanceCloseRefusal::UnknownAllowance)?;
        Ok(match &allowance.state {
            AllowanceState::Signed(journaled) => {
                SuccessorStatus::Signed(Box::new(SignedSuccessor {
                    allowance_id: id,
                    charge: journaled.charge,
                    state_key: edge.state_key(),
                    commitment: journaled.commitment,
                    anchor: journaled.anchor,
                    signature: journaled.signature.clone(),
                }))
            }
            _ => SuccessorStatus::Pending,
        })
    }

    /// The sweep (ADR 0080 decision 4): close every allowance past its
    /// expiry, and charge every closing one with nothing left in flight --
    /// including a close a crash interrupted. Run periodically by
    /// [`Self::run_allowance_sweep`], and once at boot by
    /// [`Self::with_zk_edge`]. A no-op on a gate that is no zk edge.
    pub fn sweep_allowances(&self) {
        let Some(parts) = self.zk_parts() else {
            return;
        };
        let now = now_ms(parts.edge.clock.as_ref());
        let due: Vec<AllowanceId> = {
            let ledger = parts.ledger.lock().expect("zk ledger lock poisoned");
            ledger
                .allowances
                .iter()
                .filter(|(_, allowance)| match allowance.state {
                    AllowanceState::Open => now >= allowance.expires_at_ms,
                    AllowanceState::Closing => {
                        !allowance.closing_journaled || allowance.in_flight == 0
                    }
                    AllowanceState::Charged { .. } => true,
                    AllowanceState::Signed(_) => false,
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for id in due {
            // A start that cannot be journaled is logged, and retried on the
            // next sweep.
            if parts.start_close(id).is_ok() {
                parts.finish(id);
            }
        }
    }

    /// [`Self::sweep_allowances`], every `every`, for as long as the node
    /// runs. A sweep that cannot finish a close logs it and is retried on
    /// the next tick.
    pub async fn run_allowance_sweep(self: Arc<Self>, every: Duration) {
        let mut ticks = tokio::time::interval(every);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            self.sweep_allowances();
        }
    }
}

/// A request to close an allowance (ADR 0080 decision 4): its id, and the
/// session key's signature over the close fields
/// (`connector_zk_note::session::allowance_close_fields`), `rx ‖ s`
/// big-endian.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceClose {
    pub allowance_id: AllowanceId,
    pub signature: [u8; 64],
}

/// Why a close or a successor fetch was refused, by its rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AllowanceCloseRefusal {
    #[error("refused: {}", NotAZkEdge)]
    NotAZkEdge,
    #[error("refused: no allowance by that id was granted here")]
    UnknownAllowance,
    #[error("refused: the close is not signed by the allowance's session key")]
    SignatureInvalid,
    #[error(
        "refused: this zk edge could not durably record the close; the allowance takes no \
         more claims -- retry the close"
    )]
    NotDurable,
}

impl AllowanceCloseRefusal {
    /// The rule's name, as the endpoints answer it.
    pub fn name(&self) -> &'static str {
        match self {
            AllowanceCloseRefusal::NotAZkEdge => NotAZkEdge::NAME,
            AllowanceCloseRefusal::UnknownAllowance => "unknown_allowance",
            AllowanceCloseRefusal::SignatureInvalid => "signature_invalid",
            AllowanceCloseRefusal::NotDurable => "not_durable",
        }
    }
}

/// Where an allowance's successor is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessorStatus {
    /// Not yet charged: the allowance is open, or closing and waiting for
    /// its forwards in flight.
    Pending,
    /// Charged, and its successor signed.
    Signed(Box<SignedSuccessor>),
}

/// A closed allowance's successor state (ADR 0080 decisions 2 and 4): the
/// proof's commitment charged Δ, the next anchor, and the state key's
/// signature over zk_toon's state message. The payer checks the commitment
/// is its old balance less [`Self::charge`], and that the charge is at most
/// the highest claim it signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedSuccessor {
    pub allowance_id: AllowanceId,
    /// Δ: the allowance's final watermark.
    pub charge: u64,
    /// The key that signed it: the zk edge's state key.
    pub state_key: CompressedPubKey,
    pub commitment: Commitment,
    pub anchor: Field,
    pub signature: Signature,
}

impl SignedSuccessor {
    /// The message signed, zk_toon's five fields.
    pub fn message(&self) -> StateMessage {
        StateMessage::new(&self.state_key, self.commitment, self.anchor)
    }
}

/// One packet admitted on an allowance and not yet resolved (ADR 0080
/// decision 4). Held by the carriage from admission until the packet has
/// been fulfilled or rejected -- past the rollback of an uncarried forward
/// -- and dropped then: an allowance is charged only once none of its
/// forwards is in flight. Inert for a voucher.
#[must_use = "dropping it ends the forward it holds open"]
pub struct InFlightForward(Option<(ZkParts, AllowanceId)>);

impl InFlightForward {
    /// A forward no allowance waits on: a voucher's.
    pub(crate) fn untracked() -> InFlightForward {
        InFlightForward(None)
    }
}

impl Drop for InFlightForward {
    fn drop(&mut self) {
        let Some((parts, id)) = self.0.take() else {
            return;
        };
        let ready = {
            let mut ledger = parts.ledger.lock().expect("zk ledger lock poisoned");
            ledger.allowances.get_mut(&id).is_some_and(|allowance| {
                allowance.in_flight = allowance.in_flight.saturating_sub(1);
                allowance.state == AllowanceState::Closing && allowance.in_flight == 0
            })
        };
        if ready {
            parts.finish(id);
        }
    }
}

impl std::fmt::Debug for InFlightForward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some((_, id)) => write!(f, "InFlightForward({id})"),
            None => write!(f, "InFlightForward(untracked)"),
        }
    }
}

/// What finishing a close touches, cloned out of the gate so an
/// [`InFlightForward`] can finish one when it drops.
#[derive(Clone)]
struct ZkParts {
    edge: Arc<ZkEdge>,
    ledger: Arc<Mutex<ZkLedger>>,
    watermarks: Arc<RwLock<HashMap<String, LiveClaim>>>,
    journal: Arc<dyn Journal>,
    /// Held across a whole close, so two never interleave their writes.
    closing: Arc<Mutex<()>>,
}

impl ZkParts {
    /// Stop `id` taking claims at once (decision 4), and journal that it
    /// has, so a restart brings it back closing. A no-op for an allowance
    /// already closing with its start journaled, or past closing.
    ///
    /// # Errors
    /// [`JournalError`] when the entry could not be written; the allowance
    /// still takes no claims, and the sweep retries the entry.
    fn start_close(&self, id: AllowanceId) -> Result<(), JournalError> {
        // Under the close lock, so the start is journaled once.
        let _closing = self.closing.lock().expect("zk close lock poisoned");
        let unjournaled = {
            let mut ledger = self.ledger.lock().expect("zk ledger lock poisoned");
            let Some(allowance) = ledger.allowances.get_mut(&id) else {
                return Ok(());
            };
            if allowance.state == AllowanceState::Open {
                allowance.state = AllowanceState::Closing;
                tracing::info!(allowance = %id, "closing a zk-note allowance");
            }
            allowance.state == AllowanceState::Closing && !allowance.closing_journaled
        };
        if !unjournaled {
            return Ok(());
        }
        self.journal
            .append(&JournalEntry::AllowanceClosing {
                allowance_id: id.to_string(),
            })
            .inspect_err(|error| {
                tracing::error!(
                    allowance = %id, %error,
                    "a zk-note allowance's close could not be journaled; the sweep retries"
                );
            })?;
        if let Some(allowance) = self
            .ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .allowances
            .get_mut(&id)
        {
            allowance.closing_journaled = true;
        }
        Ok(())
    }

    /// Finish `id`'s close as far as it will go: charge a closing one with
    /// nothing in flight, then sign the successor of a charged one. Each
    /// step is durable before the next (decision 11): Δ is journaled, then
    /// the evidence file's close record, then the signed successor. A step
    /// that fails is logged and left for the next sweep; nothing is ever
    /// charged twice.
    fn finish(&self, id: AllowanceId) {
        let _closing = self.closing.lock().expect("zk close lock poisoned");
        let Some(allowance) = self
            .ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .allowances
            .get(&id)
            .cloned()
        else {
            return;
        };
        let charge = match allowance.state {
            // Charged only once its start is journaled: a restart must find
            // it closing before it can find it charged.
            AllowanceState::Closing if allowance.in_flight == 0 && allowance.closing_journaled => {
                // Nothing in flight and nothing admitted: the watermark is
                // final.
                let watermark = self
                    .watermarks
                    .read()
                    .expect("client claim watermarks lock poisoned")
                    .get(&id.claim_key())
                    .map(|record| record.watermark.cumulative_amount);
                let charge = match allowance_close_charge(watermark, allowance.tier) {
                    Ok(charge) => charge,
                    Err(error) => {
                        tracing::error!(allowance = %id, %error, "not charging a zk-note allowance");
                        return;
                    }
                };
                if let Err(error) = self.journal.append(&JournalEntry::AllowanceClosed {
                    allowance_id: id.to_string(),
                    charge,
                }) {
                    tracing::error!(
                        allowance = %id, %error,
                        "a zk-note allowance's charge could not be journaled; the sweep retries"
                    );
                    return;
                }
                self.set_state(id, AllowanceState::Charged { charge });
                self.ledger
                    .lock()
                    .expect("zk ledger lock poisoned")
                    .release_subsidy(&id.to_string(), charge);
                tracing::info!(allowance = %id, charge, "charged a closed zk-note allowance");
                charge
            }
            AllowanceState::Charged { charge } => charge,
            _ => return,
        };

        // The challenger's close record, before the successor is released
        // (zk_toon ADR 0004). Appended only if missing, so a close a crash
        // interrupted is finished without a second record.
        //
        // Both it and the successor carry what the *note* is charged: for a
        // subsidised allowance that is nothing (decision 14), and Δ stays the
        // subsidy budget's, in the journal.
        let charge = allowance.note_charge(charge);
        let payload = match (Record::Close(Close {
            nullifier: allowance.nullifier,
            allowance_id: id.as_bytes().to_vec(),
            charge,
        }))
        .encode()
        {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(
                    allowance = %id,
                    error = %decode_error_message(&error),
                    "a close record could not be encoded"
                );
                return;
            }
        };
        if !self.edge.evidence.holds(&evidence_hash(&payload)) {
            if let Err(error) = self.edge.evidence.append(&payload) {
                tracing::error!(
                    allowance = %id, %error,
                    "a zk-note close record could not be written; the sweep retries"
                );
                return;
            }
        }

        let Some(next) = successor_state(
            &self.edge.state_key(),
            allowance.commitment,
            allowance.nullifier,
            charge,
        ) else {
            tracing::error!(allowance = %id, "a zk-note successor charges to no point; not signed");
            return;
        };
        let signature = self.edge.state_signer.sign(&next.message);
        if let Err(error) = self
            .journal
            .append(&JournalEntry::AllowanceSuccessorSigned {
                allowance_id: id.to_string(),
                commitment_x: field_to_le_bytes(next.commitment.x),
                commitment_y: field_to_le_bytes(next.commitment.y),
                anchor: field_to_le_bytes(next.anchor),
                signature_rx: field_to_le_bytes(signature.rx),
                signature_s: scalar_to_le_bytes(signature.s),
            })
        {
            tracing::error!(
                allowance = %id, %error,
                "a zk-note successor could not be journaled; the sweep retries"
            );
            return;
        }
        self.set_state(
            id,
            AllowanceState::Signed(JournaledSuccessor {
                charge,
                commitment: next.commitment,
                anchor: next.anchor,
                signature,
            }),
        );
        tracing::info!(allowance = %id, "signed a zk-note successor");
    }

    fn set_state(&self, id: AllowanceId, state: AllowanceState) {
        if let Some(allowance) = self
            .ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .allowances
            .get_mut(&id)
        {
            allowance.state = state;
        }
    }
}
#[cfg(test)]
#[path = "allowance_tests.rs"]
mod tests;
