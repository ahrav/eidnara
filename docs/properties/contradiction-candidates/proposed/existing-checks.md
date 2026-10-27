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
| `the_page_cursor_is_a_keyset_position_and_only_issued_cursors_parse` | `:513` | rows added before or removed from page one move nothing after the cursor; non-canonical and oversized cursors refused | read (paging) | unaudited |
| `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` | `:560` | K accepted, K + 1 refused with held rows kept; multi-row eviction; smallest-key tie-break | row key and caps | unaudited |
| `a_keyed_delete_names_its_project_and_the_gauge_counts_bytes` | `:624` | a delete is project-scoped; the gauge counts UTF-8 bytes | sweep (store arm) | unaudited |

Baseline: `crates/memory-store/tests/baseline.rs` pins the table and its two
indexes in `EXPECTED_OBJECTS` and refuses a store at the prior baseline
untouched (`:495`).

## S-E sweep, S-G flag, and R5 (`crates/daemon/tests/contradiction_candidates_sweep.rs`, #1166)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `the_eligibility_rule_admits_exactly_active_stale_and_disputed_decisions_of_the_project` | `:377` | dispositions read back through claim facts; the predicate admits active, sensitive, stale, disputed of the bound project only | sweep | unaudited |
| `one_sweep_drops_every_dead_row_keeps_the_eligible_ones_and_a_repeat_drops_none` | `:432` | each drop reason on its own row; repeat drops zero; tip and job facts unchanged; gauge | sweep, R5 | unaudited |
| `a_row_at_the_age_bound_stays_and_one_past_it_drops` | `:485` | age boundary | sweep | unaudited |
| `a_closed_kernel_drops_nothing_and_counts_the_skip` | `:501` | no Kernel: no read, one skip | sweep | unaudited |
| `the_watermark_covers_three_pages_in_three_runs_and_the_fourth_starts_over` | `:516` | page bound and wrap | sweep | unaudited |
| `a_false_flag_runs_no_table_statement_and_moves_no_counter` | `:552` | hidden table, false flag: no statement, zero counters | flag off (sweep arm) | unaudited |
| `the_counter_block_holds_exactly_the_closed_vocabulary_as_integers` | `:600` | the 27 names, integers only | flag off (status arm) | unaudited |
| `the_evidence_read_answers_liveness_at_its_snapshot_in_request_order` | `:640` | Live, Retired, Unknown; bound; deadline | sweep | unaudited |
| `the_candidate_modules_name_no_kernel_write_path` | `:694` | no Kernel write entry point named in the module's files (lexical) | R5 | unaudited |
| `every_reviewer_job_the_daemon_builds_carries_no_signal` | `:718` | every `CausalInputs {` literal under `crates/daemon/src` has empty `signals` (lexical) | R5 | unaudited |
| `a_failed_claim_read_keeps_its_rows_and_the_kernel_free_drops_still_land` | `:764` | a refused chunk keeps its rows; Kernel-free drops land | sweep | unaudited |
| `a_row_written_after_the_sweep_tip_stays_until_a_later_sweep_judges_it` | `:816` | rows above the tip are not judged | sweep | unaudited |
| `a_cancelled_sweep_deletes_nothing` | `:842` | cancellation | sweep | unaudited |
| `every_claim_chunk_of_a_page_is_read` | `:854` | two claim chunks | sweep | unaudited |
| `the_cleanup_pass_carries_the_watermark_and_reads_the_flag_each_pass` | `:899` | lifecycle wiring, flag per pass, cancel | sweep, flag off | unaudited |
| `the_evidence_read_refuses_another_incarnation_and_a_future_snapshot` | `:969` | incarnation and snapshot refusals | sweep | unaudited |

Also: `crates/daemon/src/config.rs:2299` and `:2320` (default, tier, and
resolved-config freezing), `crates/daemon/src/lib.rs:21367` (bound route and
status block), `crates/host-runtime/src/control.rs:2004` (host forwarding
allowlist against `docs/host-wire-protocol.md`), and
`crates/memory-store/tests/contradiction_candidates.rs:670` (a delete as
read spares a rewritten row).

