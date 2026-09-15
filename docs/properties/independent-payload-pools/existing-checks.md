# Existing-check inventory: independent payload pools

Every claim-bearing test for the payload-pool transport at the tree of this catalog's introducing commit.
An existing check does not remove a property from the catalog; each entry is
`unaudited` until an invariant-test review reads its assertion.


## `crates/shm-transport/src/backend/ring.rs` - 26 tests (Ring unit and Miri tests)

| Test | Status |
| --- | --- |
| `every_page_accessor_reads_the_initialized_zero_state` (`:1929`) | unaudited |
| `slot_and_cell_indexes_past_their_regions_are_refused_before_any_dereference` (`:1984`) | unaudited |
| `descriptor_snapshot_is_copied_out_field_by_field_and_validated` (`:2004`) | unaudited |
| `completion_cell_publication_is_monotonic` (`:2031`) | unaudited |
| `lifecycle_snapshot_sees_a_write_made_through_the_raw_page` (`:2042`) | unaudited |
| `held_payload_stays_intact_while_another_block_is_reused_beyond_descriptor_laps` (`:2144`) | unaudited |
| `descriptor_consumption_frees_a_slot_while_the_payload_stays_held` (`:2179`) | unaudited |
| `every_class_backpressures_without_spill_and_returns_wake_capacity` (`:2213`) | unaudited |
| `abort_underfill_and_short_commit_conserve_blocks_and_records` (`:2242`) | unaudited |
| `zero_body_class_boundaries_maximum_and_maximum_plus_one_have_explicit_outcomes` (`:2277`) | unaudited |
| `reserved_inventories_progress_when_ordinary_descriptors_are_exhausted` (`:2319`) | unaudited |
| `forged_descriptors_quarantine_before_exposing_bytes` (`:2397`) | unaudited |
| `take_reclaimed_never_reports_a_block_reserved_since_its_return` (`:2370`) | unaudited |
| `stale_returns_free_nothing_and_future_completions_quarantine` (`:2475`) | unaudited |
| `retirement_at_a_counter_boundary_preserves_live_leases` (`:2503`) | unaudited |
| `owned_lease_outlives_both_endpoint_handles_and_returns_once` (`:2535`) | unaudited |
| `worker_final_drop_wakes_a_capacity_parked_producer_without_incoming_data` (`:2559`) | unaudited |
| `descriptor_consumption_alone_wakes_a_descriptor_parked_producer` (`:2599`) | unaudited |
| `wake_failure_after_publication_quarantines_but_leaves_the_frame_published` (`:2641`) | unaudited |
| `attach_rejects_eventfd_and_datagram_doorbells_and_a_second_producer` (`:2674`) | unaudited |
| `attach_sets_close_on_exec_on_every_descriptor` (`:2741`) | unaudited |
| `grant_round_trips_and_rejects_every_malformation` (`:2761`) | unaudited |
| `attachment_can_be_handed_to_another_thread_and_the_ring_cannot` (`:2800`) | unaudited |
| `quarantine_rejects_operations_and_survives_the_peer_clearing_the_flag` (`:2817`) | unaudited |
| `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:2844`) | unaudited |
| `forbidden_operation_observers_stay_unreached_across_a_saturated_drop_storm` (`:2856`) | unaudited |

## `crates/shm-transport/src/lease.rs` - 10 tests (Lease and copy tests (Miri))

| Test | Status |
| --- | --- |
| `owned_lease_exposes_exact_bytes_and_returns_exactly_once` (`:563`) | unaudited |
| `owned_lease_drop_returns_once_after_moving_to_another_thread` (`:592`) | unaudited |
| `stale_completion_cannot_lower_a_newer_one` (`:607`) | unaudited |
| `backing_outlives_the_last_endpoint_handle_until_the_lease_returns` (`:625`) | unaudited |
| `final_drop_reaches_no_forbidden_operation` (`:640`) | unaudited |
| `copy_in_then_copy_out_round_trips_at_every_alignment_and_length` (`:650`) | unaudited |
| `access_shape_partitions_the_range_on_aligned_words` (`:684`) | unaudited |
| `read_byte_agrees_with_copy_to_at_every_alignment` (`:718`) | unaudited |
| `span_null_base_is_refused` (`:749`) | unaudited |
| `span_reads_tolerate_a_concurrent_writer` (`:756`) | unaudited |

## `crates/shm-transport/src/pool.rs` - 5 tests (Geometry tests)

| Test | Status |
| --- | --- |
| `production_geometry_matches_the_specified_inventory` (`:526`) | unaudited |
| `placement_is_dense_and_offsets_never_overlap` (`:545`) | unaudited |
| `class_selection_takes_the_smallest_fit_without_spill` (`:563`) | unaudited |
| `geometry_rejects_every_invalid_shape` (`:598`) | unaudited |
| `layout_places_every_region_in_order_and_pads_to_pages` (`:648`) | unaudited |

## `crates/shm-transport/tests/ring.rs` - 8 tests (Real-endpoint and two-process tests)

| Test | Status |
| --- | --- |
| `production_profile_round_trips_a_maximum_frame_in_both_directions` (`:73`) | unaudited |
| `artifact_mismatch_fails_before_mapping_and_unsealed_objects_are_rejected` (`:103`) | unaudited |
| `non_regular_attachment_object_is_rejected_before_mapping` (`:180`) | unaudited |
| `ring_memfd_carries_the_registered_name` (`:191`) | unaudited |
| `two_process_exchange_holds_a_reuses_b_and_wakes_on_return` (`:317`) | unaudited |
| `ring_child_exchange` (`:410`) | unaudited |
| `two_process_descriptor_consumption_wakes_a_parked_producer_without_a_return` (`:452`) | unaudited |
| `ring_child_hold` (`:518`) | unaudited |

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

