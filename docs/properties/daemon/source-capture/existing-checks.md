# Source capture: existing checks

Every claim-bearing check the records cite, grouped by area, each with status
`unaudited`. An existing check never removes a property from the catalog;
adequacy verdicts belong to `/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for production
guards. Names are the test and function names at authoring time.

Abbreviations: `hs.rs` is `crates/daemon/src/harness_sources.rs`; `hs-tests`
is `crates/daemon/tests/harness_sources.rs`; `sc.rs` is
`crates/daemon/src/source_capture.rs`; `sc-tests` is
`crates/daemon/src/source_capture_tests.rs`.

## Harness adapters and publisher

All `unaudited`.

| Check | Covers |
| --- | --- |
| `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` | identity tuples for both adapters; every refusal arm; Pi rows without `type` (witness `pi_wire_row_without_type`) |
| `hs-tests` `pi_refuses_unsupported_message_roles`, `opencode_refuses_unsupported_roles_before_projecting_any_parts` | role gates |
| `hs-tests` `pi_tool_results_require_boolean_error_status` | boolean `isError` |
| `hs-tests` `opencode_refuses_pi_namespace`, `pi_refuses_opencode_namespace` | harness mismatch |
| `hs-tests` `opencode_requires_parts_array` | `parts` shape |
| `hs-tests` `unfinished_assistant_text_is_deferred_but_settled_tools_publish` | deferred assistant text, settled tools publish |
| `hs-tests` `an_interrupted_call_waits_for_its_turn_to_complete` | interrupted tool deferred until turn completes |
| `hs-tests` `publication_receipt_digest_layout_is_pinned` | three pinned `source-request.v2` digests |
| `hs-tests` `publication_receipt_conflicts_on_provider_egress_change` | egress change conflicts, nothing staged |
| `hs-tests` `publication_receipt_distinguishes_delimiters_inside_fields` | length-prefixed digest |
| `hs-tests` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` | replay, `IdentityReused`, `RevisionAhead` at `i64::MAX`, succession |
| `hs-tests` `refused_bytes_and_identities_retain_nothing` | secret refusal; 600-byte identity refusal |
| `hs-tests` `mixed_sessions_create_pending_only_for_message_text` | fixed pairs; descriptor sensitivity equals evidence sensitivity |
| `hs-tests` `repository_documents_publish_untrusted_supersede_and_project` | repository document pair |
| `hs-tests` `capture_batch_names_the_unit_whose_receipt_conflicts` | batch conflict names its unit |
| `hs-tests` `receipts_written_before_the_outcome_record_replay_under_v2` | frozen v1 digest; v1 replay; neither-digest conflict |
| `hs.rs` `pins::revision_lead_matches_the_evaluator_pin` | lead constant equals the evaluator's |
| `crates/daemon/tests/eval_seam_probe.rs` | lead constant equals the evaluator's |
| `eval_ingestion.rs` `generated_observations_never_lead_and_a_boundary_fixture_refuses` | lead boundary: equal accepted, one more refused |
| `eval_ingestion.rs` `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none` | identity fields move ids; payload fields move none |
| `git_sources.rs` `selected_commits_are_retained_exactly_and_rebuild_without_the_repository` | git pair |
| `kernel_admission.rs` `an_inert_citation_does_not_consume_approval_capacity` | `Other` keeps `Candidate` |
| `kernel_admission.rs` `visible_reads_exclude_decisionless_objects_and_apply_maturity_rows` | `Candidate` hidden from AutoInject |
| `kernel_exact_artifacts.rs` `a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content` | exact lane refuses secrets |
| `kernel_exact_artifacts.rs` `oversized_and_non_utf8_payloads_are_refused_before_materialization` | exact lane refuses non-UTF-8 |
| `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` | clamp on the redacting lane; Remote denied |
| `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` | production publication; tool pair and `Sensitive`; replay after memo loss (the `replay_after_memo_loss` marker is recorded in `a_rerun_keeps_one_live_generation_across_a_restart`) |
| `sc-tests` `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` | no publication without consent |
| `sc-tests` `changed_bytes_under_one_identity_are_terminal_and_never_retried` | conflict terminal in memo |
| `sc-tests` `a_concurrent_publication_of_a_unit_replays_or_conflicts` | second writer before the preview (F5, partial) |
| `sc-tests` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | no decision; hidden after reopen |
| `sc-tests` `a_revision_lead_leaves_the_unit_pending` | F25 through capture, retryable and pending |
| `sc-tests` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` | Pi rows without `type` counted `adapter_refused` |
| `crates/eval-core/src/manifest.rs` `Ingestion::AdapterIngestedNoProductionCaller` | label only |

None found: a capture-path test of an unfinished assistant window; a capture-path
test of a secret-bearing unit beside a clean unit; a direct `is_permanent` table
test; a read-back of `event_kind` on a publication's admission row; a receipt
committed between preview and commit.

## Kernel identity, descriptors, retention, and disclosure

| Check | Covers | Status |
| --- | --- | --- |
| `crates/kernel/tests/kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` | `Sensitive` artifact denied `Remote`, allowed `Local` | unaudited |
| `crates/kernel/src/admission.rs` `visibility_matrix_and_sensitivity_ceiling_are_total` | `surface_visibility` ceiling for `Secret` and `Sensitive` | unaudited |
| `crates/kernel/tests/kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility` | `AutoSearch` hides `Sensitive`; `ExplicitSearch` shows it | unaudited |
| `crates/kernel/tests/kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table` | `ProviderSensitive` by destination | unaudited |
| `crates/daemon/tests/kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class` | `OwnerSensitive` refusal, zero provider requests | unaudited |
| `crates/daemon/src/source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | real captures hidden from `AutoInject` after reopen | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` | `retire_evidence` conflict and rollback | unaudited |
| `crates/daemon/tests/harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt` | failed compensating retirement is reported | unaudited |
| `crates/daemon/tests/harness_sources.rs` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | cited evidence kept by the harness sweep | unaudited |
| `crates/daemon/tests/harness_sources.rs` `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` | canonical rows untouched by the harness sweep at `NOW + 32 d` | unaudited |
| `crates/kernel/tests/kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference` | `None` and past deadline refused | unaudited |
| `crates/kernel/tests/kernel_memory_reviewer_holds.rs` `a_committed_capture_replays_after_its_retention_deadline` | replay after deadline (wall clock) | unaudited |
| `crates/kernel/src/local_file.rs` `a_caller_cannot_seat_a_receipt_the_expiry_would_replay` | reserved expiry producer | unaudited |
| `crates/kernel/src/local_file.rs` `a_replayed_expiry_receipt_is_not_counted_as_a_retirement` | replay not counted | unaudited |
| `crates/kernel/src/local_file.rs` `a_capture_pinned_after_it_was_selected_for_expiry_is_kept` | F12 for `curator_capture` | unaudited |
| `crates/kernel/src/local_file.rs` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | F12 for `harness_capture`; class scoping; undated refusal | unaudited |
| `crates/kernel/src/local_file.rs` `a_purge_degraded_pin_does_not_keep_a_lapsed_capture` | degraded pin is not a live pin | unaudited |
| `crates/kernel/src/local_file.rs` `expiry_candidate_cost_does_not_grow_with_retired_history` | candidate query cost bound | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support` | hold, citation, canonical bytes | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `retained_captures_do_not_starve_newer_expired_captures` | page composition with 64 retained | unaudited |
| `crates/daemon/tests/memory_reviewer_project_text.rs` `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep` | non-evidence rows excluded | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `referenced_and_recently_invalidated_artifacts_survive` | live and recently invalidated rows retained | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `invalidated_artifact_reclaims_after_grace_but_backward_clock_keeps_it` | 14-day grace | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `retain_until_is_an_absolute_floor_not_an_extra_grace_start` | `retain_until` floor | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `overflowing_grace_deadline_keeps_artifact` | overflow retains | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `active_capture_pin_and_pin_release_grace_protect_artifact` | pin and release grace | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `maintenance_preserves_only_the_grace_remaining_after_pin_expiry` | expired pin reaped, grace from release | unaudited |
| `crates/kernel/tests/kernel_gc.rs` `pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps` | purge override of live reference | unaudited |
| `crates/kernel/tests/eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` | mirror equality, 24 goldens | unaudited |
| `crates/retrieval/tests/schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums` | CHECK equals `OccurrenceClass::ALL` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals` | unknown class refused first | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt` | one-commit supersession | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `representations_and_spans_of_one_source_are_separate_lineages` | `RevisionNotAdvanced` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `concurrent_successors_serialize_into_one_chain` | racing successors | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `retirement_preserves_domain_ownership_without_reusing_old_revision_ids` | F10 for `messages` | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs` `repository_document_revisions_exceed_the_historical_head_including_retired_rows` | F10 for the guarded class; witness `retired_lineage_republished` | unaudited |
| `crates/daemon/tests/harness_sources.rs` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` | publisher surfaces `RevisionNotAdvanced` | unaudited |
| `crates/kernel/tests/kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent` | purge blocks re-admission | unaudited |
| `crates/kernel/tests/kernel_deletion.rs` `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen` | blocked inside the pending-unlink window | unaudited |
| `crates/kernel/tests/kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone` | tombstone blocks ingest and eligibility | unaudited |
| `crates/kernel/tests/kernel_backup.rs` `a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored` | restore keeps the purge | unaudited |

