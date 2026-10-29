# Existing checks: P5 proposed obligations

The S-A rows are verified at the #1164 branch tip (base `979fb814e`, scorer
commits `05acd0107` and `446c7363f`); S-D at the #1165 branch tip; the S-B and
S-C rows at the #1167 branch tip. Every status is
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
| `a_non_ascii_spelling_that_folds_to_ascii_matches_the_ascii_spelling` | `:404-425` | the Kelvin sign spelling `\u{212A}elvin` folds to {kelvin} and matches `Kelvin` and `kelvin` across the two sides, 3 of 4 | case fold | unaudited |
| `a_summary_with_more_than_64_distinct_terms_scores_every_term` | `:429-452` | 70 `termNN` atoms hold 71 terms and score 41 of 71 against 40 of them; the uppercase spelling plus `École` scores 41 of 72 | none directly | unaudited |

## S-A scorer, differential (`crates/daemon/tests/contradiction_candidates_scorer_differential.rs`)

`reference` (`:16-65`) applies the documented rules directly: atoms and
parts, `str::to_lowercase` per term, the floor on scalar values, a
`BTreeSet`, and containment with the three gates.

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `term_sets_match_the_documented_rules` | `:179-189` | `term_set` equals the reference under both bounds for generated mixed ASCII and non-ASCII text | case fold | unaudited |
| `summary_scores_match_the_documented_rules` | `:191-205` | `DescriptorTerms::score` equals the reference for generated descriptors and summaries | case fold, oversize (unscorable versus score 0) | unaudited |
| `summaries_drawn_from_the_descriptor_score_as_the_rules_say` | `:207-230` | summaries built from the descriptor's own words, some uppercased, score as the reference | case fold | unaudited |
| `ascii_summaries_with_many_distinct_terms_score_as_the_rules_say` | `:235-245` | ASCII summaries of up to 160 numbered words, past 64 distinct terms, score as the reference | none directly | unaudited |
| `score_descriptor_keeps_the_reference_top_k` | `:247-284` | `score_descriptor` keeps the reference's top K in rank order and lists the reference's unscorable ids | determinism | unaudited |

Production guards: the compile-time assertion that the descriptor bound is
at least `MAX_SOURCE_UNIT_BYTES`
(`crates/daemon/src/contradiction_candidates/scorer.rs:66-68`); the
analyzer's refusals (`crates/retrieval/src/lexical/analysis.rs:78-105`),
which the scorer propagates.

Analyzer checks the scorer relies on:
`crates/retrieval/tests/lexical_analysis.rs:244-297` (bounds) and
`identity_pins_the_manifest` (`:389-413`); see
`../dependencies/existing-checks.md` S1.

