# Grouping and optional-scan portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.8 U3 by an evaluator that had not taken part in the discovery. It
was given `../../METHOD.md`, `../README.md`, `catalog.md`,
`existing-checks.md`, `fault-map.md`, `../marker-ledger.md`, the sibling
`identity/` and `required/` catalogs, `crates/retrieval/src/packing/`,
`crates/daemon/src/packing/mod.rs`, `crates/daemon/src/m0_compose.rs`,
`crates/daemon/src/canonical_memory.rs`, the two test files, the frozen
reference, the fixture, and `crates/daemon/tests/support/packing.rs`. It did
not open `evidence/`. It ran `cargo test --locked -p retrieval --test
packing_grouping` (9 tests, pass), `cargo test --locked -p daemon --test
packing_optional` (6 tests, pass), `grep -rn 'prepare_optional' crates
--include=*.rs` (`crates/daemon/src/packing/mod.rs` and
`crates/daemon/tests/packing_optional.rs` only), and `grep -rn
'skip_and_continue' crates --include=*.rs` (`crates/daemon/src/m0_compose.rs`
is the one non-packing, non-test caller). The disposition is ours.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 6 | applied to the catalog, existing-checks, and fault-map |
| gap | 4 | one closed at this change, three queued |
| bias | 4 | require human judgment, listed below |

## Refinements applied

1. **The scan record said the memory-trim path has "no configuration gate".**
   The chain is `crates/daemon/src/canonical_memory.rs` `read_project_memory`
   under `crates/daemon/src/lib.rs` `project_memory_read`, which runs only when
   `cfg.memory_enabled` is set; `crates/daemon/src/config.rs` defaults it to
   `true`, so `default-production` stands. The record now names the switch.
2. **The scan record's Check cited the memory-trim comparison that its
   Exercised list did not name.** `budget_boundaries_match_the_replaced_loop`
   and `skip_and_continue_delegation_matches_the_replaced_loop` in
   `crates/daemon/src/m0_compose.rs` are now listed.
3. **`existing-checks.md` credited "`render_m0` tests in `lib.rs`".** That
   `lib.rs` function is a `bench_internals` wrapper for `benches/hot_path.rs`,
   not a test. The row now points at `crates/daemon/src/canonical_memory.rs`
   `rows_past_the_memory_budget_are_neither_injected_nor_digested`, and the
   two `m0_compose` unit tests have their own row.
4. **"out-of-parent" was misnamed.** The packer has no parent length; the span
   is refused only because a whole-buffer sibling of the same key is present
   (`grouping.rs` `merge`). The catalog and fault-map now say "past its
   whole-buffer sibling".
5. **The partition record under-reported its daemon test.**
   `optional_faults_are_excluded_with_a_reason_and_never_refuse_the_preparation`
   asserts `Duplicate`, `Missing`, `Excluded(Retracted)`, and two `Stale`
   exclusions in order and that the duplicate is loaded once. The Guarantee,
   Check, and Required-faults lines now say so, and the fault-map row matches.
6. **The bounds record claimed all six bounds refuse "before any optional
   payload is loaded" through the daemon entry.** Only `max_fused_candidates`
   is tightened through the daemon; the other five are asserted through the
   pure function, and their position before the load hold is fixed by
   `prepare_optional`'s order alone. The Check says so and the record carries
   an open question.

## Gaps

1. **`OptionalExclusion::Corrupt` was never presented through the store.**
   The load-time exclusion path in `prepare_optional` was reached by no
   integration test; the required part's payload-rewrite seam existed. Closed
   at this change: `a_corrupt_optional_payload_is_excluded_and_the_scan_continues`
   alters a payload row, asserts the `Corrupt` exclusion, the sound row's
   admission, and that the corrupt load is not charged. Listed under the
   partition record and the fault-map.
2. **`OptionalCostOverflow` has no record.** `prepare_optional` refuses a group
   whose summed range cost is unrepresentable, and
   `a_group_whose_cost_overflows_is_refused_not_admitted_at_a_saturated_cost`
   asserts it, but no record or Q2 ruling names it. Queued for the accounting
   part (RP2.8 U5a), which owns cost composition; until then the relationship
   map points here.
3. **No optional-phase deadline check at evaluation time.** Every daemon
   optional test then used `EvalBudget::unbounded()`, so `hold` took the
   uninterruptible branch. Partly closed at this change:
   `a_deadline_that_passes_while_the_connection_is_held_refuses_the_optional_phase_without_reading`
   moves the required part's `while_connection_is_held` helper into the shared
   support and asserts `prepare_optional` refuses with `Deadline` inside the
   budget with no optional event, covering the acquisition deadline and the
   pre-read poll. Still open: a deadline or cancellation that lands while an
   optional statement is running. The "interrupted statement ends the
   statements at once" rule is observed only by
   `optional_statements_stop_at_the_first_non_excludable_fault`, a combinator
   unit test that never calls `prepare_optional`, because no test seam raises
   the budget's interrupt mid-batch. Queued for RP2.8 U4, which lands the
   production caller and its request budget.
4. **Two checks appeared in no artifact.** The combinator unit test above and
   `selected_bytes_are_never_printed_by_grouping_types` are now rows in
   `existing-checks.md`, status `unaudited`.

## Biases for a human

1. **Every record is `always`; the distribution is `always` 4, others 0.**
   Right for pure functions, and every marker in `../marker-ledger.md` is
   `sometimes`, so situation coverage lives in the ledger rather than in
   records, as in the `fusion/` precedent. The cost is that the one
   `default-production` record is exercised in production only through the
   memory trim, whose subject still clamps a float budget before delegating.
2. **The frozen reference is independent on the merge, not on the pre-filter
   or the key.** `frozen_packer.rs` merges through a byte-coverage map with no
   production analogue, but its refusal rules transliterate `grouping.rs`
   `merge` rule for rule, and `ref_spans` takes the group key from production
   `Grouping::derive`. "No code in common" is literally true; "independent
   oracle" holds for merging only. Whether the refusal rules deserve a second
   oracle is a cost call. The seeded runner (`SEED`, 512 cases) is
   deterministic and matches the ledger's marker status.
3. **`Debug` redaction has a test in each part and a record in neither.**
   `grouping.rs` and `packing.rs` redact; `packing_grouping.rs` and
   `packing.rs` assert it; the catalog states it as a rule under the Q2
   section. The `required/` catalog has no record either. Decide once for both
   parts; the checks are listed as `unaudited` meanwhile.
4. **`RequiredMaterialization` is the "witness that the required phase
   completed" but is a plain struct with public fields.** Any caller can
   build one with an arbitrary `remaining` and skip `prepare_required`;
   `group` likewise assumes deduplicated input, which `prepare_optional`
   provides by convention. Both contracts are held by convention, not by type.
   The owner declined sealing the witness at this change because RP2.8 U4
   lands the single call site inside the same crate; whether the catalog
   should name the limit under the `required/` precedence record is an owner
   decision when U4 exposes the pair.
