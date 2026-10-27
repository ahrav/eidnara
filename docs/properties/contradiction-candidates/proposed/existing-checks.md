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
| `each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` | `:73` | one row per key; lower-tip rewrite keeps the tip; a newer write removes unnamed keys | row key and caps | unaudited |
| `a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` | `:115` | K + 1 rows, bad counts, repeated decisions, bad ids and digests refused; nothing written | row key and caps | unaudited |
| `an_id_the_scanner_detects_is_refused_and_nothing_is_written` | `:169` | a secret-bearing id is refused | read keys (no text) | unaudited |
| `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest` | `:188` | eviction order and count; another project untouched | row key and caps | unaudited |
| `a_decision_pages_in_reader_order_with_a_stable_cursor` | `:236` | reader order; 70 rows at limit 64 page as 64, 6, none; clamp | read serves only live rows (store arm) | unaudited |
| `the_sweep_pages_by_rowid_and_deletes_remove_exactly_the_named_keys` | `:309` | sweep page by rowid; keyed delete; row and byte gauge | sweep (store arm) | unaudited |
| `truncating_and_replaying_the_writes_rebuilds_the_same_rows` | `:376` | truncate then replay reproduces the rows | rebuildable (store arm) | unaudited |
| `the_table_holds_ids_integers_and_times_and_checks_its_shapes` | `:415` | the column set; `score_bp` and count CHECKs | read keys (no text) | unaudited |
| `a_late_write_replaces_held_keys_and_adds_none` | `:458` | a write below the held tip adds no key | row key and caps | unaudited |
| `a_write_under_another_kernel_incarnation_takes_its_own_tip` | `:492` | a new incarnation's write carries its own tip | sweep and reads (incarnation filter) | unaudited |
| `the_page_cursor_is_a_keyset_position_and_only_issued_cursors_parse` | `:517` | rows added before or removed from page one move nothing after the cursor; non-canonical cursors refused | read (paging) | unaudited |
| `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` | `:570` | K accepted, K + 1 refused with held rows kept; multi-row eviction; smallest-key tie-break | row key and caps | unaudited |
| `a_keyed_delete_names_its_project_and_the_gauge_counts_bytes` | `:634` | a delete is project-scoped; the gauge counts UTF-8 bytes | sweep (store arm) | unaudited |
| `random_write_and_delete_sequences_match_the_reference_model` | `:1049` | seeded write and delete sequences match a reference model: rows, counts, rowid stability, and the gauge each step; pages, cursors across writes, and the sweep every seventh step | row key and caps; read (paging); sweep (store arm) | unaudited |
| `the_hex_checks_refuse_uppercase_non_hex_and_misfit_lengths` | `:1177` | the hex-shape CHECKs refuse uppercase, non-hex, and wrong-length digests | read keys (no text) | unaudited |
| `the_totals_follow_replaced_ids_late_writes_deletes_and_truncation` | `:1243` | the gauge equals a sum over the rows through replaced ids, late writes, deletes, and truncation | sweep (store arm) | unaudited |
| `a_new_row_the_cap_evicts_at_once_counts_as_written_and_dropped` | `:1298` | a new row below the cap's floor is counted written and dropped; held rows and rowids stay; a landing row takes a fresh rowid | row key and caps | unaudited |
| `only_the_candidate_module_writes_the_candidate_tables` | `:1429` | no other memory-store source, trigger, or session purge writes either candidate table, so the totals the cap and gauge read stay exact | row key and caps; sweep (store arm) | unaudited |

Baseline: `crates/memory-store/tests/baseline.rs` pins the table, its two
indexes, and `contradiction_candidate_totals` in `EXPECTED_OBJECTS` and
refuses a store at the prior baseline untouched (`:496`).

## S-B, S-C, S-E to S-H and R5

None found. The trigger, candidate set, store, sweep, read, flag, and
dispute-handoff behavior does not exist at the branch tip; the store
(#1165), sweep (#1166), pass (#1167), and read (#1168) tickets add the code
and the checks. The 12 proposed records carry `Existing check: none;
proposed`.

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