## S-D store (`crates/memory-store/tests/contradiction_candidates.rs`, #1165)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` | `:83` | one row per key; lower-tip rewrite keeps the tip; a newer write removes unnamed keys | row key and caps | unaudited |
| `a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` | `:125` | K + 1 rows, bad counts, repeated decisions, bad ids and digests refused; nothing written | row key and caps | unaudited |
| `an_id_the_scanner_detects_is_refused_and_nothing_is_written` | `:179` | a secret-bearing id is refused | read keys (no text) | unaudited |
| `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest` | `:198` | eviction order and count; another project untouched | row key and caps | unaudited |
| `a_decision_pages_in_reader_order_with_a_stable_cursor` | `:246` | reader order; 70 rows at limit 64 page as 64, 6, none; clamp | read serves only live rows (store arm) | unaudited |
| `the_sweep_pages_by_rowid_and_deletes_remove_exactly_the_named_keys` | `:319` | sweep page by rowid; keyed delete; row and byte gauge | sweep (store arm) | unaudited |
| `truncating_and_replaying_the_writes_rebuilds_the_same_rows` | `:371` | truncate then replay reproduces the rows | rebuildable (store arm) | unaudited |
| `the_table_holds_ids_integers_and_times_and_checks_its_shapes` | `:410` | the column set; `score_bp` and count CHECKs | read keys (no text) | unaudited |
| `a_late_write_replaces_held_keys_and_adds_none` | `:453` | a write below the held tip adds no key | row key and caps | unaudited |
| `a_write_under_another_kernel_incarnation_takes_its_own_tip` | `:488` | a new incarnation's write carries its own tip | sweep and reads (incarnation filter) | unaudited |
| `the_page_cursor_is_a_keyset_position_and_only_issued_cursors_parse` | `:599` | rows added before or removed from page one move nothing after the cursor; non-canonical, oversized, and unstorable-id cursors refused; the longest issued cursor parses and one byte longer is refused | read (paging) | unaudited |
| `the_under_route_page_read_answers_the_authority_before_the_cursor` | `:517` | a route without a MODULE row reads no page and judges no cursor; a bound route reads the plain read's page and judges a malformed cursor after it | read (terminals) | unaudited |
| `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` | `:700` | K accepted, K + 1 refused with held rows kept; multi-row eviction; smallest-key tie-break | row key and caps | unaudited |
| `a_keyed_delete_names_its_project_and_the_gauge_counts_bytes` | `:764` | a delete is project-scoped; the gauge counts UTF-8 bytes | sweep (store arm) | unaudited |
| `random_write_and_delete_sequences_match_the_reference_model` | `:1219` | seeded write and as-read delete sequences match a reference model: rows, counts, rowid stability, and the gauge each step; pages, cursors across writes, and the sweep every seventh step | row key and caps; read (paging); sweep (store arm) | unaudited |
| `the_hex_checks_refuse_uppercase_non_hex_and_misfit_lengths` | `:1370` | the hex-shape CHECKs refuse uppercase, non-hex, and wrong-length digests | read keys (no text) | unaudited |
| `the_totals_follow_replaced_ids_late_writes_deletes_and_truncation` | `:1436` | the gauge equals a sum over the rows through replaced ids, late writes, deletes, and truncation | sweep (store arm) | unaudited |
| `a_new_row_the_cap_evicts_at_once_counts_as_written_and_dropped` | `:1489` | a new row below the cap's floor is counted written and dropped; held rows and rowids stay; a landing row takes a fresh rowid | row key and caps | unaudited |
| `only_the_candidate_module_writes_the_candidate_tables` | `:1619` | no other memory-store source, trigger, or session purge writes either candidate table, so the totals the cap and gauge read stay exact | row key and caps; sweep (store arm) | unaudited |
| `every_candidate_statement_reuses_its_cached_plan` | `:1544` | 12 rounds of writes, late writes, pages, sweep pages, keyed deletes, and gauge reads run 16 distinct candidate statements; the statement cache re-creates and re-plans none | row key and caps; read (paging); sweep (store arm) (the cost of the checks, not a record directly) | unaudited |
| `the_page_sweep_and_eviction_scans_stop_at_their_bound` | `:1707` | over a 300-row project, a two-row page, a three-row sweep page, and a one-row eviction each run under 500 VM steps | read (paging); sweep (store arm); row key and caps (bounded scans) | unaudited |
| `a_continuation_page_seeks_to_its_cursor_inside_a_run_of_equal_scores` | `:1751` | 256 rows into a run of equal scores, with and without equal `created_at_ms`, the continuation page returns the next two rows under 500 VM steps | read (paging) | unaudited |
| `a_read_with_a_secret_shaped_id_answers_an_empty_page` | `:1802` | a page read for a secret-shaped decision id answers an empty page; an empty id is still refused by the shape check | read keys (no text); read (paging) | unaudited |

