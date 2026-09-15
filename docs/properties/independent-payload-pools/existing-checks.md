# Existing-check inventory: independent payload pools

Every claim-bearing test for the payload-pool transport at the branch head; line numbers are re-resolved with each code change on the branch.
An existing check does not remove a property from the catalog; each entry is
`unaudited` until an invariant-test review reads its assertion.

## `crates/shm-transport/src/backend/ring.rs` - 33 tests (Ring unit and Miri tests)

| Test | Status |
| --- | --- |
| `every_page_accessor_reads_the_initialized_zero_state` (`:2072`) | unaudited |
| `slot_and_cell_indexes_past_their_regions_are_refused_before_any_dereference` (`:2127`) | unaudited |
| `descriptor_snapshot_is_copied_out_field_by_field_and_validated` (`:2147`) | unaudited |
| `completion_cell_publication_is_monotonic` (`:2174`) | unaudited |
| `lifecycle_snapshot_sees_a_write_made_through_the_raw_page` (`:2185`) | unaudited |
| `held_payload_stays_intact_while_another_block_is_reused_beyond_descriptor_laps` (`:2287`) | unaudited |
| `descriptor_consumption_frees_a_slot_while_the_payload_stays_held` (`:2322`) | unaudited |
| `every_class_backpressures_without_spill_and_returns_wake_capacity` (`:2356`) | unaudited |
| `abort_underfill_and_short_commit_conserve_blocks_and_records` (`:2385`) | unaudited |
| `zero_body_class_boundaries_maximum_and_maximum_plus_one_have_explicit_outcomes` (`:2420`) | unaudited |
| `reserved_inventories_progress_when_ordinary_descriptors_are_exhausted` (`:2462`) | unaudited |
| `take_reclaimed_never_reports_a_block_reserved_since_its_return` (`:2513`) | unaudited |
| `forged_descriptors_quarantine_before_exposing_bytes` (`:2540`) | unaudited |
| `stale_returns_free_nothing_and_future_completions_quarantine` (`:2635`) | unaudited |
| `unflagged_future_completions_are_caught_at_reuse_and_by_probe` (`:2662`) | unaudited |
| `retirement_at_a_counter_boundary_preserves_live_leases` (`:2708`) | unaudited |
| `owned_lease_outlives_both_endpoint_handles_and_returns_once` (`:2740`) | unaudited |
| `worker_final_drop_wakes_a_capacity_parked_producer_without_incoming_data` (`:2764`) | unaudited |
| `descriptor_consumption_alone_wakes_a_descriptor_parked_producer` (`:2804`) | unaudited |
| `arm_capacity_wait_refuses_to_park_over_a_return_that_landed_before_arming` (`:2846`) | unaudited |
| `is_fresh_reads_the_live_cursors_not_the_attach_snapshot` (`:2897`) | unaudited |
| `consumer_inventory_does_not_reclaim_the_producer_completion_stream` (`:2908`) | unaudited |
| `a_failed_return_wake_reports_wake_failed_and_keeps_the_completion` (`:2928`) | unaudited |
| `a_failed_consumption_wake_quarantines_the_consumer_and_returns_the_block` (`:2963`) | unaudited |
| `reclaim_loads_only_the_cells_of_returned_blocks_while_others_stay_held` (`:2987`) | unaudited |
| `wake_failure_after_publication_quarantines_but_leaves_the_frame_published` (`:3028`) | unaudited |
| `attach_rejects_eventfd_and_datagram_doorbells_and_a_second_producer` (`:3061`) | unaudited |
| `attach_sets_close_on_exec_on_every_descriptor` (`:3128`) | unaudited |
| `grant_round_trips_and_rejects_every_malformation` (`:3148`) | unaudited |
| `attachment_can_be_handed_to_another_thread_and_the_ring_cannot` (`:3187`) | unaudited |
| `quarantine_rejects_operations_and_survives_the_peer_clearing_the_flag` (`:3204`) | unaudited |
| `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:3231`) | unaudited |
| `forbidden_operation_observers_stay_unreached_across_a_saturated_drop_storm` (`:3243`) | unaudited |

