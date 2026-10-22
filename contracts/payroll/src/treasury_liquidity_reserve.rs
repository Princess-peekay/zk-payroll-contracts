//! Contract treasury liquidity reserve monitoring (#650).
//!
//! The admin configures two reserve levels for the treasury's free liquidity
//! (token balance minus funds locked for pending payroll runs): a `warning`
//! level and a lower `critical` level. Anyone may then read the current
//! reserve status, and keepers or dashboards may record a check, which
//! stores the latest report and emits an event whenever the status moves
//! between `Healthy`, `Warning`, `Critical`, and `Deficit`.
//!
//! Monitoring never blocks payroll: enforcement stays with the existing
//! reservation and balance-buffer guards. Reports carry only aggregate
//! treasury figures, never employee rows or salary values.

use soroban_sdk::{contracterror, contractevent, contractimpl, contracttype, token, Address, Env};

use crate::{ContractAddresses, DataKey, Payroll, PayrollArgs, PayrollClient};

/// Storage keys for liquidity reserve monitoring.
///
/// Kept separate from [`DataKey`], which is past the Soroban contract-spec
/// ceiling of 50 cases.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiquidityReserveKey {
    /// Admin-configured [`LiquidityReserveThresholds`].
    Thresholds,
    /// Most recent recorded [`LiquidityReserveReport`].
    LastReport,
}

/// Admin-configured reserve levels, in the treasury asset's base units.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiquidityReserveThresholds {
    /// Free liquidity below this level is reported as `Warning`.
    pub warning_reserve: i128,
    /// Free liquidity below this level is reported as `Critical`.
    pub critical_reserve: i128,
    pub updated_at: u64,
    pub updated_by: Address,
}

/// Reserve health derived from free liquidity and the configured levels.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum LiquidityReserveStatus {
    /// Free liquidity is at or above the warning reserve.
    Healthy = 0,
    /// Free liquidity is below the warning reserve but at or above critical.
    Warning = 1,
    /// Free liquidity is below the critical reserve but not negative.
    Critical = 2,
    /// Locked payroll funds exceed the treasury balance.
    Deficit = 3,
}

/// Aggregate liquidity reserve report for the treasury asset.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiquidityReserveReport {
    pub asset: Address,
    pub treasury_balance: i128,
    pub locked_funds: i128,
    /// `treasury_balance - locked_funds`; negative in a deficit.
    pub free_liquidity: i128,
    pub warning_reserve: i128,
    pub critical_reserve: i128,
    /// `false` when no thresholds are set; both levels then read as zero.
    pub configured: bool,
    pub status: LiquidityReserveStatus,
    pub checked_at: u64,
}

/// Errors returned by treasury liquidity reserve monitoring (#650).
#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum LiquidityReserveError {
    /// Payroll has not been initialized.
    NotInitialized = 6520,
    /// The caller is not the configured payroll admin.
    Unauthorized = 6521,
    /// Reserve levels cannot be negative.
    NegativeReserve = 6522,
    /// The critical reserve must not exceed the warning reserve.
    ReserveOrderInvalid = 6523,
    /// The asset is not this contract's canonical treasury asset.
    ForeignAsset = 6524,
    /// The token did not answer a balance query for the treasury.
    TreasuryUnavailable = 6525,
}

/// Emitted as `("payroll", "liq_reserve_set")` when thresholds change.
#[contractevent(topics = ["payroll", "liq_reserve_set"], data_format = "vec")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityReserveThresholdsSet {
    pub warning_reserve: i128,
    pub critical_reserve: i128,
    pub updated_at: u64,
}

/// Emitted as `("payroll", "liq_reserve_status")` when a recorded check
/// observes a different status than the previous recorded check (or the
/// first check ever recorded).
#[contractevent(topics = ["payroll", "liq_reserve_status"], data_format = "vec")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityReserveStatusChanged {
    pub asset: Address,
    pub previous: Option<LiquidityReserveStatus>,
    pub status: LiquidityReserveStatus,
    pub free_liquidity: i128,
    pub checked_at: u64,
}

fn load_addresses(e: &Env) -> Result<ContractAddresses, LiquidityReserveError> {
    e.storage()
        .persistent()
        .get(&DataKey::Addresses)
        .ok_or(LiquidityReserveError::NotInitialized)
}

fn load_thresholds(e: &Env) -> Option<LiquidityReserveThresholds> {
    e.storage()
        .persistent()
        .get(&LiquidityReserveKey::Thresholds)
}

/// Classify free liquidity against the reserve levels.
pub fn classify(
    free_liquidity: i128,
    warning_reserve: i128,
    critical_reserve: i128,
) -> LiquidityReserveStatus {
    if free_liquidity < 0 {
        LiquidityReserveStatus::Deficit
    } else if free_liquidity < critical_reserve {
        LiquidityReserveStatus::Critical
    } else if free_liquidity < warning_reserve {
        LiquidityReserveStatus::Warning
    } else {
        LiquidityReserveStatus::Healthy
    }
}

