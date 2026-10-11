# Existing checks: lexical retrieval

Every claim-bearing check the ten records cite, plus the lexical checks in the
same test files that no record cites, with status `unaudited`. Adequacy
verdicts belong to `/testing:invariant-test-review`; a listed check is a link,
not a proof. Locations are verified against the #391 branch at `b3b9b7a80`.

Three test fixtures appear below. `lexical_engine.rs` runs probes against a
bare FTS5 engine built by `engine()` (`crates/retrieval/tests/lexical_engine.rs:16`)
and `populated()` (`:180`), with no projection, kernel, or admission.
`lexical_retrieval.rs` runs `retrieve` against a projected store opened by
`open_store` (`crates/retrieval/tests/lexical_retrieval.rs:109`) under
`bounds()` (`:153`: eight probes, 64 scan rows, 64 accepted, batches of two,
the owner-approved 10,000 qualifying matches and 15,000 ranked matches); `production_bounds`
(`:2000`) and `small_thresholds` (`:2597`) vary them. `claim_eligibility.rs` and
`query_route.rs` drive the daemon's routes and the kernel.

## Literal probes (AC1)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `literal_operators_are_terms` | `crates/retrieval/tests/lexical_engine.rs:196` | `OR`, `NEAR`, quotes, and syntax-shaped text match only the hand-written term sets on a populated engine | unaudited |
| `removing_the_quotes_makes_operators_operate` | `lexical_engine.rs:213` | the unquoted control lets the operators widen or narrow the match set | unaudited |
| `a_probe_matches_only_its_term_and_operators_in_text_stay_literal` | `crates/retrieval/tests/lexical_retrieval.rs:596` | operator text through `retrieve` on a projection | unaudited |
| `atoms_never_contain_quotes` | `crates/retrieval/tests/lexical_analysis.rs:236` | analysis emits no `"` inside an atom | unaudited |
| `syntax_shaped_input_is_ordinary_atoms` | `lexical_analysis.rs:170` | query syntax in request text analyzes to ordinary atoms | unaudited |
| `an_atom_of_engine_separators_probes_for_an_empty_phrase_that_matches_nothing` | `lexical_engine.rs:411` | an atom the engine tokenizes to nothing matches no row | unaudited |

Production guard: `compile` quotes every atom with `quote_atom` in
`crates/retrieval/src/lexical/compile.rs`.

## Zero terms (AC1)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `zero_probes_run_no_match_while_a_control_probe_contributes` | `lexical_retrieval.rs:627` | zero probes yield `Completion::Empty` and no contribution; the control contributes | unaudited |
| `zero_atoms_issue_no_probe_while_a_nonempty_control_completes` | `lexical_engine.rs:268` | zero atoms issue no probe on the engine | unaudited |

## Order and tie identity (AC2)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation` | `lexical_retrieval.rs:687` | order equals an independent reference under duplicated and permuted probes | unaudited |
| `equal_ranks_from_distinct_probes_keep_the_lowest_ordinal` | `lexical_retrieval.rs:728` | exact ties keep the lowest probe ordinal | unaudited |
| `a_large_equal_rank_group_at_the_bound_keeps_the_lowest_identifiers` | `lexical_retrieval.rs:2385` | 3,000 equal-rank rows across a 64-row bound keep the lowest identifiers regardless of storage order | unaudited |
| `a_tie_group_spread_thinly_through_the_table_keeps_the_lowest_identifiers` | `lexical_retrieval.rs:2419` | 240 equal-rank rows among 3,000 unmatched rows keep the lowest identifiers at a 240-row and a 64-row bound | unaudited |
| `dead_rows_in_a_large_tie_group_keep_their_window_slots` | `lexical_retrieval.rs:2457` | tombstoned leaders of a tie group | unaudited |
| `dead_rows_inside_a_distinct_rank_probe_keep_their_window_slots` | `lexical_retrieval.rs:2937` | one rank per match, dead rows at the front and around the bound | unaudited |
| `a_bound_that_ends_at_a_rank_change_reads_one_row_of_the_next_rank_group` | `lexical_retrieval.rs:3078` | a 320-row tie past a bound that ends at a rank change costs one witness row | unaudited |
| `a_dead_row_at_a_rank_change_opens_no_slot_for_the_next_rank_group` | `lexical_retrieval.rs:3078` | a dead row at a rank change keeps its slot, so no next-group row enters the window | unaudited |
| `common_probes_are_read_in_descending_rowid_order_and_a_mixed_query_ranks_only_the_qualifying_probe` | `lexical_retrieval.rs:1689` | common probes read in descending rowid order; only the qualifying probe is ranked | unaudited |

Production comparator: `by_id` (`crates/retrieval/src/lexical/retrieve.rs:280`),
`rank.total_cmp` in `scan_ranked` (`:665`) and `merge_best` (`:697`), and
`by_id` then ordinal in `identify` (`:811`).

