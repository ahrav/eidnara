# Part 1 existing-check inventory

Every claim-bearing check that exists today for `crates/shm-transport` and
`packages/shm-native`, at `9c1eb4d1`.

An existing check does not remove a property from the catalog. A check can be
weak or vacuous, so each entry carries a status. Every status below is
**unaudited**: adequacy verdicts for tests belong to
`/testing:invariant-test-review`, and verdicts for production assertions and
runtime invariant guards belong to
`/low-level-systems:defensive-assertions-and-invariant-guards`.

## Rust integration tests

Inventory regenerated 2026-09-05 from every `#[test]` under
`crates/shm-transport/tests`. The earlier tables described the source tree's
layout: they attributed tests to `contract.rs` that now live in `profile.rs`
and `evidence.rs`, kept names `ring.rs` no longer has, and listed an
`iceoryx.rs` file that does not exist here. The "Claim asserted" column
transcribes each test's name; it is a locator, not an audit, and every row is
`unaudited` until a record reads the assertion. Ignored tests are marked.

### `crates/shm-transport/tests/contract.rs` - 11 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `descriptor_rejects_every_untrusted_identity_and_span_failure` (`:58`) | Descriptor rejects every untrusted identity and span failure | unaudited |
| `arena_plans_wrap_and_conserves_all_states` (`:220`) | Arena plans wrap and conserves all states | unaudited |
| `arena_reserve_and_prefix_report_every_failure_mode` (`:262`) | Arena reserve and prefix report every failure mode | unaudited |
| `span_accessors_return_none_past_span_count_without_panicking` (`:329`) | Span accessors return none past span count without panicking | unaudited |
| `hardware_profile_id_deserialization_enforces_constructor_rules` (`:356`) | Hardware profile id deserialization enforces constructor rules | unaudited |
| `lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal` (`:379`) | Lifecycle accepts only diagram edges and quarantine is terminal | unaudited |
| `debug_and_errors_redact_every_sentinel` (`:446`) | Debug and errors redact every sentinel | unaudited; cited by `transport-debug-output-redacts-every-sentinel` |
| `sample_prefix_rejects_every_truncation_point_and_bounds_the_body` (`:479`) | Sample prefix rejects every truncation point and bounds the body | unaudited |
| `sample_prefix_rejects_identity_schema_length_and_wire_failures` (`:528`) | Sample prefix rejects identity schema length and wire failures | unaudited |
| `frame_descriptor_rejects_span_count_and_allocation_extremes` (`:644`) | Frame descriptor rejects span count and allocation extremes | unaudited |
| `sample_errors_redact_every_sentinel` (`:714`) | Sample errors redact every sentinel | unaudited; cited by `transport-debug-output-redacts-every-sentinel` |

### `crates/shm-transport/tests/ring.rs` - 13 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `boundary_round_trips_include_wrap_and_exact_maximum` (`:41`) | Boundary round trips include wrap and exact maximum | unaudited |
| `retained_oldest_lease_enforces_fifo_reclamation` (`:124`) | Retained oldest lease enforces fifo reclamation | unaudited |
| `quarantine_rejects_all_operations_and_reports_conservation` (`:177`) | Quarantine rejects all operations and reports conservation | unaudited |
| `probe_reads_shared_state_without_consuming_a_frame` (`:201`) | Probe reads shared state without consuming a frame | unaudited |
| `lease_limit_reports_backpressure_then_recovers_after_release` (`:214`) | Lease limit reports backpressure then recovers after release | unaudited |
| `one_span_profile_is_rejected_at_creation` (`:231`) | One span profile is rejected at creation | unaudited |
| `sealed_sparse_object_repeated_setup_and_stress_conservation` (`:250`) | Sealed sparse object repeated setup and stress conservation | unaudited |
| `artifact_mismatch_fails_before_mapping_and_unsealed_objects_are_rejected` (`:306`) | Artifact mismatch fails before mapping and unsealed objects are rejected | unaudited |
| `non_regular_attachment_object_is_rejected_before_mapping` (`:390`) | Non regular attachment object is rejected before mapping | unaudited |
| `grant_slice_rejects_every_truncation_point_and_one_byte_suffix` (`:401`) | Grant slice rejects every truncation point and one byte suffix | unaudited |
| `ring_memfd_carries_the_registered_name` (`:467`) | Ring memfd carries the registered name | unaudited |
| `two_process_zero_copy_exchange_uses_authenticated_grant` (`:489`) | Two process zero copy exchange uses authenticated grant | unaudited |
| `ring_child_exchange` (`:547`) (ignored child role) | Ring child exchange | unaudited |