/// Build a reserve report for `asset` from live treasury data.
pub fn build_report(
    e: &Env,
    asset: &Address,
) -> Result<LiquidityReserveReport, LiquidityReserveError> {
    let addrs = load_addresses(e)?;
    if asset != &addrs.token {
        return Err(LiquidityReserveError::ForeignAsset);
    }
    let treasury_balance = match token::Client::new(e, asset).try_balance(&addrs.treasury) {
        Ok(Ok(balance)) => balance,
        _ => return Err(LiquidityReserveError::TreasuryUnavailable),
    };
    let locked_funds = Payroll::get_locked_funds(e.clone(), asset.clone());
    let free_liquidity = treasury_balance.saturating_sub(locked_funds);
    let thresholds = load_thresholds(e);
    let (warning_reserve, critical_reserve) = thresholds
        .as_ref()
        .map(|t| (t.warning_reserve, t.critical_reserve))
        .unwrap_or((0, 0));

    Ok(LiquidityReserveReport {
        asset: asset.clone(),
        treasury_balance,
        locked_funds,
        free_liquidity,
        warning_reserve,
        critical_reserve,
        configured: thresholds.is_some(),
        status: classify(free_liquidity, warning_reserve, critical_reserve),
        checked_at: e.ledger().timestamp(),
    })
}

#[contractimpl]
impl Payroll {
    /// Configure the treasury liquidity reserve levels (#650). Admin only.
    ///
    /// Requires `0 <= critical_reserve <= warning_reserve`. Setting both to
    /// zero keeps monitoring on but only reports `Healthy` or `Deficit`.
    pub fn set_liquidity_reserve_thresholds(
        e: Env,
        admin: Address,
        warning_reserve: i128,
        critical_reserve: i128,
    ) -> Result<LiquidityReserveThresholds, LiquidityReserveError> {
        let addrs = load_addresses(&e)?;
        if admin != addrs.admin {
            return Err(LiquidityReserveError::Unauthorized);
        }
        admin.require_auth();
        if warning_reserve < 0 || critical_reserve < 0 {
            return Err(LiquidityReserveError::NegativeReserve);
        }
        if critical_reserve > warning_reserve {
            return Err(LiquidityReserveError::ReserveOrderInvalid);
        }

        let thresholds = LiquidityReserveThresholds {
            warning_reserve,
            critical_reserve,
            updated_at: e.ledger().timestamp(),
            updated_by: admin,
        };
        e.storage()
            .persistent()
            .set(&LiquidityReserveKey::Thresholds, &thresholds);
        LiquidityReserveThresholdsSet {
            warning_reserve,
            critical_reserve,
            updated_at: thresholds.updated_at,
        }
        .publish(&e);
        Ok(thresholds)
    }

    /// Return the configured liquidity reserve levels, if any (#650).
    pub fn get_liquidity_reserve_thresholds(e: Env) -> Option<LiquidityReserveThresholds> {
        load_thresholds(&e)
    }

    /// Read-only: current liquidity reserve report for `asset` (#650).
    pub fn get_liquidity_reserve_status(
        e: Env,
        asset: Address,
    ) -> Result<LiquidityReserveReport, LiquidityReserveError> {
        build_report(&e, &asset)
    }

    /// Record a liquidity reserve check for `asset` (#650).
    ///
    /// Permissionless so keepers can poll it: it only persists a report
    /// derived from on-chain data. Emits `liq_reserve_status` when the status
    /// differs from the previously recorded check.
    pub fn record_liquidity_reserve_check(
        e: Env,
        asset: Address,
    ) -> Result<LiquidityReserveReport, LiquidityReserveError> {
        let report = build_report(&e, &asset)?;
        let previous: Option<LiquidityReserveReport> = e
            .storage()
            .persistent()
            .get(&LiquidityReserveKey::LastReport);
        let previous_status = previous.map(|r| r.status);
        e.storage()
            .persistent()
            .set(&LiquidityReserveKey::LastReport, &report);
        if previous_status != Some(report.status) {
            LiquidityReserveStatusChanged {
                asset: report.asset.clone(),
                previous: previous_status,
                status: report.status,
                free_liquidity: report.free_liquidity,
                checked_at: report.checked_at,
            }
            .publish(&e);
        }
        Ok(report)
    }

    /// Return the most recently recorded liquidity reserve report (#650).
    pub fn get_last_liquidity_reserve(e: Env) -> Option<LiquidityReserveReport> {
        e.storage()
            .persistent()
            .get(&LiquidityReserveKey::LastReport)
    }
}
