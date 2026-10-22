//! Contract payroll reference integrity validation (#649).
//!
//! Payroll runs and drafts are linked to each other through several storage
//! references: the run counter, pending and completed run records, the
//! consumed run nonce (which stores the run id it was used by), the
//! pre-committed draft hash, and the per-period active draft slot. Each
//! write path maintains its own piece of that graph; this module checks the
//! pieces agree, so integrators and auditors can detect a dangling or
//! contradictory reference before relying on a run or draft.
//!
//! The checks are read-only, emit no events, and expose only identifiers and
//! booleans; no salary values or employee rows are read.

use soroban_sdk::{contracterror, contractimpl, contracttype, BytesN, Env, Symbol};

use crate::{
    DataKey, Payroll, PayrollArgs, PayrollClient, PayrollRun, PayrollRunDraft, PayrollRunState,
    PendingPayrollRun, RunDraftState,
};

/// Where a run's authoritative record lives.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RunReferenceLocation {
    /// Prepared and awaiting finalization (`PendingRun`).
    Pending = 0,
    /// Executed (`PayrollRun`).
    Completed = 1,
    /// Cancelled or expired: the record was intentionally released and only
    /// the run state remains.
    Released = 2,
}

/// Summary of a run whose references are consistent.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReferenceIntegrity {
    pub run_id: u64,
    pub location: RunReferenceLocation,
    /// `true` when the run is bound to a non-zero draft hash that has a
    /// matching pre-commitment. `false` when no draft hash is bound.
    pub draft_hash_committed: bool,
}

/// Summary of a draft whose references are consistent.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftReferenceIntegrity {
    pub draft_id: u64,
    pub state: RunDraftState,
    pub period_label: Symbol,
    /// `true` when this draft currently holds its period's active slot.
    pub holds_period_slot: bool,
}

/// Errors returned by payroll reference integrity validation (#649).
#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PayrollReferenceError {
    /// Run and draft ids start at 1; `0` is never a valid reference.
    InvalidReferenceId = 6490,
    /// The run id is above the run counter, so no run was ever issued it.
    RunNotIssued = 6491,
    /// The run id was issued but has no record and no terminal state.
    RunRecordMissing = 6492,
    /// The run exists as both a pending and a completed record.
    RunRecordDuplicated = 6493,
    /// The stored record's `run_id` field does not match its storage key.
    RunIdMismatch = 6494,
    /// The run's nonce is not registered as consumed.
    NonceReferenceMissing = 6495,
    /// The run's nonce is registered as consumed by a different run.
    NonceReferenceMismatch = 6496,
    /// The run is bound to a draft hash that was never pre-committed.
    DraftCommitmentMissing = 6497,
    /// No draft exists for this id.
    DraftNotFound = 6498,
    /// The stored draft's `draft_id` field does not match its storage key.
    DraftIdMismatch = 6499,
    /// The period's active-draft slot disagrees with the draft's state: an
    /// open (`Pending`/`Finalized`) draft does not hold its slot, or a
    /// submitted, cancelled, or expired draft still holds it (which would
    /// block new drafts for the period).
    PeriodSlotMismatch = 6500,
}

fn zero_hash(e: &Env) -> BytesN<32> {
    BytesN::from_array(e, &[0u8; 32])
}

