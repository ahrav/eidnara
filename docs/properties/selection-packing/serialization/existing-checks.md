# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `016c7127`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/daemon/tests/prepared_output.rs`, `cached_bytes_copy_only_after_destination_reservation`, `cap_plus_one_and_arithmetic_overflow_fail_before_write`, `inconsistent_source_reports_length_mismatch_without_emission`, `destination_failure_retains_no_partial_terminal` | `PreparedOutput::measure` refuses a body past the wire cap before any write, `write_to` copies only after the destination is reserved, reports a length mismatch, and leaves no partial terminal on a destination failure. | unaudited | The guard alone, not a packed body passing through it. |
| `crates/daemon/tests/projection_gates.rs`, refusal tests | Missing, unknown, non-numeric, and version-mismatched limits fail at parse. | unaudited | The frozen required set and the vector group; the packing group is new. |
| `crates/daemon/tests/projection_gates.rs`, `gate_tables_match_the_frozen_construction_contract` | The gate's required limits equal the construction contract's. | unaudited | Keeps the packing names out of the frozen set; does not read them. |
| `crates/daemon/tests/packing_accounting.rs`, `rendered_bytes_and_estimated_tokens_bounds_refuse_at_limit_plus_one` | Accounting bounds refuse one past the closed render. | unaudited | Refusal without repair; adjustment is new. |
| `crates/daemon/src/token_cache.rs`, `a_count_cached_under_one_revision_is_not_served_under_another` | A count cached under one accounting revision is not served under another. | unaudited | Owned by the accounting part; the byte-identical record relies on it for the rotated-cache case without re-proving it. |

Suspiciously quiet areas: no check compared a body written through the guard
to the ledger it came from, and no check exercised output identity across
cache states before this part.