## S-B trigger and S-C candidate set (`crates/daemon/src/candidate_pass_tests.rs`, #1167)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `a_route_without_consent_offers_no_pass_and_writes_no_row` | `:251` | consent off on the offer: zero counters, no row | flag off (pass arm) | unaudited |
| `a_pass_starts_only_after_the_job_releases_the_slot_and_the_gate` | `:282` | no offer while the job holds the slot; slot idle and gate free at start and while scoring; memo hit offers nothing; replay rescored | trigger, row key | unaudited |
| `a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass` | `:398` | busy refusal; cancel at the second of three descriptors leaves the first one's rows | single flight, partial result | unaudited |
| `the_candidate_set_keeps_the_newest_eligible_decisions_of_the_project_and_counts_the_cut` | `:560` | N + 1 at a reduced N; other project never scored; truncation counted | candidate set | unaudited |
| `a_superseded_decision_is_never_scored_and_the_default_bounds_are_the_named_constants` | `:603` | superseded unread; production bounds pinned | candidate set | unaudited |
| `the_summary_byte_cut_skips_payloads_before_any_load` | `:667` | byte cut before load, counted | candidate set | unaudited |
| `an_oversize_descriptor_writes_nothing_and_an_unscorable_summary_leaves_the_others` | `:695` | descriptor one byte over refused; over-bound and floor-emptied summaries unscorable | oversize (pass arm) | unaudited |
| `a_deadline_during_the_third_descriptor_leaves_the_first_two` | `:727` | deadline during the third of five: full top-K for the first two | partial result | unaudited |
| `a_budget_spent_before_the_pass_writes_nothing_and_counts_every_descriptor` | `:759` | spent budget: no row, all unscored, one deadline | partial result | unaudited |
| `the_row_caps_hold_per_descriptor_per_pass_and_per_project_and_a_replay_keeps_the_rows` | `:779` | K, M, a write over the project cap, newer-tip rescore, held rows past M | row key | unaudited |
| `a_rerun_from_the_retained_bytes_at_one_tip_rebuilds_the_rows_and_the_kernel_is_untouched` | `:850` | retained texts equal published texts; truncate and rescore rebuild the rows; tip and jobs unchanged | rebuild, R5 | unaudited |
| `the_route_consent_reaches_the_pass_through_a_transform` | `:933` | the route's frozen value, not the daemon's, decides the offer | flag off (pass arm), trigger | unaudited |
| `a_panicking_pass_releases_its_permit_and_charge_and_shutdown_waits_for_a_parked_pass` | `:983` | panic counted with permit and charge released; kept-text charge below the job's; shutdown waits | single flight | unaudited |
| `publish_latency_with_candidates_on_stays_within_the_off_baseline` | `:1041` | median publication hold, with at least one publication beside a running pass, within 3x + 50 ms of the flag-off median | trigger (C1) | unaudited |

## S-F read, S-G read and pass arms, S-H confirm (#1168)

Rust checks are in `crates/daemon/src/candidate_pass_tests.rs`; TypeScript
checks are named with their files.

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `review_candidates_pages_live_rows_and_answers_each_terminal` | `:1242` | 8-key items; clamps at 65 and 0; order and disjoint two-page walk; four stale rows filtered and counted; `not_live` for superseded and unknown; empty page, including for a live decision whose id holds a control character; `invalid_params` bodies; scope refusal before the body; observational read; flag off with the table hidden and a flag-on control; `store_unavailable` before a malformed cursor on the hidden table; tip, jobs, and rows unchanged | read serves, read terminals, flag off (read arm) | unaudited |
| `review_candidates_answers_the_store_and_kernel_states_before_any_page` | `:1575` | Kernel starting, Kernel open failed, and no Memory Store answer their states without `items` | read terminals | unaudited |
| `a_listed_candidate_confirms_through_dispute_while_its_evidence_lives` | `:1643` | listed evidence id is a published descriptor's; confirm lands `disputed` with the table truncated; after retention the row is filtered and the earlier form is `not_found` | confirm linkage | unaudited |
| `the_confirm_path_never_reads_the_candidate_table` | `:1749` | `commit.rs` and `dispute.rs` name no candidate store method (lexical) | confirm linkage | unaudited |
| `a_publication_without_consent_runs_no_candidate_table_statement` | `:1767` | consent-off publications succeed with the table hidden and every counter zero; a consenting publication's pass fails on it | flag off (pass arm) | unaudited |
| `eidnara_memory candidates` | `packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1506` | default read without a tool-call id; request body; confirm form; terminals by name; extra-key refusal; dispute-admission note; foreign-decision and paging refusals | read serves (client), confirm linkage | unaudited |
| `Pi eidnara_memory candidates` | `packages/pi-plugin/src/tools/eidnara-memory.test.ts:494` | default schema with paging fields; `disabled` rendered; dispute-admission note from the Pi allowlist | read terminals, flag off (read arm) | unaudited |
| `review.candidates is a listed method whose items and terminals are the ones the parser admits` | `packages/cli/src/commands/review-wire.contract.test.ts:120` | section 7.8 lists the method; its item keys and terminals equal the parser's | read serves (wire) | unaudited |

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
- The pass records `matcher_digest()` with each row
  (`crates/daemon/src/contradiction_candidates/pass.rs:342`); no test reads
  the digest back from a pass-written row.
- No test fails a store write inside a pass; that path counts the failed
  descriptor as analyzed and the later ones as unscored.
