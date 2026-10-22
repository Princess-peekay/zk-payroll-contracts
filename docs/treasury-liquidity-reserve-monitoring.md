# Treasury Liquidity Reserve Monitoring (#650)

Reserve monitoring reports how much **free liquidity** the treasury has,
which is its token balance minus the funds locked for pending payroll runs,
against two admin-configured levels. It is observational only: payroll
execution is still governed by the
[reservation checks](./treasury-reservation-checks.md) and the
[balance buffer](./treasury-balance-buffer.md).

## Configuration

`set_liquidity_reserve_thresholds(admin, warning_reserve, critical_reserve)`
requires `0 <= critical_reserve <= warning_reserve` and is admin-only. It
emits `("payroll", "liq_reserve_set")`. `get_liquidity_reserve_thresholds()`
returns the current levels.

## Status

| Status | Condition |
|--------|-----------|
| `Healthy` | `free_liquidity >= warning_reserve` |
| `Warning` | `critical_reserve <= free_liquidity < warning_reserve` |
| `Critical` | `0 <= free_liquidity < critical_reserve` |
| `Deficit` | `free_liquidity < 0` (locked funds exceed the balance) |

Without configured levels both read as zero (`configured = false`), so only
`Healthy` or `Deficit` is reported.

## Entry points

- `get_liquidity_reserve_status(asset)`: read-only `LiquidityReserveReport`.
- `record_liquidity_reserve_check(asset)`: permissionless, so keepers can poll
  it. Stores the report and emits `("payroll", "liq_reserve_status")` with
  `(asset, previous, status, free_liquidity, checked_at)` whenever the status
  differs from the previously recorded check.
- `get_last_liquidity_reserve()`: the most recently recorded report.

## Errors (`LiquidityReserveError`)

| Code | Variant | Recovery |
|------|---------|----------|
| 6520 | `NotInitialized` | Initialize the contract. |
| 6521 | `Unauthorized` | Sign with the payroll admin. |
| 6522 | `NegativeReserve` | Use non-negative levels. |
| 6523 | `ReserveOrderInvalid` | Keep `critical_reserve <= warning_reserve`. |
| 6524 | `ForeignAsset` | Query the canonical treasury asset. |
| 6525 | `TreasuryUnavailable` | The token did not answer a balance query; retry or check the asset. |

Reports contain only aggregate treasury figures, never employee rows or
salary values.
