# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `016c7127`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/daemon/src/dispatch.rs` guard tests | `PreparedOutput::measure` refuses an oversized body and `write_to` reports a length mismatch. | unaudited | The guard alone, not a packed body passing through it. |
| `crates/daemon/tests/projection_gates.rs`, refusal tests | Missing, unknown, non-numeric, and version-mismatched limits fail at parse. | unaudited | The frozen required set and the vector group; the packing group is new. |
| `crates/daemon/tests/projection_gates.rs`, `gate_tables_match_the_frozen_construction_contract` | The gate's required limits equal the construction contract's. | unaudited | Keeps the packing names out of the frozen set; does not read them. |
| `crates/daemon/tests/packing_accounting.rs`, `rendered_bytes_and_estimated_tokens_bounds_refuse_at_limit_plus_one` | Accounting bounds refuse one past the closed render. | unaudited | Refusal without repair; adjustment is new. |

Suspiciously quiet areas: no check compared a body written through the guard
to the ledger it came from, and no check exercised output identity across
cache states before this part.