## `crates/shm-transport/src/lease.rs` - 10 tests (Lease and copy tests (Miri))

| Test | Status |
| --- | --- |
| `owned_lease_exposes_exact_bytes_and_returns_exactly_once` (`:544`) | unaudited |
| `owned_lease_drop_returns_once_after_moving_to_another_thread` (`:573`) | unaudited |
| `stale_completion_cannot_lower_a_newer_one` (`:588`) | unaudited |
| `backing_outlives_the_last_endpoint_handle_until_the_lease_returns` (`:606`) | unaudited |
| `final_drop_reaches_no_forbidden_operation` (`:621`) | unaudited |
| `copy_in_then_copy_out_round_trips_at_every_alignment_and_length` (`:631`) | unaudited |
| `access_shape_partitions_the_range_on_aligned_words` (`:665`) | unaudited |
| `read_byte_agrees_with_copy_to_at_every_alignment` (`:699`) | unaudited |
| `span_null_base_is_refused` (`:730`) | unaudited |
| `span_reads_tolerate_a_concurrent_writer` (`:737`) | unaudited |

## `crates/shm-transport/src/pool.rs` - 5 tests (Geometry tests)

| Test | Status |
| --- | --- |
| `production_geometry_matches_the_specified_inventory` (`:551`) | unaudited |
| `placement_is_dense_and_offsets_never_overlap` (`:570`) | unaudited |
| `class_selection_takes_the_smallest_fit_without_spill` (`:588`) | unaudited |
| `geometry_rejects_every_invalid_shape` (`:623`) | unaudited |
| `layout_places_every_region_in_order_and_pads_to_pages` (`:673`) | unaudited |

## `crates/shm-transport/tests/ring.rs` - 9 tests (Real-endpoint and two-process tests)

| Test | Status |
| --- | --- |
| `production_profile_round_trips_a_maximum_frame_in_both_directions` (`:74`) | unaudited |
| `artifact_mismatch_fails_before_mapping_and_unsealed_objects_are_rejected` (`:104`) | unaudited |
| `attachment_object_must_carry_exactly_the_owner_read_write_mode` (`:181`) | unaudited |
| `non_regular_attachment_object_is_rejected_before_mapping` (`:194`) | unaudited |
| `ring_memfd_carries_the_registered_name` (`:205`) | unaudited |
| `two_process_exchange_holds_a_reuses_b_and_wakes_on_return` (`:347`) | unaudited |
| `ring_child_exchange` (`:443`) | unaudited |
| `two_process_descriptor_consumption_wakes_a_parked_producer_without_a_return` (`:486`) | unaudited |
| `ring_child_hold` (`:566`) | unaudited |

## `crates/shm-transport/tests/contract.rs` - 9 tests (Contract tests)

| Test | Status |
| --- | --- |
| `descriptor_rejects_every_untrusted_identity_block_and_length_failure` (`:27`) | unaudited |
| `completion_rejects_stale_future_and_wrong_pool_returns` (`:118`) | unaudited |
| `wire_header_check_is_shared_by_producer_and_consumer` (`:152`) | unaudited |
| `sole_identifiers_are_four_and_the_pool_profile` (`:167`) | unaudited |
| `class_boundaries_are_full_frame_capacity_minus_the_header` (`:187`) | unaudited |
| `hardware_profile_id_deserialization_enforces_constructor_rules` (`:226`) | unaudited |
| `lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal` (`:249`) | unaudited |
| `debug_and_errors_redact_every_sentinel` (`:317`) | unaudited |
| `payload_pool_protocol_document_agrees_with_the_implementation` (`:364`) | unaudited |

## `crates/shm-transport/tests/profile.rs` - 8 tests (Profile and admission tests)

