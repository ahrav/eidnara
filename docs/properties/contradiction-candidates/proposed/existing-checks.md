# Existing checks: P5 proposed obligations

Verified at the #1164 branch tip (base `979fb814e`, scorer commits
`05acd0107` and `446c7363f`). Every status is
`unaudited`: this file records that a check exists and what it asserts;
adequacy belongs to `/testing:invariant-test-review` (tests) and
`/low-level-systems:defensive-assertions-and-invariant-guards` (production
guards). An existing check never removes a record from the catalog.

## S-A scorer (`crates/daemon/tests/contradiction_candidates_scorer.rs`)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `term_sets_follow_the_documented_fold_and_floor` | `:31-80` | hand-written term sets: fold, floor after fold, parts, digit boundaries, private-use and underscore atoms, combining marks, duplicates, `İ` | case fold | unaudited |
| `one_identifier_can_satisfy_the_term_gates` | `:82-98` | one three-part identifier holds four terms and passes the gates | none directly | unaudited |
| `a_contained_summary_scores_full_and_disjoint_texts_score_nothing` | `:100-129` | a contained summary scores 6 of 6; a disjoint pair is `Below { matched_terms: 0, .. }` | oversize (refusal is not score 0) | unaudited |
| `case_folds_identically_on_both_sides_and_diacritics_do_not_fold` | `:131-159` | case-only difference scores 5 of 5 and equals self-score; plain-`e` and NFD spellings score 0 of 4 | case fold | unaudited |
| `a_case_change_inside_an_identifier_changes_its_parts` | `:163-171` | `configLoader` gives {configloader, config, loader}; `CONFIGLOADER` and `configloader` give {configloader} | case fold (the case change crosses a part boundary) | unaudited |
| `identifiers_in_the_descriptor_match_prose_in_the_summary` | `:173-185` | `HTTPServer::configLoader` matches "HTTP server config loader" 4 of 5 | none directly | unaudited |
| `the_count_floors_block_short_or_thin_overlaps` | `:187-222` | each of the three gates blocks; exactly 1/2 passes | none directly | unaudited |
| `an_oversize_descriptor_is_refused_whole_and_an_exact_fit_is_analyzed` | `:224-241` | bounds `{65536, 32769}`; 65536 bytes with 32768 atoms analyzed; 65537 bytes is `InputTooLong` | oversize | unaudited |
| `an_unscorable_summary_is_reported_and_the_others_still_score` | `:243-274` | bound 4097 atoms; 8193-byte and floor-emptied summaries unscorable in id order; at-bound and plain summaries score, checked by kept ids only | oversize (partial: no comparison against a run without the unscorable summary) | unaudited |
| `the_top_k_is_the_same_under_every_input_order` | `:276-318` | K + 1 ties, a lower-score wider match, and an unscorable summary give an equal result over 18 input orders | determinism | unaudited |
| `a_decision_object_id_appears_once_and_the_better_entry_wins_in_any_order` | `:320-346` | one entry per object id, the better-ranked one, in both orders | determinism | unaudited |
| `the_matcher_digest_pins_every_rule` | `:348-400` | the full preimage, `ANALYSIS_CONTRACT_EPOCH`, the matcher epoch, and the digest literal | determinism, case fold (pins the rules) | unaudited |

Production guards: the compile-time assertion that the descriptor bound is
at least `MAX_SOURCE_UNIT_BYTES`
(`crates/daemon/src/contradiction_candidates/scorer.rs:65-67`); the
analyzer's refusals (`crates/retrieval/src/lexical/analysis.rs:78-105`),
which the scorer propagates.

Analyzer checks the scorer relies on:
`crates/retrieval/tests/lexical_analysis.rs:244-297` (bounds) and
`identity_pins_the_manifest` (`:389-413`); see
`../dependencies/existing-checks.md` S1.

## S-D store (`crates/memory-store/tests/contradiction_candidates.rs`, #1165)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` | `:72` | one row per key; lower-tip rewrite keeps the tip; a newer write removes unnamed keys | row key and caps | unaudited |
| `a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` | `:114` | K + 1 rows, bad counts, repeated decisions, bad ids and digests refused; nothing written | row key and caps | unaudited |
| `an_id_the_scanner_detects_is_refused_and_nothing_is_written` | `:168` | a secret-bearing id is refused | read keys (no text) | unaudited |
| `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest` | `:187` | eviction order and count; another project untouched | row key and caps | unaudited |
| `a_decision_pages_in_reader_order_with_a_stable_cursor` | `:235` | reader order; 70 rows at limit 64 page as 64, 6, none; clamp | read serves only live rows (store arm) | unaudited |
| `the_sweep_pages_by_rowid_and_deletes_remove_exactly_the_named_keys` | `:308` | sweep page by rowid; keyed delete; row and byte gauge | sweep (store arm) | unaudited |
| `truncating_and_replaying_the_writes_rebuilds_the_same_rows` | `:372` | truncate then replay reproduces the rows | rebuildable (store arm) | unaudited |
| `the_table_holds_ids_integers_and_times_and_checks_its_shapes` | `:411` | the column set; `score_bp` and count CHECKs | read keys (no text) | unaudited |
| `a_late_write_replaces_held_keys_and_adds_none` | `:454` | a write below the held tip adds no key | row key and caps | unaudited |
| `a_write_under_another_kernel_incarnation_takes_its_own_tip` | `:488` | a new incarnation's write carries its own tip | sweep and reads (incarnation filter) | unaudited |
| `the_page_cursor_is_a_keyset_position_and_only_issued_cursors_parse` | `:513` | rows added before or removed from page one move nothing after the cursor; non-canonical cursors refused | read (paging) | unaudited |
| `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` | `:560` | K accepted, K + 1 refused with held rows kept; multi-row eviction; smallest-key tie-break | row key and caps | unaudited |
| `a_keyed_delete_names_its_project_and_the_gauge_counts_bytes` | `:624` | a delete is project-scoped; the gauge counts UTF-8 bytes | sweep (store arm) | unaudited |

