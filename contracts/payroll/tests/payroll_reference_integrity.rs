//! Contract payroll reference integrity validation (#649).

#![cfg(test)]

mod common;

use payroll::{
    DataKey, PayrollClient, PayrollReferenceError, PendingPayrollRun, RunDraftState,
    RunReferenceLocation,
};
use soroban_sdk::{Address, BytesN, Env, Symbol};

fn prepare(
    env: &Env,
    client: &PayrollClient,
    employee: &Address,
    marker: u8,
    draft_hash: Option<BytesN<32>>,
) -> u64 {
    let (proofs, amounts, employees) = common::one_payment(env, employee);
    client.prepare_payroll_run(
        &proofs,
        &amounts,
        &employees,
        &100,
        &common::nonce(env, marker),
        &draft_hash,
    )
}

// ── Runs ─────────────────────────────────────────────────────────────────────

#[test]
fn run_references_follow_the_run_lifecycle() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);
    let admin = client.get_addresses().admin;

    let run_id = prepare(&env, &client, &employee, 1, None);
    let pending = client.check_run_references(&run_id);
    assert_eq!(pending.run_id, run_id);
    assert_eq!(pending.location, RunReferenceLocation::Pending);
    assert!(!pending.draft_hash_committed);

    client.finalize_payroll_run(&admin, &run_id);
    let completed = client.check_run_references(&run_id);
    assert_eq!(completed.location, RunReferenceLocation::Completed);

    let cancelled = prepare(&env, &client, &employee, 2, None);
    client.cancel_payroll_run(&admin, &cancelled, &Symbol::new(&env, "test"));
    assert_eq!(
        client.check_run_references(&cancelled).location,
        RunReferenceLocation::Released
    );
}

#[test]
fn committed_draft_hash_is_reported() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);
    let admin = client.get_addresses().admin;
    let draft_hash = BytesN::from_array(&env, &[7; 32]);
    client.commit_draft(&admin, &draft_hash);

    let run_id = prepare(&env, &client, &employee, 1, Some(draft_hash));
    assert!(client.check_run_references(&run_id).draft_hash_committed);
}

#[test]
fn unissued_run_ids_are_rejected() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);

    assert_eq!(
        client.try_check_run_references(&0),
        Err(Ok(PayrollReferenceError::InvalidReferenceId))
    );
    assert_eq!(
        client.try_check_run_references(&1),
        Err(Ok(PayrollReferenceError::RunNotIssued))
    );

    // The boundary: the latest issued id is valid, the next one is not.
    let run_id = prepare(&env, &client, &employee, 1, None);
    assert!(client.try_check_run_references(&run_id).is_ok());
    assert_eq!(
        client.try_check_run_references(&(run_id + 1)),
        Err(Ok(PayrollReferenceError::RunNotIssued))
    );
}

#[test]
fn dangling_nonce_references_are_detected() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);
    let run_id = prepare(&env, &client, &employee, 1, None);
    let nonce_key = DataKey::RunNonce(common::nonce(&env, 1));

    env.as_contract(&client.address, || {
        env.storage().persistent().set(&nonce_key, &(run_id + 5));
    });
    assert_eq!(
        client.try_check_run_references(&run_id),
        Err(Ok(PayrollReferenceError::NonceReferenceMismatch))
    );

    env.as_contract(&client.address, || {
        env.storage().persistent().remove(&nonce_key);
    });
    assert_eq!(
        client.try_check_run_references(&run_id),
        Err(Ok(PayrollReferenceError::NonceReferenceMissing))
    );
}

#[test]
fn missing_or_mismatched_run_records_are_detected() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);
    let run_id = prepare(&env, &client, &employee, 1, None);
    let key = DataKey::PendingRun(run_id);

    env.as_contract(&client.address, || {
        let mut run: PendingPayrollRun = env.storage().persistent().get(&key).unwrap();
        run.run_id += 1;
        env.storage().persistent().set(&key, &run);
    });
    assert_eq!(
        client.try_check_run_references(&run_id),
        Err(Ok(PayrollReferenceError::RunIdMismatch))
    );

    // Removing the record without a cancelled/expired state leaves the id
    // dangling.
    env.as_contract(&client.address, || {
        env.storage().persistent().remove(&key);
    });
    assert_eq!(
        client.try_check_run_references(&run_id),
        Err(Ok(PayrollReferenceError::RunRecordMissing))
    );
}

#[test]
fn uncommitted_draft_hash_is_detected() {
    let env = Env::default();
    let (client, _token, employee) = common::setup(&env);
    let admin = client.get_addresses().admin;
    let draft_hash = BytesN::from_array(&env, &[9; 32]);
    client.commit_draft(&admin, &draft_hash);
    let run_id = prepare(&env, &client, &employee, 1, Some(draft_hash.clone()));

    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .remove(&DataKey::DraftCommitment(draft_hash));
    });
    assert_eq!(
        client.try_check_run_references(&run_id),
        Err(Ok(PayrollReferenceError::DraftCommitmentMissing))
    );
}

// ── Drafts ───────────────────────────────────────────────────────────────────

#[test]
fn draft_references_follow_the_draft_lifecycle() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);
    let admin = client.get_addresses().admin;
    let period = Symbol::new(&env, "P1");
    let draft_id = client.create_run_draft(&admin, &1_000, &3, &period);

    let pending = client.check_draft_references(&draft_id);
    assert_eq!(pending.state, RunDraftState::Pending);
    assert_eq!(pending.period_label, period);
    assert!(pending.holds_period_slot);

    client.finalize_run_draft(&admin, &draft_id);
    assert!(client.check_draft_references(&draft_id).holds_period_slot);

    client.submit_run_draft(&admin, &draft_id);
    let submitted = client.check_draft_references(&draft_id);
    assert_eq!(submitted.state, RunDraftState::Submitted);
    assert!(!submitted.holds_period_slot);
}

#[test]
fn unknown_drafts_are_rejected() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);

    assert_eq!(
        client.try_check_draft_references(&0),
        Err(Ok(PayrollReferenceError::InvalidReferenceId))
    );
    assert_eq!(
        client.try_check_draft_references(&42),
        Err(Ok(PayrollReferenceError::DraftNotFound))
    );
}

#[test]
fn period_slot_disagreements_are_detected() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);
    let admin = client.get_addresses().admin;
    let period = Symbol::new(&env, "P1");
    let draft_id = client.create_run_draft(&admin, &1_000, &3, &period);
    let slot = DataKey::ActiveDraftForPeriod(period);

    // An open draft that lost its slot.
    env.as_contract(&client.address, || {
        env.storage().persistent().remove(&slot);
    });
    assert_eq!(
        client.try_check_draft_references(&draft_id),
        Err(Ok(PayrollReferenceError::PeriodSlotMismatch))
    );

    // A cancelled draft that still holds its slot would block the period.
    client.cancel_run_draft(&admin, &draft_id);
    env.as_contract(&client.address, || {
        env.storage().persistent().set(&slot, &draft_id);
    });
    assert_eq!(
        client.try_check_draft_references(&draft_id),
        Err(Ok(PayrollReferenceError::PeriodSlotMismatch))
    );
}