| Test | Status |
| --- | --- |
| `debug_redacts_profile_admission_and_quarantine_record` (`:59`) | unaudited |
| `host_admission_retains_quarantined_commitments` (`:79`) | unaudited |
| `exact_aggregate_capacity_admits_n_and_rejects_n_plus_one_without_charging` (`:107`) | unaudited |
| `every_limit_is_checked_in_field_order` (`:131`) | unaudited |
| `worker_limit_is_the_only_limit_that_refuses_a_second_fused_admission` (`:197`) | unaudited |
| `split_admission_settles_worker_and_backing_charges_independently` (`:215`) | unaudited |
| `host_payload_pool_profile_names_one_geometry_and_complete_charges` (`:261`) | unaudited |
| `profile_refuses_a_geometry_that_cannot_place_a_maximum_frame` (`:293`) | unaudited |

## `crates/shm-transport/tests/fuzz_corpus.rs` - 2 tests (Fuzz corpus replay)

| Test | Status |
| --- | --- |
| `every_decoder_corpus_replays_without_panic` (`:78`) | unaudited |
| `golden_grant_fixture_matches_the_frozen_pool_profile_encoding` (`:92`) | unaudited |

## `crates/host-runtime/src/ring_transport.rs` - 45 tests (Host transport tests)

| Test | Status |
| --- | --- |
| `production_profile_affords_five_connections_under_the_byte_ceiling` (`:1714`) | unaudited |
| `process_limits_reject_counts_above_the_resident_byte_ceiling` (`:1724`) | unaudited |
| `shared_memory_workers_have_no_periodic_polling` (`:1752`) | unaudited |
| `finish_wakes_after_read_cancellation_with_unread_peer_data` (`:1767`) | unaudited |
| `a_finishing_endpoint_with_a_blocked_head_parks_instead_of_spinning` (`:1845`) | unaudited |
| `construction_has_no_ring_side_effects` (`:1928`) | unaudited |
| `diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts` (`:1936`) | unaudited |
| `grant_hex_is_strict_lowercase_ascii_without_panics` (`:1985`) | unaudited |
| `setup_rejects_grants_whose_lanes_do_not_match_their_direction` (`:1994`) | unaudited |
| `inbound_materialization_cannot_exceed_its_byte_budget` (`:2031`) | unaudited |
| `a_blocked_ping_sharing_a_correlation_does_not_hold_back_a_channel_zero_error` (`:2131`) | unaudited |
| `arming_publishes_a_bypassable_terminal_whose_block_returned_before_arming` (`:2161`) | unaudited |
| `arming_against_the_blocked_head_refuses_to_park_over_a_return_before_arming` (`:2192`) | unaudited |
| `a_blocked_ticket_deadline_retires_the_generation_while_delivery_is_blocked` (`:2218`) | unaudited |
| `a_budget_wait_publishes_a_blocked_ticket_when_the_peer_returns_capacity` (`:2281`) | unaudited |
| `control_frame_body_is_copied_out_of_the_ring` (`:2349`) | unaudited |
| `budget_wait_observes_read_cancellation_without_retiring` (`:2406`) | unaudited |
| `budget_wait_observes_discard_without_retiring` (`:2463`) | unaudited |
| `read_cancellation_drains_frames_committed_before_it` (`:2516`) | unaudited |
| `cancellation_reports_after_one_ring_depth_under_sustained_inbound` (`:2579`) | unaudited |
| `root_cancellation_is_observed_under_sustained_inbound` (`:2647`) | unaudited |
| `root_cancellation_is_observed_while_the_inbound_queue_is_full` (`:2714`) | unaudited |
| `transport_fault_is_reported_while_the_inbound_queue_is_full` (`:2767`) | unaudited |
| `endpoint_panic_is_reported_while_the_inbound_queue_is_full` (`:2812`) | unaudited |
| `a_peer_still_attached_after_an_orderly_close_keeps_the_backing_charge_in_quarantine` (`:2878`) | unaudited |
| `a_doorbell_with_a_queued_token_ahead_of_end_of_file_still_reads_as_released` (`:2918`) | unaudited |
| `a_lease_the_peer_keeps_after_closing_holds_the_backing_charge_in_quarantine` (`:2935`) | unaudited |
| `peer_close_refunds_admission_although_the_backend_quarantines_the_ring` (`:3001`) | unaudited |
| `root_cancellation_ends_a_budget_wait` (`:3065`) | unaudited |
| `a_commit_past_the_write_deadline_is_refused` (`:3113`) | unaudited |
| `a_client_send_past_its_frame_deadline_publishes_nothing` (`:3149`) | unaudited |
| `client_send_and_try_send_share_the_frame_inventory` (`:3180`) | unaudited |
| `quarantined_ring_moves_its_charges_to_the_quarantined_bucket` (`:3226`) | unaudited |
| `a_publisher_does_not_preallocate_its_configured_depth` (`:3325`) | unaudited |
| `into_private_reports_a_failed_return_wake_as_a_transport_error` (`:3339`) | unaudited |
| `eligible_controls_and_unrelated_terminals_publish_past_a_blocked_ordinary_ticket` (`:3372`) | unaudited |
| `an_unreserved_direct_serializer_never_runs_and_a_reserved_one_runs_once` (`:3432`) | unaudited |
| `a_terminal_credit_returns_with_its_block_not_with_settlement` (`:3484`) | unaudited |
| `a_credit_on_a_reused_block_waits_for_the_new_publication_to_return` (`:3518`) | unaudited |
| `a_pending_ticket_past_its_deadline_retires_instead_of_waiting` (`:3593`) | unaudited |
| `inventory_classification_reserves_controls_and_small_terminals_only` (`:3606`) | unaudited |
| `barrier_held_copy_returns_block_and_charge_once_after_physical_completion` (`:3653`) | unaudited |
| `refusals_are_counted_by_exhausted_resource_and_charge_nothing` (`:3775`) | unaudited |
| `ended_connections_leave_no_dead_backing_entries_without_a_status_request` (`:3827`) | unaudited |
| `return_snapshot_separates_outstanding_leases_from_released_backing` (`:3855`) | unaudited |