Baseline: `crates/memory-store/tests/baseline.rs` pins the table and its two
indexes in `EXPECTED_OBJECTS` and refuses a store at the prior baseline
untouched (`:495`).

## S-E sweep, S-G flag, and R5 (`crates/daemon/tests/contradiction_candidates_sweep.rs`, #1166)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `the_eligibility_rule_admits_exactly_active_stale_and_disputed_decisions_of_the_project` | `:378` | dispositions read back through claim standing; the predicate admits active, sensitive, stale, disputed of the bound project only | sweep | unaudited |
| `one_sweep_drops_every_dead_row_keeps_the_eligible_ones_and_a_repeat_drops_none` | `:433` | each drop reason on its own row; repeat drops zero; tip and job facts unchanged; gauge | sweep, R5 | unaudited |
| `a_row_at_the_age_bound_stays_and_one_past_it_drops` | `:486` | age boundary | sweep | unaudited |
| `a_closed_kernel_drops_nothing_and_counts_the_skip` | `:502` | no Kernel: no read, one skip | sweep | unaudited |
| `the_watermark_covers_three_pages_in_three_runs_and_the_fourth_starts_over` | `:517` | page bound and wrap | sweep | unaudited |
| `a_false_flag_runs_no_table_statement_and_moves_no_counter` | `:553` | hidden table, false flag: no statement, zero counters | flag off (sweep arm) | unaudited |
| `the_counter_block_holds_exactly_the_closed_vocabulary_as_integers` | `:601` | the 27 names, integers only | flag off (status arm) | unaudited |
| `the_evidence_read_answers_liveness_at_its_snapshot_in_request_order` | `:641` | Live, Retired, Unknown; bound; deadline | sweep | unaudited |
| `the_candidate_modules_name_no_kernel_write_path` | `:695` | no Kernel write entry point named in the module's files (lexical) | R5 | unaudited |
| `every_reviewer_job_the_daemon_builds_carries_no_signal` | `:719` | every `CausalInputs {` literal under `crates/daemon/src` has empty `signals` (lexical) | R5 | unaudited |
| `a_failed_claim_read_keeps_its_rows_and_the_kernel_free_drops_still_land` | `:766` | a refused chunk keeps its rows; Kernel-free drops land | sweep | unaudited |
| `a_row_written_after_the_sweep_tip_stays_until_a_later_sweep_judges_it` | `:817` | rows above the tip are not judged | sweep | unaudited |
| `a_cancelled_sweep_deletes_nothing` | `:843` | cancellation | sweep | unaudited |
| `every_claim_chunk_of_a_page_is_read` | `:856` | two claim chunks | sweep | unaudited |
| `the_cleanup_pass_carries_the_watermark_and_reads_the_flag_each_pass` | `:901` | lifecycle wiring, flag per pass, cancel | sweep, flag off | unaudited |
| `the_evidence_read_refuses_another_incarnation_and_a_future_snapshot` | `:970` | incarnation and snapshot refusals | sweep | unaudited |
| `every_standing_equals_the_claim_facts_of_its_claim_at_every_snapshot`, `standing_refusals_match_the_claim_facts_refusals` | `crates/kernel/tests/kernel_claim_standing.rs:111`, `:168` | the sweep's claim standing read equals the registry row, decision row, own admission row, and served standing of claim facts at every snapshot, and refuses as claim facts does | sweep | unaudited |

Also: `crates/daemon/src/config.rs:2299` and `:2320` (default, tier, and
resolved-config freezing), `crates/daemon/src/lib.rs:21340` (bound route and
status block), `crates/host-runtime/src/control.rs:2004` (host forwarding
allowlist against `docs/host-wire-protocol.md`), and
`crates/memory-store/tests/contradiction_candidates.rs:663` (a delete as
read spares a rewritten row).

## S-B, S-C, S-F, S-H

None found. The trigger, candidate set, read, and dispute-handoff behavior
does not exist at the branch tip; the pass (#1167) and read (#1168) tickets
add the code and the checks.

## Suspiciously quiet areas

- No test runs the scorer on a second thread or process (see the
  determinism record's open question).
- No test combines a repeated decision object id with more than K passing
  summaries across many orders; a property test over random permutations
  is the stronger oracle for the determinism record.
- No test checks that the other summaries' results equal a run without the
  unscorable summary (the oversize record's partial clause).
- No test ties `matcher_digest()` to the scores of a fixed corpus, so a
  behavior change that keeps the preimage's rule ids leaves the digest
  unchanged (portfolio G1).
- The scorer's `TooManyAtoms` path is unreachable by construction and has no
  test; the analyzer's own test covers the refusal.
- No runtime caller exists, so nothing yet checks that the pass records
  `matcher_digest()` with each row (#1165, #1167).