## Eligibility before accepted slots (AC3, AC4)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `an_ineligible_leader_is_excluded_without_taking_an_accepted_slot` | `lexical_retrieval.rs:762` | an ineligible leader is judged before any slot is used | unaudited |
| `admission_batches_scale_the_open_slots_by_the_acceptance_so_far_and_an_exact_fill_stays_complete` | `lexical_retrieval.rs:797` | the first batch judges the open slots and later ones scale them by the acceptance so far; an exact fill is complete | unaudited |
| `equal_byte_siblings_stay_distinct_and_fill_past_an_ineligible_leader` | `lexical_retrieval.rs:2597` | three equal-byte occurrences of distinct objects, one hidden, one-row batches, duplicated and permuted probes | unaudited |
| `scan_then_admit_under_one_connection_equals_retrieve_and_carries_the_judged_candidate` | `lexical_retrieval.rs:652` | `scan` then `admit` equals `retrieve` | unaudited |
| `revalidation_drops_an_occurrence_retired_after_admission` | `lexical_retrieval.rs:1200` | retirement between admission and revalidation | unaudited |
| `a_snapshot_that_moves_between_admission_batches_stops_admission` | `lexical_retrieval.rs:1223` | a kernel snapshot move between batches | unaudited |
| `a_moved_snapshot_batch_still_tallies_its_exclusions` | `lexical_retrieval.rs:1253` | exclusions tallied in the batch the move interrupts | unaudited |
| `a_kernel_restore_before_revalidation_marks_the_incarnation_change` | `lexical_retrieval.rs:1293` | an incarnation change before revalidation | unaudited |
| `an_authority_move_outranks_a_coverage_bound_already_recorded` | `lexical_retrieval.rs:2631` | incomplete-reason precedence | unaudited |

Test seam: `retrieve_with_hook_for_test` (`retrieve.rs:373`) runs a hook after
every admission batch and before the final re-judgment.

## One kernel policy (AC3)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot` | `crates/daemon/tests/claim_eligibility.rs:184` | the retrieval adapter, the daemon route, and the kernel agree on every occurrence, under retirement, correction, and a foreign project | unaudited |
| `canonical_eligibility_has_one_kernel_policy_entry_outside_the_kernel` | `claim_eligibility.rs:456` | every workspace member's `src` outside `crates/kernel` names `judge_eligibility` only in the three allowed files; retrieval's dependencies include `kernel` and exclude `daemon`; the kernel's `judge` is private | unaudited |

## Bounded and observed work (AC5)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `the_scan_bound_marks_incomplete_only_past_the_bound_and_refusals_precede_every_probe` | `lexical_retrieval.rs:838` | incompleteness at the bound; refusals before any probe runs | unaudited |
| `a_repeated_probe_runs_the_engine_once_and_adds_no_work` | `lexical_retrieval.rs:1119` | a repeated probe counts once | unaudited |
| `ranking_work_is_admitted_by_exact_counts_at_the_d26b_boundaries` | `lexical_retrieval.rs:1626` | qualifying and rank-budget thresholds at exact counts | unaudited |
| `a_mixed_query_records_both_its_common_skip_and_its_budget_skip` | `lexical_retrieval.rs:2606` | both skip reasons recorded under `small_thresholds` | unaudited |
| `a_probe_with_no_match_leaves_the_common_probe_to_its_bounded_scan` | `lexical_retrieval.rs:2358` | a no-match probe beside a common probe | unaudited |
| `allocations_follow_the_scan_bound_and_the_rank_budget` | `lexical_retrieval.rs:2768` | Rust allocation events and bytes at the bound, one row below it, and 32 rows past the common thresholds; the daemon's `alloc_recorder` as the global allocator | unaudited |
| `work_counters_report_exact_and_over_bound_work` | `lexical_retrieval.rs:2843` | probes, counted rows, scanned rows, ranked matches, judgments, batches, and `sql_steps` at the bound and one row below it; equal work under a result cap of one | unaudited |
| `lexical_scan_p99_at_one_million_occurrences` | `lexical_retrieval.rs:2170` | D26b open-loop p99 at one million occurrences under production bounds; `#[ignore]`, release, D21 host | unaudited |
| `each_bound_saturates_before_its_protected_work` | `crates/daemon/tests/query_route.rs:320` | each D23 limit refuses or truncates before the work it protects | unaudited |
| `the_lexical_ranking_bounds_report_every_scope_they_skip` | `query_route.rs:679` | the route reports every skipped scope | unaudited |
| `production_limits_are_the_d23_set` | `query_route.rs:532` | the daemon's limits equal D23 | unaudited |

Production counters: `Consumed` in `retrieve.rs`; `vm_steps` (`retrieve.rs:636`)
reads `SQLITE_STMTSTATUS_VM_STEP` per statement.