### `crates/shm-transport/tests/profile.rs` - 7 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `fixed_ring_identity_survives_profile_validation` (`:11`) | Fixed ring identity survives profile validation | unaudited |
| `debug_redacts_profile_admission_and_quarantine_record` (`:22`) | Debug redacts profile admission and quarantine record | unaudited; cited by `transport-debug-output-redacts-every-sentinel` |
| `host_admission_retains_quarantined_commitments` (`:50`) | Host admission retains quarantined commitments | unaudited |
| `exact_aggregate_capacity_admits_n_and_rejects_n_plus_one_without_charging` (`:85`) | Exact aggregate capacity admits n and rejects n plus one without charging | unaudited |
| `worker_limit_is_the_only_limit_that_refuses_a_second_fused_admission` (`:128`) | Worker limit is the only limit that refuses a second fused admission | unaudited |
| `released_admissions_recompute_active_span_charge` (`:168`) | Released admissions recompute active span charge | unaudited |
| `host_test_ring_profile_names_one_geometry` (`:202`) | Host test ring profile names one geometry | unaudited |

### `crates/shm-transport/tests/evidence.rs` - 3 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `purity_gate_rejects_injected_copy_allocation_queue_and_wake` (`:4`) | Purity gate rejects injected copy allocation queue and wake | unaudited |
| `purity_gate_excuses_wake_operations_only_for_a_qualified_arm_that_parked` (`:33`) | Purity gate excuses wake operations only for a qualified arm that parked | unaudited |
| `purity_gate_never_excuses_a_syscall_the_doorbell_did_not_issue` (`:78`) | Purity gate never excuses a syscall the doorbell did not issue | unaudited |

### `crates/shm-transport/tests/fuzz_corpus.rs` - 4 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `frame_descriptor_corpus_replays_without_panic` (`:76`) | Frame descriptor corpus replays without panic | unaudited |
| `provider_grant_corpus_replays_without_panic` (`:81`) | Provider grant corpus replays without panic | unaudited |
| `provider_sample_corpus_replays_without_panic` (`:86`) | Provider sample corpus replays without panic | unaudited |
| `golden_grant_fixture_matches_the_frozen_ring_profile_encoding` (`:93`) | Golden grant fixture matches the frozen ring profile encoding | unaudited |

Total: 38 integration tests across five files.

### In-crate unit tests

Inventory regenerated 2026-09-05 from every `#[test]` under
`crates/shm-transport/src` and `packages/shm-native/src`. The earlier statement
that exactly one in-crate test existed described the source tree. The "Claim
asserted" column below transcribes each test's name into a sentence; it is a
locator, not an audit, and every row is `unaudited` until a record reads the
assertion. Rows that a catalog record already cites are marked with the record.

