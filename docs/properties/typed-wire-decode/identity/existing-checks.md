# Existing identity checks

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
System and external leads: [catalog.md](catalog.md#scope-and-provenance).

Every row is **unaudited**. The inventory describes assertions present in
source, not successful executions or adequacy verdicts. A test's name is not
proof of its claim. No tests or benchmarks ran. Legacy checks that pin removed
originals or unknown-envelope replay are retained as contract conflicts.

## Survey and inclusion boundary

The inspected source sizes at HEAD are: memory-store `lib.rs` 26,806 lines;
daemon `transform.rs` 29,195; `wire.rs` 1,863; `served_json.rs` 253;
`codec/sidecar.rs` 558; `history_summarizer.rs` 4,387; `history_summarizer_chunk.rs` 1,868;
`injection.rs` 838; plugin `module-wire.ts` 1,127 and its tests 1,494.
Large files contain unrelated subsystems. Counts are physical source lines,
not a complexity or coverage score.

Scope includes claim-bearing checks for canonical CK serialization, projection
identity, receipt matching, sibling/native metadata isolation, synthetic
classification, raw-history recovery, and cache/persistence identity seams.
Search uses `colgrep` first, then exact HEAD source scans where its index has no
code units. For dirty daemon `lib.rs`, every listed line comes from
`git show HEAD:crates/daemon/src/lib.rs`, not worktree offsets.

Each section names the complete source path; its Line column resolves within
that file. Each test row inventories the assertion set of one named test.
Guards are separately listed by exact sites and failure behavior. The tables
are the scoped check inventory; unrelated tests elsewhere in these large
modules are not claimed as identity coverage.

The tables identify 251 tests: the initial 178 cases plus 73 targeted hygiene,
lineage, and numeric-consumer checks from the completed independent findings.
Source-only assertion density
in the primitive prefixes is low: `wire.rs:1-914` contains two debug assertion
macros and two `expect` calls; `served_json.rs:1-166` contains no assertion
macros and eight `expect` calls, one in its test-support wrapper;
`codec/sidecar.rs:1-480` contains neither. These lexical counts include the
specified prefixes only. They do not count serde validation, error returns,
or prove runtime enforcement strength; those surfaces are inventoried below.

## Canonical serialization

Source: `crates/daemon/src/served_json.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 171 | `canonical_encoding_visits_each_serialize_implementation_once` | Counts serialization visits and pins literal bytes. | unaudited |
| 196 | `canonical_encoding_preserves_nested_scalars_and_decoded_key_order` | Compares nested scalar bytes with value-round-trip reference. | unaudited |
| 218 | `canonical_encoding_orders_prefix_and_escaped_keys_like_decoded_strings` | Pins prefix and escaped-key ordering. | unaudited |

Source: `crates/daemon/src/transform.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 13605 | `served_fingerprint_block_ids_pin_flat_mid_index_format` | Pins served block IDs including synthetic IDs. | unaudited |
| 13638 | `served_fingerprint_records_cold_start_and_attributes_middle_changes` | Checks persisted served fingerprints and divergence attribution. | unaudited |
| 13714 | `served_canonical_shell_bytes_and_segments_are_frozen` | Literal original/latent/typed/edited bytes, hash, output identity, segment length. | unaudited |
| 13760 | `served_canonical_frozen_corpus_matches_value_reference_for_both_shells` | Value-reference bytes and finite equality/digest matrix. | unaudited |
| 13797 | `served_fingerprint_fallback_preserves_complete_identity_and_first_match` | Original-sensitive equality, first match, positional precedence, signed-zero reuse. | unaudited |
| 13942 | `served_serialization_has_no_value_round_trip_and_fallback_reuses_receipt_helper` | Source-shape guard and receipt helper use; not replacement execution proof. | unaudited |
| 13993 | `parked_p2_fingerprint_reuse_and_tag_frontier_match_baseline` | Compares parked receipt/tag frontier behavior with baseline. | unaudited |
| 14452 | `wire_golden_bytes_match_cross_repo_pin` | SHA-256 pin; message says update only for deliberate fixture re-vendor. | unaudited |
| 14467 | `wire_golden_projects_to_flat_blocks` | Compares full serialized flat records to projection golden. | unaudited |
| 15140 | `unreduced_golden_messages_are_passed_through_by_identity` | Checks unreduced frozen messages retain identity. | unaudited |
| 15160 | `pure_passthrough_defer_round_trips_tail_byte_identical` | Pins deferred pass-through tail bytes. | unaudited |
| 16609 | `v2_defer_replays_messages_byte_identically_and_echoes_fingerprint` | Pins replay bytes and echoed request fingerprint. | unaudited |
| 16744 | `adapter_reasoning_goldens_reach_the_module_strip_contract` | Carries reasoning golden forms through the strip contract. | unaudited |
| 21567 | `tagging_gate_off_preserves_unreduced_golden_messages` | Checks gate-disabled unreduced output. | unaudited |

Source: `crates/daemon/src/differential_goldens.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 41 | `dg_goldens_match_ts_wire_surface_and_gate_labels` | Three-family wire values plus provenance and gate-label assertions. | unaudited |
| 72 | `dg_golden_vacuity_guard_rejects_one_byte_fixture_perturbation_per_family` | Perturbs expected fixture comparisons; counts all three families. | unaudited |
| 108 | `dg_goldens_exercise_incremental_native_differential_mode` | Runs golden inputs through incremental native differential setup. | renamed by #830 to `dg_goldens_encode_native_output_deterministically`, which compares two full native encodes; unaudited |

Source: `crates/daemon/tests/prepared_output.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 33 | `transform_segments_preserve_existing_golden_bytes` | Compares segmented output to existing golden serialization. | unaudited |
| 207 | `inconsistent_source_reports_length_mismatch_without_emission` | Rejects a declared/actual byte-length mismatch. | unaudited |

## Wire projection, synthetic exclusion, and mutation

Source: `crates/daemon/src/wire.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 958 | `projection_retained_bytes_counts_wire_and_frontier_allocations_once` | Checks retained projection ownership accounting; adjacent identity seam. | deleted by #828 with the mechanism it checked |
| 1242 | `repeated_call_id_within_owner_message_shares_one_arc_identity` | Repeated call IDs use one owner-scoped arc. | unaudited |
| 1276 | `reasoning_joins_the_arc_its_adjacent_call_was_assigned` | Adjacent reasoning uses assigned call arc. | unaudited |
| 1357 | `user_carried_tool_result_pairs_with_prior_assistant_call` | Checks valid call/result pairing across roles. | unaudited |
| 1422 | `user_carried_tool_result_without_prior_call_still_rejects` | Rejects unpaired result input. | unaudited |
| 1447 | `opaque_and_media_inside_tool_result_content_are_accepted_and_projected` | Checks nested result payload projection. | unaudited |
| 1521 | `incremental_projection_reuses_prefix_storage_and_preserves_tool_arc_state` | Compares incremental state and prefix sharing. | deleted by #828 with the mechanism it checked |
| 1578 | `empty_and_reserved_message_ids_are_rejected` | Rejects empty or hash-containing mids. | unaudited |
| 1595 | `duplicate_message_ids_are_rejected_across_the_incremental_prefix` | Checks duplicate identity rejection across prefix boundary. | unaudited; #828 deletes the incremental arm and renames the test `duplicate_message_ids_are_rejected` |
| 1624 | `reduced_tool_result_keeps_failure_variant_and_output_extras` | Preserves tool error class and output extras on reduction. | unaudited |
| 1708 | `reattach_keeps_block_level_original_but_rebuilds_the_message_shell` | Pins old block-original versus message-shell behavior. | unaudited |
| 1749 | `repeated_prefix_reattachment_shares_canonical_shells` | Checks repeated sharing without mutation of raw ingress. | deleted by #828 with the mechanism it checked |
| 1796 | `shared_ingress_is_send_and_preserves_decode_refusals` | Compile-time Send/static and malformed-input assertions. | unaudited |
| 1818 | `incremental_projection_checks_effective_synthetic_status` | Effective synthetic status invalidates unsafe prefix reuse. | deleted by #828 with the mechanism it checked |

Source: `crates/memory-store/src/lib.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 16096 | `a_block_edit_leaves_its_sibling_byte_identical` | Checks edited text and an untouched unknown-envelope sentinel; legacy contract conflict. | unaudited |
| 18208 | `commit_then_load_roundtrips_and_bumps_row_version` | Metadata/core round-trip boundary for durable identity state. | unaudited |
| 21118 | `publish_history_summarizer_chunk_scans_transcript_and_raw_chunk_messages_before_storing` | Redacts/scans raw history before durable storage. | unaudited |
| 26369 | `descent_copies_raw_chunk_messages_through_transaction_redaction` | Preserves the expected redacted copied raw row. | unaudited |
| 26390 | `descent_refuses_raw_chunk_messages_under_a_protected_key` | Rejects protected-key raw history during copy. | unaudited |

Source: `crates/daemon/src/transform.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 12733 | `overlay_canonicalizes_only_the_mutated_block` | Checks tag edit, unknown sibling sentinel, and message provenance; legacy conflict. | unaudited |
| 14494 | `arc_identity_is_session_injective_for_reused_tool_ids` | Separates reused call IDs across owner messages. | unaudited |
| 14672 | `tail_identity_drift_re_adopts_atomically_and_attributes_served_divergence` | Checks permitted tail adoption and diagnostic attribution. | unaudited |
| 14832 | `boundary_anchor_and_frozen_tail_identity_drift_still_reject` | Checks protected identity rejection. | unaudited |
| 17917 | `completed_reasoning_first_execute_encodes_tagged_sibling_natively` | Checks native encoding of a mutated sibling beside reasoning. | unaudited |
| 25037 | `reduction_on_one_block_keeps_sibling_terse_text_compression_payload_across_restart` | Checks sibling frozen payload across restart. | unaudited |
| 25815 | `covered_system_content_drift_fails_identity_guard` | Rejects covered system-content drift. | unaudited |
| 27958 | `warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live` | Checks synthetic/live classification under warm-cache selection. | unaudited |
| 28739 | `fake_compaction_anchor_fail_closes_but_sibling_date_and_overlays_are_exempt` | Distinguishes protected anchor drift from permitted sibling changes. | unaudited |
| 28976 | `lineage_rebase_preserves_unflagged_synthetic_head` | Checks unflagged synthetic handling during lineage rebase. | unaudited |
| 29063 | `continued_lineage_tolerates_a_synthetic_head_like_the_seam_check` | Checks synthetic-head continuation behavior. | unaudited |

## Sidecar and native reconstruction

Source: `crates/daemon/src/codec/sidecar.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 488 | `oversized_alignment_takes_the_linear_memory_path_and_keeps_positional_pairs` | Checks bounded fallback retains positional pairs. | unaudited |
| 509 | `greedy_alignment_never_reuses_a_meta_and_preserves_order` | Checks one-use ordered matching. | unaudited |
| 520 | `greedy_alignment_pairs_fingerprinted_metas_through_the_fingerprint_index` | Checks fingerprint-indexed matching. | unaudited |

Source: `crates/daemon/src/codec/mod.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 59 | `opencode_golden_round_trips_wire_projected_parts_and_is_deterministic` | Compares decoded/encoded native values and declared coverage classes. | unaudited |
| 97 | `serve_native_golden_preserves_ingress_and_pins_synthetic_shapes` | Pins native ingress and synthetic boundary/todo shapes. | unaudited |
| 133 | `fresh_boundary_prefix_does_not_borrow_persisted_synthetic_meta` | Checks fresh boundary metadata does not alias a stored synthetic message. | unaudited |
| 184 | `pi_golden_round_trips_non_compaction_entries_and_is_deterministic` | Sibling codec compatibility check, not evidence Pi is a transform emitter. | unaudited |
| 222 | `codec_conformance_removes_leading_native_blocks_without_reindex_drift` | Checks deletion alignment across codecs. | unaudited |

Source: `crates/daemon/src/codec/opencode.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1389 | `every_fresh_tool_part_from_folded_transform_fixture_has_complete_state` | Checks fresh tool state completeness. | unaudited |
| 1463 | `fresh_tool_parts_are_byte_identical_across_consecutive_defer_passes` | Compares consecutive native tool bytes. | unaudited |
| 1492 | `adjacent_equal_kind_deletion_retains_the_surviving_native_part` | Checks identity rather than kind-only matching after deletion. | unaudited |
| 1531 | `mutated_survivor_keeps_its_own_native_extras_after_sibling_deletion` | Checks native metadata ownership after mutation/deletion. | unaudited |
| 1561 | `adjacent_idless_tools_match_their_synthesized_identity` | Checks stable synthetic tool identity matching. | unaudited |
| 1603 | `completed_and_error_attachments_round_trip_with_native_polarity` | Preserves attachment content and error polarity. | unaudited |
| 1699 | `adjacent_fresh_call_and_result_coalesce_for_message_v2_conversion` | Checks native call/result coalescing. | unaudited |
| 1741 | `mark_modified_tool_mutation_preserves_native_time_verbatim` | Checks native time survives a typed tool edit. | unaudited |
| 1783 | `empty_text_and_ignored_text_obey_wire_reachability` | Checks empty/ignored part classification. | unaudited |
| 1804 | `synthetic_todo_marker_survives_collapsed_pair_decode` | Preserves effective synthetic classification. | unaudited |
| 1830 | `compaction_is_extracted_as_boundary_not_content` | Keeps compaction out of block content. | unaudited |
| 1855 | `resolved_reasoning_exemption_and_completed_sibling_mutation_are_distinct` | Separates reasoning exemption from sibling mutation. | unaudited |
| 1910 | `lineage_anchor_and_live_assistant_exemptions_replay_native_envelopes_exactly` | Pins native replay for exempt envelopes. | unaudited |
| 1950 | `hard_epoch_fold_with_head_todo_coalesces_unmatched_reduced_tool_arc` | Checks reduced tool-arc native reconstruction. | unaudited |
| 2049 | `whole_array_tool_use_guard_rejects_both_id_collision_directions` | Rejects call-ID collisions in both directions. | unaudited |
| 2083 | `incremental_sidecar_carries_pins_across_three_generations` | Compares pin retention through multiple generations. | unaudited |
| 2157 | `text_before_latest_reasoning_uses_typed_wire_projection` | Checks the typed projection beside reasoning. | unaudited |

Source: `crates/daemon/src/codec/pi.rs`. These are shared-type regression leads,
not proof that Pi emits ingress through the scoped production transform path.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1051 | `response_id_arriving_later_does_not_replace_pinned_timestamp_mid` | Preserves pinned message identity. | unaudited |
| 1089 | `split_pipe_tool_ids_decode_to_canonical_id_and_round_trip` | Checks tool-ID translation and round trip. | unaudited |
| 1121 | `adjacent_tool_deletion_matches_the_surviving_native_id` | Checks surviving identity after deletion. | unaudited |
| 1151 | `leading_deletion_does_not_shift_the_next_block_onto_the_removed_slot` | Prevents positional metadata drift. | unaudited |
| 1169 | `duplicate_kind_adjacency_removes_only_the_deleted_text` | Separates equal-kind siblings. | unaudited |
| 1199 | `mutated_text_survivor_keeps_its_own_signature_and_vendor_extras` | Preserves surviving signature/extras. | unaudited |
| 1245 | `untouched_multi_text_tool_result_replays_raw_part_boundaries_and_extras` | Pins raw result boundaries/extras. | unaudited |
| 1269 | `mixed_image_error_tool_result_preserves_polarity_and_part_extras_on_mutation` | Preserves mixed media/error semantics. | unaudited |
| 1323 | `mixed_opaque_error_tool_result_retains_opaque_part` | Preserves opaque output payload. | unaudited |
| 1374 | `empty_error_tool_result_retains_empty_content_and_error_polarity` | Preserves empty error shape. | unaudited |
| 1398 | `frozen_deletion_replay_is_byte_stable` | Pins replay bytes after deletion. | unaudited |
| 1418 | `untouched_message_replays_the_exact_retained_raw_value` | Checks native raw replay, distinct from removed CK originals. | unaudited |
| 1441 | `deleted_tool_result_does_not_replay_the_retained_raw_entry` | Excludes a removed result on replay. | unaudited |
| 1458 | `compaction_entry_is_boundary_signal` | Excludes compaction from ordinary content. | unaudited |

## HistorySummarizer and synthetic pair

Source: `crates/daemon/src/history_summarizer.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 2275 | `selected_range_identity_drift_during_await_rejects_without_cooldown` | Checks selected-range identity drift while awaiting output. | unaudited |
| 2843 | `reattach_equal_length_identity_drift_rejects_before_publish` | Distinguishes equal-length content drift from length fingerprint equality. | unaudited |
| 3539 | `reattach_fingerprint_mismatch_recovers_to_idle_and_releases_routes` | Checks mismatched fingerprint recovery. | unaudited |
| 3925 | `chunk_fingerprint_uses_id_kind_and_byte_length` | Pins exact joined string and UTF-8 lengths. | unaudited |
| 4025 | `fingerprint_mismatch_at_publish_abandons_and_releases_single_flight` | Checks publication mismatch branch. | unaudited |

Source: `crates/daemon/src/history_summarizer_chunk.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1076 | `chunk_uses_flat_block_ids_and_covers_system_ordinals_without_their_text` | Checks item identities and filtered system coverage. | unaudited |
| 1106 | `media_in_compactable_head_uses_a_deterministic_placeholder` | Pins media rendering in a chunk. | unaudited |
| 1139 | `zero_block_messages_are_absorbed_as_pending_noise` | Checks empty-message range accounting. | unaudited |
| 1162 | `filtered_noise_adjacent_to_boundary_does_not_create_validation_gap` | Checks filtered range boundaries. | unaudited |
| 1203 | `duplicate_tool_call_ids_resolve_results_by_arc_id` | Checks result matching by arc identity. | unaudited |
| 1269 | `tool_result_only_message_absorbs_into_preceding_assistant_block` | Checks result carrier grouping. | unaudited |
| 1485 | `assemble_and_validate_second_fold_accepts_sparse_consumer_ordinals` | Checks sparse range assembly. | unaudited |
| 1642 | `budget_stop_and_tool_only_ranges_are_recorded` | Checks selected range bookkeeping. | unaudited |
| 1678 | `pending_noise_does_not_leak_when_budget_stops_before_next_block` | Checks pending filtered input at a stop boundary. | unaudited |
| 1711 | `separator_accounting_keeps_joined_32k_chunk_within_budget` | Checks joined byte accounting. | unaudited |
| 1744 | `truncation_uses_marker_and_keeps_multibyte_boundaries` | Checks exact marker and valid text boundary. | unaudited |
| 1773 | `history_summarizer_chunk_golden_fixture_matches_builder` | Compares chunk fixture with builder output. | unaudited |
| 1861 | `fixture_builder_drives_boundary_chunk_assembly` | Checks boundary assembly using fixture builder. | unaudited |

Source: `crates/daemon/src/injection.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 518 | `constants_match_ts_golden` | Pins shared todo constants. | unaudited |
| 530 | `injection_golden_matches_ts_todo_view` | Compares synthetic injection shape with TS golden. | unaudited |
| 569 | `defer_keeps_the_frozen_unit_and_bust_follows_the_persisted_state` | Checks frozen replay versus persisted replacement. | unaudited |
| 609 | `provisional_and_enabled_verdicts_capture_the_newest_visible_todowrite` | Checks state source for pair construction. | unaudited |
| 640 | `disabled_verdict_replays_until_bust_then_clears_without_recapture` | Checks frozen replay and disabled clearing. | unaudited |
| 685 | `aged_out_todowrite_injects_from_module_meta` | Checks construction from durable state. | unaudited |
| 702 | `explicit_empty_todowrite_clears_terminal_state` | Checks empty-state clearing. | unaudited |
| 718 | `defer_after_capture_replays_frozen_bytes` | Pins deferred pair replay. | unaudited |
| 752 | `empty_and_all_terminal_bust_clear_only_when_frozen` | Checks bounded pair lifecycle state. | unaudited |
| 776 | `none_state_does_not_first_apply_until_next_bust` | Checks no-state transition. | unaudited |
| 794 | `pair_byte_determinism_golden` | Same-input bytes/call IDs compare equal; changed-input values compare unequal. No literal or old-release byte pin. | unaudited |
| 834 | `synthetic_id_detection_is_prefix_only` | Pins synthetic-ID classification. | unaudited |

## Cache and persisted-reader seams

Source: `crates/daemon/src/lib.rs`, with HEAD line numbers.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 18128 | `boundary_token_cache_hash_fences_same_length_edits_and_evicts_lru` | Checks a content hash rather than length-only reuse. | unaudited |
| 18182 | `history_summarizer_boundary_construction_matches_owned_reference` | Compares firing construction with owned reference including raw bytes. | unaudited |
| 21811 | `cached_transform_response_writer_is_byte_identical_to_value_round_trip` | Compares cached response bytes with value reference. | unaudited |
| 22177 | `serve_native_adds_opencode_messages_without_changing_wire_response` | Preserves CK response when adding native messages. | unaudited |
| 22568 | `incremental_native_cache_replays_complex_prefix_and_encodes_only_tail` | Checks full/shared/reattached complex prefix output. | unaudited |
| 23008 | `native_cache_charge_keeps_raw_allocation_floor_beside_sidecar_estimate` | Checks ownership accounting beside identity cache state. | unaudited |
| 23248 | `astro_scale_projection_cache_reuses_on_the_second_pass` | Checks projection reuse. | deleted by #828 with the mechanism it checked |
| 23357 | `projection_cache_invalidators_are_mutation_checked_in_both_directions` | Perturbs projection cache inputs and invalidators. | deleted by #828 with the mechanism it checked |
| 23536 | `incremental_native_cache_invalidates_every_byte_affecting_input` | Checks byte-affecting native input invalidation. | deleted by #830 with the mechanism it checked |
| 23619 | `renderer_transition_class_sets_invalidate_with_consumed_boolean_stable` | Checks class-set invalidation beyond a stable boolean. | deleted by #830 with the mechanism it checked |
| 23734 | `reduced_shell_rematches_across_three_incremental_sidecar_generations` | Checks reduced shell matching across generations. | renamed by #830 to `reduced_shell_rematches_across_three_sidecar_generations`; unaudited |
| 23843 | `marker_representation_reconciles_after_changed_native_frontier` | Checks marker representation at changed frontier. | unaudited |
| 23913 | `newest_reasoning_becomes_historical_after_watermark_tail_advance` | Checks reasoning state transition at a moved frontier. | unaudited |
| 23988 | `frontier_vacuity_covers_opaque_repeats_eviction_and_same_length_edits` | Supplies opaque, eviction, and same-length frontier perturbations. | renamed by #830 to `same_length_edit_reaches_the_native_output`, which keeps only the same-length edit; unaudited |
| 24059 | `differential_assert_rejects_frontier_inside_mutated_native_region` | Negative control for native frontier differential. | unaudited |
| 24108 | `differential_assert_catches_corrupt_sidecar_key_derivation` | Negative control for sidecar key derivation. | deleted by #830 with the mechanism it checked |
| 25162 | `unflagged_synthetic_delta_prepares_history_summarizer_and_native_output` | Exercises synthetic normalization at history_summarizer/native seams. | unaudited |
| 25557 | `native_attachment_reuses_transform_tag_baseline_and_preserves_bytes` | Deleted with the tag baseline cache. | invalidated |
| 28294 | `ctx_expand_and_eidnara_note_facades_are_session_scoped` | Contains persisted history/facade session-scoping checks. | unaudited |
| 29493 | `ctx_expand_verbose_range_separates_messages_and_previews_raw_parts` | Checks expanded message formatting and raw part visibility. | unaudited |

Source: `crates/daemon/src/transform.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 28295 | `serialized_output_cache_reuses_steady_state_and_matches_fresh_bytes` | Compares cached and fresh served bytes. | unaudited |
| 28323 | `serialized_output_cache_tag_overlay_invalidates_only_its_message` | Checks mutation-local cache invalidation. | unaudited |
| 28361 | `serialized_output_cache_drop_invalidates_only_the_target` | Checks deletion-local invalidation. | unaudited |
| 28399 | `serialized_output_cache_fold_refreshes_prefix_and_reuses_tail` | Checks fold prefix replacement and tail reuse. | unaudited |
| 28585 | `serialized_output_cache_revert_epoch_bump_evicts_session` | Checks epoch-scoped eviction. | unaudited |

Source: `crates/daemon/src/divergence.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 124 | `first_divergence_classifies_each_boundary_kind_and_ignores_appends` | Checks receipt-vector divergence classes and cold-start/appended-prefix cases. | unaudited |

## Targeted durable hygiene and policy consumers

Source: `crates/daemon/src/tail_hygiene.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1148 | `measurement_is_identical_with_cold_and_warm_token_cache` | Compares cold/warm measurements. | unaudited |
| 1217 | `hygiene_digest_and_token_key_use_kind_prefixed_content` | Checks content-domain hash and token key. | unaudited |
| 1265 | `memo_preserves_each_derived_digest_domain` | Checks distinct measurement digest domains. | unaudited |
| 1366 | `memo_rechecks_terse_text_compression_identity_payload_and_context` | Perturbs memo identity, payload, and context. | unaudited |
| 1506 | `memo_bounds_sessions_bytes_and_refuses_over_budget_blocks` | Checks memo bounds and refusal without treating memo as durable baseline. | unaudited |
| 1638 | `memo_over_budget_keeps_a_warm_prefix_instead_of_resetting` | Preserves warm prefix under memo pressure. | unaudited |
| 1696 | `memo_retention_is_independent_of_terse_text_compression_payload_size` | Checks memo ownership/retention boundary. | unaudited |
| 1766 | `memo_namespaces_are_separate_and_the_least_recently_used_session_is_evicted` | Checks namespace separation and eviction. | unaudited |
| 1855 | `poisoned_session_memo_recovers_cold_through_every_path` | Checks cold memo recovery paths. | unaudited |
| 1905 | `distinct_sessions_neither_block_nor_evict_each_other_up_to_the_limit` | Checks session isolation. | unaudited |
| 1970 | `all_memo_sessions_near_budget_match_independent_retained_accounting` | Checks retained accounting beside memo identity. | unaudited |
| 2027 | `defer_delta_and_boundary_advance_are_additive` | Checks evaluability, deltas, and preserved baseline generation. | unaudited |
| 2071 | `non_append_mutation_invalidates_until_a_bust` | Asserts mutated baseline is unevaluable and invalidated. | unaudited |
| 2290 | `parity_golden_matches_ts_reference_across_full_corpus` | Compares hygiene corpus with TS reference. | unaudited |
| 2386 | `shared_row_iterator_matches_slice_for_protected_legacy_orphan` | Checks protected legacy-row input equivalence. | unaudited |
| 2434 | `reasoning_mutant_changes_neither_term_but_tagged_text_mutant_reddens` | Separates numeric U/T effects of part classes. | unaudited |
| 2492 | `consumed_protected_set_excludes_the_whole_exemplar_tool_arc_from_u` | Checks protected tool-arc U exclusion. | unaudited |
| 2572 | `recurring_raw_call_id_orphan_is_conservative_t_only` | Checks conservative orphan attribution. | unaudited |

Source: `crates/daemon/src/boundary.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1962 | `boundary_constants_match_ts_sources` | Pins boundary constants. | unaudited |
| 2035 | `chunk_golden_matches_ts_formatting` | Checks chunk output reference. | unaudited |
| 2073 | `boundary_golden_matches_ts_resolution` | Checks boundary resolution against reference. | unaudited |
| 2117 | `trigger_golden_matches_ts_decision_core` | Checks trigger decision/reference fields. | unaudited |
| 2261 | `completed_arcs_fence_a_straddling_boundary_backward_with_or_without_reasoning` | Checks arc fencing around a boundary. | unaudited |
| 2279 | `backward_reasoning_fence_closes_forward_when_the_invocation_is_already_covered` | Checks covered-call boundary correction. | unaudited |
| 2322 | `eligible_end_beyond_the_result_is_unchanged_by_the_backward_fence` | Checks unaffected eligible end. | unaudited |
| 2357 | `backward_reasoning_fence_cannot_split_a_neighboring_ordinary_arc` | Checks neighboring arc preservation. | unaudited |
| 2379 | `completed_arc_rule_keeps_newest_tool_result_out_of_eligible_head` | Checks newest result protection. | unaudited |
| 2400 | `fold_only_guard_protects_whole_newest_tool_arc` | Checks fold-only whole-arc protection. | unaudited |
| 2423 | `fold_only_guard_still_folds_large_plain_head` | Checks eligible plain-head folding. | unaudited |
| 2444 | `fold_only_guard_applies_during_emergency_tail_scaling` | Checks emergency scale and protection interaction. | unaudited |
| 2461 | `fold_only_guard_protects_multi_result_newest_arc` | Checks multi-result protection. | unaudited |
| 2528 | `fold_only_guard_folds_large_head_before_deep_newest_arc` | Checks large-head/deep-tail split. | unaudited |
| 2560 | `wrapup_counts_every_role_and_keeps_the_newest_messages` | Checks original-token role accounting. | unaudited |
| 2577 | `wrapup_moves_the_keep_watermark_before_a_straddling_tool_arc` | Checks wrapup ordinal protection. | unaudited |
| 2594 | `wrapup_terminal_guard_retains_the_newest_messages_complete_arc` | Checks newest complete-arc retention. | unaudited |
| 2610 | `wrapup_coverage_beyond_cached_terminal_returns_offset_anchored_empty_range` | Checks empty-range boundary at terminal. | unaudited |
| 2628 | `wrapup_user_snap_window_scales_with_session_geometry` | Checks geometry-dependent window. | unaudited |
| 2655 | `adding_newer_items_never_moves_protected_start_below_anchor` | Checks protected-start monotonic constraint. | unaudited |
| 2670 | `trigger_never_consumes_the_protected_tail` | Checks consume-through versus protection. | unaudited |
| 2689 | `zero_based_trigger_counts_ordinal_zero_content` | Checks zero ordinal token accounting. | unaudited |
| 2710 | `history_segment_ending_at_ordinal_zero_starts_next_window_at_one` | Checks next-range ordinal arithmetic. | unaudited |
| 2736 | `boundary_measures_original_bytes_not_rendered_reduction_placeholders` | Checks original-token basis independently of rendered reduction. | unaudited |

Source: `crates/daemon/src/selection.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 1621 | `selection_golden_matches_ts_selectors` | Compares selector outcomes with reference. | unaudited |
| 1944 | `emergency_floor_counts_non_tool_live_text_and_reasoning` | Checks byte-derived active floor. | unaudited |
| 1992 | `emergency_reclaim_does_not_count_retained_reasoning` | Checks reclaim accounting excludes retained content. | unaudited |
| 2031 | `emergency_eviction_carries_pending_supersession` | Checks emergency selection with pending work. | unaudited |
| 2338 | `supersession_floor_tracks_active_tag_window_on_thirty_arc_fixture` | Checks floor/window interaction. | unaudited |
| 2419 | `provider_executed_arc_never_targeted` | Checks provider-executed exclusion. | unaudited |
| 2473 | `dynamic_block_protection_filters_automatic_and_agent_drop_decisions` | Checks protection in selection outputs. | unaudited |
| 3182 | `defer_pass_produces_nothing` | Checks no reduction decisions on defer. | unaudited |

Source: `crates/daemon/tests/selection_differential.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 2186 | `optimized_matches_frozen_reference` | Property comparison with frozen selector. | unaudited |
| 2198 | `optimized_matches_frozen_reference_across_skeleton_window` | Checks skeleton-window cases. | unaudited |
| 2208 | `optimized_matches_frozen_reference_for_cross_owner_duplicates` | Checks owner-scoped duplicate inputs. | unaudited |
| 2238 | `optimized_matches_frozen_reference_for_tied_duplicate_ordinals` | Checks tied ordinal cases. | unaudited |
| 2274 | `optimized_matches_frozen_reference_for_distinct_edit_paths` | Checks distinct tool-edit inputs. | unaudited |
| 2304 | `optimized_matches_frozen_reference_at_age_reclaim_threshold` | Checks threshold-adjacent age reclaim. | unaudited |
| 2336 | `optimized_matches_frozen_reference_at_payload_boundaries` | Checks payload-boundary behavior. | unaudited |
| 2399 | `optimized_matches_frozen_reference_at_emergency_rearm_threshold` | Checks emergency threshold behavior. | unaudited |
| 2443 | `optimized_matches_frozen_reference_for_agent_drop_at_exact_ceiling` | Checks exact-ceiling behavior. | unaudited |
| 2470 | `optimized_matches_frozen_reference_across_emergency_tiers` | Checks tiered selection. | unaudited |
| 2482 | `optimized_matches_frozen_reference_on_reasoning_adjacency` | Checks adjacent reasoning. | unaudited |
| 2494 | `optimized_matches_frozen_reference_on_duplicate_calls` | Checks duplicate calls. | unaudited |
| 2506 | `optimized_matches_frozen_reference_across_eidnara_reduce_keep_boundary` | Checks keep boundary. | unaudited |
| 2518 | `optimized_matches_frozen_reference_on_clamped_payloads` | Checks clamped payloads. | unaudited |
| 2530 | `optimized_matches_frozen_reference_on_withheld_supersession` | Checks withheld outcome behavior. | unaudited |
| 2540 | `reasoning_guard_matches_frozen_reference_on_unfiltered_decisions` | Checks reasoning guard against reference. | unaudited |
| 2624 | `generators_reach_every_decision_class` | Checks generator decision-class reachability. | unaudited |
| 2690 | `every_production_variant_is_generated` | Checks generator variant coverage; not the new wire-shape marker. | unaudited |

Source: `crates/daemon/src/transform.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 8998 | `channel1_minimum_tail_floor_bands_and_cadence_match_reference` | Checks reminder floor/band/cadence behavior. | unaudited |
| 20078 | `missing_publication_floor_recuts_conservatively_and_backfills_on_hard` | Checks stored floor consumer and backfill. | unaudited |
| 28914 | `continued_lineage_stable_defer_rejects_mutated_anchor_after_post_seam_fold` | Checks changed anchor enforcement after fold. | unaudited |
| 29121 | `continued_lineage_missing_boundary_aborts_instead_of_rebasing_to_one` | Checks missing continued-lineage boundary. | unaudited |

Source: `crates/memory-store/src/lib.rs`.

| Line | Test | Check semantics or message | Status |
| --- | --- | --- | --- |
| 26125 | `descent_copies_verbatim_ranges_and_session_notes_without_replay_duplicates` | Includes assertions for copied anchor hash/base at the durable boundary. | unaudited |

## Plugin emission and scalar contract

Source: `packages/opencode-plugin/src/hooks/context/module-wire.test.ts`.
The Check column names the individual `it` case and its assertion claim.

| Line | Check | Status |
| --- | --- | --- |
| 19 | `marks a collapsed synthetic todo pair as synthetic CK ingress` | unaudited |
| 45 | `emits only a tool_result for a later part that repeats an already-emitted call id` | unaudited |
| 69 | `keeps the tool_call for a first-seen unfinished tool part without input` | unaudited |
| 84 | `keeps non-object parts as unknown opaque blocks` | unaudited |
| 120 | `reads the nested creation timestamp before the flat aliases` | unaudited |
| 134 | `preserves an explicitly empty reasoning signature` | unaudited |
| 148 | `hashes fallback ids over the value JSON serialization emits` | unaudited |
| 159 | `preserves an explicitly empty tool-call id as the daemon does` | unaudited |
| 177 | `preserves an explicit zero absolute ordinal` | unaudited |
| 190 | `ignores explicit ordinals the daemon cannot read as u64` | unaudited |
| 202 | `propagates provider-executed metadata to the call and result blocks` | unaudited |
| 241 | `honors the daemon's tool-name aliases and default` | unaudited |
| 268 | `synthesizes the daemon's deterministic id for a tool part without one` | unaudited |
| 301 | `reads a reasoning signature nested under metadata` | unaudited |
| 327 | `reads completion status and output from top-level tool fields` | unaudited |
| 370 | `carries tool-result attachments as content result blocks` | unaudited |
| 441 | `keeps step-finish parts as opaque blocks with the daemon's source shape` | unaudited |
| 467 | `carries part metadata as the daemon's provider extras` | unaudited |
| 496 | `decodes an empty reasoning part with redacted data as redacted reasoning` | unaudited |
| 516 | `attaches the daemon's approval arc to opaque parts with an approvalId` | unaudited |
| 544 | `falls back to the daemon's message id chain and stable hash` | unaudited |
| 559 | `falls back to the top-level role before defaulting to user` | unaudited |
| 572 | `carries the daemon's message origin from provider and model ids` | unaudited |
| 597 | `encodes file and image parts as media blocks` | unaudited |
| 656 | `matches the module golden generated from raw OpenCode reasoning parts` | unaudited |
| 688 | `renders numbers the way the daemon's canonical_number does` | unaudited |
| 717 | `serializes values the way serde_json::to_string does` | unaudited |
| 763 | `hashes tool inputs the way the daemon's stable_hash_prefix does` | unaudited |
| 786 | `orders object keys by Unicode code point like Rust strings` | unaudited |
| 1023 | `uses one predicate for the ordinal resolver and the CK encoder` | unaudited |
| 1080 | `lets a wholly synthetic message borrow the preceding ordinal` | unaudited |

Paging, database race, transport, and admission tests in this module are owned
by the decode/latency catalogs; they are not evidence for canonical block bytes.

## Production assertions and invariant guards

| HEAD location | Semantics and message or failure | Status |
| --- | --- | --- |
| `crates/daemon/src/served_json.rs:54` | `expect`: serde must close an open object. | unaudited |
| `crates/daemon/src/served_json.rs:65,71,77` | `expect`: keys/values belong to an open object. | unaudited |
| `crates/daemon/src/served_json.rs:72,78` | `expect`: a key has begun before completion. | unaudited |
| `crates/daemon/src/served_json.rs:161` | `expect`: serde emits valid escaped string keys. | unaudited |
| `crates/daemon/src/wire.rs:374` | `expect`: differential projection bytes serialize. | unaudited |
| `crates/daemon/src/wire.rs:516-517` | Debug-only equality of message/frontier vector lengths. | unaudited |
| `crates/daemon/src/wire.rs:528-537` | Runtime rejection of empty, reserved-hash, or duplicate mids. | unaudited |
| `crates/daemon/src/wire.rs:599-617` | Runtime filter excludes synthetic ingress identities. | unaudited |
| `crates/daemon/src/wire.rs:805-817` | Runtime `UnpairedToolResult` rejection. | unaudited |
| `crates/daemon/src/wire.rs:875-879` | Equality gate before projected receipt reuse. | unaudited |
| `crates/daemon/src/wire.rs:901` | `expect`: CK block equality identity must serialize. | unaudited |
| `crates/daemon/src/transform.rs:166,199` | `expect`: CK message/block serialization succeeds. | unaudited |
| `crates/daemon/src/transform.rs:2035-2044` | Differential assertions: `incremental prefix projection byte drift` and `state drift`. | unaudited |
| `crates/daemon/src/transform.rs:2126-2141` | Runtime normalization of synthetic todo IDs in a pass-local view. | unaudited |
| `crates/daemon/src/transform.rs:5172-5225` | Stored identity equality, scoped re-adoption, `IdentityDrift`, and `FrozenRedTargetVanish`. | unaudited |
| `crates/daemon/src/transform.rs:5265,5284` | `expect`: identity vectors serialize and re-adoptions name projected messages. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:169-174` | Exact codec namespace strip; serialization failure falls back to `Null`. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:196-201` | Stamp validation requires integer indexes and string fingerprint. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:209-241` | Fingerprint equality/origin gates before unchanged-content or alignment decisions. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:263-267` | Alignment-size guard selects bounded-memory fallback. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:407-420` | Serialization failure hashes empty bytes; no explicit invariant assertion. | unaudited |
| `crates/daemon/src/codec/sidecar.rs:442-450` | Synthetic messages cannot use positional metadata fallback. | unaudited |
| `crates/daemon/src/codec/opencode.rs:277-278` | Debug-only bounds on incremental replacement against messages and prior order. | unaudited |
| `crates/daemon/src/codec/opencode.rs:501-508` | Debug-only uniqueness assertion: `OpenCode serialization produced duplicate tool_use ids`. | unaudited |
| `crates/daemon/src/history_summarizer.rs:326-334,397-419` | Fingerprint equality gate rejects mismatch, including at publish. | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs:418-430` | Snapshot filters synthetic/system/out-of-range blocks before measuring length. | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs:646-675` | Missing selected identity refuses firing; raw arrays exclude synthetic messages. | unaudited |
| `crates/daemon/src/lib.rs:16443-16461` | Missing/malformed raw rows skip; ordinal bounds and first-per-ordinal selection apply. | unaudited |
| `crates/daemon/src/lib.rs:13306` | `expect`: OpenCode sidecar metadata serializes for its cache key. | unaudited |
| `crates/daemon/src/lib.rs:13630,13665` | `expect`: a compatible native cache has a snapshot. | unaudited |
| `crates/daemon/src/lib.rs:13722-13738` | Enabled differential serializes full/incremental native output and asserts no `incremental native attachment cache drift`. | unaudited |
| `crates/daemon/src/transform.rs:4929` | `expect`: divergence diagnostics serialize before the new durable receipt vector is assigned. | unaudited |
| `crates/memory-store/src/lib.rs:1301-1319` | Frozen-pair decode clears retained originals for typed replay. | unaudited |
| `crates/memory-store/src/lib.rs:11298-11321` | Raw history enters durable redaction before storage. | unaudited |
| `crates/daemon/src/tail_hygiene.rs:975-1004` | Prefix identity guard compares hashes, kinds, tokens, tags, and protection; returns no matching prefix on drift. | unaudited |
| `crates/daemon/src/tail_hygiene.rs:1016-1045` | Preserves invalidation or makes nonmatching non-bust baseline unevaluable; `expect` guards require the prior baseline. | unaudited |
| `crates/daemon/src/tail_hygiene.rs:1066-1095` | Clamps U/T and selects reminder bands at explicit floors/ratios. | unaudited |
| `crates/daemon/src/transform.rs:2199-2259` | Anchor identity/base, ordinal, live/text position, and exact content-hash validation. | unaudited |
| `crates/daemon/src/transform.rs:3957-3960,4614-4616,4782-4788` | Anchor failure selects Defer/reconcile and no-trim output view. | unaudited |
| `crates/daemon/src/transform.rs:4592-4602` | Publication floor is backfilled only on an applicable initialized HARD/MigrateHard covered pass. | unaudited |
| `crates/daemon/src/transform.rs:5898-5913` | Accumulated byte-token estimates stop at the protected floor target. | unaudited |
| `crates/daemon/src/lib.rs:2055-2080` | Cached boundary token count requires matching block ID, byte length, and hash; otherwise exact bytes are tokenized. | unaudited |
| `crates/daemon/src/boundary.rs:398-409,1089-1099` | Empty/nonfinite/undersized token-index boundary cases have explicit results. | unaudited |
| `crates/daemon/src/selection.rs:938-954,984-1011` | Numeric/idempotence guards, protection/reserve exclusion, and reclaim stop govern selection. | unaudited |
| `crates/memory-store/src/lib.rs:9982,10364-10366` | Anchor hash crosses existing-identity validation and is assigned with anchor ID/base to durable metadata. | unaudited |

The message-only test-support wrapper also has a serialization `expect` at
`crates/daemon/src/served_json.rs:118` (unaudited). Serde type/default validation
for wire blocks is at `crates/memory-store/src/lib.rs:114-161,243-279,326-447`
(unaudited). These are existing enforcement surfaces, not proof that a new
default-omission policy has been validated.

## Empty categories and suspiciously quiet areas

- No common `canonical_block_bytes` implementation or `block_bases_agree.rs`
  check is found at HEAD. The plan proposes both.
- No complete emitter-shaped projection corpus is found in the current
  `wire-golden.json`/projection pair. Ten explicit-false tool blocks are not
  absent/true plugin coverage. No media entry exists in that pair.
- No independent 25-byte tool-block witness is found. Exact compact member
  removal with its comma is 26 bytes. No benchmark was used to derive it.
- No old/new decoder upgrade check through the real durable reader is found.
  Stored-history tests do not establish replacement readability by name alone.
- No production snapshot check requires a synthetic todo item; the inspected
  production builder excludes it. Such an item is test-only if fabricated.
- No implementation of the twelve independent identity situation markers is
  found. The catalog record is rollup only, not an aggregate runtime check.
- No replacement-decode campaign freezes and reloads durable hygiene parts,
  content signatures, or anchor hashes. Existing tests do not establish that
  upgrade property by name alone.
- No replacement-decode corpus freezes all token/fold/protected-tail outcomes.
  Existing numeric tests are leads, not permission to accept semantic drift.
- The current four-output plugin matrix is not complete typed coverage. The
  separate typed-only marker includes all seven outputs and null denial reason.
- No proof that changed output hashes are exclusively ephemeral exists;
  durable served receipts refute that blanket premise.
- No standalone memory-store test freezes all new unknown/default omission
  rules. Legacy sibling/original tests assert different behavior.
- No distributed-consensus, transport-authentication, or crash-durability check
  is required by this local serialization slice. This is a scope statement,
  not a claim those domains have no tests in the repository.

## Handoffs

`/testing:invariant-test-review` receives every test row, prioritizing legacy
original-sensitive goldens, signed-zero reuse, and native matching. It must
assess the actual oracle rather than infer adequacy from these descriptions.
`/low-level-systems:defensive-assertions-and-invariant-guards` receives every
production guard, including debug-only guards and silent fallback behavior.
`/testing:test-strategy` owns new checks at the exact seams in the fault map.
Completed fresh evaluator `ses_f6756093fffeVjNp36S3E8pKrM` supplied the gaps
dispositioned in `portfolio-evaluation.md`; no rerun is claimed. Existing
latency-audit execution evidence stays in its own files. W1 is invalidated,
has no active measurement ownership, and is not reactivated here.
