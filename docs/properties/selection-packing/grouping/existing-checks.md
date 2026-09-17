# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `89c5589e`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy. This table inventories checks that predate the part
and checks that carry a claim without a record; the checks that exercise a
record are linked from that record's `Exercised` line in `catalog.md`, as in
the `identity/`, `required/`, and `fusion-routes` parts.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/identity.rs`, `parent_groups_share_a_parent_across_spans_and_never_replace_occurrences` | Spans of one source share a parent key; a revision or representation change derives another. | unaudited | Fixes the key only; no merge, order, or coverage clause. |
| `crates/retrieval/tests/packing_identity.rs`, `grouping_keys_need_parent_revision_and_representation_together` | The grouping key needs class, parent, revision, and representation together. | unaudited | Key equality, not merging. |
| `crates/daemon/src/canonical_memory.rs`, `rows_past_the_memory_budget_are_neither_injected_nor_digested` | A long memory row that does not fit is skipped and a shorter later row is still admitted through the production snapshot path. | unaudited | The precedent for the rule; its budget is a clamped float. |
| `crates/daemon/src/m0_compose.rs`, `budget_boundaries_match_the_replaced_loop` and `skip_and_continue_delegation_matches_the_replaced_loop` | The delegation to `skip_and_continue` matches the replaced loop on boundary budgets and generated rows. | unaudited | Compares against the trim's own former loop, not an independent oracle. |
| `crates/daemon/src/packing/mod.rs`, `optional_statements_stop_at_the_first_non_excludable_fault` | A missing or corrupt row is excluded and the next statement runs; an interrupted statement is the fault and no later statement runs. | unaudited | Exercises the combinator over a fixed outcome list, not `prepare_optional` under a deadline. |
| `crates/daemon/tests/packing_optional.rs`, `a_deadline_that_passes_while_the_connection_is_held_refuses_the_optional_phase_without_reading` | With the projection connection held on another thread, a 200 ms deadline refuses `prepare_optional` with `Deadline` inside two seconds and records no optional event. | unaudited | Covers the hold's deadline, not cancellation, for the optional phase; the required part covers both. |
| `crates/daemon/tests/packing_optional.rs`, `a_budget_that_ends_during_optional_costing_refuses_the_admission` | A budget cancelled by the estimator while optional groups are costed refuses the admission with `Deadline` and records no `Scanned` event. | unaudited | Cancels through the estimator, not a wall-clock deadline. |
| `crates/daemon/tests/packing_optional.rs`, `an_optional_set_with_no_live_row_completes_without_a_kernel_judgment` | With every kernel reader held on another thread, a set whose requests are all missing completes with `Missing` exclusions inside a 300 ms budget instead of waiting on the reader. | unaudited | Holds the reader for four seconds; the check bounds the wait at two. |
| `crates/retrieval/tests/packing_grouping.rs`, `selected_bytes_are_never_printed_by_grouping_types`; `crates/daemon/src/packing/mod.rs`, `materialized_bytes_are_never_printed` | `Debug` output of grouping and required-phase types names byte lengths, never selected content. | unaudited | Asserts the crate-level "payloads are never logged" claim; no record owns it in this part or `required/`. |

Suspiciously quiet areas: no check merged byte ranges before this part, and
no check compared a packer against an independent reference.