#### `crates/shm-transport/src/backend/ring.rs` - 65 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `every_page_accessor_reads_the_initialized_zero_state` (`:2902`) | Every page accessor reads the initialized zero state | unaudited |
| `slot_index_past_depth_is_refused_before_any_dereference` (`:2959`) | Slot index past depth is refused before any dereference | unaudited |
| `descriptor_round_trips_through_the_volatile_cell` (`:2972`) | Descriptor round trips through the volatile cell | unaudited |
| `lifecycle_snapshot_sees_a_write_made_through_the_raw_page` (`:2996`) | Lifecycle snapshot sees a write made through the raw page | unaudited |
| `syscall_counters_track_only_actual_ring_syscalls` (`:3050`) | Syscall counters track only actual ring syscalls | unaudited; also cited by `trim-removes-only-dead-pages-below-the-write-cursor` |
| `doorbell_attachment_requires_connected_unix_stream_socket` (`:3096`) | Doorbell attachment requires connected unix stream socket | unaudited; cited by `attach-validates-doorbell-sockets` |
| `doorbell_never_blocks_after_either_end_clears_nonblock` (`:3137`) | Doorbell never blocks after either end clears nonblock | unaudited; cited by `attach-validates-doorbell-sockets` |
| `closed_peer_doorbell_fails_instead_of_blocking` (`:3163`) | Closed peer doorbell fails instead of blocking | unaudited; cited by `attach-validates-doorbell-sockets` |
| `creator_observes_peer_exit_once_the_attachment_is_handed_over` (`:3172`) | Creator observes peer exit once the attachment is handed over | unaudited |
| `quarantine_wakes_a_parked_peer` (`:3191`) | Quarantine wakes a parked peer | unaudited; cited by `quarantine-wakes-a-parked-waiter` |
| `commit_after_quarantine_is_refused_and_aborts` (`:3208`) | Commit after quarantine is refused and aborts | unaudited; cited by `quarantine-gates-cover-every-storage-mutation` |
| `only_a_producer_handle_may_trim` (`:3221`) | Only a producer handle may trim | unaudited; cited by `trim-removes-only-dead-pages-below-the-write-cursor` |
| `lengthened_released_descriptor_cannot_reclaim_a_live_frame` (`:3234`) | Lengthened released descriptor cannot reclaim a live frame | unaudited; cited by `reclaim-advance-bounded-by-the-producer-reservation` |
| `forged_active_lease_count_quarantines_on_release` (`:3255`) | Forged active lease count quarantines on release | unaudited |
| `rewound_arena_write_quarantines_instead_of_overlapping_a_live_frame` (`:3271`) | Rewound arena write quarantines instead of overlapping a live frame | unaudited |
| `rewound_published_cursor_quarantines_even_with_a_freed_slot` (`:3288`) | Rewound published cursor quarantines even with a freed slot | unaudited |
| `forged_consumer_cursors_fail_waits_instead_of_parking` (`:3304`) | Forged consumer cursors fail waits instead of parking | unaudited |
| `trim_reclaims_pending_releases_before_punching` (`:3329`) | Trim reclaims pending releases before punching | unaudited; cited by `trim-removes-only-dead-pages-below-the-write-cursor` |
| `armed_wait_recheck_sees_a_quarantine_that_sent_no_token` (`:3347`) | Armed wait recheck sees a quarantine that sent no token | unaudited; cited by `quarantine-wakes-a-parked-waiter` |
| `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:3369`) | Peer closing its doorbell quarantines the waiting side | unaudited; cited by `quarantine-wakes-a-parked-waiter` |
| `sealed_object_of_the_wrong_size_is_refused_before_mapping` (`:3398`) | Sealed object of the wrong size is refused before mapping | unaudited |
| `probe_checks_cursors_against_slot_states` (`:3414`) | Probe checks cursors against slot states | unaudited |
| `rewound_published_cursor_does_not_hide_a_queued_frame` (`:3456`) | Rewound published cursor does not hide a queued frame | unaudited |
| `attach_sets_close_on_exec_on_every_descriptor` (`:3474`) | Attach sets close on exec on every descriptor | unaudited; cited by `attach-makes-every-received-descriptor-close-on-exec` |
| `attach_refuses_a_mapping_whose_cursors_already_break_the_protocol` (`:3494`) | Attach refuses a mapping whose cursors already break the protocol | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `probe_tolerates_every_intermediate_state_of_honest_transitions` (`:3523`) | Probe tolerates every intermediate state of honest transitions | unaudited |
| `probe_rejects_a_lease_count_more_than_one_transition_from_the_slots` (`:3548`) | Probe rejects a lease count more than one transition from the slots | unaudited |
| `attach_refuses_a_phantom_lease_count_that_a_probe_would_tolerate` (`:3567`) | Attach refuses a phantom lease count that a probe would tolerate | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `published_running_ahead_of_depth_quarantines_before_any_delivery` (`:3579`) | Published running ahead of depth quarantines before any delivery | unaudited |
| `attach_refuses_an_orphaned_receiver_slot` (`:3603`) | Attach refuses an orphaned receiver slot | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `probe_treats_receiver_slots_beyond_the_cursor_gap_as_a_fault` (`:3616`) | Probe treats receiver slots beyond the cursor gap as a fault | unaudited |
| `owned_cursor_advance_fails_closed_when_the_shared_value_moved` (`:3633`) | Owned cursor advance fails closed when the shared value moved | unaudited |
| `publication_that_raced_a_quarantine_is_not_reported_as_delivered` (`:3648`) | Publication that raced a quarantine is not reported as delivered | unaudited |
| `health_check_bounds_do_not_overflow_on_forged_cursors` (`:3667`) | Health check bounds do not overflow on forged cursors | unaudited |
| `aborted_reservation_leaves_no_resident_pages` (`:3677`) | Aborted reservation leaves no resident pages | unaudited |
| `attach_refuses_a_quarantined_ring` (`:3697`) | Attach refuses a quarantined ring | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `receive_that_raced_a_quarantine_is_not_reported_as_delivered` (`:3705`) | Receive that raced a quarantine is not reported as delivered | unaudited |
| `two_producer_reserved_slots_are_impossible` (`:3719`) | Two producer reserved slots are impossible | unaudited |
| `release_leaves_the_consumers_data_wait_armed_for_the_next_publish` (`:3737`) | Release leaves the consumers data wait armed for the next publish | unaudited |
| `attach_refuses_a_write_cursor_beyond_the_committed_frames` (`:3766`) | Attach refuses a write cursor beyond the committed frames | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `attach_refuses_a_live_slot_whose_descriptor_does_not_validate` (`:3780`) | Attach refuses a live slot whose descriptor does not validate | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `descriptor_depth_above_the_cap_is_rejected_before_any_allocation` (`:3801`) | Descriptor depth above the cap is rejected before any allocation | unaudited |
| `oversized_active_lease_count_quarantines_on_receive` (`:3827`) | Oversized active lease count quarantines on receive | unaudited |
| `unaligned_arena_is_rejected_before_any_frame_flows` (`:3840`) | Unaligned arena is rejected before any frame flows | unaudited |
| `mismatched_release_identity_names_the_field_and_quarantines` (`:3871`) | Mismatched release identity names the field and quarantines | unaudited |
| `stale_lap_release_cannot_complete_recycled_slot` (`:3910`) | Stale lap release cannot complete recycled slot | unaudited |
| `shared_quarantine_flag_latches_locally_when_observed` (`:3942`) | Shared quarantine flag latches locally when observed | unaudited; cited by `quarantine-authority-survives-peer-writes` |
| `foreign_slot_state_on_reserve_is_a_fault_not_backpressure` (`:3960`) | Foreign slot state on reserve is a fault not backpressure | unaudited; cited by `foreign-slot-state-on-reserve-is-a-fault-not-backpressure` |
| `failed_publication_wake_leaves_the_slot_published` (`:3973`) | Failed publication wake leaves the slot published | unaudited |
| `forged_arena_write_quarantines_instead_of_underflowing` (`:3999`) | Forged arena write quarantines instead of underflowing | unaudited |
| `unaligned_batch_boundaries_do_not_strand_pages` (`:4016`) | Unaligned batch boundaries do not strand pages | unaudited |
| `residency_vector_tracks_runtime_page_size` (`:4046`) | Residency vector tracks runtime page size | unaudited |
| `removal_ranges_exclude_partial_pages_and_split_once_at_wrap` (`:4053`) | Removal ranges exclude partial pages and split once at wrap | unaudited |
| `reclaimed_pages_leave_residency_and_reuse_as_zeroes` (`:4073`) | Reclaimed pages leave residency and reuse as zeroes | unaudited |
| `subpage_releases_stay_resident_until_trim` (`:4091`) | Subpage releases stay resident until trim | unaudited |
| `partial_page_reclaim_preserves_live_neighbor` (`:4118`) | Partial page reclaim preserves live neighbor | unaudited |
| `trim_preserves_bytes_of_an_uncommitted_reservation` (`:4136`) | Trim preserves bytes of an uncommitted reservation | unaudited; cited by `trim-removes-only-dead-pages-below-the-write-cursor` |
| `outstanding_reservation_is_refused_without_parking` (`:4164`) | Outstanding reservation is refused without parking | unaudited |
| `reserve_until_deadline_leaves_the_capacity_wake_unparked` (`:4189`) | Reserve until deadline leaves the capacity wake unparked | unaudited |
| `stale_capacity_token_after_a_drain_does_not_deadlock_the_next_park` (`:4213`) | Stale capacity token after a drain does not deadlock the next park | unaudited |
| `page_removal_failure_quarantines_before_capacity_publication` (`:4243`) | Page removal failure quarantines before capacity publication | unaudited |
| `quarantine_survives_peer_clearing_shared_flag` (`:4261`) | Quarantine survives peer clearing shared flag | unaudited; cited by `quarantine-authority-survives-peer-writes`; also cited by `trim-removes-only-dead-pages-below-the-write-cursor` |
| `impossible_slot_state_quarantines_the_receiver` (`:4277`) | Impossible slot state quarantines the receiver | unaudited; cited by `foreign-slot-state-on-reserve-is-a-fault-not-backpressure` |
| `forged_reclaim_length_quarantines_the_producer` (`:4291`) | Forged reclaim length quarantines the producer | unaudited |
| `wrapped_errors_preserve_sources` (`:4309`) | Wrapped errors preserve sources | unaudited |