## Original budget (AC6)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `cancellation_and_deadline_are_observed_in_every_phase` | `query_route.rs:74` | cancellation and a passed deadline before each `Phase`, including `Phase::Lexical` (`crates/daemon/src/query_route.rs:1383`) | unaudited |
| `an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion` | `lexical_retrieval.rs:1457` | a connection interrupt through `with_conn_interruptible` during the lexical scan | unaudited |
| `an_interrupt_anywhere_in_counting_ranking_or_a_common_scan_is_budget_exhaustion` | `lexical_retrieval.rs:2510` | the progress handler fires at every point of counting, ranking, and a common scan; the connection serves the next request | unaudited |
| `a_budget_that_ends_after_a_probe_yields_an_incomplete_result_with_no_contributions` | `lexical_retrieval.rs:1350` | a budget that ends between probes | unaudited |
| `a_held_kernel_reader_does_not_outlive_the_budget` | `lexical_retrieval.rs:1566` | the kernel reader is released at the budget's end | unaudited |
| `claim_validation_stops_waiting_for_a_held_connection_when_the_budget_ends` | `query_route.rs:112` | a queue wait that outlives the budget | unaudited |

Production guard: `budget.check()` before each row in `count_probe`,
`ranked_rows`, `identify`, `Descending::head`, and `resolve`, and after the
window partition in `scan_ranked`
(`retrieve.rs:624`, `:745`, `:778`, `:843`, `:935`, `:687`), and the connection's
progress handler at `READ_INTERRUPT_STEPS` (`crates/storage/src/lib.rs:191`).

## No payload reads (AC7)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `retrieval_reads_no_payload_bytes` | `lexical_retrieval.rs:1325` | the `payloads` table is renamed away and the result is unchanged | unaudited |

## Dead rows (AC2, AC5)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `a_tombstoned_row_is_excluded_at_the_engine` | `lexical_retrieval.rs:916` | a tombstone excludes the row inside the scan | unaudited |
| `an_orphaned_lexical_row_is_excluded_at_the_engine` | `lexical_retrieval.rs:934` | a lexical row with no occurrence | unaudited |
| `tombstoned_rows_inside_the_scan_bound_are_dropped_without_replacement` | `lexical_retrieval.rs:1040` | dead rows at every scan bound via `assert_stale_row_excluded_at_every_scan_bound` (`:963`) | unaudited |
| `a_dead_row_inside_the_window_still_counts_toward_the_bound` | `lexical_retrieval.rs:2937` | a dead row inside the window still counts toward the bound | unaudited |
| `dead_rows_leading_a_common_probe_keep_their_window_slots` | `lexical_retrieval.rs:2660` | dead leaders of a common probe | unaudited |

Test seams: `project_bulk` (`lexical_retrieval.rs:1012`) and `tombstone_raw`
(`:1028`) write rows and tombstones under the projection's own schema.

## One index (AC10)

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `the_baseline_matches_the_frozen_inventory_field_for_field` | `crates/retrieval/tests/schema_inventory.rs:312` | the stored schema equals the frozen inventory; the `lexical` table is `crates/retrieval/baseline.sql:94` | unaudited |
| `no_prefix_expansion_and_the_prefix_control_differs` | `lexical_engine.rs:248` | no implicit prefix expansion; the prefix control differs | unaudited |
| `parts_extend_recall_without_implicit_expansion` | `lexical_engine.rs:233` | the `parts` column extends recall without expansion | unaudited |
| `long_tokens_share_the_engines_truncated_term_without_a_prefix_operator` | `lexical_engine.rs:340` | the engine's own truncation, with no prefix operator | unaudited |

## Host application (AC8)

None found: no OpenCode or Pi test applies a lexical contribution through the
exact, lexical, f32, fusion, packing, and application path.

## Suspiciously quiet areas

- No check compares `sql_steps` with an approved bound; every step assertion is
  relative (fewer under a smaller bound, more with one more row). RP2.9 has
  approved no allocation or step bound.
- No check observes the budget during the in-memory merge and window partition
  in `scan_ranked` (`retrieve.rs:675`); both are bounded by `rank_budget` and
  the next `budget.check()` follows them (`:687`), but no test times a
  cancellation that lands inside them.
- SQLite's own allocations and FTS5's internal rank statements are outside the
  allocation recorder and `sql_steps`; no check bounds them.
- `lexical_scan_p99_at_one_million_occurrences` is `#[ignore]`. The #896 runs
  on an AMD EPYC 9R14 pinned to four cores with the projection on NVMe pass
  the 50 ms gate warm (p99 43.2 ms at 10 queries per second); no run on the
  D21 runner exists, and a cold page cache misses the gate by orders of
  magnitude.
- The architecture scan in `claim_eligibility.rs:456` matches two string
  patterns in `.rs` files; a caller that reaches the entry points through a
  macro, a `use ... as` alias, or generated code is not seen. The scan also
  covers only `judge_eligibility`; the kernel's public
  `judge_surface_eligibility` family (`crates/kernel/src/eligibility.rs:369`,
  `:381`, `:439`, `:467`) is called from `crates/daemon/src/memory_reviewer/broker.rs:1480`,
  `crates/daemon/src/memory_reviewer/selection.rs:155`, and
  `crates/retrieval/src/claims.rs:517`. Every one funnels into the private
  `judge` (defined at `:198`, called at `:310`), so the verdict policy stays
  single, but no check pins that funnel.
- No check drives a real host through lexical application (AC8).