## `crates/host-runtime/tests/dispatch.rs` - 31 tests (Host dispatch tests)

| Test | Status |
| --- | --- |
| `a_unary_request_dispatches_once_with_one_matching_terminal` (`:23`) | unaudited |
| `an_empty_body_request_dispatches_and_settles` (`:57`) | unaudited |
| `an_application_error_is_a_terminal_for_its_correlation_only` (`:81`) | unaudited |
| `streams_are_ordered_and_end_with_exactly_one_terminal` (`:130`) | unaudited |
| `correlation_namespaces_are_per_generation_and_strictly_increasing` (`:179`) | unaudited |
| `a_non_increasing_correlation_closes_the_generation_before_dispatch` (`:234`) | unaudited |
| `an_unknown_route_is_refused_with_zero_dispatch` (`:294`) | unaudited |
| `saturated_request_capacity_returns_server_busy_without_dispatch` (`:318`) | unaudited |
| `cancel_and_completion_settle_exactly_once` (`:381`) | unaudited |
| `simultaneous_cancel_and_completion_still_emit_one_terminal` (`:476`) | unaudited |
| `cancelling_a_stream_stops_it_with_one_terminal` (`:527`) | unaudited |
| `a_handler_panic_maps_to_one_redacted_internal_error` (`:575`) | unaudited |
| `handler_panic_payload_is_redacted_from_process_stderr` (`:627`) | unaudited |
| `blocking_work_panic_payload_is_redacted_from_process_stderr` (`:634`) | unaudited |
| `panic_redaction_subprocess_child` (`:667`) | unaudited |
| `cancel_waits_for_the_request_blocking_work` (`:749`) | unaudited |
| `route_close_waits_for_the_request_blocking_work` (`:805`) | unaudited |
| `route_close_waits_for_blocking_work_the_handler_did_not_await` (`:854`) | unaudited |
| `a_blocking_work_panic_settles_as_one_internal_error` (`:896`) | unaudited |
| `oversized_handler_output_cannot_corrupt_framing` (`:933`) | unaudited |
| `concurrent_handler_output_is_reserved_before_allocation` (`:980`) | unaudited |
| `egress_budget_deadline_retires_the_generation` (`:1056`) | unaudited |
| `a_control_rejection_settles_while_ordinary_egress_is_exhausted` (`:1105`) | unaudited |
| `closing_a_route_settles_its_admitted_work` (`:1157`) | unaudited |
| `concurrent_requests_never_interleave_frame_bytes` (`:1215`) | unaudited |
| `saturated_model_execution_reserve_cannot_consume_a_general_slot` (`:1294`) | unaudited |
| `saturated_general_capacity_cannot_consume_the_model_execution_reserve` (`:1416`) | unaudited |
| `work_offered_while_the_route_drains_is_joined_or_refused` (`:1512`) | unaudited |
| `detached_blocking_work_observes_route_cancellation` (`:1574`) | unaudited |
| `held_blocking_work_retains_handler_and_instance_after_fatal_close` (`:1604`) | unaudited |
| `terminal_credits_bound_admission_and_return_with_the_settled_block` (`:1649`) | unaudited |

