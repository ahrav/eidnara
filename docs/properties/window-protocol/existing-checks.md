# Existing checks: window protocol (M1)

Status is per METHOD: `unaudited` means the check exists and ran green in the
named recorded PR run but has had no independent adequacy review. Adequacy
belongs to `/testing:invariant-test-review`; production guard adequacy belongs
to `/low-level-systems:defensive-assertions-and-invariant-guards`. The
revision 3 checks ran in the recorded gates of #881 (#831, head `1c66f16c`),
#883 (#832 PR one, head `d7712d75`), and #884 (#832 PR two, head `f8734c12`);
the #833 checks ran in the gates the #833 PR description records (gates at
the final head, code at `3ebfc3b9`).

Every test name below was located with `git grep` at the named tree. Unless a
row names another tree, `file:line` is at `f2442b2f`, the last code commit of
#833 (`window-protocol/m1-exit`, base `main` `d68aedf3`). Lines read at
`f8734c12` were moved by a line diff and each name was found again at
`f2442b2f`. A test deleted by #833 is cited at `f8734c12`. `3ebfc3b9`
changes only the test-support statement ledger and the #874 inventory test;
rows citing those files are marked at `3ebfc3b9`.

## Daemon and store

| Check | Location | Records | Recorded run | Status |
| --- | --- | --- | --- | --- |
| `empty_store_bootstrap_then_defers_stably_without_hard_oscillation` | `crates/daemon/src/transform.rs:19100` | WP-E01 | #880 | unaudited |
| `reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard` | `crates/daemon/src/transform.rs:19420` | WP-E01 | #880 | unaudited |
| `reconcile_rematerialize_with_unrecut_store_truncates_and_refolds_prefix` | `crates/daemon/src/transform.rs:19455` | WP-E02 | #880 | unaudited |
| `reconcile_recut_nothing_survives_arms_pending_raw_without_truncate` | `crates/daemon/src/transform.rs:19785` | WP-E03 | #880 | unaudited |
| `pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile` | `crates/daemon/src/transform.rs:19950` | WP-E03 | #880 | unaudited |
| `pending_rewrite_persists_across_store_restart` | `crates/daemon/src/transform.rs:20017` | WP-E03 | #880 | unaudited |
| `selected_range_identity_drift_during_await_rejects_without_cooldown` | `crates/daemon/src/history_summarizer.rs:3799` | WP-E04 | #873 | unaudited |
| `tail_identity_extension_during_await_still_publishes` | `crates/daemon/src/history_summarizer.rs:3850` | WP-E04 | #873 | unaudited |
| `publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict` | `crates/memory-store/src/lib.rs:26308` | WP-E04 | #873 | unaudited |
| `publish_rejects_a_firing_whose_set_was_truncated_and_regrown_to_the_same_maximum` | `crates/memory-store/src/lib.rs:22983` | WP-E04 | #873 | unaudited |
| `native_previous_keeps_bind_the_applied_revision` | `crates/daemon/src/lib.rs:25487` | WP-E05 | #879 | unaudited |
| `transform_snapshot_cache_is_generation_safe_and_lru_bounded` | `crates/daemon/src/lib.rs:21135` | WP-E08 | #878 | unaudited |
| `snapshot_lease_budget_survives_cache_churn_and_releases_exact_charge` | `crates/daemon/src/lib.rs:21175` | WP-E08 | #878 | unaudited |
| `ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` | `crates/daemon/src/lib.rs:21230` | WP-E08 | #878 | unaudited |
| `window_tag_read_keeps_every_session_relative_tag_decision` | `crates/daemon/src/transform.rs:22558` | WP-E09 | #874 | unaudited |
| `shared_row_iterator_matches_slice_for_protected_legacy_orphan` | `crates/daemon/src/tail_hygiene.rs:2564` | WP-E09 | #874 | unaudited |
| `first_hard_pass_meta_respects_the_store_durable_text_bound` | `crates/daemon/src/transform_meta_bound.rs:130` at `f8734c12` | WP-E10 (deleted by #833, `7a8fb84b`) | #873, #881 | unaudited |
| `meta_bytes_stay_flat_as_covered_history_grows` | `crates/daemon/src/transform_meta_bound.rs:141` at `f8734c12` | WP-E10 (deleted by #833, `7a8fb84b`) | #873 (named `..._and_covered_drift_still_rejects` at `43e88bdc`); #881 (renamed, covered-drift half dropped) | unaudited |
| `native_output_store_enforces_entry_cap_lru_and_revert_epoch` | `crates/daemon/src/lib.rs:25333` | WP-E12, WP-E05 | #879 | unaudited |
| `handler_native_cache_adopts_the_bumped_durable_revert_epoch` | `crates/daemon/src/lib.rs:25284` | WP-E12 | #879 | unaudited |
| `truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch` | `crates/memory-store/src/lib.rs:26148` | WP-E02, WP-E12 | #879 | unaudited |
| `serialized_output_cache_revert_epoch_bump_evicts_session` | `crates/daemon/src/transform.rs:28657` at `f8734c12` | WP-E12 (deleted with the memo by #833, `6477c9f2`) | #879 | unaudited |
| `multiple_large_sessions_do_not_ping_pong_under_the_native_output_total_budget` | `crates/daemon/src/lib.rs:25438` | WP-E12 (budget) | #879 | unaudited |
| `handler_small_native_lru_eviction_and_refusal_still_serve` | `crates/daemon/src/lib.rs:25120` | WP-E05, WP-E12 | #879 | unaudited |
| `handler_warm_normalization_matches_cold_when_a_reserved_todo_follows_the_prefix` | `crates/daemon/src/lib.rs:25224` | WP-E05 | #879 | unaudited |
| `each_resolution_outcome_has_its_cut_ordinals_and_keeps` | `crates/daemon/src/window_coverage/tests.rs:99` | WP-P02 | #875 | unaudited |
| `impossible_declarations_are_invalid_params` | `crates/daemon/src/window_coverage/tests.rs:217` | WP-P01, WP-P02 | #875 | unaudited |
| `a_declared_row_without_a_rendered_boundary_is_unknown` | `crates/daemon/src/window_coverage/tests.rs:237` | WP-P02 | #875 | unaudited |
| `a_publish_between_the_core_read_and_the_declared_read_is_invisible` | `crates/daemon/src/window_coverage/tests.rs:287` | WP-P02 | #875 | unaudited |
| `a_removal_between_the_core_read_and_the_intersection_is_invisible` | `crates/daemon/src/window_coverage/tests.rs:317` | WP-P02 (the description's "null-path barrier test", identified by content) | #875 | unaudited |
| `the_null_anchor_intersection_matches_mids_at_their_positional_ordinals` | `crates/daemon/src/window_coverage/tests.rs:343` | WP-P02, WP-P07 | #875 | unaudited |
| `synthetic_borrowing_matches_the_plugin_rule_case_for_case` | `crates/daemon/src/window_coverage/tests.rs:412` | WP-P03 | #875 | unaudited |
| `resolved_ordinals_equal_the_independent_model` | `crates/daemon/src/window_coverage/tests.rs:436` | WP-P03 | #875 | unaudited |
| `a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input` | `crates/daemon/src/window_coverage/tests.rs:513` | WP-P05, WP-P19 | #875 | unaudited |
| `intersection_and_page_work_is_independent_of_history_length` | `crates/daemon/src/window_coverage/tests.rs:591` | WP-P07 | #875 | unaudited |
| `a_long_run_of_unlisted_rows_does_not_end_the_walk` | `crates/daemon/src/window_coverage/tests.rs:622` | WP-P10 (the description's "unlisted-run test (4,499 rows)", identified by content) | #875 | unaudited |
| `the_sql_anchor_grammar_matches_split_block_id` | `crates/daemon/src/window_coverage/tests.rs:687` | WP-P10 | #875 | unaudited |
| `boundary_walk_is_exhaustive_newest_first_and_bounded_by_the_rendered_row` | `crates/daemon/src/window_coverage/dispatch_tests.rs:45` | WP-P10 | #875 | unaudited |
| `boundary_page_is_empty_without_a_rendered_boundary` | `crates/daemon/src/window_coverage/dispatch_tests.rs:72` | WP-P10 | #875 | unaudited |
| `malformed_boundary_bodies_are_invalid_params` | `crates/daemon/src/window_coverage/dispatch_tests.rs:81` | WP-P10, WP-P24 | #875 | unaudited |
| `session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order` | `crates/daemon/src/transform_unit/tests.rs:1016` | WP-P04, WP-P18 | #875 | unaudited |
| `session_lane_activates_in_join_order_under_contention` | `crates/daemon/src/transform_unit/tests.rs:1141` | WP-P04, WP-P18 | #875 | unaudited |
| `session_lane_waiter_dropped_after_hand_off_releases_the_lane` | `crates/daemon/src/transform_unit/tests.rs:1202` | WP-P04 | #875 | unaudited |
| `session_lane_handed_off_waiter_dropped_unpolled_wakes_the_next` | `crates/daemon/src/transform_unit/tests.rs:1214` | WP-P04 | #875 | unaudited |
| `session_lane_waiter_leaving_before_hand_off_frees_the_slot` | `crates/daemon/src/transform_unit/tests.rs:1226` | WP-P04 | #875 | unaudited |
| `cancelled_pass_keeps_its_lane_place_until_its_blocked_unit_finishes` | `crates/daemon/src/transform_unit/tests.rs:1083` | WP-P04 | #875 | unaudited |
| `emergency_cancellation_between_units_preserves_commit_and_releases_scratch` | `crates/daemon/src/transform_unit/tests.rs:791` | WP-P04 | #875 | unaudited |
| `waiting_unpaged_pass_is_refused_when_a_page_stream_started_meanwhile` | `crates/daemon/src/transform_unit/tests.rs:1241` | WP-P04 | #875 | unaudited |
| `bounded_fold_matches_the_full_read_over_the_store_shape_fixture` | `crates/daemon/src/m0_compose.rs:565` | WP-P08 | #873 | unaudited |
| `bounded_fold_matches_the_full_read_over_sixty_thousand_segments` | `crates/daemon/src/m0_compose.rs:636` | WP-P08 | #873 | unaudited |
| `bounded_fold_work_is_independent_of_history_length` | `crates/daemon/src/m0_compose.rs:710` | WP-P07, WP-P08 | #873 | unaudited |
| `bounded_m1_matches_the_full_read_and_withholds_an_overflowing_body` | `crates/daemon/src/m0_compose.rs:742` | WP-P08 | #873 | unaudited |
| `additive_soft_retries_when_rows_past_the_m1_cap_land_mid_pass` | `crates/daemon/src/transform.rs:14955` | WP-P08 | #873 | unaudited |
| `writers_that_add_a_legacy_row_clear_the_persisted_legacy_list` | `crates/memory-store/src/lib.rs:22858` | WP-P08 | #873 | unaudited |
| `per_pass_history_reads_do_constant_work_as_history_grows` | `crates/memory-store/src/lib.rs:22466` | WP-P07 | #873 | unaudited |
| `transform_snapshot_reads_only_the_named_blocks_overlays` | `crates/memory-store/src/lib.rs:22509` | WP-P07 | #873 | unaudited |
| `foreign_block_overlays_leave_the_pass_unchanged` | `crates/daemon/src/transform.rs:19172` | WP-P07 | #873 | unaudited |
| `state_sync_refuses_rows_that_overlap_stored_neighbours` | `crates/memory-store/src/lib.rs:22605` | WP-P07, WP-P08 (range invariant) | #873 | unaudited |
| `initialized_state_sync_skips_retained_rows_and_refuses_an_overlapping_new_row` | `crates/memory-store/src/lib.rs:22664` | WP-P07 | #873 | unaudited |
| `state_sync_refuses_duplicate_sequences_in_one_batch` | `crates/memory-store/src/lib.rs:22726` | WP-P07 | #873 | unaudited |
| `every_pass_read_is_bounded_independent_of_history_size` | `crates/daemon/src/transform_read_bound.rs:627` (at `3ebfc3b9`) | WP-P07, WP-P15 | #874 (at `704568ec`: `:537`); bytes bound `3ebfc3b9` | unaudited |
| Statement-work ledger tests (four, `crates/storage`) | `crates/storage/src/lib.rs` (hook at `:306`) | WP-P07 | #873 (`test -p storage` 107); `statement_work_counts_the_bytes_of_every_returned_value` (`:6401`, `3ebfc3b9`) | unaudited; #873's names not listed in the description |
| `a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change` | `crates/daemon/src/transform/revision_3.rs:158` | WP-P24 | #881 | unaudited |
| `boundary_presence_head_sequence_and_duplicates_are_invalid_params` | `crates/daemon/src/transform/revision_3.rs:193` | WP-P01, WP-P24 | #881 | unaudited |
| `a_boundary_that_does_not_decode_is_bad_request` | `crates/daemon/src/transform/revision_3.rs:231` | WP-P24 | #881 | unaudited |
| `paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request` | `crates/daemon/src/transform/revision_3.rs:287` | WP-P01, WP-P24 | #881 | unaudited |
| `each_resolution_outcome_runs_through_the_handler_with_its_effects` | `crates/daemon/src/transform/revision_3.rs:323` | WP-P02, WP-P03, WP-E03 (pass-through arm) | #881 | unaudited |
| `a_revert_through_no_anchor_outside_the_pending_rewrite_arm_removes_every_segment` | `crates/daemon/src/transform/revision_3.rs:824` | WP-E03 (context; not the #833 reset) | #881 | unaudited |
| `a_cas_conflict_after_the_unanchored_revert_removal_converges` | `crates/daemon/src/transform/revision_3.rs:844` | WP-E03 (context) | #881 | unaudited |
| `a_panic_plus_reopen_after_the_unanchored_revert_removal_converges` | `crates/daemon/src/transform/revision_3.rs:859` | WP-E03 (context) | #881 | unaudited |
| `a_publish_during_a_null_boundary_pass_keeps_the_cut_at_the_rendered_boundary` | `crates/daemon/src/transform/revision_3.rs:876` | WP-P02 | #881 | unaudited |
| `a_reset_that_moves_the_cut_leaves_the_served_window_in_the_ready_snapshot` | `crates/daemon/src/transform/revision_3.rs:917` | WP-P02 | #881 | unaudited |
| `stale_slice_input_keeps_address_the_submitted_native_window` | `crates/daemon/src/transform/revision_3.rs:988` | WP-P05, WP-P19, WP-E05 | #881 | unaudited |
| `a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` | `crates/daemon/src/transform/revision_3.rs:1147` | WP-E02, WP-E12 | #881 | unaudited |
| `a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass` | `crates/daemon/src/transform/revision_3.rs:1163` | WP-E02, WP-E12 | #881 | unaudited |
| `a_pending_reconcile_renders_the_newest_row_the_truncate_left` | `crates/daemon/src/window_coverage/tests.rs:254` | WP-P02, WP-E02 | #881 | unaudited |
| `a_null_anchor_hit_above_the_rendered_row_is_not_cut_at` | `crates/daemon/src/window_coverage/tests.rs:580` | WP-P02 | #881 | unaudited |
| `a_legacy_meta_row_with_the_retired_divergence_counter_loads` | `crates/memory-store/src/lib.rs:25989` | WP-P25 (decode compatibility only) | #881 | unaudited |
| `revision_3_replays_the_revision_2_goldens` | `crates/daemon/src/transform/revision_goldens.rs:428` (fixture `crates/daemon/tests/fixtures/transform-revision-2-goldens.json`, 9 traces) | WP-P02, WP-P03, WP-P05 (acceptance differential; after a revert before the first anchor it compares against a fresh session's step instead of the revision 2 golden, and after a pass revision 2 refused as covered drift the golden has no output to compare until such a reset, `revision_goldens.rs:421-426`; it is not strict equivalence with revision 2) | #881 | unaudited |
| `a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass` | `crates/daemon/src/transform/revision_3.rs:447` | WP-E03 (D10 reset contrast), WP-P03 | #833 | unaudited |
| `a_cas_conflict_on_the_no_survivor_reset_resolves_again_and_resets_once` | `crates/daemon/src/transform/revision_3.rs:498` | WP-E03 | #833 | unaudited |
| `a_revert_under_pressure_defers_then_folds_and_prunes_to_the_window` | `crates/daemon/src/transform/revision_3.rs:567` | WP-P06 (revert prunes only at its HARD), WP-E02 | #833 | unaudited |
| `a_no_survivor_reset_that_always_loses_its_cas_surfaces_the_conflict` | `crates/daemon/src/transform/revision_3.rs:610` | WP-E03 (retry bound) | #833 | unaudited |
| `a_committed_reset_leaves_the_whole_retry_budget_to_the_pass` | `crates/daemon/src/transform/revision_3.rs:635` | WP-E03 (retry bound) | #833 | unaudited |
| `a_second_no_survivor_resolution_in_one_pass_fails_with_a_cas_conflict` | `crates/daemon/src/transform/revision_3.rs:657` | WP-E03 (one reset per pass) | #833 | unaudited |
| `a_same_pass_reset_reuses_no_previous_output` | `crates/daemon/src/transform/revision_3.rs:701` | WP-E12, WP-E05 | #833 | unaudited |
| `a_prune_that_commits_first_fences_the_publication_out` | `crates/daemon/src/transform.rs:19708` | WP-P06, WP-P16, WP-E04 | #833 | unaudited |
| `a_publication_that_commits_first_makes_the_transform_reload_and_match_the_serial_run` | `crates/daemon/src/transform.rs:19741` | WP-P06, WP-P16 | #833 | unaudited |
| `serialized_output_cache_take_under_a_new_epoch_returns_nothing` | `crates/daemon/src/transform.rs:27963` | WP-E12 | #833 | unaudited |
| `serialized_output_cache_evicts_the_least_recently_recorded_session` | `crates/daemon/src/transform.rs:27976` | WP-E12 (budget) | #833 | unaudited |
| `a_hundred_thousand_message_session_commits_a_three_hundred_message_window` | `crates/daemon/src/transform_meta_bound.rs:93` | WP-E10, WP-P07 | #833 | unaudited |
| `a_legacy_row_is_read_after_a_restart_and_pruned_on_its_first_commit` | `crates/daemon/src/transform_meta_bound.rs:177` | WP-P25 | #833 | unaudited |
| `a_legacy_prune_that_loses_its_cas_reloads_and_prunes` | `crates/daemon/src/transform_meta_bound.rs:195` | WP-P25 | #833 | unaudited |
| `a_writer_that_loses_to_the_legacy_prune_reloads_the_pruned_row` | `crates/daemon/src/transform_meta_bound.rs:230` | WP-P25 | #833 | unaudited |
| `an_unsafe_negative_sequence_is_not_listed` | `crates/daemon/src/window_coverage/tests.rs:655` | WP-P10, WP-P24 (`2d58cc20`) | #881, #833 (after #875's description run) | unaudited |
| `one_unpaged_three_mb_transform_body_commits` | `crates/daemon/tests/direct_host.rs:136` | WP-P15 (one unpaged transform) | #833 | unaudited |

## Plugin

Paths are relative to `packages/opencode-plugin/src/hooks/context/`.

| Check | Location | Records | Recorded run | Status |
| --- | --- | --- | --- | --- |
| "agree with the shared cases on acceptance and exact output" | `edit-recipe.test.ts:69` | WP-E05 | #877, #878 | unaudited |
| "keeps a previous entry by reference and never edits either source" | `edit-recipe.test.ts:117` | WP-E05 | #877, #878 | unaudited |
| "rejects mutated retained output ${mutationTime}" | `rust-mode-transform.test.ts:1432` | WP-E05 | #877, #878 | unaudited |
| "keeps from the applied previous output and the submitted input, then acks its note deliveries" | `rust-mode-transform.test.ts:1488` | WP-E05 | #877, #878 | unaudited |
| "rejects containers whose element or length assignment could throw" | `transform-capture.test.ts:1193` | WP-E06 (rewritten for `publicationRejection` by #877), WP-P09 | #877 | unaudited |
| "keeps counts and charges equal to a reference model across generated lease sequences" | `transform-capture.test.ts:1213` | WP-E07 | #877 | unaudited |
| "holds one lease per session, declines a newer call, and cancels the holder" | `transform-capture.test.ts:1344` | WP-E07 | #877 | unaudited |
| "charges bytes against one aggregate budget and releases them exactly once" | `transform-capture.test.ts:1370` | WP-E07 | #877 | unaudited |
| "never releases an active capture lease when the ${limit} evicts its session's output" | `rust-mode-transform.test.ts:1319` | WP-E07 | #877 | unaudited |
| "does not resend after an outcome-unknown transport failure and recovers on the next attempt" | `rust-mode-transform.test.ts:3302` | WP-E11 | #877, #878 | unaudited |
| "declines a daemon ${status} pass without publishing or promoting" | `rust-mode-transform.test.ts:441` | WP-P04 | #875 | unaudited |
| "sends the whole captured array on every pass with no delta or fingerprint" | `rust-mode-transform.test.ts:385` | revision 2 interim (#877); superseded by the window test | #877 | unaudited |
| "shrinks first and leaves exactly the candidate in the original array object" | `transform-capture.test.ts:1067` | WP-P09 | #877 | unaudited |
| "restores the captured references after a shrink stopped by a planted non-configurable slot" | `transform-capture.test.ts:1098` | WP-P09, WP-P20 | #877 | unaudited |
| "reports a throw while restoring the captured references" | `transform-capture.test.ts:1153` | WP-P09 | #877 | unaudited |
| "leaves exactly a shorter candidate in the original array object" | `rust-mode-transform.test.ts:3236` | WP-P09 | #877 | unaudited |
| "restores the captured references and promotes nothing when a planted slot stops the shrink" | `rust-mode-transform.test.ts:3252` | WP-P09, WP-P20 | #877 | unaudited |
| "appends the captured window after a shrink stopped at a covered or window slot" | `transform-capture.test.ts:1138` | WP-P09 | #883, #884 | unaudited |
| "crosses planted proxies, accessors, and revoked proxies without invoking any hook" | `rust-mode-window.test.ts:123` | WP-P13, WP-P23 | #883, #884 | unaudited |
| "holds every present id, refuses an unaffordable filter, and retains no id string" | `rust-mode-window.test.ts:166` | WP-P13, WP-E07 | #883, #884 | unaudited |
| "sends the declared window in both representations and publishes the recipe at boundaryIndex + i" | `rust-mode-window.test.ts:182` | WP-P01, WP-P05, WP-E05 | #883, #884 | unaudited |
| "rejects a duplicate id inside the window and ignores one outside it" | `rust-mode-window.test.ts:228` | WP-P01 | #883, #884 | unaudited |
| "declines a matched boundary that fails the hostile walk instead of picking another" | `rust-mode-window.test.ts:256` | WP-P13 | #883, #884 | unaudited |
| "declines ${name} during the await with no candidate write and no promotion" (4 cases) | `rust-mode-window.test.ts:302` | WP-P12, WP-P17, WP-P22 | #883, #884 | unaudited |
| "declares a match found past page one" | `rust-mode-window.test.ts:341` | WP-P10, WP-P21 | #883, #884 | unaudited |
| "sends null with the whole array only after an empty page" | `rust-mode-window.test.ts:486` | WP-P10 | #883, #884 | unaudited |
| "verifies a filter hit by id scan and rejects a colliding absent id" | `rust-mode-window.test.ts:502` | WP-P10 | #883, #884 | unaudited |
| "declines ${name} without sending null" (6 cases) | `rust-mode-window.test.ts:562` | WP-P10, WP-P24 | #883, #884 | unaudited |
| "declines when the time budget fires with no null submission" | `rust-mode-window.test.ts:589` | WP-P10, WP-P21 | #883, #884 | unaudited |
| "rediscovers once after boundary_unknown and declines the second in one pass" | `rust-mode-window.test.ts:607` | WP-P10 | #883, #884 | unaudited |
| "serves raw and logs an upgrade hint when the daemon refuses the revision" | `rust-mode-window.test.ts:938` | WP-P24 | #883, #884 | unaudited |
| "stays equal at a fixed window for 10k and 100k host messages" | `rust-mode-window.test.ts:998` | WP-P15 | #883, #884 | unaudited |
| "appends exactly the unacknowledged window suffix and promotes no basis" | `rust-mode-window.test.ts:1031` | WP-P11 | #884 | unaudited |
| "serves raw against a mismatched basis anchor or a changed terminal message" | `rust-mode-window.test.ts:1207` | WP-P11 | #884 | unaudited |
| "takes the verdict from the earliest user row in both signal directions" | `rust-mode-window.test.ts:1278` | WP-P14 | #884 | unaudited |
| "freezes fail-open without a database and stays fail-closed for an unpersisted session" | `rust-mode-window.test.ts:1293` | WP-P14 | #884 | unaudited |
| "declines when the host moves the discovered anchor before the window is copied" | `rust-mode-window.test.ts:845` | WP-P01, WP-P12, WP-P22 | #883, #884 | unaudited |
| "declines cleanly when the host replaces its root array before a rerun" | `rust-mode-window.test.ts:734` | WP-P12 | #883, #884 | unaudited |
| "rediscovers and publishes when an anchor it just discovered draws boundary_unknown" | `rust-mode-window.test.ts:647` | WP-P10 | #883 (review update, gates at `adcc7baf`) | unaudited |
| "declines a rediscovery once a slow first send spent the pass's discovery budget" | `rust-mode-window.test.ts:754` | WP-P10, WP-P21 | #883, #884 | unaudited |
| "refunds the first attempt so a rediscovered window near the byte limit still publishes" | `rust-mode-window.test.ts:802` | WP-E07 | #883, #884 | unaudited |
| "carries v and boundary ${JSON.stringify(boundary)} on the final page only" | `module-wire.test.ts:901` | WP-P24 | #883, #884 | unaudited |
| "serves raw when a rerun ${...} after rediscovering the basis the daemon disowned" | `rust-mode-window.test.ts:1092` | WP-P11 | #884 | unaudited |
| "appends the unacknowledged window suffix on a ${status} decline" | `rust-mode-window.test.ts:1061` | WP-P11 | #884 | unaudited |
| "serves the last applied output plus the appended messages on a ${status} decline" | `rust-mode-transform.test.ts:3394` | WP-P11, WP-E11 (`1431467c` declines, ported) | #884 | unaudited |
| "serves the last applied output plus the appended messages on a capture_bytes decline" | `rust-mode-transform.test.ts:3653` | WP-P11, WP-P01 (admission cap) | #884 | unaudited |
| "keeps separate eidnara_reduce and todowrite verdicts for one session" | `eidnara-reduce-availability.test.ts:93` | WP-P14 | #884 | unaudited |
| "finds a cold start's anchor with one backward scan of the window's distance" | `rust-mode-window.test.ts:384` | WP-P15, WP-P10 (D17) | #833 | unaudited |
| "matches a cold start's first page by the daemon's top-level id fallback" | `rust-mode-window.test.ts:405` | WP-P10, WP-P13 | #833 | unaudited |
| "builds the membership filter only after the first page misses the whole host" | `rust-mode-window.test.ts:417` | WP-P15, WP-P10 | #833 | unaudited |
| "declares the anchor latest in the host on either path, the newer of two sharing a mid" | `rust-mode-window.test.ts:436` | WP-P01, WP-P02 | #833 | unaudited |
| "sends a body above the page limit unpaged while it fits the daemon's transform limit", "pages a body above its unpaged limit", "pins the unpaged limit to the daemon's transform body limit" | `module-wire.test.ts:815`, `:827`, `:840` | WP-P24, WP-P15 | #833 | unaudited |

## Goldens and end-to-end checks

| Check | Location | Records | Recorded run | Status |
| --- | --- | --- | --- | --- |
| Decay goldens `render-golden.json`, `decay-store-shape.json` (388 rows), `decay-store-differential.json`, `render-tight-golden.json` | `crates/daemon` test fixtures | WP-P08, hot-path H1, H2 | #873 | unaudited |
| Edit-recipe fixtures and generated single-fault mutations | shared recipe fixtures | WP-E05 | #879 | unaudited |
| e2e steady-state byte identity | `packages/e2e-tests` (`test:rust`) | WP-E05, served bytes | #879 (23 pass, 17 skip, 0 fail) | unaudited |
| e2e `test:rust` with the revision 2 plugin against the revision 3 daemon | `packages/e2e-tests` | WP-P24 (skew refusal) | #881 alone (9 pass, 14 fail by design) | unaudited |
| e2e `test:rust` with the paired revision 3 plugin and daemon | `packages/e2e-tests` | WP-P01, WP-P10, WP-P24 | #881 with #883, #883, #884 (23 pass, 17 skip, 0 fail each) | unaudited |

## Names from the sources that do not resolve at `f2442b2f`

Each was searched with `git grep` at `f2442b2f` and at the head of the PR
that cited it.

| Name | Cited by | Finding |
| --- | --- | --- |
| `need_full_sync` check at `daemon/lib.rs:37584-37600` | catalog revision 2 (comment 3), at `265df096` | Deleted with the status by #878; two remaining hits at `f2442b2f` (`crates/daemon/src/lib.rs:25180`, `:25518`) assert the field is absent. Dropped. |
| "reports a destination slot that stopped accepting writes and reads no candidate getter", "replaces every slot and the length of an accepted destination in place" | WP-E06 (catalog revision 2) | Present at `328ab11c` (`transform-capture.test.ts:1000`, `:1015`); deleted by #877 with the all-slot preflight. Kept in WP-E06 as the baseline run of #875. |
| `alternating_sessions_keep_their_delta_prefix_under_the_shared_snapshot_budget`, `cold_soft_plus_over_ready_budget_refuses_the_next_tail_delta`, `giant_degraded_snapshot_refuses_tail_delta_without_a_full_request`, `handler_delta_rechecks_durable_revert_epoch`, `handler_tail_delta_cross_frontier_tool_arc_matches_full_control`, `handler_delta_normalization_matches_full_when_reserved_todo_starts_at_frontier` | #876 | Present at `f8940a26` (for example `lib.rs:26275`, `:25441`, `:27323`); deleted by #878 with the delta channel. Not cited by any record. |
| `native_attachment_cache_refuses_an_entry_above_its_cap_and_stores_one_under_it` | #878 | Present at `f6b3d7bd` (`lib.rs:26118`) and `7a3db43b` (`:26305`); deleted by #880 with the cache. Its store-level successor is `native_output_store_enforces_entry_cap_lru_and_revert_epoch`. |
| `no_revert_prefix_survives_matches_the_full_prefix_scan` | #874 ("equivalence-tested") | Present at `704568ec` (`transform.rs:13365`) and `be542f0c` (`:13180`); no hit at `1c66f16c`: #881 deletes `surviving_revert_prefix_seq` and its test. |
| `translate_input_keeps` | #875 | A production helper, not a test; present at `be542f0c` (`window_coverage.rs:248`); #881 deletes it (0 hits in `crates` at `1c66f16c`). |
| "the null-path barrier test", "the unlisted-run test (4,499 rows)" | #875 | Descriptive references; resolved by content to `a_removal_between_the_core_read_and_the_intersection_is_invisible` and `a_long_run_of_unlisted_rows_does_not_end_the_walk`. |

## Gaps and quiet areas

- A real reset under a retained plugin boundary (WP-FM06): none found;
  #833's D10 reset runs only for a `boundary: null` window.
- The D10 reset's log line (WP-E03, WP-FM02): not asserted; #833 asserts
  `removed_sequence_range` only.
- Refused transform commit leaves no row and no meta (WP-E10): none found
  since `e15a09a6` replaced the 1,400-message refusal pin on `main`.
- Steady-state N sweep at W = 300 and the event measurements (WP-P15): no
  committed check; #833's uncommitted acceptance drivers (gitignored
  `docs/performance/`) measure them and the #833 PR description carries the
  numbers. Bytes decoded per D15 row are recorded since `3ebfc3b9`, and the
  #874 inventory test asserts their bound across H at fixed W.
- Hostile covered slots crossed by a real discovery pass (WP-P13, WP-P23):
  only the primitive-level witness exists at `f2442b2f`; #883 names no
  pass-level variant (needs human input: accept the primitive witness or add
  one).
- Handler-level CK input keep at a nonzero cut (WP-P05): #881's handler test
  builds a native body only.
- Frozen-status assertion for the first-user verdict (WP-P14): #884's tests
  compare tool flags only.
- Corrupt-storage branches for negative stored ordinals and row versions in
  `resolve`: untested; #875 records the gap (only raw-SQL corruption reaches
  them).
- Late PR commits (`efb3fe9c`, `08afec9d`, `cd596c15`, `c239461c`,
  `1431467c`, `2d58cc20`): on `main`, so #881's, #883's, #884's, and #833's
  gates ran them, but no description records their own numbers; #884 ports
  `1431467c`'s decline tests under the window rule.