#### `crates/shm-transport/src/lease.rs` - 7 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `failed_explicit_release_is_not_retried_by_drop` (`:446`) | Failed explicit release is not retried by drop | unaudited |
| `drop_releases_exactly_once` (`:463`) | Drop releases exactly once | unaudited |
| `copy_in_then_copy_out_round_trips_at_every_alignment_and_length` (`:475`) | Copy in then copy out round trips at every alignment and length | unaudited |
| `access_shape_partitions_the_range_on_aligned_words` (`:509`) | Access shape partitions the range on aligned words | unaudited |
| `read_byte_agrees_with_copy_to_at_every_alignment` (`:543`) | Read byte agrees with copy to at every alignment | unaudited |
| `span_null_base_is_refused` (`:574`) | Span null base is refused | unaudited |
| `span_reads_tolerate_a_concurrent_writer` (`:581`) | Span reads tolerate a concurrent writer | unaudited |

#### `crates/shm-transport/src/profile.rs` - 2 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `cpu_list_accepts_singletons_and_ascending_ranges` (`:718`) | Cpu list accepts singletons and ascending ranges | unaudited |
| `cpu_list_rejects_every_malformed_item_rather_than_returning_a_subset` (`:728`) | Cpu list rejects every malformed item rather than returning a subset | unaudited |

#### `crates/shm-transport/src/setup_auth.rs` - 7 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `descriptor_count_matches_setup_contract` (`:152`) | Descriptor count matches setup contract | unaudited |
| `verify_proof_accepts_committed_vectors_and_rejects_every_altered_input` (`:162`) | Verify proof accepts committed vectors and rejects every altered input | unaudited |
| `verify_proof_agrees_with_compute_proof` (`:297`) | Verify proof agrees with compute proof | unaudited |
| `committed_daemon_ver_carries_the_published_prefix` (`:325`) | Committed daemon ver carries the published prefix | unaudited |
| `committed_vectors_pin_the_shared_construction` (`:331`) | Committed vectors pin the shared construction | unaudited; cited by `setup-proof-vectors-pin-the-shared-hmac-transcript` |
| `daemon_ver_is_bound_into_the_proof` (`:358`) | Daemon ver is bound into the proof | unaudited; cited by `setup-proof-vectors-pin-the-shared-hmac-transcript` |
| `domains_separate_the_two_proofs` (`:380`) | Domains separate the two proofs | unaudited |