## `crates/host-runtime/src/config.rs` - 12 tests (Host limits tests)

| Test | Status |
| --- | --- |
| `defaults_validate` (`:516`) | unaudited |
| `noncanonical_payload_digests_are_rejected` (`:521`) | unaudited |
| `zero_limits_rejected` (`:551`) | unaudited |
| `the_resident_cap_splits_into_three_non_overlapping_pools` (`:563`) | unaudited |
| `byte_budget_below_interop_minimum_rejected` (`:588`) | unaudited |
| `aggregate_overflow_is_rejected_before_activation` (`:615`) | unaudited |
| `oversize_byte_budget_rejected` (`:646`) | unaudited |
| `constructor_capacity_bounds_are_validated` (`:658`) | unaudited |
| `daemon_version_boundary_keeps_auth_and_discovery_readable` (`:685`) | unaudited |
| `daemon_version_must_carry_the_published_prefix` (`:719`) | unaudited |
| `zero_durations_rejected` (`:748`) | unaudited |
| `overflowing_durations_rejected` (`:758`) | unaudited |

## `crates/host-runtime/tests/client.rs` - 11 tests (Managed Rust client tests)

| Test | Status |
| --- | --- |
| `authenticates_attaches_ring_routes_unary_and_closes` (`:32`) | unaudited |
| `ring_stream_and_control_traffic_share_one_live_generation` (`:63`) | unaudited |
| `ring_terminal_is_typed_redacted_and_generation_remains_usable` (`:113`) | unaudited |
| `host_terminal_codes_are_prefixed_and_local_codes_are_not` (`:152`) | unaudited |
| `request_binary_flag_reaches_the_host_in_both_states` (`:190`) | unaudited |
| `caller_cancellation_is_correlation_scoped` (`:223`) | unaudited |
| `request_deadline_is_one_absolute_owner_and_honors_overrides` (`:263`) | unaudited |
| `host_status_decodes_the_hosts_own_response_shape` (`:305`) | unaudited |
| `close_rejects_new_sends` (`:343`) | unaudited |
| `managed_client_witnesses_current_layout_maximum_bodies_and_controlled_recovery` (`:365`) | unaudited |
| `a_retained_response_stays_private_while_transport_storage_is_reused_and_after_close` (`:498`) | unaudited |

## Native and TypeScript

| Check | Status |
| --- | --- |
| `packages/shm-native/tests/mechanism.ts` raw N-API descriptor boundary, readiness dispatch, lease release and class exhaustion (21 tests under `bun test`) | unaudited |
| `packages/shm-native/tests/runtime.ts` `runNativeLifecycle` | unaudited; capability skip on Bun 1.3.14 and Node (see `real-process-current-layout-witness`) |
| `packages/opencode-plugin/src/shared/host-client/*.test.ts` (202 tests) | unaudited |

## Suspiciously quiet areas

- No test injects a detach or reference-deletion failure (F7); #550.
- No test exercises the native publisher's selection under exhaustion (F10); #550.
- No test injects a failure between descriptor duplication and grant transfer;
  the last implementation task's combined matrix.
- No test runs the two-process exchange under Valgrind; the memcheck runner
  cannot trace the child, so those witnesses run unrunnered in a separate job.