Baseline: `crates/memory-store/tests/baseline.rs` pins the table, its two
indexes, and `contradiction_candidate_totals` in `EXPECTED_OBJECTS` and
refuses a store at the prior baseline untouched (`:496`).

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
| `a_failed_claim_read_keeps_its_rows_and_the_kernel_free_drops_still_land` | `:765` | a refused chunk keeps its rows; Kernel-free drops land | sweep | unaudited |
| `a_row_written_after_the_sweep_tip_stays_until_a_later_sweep_judges_it` | `:994` | rows above the tip are not judged | sweep | unaudited |
| `a_cancelled_sweep_deletes_nothing_and_reads_no_kernel_row` | `:1018` | cancellation before the first Kernel read: no reader taken, nothing deleted | sweep | unaudited |
| `a_sweep_cancelled_during_the_single_decision_reads_reads_no_further_and_deletes_nothing` | `:816` | cancellation inside the single-decision fallback: the tip capture and the refused chunk are the only Kernel reads; nothing deleted | sweep | unaudited |
| `a_sweep_cancelled_before_the_scope_reads_reads_no_scope_and_deletes_nothing` | `:934` | cancellation before the scope-term reads: the tip capture and the chunk read are the only Kernel reads; nothing deleted | sweep | unaudited |
| `a_restore_between_the_claim_read_and_the_evidence_read_deletes_nothing` | `:872` | a restore between the claim read and the evidence read keeps the row the claim read had marked dead and counts one skip | sweep | unaudited |
| `an_empty_page_reads_no_kernel_row_and_still_refreshes_the_gauge` | `:970` | an empty page takes no Kernel reader, counts no skip, and refreshes `retained_bytes` | sweep | unaudited |
| `every_claim_chunk_of_a_page_is_read` | `:1047` | two claim chunks | sweep | unaudited |
| `the_cleanup_pass_carries_the_watermark_and_reads_the_flag_each_pass` | `:1092` | lifecycle wiring, flag per pass, cancel | sweep, flag off | unaudited |
| `the_evidence_read_refuses_another_incarnation_a_future_snapshot_and_a_negative_one` | `:1161` | incarnation, future-snapshot, and negative-snapshot refusals | sweep | unaudited |
| `a_busy_reader_does_not_hold_the_sweep_past_its_budget` | `:1203` | one held reader: the sweep's scope reads take the free reader and the run stays inside its budget | sweep | unaudited |
| `the_budgeted_scope_read_answers_as_the_unbudgeted_one_and_stops_at_its_deadline` | `crates/kernel/tests/kernel_scope.rs:460` | `scope_terms_within_budget` equals `scope_terms`, answers `None` for a missing scope, and returns `Deadline` on an expired budget and on a fully held reader pool | sweep | unaudited |
| `every_standing_equals_the_claim_facts_of_its_claim_at_every_snapshot`, `standing_refusals_match_the_claim_facts_refusals` | `crates/kernel/tests/kernel_claim_standing.rs:111`, `:168` | the sweep's claim standing read equals the registry row, decision row, own admission row, and served standing of claim facts at every snapshot, and refuses as claim facts does | sweep | unaudited |

Also: `crates/daemon/src/config.rs:2299` and `:2320` (default, tier, and
resolved-config freezing), `crates/daemon/src/lib.rs:21502` (bound route and
status block), `crates/host-runtime/src/control.rs:2011` (host forwarding
allowlist against `docs/host-wire-protocol.md`), and
`a_delete_as_read_spares_a_row_rewritten_since_the_read`
(`crates/memory-store/tests/contradiction_candidates.rs:791`).