#### `packages/shm-native/src/lib.rs` - 3 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `token_allocation_wraps_and_skips_outstanding_tokens` (`:1246`) | Token allocation wraps and skips outstanding tokens | unaudited; cited by `addon-tokens-never-collide-with-live-entries` |
| `profile_geometry_admits_the_test_profile_and_rejects_another_depth` (`:1264`) | Profile geometry admits the test profile and rejects another depth | unaudited |
| `channel_drops_borrowing_reservations_before_the_ring` (`:1282`) | Channel drops borrowing reservations before the ring | unaudited; cited by `addon-reservations-drop-before-the-ring` |

#### `packages/shm-native/src/scheduling.rs` - 4 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `pending_callback_waits_for_acknowledgement` (`:406`) | Pending callback waits for acknowledgement | unaudited; cited by `addon-scheduling-wakes-only-on-acknowledged-readiness` |
| `setup_socket_eof_is_reported_once` (`:439`) | Setup socket EOF is reported once (renamed from `setup_socket_eof_is_reactor_readiness`) | unaudited; cited by `addon-scheduling-reaches-peer-eof-and-interrupted-wait` |
| `channel_events_round_trip_and_never_collide_with_control` (`:470`) | Channel events round trip and never collide with control | unaudited |
| `interrupted_wait_retries_until_success_or_close` (`:480`) | Interrupted wait retries until success or close | unaudited; cited by `addon-scheduling-wakes-only-on-acknowledged-readiness` |

#### `packages/shm-native/src/setup.rs` - 9 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `grant_message_accepts_tagged_setup_envelope` (`:607`) | Grant message accepts tagged setup envelope | unaudited; cited by `addon-grant-decoding-is-the-shared-setup-envelope` |
| `auth_proofs_match_committed_wire_vectors` (`:626`) | Auth proofs match committed wire vectors | unaudited; cited by `setup-proof-vectors-pin-the-shared-hmac-transcript` |
| `distinct_descriptors_are_accepted_and_a_dup_is_rejected` (`:671`) | Distinct descriptors are accepted and a dup is rejected | unaudited; cited by `setup-descriptors-name-distinct-open-files` |
| `inode_fallback_rejects_the_same_aliases` (`:694`) | Inode fallback rejects the same aliases | unaudited; cited by `setup-descriptors-name-distinct-open-files` |
| `kcmp_separates_eventfds_that_share_an_inode` (`:723`) | Kcmp separates eventfds that share an inode | unaudited; cited by `setup-descriptors-name-distinct-open-files` |
| `identity_mismatch_is_distinguished_from_socket_permission_errors` (`:749`) | Identity mismatch is distinguished from socket permission errors | unaudited |
| `peer_closed_reports_live_then_dropped_sentinel` (`:761`) | Peer closed reports live then dropped sentinel | unaudited; cited by `addon-grant-decoding-is-the-shared-setup-envelope` |
| `connect_honors_the_deadline_when_the_backlog_is_full` (`:775`) | Connect honors the deadline when the backlog is full | unaudited; cited by `setup-connect-honors-its-deadline` |
| `connect_succeeds_against_an_accepting_listener` (`:821`) | Connect succeeds against an accepting listener | unaudited; cited by `setup-connect-honors-its-deadline` |

Total: 78 in-crate tests. Suspiciously quiet: no in-crate test names a
non-4096 page size, a receiver killed while holding leases, or a full
`Ring::attach` with one substituted doorbell descriptor.

## Fuzz targets

Three 11-line shims over `src/harness.rs`, so libFuzzer and the corpus replay
exercise identical code.

| Target | Input and oracle |
| --- | --- |
| `frame_descriptor` | Requires an exact 108-byte input, hand-decodes every field, calls the production validator; on accept asserts body bound, span count, span bounds, no offset overflow, and that span lengths sum to the body length. Always asserts a lane-flipped identity is rejected. |
| `provider_grant` | Decodes a grant slice; on accept asserts a byte-exact re-encode, which proves no region is ignored or defaulted. |
| `provider_sample` | Snapshots and validates a sample prefix; on accept asserts the body range starts after the prefix, is non-inverted, ends inside the allocation, and has the declared width. Always asserts a lane-flipped identity is rejected. |

Corpus: five seeds per target (`empty`, `all-zero`, `all-ff`, `valid`,
`near-valid`), 15 files. The grant `valid` seed doubles as the golden geometry
fixture.

Gaps: the harness hand-rolls the descriptor byte layout rather than decoding
through the production snapshot function, so adding a field leaves the fuzzer
exploring a stale wire shape. All three targets model immutable byte decoders
only — never the shared control pages, the slot state machine, concurrent peer
mutation, an arena larger than the minimum, or the wire header bytes that reach
the host's header validation.

**Fuzzing never runs in normal CI.** The hardening workflow is
`workflow_dispatch` only.

## TypeScript and native tests

