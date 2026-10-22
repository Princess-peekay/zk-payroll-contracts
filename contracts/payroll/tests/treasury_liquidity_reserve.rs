//! Contract treasury liquidity reserve monitoring (#650).
//!
//! `common::setup` funds the treasury with 1_000_000 units and prepares no
//! runs, so free liquidity starts at 1_000_000.

#![cfg(test)]

mod common;

use payroll::{
    ContractAddresses, DataKey, LiquidityReserveError, LiquidityReserveStatus, Payroll,
    PayrollClient,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

const FUNDED: i128 = 1_000_000;

fn prepare(env: &Env, client: &PayrollClient, employee: &Address, marker: u8) -> u64 {
    let (proofs, amounts, employees) = common::one_payment(env, employee);
    client.prepare_payroll_run(
        &proofs,
        &amounts,
        &employees,
        &100,
        &common::nonce(env, marker),
        &None,
    )
}

#[test]
fn unconfigured_monitoring_reports_healthy_free_liquidity() {
    let env = Env::default();
    let (client, token, _employee) = common::setup(&env);

    assert_eq!(client.get_liquidity_reserve_thresholds(), None);
    let report = client.get_liquidity_reserve_status(&token);
    assert_eq!(report.asset, token);
    assert_eq!(report.treasury_balance, FUNDED);
    assert_eq!(report.locked_funds, 0);
    assert_eq!(report.free_liquidity, FUNDED);
    assert!(!report.configured);
    assert_eq!(report.status, LiquidityReserveStatus::Healthy);
}

#[test]
fn locked_payroll_funds_move_the_status_through_each_level() {
    let env = Env::default();
    let (client, token, employee) = common::setup(&env);
    let admin = client.get_addresses().admin;

    // Free liquidity exactly at the warning level is still healthy.
    client.set_liquidity_reserve_thresholds(&admin, &FUNDED, &(FUNDED - 150));
    let report = client.record_liquidity_reserve_check(&token);
    assert!(report.configured);
    assert_eq!(report.status, LiquidityReserveStatus::Healthy);

    prepare(&env, &client, &employee, 1);
    let report = client.record_liquidity_reserve_check(&token);
    assert_eq!(report.locked_funds, 100);
    assert_eq!(report.free_liquidity, FUNDED - 100);
    assert_eq!(report.status, LiquidityReserveStatus::Warning);

    prepare(&env, &client, &employee, 2);
    let report = client.record_liquidity_reserve_check(&token);
    assert_eq!(report.free_liquidity, FUNDED - 200);
    assert_eq!(report.status, LiquidityReserveStatus::Critical);
    assert_eq!(client.get_last_liquidity_reserve(), Some(report));
}

#[test]
fn locked_funds_above_the_balance_report_a_deficit() {
    let env = Env::default();
    let (client, token, _employee) = common::setup(&env);

    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .set(&DataKey::LockedPayrollFunds(token.clone()), &(FUNDED + 1));
    });
    let report = client.record_liquidity_reserve_check(&token);
    assert_eq!(report.free_liquidity, -1);
    assert_eq!(report.status, LiquidityReserveStatus::Deficit);
}

#[test]
fn reserve_levels_are_validated() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);
    let admin = client.get_addresses().admin;

    assert_eq!(
        client.try_set_liquidity_reserve_thresholds(&Address::generate(&env), &10, &5),
        Err(Ok(LiquidityReserveError::Unauthorized))
    );
    assert_eq!(
        client.try_set_liquidity_reserve_thresholds(&admin, &-1, &0),
        Err(Ok(LiquidityReserveError::NegativeReserve))
    );
    assert_eq!(
        client.try_set_liquidity_reserve_thresholds(&admin, &5, &10),
        Err(Ok(LiquidityReserveError::ReserveOrderInvalid))
    );
    assert_eq!(client.get_liquidity_reserve_thresholds(), None);

    // Equal levels are allowed: there is simply no warning band.
    let set = client.set_liquidity_reserve_thresholds(&admin, &10, &10);
    assert_eq!(set.warning_reserve, 10);
    assert_eq!(set.critical_reserve, 10);
    assert_eq!(set.updated_by, admin);
    assert_eq!(client.get_liquidity_reserve_thresholds(), Some(set));
}

#[test]
fn foreign_assets_and_uninitialized_contracts_are_rejected() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);

    assert_eq!(
        client.try_get_liquidity_reserve_status(&Address::generate(&env)),
        Err(Ok(LiquidityReserveError::ForeignAsset))
    );
    assert_eq!(
        client.try_record_liquidity_reserve_check(&Address::generate(&env)),
        Err(Ok(LiquidityReserveError::ForeignAsset))
    );
    assert_eq!(client.get_last_liquidity_reserve(), None);

    let bare = PayrollClient::new(&env, &env.register(Payroll, ()));
    assert_eq!(
        bare.try_get_liquidity_reserve_status(&Address::generate(&env)),
        Err(Ok(LiquidityReserveError::NotInitialized))
    );
}

#[test]
fn unreachable_treasury_token_is_reported() {
    let env = Env::default();
    let (client, _token, _employee) = common::setup(&env);

    // Point the configured token at an address with no deployed contract,
    // as if the token had become unreachable after initialization.
    let gone = Address::generate(&env);
    env.as_contract(&client.address, || {
        let mut addrs: ContractAddresses =
            env.storage().persistent().get(&DataKey::Addresses).unwrap();
        addrs.token = gone.clone();
        env.storage().persistent().set(&DataKey::Addresses, &addrs);
    });

    assert_eq!(
        client.try_get_liquidity_reserve_status(&gone),
        Err(Ok(LiquidityReserveError::TreasuryUnavailable))
    );
    assert_eq!(client.get_last_liquidity_reserve(), None);
}
