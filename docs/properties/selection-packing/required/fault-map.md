# Required-phase fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `d3d7663b`.

| Fault or state | Available seam | Marker | Records |
| --- | --- | --- | --- |
| Required identifier never persisted | A `RequiredRequest` for an unpersisted span. | `packing.required.missing_required_presented` | packing-required-phase-precedes-optional-work |
| Required revision differs, or row tombstoned | A request at revision 2 against a revision 1 row; `tombstone_occurrence`. | `packing.required.stale_required_presented` | packing-required-phase-precedes-optional-work |
| Kernel excludes the source object | A kernel seeded without the row's object (`Retracted`); the hidden, stale, and superseded verdicts as `Disposition` inputs at the pure layer. | `packing.required.excluded_required_presented` | packing-required-phase-precedes-optional-work |
| Payload bytes, tuple, or eligibility digest rewritten | A raw connection altering `payloads.bytes`, `occurrences.tuple`, or `occurrences.source_artifact_digest`. | `packing.required.corrupt_required_presented` | packing-required-phase-precedes-optional-work |
| Per-item, load-count, or byte-total bound below the set | `RequiredBounds` fixture values. | `packing.required.oversized_required_presented` | packing-required-phase-precedes-optional-work |
| Token limit one below the required cost | `byte_profile()` with `token_limit = total - 1`. | `packing.required.over_budget_required_presented` | packing-required-phase-precedes-optional-work, packing-required-bytes-are-charged-untruncated |
| `EvalBudget` expired or cancelled before the phase, or cancelled inside the profile's count | `EvalBudget::new` with a past deadline; `EvalBudget::cancel`, including from a heuristic profile's count function. | `packing.required.budget_exhausted_before_optional` | packing-required-phase-precedes-optional-work |
| Malformed floating budget | `ClaudeTokens::from_budget` with NaN, negative, fractional, infinite, 2^53-and-above, or absent values; the legacy trim with the same values. | `packing.budget.malformed_budget_presented` | packing-budget-is-an-integer-never-clamped |
| Payload longer than 64 KiB | A 64 KiB + 7 payload through the estimator and the legacy memory line. | `packing.required.payload_beyond_legacy_cut` | packing-required-bytes-are-charged-untruncated |

Every seam is an in-process store, a pure function, or a constructed budget;
no fault injection framework is needed for this part. Marker status lives in
`../marker-ledger.md`.