| File | Claims asserted | Status |
| --- | --- | --- |
| `packages/shm-native/tests/capability.ts` | Channel count is zero before and after the probe; if capable, a test pair opens two channels and closes to zero; if not, construction throws and the count stays zero | unaudited — both branches print and exit 0; CI does not parse stdout |
| `packages/shm-native/tests/mechanism.ts` | Runtime mechanism gate or clean omission; cleanup hook runs at exit with empty stderr; six raw-descriptor boundary suites covering non-objects, unsafe numerics, malformed grant text, throwing accessors, wrong profile, and an unresolvable descriptor; per-test rows below | unaudited — six suites self-skip when the addon is absent; the raw-descriptor suites run on Linux and Darwin and skip elsewhere; per-test rows below |
| `packages/shm-native/tests/runtime.ts` | Producer aliases detached before publish; receive segment has exact bounds; transfer refused; post-release reads are zeroed; double release throws; a throwing fill publishes nothing; descriptor and arena exhaustion recover; an external-view failpoint leaves the channel usable; leaked leases survive a forced GC | unaudited |

### `packages/shm-native/tests/mechanism.ts` - 21 tests

Every test self-skips when the addon is absent; the raw-descriptor suites run on
Linux and Darwin and skip on every other platform.

| Test | Claim asserted | Status |
| --- | --- | --- |
| `proves every required runtime mechanism or omits capability` (`:28`) | `probeCapabilities` reports available only when every mechanism proves, otherwise a closed omission reason | unaudited; cited by `capability-probe-gates-every-advertised-mechanism` |
| `exported wire constants match the addon` (`:46`) | Exported wire constants match the addon | unaudited |
| `environment cleanup hook runs at runtime exit when addon loads` (`:58`) | The cleanup hook runs at exit with empty stderr | unaudited |
| `a dead peer ends dispatch for its channel and reports peerClosed` (`:190`) | A dead peer ends dispatch for its channel and reports peerClosed | unaudited |
| `one channel handler failure does not starve later channels` (`:224`) | A throwing readiness handler does not prevent later handlers in the same batch from running | unaudited; also cited by `each-channel-wake-survives-a-shared-acknowledgement` |
| `out-of-range u32 arguments are rejected before reaching the addon` (`:263`) | Out-of-range u32 arguments are rejected before reaching the addon | unaudited |
| `byte arguments cross into the addon as private non-shared copies` (`:272`) | Byte arguments cross into the addon as private non-shared copies | unaudited |
| `a dead peer leaves the reactor instead of redispatching forever` (`:292`) | A dead peer leaves the reactor instead of redispatching forever | unaudited |
| `a handler that throws over an undrained frame does not liveloop the event loop` (`:347`) | A handler that throws over an undrained frame does not liveloop the event loop | unaudited |
| `a dead channel's handler is pruned once the reactor drops it` (`:405`) | A dead channel's handler is pruned once the reactor drops it | unaudited |
| `a handler that takes one frame per callback is redispatched until the ring is empty` (`:469`) | A handler that takes one frame per callback is redispatched until the ring is empty | unaudited |
| `readiness acknowledgement preserves a frame published during callback` (`:527`) | A frame published while a callback is in flight is delivered by the next callback, with exactly two callbacks | unaudited; cited by `wake-published-during-readiness-callback-is-not-lost`, `reactor-callback-is-one-in-flight`, `each-channel-wake-survives-a-shared-acknowledgement` |
| `releasing a lease returns its slot; an unreleased ring fills` (`:652`) | Unreleased receive leases fill the ring until publish fails with `ring is full`; releasing one returns its slot; a stale token is refused | unaudited; cited by `lease-saturation-is-reached-then-drains` |
| `a header that disagrees with the body is refused before beforePublish runs` (`:703`) | A header that disagrees with the body is refused before beforePublish runs | unaudited |
| `a rejected commit leaves the reservation retryable and abort is idempotent` (`:733`) | A rejected commit leaves the reservation retryable and abort is idempotent | unaudited |
| `rejects non-object and structurally hostile arguments` (`:804`) | Raw `attach` rejects non-objects and hostile shapes with the fixed descriptor error and no counter change | unaudited; cited by `raw-native-attach-rejects-hostile-descriptors-without-effects` |
| `rejects every unsafe numeric representation before narrowing` (`:827`) | Negative, fractional, NaN, out-of-range, and string numerics are refused before narrowing | unaudited; cited by same record |
| `rejects malformed, non-ASCII, and aliased grant text` (`:849`) | Malformed or aliased grant text is refused with no counter change | unaudited; cited by same record |
| `accessor objects and proxies get one bounded redacted error` (`:879`) | Accessor and proxy descriptors produce exactly `invalid shared-memory descriptor` and nothing else | unaudited; cited by same record |
| `a wrong profile is refused before any attachment effect` (`:912`) | A descriptor naming another profile is refused before any registration | unaudited; cited by same record |
| `a well-formed but unresolvable descriptor fails without registry effects` (`:922`) | A structurally valid descriptor whose handles do not resolve fails without registry effects | unaudited; cited by same record |

The addon's negative tests pin channel count, external-ref count, and leak
diagnostics across each throw, which is the right shape. Two weaknesses: the
channel-count assertion reads zero by default when the addon cannot load, and
both leak counters saturate rather than overflow while the assertion only checks
equality against the pre-state.

