# Existing checks: fold authority (M1 and M2)

Status is per METHOD: `unaudited` means the check exists and ran green in the
named recorded PR run but has had no independent adequacy review. Adequacy
belongs to `/testing:invariant-test-review`; production guard adequacy belongs
to `/low-level-systems:defensive-assertions-and-invariant-guards`. The
recorded runs are listed in `catalog.md` under "Evidence boundary". Every
test name below was located with `git grep` at `0ff62b29a`, and `file:line`
is at that tree unless a row names another. A row that names #859 PR A, PR
B, or PR C ran in that slice's recorded run, and every Rust row ran again in
#859 PR C's run at `0ff62b29a`; a TypeScript row that names #904 ran in
#904's `check:repo` and again in #859 PR A's.

## Daemon and store

| Check | Location | Records | Recorded run | Status |
| --- | --- | --- | --- | --- |
| `a_prepared_content_field_that_grows_past_the_durable_bound_on_redaction_is_refused` | `crates/memory-store/src/lib.rs:31877` | FA-E03 (exact boundary, content) | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `state_sync_metadata_scan_failure_rolls_back_earlier_writes` | `crates/memory-store/src/lib.rs:31704` | FA-E03 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `budget_stop_and_tool_only_ranges_are_recorded` | `crates/daemon/src/history_summarizer_chunk.rs:2814` | FA-E04 (range only) | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `history_summarizer_boundary_construction_matches_owned_reference` | `crates/daemon/src/lib.rs:18613` | FA-E04 (missing identity) | #859 PR A (run at `fd0b52aa5`); fingerprint assertions changed by #859 PR C | unaudited |
| `no_fire_reason_is_durable_change_gated_and_cleared_by_fire` | `crates/daemon/src/lib.rs:41167` | FA-E05 (surviving stall clause), FA-E06 (live versus captured chain) | #903 (#855); #859 PR A | unaudited |
| `session_wrapup_no_models_is_terminal_and_retains_command` | `crates/daemon/src/lib.rs:37569` | FA-E05 (wrapup late check) | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `session_status_names_a_stalled_eidnara_summarizer_in_the_authority_prefix` | `crates/daemon/src/fold_authority_handler_tests.rs:1037` | FA-E05 | #904 (#856); #859 PR A | unaudited |
| `defer_delta_and_boundary_advance_are_additive` | `crates/daemon/src/tail_hygiene.rs:2148` | FA-E07 | #906 | unaudited |
| `non_append_mutation_invalidates_until_a_bust` | `crates/daemon/src/tail_hygiene.rs:2210` | FA-E07, FA-N11 | #906 | unaudited |
| `first_divergence_classifies_each_boundary_kind_and_ignores_appends` | `crates/daemon/src/divergence.rs:124` | FA-E08 | #906 | unaudited |
| `publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict` | `crates/memory-store/src/lib.rs:29830` | FA-E09 | #905 | unaudited |
| `selected_range_identity_drift_during_await_rejects_without_cooldown` | `crates/daemon/src/history_summarizer.rs:4047` | FA-E09 | #905 | unaudited |
| `tail_identity_extension_during_await_still_publishes` | `crates/daemon/src/history_summarizer.rs:4101` | FA-E09 | #905 | unaudited |
| `tier_policy_ignores_project_models_and_rejects_project_lowering` | `crates/daemon/src/config.rs:1509` | FA-E10 | #859 PR A | unaudited |
| `compaction_enabled_defaults_true_and_is_user_tier_only` | `crates/daemon/src/config.rs:1616` | FA-E10 | #859 PR A | unaudited |
| `module_model_keys_replace_the_plugin_chain_only_when_the_module_model_is_set` | `crates/daemon/src/config.rs:2237` | FA-E10 | #859 PR A | unaudited |
| `mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change` | `crates/daemon/src/config.rs:2350` | FA-E10, FA-E06 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `an_oversized_tier_file_is_ignored_with_a_warning` | `crates/daemon/src/config.rs:1420` | FA-E10 | #859 PR A | unaudited |
| `unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent` | `crates/daemon/src/config.rs:2390` | FA-E10, FA-N01 | #898 (#852); #859 PR A | unaudited |
| `every_fixture_row_matches_through_the_configuration_cache` | `crates/daemon/src/config.rs:2539` | FA-N01, FA-E10 | #898 (#852); #859 PR A | unaudited |
| `excluded_chain_values_warn_with_their_key` | `crates/daemon/src/config.rs:2546` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `the_pre_parsed_merge_seam_fails_the_fixture` | `crates/daemon/src/config.rs:2573` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `jsonc_only_discovery_fails_the_fixture` | `crates/daemon/src/config.rs:2588` | FA-N01, FA-E10 | #898 (#852); #859 PR A | unaudited |
| `a_rejected_reload_keeps_the_last_admitted_configuration` | `crates/daemon/src/config.rs:2599` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `a_first_load_rejection_keeps_the_tier_outside_the_authority_blocks` | `crates/daemon/src/config.rs:2664` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `an_unresolved_configuration_withdraws_fold_authority_from_the_retained_documents` | `crates/daemon/src/config.rs:2711` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `retained_project_documents_are_bounded_and_the_user_document_survives_churn` | `crates/daemon/src/config.rs:2741` | FA-N01 | #898 (#852); #859 PR A | unaudited |
| `an_unresolved_configuration_adopts_nothing` | `crates/daemon/src/fold_authority_handler_tests.rs:203` | FA-N01, FA-N02 | #903 (#855); #859 PR A | unaudited |
| `the_first_committing_pass_adopts_over_a_state_sync_row` | `crates/daemon/src/fold_authority_handler_tests.rs:135` | FA-N02 | #903 (#855); #859 PR A | unaudited |
| `a_losing_first_adopter_converges_on_the_winner` | `crates/daemon/src/fold_authority_handler_tests.rs:163` | FA-N02 | #903 (#855); #859 PR A | unaudited |
| `legacy_rows_adopt_by_their_fold_artifacts` | `crates/daemon/src/fold_authority_handler_tests.rs:329` | FA-N02 | #903 (#855); #859 PR A | unaudited |
| `a_fold_artifact_written_after_the_plan_replans_the_adoption` | `crates/daemon/src/fold_authority_handler_tests.rs:742` | FA-N02 | #903 (#855); #859 PR A | unaudited |
| `the_transition_table_follows_the_session_authority_rules` | `crates/daemon/src/fold_authority.rs:200` | FA-N02, FA-N03 | #903 (#855); #859 PR A | unaudited |
| `a_non_boolean_fold_authority_is_a_serde_error_on_every_read` | `crates/memory-store/src/lib.rs:29410` | FA-N02 | #903 (#855); #859 PR A | unaudited |
| `bounded_operation_histories_follow_the_authority_model` | `crates/daemon/src/fold_authority_handler_tests.rs:1030` | FA-N02, FA-N03, FA-N04, FA-N06 | #903 (#855); #859 PR A | unaudited |
| `a_quiescent_bind_changes_authority_in_both_directions_through_the_reset` | `crates/daemon/src/fold_authority_handler_tests.rs:219` | FA-N03, FA-N04, FA-N06 | #903 (#855); #859 PR A | unaudited |
| `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind` | `crates/daemon/src/fold_authority_handler_tests.rs:245` | FA-N03, FA-N05, FA-E06 | #903 (#855); #859 PR A | unaudited |
| `a_busy_summarizer_keeps_the_change_pending_across_a_restart` | `crates/daemon/src/fold_authority_handler_tests.rs:289` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `an_ordinary_recomp_reset_preserves_the_adopted_authority` | `crates/daemon/src/fold_authority_handler_tests.rs:314` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `an_emergency_rerun_after_publication_keeps_the_change_pending` | `crates/daemon/src/fold_authority_handler_tests.rs:656` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `a_sibling_bound_during_the_change_keeps_it_pending` | `crates/daemon/src/fold_authority_handler_tests.rs:778` | FA-N03, FA-N05, FA-E06 | #903 (#855); #859 PR A | unaudited |
| `the_authority_reset_writes_its_replacement_and_ordinary_resets_keep_the_authority` | `crates/memory-store/src/lib.rs:29456` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `a_busy_summarizer_refuses_the_authority_reset` | `crates/memory-store/src/lib.rs:29489` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `a_pending_publication_refuses_the_authority_reset_of_an_idle_session` | `crates/memory-store/src/lib.rs:29516` | FA-N03 | #903 (#855); #859 PR A | unaudited |
| `authority_changes_survive_restarts_and_serve_first_passes` | `crates/daemon/src/fold_authority_handler_tests.rs:840` | FA-N04, FA-N06 | #903 (#855); #859 PR A | unaudited |
| `a_descended_target_whose_binding_disagrees_resets_to_a_first_pass` | `crates/daemon/src/fold_authority_handler_tests.rs:591` | FA-N04 | #903 (#855); #859 PR A | unaudited |
| `native_authority_skips_every_fold_step_even_with_a_live_chain` | `crates/daemon/src/fold_authority_handler_tests.rs:46` | FA-N05, FA-N06, FA-E01 (replacement) | #903 (#855); #859 PR A | unaudited |
| `wrapup_is_refused_under_native_authority` | `crates/daemon/src/fold_authority_handler_tests.rs:107` | FA-N05 | #903 (#855); #859 PR A | unaudited |
| `a_resent_descent_after_a_change_to_native_is_acknowledged_as_a_replay` | `crates/daemon/src/fold_authority_handler_tests.rs:555` | FA-N06 | #903 (#855); #859 PR A | unaudited |
| `native_metadata_does_not_grow_with_the_message_count` | `crates/daemon/src/fold_authority_handler_tests.rs:645` | FA-N06, FA-N14, FA-E01 | #903 (#855); #859 PR A | unaudited |
| `a_native_authority_state_sync_keeps_the_session_free_of_fold_coordinates` | `crates/memory-store/src/lib.rs:24992` | FA-N06 | #903 (#855); #859 PR A | unaudited |
| `native_pass_reads_are_bounded_independent_of_stored_rows` | `crates/daemon/src/transform_read_bound.rs:756` | FA-N06 | #903 (#855); #859 PR A | unaudited |
| `a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly` | `crates/daemon/src/transform.rs:14628` | FA-N06 (folding-path counterpart), FA-N12, FA-E09 | #903 (#855); #859 PR A, #905 | unaudited |
| `committing_and_no_write_passes_promote_under_a_charged_lease_and_a_restart_forgets` | `crates/daemon/src/lib.rs:35175` | FA-N09, FA-N11 | #906 | unaudited |
| `a_pass_refused_a_lease_keeps_the_retained_baseline_parts` | `crates/daemon/src/lib.rs:35255` | FA-N09, FA-N11 | #906 | unaudited |
| `a_promotion_without_parts_keeps_the_retained_parts_of_its_epoch_only` | `crates/daemon/src/lib.rs:35288` | FA-N09, FA-N11 | #906 | unaudited |
| `an_emergency_pass_holds_a_derived_lease_only_inside_each_transform` | `crates/daemon/src/lib.rs:35313` | FA-N09 | #906 | unaudited |
| `a_pass_that_loses_every_compare_and_swap_publishes_nothing` | `crates/daemon/src/lib.rs:35399` | FA-N09 | #906 | unaudited |
| `a_committed_pass_whose_boundary_read_fails_still_promotes` | `crates/daemon/src/lib.rs:35437` | FA-N09 | #906 | unaudited |
| `a_sibling_route_waits_for_promotion_and_a_delete_revokes_the_paused_incarnation` | `crates/daemon/src/lib.rs:35501` | FA-N09 | #906 | unaudited |
| `a_session_purged_while_its_pass_awaits_promotion_stays_absent` | `crates/daemon/src/lib.rs:35603` | FA-N09 | #906 | unaudited |
| `a_worker_lost_between_commit_and_promotion_leaves_the_commit_and_no_derived_state` | `crates/daemon/src/lib.rs:35623` | FA-N09 | #906 | unaudited |
| `a_caller_cancelled_after_the_commit_still_promotes_once_the_worker_finishes` | `crates/daemon/src/lib.rs:35651` | FA-N09 | #906 | unaudited |
| `derived_state_keeps_the_newest_acceptance_of_a_live_generation_under_the_shared_budget` | `crates/daemon/src/lib.rs:21722` | FA-N09 | #906 | unaudited |
| `newer_epochs_and_versions_supersede_and_keys_select_by_epoch_and_generation` | `crates/daemon/src/derived_state.rs:122` | FA-N09, FA-N10, FA-N11 | #906 | unaudited |
| `a_no_write_proposal_is_accepted_only_while_its_read_version_is_current` | `crates/daemon/src/derived_state.rs:148` | FA-N09 | #906 | unaudited |
| `a_process_killed_between_commit_and_promotion_keeps_the_commit_and_forgets_the_derived_state` | `crates/daemon/tests/derived_state_crash_cut.rs:76` | FA-N09, FA-N11 | #906 | unaudited |
| `a_new_hint_defers_only_when_its_target_may_have_been_served_outside_a_bust` | `crates/daemon/src/transform.rs:13329` | FA-N10, FA-E08 | #906 | unaudited |
| `divergence_reports_nothing_against_an_unknown_served_history` | `crates/daemon/src/transform.rs:13384` | FA-N10 | #906 | unaudited |
| `a_new_hint_defers_after_a_restart_forgets_what_was_served` | `crates/daemon/tests/eval_surface_ledger.rs:442` | FA-N10 | #906 | unaudited |
| `a_new_hint_is_skipped_while_every_deferral_slot_is_taken` | `crates/daemon/tests/eval_surface_ledger.rs:533` | FA-N10, FA-N14 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `status_reads_hygiene_validity_joined_with_the_retained_parts` | `crates/daemon/src/lib.rs:35724` | FA-N11 | #906 | unaudited |
| `durable_scalars_join_retained_parts_by_generation` | `crates/daemon/src/tail_hygiene.rs:2278` | FA-N11, FA-E07 | #906 | unaudited |
| `derived_output_state_is_absent_from_serialized_meta_and_legacy_rows_still_load` | `crates/memory-store/src/lib.rs:29595` | FA-N11 | #906 | unaudited |
| `requested_identity_reads_do_not_grow_with_the_identity_table` | `crates/memory-store/src/lib.rs:23853` | FA-N12 | #905 | unaudited |
| `block_identity_deltas_keep_omitted_rows_and_scan_only_the_rows_they_write` | `crates/memory-store/src/lib.rs:33283` | FA-N12 | #905 | unaudited |
| `a_rejected_commit_writes_no_identity_row` | `crates/memory-store/src/lib.rs:33369` | FA-N12 | #905 | unaudited |
| `a_failure_after_each_identity_mutation_rolls_the_whole_commit_back` | `crates/memory-store/src/lib.rs:33401` | FA-N12 | #905 | unaudited |
| `a_refused_commit_writes_no_identity_row` | `crates/memory-store/src/lib.rs:33555` | FA-N12, FA-E03 | #905 (#857), #859 PR A | unaudited |
| `a_delta_that_writes_and_deletes_one_mid_is_refused` | `crates/memory-store/src/lib.rs:33595` | FA-N12 | #905 | unaudited |
| `receipt_retirement_reads_at_most_one_row_beyond_the_released_ones` | `crates/memory-store/src/lib.rs:33624` | FA-N12 | #905 | unaudited |
| `descent_copies_identity_rows_without_their_scan_owner` | `crates/memory-store/src/lib.rs:33723` | FA-N12 | #905 | unaudited |
| `identity_histories_match_a_per_session_reference_map` | `crates/memory-store/src/lib.rs:33775` | FA-N12 | #905 | unaudited |
| `descent_copies_block_identities_and_recomp_reset_clears_them` | `crates/memory-store/src/lib.rs:33872` | FA-N12 | #905 | unaudited |
| `transform_snapshot_resists_commit_between_state_and_overlay_reads` | `crates/memory-store/src/lib.rs:21840` | FA-N12 | #905 | unaudited |
| `publish_rejects_a_selected_message_whose_identity_row_is_gone` | `crates/memory-store/src/lib.rs:25736` | FA-N12, FA-E09 | #905 | unaudited |
| `mid_turn_tail_stays_provisional_and_re_adopts_completed_tail` | `crates/daemon/src/transform.rs:14673` | FA-N12 | #905 | unaudited |
| `a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows` | `crates/daemon/src/transform.rs:20444` | FA-N12, FA-E09 | #905 (renamed from `a_window_that_omits_the_selected_message_keeps_its_identity_for_the_publication` by #905's review commits) | unaudited |
| `publish_rejects_a_firing_whose_selected_message_left_the_window` | `crates/memory-store/src/lib.rs:25791` | FA-E09 | #905 | unaudited |
| `an_over_budget_selection_stops_at_the_longest_prefix_of_whole_blocks` | `crates/daemon/src/history_summarizer_chunk.rs:1960` | FA-N13, FA-E04 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `an_indivisible_first_block_over_the_identity_budget_no_fires` | `crates/daemon/src/history_summarizer_chunk.rs:1981` | FA-N13, FA-E04 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `escaped_and_unicode_mids_are_charged_as_they_serialize` | `crates/daemon/src/history_summarizer_chunk.rs:1995` | FA-N13, FA-E04 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `an_indivisible_block_over_the_identity_budget_no_fires_and_reserves_nothing` | `crates/daemon/src/lib.rs:36285` | FA-N13 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `module_meta_size_is_independent_of_message_count_and_window_size` | `crates/daemon/src/transform_meta_bound.rs:1273` | FA-N14, FA-E03 (guard not reached) | #859 PR A (run at `fd0b52aa5`); escaping negative control added by #859 PR C | unaudited |
| `every_metadata_field_has_a_recorded_bound_within_the_headroom` | `crates/daemon/src/transform_meta_bound.rs:732` | FA-N14 | #859 PR A; `Unbounded` entries removed by #859 PR B; summarizer inventory and selection sum by #859 PR C, the table built with `inventory!` by `6fc85742f`, and `last_recut`, `pending_rewrite`, and the table's reuse as `recorded_metadata_bounds` by `186f3c076` and `e02b22383`, and the five-input `last_render_config` bound by `78fc312db` (run at `0ff62b29a`) | unaudited |
| `request_identity_strings_over_their_bound_are_refused_before_any_read` | `crates/daemon/src/transform_meta_bound.rs:1371` | FA-N14 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `a_hundred_thousand_message_session_commits_a_three_hundred_message_window` | `crates/daemon/src/transform_meta_bound.rs:91` | FA-N14 | #859 PR A (run at `fd0b52aa5`); earlier #833 | unaudited |
| `state_sync_refuses_anchors_and_watermarks_over_their_bounds` | `crates/memory-store/src/lib.rs:24334` | FA-N14 | #859 PR A (run at `fd0b52aa5`); todo serialized-length cap added by #859 PR C | unaudited |
| `empty_and_reserved_message_ids_are_rejected` (mid length at `:986-996`) | `crates/daemon/src/wire.rs:975` | FA-N14 | #859 PR A (run at `fd0b52aa5`) | unaudited |
| `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix` | `crates/daemon/src/lib.rs:36313` | FA-N13, FA-E04 (range) | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget` (64-case proptest) | `crates/daemon/src/history_summarizer_chunk.rs:2169` | FA-N13, FA-E04 (mixed system and noise roles) | #859 PR C (run at `0ff62b29a`) | unaudited |
| `covered_systems_grow_in_m0_while_the_stored_meta_stays_fixed` | `crates/daemon/src/transform.rs:26366` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `covered_system_rows_round_trip_in_ordinal_order_and_retire_their_receipts` | `crates/memory-store/src/lib.rs:33972` | FA-N14, FA-E03 (lost CAS and write-and-delete refusal) | #859 PR B (run at `b45416ac0`) | unaudited |
| `covered_system_rows_past_one_scan_document_split_and_retire_every_receipt` | `crates/memory-store/src/lib.rs:34077` | FA-E03 (rows past one scan document), FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `covered_system_content_is_stored_as_the_meta_scan_redacts_it` | `crates/memory-store/src/lib.rs:34147` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `reset_and_delete_remove_covered_system_rows_and_their_receipts` | `crates/memory-store/src/lib.rs:34183` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `descent_leaves_the_target_without_covered_system_rows` | `crates/memory-store/src/lib.rs:34213` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `state_sync_refuses_values_whose_redacted_form_passes_their_bound` | `crates/memory-store/src/lib.rs:24576` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `state_sync_refuses_a_result_over_the_legacy_segment_cap` | `crates/memory-store/src/lib.rs:24947` | FA-N14 | #859 PR B (run at `b45416ac0`) | unaudited |
| `every_history_summarizer_field_has_an_enforced_bound` | `crates/daemon/src/transform_meta_bound.rs:588` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `an_abandon_keeps_a_failure_detail_within_its_serialized_bound` | `crates/memory-store/src/lib.rs:25617` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_redacted_detail_cut_keeps_its_length_through_another_redaction` | `crates/memory-store/src/lib.rs:24666` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `an_abandon_keeps_a_secret_bearing_detail_within_its_bound_after_redaction` | `crates/memory-store/src/lib.rs:24854` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `set_todo_state_refuses_a_state_whose_redacted_form_passes_its_bound` | `crates/memory-store/src/lib.rs:24894` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_secret_bearing_start_failure_stays_within_its_bound_once_stored` | `crates/daemon/src/history_summarizer.rs:4267` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_captured_state_whose_redacted_form_passes_its_bound_reads_as_an_empty_list` | `crates/daemon/src/injection.rs:773` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_synthetic_pair_over_its_bound_after_redaction_is_refused` | `crates/daemon/src/injection.rs:793` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total` | `crates/daemon/src/transform_meta_bound.rs:978` | FA-N14 (composite commit; synthetic stress state) | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance` | `crates/daemon/src/transform_meta_bound.rs:1342` | FA-N14 (render identity bound) | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_secret_bearing_summarizer_detail_is_stored_within_its_bound_with_its_detection_recorded` | `crates/memory-store/tests/production_redaction.rs:968` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_committed_last_recut_is_stored_within_its_bound` | `crates/memory-store/src/lib.rs:24699` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_revert_keeps_last_recut_within_its_bound_when_a_surviving_id_is_long` | `crates/memory-store/src/lib.rs:24719` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `state_sync_refuses_values_whose_stored_form_passes_their_bound` | `crates/memory-store/src/lib.rs:24759` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_replacement_pair_over_its_bound_after_redaction_clears_the_persisted_pair` | `crates/daemon/src/transform.rs:21623` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_replacement_pair_near_its_bound_persists_and_reloads_within_it` | `crates/daemon/src/transform.rs:21655` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_reservation_identity_over_its_serialized_bound_is_refused_before_any_write` | `crates/memory-store/src/lib.rs:26514` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_producer_start_failure_records_a_bounded_detail` | `crates/daemon/src/history_summarizer.rs:4235` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_run_id_over_the_producer_identity_bound_is_a_start_failure` | `crates/daemon/src/history_summarizer.rs:4300` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_harness_over_the_producer_identity_bound_writes_nothing` | `crates/daemon/src/history_summarizer.rs:4331` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_producer_session_id_keeps_a_bounded_slug` | `crates/daemon/src/history_summarizer.rs:6271` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `chunk_fingerprint_uses_id_kind_and_byte_length` (now a SHA-256 digest of the join) | `crates/daemon/src/history_summarizer.rs:6232` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `chunk_failures_count_per_chunk_and_ignore_provider_errors` (model-chain digest, older chain form, 1,000-model chain) | `crates/daemon/src/history_summarizer.rs:2714` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |
| `a_recorded_failure_detail_is_cut_at_a_character_boundary_within_its_raw_bound` | `crates/daemon/src/history_summarizer.rs:3175` | FA-N14 | #859 PR A; #859 PR C | unaudited |
| `a_credential_that_crosses_the_detail_bound_is_redacted_whole` | `crates/daemon/src/history_summarizer.rs:4351` | FA-N14 | #859 PR C | unaudited |
| `a_cut_that_exposes_a_finding_backs_off_in_a_bounded_number_of_scans` | `crates/memory-store/src/lib.rs:24694` | FA-N14 | #859 PR C | unaudited |
| `todo_state_bounds_hold_for_the_raw_and_the_redacted_form` | `crates/memory-store/src/lib.rs:24715` | FA-N14 | #859 PR C | unaudited |
| `a_captured_state_over_its_raw_bound_reads_as_an_empty_list_whatever_its_redacted_form` | `crates/daemon/src/injection.rs:805` | FA-N14 | #859 PR C | unaudited |
| `a_state_whose_pair_is_refused_leaves_no_injection_pending` | `crates/daemon/src/injection.rs:840` | FA-N14 | #859 PR C | unaudited |
| `a_detail_is_cut_by_its_serialized_length` | `crates/memory-store/src/summarizer_timeline.rs:490` | FA-N14 | #859 PR C (run at `0ff62b29a`) | unaudited |

## Plugin, Pi, and CLI

| Check | Location | Records | Recorded run | Status |
| --- | --- | --- | --- | --- |
| "isCompactionEnabled requires a summarizer chain and a compaction setting that is not false" | `packages/opencode-plugin/src/config/compaction-accessor-guard.test.ts:67` | FA-E02 (replacing clause) | #898 (#852); #904 (#856) last `check:repo` | unaudited |
| "compaction-off tool set = mode-on tool set minus exactly the reduce factory's IDs, with the other tools' fields intact" | `packages/opencode-plugin/src/plugin/tool-registry.test.ts:127` | FA-E02 (surviving clause) | #904 (#856) `check:repo` | unaudited |
| "an empty summarizer chain registers the compaction-off tool set" | `packages/opencode-plugin/src/plugin/tool-registry.test.ts:154` | FA-E02 | #898 (#852); #904 (#856) `check:repo` | unaudited |
| "refuses /${command} without a daemon call when compaction is off" | `packages/opencode-plugin/src/hooks/context/command-handler.test.ts:144` | FA-E02 (surviving clause) | #904 (#856) `check:repo` | unaudited |
| "resolves to an existing eidnara.json when no eidnara.jsonc exists" | `packages/opencode-plugin/src/config/config-paths.test.ts:66` | FA-E11 | #904 (#856) | unaudited |
| "keeps {env:} and {file:} expansion enabled for user config" | `packages/opencode-plugin/src/config/index.test.ts:689` | FA-E11 | #904 (#856) | unaudited |
| "leaves {env:} and {file:} tokens literal in project config and warns" | `packages/opencode-plugin/src/config/index.test.ts:715` | FA-E11 | #904 (#856) | unaudited |
| "keeps history_summarizer model selection user-owned when project config tries to override it" | `packages/opencode-plugin/src/config/index.test.ts:760` | FA-E11 | #904 (#856) | unaudited |
| "derives every mode from the shared config through the plugin loader" | `packages/cli/src/lib/eidnara-modes.test.ts:24` | FA-E12, FA-N01 | #898 (#852); #904 (#856) | unaudited |
| "forwards the summarizer chain after substitution and reference exclusion" | `packages/cli/src/lib/eidnara-modes.test.ts:53` | FA-E12, FA-N01 | #898 (#852); #904 (#856) | unaudited |
| "reports a rejected user tier as unresolved" | `packages/cli/src/lib/eidnara-modes.test.ts:68` | FA-E12, FA-N01 | #898 (#852); #904 (#856) | unaudited |
| "resolves the compaction mode setup produces once it writes the summarizer model" | `packages/cli/src/lib/eidnara-modes.test.ts:121` | FA-E12 | #904 (#856) | unaudited |
| "leaves compaction untouched when compactionEnabled=false and turns it off once mode is on" | `packages/cli/src/commands/setup-opencode.test.ts:670` | FA-E12 | #904 (#856) | unaudited |
| "does not create a compaction block when compactionEnabled=false and none exists" | `packages/cli/src/commands/setup-opencode.test.ts:689` | FA-E12 | #904 (#856) | unaudited |
| "OPENCODE_DISABLE_AUTOCOMPACT does not override the resolved arm (host already applied it)" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:743` | FA-E13 | #904 (#856) | unaudited |
| "OPENCODE_DISABLE_AUTOCOMPACT host semantics (file-based arm)" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:1043` | FA-E13 | #904 (#856) | unaudited |
| "leaves the user config untouched when the project layer set auto=true" | `packages/opencode-plugin/src/shared/conflict-fixer.test.ts:290` | FA-E13 | #904 (#856) | unaudited |
| "does NOT flip compaction.auto to false when compaction-off" | `packages/opencode-plugin/src/shared/conflict-fixer.test.ts:706` | FA-E13 | #904 (#856) | unaudited |
| "a stalled summarizer and an unknown daemon path render without a conflict" | `packages/opencode-plugin/src/plugin/rpc-handlers.test.ts:246` | FA-E14, FA-E05 (stall rendering) | #904 (#856) `check:repo` | unaudited |
| "every row matches through the production loader" | `packages/opencode-plugin/src/config/index.test.ts:1114` | FA-N01, FA-E11 | #898 (#852); #904 (#856) | unaudited |
| "withdraws fold authority and says so when the configuration is unresolved" | `packages/opencode-plugin/src/config/index.test.ts:1124` | FA-N01 | #898 (#852); #904 (#856) | unaudited |
| "skips substitution of an excluded chain value, so a FIFO target never blocks" | `packages/opencode-plugin/src/config/index.test.ts:1143` | FA-N01, FA-E11 | #898 (#852); #904 (#856) | unaudited |
| "warns for every excluded chain value with its key" | `packages/opencode-plugin/src/config/index.test.ts:1158` | FA-N01 | #898 (#852); #904 (#856) | unaudited |
| "rejects a raw lenient read that skips the loader pipeline" | `packages/opencode-plugin/src/config/index.test.ts:1179` | FA-N01, FA-E12 | #898 (#852); #904 (#856) | unaudited |
| "every row matches through the Pi loader" | `packages/pi-plugin/src/config/index.test.ts:765` | FA-N01, FA-E11 | #898 (#852); #904 (#856) | unaudited |
| `it.each` mode-by-host rows of "compaction-off mode matrix (issue #266)" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:502` | FA-N07, FA-E13 | #899 (#853); #904 (#856) | unaudited |
| `it.each` "native folds + auto=false + %s → disable, keeping the auto=true patch" (DCP, OMO) | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:558` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "reports an environment-forced auto=false under native folds as unresolved by its source" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:578` | FA-N07, FA-E13 | #899 (#853); #904 (#856) | unaudited |
| "reports an inline OPENCODE_CONFIG_CONTENT auto=false under native folds as unresolved by its source" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:589` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "reports a host-resolved auto=false the config files do not set as unresolved" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:606` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "formats a warning without claiming Eidnara is disabled" | `packages/opencode-plugin/src/shared/conflict-detector.test.ts:625` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "turns auto back on in the winning layer, leaves prune, and re-detects clean" | `packages/opencode-plugin/src/shared/conflict-fixer.test.ts:659` | FA-N07, FA-E13 | #899 (#853); #904 (#856) | unaudited |
| "writes through a symlink to the shared file" | `packages/opencode-plugin/src/shared/conflict-fixer.test.ts:679` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "edits no file for an environment-forced auto=false" | `packages/opencode-plugin/src/shared/conflict-fixer.test.ts:689` | FA-N07, FA-E13 | #899 (#853); #904 (#856) | unaudited |
| "a configuration with no fold authority boots with a warning and keeps the RPC server" | `packages/opencode-plugin/src/index.entry.test.ts:203` | FA-N07, FA-E13 | #899 (#853); #904 (#856) | unaudited |
| "the snapshot carries the host's compaction ownership only while Eidnara compaction is off" | `packages/opencode-plugin/src/plugin/rpc-handlers.test.ts:582` | FA-N07, FA-E14 | #899 (#853); #904 (#856) `check:repo` | unaudited |
| "a pending authority reaches both RPCs and raises a warn conflict while they keep answering" | `packages/opencode-plugin/src/plugin/rpc-handlers.test.ts:179` | FA-N07, FA-E14 (added beside it) | #904 (#856) `check:repo` | unaudited |
| "names the owner from compaction.auto alone and marks an absent observation unknown" | `packages/opencode-plugin/src/tui/compaction-off.test.ts:75` | FA-N07, FA-E14 | #899 (#853); #904 (#856) `check:repo` | unaudited |
| "publishes a warn conflict that formats under the warning header, and a clear state" | `packages/opencode-plugin/src/shared/fold-authority-status.test.ts:282` | FA-N07 | #904 (#856; renamed on `main` when warnings became published states) | unaudited |
| "replaces a warning the other disposition left instead of keeping both" | `packages/opencode-plugin/src/plugin/conflict-warning-hook.test.ts:198` | FA-N07 | #899 (#853); #904 (#856) | unaudited |
| "a fresh setup without a summarizer turns OpenCode's compaction on and says why" | `packages/cli/src/commands/setup-opencode-authority.test.ts:136` | FA-N08, FA-E12 | #900 (#854); #904 (#856) | unaudited |
| "a dry run prints the same native proposal and writes nothing" | `packages/cli/src/commands/setup-opencode-authority.test.ts:151` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a setup that picks a model turns OpenCode's compaction off as before" | `packages/cli/src/commands/setup-opencode-authority.test.ts:165` | FA-N08, FA-E12 | #900 (#854); #904 (#856) | unaudited |
| "repairs an existing native-folds host for the summarizer it is about to write" | `packages/cli/src/commands/setup-opencode-authority.test.ts:176` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "keeps prune as found and re-enables auto under native folds" | `packages/cli/src/commands/setup-opencode-authority.test.ts:221` | FA-N08, FA-E12 | #900 (#854); #904 (#856) | unaudited |
| "keep and remove derive the authority from the resulting document" | `packages/cli/src/commands/setup-opencode-authority.test.ts:230` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "an edit made while a prompt is open stops setup before any write" | `packages/cli/src/commands/setup-opencode-authority.test.ts:262` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a project tier that stops the plugin blocks every host edit" | `packages/cli/src/commands/setup-opencode-authority.test.ts:323` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "declining the OpenCode edit leaves the authority and the host settings as they are" | `packages/cli/src/commands/setup-opencode-authority.test.ts:337` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a declined repair under Eidnara folds leaves OpenCode's compaction on and reports it" | `packages/cli/src/commands/setup-opencode-authority.test.ts:347` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "reports an environment-forced auto=false as unresolved by its source" | `packages/cli/src/commands/setup-opencode-authority.test.ts:357` | FA-N08, FA-N07 | #900 (#854); #904 (#856) | unaudited |
| "an unresolved proposed document blocks every host edit" | `packages/cli/src/commands/setup-opencode-authority.test.ts:369` | FA-N08, FA-N01 | #900 (#854); #904 (#856) | unaudited |
| "a process killed while removing the summarizer leaves a mismatch detection reports" | `packages/cli/src/commands/setup-opencode-authority.test.ts:407` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a process killed while adding a summarizer leaves OpenCode's compaction folding" | `packages/cli/src/commands/setup-opencode-authority.test.ts:424` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a rolled-back rerun whose files already match the proposal is not reported as written" | `packages/cli/src/commands/setup-opencode-authority.test.ts:438` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "a failure between the two writes reports the read-back of both files" | `packages/cli/src/commands/setup-opencode-authority.test.ts:465` | FA-N08 | #900 (#854); #904 (#856) | unaudited |
| "reports the fold authority and repairs a native-folds auto=false without a registered plugin" | `packages/cli/src/commands/doctor-opencode.test.ts:359` | FA-N08, FA-N07, FA-E13 | #900 (#854); #904 (#856) | unaudited |
| "turns auto back on under native folds even when a DCP conflict blocks the other repairs" | `packages/cli/src/commands/doctor-opencode.test.ts:398` | FA-N08, FA-E13 | #900 (#854); #904 (#856) | unaudited |
| "treats a configuration that does not load as unresolved and leaves host settings alone" | `packages/cli/src/commands/doctor-opencode.test.ts:441` | FA-N08 | #900 (#854); #904 (#856) | unaudited |

## Categories with no checks

- No campaign runtime exists; every `sometimes` record is witnessed by a
  named deterministic test (see `fault-map.md`).
- No test kills the daemon between an authority reset commit and the same
  pass's core commit (FA-N04).
- No test starts the TUI under a `warn` disposition (FA-N07).

## Suspiciously quiet areas

- No native-authority pass runs at or above 95 percent of the context
  limit (FA-N05). The emergency rerun caller is not reachable under native
  authority: it follows only a completed `Busy` wait, which native
  preparation never returns (`crates/daemon/src/lib.rs:8957-8974`,
  `:9428-9432`), so the shared gate covers it structurally.
- The FA-E04 mixed-range witness is a proptest that does not count the
  mixed ranges it generates; the fixed budget-edge fixtures are synthetic
  stress states (FA-E04, FA-N13).
- The composite metadata witness commits a directly constructed record, a
  synthetic stress state; no pass produces every field at its bound at once
  (FA-N14).
- No transform-level test refuses a first commit over the durable-text
  guard and checks that the row stays absent, and no durable-text refusal
  checks the covered-system rows (FA-E03, shared with WP-E10).