None found: a GC sweep at a far-future `now` over a live row; a direct
`retire_evidence` on pinned, uncited evidence; re-ingest of identical bytes
after ordinary GC reclaimed an expired capture (F27); a pending unlink with no
tombstone.

## Summarizer, reduction, project documents, snapshot, and session delete

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/src/wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras` | eight `OutputKind` mappings, identity, both extras maps | unaudited |
| `crates/daemon/src/selection.rs::frozen_arc_blocks_never_re_emitted` | a frozen result id suppresses every decision of its arc | unaudited |
| `crates/daemon/src/transform.rs::agent_drop_ids_freeze_add_only_through_flat_ids` | served `[dropped]` bytes after agent drops | unaudited |
| `crates/daemon/src/transform.rs::coverage_extending_soft_prunes_covered_red_units` | served `[dropped]` bytes on soft prune | unaudited |
| `crates/daemon/src/transform.rs::host_reversibility_true_reduction_stays_canonical_after_false_flip` | frozen `[dropped]` payload across a flip | unaudited |
| `crates/daemon/src/transform.rs::owned_profile_drains_pending_drops_without_request_surface` | served `[dropped]` bytes on an owned profile | unaudited |
| `crates/daemon/src/transform.rs::reduced_block_renders_placeholder_without_tag_prefix` | tagged `[dropped §2§]` form | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::tool_output_contains_sentinel` | no output byte outside the excerpts, chunk builder | unaudited |
| `crates/daemon/src/boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls` | same at the boundary site; answered and unanswered calls | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` | F17; error status | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::a_digest_cut_by_the_budget_withdraws_its_alias` | F16 alias withdrawal | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::duplicate_tool_call_ids_resolve_results_by_arc_id` | call summary lookup by arc | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::tool_result_only_message_absorbs_into_preceding_assistant_block` | tool-only absorption | unaudited |
| `crates/daemon/tests/history_summarizer_truncate_differential.rs::optimized_matches_frozen_reference_at_production_windows` | truncation against its reference | unaudited |
| `crates/daemon/src/project_docs.rs::project_docs_golden_matches_reference` | rendered block and canonical hash, six cases | unaudited |
| `crates/daemon/src/project_docs.rs::symlinked_doc_is_skipped` | symlink skipped, other file renders | unaudited |
| `crates/daemon/src/project_docs.rs::a_doc_that_outgrows_the_cap_under_the_read_is_skipped` | sparse oversize; FIFO without blocking | unaudited |
| `crates/daemon/src/project_docs.rs::oversized_doc_is_skipped` | oversize skipped | unaudited |
| `crates/daemon/src/project_docs.rs::a_file_is_missing_only_from_a_directory_that_exists` | `Missing` versus `Unreadable` | unaudited |
| `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | whitespace-only edit keeps `canonical_hash` | unaudited |
| `crates/daemon/src/source_capture_tests.rs::document_capture_leaves_the_response_and_respects_inject_docs` | `inject_docs: false`; response equality | unaudited |
| `crates/daemon/src/lib.rs::ready_snapshot_shares_messages_and_charges_at_least_their_bytes` | `native_messages: None`, `Arc::ptr_eq`, charge | unaudited |
| `crates/daemon/src/lib.rs::ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` | handler path, 64 KiB native payload uncharged | unaudited |
| `crates/daemon/src/lib.rs::TransformSnapshotCache::finish_ready` `debug_assert!` | absence in debug builds only | unaudited |
| `crates/daemon/src/transform/surface_census.rs::auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query` | CK profile parses with no native window | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` | AE5, F23 cut, retry | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_with_the_kernel_starting_deletes_nothing` | OQ12 fail closed | unaudited |
| `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` | queued capture after delete | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` | scope filter, stop between pages | unaudited |
| `crates/daemon/src/source_capture_tests.rs::session_retirement_pages_are_keyed_by_session_and_ids` | page receipt keys, replay | unaudited |
| `crates/daemon/src/lib.rs::a_cancelled_session_delete_still_purges_in_memory_state`, `explicit_capture_checkpoints_serialize_with_session_delete_off_the_worker`, `session_delete_waits_for_the_capture_gate_off_the_runtime_worker` | memory-store delete under `capture_commit_gate` | unaudited |