## S-B trigger and S-C candidate set (`crates/daemon/src/candidate_pass_tests.rs`, #1167)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `a_route_without_consent_offers_no_pass_and_writes_no_row` | `:252` | consent off on the offer: zero counters, no row | flag off (pass arm) | unaudited |
| `a_pass_starts_only_after_the_job_releases_the_slot_and_the_gate` | `:283` | no offer while the job holds the slot; slot idle and gate free at start and while scoring; memo hit offers nothing; replay rescored | trigger, row key | unaudited |
| `a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass` | `:399` | busy refusal; cancel at the second of three descriptors leaves the first one's rows | single flight, partial result | unaudited |
| `a_pass_queued_past_its_budget_ends_at_the_deadline_it_was_admitted_under` | `:466` | a pass queued behind a full blocking pool past its budget ends `deadline` at its first check, writes nothing, and holds the permit while queued | single flight, partial result | unaudited |
| `the_candidate_set_keeps_the_newest_scope_matched_decisions_of_the_project_and_counts_the_cut` | `:664` | N + 2 at a reduced N; other project never scored; each cut decision counted | candidate set | unaudited |
| `a_superseded_decision_is_never_scored_and_the_default_bounds_are_the_named_constants` | `:712` | superseded unread; production bounds pinned | candidate set | unaudited |
| `the_summary_byte_cut_skips_payloads_before_any_load` | `:776` | byte cut before load, counted | candidate set | unaudited |
| `an_oversize_descriptor_writes_nothing_and_an_unscorable_summary_leaves_the_others` | `:804` | descriptor one byte over refused; over-bound and floor-emptied summaries unscorable | oversize (pass arm) | unaudited |
| `a_deadline_during_the_third_descriptor_leaves_the_first_two` | `:836` | deadline during the third of five: full top-K for the first two | partial result | unaudited |
| `a_budget_spent_before_the_pass_writes_nothing_and_counts_every_descriptor` | `:868` | spent budget: no row, all unscored, one deadline | partial result | unaudited |
| `the_row_caps_hold_per_descriptor_per_pass_and_per_project_and_a_replay_keeps_the_rows` | `:888` | K, M, a write over the project cap, newer-tip rescore, held rows past M with and without candidates at the tip | row key | unaudited |
| `each_descriptor_keeps_the_top_k_that_score_descriptor_ranks_over_the_loaded_summaries` | `:979` | the pass's single scan writes, per descriptor, the rows `score_descriptor` ranks over the loaded summaries | determinism, candidate set | unaudited |
| `a_rerun_from_the_retained_bytes_at_one_tip_rebuilds_the_rows_and_the_kernel_is_untouched` | `:1083` | retained texts equal published texts; truncate and rescore rebuild the rows; tip and jobs unchanged | rebuild, R5 | unaudited |
| `the_route_consent_reaches_the_pass_through_a_transform` | `:1166` | the route's frozen value, not the daemon's, decides the offer | flag off (pass arm), trigger | unaudited |
| `a_panicking_pass_releases_its_permit_and_charge_and_shutdown_waits_for_a_parked_pass` | `:1216` | panic counted with permit and charge released; kept-text charge below the job's; shutdown waits | single flight | unaudited |
| `publish_latency_with_candidates_on_stays_within_the_off_baseline` | `:1274` | median publication hold, with at least one publication beside a running pass, within 3x + 50 ms of the flag-off median | trigger (C1) | unaudited |
| `equal_texts_in_one_batch_cite_one_evidence_row_and_the_pass_scores_it_once` | `:1374` | two equal-text units publish as two units citing one evidence row; the pass analyzes one descriptor and writes its rows once | row key | unaudited |
| `a_decision_row_that_disagrees_with_its_registry_row_ends_the_pass_and_keeps_the_held_rows` | `:1399` | a decision whose typed row disagrees with its registry row ends the pass `kernel_unavailable` through the claim-facts read, with the descriptor's held rows unchanged | candidate set | unaudited |
| `a_pass_stopped_during_the_scan_counts_the_summaries_the_analyzer_refused_before_the_stop` | `:1437` | a stop at the check after 256 summaries ends `budget_exhausted` with the three summaries the analyzer refused before it counted `summaries_unscorable` | partial result | unaudited |
| `a_pass_stopped_between_payload_chunks_counts_the_summaries_already_loaded` | `:1471` | a stop before the second payload chunk ends `budget_exhausted` with the first chunk counted `summaries_loaded` | partial result | unaudited |
| `a_restore_between_the_reads_and_the_write_ends_the_pass_and_keeps_the_held_rows` | `:1496` | a Kernel restore after the pass's last read ends it `kernel_unavailable` before any write: every descriptor unscored, held rows in place | partial result | unaudited |
| `a_write_behind_a_held_store_connection_ends_at_the_pass_deadline` | `:1546` | a store connection another user holds past the budget ends the pass `deadline` at the write, in under five seconds, with no row; the scored descriptor is analyzed, and the end count names its unwritten rows | partial result | unaudited |
| `a_job_whose_document_capture_ends_in_deadline_still_offers_its_published_units` | `:1617` | a window batch that committed before the document batch ended the job `deadline` is offered to a pass: one pass, one row | trigger | unaudited |
| `a_pass_queued_past_its_budget_ends_at_the_deadline_when_the_kernel_is_gone_too` | `:480` | a pass queued past its budget whose Kernel is gone when it starts ends `deadline`, not `kernel_unavailable` | single flight, partial result | unaudited |
| `a_visible_decision_read_behind_held_kernel_readers_ends_at_the_pass_deadline` | `:1720` | both Kernel readers held from the target capture: the visible-decision read ends the pass `deadline` within the budget | partial result | unaudited |
| `a_scope_term_read_behind_held_kernel_readers_ends_at_the_pass_deadline` | `:1725` | both Kernel readers held from the first scope-filter check: the scope-term read ends the pass `deadline` within the budget | partial result | unaudited |
| `a_payload_size_read_behind_held_kernel_readers_ends_at_the_pass_deadline` | `:1730` | both Kernel readers held after the eligibility judgment: the payload-size read ends the pass `deadline` within the budget | partial result | unaudited |