## `crates/host-runtime/src/ring_transport.rs` - 33 tests (Host transport tests)

| Test | Status |
| --- | --- |
| `process_limits_reject_counts_above_the_resident_byte_ceiling` (`:1554`) | unaudited |
| `shared_memory_workers_have_no_periodic_polling` (`:1582`) | unaudited |
| `finish_wakes_after_read_cancellation_with_unread_peer_data` (`:1592`) | unaudited |
| `a_finishing_endpoint_with_a_blocked_head_parks_instead_of_spinning` (`:1670`) | unaudited |
| `construction_has_no_ring_side_effects` (`:1753`) | unaudited |
| `diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts` (`:1761`) | unaudited |
| `grant_hex_is_strict_lowercase_ascii_without_panics` (`:1810`) | unaudited |
| `inbound_materialization_cannot_exceed_its_byte_budget` (`:1819`) | unaudited |
| `a_budget_wait_publishes_a_blocked_ticket_when_the_peer_returns_capacity` (`:1868`) | unaudited |
| `control_frame_body_is_copied_out_of_the_ring` (`:1936`) | unaudited |
| `budget_wait_observes_read_cancellation_without_retiring` (`:1993`) | unaudited |
| `budget_wait_observes_discard_without_retiring` (`:2050`) | unaudited |
| `read_cancellation_drains_frames_committed_before_it` (`:2103`) | unaudited |
| `cancellation_reports_after_one_ring_depth_under_sustained_inbound` (`:2166`) | unaudited |
| `root_cancellation_is_observed_under_sustained_inbound` (`:2234`) | unaudited |
| `root_cancellation_is_observed_while_the_inbound_queue_is_full` (`:2301`) | unaudited |
| `transport_fault_is_reported_while_the_inbound_queue_is_full` (`:2349`) | unaudited |
| `endpoint_panic_is_reported_while_the_inbound_queue_is_full` (`:2394`) | unaudited |
| `peer_close_refunds_admission_although_the_backend_quarantines_the_ring` (`:2460`) | unaudited |
| `root_cancellation_ends_a_budget_wait` (`:2521`) | unaudited |
| `a_commit_past_the_write_deadline_is_refused` (`:2567`) | unaudited |
| `a_client_send_past_its_frame_deadline_publishes_nothing` (`:2603`) | unaudited |
| `quarantined_ring_moves_its_charges_to_the_quarantined_bucket` (`:2634`) | unaudited |
| `eligible_controls_and_unrelated_terminals_publish_past_a_blocked_ordinary_ticket` (`:2728`) | unaudited |
| `an_unreserved_direct_serializer_never_runs_and_a_reserved_one_runs_once` (`:2788`) | unaudited |
| `a_terminal_credit_returns_with_its_block_not_with_settlement` (`:2840`) | unaudited |
| `a_credit_on_a_reused_block_waits_for_the_new_publication_to_return` (`:2874`) | unaudited |
| `a_pending_ticket_past_its_deadline_retires_instead_of_waiting` (`:2949`) | unaudited |
| `inventory_classification_reserves_controls_and_small_terminals_only` (`:2962`) | unaudited |
| `barrier_held_copy_returns_block_and_charge_once_after_physical_completion` (`:3009`) | unaudited |
| `refusals_are_counted_by_exhausted_resource_and_charge_nothing` (`:3131`) | unaudited |
| `ended_connections_leave_no_dead_backing_entries_without_a_status_request` (`:3176`) | unaudited |
| `return_snapshot_separates_outstanding_leases_from_released_backing` (`:3203`) | unaudited |

## `crates/host-runtime/tests/dispatch.rs` - 30 tests (Host dispatch tests)

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
| `closing_a_route_settles_its_admitted_work` (`:1103`) | unaudited |
| `concurrent_requests_never_interleave_frame_bytes` (`:1161`) | unaudited |
| `saturated_model_execution_reserve_cannot_consume_a_general_slot` (`:1240`) | unaudited |
| `saturated_general_capacity_cannot_consume_the_model_execution_reserve` (`:1362`) | unaudited |
| `work_offered_while_the_route_drains_is_joined_or_refused` (`:1458`) | unaudited |
| `detached_blocking_work_observes_route_cancellation` (`:1520`) | unaudited |
| `held_blocking_work_retains_handler_and_instance_after_fatal_close` (`:1550`) | unaudited |
| `terminal_credits_bound_admission_and_return_with_the_settled_block` (`:1595`) | unaudited |

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

## Native and TypeScript

| Check | Status |
| --- | --- |
| `packages/shm-native/tests/mechanism.ts` raw N-API descriptor boundary, readiness dispatch, lease release and class exhaustion (21 tests under `bun test`) | unaudited |
| `packages/shm-native/tests/runtime.ts` `runNativeLifecycle` | unaudited; capability skip on Bun 1.3.14 and Node (see `real-process-current-layout-witness`) |
| `packages/opencode-plugin/src/shared/host-client/*.test.ts` (202 tests) | unaudited |

## Suspiciously quiet areas

- No test injects a detach or reference-deletion failure (F7); #550.
- No test exercises client publisher selection under exhaustion (F10); #552, #550.
- No test injects a failure between descriptor duplication and grant transfer;
  the last implementation task's combined matrix.
- No test runs the two-process exchange under Valgrind; the memcheck runner
  cannot trace the child, so those witnesses run unrunnered in a separate job.
