# Payroll Reference Integrity Validation (#649)

Runs and drafts are linked through several storage references that are each
written by a different flow. Two read-only entrypoints check that those
references agree, so integrators and auditors can detect a dangling or
contradictory reference before relying on a run or draft. Neither writes
state or emits events.

## `check_run_references(run_id)`

Returns a `RunReferenceIntegrity { run_id, location, draft_hash_committed }`
where `location` is `Pending`, `Completed`, or `Released` (cancelled or
expired runs, whose record is intentionally removed). Checks, in order:

| Code | Error | Meaning |
|------|-------|---------|
| 6490 | `InvalidReferenceId` | `run_id` is `0`. |
| 6491 | `RunNotIssued` | `run_id` is above the run counter. |
| 6493 | `RunRecordDuplicated` | The run has both a pending and a completed record. |
| 6492 | `RunRecordMissing` | No record exists and the run is not cancelled/expired. |
| 6494 | `RunIdMismatch` | The stored record's `run_id` differs from its key. |
| 6495 | `NonceReferenceMissing` | The run's nonce is not registered as consumed. |
| 6496 | `NonceReferenceMismatch` | The nonce is registered to a different run. |
| 6497 | `DraftCommitmentMissing` | The bound draft hash was never pre-committed. |

## `check_draft_references(draft_id)`

Returns a `DraftReferenceIntegrity { draft_id, state, period_label, holds_period_slot }`.
A draft holds its period's active-draft slot from creation until it is
submitted, cancelled, or expired.

| Code | Error | Meaning |
|------|-------|---------|
| 6490 | `InvalidReferenceId` | `draft_id` is `0`. |
| 6498 | `DraftNotFound` | No draft exists for this id. |
| 6499 | `DraftIdMismatch` | The stored draft's `draft_id` differs from its key. |
| 6500 | `PeriodSlotMismatch` | An open draft does not hold its period slot, or a closed draft still does (which would block new drafts for the period). |

Results expose only identifiers, lifecycle state, and booleans; no salary
values or employee rows are read.