/// Validate every stored reference for `run_id`.
pub fn validate_run_references(
    e: &Env,
    run_id: u64,
) -> Result<RunReferenceIntegrity, PayrollReferenceError> {
    if run_id == 0 {
        return Err(PayrollReferenceError::InvalidReferenceId);
    }
    let counter: u64 = e
        .storage()
        .persistent()
        .get(&DataKey::RunCounter)
        .unwrap_or(0);
    if run_id > counter {
        return Err(PayrollReferenceError::RunNotIssued);
    }

    let storage = e.storage().persistent();
    let pending: Option<PendingPayrollRun> = storage.get(&DataKey::PendingRun(run_id));
    let completed: Option<PayrollRun> = storage.get(&DataKey::PayrollRun(run_id));

    let (location, record_id, nonce, draft_hash) = match (pending, completed) {
        (Some(_), Some(_)) => return Err(PayrollReferenceError::RunRecordDuplicated),
        (Some(run), None) => (
            RunReferenceLocation::Pending,
            run.run_id,
            run.nonce,
            run.draft_hash,
        ),
        (None, Some(run)) => (
            RunReferenceLocation::Completed,
            run.run_id,
            run.nonce,
            run.draft_hash,
        ),
        (None, None) => {
            let state: Option<PayrollRunState> = storage.get(&DataKey::PayrollState(run_id));
            return match state {
                Some(PayrollRunState::Cancelled) | Some(PayrollRunState::Expired) => {
                    Ok(RunReferenceIntegrity {
                        run_id,
                        location: RunReferenceLocation::Released,
                        draft_hash_committed: false,
                    })
                }
                _ => Err(PayrollReferenceError::RunRecordMissing),
            };
        }
    };

    if record_id != run_id {
        return Err(PayrollReferenceError::RunIdMismatch);
    }

    let nonce_owner: Option<u64> = storage.get(&DataKey::RunNonce(nonce));
    match nonce_owner {
        None => return Err(PayrollReferenceError::NonceReferenceMissing),
        Some(owner) if owner != run_id => {
            return Err(PayrollReferenceError::NonceReferenceMismatch)
        }
        Some(_) => {}
    }

    let draft_hash_committed = if draft_hash == zero_hash(e) {
        false
    } else if storage.has(&DataKey::DraftCommitment(draft_hash)) {
        true
    } else {
        return Err(PayrollReferenceError::DraftCommitmentMissing);
    };

    Ok(RunReferenceIntegrity {
        run_id,
        location,
        draft_hash_committed,
    })
}

/// Validate every stored reference for `draft_id`.
pub fn validate_draft_references(
    e: &Env,
    draft_id: u64,
) -> Result<DraftReferenceIntegrity, PayrollReferenceError> {
    if draft_id == 0 {
        return Err(PayrollReferenceError::InvalidReferenceId);
    }
    let storage = e.storage().persistent();
    let draft: PayrollRunDraft = storage
        .get(&DataKey::RunDraft(draft_id))
        .ok_or(PayrollReferenceError::DraftNotFound)?;
    if draft.draft_id != draft_id {
        return Err(PayrollReferenceError::DraftIdMismatch);
    }

    let slot: Option<u64> = storage.get(&DataKey::ActiveDraftForPeriod(draft.period_label.clone()));
    let holds_period_slot = slot == Some(draft_id);
    // A draft keeps its period slot from creation until it is submitted,
    // cancelled, or expired, so only open drafts may (and must) hold it.
    let is_open = matches!(
        draft.state,
        RunDraftState::Pending | RunDraftState::Finalized
    );
    if is_open != holds_period_slot {
        return Err(PayrollReferenceError::PeriodSlotMismatch);
    }

    Ok(DraftReferenceIntegrity {
        draft_id,
        state: draft.state,
        period_label: draft.period_label,
        holds_period_slot,
    })
}

#[contractimpl]
impl Payroll {
    /// Validate the stored references of a payroll run (#649).
    ///
    /// Confirms the run id was issued, exactly one run record exists (or the
    /// run was cancelled/expired), the record's id matches, its nonce is
    /// registered to this run, and any bound draft hash was pre-committed.
    pub fn check_run_references(
        e: Env,
        run_id: u64,
    ) -> Result<RunReferenceIntegrity, PayrollReferenceError> {
        validate_run_references(&e, run_id)
    }

    /// Validate the stored references of a payroll run draft (#649).
    ///
    /// Confirms the draft exists, its id matches, and the period's active
    /// draft slot points at it exactly while it is `Pending` or `Finalized`.
    pub fn check_draft_references(
        e: Env,
        draft_id: u64,
    ) -> Result<DraftReferenceIntegrity, PayrollReferenceError> {
        validate_draft_references(&e, draft_id)
    }
}
