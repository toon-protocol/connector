//! The zk edge's half of the claim gate (ADR 0080, issue #1522): admitting
//! an allowance, and admitting allowance claims against it under a voucher's
//! rules.
//!
//! **Admitting an allowance** is cheapest refusal first, and nothing is
//! written until every check has passed (decision 3, as amended): the
//! request's structure, a tier this edge publishes, an expiry within the
//! allowance lifetime, the proof being about this edge's own state key, a
//! request time inside zk_toon's window of this edge's clock, no subsidy
//! (until #1581), the binding tag -- and then the two refusals this gate's
//! own records can make, a nullifier it already spent and the open-budget
//! cap, before the three that cost a vault read: a known root, a nullifier
//! unused on chain, and the proof. Every check that needs no vault runs
//! before any that does, so a replayed or mis-bound proof costs nothing.
//!
//! A granted allowance is evidence before it is an answer (decision 11): its
//! grant record is appended to the evidence file and fsynced, then
//! [`JournalEntry::AllowanceGranted`] -- the spent nullifier, the record's
//! hash and the terms -- is appended to the journal and fsynced, and only
//! then is the grant returned. A nullifier is reserved, under the ledger's
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
//! its budget, and stops new claims at once. Every packet admitted on an
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
//! Nothing here logs or reports a proof, a nullifier or a session key: a
//! refusal names its rule, and a log line names the allowance by its opaque
//! id.

use std::time::Duration;

use connector_domain::client_claim::{AllowanceClaim, AllowanceId};
use connector_domain::{allowance_close_charge, validate_allowance_claim};
use connector_runtime::Clock;
use connector_zk_note::evidence::{evidence_hash, CloseRecord, GrantRecord, Record};
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
    pub clock: Arc<dyn Clock>,
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
    #[error("allowance refused: this connector is not a zk edge")]
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
    #[error("allowance refused: subsidised allowances are not offered here")]
    Subsidised,
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
            AllowanceRefusal::NotAZkEdge => "zk_note_not_offered",
            AllowanceRefusal::Malformed(_) => "request_malformed",
            AllowanceRefusal::TierNotOffered { .. } => "tier_not_offered",
            AllowanceRefusal::ExpiryOutsideLifetime { .. } => "expiry_outside_lifetime",
            AllowanceRefusal::ForeignStateKey => "foreign_state_key",
            AllowanceRefusal::RequestTimeOutsideWindow { .. } => "request_time_outside_window",
            AllowanceRefusal::Subsidised => "subsidised",
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
    /// Packets admitted on this allowance and not yet resolved: each is an
    /// [`InFlightForward`] somebody holds.
    in_flight: usize,
}

impl Allowance {
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
    pending: HashMap<[u8; 32], u64>,
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
                } => {
                    let corrupt =
                        || JournalError::Corrupt(format!("allowance_granted {allowance_id}"));
                    let id: AllowanceId = allowance_id.parse().map_err(|_| corrupt())?;
                    let x = field_from_le_bytes(session_key_x).ok_or_else(corrupt)?;
                    ledger.spent.insert(*nullifier);
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
                            in_flight: 0,
                        },
                    );
                }
                JournalEntry::AllowanceClosed {
                    allowance_id,
                    charge,
                } => {
                    let corrupt =
                        || JournalError::Corrupt(format!("allowance_closed {allowance_id}"));
                    let allowance = ledger.replayed(allowance_id).ok_or_else(corrupt)?;
                    if allowance.state != AllowanceState::Open {
                        // Δ is written once; a second is a journal this
                        // build did not write.
                        return Err(corrupt());
                    }
                    allowance.state = AllowanceState::Charged { charge: *charge };
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
                        charge,
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

    /// The budgets of open allowances -- every allowance granted and not yet
    /// charged (ADR 0080 decision 5) -- and of those being admitted.
    fn open_budget(&self) -> u64 {
        self.allowances
            .values()
            .filter(|allowance| allowance.is_open())
            .map(|allowance| allowance.tier)
            .chain(self.pending.values().copied())
            .fold(0, u64::saturating_add)
    }
}

