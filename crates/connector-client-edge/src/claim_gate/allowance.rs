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
//! Nothing here logs or reports a proof, a nullifier or a session key: a
//! refusal names its rule, and a log line names the allowance by its opaque
//! id.

use std::time::Duration;

use connector_domain::client_claim::{AllowanceClaim, AllowanceId};
use connector_domain::validate_allowance_claim;
use connector_runtime::Clock;
use connector_zk_note::evidence::{GrantRecord, Record};
use connector_zk_note::note::policy;
use connector_zk_note::request::{expected_tag, RequestProof};
use connector_zk_note::session::{signature_from_be_bytes, verify_allowance_claim};
use connector_zk_note::tiers::Tiers;
use connector_zk_note::{
    decompress, field_from_le_bytes, field_to_le_bytes, CompressedPubKey, Field,
};

use super::*;
use crate::evidence_file::EvidenceFile;
use crate::zk_note::ZkNoteVault;

/// What makes a gate a zk edge: its vault, its evidence file, its state
/// key, and the terms the `[zk_note]` table declares.
pub struct ZkEdge {
    pub vault: Arc<dyn ZkNoteVault>,
    pub evidence: EvidenceFile,
    /// The public half of the key this edge signs successor states with:
    /// a request proof must be about this key, and its x is the `edge` the
    /// binding tag is computed with.
    pub state_key: CompressedPubKey,
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

#[derive(Debug, Clone)]
struct OpenAllowance {
    tier: u64,
    expires_at_ms: u64,
    session_key: CompressedPubKey,
}

/// Every allowance granted, and every nullifier spent or being spent.
#[derive(Debug, Default)]
pub(super) struct ZkLedger {
    allowances: HashMap<AllowanceId, OpenAllowance>,
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
            let JournalEntry::AllowanceGranted {
                allowance_id,
                nullifier,
                evidence_hash,
                tier,
                expires_at_ms,
                session_key_x,
                session_key_is_odd,
                ..
            } = entry
            else {
                continue;
            };
            let corrupt = || JournalError::Corrupt(format!("allowance_granted {allowance_id}"));
            let id: AllowanceId = allowance_id.parse().map_err(|_| corrupt())?;
            let x = field_from_le_bytes(session_key_x).ok_or_else(corrupt)?;
            ledger.spent.insert(*nullifier);
            ledger.evidence.push((id, *evidence_hash));
            ledger.allowances.insert(
                id,
                OpenAllowance {
                    tier: *tier,
                    expires_at_ms: *expires_at_ms,
                    session_key: CompressedPubKey {
                        x,
                        is_odd: *session_key_is_odd,
                    },
                },
            );
        }
        Ok(ledger)
    }

    /// The budgets of open allowances, and of those being admitted. Every
    /// allowance granted is open until its close is journaled (#1523).
    fn open_budget(&self) -> u64 {
        self.allowances
            .values()
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
    /// evidence file still holds every one of them.
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
        self.zk_edge = Some(edge);
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
        if public.vault != edge.state_key {
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
            &edge.state_key,
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
                OpenAllowance {
                    tier,
                    expires_at_ms: request.expires_at_ms,
                    session_key: request.session_key,
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
    /// open and unexpired, the voucher rules and the budget under the read
    /// lock, then the session key's signature, then the authoritative
    /// re-check, the advance and the journal.
    pub(super) fn admit_allowance_claim(
        &self,
        claim: &AllowanceClaim,
        price: u64,
    ) -> Result<DurabilityTicket, ClaimIngestRejection> {
        let edge = self
            .zk_edge
            .as_ref()
            .ok_or(ClaimIngestRejection::UnknownAllowance)?;
        let allowance = self
            .zk_ledger
            .lock()
            .expect("zk ledger lock poisoned")
            .allowances
            .get(&claim.allowance_id)
            .cloned()
            .ok_or(ClaimIngestRejection::UnknownAllowance)?;
        if now_ms(edge.clock.as_ref()) >= allowance.expires_at_ms {
            return Err(ClaimIngestRejection::AllowanceExpired);
        }

        let key = claim.allowance_id.channel_key();
        let amount = u128::from(claim.cumulative_amount);
        let check = |current: Option<&LiveClaim>| {
            let watermark = current.map(|record| VoucherWatermark {
                cumulative_amount: record.watermark.cumulative_amount,
                signature: &record.signature,
            });
            validate_allowance_claim(watermark, amount, &claim.signature, price, allowance.tier)
                .map_err(claim_refusal)
        };
        {
            let watermarks = self
                .watermarks
                .read()
                .expect("client claim watermarks lock poisoned");
            if check(watermarks.get(&key))? == VoucherAdmission::Retransmission {
                drop(watermarks);
                return Ok(self.retransmission(&key));
            }
        }

        let signature = signature_from_be_bytes(&claim.signature)
            .ok_or(ClaimIngestRejection::SignatureInvalid)?;
        if !verify_allowance_claim(
            &signature,
            &allowance.session_key,
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
            return Ok(self.retransmission(&key));
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
        Ok(ticket)
    }
}

#[cfg(test)]
#[path = "allowance_tests.rs"]
mod tests;