## Production assertions and runtime guards

**Explicit `assert!` and `debug_assert!` in production paths: none found.** All
invariant enforcement is by `Result`-returning guards. The only `assert_eq!` in
`src` is inside the single unit test; `src/harness.rs` holds 12 assertions by
design as the fuzz oracle.

Guard clusters, all unaudited:

| Cluster | What it enforces |
| --- | --- |
| Mapping geometry and arithmetic | Layout overflow checks, alignment overflow, the single `ptr_at` bounds gate behind every page accessor, mmap failure, prefault verification |
| Object and runtime-directory authentication | Directory creation mode and inode identity, revalidation through the open descriptor, object owner, exact size, file type on Linux, permission bits, required seals, platform-specific object creation |
| Grant decode and geometry agreement | Reserved bytes must be zero, layout version, nonzero depth, arena floor, lease cap range, exact length, and total bytes equal to the recomputed layout; the mapped lifecycle page must equal the grant field by field |
| Profile and creation gates | Schema version, depth, arena floor, span range, lease cap, mapping floor, worker and scheduling coherence, ownership mode, charge overflow |
| Producer reservation and commit | Bound check, quarantine gate, outstanding underflow, depth exhaustion, sequence overflow, slot claim, arena exhaustion, deadline remap, abort-once, wire header agreement, commit-outside-reservation, underfill |
| Consumer receive and release | Quarantine gate, lease saturation as backpressure, empty ring, sequence overflow, slot claim, descriptor validation with quarantine on failure, two independent identity ladders, and the release compare-exchange distinguishing duplicate from invalid sequence |
| Reclamation and conservation | In-order completion walk, descriptor revalidation, strict FIFO start check, per-slot tally with overflow checks, and unknown-state rejection |
| Quarantine flag | Best-effort store, fail-closed read, probe gate |
| Decoders | 14 sequential descriptor guards, 8 sample guards, arena cursor and capacity guards, lease span bounds, lifecycle edge whitelist, evidence disqualification mapping |

Two guards are silent by construction and worth naming: `abort_reservation` is
infallible and no-ops if the slot pointer computation fails, and
`enter_quarantine` no-ops if the lifecycle pointer computation fails.

## Benchmark manifests as contracts

`crates/shm-transport/benches/manifests/v1.json` encodes gates, most of them
deliberately unfrozen: no designated host, an unset equivalence margin, unset
failure-hardening status with an empty retained-tuple list, and a selection gate
that forbids copied arms and requires the injected gate control to be
disqualified. `no_qualifying_arm_action` is to ship no shared-memory provider.

The matrix validator short-circuits on the unset status and CI runs it with
`--allow-unresolved`, so the per-tuple body has never executed against real
data. Its admission floors are restated in prose rather than derived from the
charge computation, and disagree with it in both directions.

## Suspiciously quiet areas

Code with no executed check:

1. The macOS object-creation path and the whole macOS ring path. The macOS CI
   step names `--test contract --test fuzz_corpus`, which is two of four
   integration files, and because `--test` selects integration targets it also
   excludes the lib target. Consequence, verified: **no macOS CI job ever
   constructs a `Ring`**, so `create_macos_shm` has never executed under
   observation, and the only page-size assertion in the tree does not run there
   either. Two macOS-specific fixes have no executed check. Four of the twelve
   tests in `ring.rs` are additionally Linux-gated; the other eight would run on
   macOS if the file were in the macOS command. Re-verified at HEAD
   (2026-09-05): there is no macOS path to be quiet about; `ring.rs:18-19`
   refuses every non-Linux build and the three macOS records are invalidated.
2. Layout and prefault arithmetic still use a compile-time page-size constant
   while residency verification was made runtime-aware. Nothing asserts the
   layout total is a multiple of the real page size. Re-verified at HEAD
   (2026-09-05): no longer true. `Layout::new` reads `system_page_size()`
   (`ring.rs:228`), refuses an arena that is not a multiple of it (`:231`),
   aligns every region with `align_up(.., page_size)`, and re-checks the
   offsets against the runtime page size (`:317-318`); the quiet area is now
   only that no test runs on a non-4096 host.
3. The arena padding conservation term is never produced by any production path;
   its only nonzero value is a synthetic one in a test.
4. `abort_reservation`, the sole charge-return path for commit failures, aborts,
   and drops, is infallible and silent on pointer failure.
5. Peer-originated quarantine. Nothing tests it, and no check distinguishes
   self-quarantine from peer-quarantine. Re-verified at HEAD (2026-09-05): now
   tested by `shared_quarantine_flag_latches_locally_when_observed`
   (`ring.rs:3976`), `quarantine_survives_peer_clearing_shared_flag` (`:4257`),
   and `quarantine_wakes_a_parked_peer` (`:3116`).
6. The process-wide attach claim has no test at any level; the commit that added
   it records that in-crate tests cannot link the addon runtime.
7. Attach is only ever exercised against a ring created in the same process, so
   the lifecycle equality check is only ever fed grants that process encoded.
   Re-verified at HEAD (2026-09-05): `two_process_zero_copy_exchange_uses_authenticated_grant`
   (`tests/ring.rs:489`) attaches in a child process (`ring_child_exchange`,
   `:547`) to a grant the parent encoded.