/// A nullifier and its tier held against the cap while a request is being
/// verified; released unless the grant is committed.
struct Reservation<'a> {
    ledger: &'a Mutex<ZkLedger>,
    nullifier: [u8; 32],
    committed: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut ledger = self.ledger.lock().expect("zk ledger lock poisoned");
        ledger.pending.remove(&self.nullifier);
        if self.committed {
            ledger.spent.insert(self.nullifier);
        }
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
        if public.vault != state_key {
            return Err(AllowanceRefusal::ForeignStateKey);
        }
        if !policy::is_request_time_fresh(public.request_time, now) {
            return Err(AllowanceRefusal::RequestTimeOutsideWindow {
                request_time: public.request_time,
                now,
            });
        }
        let zero = Field::from(0u64);
        if public.subsidised || public.period != zero || public.period_nullifier != zero {
            return Err(AllowanceRefusal::Subsidised);
        }

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
        let mut reservation = {
            let mut ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            if ledger.spent.contains(&nullifier) || ledger.pending.contains_key(&nullifier) {
                return Err(AllowanceRefusal::NullifierSpent);
            }
            let open = ledger.open_budget();
            if open.saturating_add(tier) > edge.open_budget_cap {
                return Err(AllowanceRefusal::OpenBudgetExceeded {
                    open,
                    tier,
                    cap: edge.open_budget_cap,
                });
            }
            ledger.pending.insert(nullifier, tier);
            Reservation {
                ledger: &self.zk_ledger,
                nullifier,
                committed: false,
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
        let payload = Record::Grant(GrantRecord {
            proof_json: proof.json().to_vec(),
            allowance_id: allowance_id.as_bytes().to_vec(),
            session_key: request.session_key.clone(),
            expiry: request.expires_at_ms,
            granted_at: now,
        })
        .to_payload()
        .map_err(|error| AllowanceRefusal::Malformed(error.to_string()))?;
        let evidence_hash = edge.evidence.append(&payload).map_err(|error| {
            tracing::error!(%error, "refusing an allowance: its evidence could not be written");
            AllowanceRefusal::NotDurable
        })?;
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
            })
            .map_err(|error| {
                tracing::error!(%error, "refusing an allowance: its grant could not be journaled");
                AllowanceRefusal::NotDurable
            })?;
        {
            let mut ledger = self.zk_ledger.lock().expect("zk ledger lock poisoned");
            ledger.allowances.insert(
                allowance_id,
                Allowance {
                    tier,
                    expires_at_ms: request.expires_at_ms,
                    session_key: request.session_key,
                    nullifier: public.nullifier,
                    commitment: public.commitment,
                    state: AllowanceState::Open,
                    in_flight: 0,
                },
            );
            ledger.evidence.push((allowance_id, evidence_hash));
            // Inserted into `spent` in the same lock hold as the allowance,
            // so the open budget never counts it twice or not at all.
            ledger.pending.remove(&nullifier);
            ledger.spent.insert(nullifier);
        }
        reservation.committed = true;
        drop(reservation);
        tracing::info!(allowance = %allowance_id, tier, "granted a zk-note allowance");
        Ok(AllowanceGrant {
            allowance_id,
            tier,
            expires_at_ms: request.expires_at_ms,
        })
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

        let key = claim.allowance_id.channel_key();
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
            // Decision 4: an allowance closes when it reaches its budget.
            self.begin_close(claim.allowance_id);
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

    /// Stop `id` taking claims; charge it now if nothing is in flight.
    fn begin_close(&self, id: AllowanceId) {
        let Some(parts) = self.zk_parts() else {
            return;
        };
        {
            let mut ledger = parts.ledger.lock().expect("zk ledger lock poisoned");
            if let Some(allowance) = ledger.allowances.get_mut(&id) {
                if allowance.state == AllowanceState::Open {
                    allowance.state = AllowanceState::Closing;
                    tracing::info!(allowance = %id, "closing a zk-note allowance");
                }
            }
        }
        parts.finish(id);
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
        self.begin_close(close.allowance_id);
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
            let mut ledger = parts.ledger.lock().expect("zk ledger lock poisoned");
            ledger
                .allowances
                .iter_mut()
                .filter_map(|(id, allowance)| {
                    if allowance.state == AllowanceState::Open && now >= allowance.expires_at_ms {
                        allowance.state = AllowanceState::Closing;
                        tracing::info!(allowance = %id, "closing an expired zk-note allowance");
                    }
                    let unfinished = matches!(allowance.state, AllowanceState::Charged { .. })
                        || (allowance.state == AllowanceState::Closing && allowance.in_flight == 0);
                    unfinished.then_some(*id)
                })
                .collect()
        };
        for id in due {
            parts.finish(id);
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
    #[error("refused: this connector is not a zk edge")]
    NotAZkEdge,
    #[error("refused: no allowance by that id was granted here")]
    UnknownAllowance,
    #[error("refused: the close is not signed by the allowance's session key")]
    SignatureInvalid,
}

impl AllowanceCloseRefusal {
    /// The rule's name, as the endpoints answer it.
    pub fn name(&self) -> &'static str {
        match self {
            AllowanceCloseRefusal::NotAZkEdge => "zk_note_not_offered",
            AllowanceCloseRefusal::UnknownAllowance => "unknown_allowance",
            AllowanceCloseRefusal::SignatureInvalid => "signature_invalid",
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
            AllowanceState::Closing if allowance.in_flight == 0 => {
                // Nothing in flight and nothing admitted: the watermark is
                // final.
                let watermark = self
                    .watermarks
                    .read()
                    .expect("client claim watermarks lock poisoned")
                    .get(&id.channel_key())
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
                tracing::info!(allowance = %id, charge, "charged a closed zk-note allowance");
                charge
            }
            AllowanceState::Charged { charge } => charge,
            _ => return,
        };

        // The challenger's close record, before the successor is released
        // (zk_toon ADR 0004). Appended only if missing, so a close a crash
        // interrupted is finished without a second record.
        let payload = match (Record::Close(CloseRecord {
            nullifier: allowance.nullifier,
            allowance_id: id.as_bytes().to_vec(),
            charge,
        }))
        .to_payload()
        {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(allowance = %id, %error, "a close record could not be encoded");
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
