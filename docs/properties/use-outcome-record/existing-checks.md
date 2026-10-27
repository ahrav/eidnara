# Existing checks: use-outcome record

Status is per METHOD: `unaudited` means the check exists and runs in the
#1156 PR's gate run but has had no independent adequacy review. Adequacy
belongs to `/testing:invariant-test-review`; production guard adequacy
belongs to `/low-level-systems:defensive-assertions-and-invariant-guards`.
Every location was read at the #1156 branch tip.

## Kernel

| Check | Location | Records | Status |
| --- | --- | --- | --- |
| `visibility_matrix_and_sensitivity_ceiling_are_total` | `crates/kernel/src/admission.rs:4915` | `uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search` | unaudited |
| `an_event_that_names_a_disposition_keeps_its_own_outcome` | `crates/kernel/src/admission.rs:5245` | `uo-disposition-events-append-rows-and-newest-by-commit-seq-wins` | unaudited |
| `unrecognized_stored_classes_read_back_restrictive` | `crates/kernel/src/cas/mod.rs:797` | `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only` | unaudited |
| `a_replayed_expiry_receipt_is_not_counted_as_a_retirement` | `crates/kernel/src/local_file.rs:737` | `uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts` | unaudited |
| `a_capture_pinned_after_it_was_selected_for_expiry_is_kept` | `crates/kernel/src/local_file.rs:760` | `uo-expiry-keeps-pinned-and-cited-evidence` | unaudited |
| `a_record_cited_after_selection_is_kept` | `crates/kernel/src/use_outcome.rs:849` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_page_another_sweep_committed_at_the_same_cutoff_replays` | `crates/kernel/src/use_outcome.rs:877-878` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_record_retired_after_selection_at_another_cutoff_is_skipped` | `crates/kernel/src/use_outcome.rs:940` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_record_kept_at_a_cutoff_retires_at_that_cutoff_once_uncited` | `crates/kernel/src/use_outcome.rs:961` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `the_sweep_and_the_reads_use_their_indexes` | `crates/kernel/src/use_outcome.rs:978` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `both_latest_decision_formulations_choose_the_same_row` | `crates/kernel/tests/kernel_admission.rs:2252` | `uo-disposition-events-append-rows-and-newest-by-commit-seq-wins` | unaudited |
| `dispositions_and_sensitivity_only_reduce_visibility` | `crates/kernel/tests/kernel_admission.rs:2735` | `uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search` | unaudited |
| `every_non_active_disposition_has_the_contracted_surface_row` | `crates/kernel/tests/kernel_admission.rs:2890` | `uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search` | unaudited |
| `malformed_stored_policy_and_sensitivity_fail_closed_without_read_error` | `crates/kernel/tests/kernel_admission.rs:2935` | `uo-admission-reads-require-a-non-null-commit-seq` | unaudited |
| `eligibility_matrix_includes_secret_unknown_and_tombstone` | `crates/kernel/tests/kernel_cas.rs:452` | `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only` | unaudited |
| `egress_facts_carry_the_eligibility_verdict_and_the_stored_class` | `crates/kernel/tests/kernel_cas.rs:545` | `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only` | unaudited |
| `every_conclusive_kernel_mismatch_is_refused_and_left_untouched` | `crates/kernel/tests/kernel_open.rs:183` | `uo-kernel-opens-only-at-the-compiled-baseline-identity` | unaudited |
| `a_family_created_under_the_previous_baseline_is_refused_untouched` | `crates/kernel/tests/kernel_open.rs:582` | `uo-kernel-opens-only-at-the-compiled-baseline-identity`, `uo-p-baseline-rotation-refuses-previous-family-untouched` | unaudited |
| `kernel_schema_has_one_ordered_full_shape` | `crates/kernel/tests/kernel_schema.rs:151` | `uo-p-baseline-rotation-refuses-previous-family-untouched` | unaudited |
| `kernel_schema_object_inventory_is_pinned_by_kind_and_name` | `crates/kernel/tests/kernel_schema.rs:1900` | `uo-kernel-opens-only-at-the-compiled-baseline-identity`, `uo-p-baseline-rotation-refuses-previous-family-untouched` | unaudited |
| `use_outcome_identity_and_served_set_are_never_rewritten` | `crates/kernel/tests/kernel_schema.rs:2177` | `uo-p-served-set-and-identity-never-rewritten`, `uo-p-record-survives-session-delete`, `uo-p-deleted-at-iff-session-delete-kernel-step`, `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `use_outcome_classification_and_identity_shapes_are_checked` | `crates/kernel/tests/kernel_schema.rs:2348` | `uo-p-served-set-and-identity-never-rewritten`, `uo-p-classification-sensitive-local-only-no-remote-egress` | unaudited |
| `an_admission_citation_names_a_recorded_use_outcome_and_never_changes` | `crates/kernel/tests/kernel_schema.rs:2429` | `uo-p-served-set-and-identity-never-rewritten` | unaudited |
| `kernel_schema_digest_is_pinned_to_the_frozen_v1_shape` | `crates/kernel/tests/kernel_schema.rs:599` | `uo-kernel-opens-only-at-the-compiled-baseline-identity`, `uo-p-baseline-rotation-refuses-previous-family-untouched` | unaudited |
| `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` | `crates/kernel/tests/kernel_source_descriptors.rs:1895` | `uo-expiry-keeps-pinned-and-cited-evidence` | unaudited |
| `served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve` | `crates/kernel/tests/kernel_use_outcome.rs:164` | `uo-p-no-row-for-withheld-composition` | unaudited |
| `duplicate_and_malformed_serves_are_refused_without_rows` | `crates/kernel/tests/kernel_use_outcome.rs:215` | `uo-p-served-set-once-per-session-revision`, `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote` | `crates/kernel/tests/kernel_use_outcome.rs:381` | `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only`, `uo-p-classification-sensitive-local-only-no-remote-egress` | unaudited |
| `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage` | `crates/kernel/tests/kernel_use_outcome.rs:400` | `uo-admission-reads-require-a-non-null-commit-seq`, `uo-p-disputed-after-serve-from-committed-events-only`, `uo-p-record-survives-session-delete`, `uo-p-counts-exact-and-unavailable-not-zero` | unaudited |
| `expiry_retires_an_expired_record_under_a_reserved_receipt` | `crates/kernel/tests/kernel_use_outcome.rs:533` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `an_expiry_backlog_drains_one_page_per_call` | `crates/kernel/tests/kernel_use_outcome.rs:595` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `an_expired_record_cited_by_a_live_report_is_retained` | `crates/kernel/tests/kernel_use_outcome.rs:665` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_repeated_serve_intent_replays_and_a_rebound_key_conflicts` | `crates/kernel/tests/kernel_use_outcome.rs:755` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `session_roles_round_trip_and_the_served_set_digest_is_order_free` | `crates/kernel/tests/kernel_use_outcome.rs:776` | `uo-p-served-ids-equal-frozen-composition`, `uo-p-producer-matches-request-subagent-flag` | unaudited |
| `disputed_after_serve_is_judged_against_each_serve` | `crates/kernel/tests/kernel_use_outcome.rs:791` | `uo-p-disputed-after-serve-from-committed-events-only` | unaudited |
| `the_cited_count_covers_the_rows_read_before_the_page_fills` | `crates/kernel/tests/kernel_use_outcome.rs:862` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_session_delete_marks_each_live_record_once` | `crates/kernel/tests/kernel_use_outcome.rs:969` | `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `last_transform_at_advances_live_records_without_a_commit` | `crates/kernel/tests/kernel_use_outcome.rs:1024` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_held_writer_bounds_the_advance_and_an_idle_session_skips_it` | `crates/kernel/tests/kernel_use_outcome.rs:1109` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `an_outside_file_lock_bounds_the_advance_at_begin` | `crates/kernel/tests/kernel_use_outcome.rs:1150` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `the_staleness_check_answers_for_null_or_earlier_values_and_writes_nothing` | `crates/kernel/tests/kernel_use_outcome.rs:1174` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `the_advance_takes_no_pooled_reader` | `crates/kernel/tests/kernel_use_outcome.rs:1206` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |

## Daemon

| Check | Location | Records | Status |
| --- | --- | --- | --- |
| `the_served_set_digest_names_the_rendered_ids_and_not_their_bytes` | `crates/daemon/src/canonical_memory.rs:380` | `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `revision_changes_only_when_the_rendered_inputs_change` | `crates/daemon/src/canonical_memory.rs:418` | `uo-composition-revision-digests-exactly-the-rendered-rows` | unaudited |
| `rows_the_renderer_drops_do_not_move_the_revision` | `crates/daemon/src/canonical_memory.rs:476` | `uo-composition-revision-digests-exactly-the-rendered-rows` | unaudited |
| `bytes_past_the_render_cap_do_not_move_the_revision` | `crates/daemon/src/canonical_memory.rs:493` | `uo-composition-revision-digests-exactly-the-rendered-rows` | unaudited |
| `rows_past_the_memory_budget_are_neither_injected_nor_digested` | `crates/daemon/src/canonical_memory.rs:515` | `uo-composition-revision-digests-exactly-the-rendered-rows`, `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `a_subagent_pass_under_native_authority_reports_native_authority` | `crates/daemon/src/fold_authority_handler_tests.rs:143` | `uo-subagent-pass-serves-no-composition-under-eidnara-folds` | unaudited |
| `route_teardown_keeps_an_admitted_checkpoint_alive` | `crates/daemon/src/lib.rs:25404` | `uo-route-teardown-purges-process-state-but-deletes-no-durable-row` | unaudited |
| `session_delete_waits_for_the_capture_gate_off_the_runtime_worker` | `crates/daemon/src/lib.rs:26454` | `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate` | unaudited |
| `history_summarizer_diagnostics_omit_project_memory_when_memory_is_disabled_or_not_fired` | `crates/daemon/src/lib.rs:39959` | `uo-consent-gate-source-is-the-route-frozen-config` | unaudited |
| `the_acceptance_gate_opens_only_for_an_owner_only_record_of_the_live_pair` | `crates/daemon/src/memory_reviewer/activation.rs:385` | `uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change` | unaudited |
| `the_gate_opens_only_for_a_matching_attested_owner_only_record` | `crates/daemon/src/memory_reviewer/activation.rs:448` | `uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change` | unaudited |
| `the_maintenance_slice_expires_use_outcomes_with_the_activation_gate_closed` | `crates/daemon/src/memory_reviewer/lifecycle.rs:829` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_cited_use_outcome_kept_by_the_sweep_is_not_progress` | `crates/daemon/src/memory_reviewer/lifecycle.rs:859` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `a_cancelled_slice_stops_before_the_use_outcome_sweep` | `crates/daemon/src/memory_reviewer/lifecycle.rs:928` | `uo-p-sweep-retires-only-expired-uncited-rows` | unaudited |
| `session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` | `crates/daemon/src/source_capture_tests.rs:1557` | `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate` | unaudited |
| `session_delete_with_the_kernel_starting_deletes_nothing` | `crates/daemon/src/source_capture_tests.rs:1627` | `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate` | unaudited |
| `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` | `crates/daemon/src/source_capture_tests.rs:1699` | `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate` | unaudited |
| `the_capture_report_reaches_host_status_field_for_field` | `crates/daemon/src/source_capture_tests.rs:845` | `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` | unaudited |
| `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot` | `crates/daemon/src/transform.rs:13886` | `uo-composition-freeze-persists-at-every-hard-fold-site` | unaudited |
| `project_memory_revision_change_requests_a_hard_fold` | `crates/daemon/src/transform.rs:13993` | `uo-served-rows-are-the-frozen-m0-rows-between-hard-folds` | unaudited |
| `a_pressure_refold_reports_its_frozen_composition` | `crates/daemon/src/transform.rs:20895` | `uo-composition-freeze-persists-at-every-hard-fold-site` | unaudited |
| `subagent_first_active_render_commits_tagged_bytes_before_replay` | `crates/daemon/src/transform.rs:23784` | `uo-subagent-pass-serves-no-composition-under-eidnara-folds` | unaudited |
| `an_equal_digest_replays_and_another_harness_or_role_conflicts` | `crates/daemon/src/use_outcome_writer_tests.rs:131` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `only_an_available_composition_is_recorded` | `crates/daemon/src/use_outcome_writer_tests.rs:181` | `uo-consent-gate-source-is-the-route-frozen-config`, `uo-p-no-row-when-memory-disabled`, `uo-p-no-row-for-withheld-composition` | unaudited |
| `a_failed_write_changes_nothing_the_harness_receives_and_a_later_fold_repairs_it` | `crates/daemon/src/use_outcome_writer_tests.rs:282` | `uo-composition-freeze-persists-at-every-hard-fold-site` | unaudited |
| `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one` | `crates/daemon/src/use_outcome_writer_tests.rs:343` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `the_session_role_follows_the_pass_subagent_flag_on_each_engine` | `crates/daemon/src/use_outcome_writer_tests.rs:414` | `uo-subagent-pass-serves-no-composition-under-eidnara-folds`, `uo-p-producer-matches-request-subagent-flag` | unaudited |
| `an_equal_served_set_at_a_later_known_as_of_replays` | `crates/daemon/src/use_outcome_writer_tests.rs:477` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `a_dispute_between_folds_records_the_next_served_set_without_it` | `crates/daemon/src/use_outcome_writer_tests.rs:513` | `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `a_withheld_read_then_an_available_read_of_the_same_rows_records_once` | `crates/daemon/src/use_outcome_writer_tests.rs:549` | `uo-p-no-row-for-withheld-composition` | unaudited |
| `a_wrapup_under_a_config_that_differs_from_the_binding_records_no_serve` | `crates/daemon/src/use_outcome_writer_tests.rs:584` | `uo-consent-gate-source-is-the-route-frozen-config` | unaudited |
| `the_real_failure_paths_count_their_outcomes_and_bound_the_wait` | `crates/daemon/src/use_outcome_writer_tests.rs:748` | `uo-p-served-set-once-per-session-revision`, `uo-p-no-row-for-withheld-composition` | unaudited |
| `a_hard_fold_records_its_frozen_composition_once_per_served_set` | `crates/daemon/src/use_outcome_writer_tests.rs:70` | `uo-p-served-set-once-per-session-revision`, `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `a_recorded_served_set_replays_while_another_writer_holds_the_writer` | `crates/daemon/src/use_outcome_writer_tests.rs:848` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `every_kernel_error_maps_to_a_closed_outcome` | `crates/daemon/src/use_outcome_writer_tests.rs:976` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `an_emergency_pass_that_reruns_records_one_serve` | `crates/daemon/src/use_outcome_writer_tests.rs:995` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `only_an_unready_kernel_leaves_the_pass_to_retry_its_serve` | `crates/daemon/src/use_outcome_writer_tests.rs:1023` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `a_hard_fold_answers_while_another_writer_holds_the_kernel_writer` | `crates/daemon/src/use_outcome_writer_tests.rs:1110` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `a_fold_past_the_writes_in_flight_counts_a_deadline_and_a_later_fold_repairs_it` | `crates/daemon/src/use_outcome_writer_tests.rs:1146` | `uo-p-served-set-once-per-session-revision` | unaudited |
| `a_fold_whose_boundary_read_fails_after_its_commit_records_its_serve` | `crates/daemon/src/use_outcome_writer_tests.rs:1175` | `uo-p-served-ids-equal-frozen-composition` | unaudited |
| `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` | `crates/daemon/tests/harness_sources.rs:3356` | `uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts` | unaudited |
| `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | `crates/daemon/tests/harness_sources.rs:3500` | `uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts`, `uo-expiry-keeps-pinned-and-cited-evidence` | unaudited |
| `a_delete_step_with_an_unknown_outcome_converges_on_retry` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:136` | `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `a_second_delete_marks_records_written_after_the_first` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:177` | `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `a_route_torn_down_after_a_hard_fold_leaves_its_records_live` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:208` | `uo-route-teardown-purges-process-state-but-deletes-no-durable-row`, `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `a_session_id_that_reads_as_a_secret_still_deletes` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:538` | `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `last_transform_at_moves_forward_only_on_committing_passes` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:227` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_held_writer_skips_the_update_and_the_next_committing_pass_catches_up` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:314` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_committing_pass_with_a_current_record_takes_no_writer` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:362` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_spent_pass_budget_skips_the_touch` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:417` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_serve_deadline_leaves_the_touch_its_own_kernel_wait` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:446` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_touch_waiting_on_the_writer_overlaps_the_settle_and_lands_before_the_response` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:483` | `uo-p-last-transform-at-monotone-per-committing-pass` | unaudited |
| `a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:41` | `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate`, `uo-per-session-pass-timestamps-die-with-the-session`, `uo-p-record-survives-session-delete`, `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry` | `crates/daemon/src/use_outcome_lifecycle_tests.rs:86` | `uo-p-record-survives-session-delete`, `uo-p-deleted-at-iff-session-delete-kernel-step` | unaudited |
| `route_teardown_releases_the_staged_upload_while_the_session_stays_bound` | `crates/daemon/tests/kernel_routes.rs:3849` | `uo-route-teardown-purges-process-state-but-deletes-no-durable-row` | unaudited |
| `review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it` | `crates/daemon/tests/memory_reviewer_wire.rs:15` | `uo-review-reads-authorize-by-bearer-and-project-binding-not-owner` | unaudited |
| `an_observational_route_reads_the_same_outcomes_as_an_ordinary_route` | `crates/daemon/tests/memory_reviewer_wire.rs:260` | `uo-review-reads-authorize-by-bearer-and-project-binding-not-owner` | unaudited |
| `a_memory_disabled_pass_takes_no_canonical_read` | `crates/daemon/tests/transform_canonical_memory.rs:320` | `uo-memory-disabled-route-takes-no-canonical-read` | unaudited |
| `transform_composes_project_memory_from_canonical_rows_and_observes_absence` | `crates/daemon/tests/transform_canonical_memory.rs:46` | `uo-served-rows-are-the-frozen-m0-rows-between-hard-folds`, `uo-composition-freeze-persists-at-every-hard-fold-site`, `uo-known-as-of-is-the-tip-bound-read-sequence` | unaudited |

## Memory Store

| Check | Location | Records | Status |
| --- | --- | --- | --- |
| `delete_session_sweeps_every_session_table_once_and_scopes_notes` | `crates/memory-store/src/lib.rs:22344` | `uo-per-session-pass-timestamps-die-with-the-session` | unaudited |

## TypeScript

| Check | Location | Records | Status |
| --- | --- | --- | --- |
| `(TypeScript test at this line)` | `packages/cli/src/commands/review.test.ts:771` | `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` | unaudited |
| `(TypeScript test at this line)` | `packages/cli/src/commands/review.test.ts:822` | `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` | unaudited |

## Categories with no check

- The per-decision count read (#1157): none found.
- Route teardown across both stores: no test asserts that teardown deletes no
  durable row in every table of either store; the #1156 test checks the use-outcome record and the session row.
- A file-backed config reload between bind and a pass or wrapup: no test; the wrapup test uses a live config that differs from the frozen binding.
- A Kernel that is Starting at a pass's read and Ready by its write: no test; the withheld-read tests leave the Kernel unopened for the whole pass.
- A process stop between a fold's Memory Store commit and its Kernel write, with recovery on reopen: no test; the post-commit fault test injects a handled Kernel failure in a live process.

## Suspiciously quiet areas

- The pressure-refold freeze site is checked with an empty Canonical read: the test reloads the persisted composition and compares it with the pinned read; no handler test drives a refold with a populated read.
- The subagent arm under `eidnara_folds` is checked through the wire response:
  `the_session_role_follows_the_pass_subagent_flag_on_each_engine` asserts no
  HARD, no composition, no `<project-memory>` block in the serialized response,
  and no record; no test inspects the applied message array itself.
- The `source_capture` status block carries no state flag, and no client
  decodes it (lead L4).