8. The wire-header setter has no test, though a mismatch is exactly what commit
   validation rejects.
9. Runtime-directory revalidation is never negative-tested. Invalidated at HEAD:
   no `RuntimeDir` exists in this tree and the covering record is
   `Status: invalidated`.
10. The iceoryx segment-growth path never executed; every test wrote tiny
    payloads. Invalidated at `e447c927`: `0f336d3c` deleted the backend, and the
    covering catalog records are `Status: invalidated`.
11. Fuzzing never runs in normal CI.
12. Three hand-synchronised copies of the ring geometry with no cross-check.
13. `docs/AUDIT-KNOWN-ISSUES.md` contains no shared-memory entries. The only
    recorded gap is the dead-peer note in the transport document, tracked as
    the source shared-memory transport task. No shared-memory bead is filed as a bug.

## Concurrency verification tooling

**None found.** No loom, shuttle, Miri, or ThreadSanitizer configuration exists
anywhere in the repository. Every memory-ordering choice in the ring backend is
currently unvalidated by any tool, and the only cross-process test is lockstep.

## Citation sweep, 2026-08-30

A citation sweep ran over this file against
`the `host` source checkout at `e447c927`. The inventory itself was
written against `9c1eb4d1` and its per-check verdicts are unchanged; only
references moved.

What changed: transport-crate line numbers were re-derived, because
`crates/shm-transport/src/backend/ring.rs`, `descriptor.rs`, `profile.rs`,
`tests/ring.rs`, `tests/contract.rs`, and `packages/shm-native/src/lib.rs` were
all edited after Part 1 was written; the `tests/iceoryx.rs` entry is marked gone,
because `0f336d3c` deleted that suite, the iceoryx backend, and the `iceoryx`
Cargo feature. No check was added, removed, or re-audited. Statuses remain
`unaudited`.

## Doorbell delivery checks (superseded inventory retired 2026-09-05)

The 2026-08-31 pass added per-merge tables for the checks PR #131 introduced in
the source tree: six new `backend/ring.rs` unit tests, a rewritten
`tests/ring.rs`, three `scheduling.rs` unit tests, two `client.rs` bridge
tests, two `mechanism.ts` readiness suites, and a `packages/plugin` suite. Those
tables described the source tree: they counted seven in-crate tests where this
tree has 59, named `repeated_subpage_releases_eventually_remove_complete_pages`,
which does not exist here, carried line numbers that no longer resolve, and
cited a `packages/plugin` directory that is not in this tree. They are retired.
The regenerated inventories above are the single source for every in-crate and
integration test; host-runtime tests are inventoried in the section below.

## Host-runtime tests that bear on transport records

Inventoried 2026-09-05. These live in `crates/host-runtime/tests` and were
outside the earlier inventory's scope, but catalog records depend on them.

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `clean_close_returns_exact_single_connection_capacity` (`shm_failure_modes.rs:203`) | A clean close returns exactly one connection's capacity | unaudited |
| `setup_active_and_idle_sigkill_each_return_exact_capacity` (`shm_failure_modes.rs:214`) | SIGKILL of a setup, active, or idle victim returns exact capacity | unaudited; cited by `dead-peer-charges-are-reclaimed-or-declared` |
| `repeated_crashes_do_not_ratchet_single_connection_capacity` (`shm_failure_modes.rs:226`) | Repeated crashes do not ratchet capacity down | unaudited |
| `exact_capacity_succeeds_and_plus_one_creates_no_ring_resources` (`shm_failure_modes.rs:258`) | Exact capacity admits; capacity plus one creates no ring resources | unaudited |
| `daemon_restart_discards_old_rings_and_accepts_fresh_client` (`shm_failure_modes.rs:303`) | A daemon restart discards old rings and accepts a fresh client | unaudited |
| `short_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded` (`shm_soak.rs:82`) | A short soak keeps fd, mapping, thread, and RSS envelopes bounded | unaudited |
| `long_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded` (`shm_soak.rs:88`) | A long soak keeps the same envelopes bounded | unaudited |
| `ring_profile_pins_per_connection_grant_geometry` (`src/ring_transport.rs:1179`) | The host profile's grant geometry is pinned | unaudited; cited by `one-profile-name-denotes-one-geometry` |
| `diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts` (`src/ring_transport.rs:1137`) | The diagnostics report carries identity, bounds, accounting, and lifecycle counts | unaudited; cited by `diagnostics-report-lifecycle-counts-in-a-fixed-shape` |

### Still quiet after the merge

1. `released-charges-wake-blocked-readers` has no check at any level: nothing
   exhausts the client read budget with the bridge parked and releases a
   charge from another thread.
2. No test lands a capacity or data signal inside the arm window
   (generation-read to poll entry); every existing wake test releases
   mid-block.
3. The concurrency-tooling verdict above is unchanged by the merge: the new
   SeqCst wake protocol has no loom, shuttle, Miri, or TSan coverage.