## Capture supervisor

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/src/source_capture_tests.rs` `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` | consent off, memory off, subagent: no job, tip unchanged | `unaudited` |
| `source_capture_tests.rs` `a_route_claiming_another_harness_captures_nothing` | route harness mismatch admits no job | `unaudited` |
| `crates/daemon/src/config.rs` `hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`, module `source_capture_parity` | key is user-only, default false | `unaudited` |
| `source_capture_tests.rs` `the_response_is_identical_whatever_capture_and_the_kernel_are_doing` | response bytes across Ready, Starting, writer held, slot full; `refused_queue_full` retains nothing (witness `handoff_refused_slot_busy`) | `unaudited` |
| `source_capture_tests.rs` `a_spent_budget_before_the_writer_commits_nothing` | `Deadline` before the writer commits nothing; response bound | `unaudited` |
| `source_capture_tests.rs` `document_capture_leaves_the_response_and_respects_inject_docs` | document capture leaves the response unchanged | `unaudited` |
| `source_capture_tests.rs` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` | Pi response independent of row identity | `unaudited` |
| `crates/daemon/src/transform_unit/host_tests.rs` `the_host_starts_source_capture_after_the_response_settles` | transport starts capture after settlement | `unaudited` |
| `source_capture_tests.rs` `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` | batch unit bound; deferred unit publishes next pass | `unaudited` |
| `source_capture_tests.rs` `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot` | slot and charges released on panic; shutdown drain | `unaudited` |
| `source_capture_tests.rs` `a_window_past_the_remaining_budget_is_refused_resident_short` | budget refusal retains nothing | `unaudited` |
| `crates/daemon/tests/harness_sources.rs` `capture_batch_defers_units_beyond_its_bounds` | 32-unit and 64 KiB batch bounds | `unaudited` |
| `harness_sources.rs` `capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` | 65536 publishes; 65537 refused whole; siblings publish | `unaudited` |
| `harness_sources.rs` `capture_batch_refusals_take_no_admission_slot` | oversized unit takes no batch slot | `unaudited` |
| `source_capture_tests.rs` `unreadable_documents_are_counted_and_retire_nothing` | oversized document counted, nothing retained | `unaudited` |
| `source_capture_tests.rs` `changed_bytes_under_one_identity_are_terminal_and_never_retried` | edited message terminal, never re-offered | `unaudited` |
| `source_capture_tests.rs` `a_concurrent_publication_of_a_unit_replays_or_conflicts` | memo answers for a terminal unit | `unaudited` |
| `harness_sources.rs` `capture_batch_names_the_unit_whose_receipt_conflicts` | conflict names its unit, commits nothing | `unaudited` |
| `crates/daemon/src/harness_sources.rs` `PublishError::is_permanent` | terminal versus retryable classification | `unaudited` |
| `source_capture_tests.rs` `a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass` | pending behind Starting publishes on pass N+1 | `unaudited` |
| `source_capture_tests.rs` `a_revision_lead_leaves_the_unit_pending` | retryable unit re-offered | `unaudited` |
| `source_capture_tests.rs` `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` | invalidated window captured next pass | `unaudited` |
| `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | AE6: no decision, hidden from AutoInject after reopen | `unaudited` |
| `source_capture_tests.rs` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` | AE3 class pair and `Sensitive`; replay after memo loss | `unaudited` |
| `source_capture_tests.rs` `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | document class pair, `Sensitive`, no decision | `unaudited` |
| `.github/workflows/ci.yml` step "Invented provenance" | no provenance construction in capture modules | `unaudited` |
| `crates/kernel/src/admission.rs` `surface_visibility` | surface visibility by class and sensitivity | `unaudited` |
| `source_capture_tests.rs` `the_capture_report_carries_no_unit_text` | report and health carry no sentinel, id, or digest | `unaudited` |
| `crates/daemon/src/source_capture.rs` test `log_lines_carry_only_fixed_codes_and_counts` | every log line is fixed characters | `unaudited` |
| `harness_sources.rs` `impl std::fmt::Debug for SourceUnit` | debug omits text (prints identity) | `unaudited` |

## Harness expiry, outcome record, and digest

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` | class and deadline on message and tool evidence; replay after deadline and after expiry keeps the deadline; `deadline - 1` and `deadline` sweeps; descriptor then evidence in one commit; canonical and repository-document rows untouched | unaudited |
| `crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | live hold blocks expiry; cited capture keeps evidence and leaves the sweep; 65 captures drain in two calls; uncited capture retires later | unaudited |
| `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | undated `harness_capture` ingest refused; pin inside the writer commit refuses `Conflict`; class isolation from the MemoryReviewer sweep | unaudited |
| `crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress` | maintenance slice runs the harness sweep; capture report progress counters | unaudited |
| `crates/kernel/src/slice/write.rs::Envelope::retire_evidence` | production guard: `Conflict` while a live row cites the evidence | unaudited |
| `crates/daemon/tests/harness_sources.rs::opencode_tool_outcomes_follow_the_pinned_trace` | OpenCode outcome mapping over the pinned trace and malformed, absent, non-`bash` edits | unaudited |
| `crates/daemon/tests/harness_sources.rs::an_interrupted_call_waits_for_its_turn_to_complete` | interrupted call published only after turn completion | unaudited |
| `crates/daemon/tests/harness_sources.rs::pi_tool_outcomes_read_status_and_reported_truncation` | Pi outcome mapping | unaudited |
| `crates/daemon/tests/harness_sources.rs::equal_tool_bytes_with_different_outcomes_are_two_descriptors_of_one_artifact` | record binds to the occurrence; one artifact, two descriptors | unaudited |
| `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` | refusal before retention on single and screened paths | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` | Kernel validation, 1024-byte edge, unchanged occurrence tuple | unaudited |
| `crates/kernel/tests/eval_identity.rs::the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` | identity goldens, unchanged by U5 | unaudited |
| `crates/daemon/tests/harness_sources.rs::receipts_written_before_the_outcome_record_replay_under_v2` | v1 receipts replay under v2; mismatches conflict; v1 evidence-only completion; v2 publish and replay | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_digest_shows_status_bytes_and_the_outer_nonblank_lines` | CRLF, single-line, empty, error-with-exit digests | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_digest_clips_on_character_boundaries_within_its_bounds` | multibyte excerpt clipping and 1 KiB call clip | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_wire_output_maps_its_variant_to_the_status` | wire variant to status mapping | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_status_spells_as_the_record_stores_it` | status spelling equals the stored record | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::every_digest_is_bounded_and_shows_only_its_outer_lines` | proptest over bounds, excerpt equality, sentinel absence | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::the_c5_case_digest_shows_completed_and_the_refusal_in_tail` | C5.V1 digest | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` | error digest; forged marker issues no target | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::tool_output_contains_sentinel` | sentinel absence at the chunk builder | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::a_digest_cut_by_the_budget_withdraws_its_alias` | budget cut inside a digest withdraws its alias | unaudited |
| `crates/daemon/src/boundary.rs::digest_tests::the_boundary_chunk_renders_digests_for_answered_calls` | boundary site digest and sentinel absence | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::rendered_aliases_match_the_hand_checked_native_oracles` | digest alias `transformed: true` in the citations golden | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias` | forged marker in native text | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::truncation_withdraws_aliases_the_model_did_not_see_whole` | truncation withdrawal | unaudited |

None found: a test of the deadline on the `publish_screened` batch path; a
test validating a citation into a digest alias; a test of a persistent
per-row refusal in the harness sweep; a test republishing a unit whose
evidence-only row expiry retired.

## Re-runs, deletion, repository documents, and resource bounds

All `unaudited`.

| Check | Covers |
| --- | --- |
| `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` | Pi re-run orderings, coexisting distinct calls, retirement commit sequence |
| `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` | OpenCode re-run through capture with memo loss; superseded counter |
| `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` | in-commit `Conflict` on a newer live generation or a changed predecessor set |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` | narrowed in-transaction read, paging, own retirements |
| `crates/daemon/tests/harness_sources.rs::rerun_lookup_cost_at_ten_thousand_live_tool_descriptors` | ignored OQ11 measurement |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` | AC5, AC15, F23 cut and one retry |
| `crates/daemon/src/source_capture_tests.rs::session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` | scope filter, stop between pages |
| `crates/daemon/src/source_capture_tests.rs::session_delete_with_the_kernel_starting_deletes_nothing` | OQ12 fail closed |
| `crates/daemon/src/source_capture_tests.rs::session_retirement_pages_are_keyed_by_session_and_ids` | page key determinism and replay |
| `crates/daemon/src/source_capture_tests.rs::a_route_claiming_another_harness_captures_nothing` | stamped harness equals the route harness |
| `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` | parked job after a delete commits nothing |
| `crates/daemon/src/source_capture_tests.rs::a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` | epoch bump under the gate; next pass recaptures |
| `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` | F10, expected head, two-producer race |
| `crates/kernel/tests/kernel_source_descriptors.rs::retirement_preserves_domain_ownership_without_reusing_old_revision_ids` | baseline live-predecessor rule for messages |
| `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` | admission pair, revision refusals, projection |
| `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | AC4, AC17, raw bytes, whitespace edit, removal and recreation |
| `crates/daemon/src/source_capture_tests.rs::a_renamed_document_retires_the_old_path_and_publishes_the_new` | rename |
| `crates/daemon/src/source_capture_tests.rs::a_document_is_judged_against_its_own_project_among_many` | per-project live lookup |
| `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` | F18 dispositions; 64 KiB unit cap on documents |
| `crates/daemon/src/source_capture_tests.rs::document_capture_leaves_the_response_and_respects_inject_docs` | response independence; `inject_docs` gate |
| `crates/daemon/src/source_capture_tests.rs::a_documents_text_is_charged_with_its_window` | document bytes charged twice |
| `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` | verified-missing rule |
| `crates/daemon/src/project_docs.rs::tests::symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped`, `oversized_doc_is_skipped`, `project_docs_golden_matches_reference` | read classification and canonical goldens |
| `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` | 64 KiB inclusive cap, whole refusal, terminal |
| `crates/daemon/tests/harness_sources.rs::capture_batch_defers_units_beyond_its_bounds` | 32 units, 64 KiB aggregate |
| `crates/daemon/tests/harness_sources.rs::capture_batch_refusals_take_no_admission_slot` | refusals take no slot |
| `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` | batch bound through capture |
| `crates/daemon/src/source_capture_tests.rs::the_capture_report_carries_no_unit_text` | `refused_window_too_large` |
| `crates/daemon/src/source_capture_tests.rs::a_window_past_the_remaining_budget_is_refused_resident_short` | retained budget refusal and release |
| `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral` | 4 MiB memo bound |
| `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` | outcome above 1 KiB refused before retention |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` | outcome record validation in the commit |
| `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines`, `a_digest_clips_on_character_boundaries_within_its_bounds` | 1 KiB digest, 128-byte excerpts |
| `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` | 64 MiB payload cap |
| `crates/kernel/tests/kernel_cas.rs::oversized_text_fields_are_rejected_before_staging` | 1 KiB text-field cap |

None found: a `raw_tool_spans` unit above 64 KiB through the capture path;
a window holding a large tool output beside small siblings; a
process-restart variant of F23; a gate-hold measurement for large
deletes.

## None found

- A capture-path test that sends a `raw_tool_spans` unit above 64 KiB through
  the transform route; the oversize tests drive `publish_screened` directly
  or use message units.
- A test that commits a receipt between the screened preview and the
  descriptor commit (F5 preview-to-commit window).
- A GC sweep at a far-future `now` over a live `canonical` row (F15).
- A process restart between the Kernel retirement commit and the
  memory-store delete; the F23 cut runs inside one process.
- A configuration reload between a pass and its capture job (F29).
- A secret-bearing unit inside a screened capture batch.
- A test that re-probes a terminal unit after the memo is lost, and a third
  pass after a slot-full refusal.
- A row-level assertion that every captured evidence row has no provenance.
- A test of the document batch's `documents_refused` drop paths and of a
  CAS retry that turns soft after composing.
- A test that ordinary GC reclaim of an expired capture followed by the same
  bytes under a new key admits the bytes (F27).

## Suspiciously quiet areas

- `docs_hash` on `M0Composition`: a comment in
  `crates/daemon/src/history_segment_coverage.rs` says it is persisted, and no
  code reads or persists it.
- `finish_ready` guards the snapshot's native-message absence with a
  `debug_assert!` only, so release builds enforce nothing there.
- The `Debug` impl for `SourceUnit` prints identity values, including a
  document's `relative_path`; no log formats it at authoring time.
- `harness_capture` ingest requires a deadline but checks its liveness at
  reference commit only for `curator_capture`.
- One tool output large enough to exceed the retained-window budget refuses
  its whole window as `refused_window_too_large` on every pass that carries
  it.
- `FrozenAlias.transformed` has no production reader; the citation guarantee
  rests on validation against the presented text.