## S-F read, S-G read and pass arms, S-H confirm (#1168)

Rust checks are in `crates/daemon/src/candidate_pass_tests.rs`; TypeScript
checks are named with their files.

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `review_candidates_pages_live_rows_and_answers_each_terminal` | `:1849` | 8-key items; clamps at 65, 0, a number past the machine word, and an integral float; order and disjoint two-page walk; four stale rows filtered and counted; `not_live` for superseded and unknown; empty page, including for a live decision whose id holds a control character; `invalid_params` bodies; scope refusal before the body; observational read; flag off with the table hidden and a flag-on control; `disabled` and `store_unavailable` before a malformed or non-string cursor; tip, jobs, and rows unchanged | read serves, read terminals, flag off (read arm) | unaudited |
| `review_candidates_answers_the_store_and_kernel_states_before_any_page` | `:2562` | Kernel starting, Kernel open failed, and no Memory Store answer their states without `items` | read terminals | unaudited |
| `review_candidates_reads_scope_terms_inside_its_budget_beside_a_held_reader` | `:2246` | with one Kernel reader held for the whole request, two reads answer pages inside the read budget: the scope-term read takes the free reader | read serves | unaudited |
| `review_candidates_refuses_a_target_whose_history_a_restore_replaced_before_the_scope_read` | `:2286` | a restore between the claim-standing read and the scope-terms read answers the Kernel's conflict state, never `not_live` | read serves, read terminals | unaudited |
| `review_candidates_answers_store_unavailable_when_the_store_connection_outlasts_its_budget` | `:2500` | a store connection held for 9 s answers `store_unavailable` inside the read budget | read terminals | unaudited |
| `review_candidates_answers_disabled_when_authority_drains_before_the_page_read` | `:2342` | memories authority begins draining between the entry check and the page read; the read answers `disabled` | read terminals | unaudited |
| `review_candidates_answers_disabled_when_authority_drains_before_a_not_live_judgment` | `:2383` | memories authority begins draining while a superseded decision is judged; the read answers `disabled` in place of `not_live` | read terminals | unaudited |
| `review_candidates_queued_past_its_budget_answers_at_the_deadline_it_was_admitted_under` | `:2448` | a read queued behind a saturated blocking pool for three budgets answers `store_unavailable` when it runs | read terminals | unaudited |
| `a_listed_candidate_confirms_through_dispute_while_its_evidence_lives` | `:2630` | listed evidence id is a published descriptor's; confirm lands `disputed` with the table truncated; after retention the row is filtered and the earlier form is `not_found` | confirm linkage | unaudited |
| `the_confirm_path_never_reads_the_candidate_table` | `:2770` | `commit.rs` and `dispute.rs` name no candidate store method (lexical) | confirm linkage | unaudited |
| `a_publication_without_consent_runs_no_candidate_table_statement` | `:2788` | consent-off publications succeed with the table hidden and every counter zero; a consenting publication's pass fails on it | flag off (pass arm) | unaudited |
| `eidnara_memory candidates` | `packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1582` | default read without a tool-call id; request body; confirm form; terminals by name; extra-key refusal; dispute-admission note; foreign-decision and paging refusals | read serves (client), confirm linkage | unaudited |
| `Pi eidnara_memory candidates` | `packages/pi-plugin/src/tools/eidnara-memory.test.ts:532` | default schema with paging fields; `disabled` rendered; dispute-admission note from the Pi allowlist | read terminals, flag off (read arm) | unaudited |
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
  (`crates/daemon/src/contradiction_candidates/pass.rs:403`); no test reads
  the digest back from a pass-written row.
- No test fails a store write inside a pass; that path counts the failed
  descriptor as analyzed and the later ones as unscored.
