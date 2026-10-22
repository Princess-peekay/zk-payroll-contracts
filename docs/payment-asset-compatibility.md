# Payment Asset Compatibility Validation (#646)

`check_payment_asset_compat(asset, amount)` is a read-only preflight that
tells integrators whether `amount` can be paid in `asset` before they build a
payroll run. It returns a `PaymentAssetCompatibility` summary on success, or
the first rule the asset fails as a typed `PaymentAssetCompatibilityError`.

## Rules

Rules are applied in this fixed order, so the same input always yields the
same error:

| Order | Rule | Error (code) | Recovery |
|-------|------|--------------|----------|
| 1 | Payroll is initialized | `NotInitialized` (6460) | Initialize the contract. |
| 2 | `asset` is the canonical treasury asset (`get_addresses().token`) | `ForeignAsset` (6461) | Pay with the canonical asset. |
| 3 | The asset is allowed | `AssetNotAllowed` (6462) | Admin calls `set_asset_allowed(asset, true)`. |
| 4 | The asset is not deprecated | `AssetDeprecated` (6463) | Admin calls `undeprecate_asset`, or migrate assets. |
| 5 | `amount > 0` | `NonPositiveAmount` (6464) | Submit a positive amount. |
| 6 | Token decimals match the pinned precision (only when pinned) | `AssetUnreachable` (6465) / `DecimalsMismatch` (6466) | Check the token contract, or correct the pin. |

## Precision pinning

Payroll amounts are integers in the token's base units, so a token with a
different number of decimals would silently scale every payout. The admin can
pin the expected precision:

- `set_payment_asset_decimals(admin, decimals)`: `decimals` must be `<= 18`
  (`InvalidDecimals`, 6467). Only the admin may call it (`Unauthorized`, 6468).
- `clear_payment_asset_decimals(admin)` removes the pin.
- `get_payment_asset_decimals()` returns the pin, if any.

While a precision is pinned, `prepare_payroll_run` also rejects a token whose
decimals differ (`"Payment asset precision mismatch: ..."`) or that does not
answer a `decimals` query (`"Payment asset precision unavailable: ..."`).
Without a pin, run preparation behaves exactly as before.

Both setters emit `("payroll", "asset_precision_set")` with the new pin
(`None` when cleared) and the ledger timestamp. No salary data is read or
emitted.
