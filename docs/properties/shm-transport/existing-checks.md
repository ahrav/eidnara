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

### `crates/shm-transport/tests/contract.rs` — 11 tests

| Test | Claim asserted | Status |
| --- | --- | --- |
| `descriptor_rejects_every_untrusted_identity_and_span_failure` | 8 tabled malformed descriptors map to exact errors, plus wrong incarnation, lane, and sequence | unaudited |
| `arena_plans_wrap_and_conserves_all_states` | A wrap reservation yields 2 spans, prefix shortens only the second, both conservation sums hold | unaudited |
| `lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal` | Skipped edges and late quarantine are invalid transitions; both terminal states behave as specified | unaudited — proves a model with no production driver |
| `host_admission_retains_quarantined_commitments` | Quarantine moves charges and zeroes pinned workers; a saturated controller refuses the next admit | unaudited — success path only |
| `released_admissions_recompute_active_span_charge` | Active span charge is a recomputed max, not a decrement | unaudited |
| `purity_gate_rejects_injected_copy_allocation_queue_and_wake` | Injected counters produce all six disqualification codes in order | unaudited — circular; supplies its own values |
| `debug_and_errors_redact_every_sentinel` | Debug output of descriptor, incarnation, identity, frame, and errors leaks no sentinel | unaudited |
| `sample_prefix_rejects_every_truncation_point_and_bounds_the_body` | Every truncation point rejected; trailing slack stays outside the body range | unaudited |
| `sample_prefix_rejects_identity_schema_length_and_wire_failures` | 8 tabled sample failures plus an over-declared body | unaudited |
| `frame_descriptor_rejects_span_count_and_allocation_extremes` | Span counts 0 and 3 rejected; allocation beyond arena and zero arena rejected | unaudited |
| `harness_replays_terminate_on_arbitrary_lengths` | All three harness decoders terminate across 10 lengths and 2 fills | unaudited — return values discarded; liveness only |
| `sample_errors_redact_every_sentinel` | Sample prefix and error formatting leak no sentinel | unaudited |

### `crates/shm-transport/tests/ring.rs` — 12 tests plus one ignored child helper

| Test | Claim asserted | Status |
| --- | --- | --- |
| `boundary_round_trips_include_wrap_and_exact_maximum` | Underfill and overflow publish nothing; 22 size boundaries round-trip; a full maximum frame round-trips; over-bound is rejected | unaudited |
| `retained_oldest_lease_enforces_fifo_reclamation_and_release_validation` | A retained oldest lease blocks reclamation; wrong incarnation, lane, sequence, and duplicate release map to exact errors | unaudited |
| `stale_lap_release_cannot_complete_recycled_slot` | After a full lap a stale identity is rejected and the fresh lease's bytes are intact | unaudited — single-threaded |
| `quarantine_rejects_all_operations_and_reports_conservation` | Post-quarantine reserve, receive, and release all fail; conservation reports full depth and arena as quarantined | unaudited — self-quarantine only |
| `probe_reads_shared_state_without_consuming_a_frame` | Probe leaves a published frame receivable and reports quarantine after entry | unaudited |
| `lease_limit_reports_backpressure_then_recovers_after_release` | A full lease set reads as empty rather than as an error, and recovers | unaudited — asserts only `is_none()`; cannot distinguish saturation from an empty ring |
| `one_span_profile_is_rejected_at_creation` | A single-span profile with a wrapping layout is rejected | unaudited |
| `sealed_object_prefault_repeated_setup_and_stress_conservation` (Linux) | One mapping, prefault verified, resize in both directions fails, 512 cycles end all-free | unaudited |
| `attach_rejects_unsealed_objects_and_tampered_grants` (Linux) | 5 tampered grant fields rejected, a nonzero reserved byte rejected at decode, an unsealed object rejected | unaudited |
| `grant_slice_rejects_every_truncation_point_and_one_byte_suffix` | Every truncation point and a one-byte suffix rejected; the exact length decodes | unaudited |
| `golden_grant_fixture_matches_the_frozen_ring_profile_encoding` | Corpus bytes equal a frozen hex literal and round-trip byte-exactly | unaudited — instructs a human to hand-update a third copy |
| `two_process_zero_copy_exchange_uses_authenticated_grant` (Linux) | A child process attached by descriptor reads a full maximum frame; the parent blocks behind the child's held lease | unaudited — lockstep with a sleep; cannot observe reordering |
| `ring_child_exchange` (Linux, ignored) | Child side of the above | unaudited |

### `crates/shm-transport/tests/iceoryx.rs` — 7 tests, requires the `iceoryx` feature

**Gone at `e447c927`.** `0f336d3c` deleted this suite, the `iceoryx` backend, and
the `iceoryx` Cargo feature; the removal holds at HEAD `46278f47a` after PR #131
(merge `5d638e3e8`), and the five Group K catalog records covering this backend
are `Status: invalidated`. The entry is kept as a record of what used to be
checked; everything in it resolves against `9c1eb4d1`. Covered allocation slack never reaching the decoder, stale-node observation
without disturbing a live backend, exact sequence progression, producer
rejection of oversized and underfilled commits, decoder rejection of truncation
and stale identity, schema and overflow extremes, and redaction. All unaudited.

**Correction, 2026-08-29.** An earlier revision of this inventory stated the
iceoryx suite is "not executed anywhere in CI; only `cargo check --features
iceoryx` runs". That is wrong. `iceoryx` is a *default* feature of
`shm-transport`, and the Linux CI step selects the crate by name
(`cargo nextest run -p shm-native -p shm-transport`), which enables its
default features regardless of how dependents declare
`default-features = false`. Confirmed by running `cargo nextest list` with that
exact package selection: the listing includes `shm-transport::iceoryx` with
all seven tests. So all seven execute on Linux, and none on macOS, where the step
names `--test contract --test fuzz_corpus`.

### `crates/shm-transport/tests/fuzz_corpus.rs` — 3 tests

Each replays all five seeds for one target through the production decoder and
asserts the seed named `valid` is accepted. **No seed is asserted to be
rejected.** Status unaudited; see
[negative-tests-fail-for-their-stated-reason](catalog.md#negative-tests-fail-for-their-stated-reason).

### In-crate unit tests

Inventory regenerated 2026-09-05 from every `#[test]` under
`crates/shm-transport/src` and `packages/shm-native/src`. The earlier statement
that exactly one in-crate test existed described the source tree. The "Claim
asserted" column below transcribes each test's name into a sentence; it is a
locator, not an audit, and every row is `unaudited` until a record reads the
assertion. Rows that a catalog record already cites are marked with the record.

#### `crates/shm-transport/src/backend/ring.rs` - 59 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `syscall_counters_track_only_actual_ring_syscalls` (`:2973`) | Syscall counters track only actual ring syscalls | unaudited |
| `doorbell_attachment_requires_connected_unix_stream_socket` (`:3021`) | Doorbell attachment requires connected unix stream socket | unaudited; cited by `attach-validates-doorbell-sockets` |
| `doorbell_never_blocks_after_either_end_clears_nonblock` (`:3062`) | Doorbell never blocks after either end clears nonblock | unaudited; cited by `attach-validates-doorbell-sockets` |
| `closed_peer_doorbell_fails_instead_of_blocking` (`:3088`) | Closed peer doorbell fails instead of blocking | unaudited; cited by `attach-validates-doorbell-sockets` |
| `creator_observes_peer_exit_once_the_attachment_is_handed_over` (`:3097`) | Creator observes peer exit once the attachment is handed over | unaudited |
| `quarantine_wakes_a_parked_peer` (`:3116`) | Quarantine wakes a parked peer | unaudited |
| `commit_after_quarantine_is_refused_and_aborts` (`:3134`) | Commit after quarantine is refused and aborts | unaudited; cited by `quarantine-gates-cover-every-storage-mutation` |
| `only_a_producer_handle_may_trim` (`:3150`) | Only a producer handle may trim | unaudited |
| `lengthened_released_descriptor_cannot_reclaim_a_live_frame` (`:3163`) | Lengthened released descriptor cannot reclaim a live frame | unaudited; cited by `reclaim-advance-bounded-by-the-producer-reservation` |
| `forged_active_lease_count_quarantines_on_release` (`:3190`) | Forged active lease count quarantines on release | unaudited |
| `rewound_arena_write_quarantines_instead_of_overlapping_a_live_frame` (`:3210`) | Rewound arena write quarantines instead of overlapping a live frame | unaudited |
| `rewound_published_cursor_quarantines_even_with_a_freed_slot` (`:3228`) | Rewound published cursor quarantines even with a freed slot | unaudited |
| `forged_consumer_cursors_fail_waits_instead_of_parking` (`:3247`) | Forged consumer cursors fail waits instead of parking | unaudited |
| `trim_reclaims_pending_releases_before_punching` (`:3276`) | Trim reclaims pending releases before punching | unaudited |
| `armed_wait_recheck_sees_a_quarantine_that_sent_no_token` (`:3294`) | Armed wait recheck sees a quarantine that sent no token | unaudited |
| `peer_closing_its_doorbell_quarantines_the_waiting_side` (`:3314`) | Peer closing its doorbell quarantines the waiting side | unaudited |
| `sealed_object_of_the_wrong_size_is_refused_before_mapping` (`:3344`) | Sealed object of the wrong size is refused before mapping | unaudited |
| `probe_checks_cursors_against_slot_states` (`:3378`) | Probe checks cursors against slot states | unaudited |
| `rewound_published_cursor_does_not_hide_a_queued_frame` (`:3425`) | Rewound published cursor does not hide a queued frame | unaudited |
| `attach_sets_close_on_exec_on_every_descriptor` (`:3444`) | Attach sets close on exec on every descriptor | unaudited |
| `attach_refuses_a_mapping_whose_cursors_already_break_the_protocol` (`:3478`) | Attach refuses a mapping whose cursors already break the protocol | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `probe_tolerates_every_intermediate_state_of_honest_transitions` (`:3511`) | Probe tolerates every intermediate state of honest transitions | unaudited |
| `probe_rejects_a_lease_count_more_than_one_transition_from_the_slots` (`:3553`) | Probe rejects a lease count more than one transition from the slots | unaudited |
| `attach_refuses_a_phantom_lease_count_that_a_probe_would_tolerate` (`:3573`) | Attach refuses a phantom lease count that a probe would tolerate | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `published_running_ahead_of_depth_quarantines_before_any_delivery` (`:3586`) | Published running ahead of depth quarantines before any delivery | unaudited |
| `attach_refuses_an_orphaned_receiver_slot` (`:3612`) | Attach refuses an orphaned receiver slot | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `probe_treats_receiver_slots_beyond_the_cursor_gap_as_a_fault` (`:3629`) | Probe treats receiver slots beyond the cursor gap as a fault | unaudited |
| `owned_cursor_advance_fails_closed_when_the_shared_value_moved` (`:3647`) | Owned cursor advance fails closed when the shared value moved | unaudited |
| `publication_that_raced_a_quarantine_is_not_reported_as_delivered` (`:3662`) | Publication that raced a quarantine is not reported as delivered | unaudited |
| `health_check_bounds_do_not_overflow_on_forged_cursors` (`:3682`) | Health check bounds do not overflow on forged cursors | unaudited |
| `aborted_reservation_leaves_no_resident_pages` (`:3695`) | Aborted reservation leaves no resident pages | unaudited |
| `attach_refuses_a_quarantined_ring` (`:3715`) | Attach refuses a quarantined ring | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `receive_that_raced_a_quarantine_is_not_reported_as_delivered` (`:3723`) | Receive that raced a quarantine is not reported as delivered | unaudited |
| `two_producer_reserved_slots_are_impossible` (`:3738`) | Two producer reserved slots are impossible | unaudited |
| `release_leaves_the_consumers_data_wait_armed_for_the_next_publish` (`:3759`) | Release leaves the consumers data wait armed for the next publish | unaudited |
| `attach_refuses_a_write_cursor_beyond_the_committed_frames` (`:3789`) | Attach refuses a write cursor beyond the committed frames | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `attach_refuses_a_live_slot_whose_descriptor_does_not_validate` (`:3806`) | Attach refuses a live slot whose descriptor does not validate | unaudited; cited by `attach-reconciles-or-refuses-stale-shared-cursors` |
| `descriptor_depth_above_the_cap_is_rejected_before_any_allocation` (`:3831`) | Descriptor depth above the cap is rejected before any allocation | unaudited |
| `oversized_active_lease_count_quarantines_on_receive` (`:3857`) | Oversized active lease count quarantines on receive | unaudited |
| `unaligned_arena_is_rejected_before_any_frame_flows` (`:3871`) | Unaligned arena is rejected before any frame flows | unaudited |
| `mismatched_release_identity_names_the_field_and_quarantines` (`:3902`) | Mismatched release identity names the field and quarantines | unaudited |
| `stale_lap_release_cannot_complete_recycled_slot` (`:3941`) | Stale lap release cannot complete recycled slot | unaudited |
| `shared_quarantine_flag_latches_locally_when_observed` (`:3976`) | Shared quarantine flag latches locally when observed | unaudited; cited by `quarantine-authority-survives-peer-writes` |
| `foreign_slot_state_on_reserve_is_a_fault_not_backpressure` (`:3996`) | Foreign slot state on reserve is a fault not backpressure | unaudited |
| `failed_publication_wake_leaves_the_slot_published` (`:4013`) | Failed publication wake leaves the slot published | unaudited |
| `forged_arena_write_quarantines_instead_of_underflowing` (`:4043`) | Forged arena write quarantines instead of underflowing | unaudited |
| `unaligned_batch_boundaries_do_not_strand_pages` (`:4063`) | Unaligned batch boundaries do not strand pages | unaudited |
| `residency_vector_tracks_runtime_page_size` (`:4093`) | Residency vector tracks runtime page size | unaudited |
| `removal_ranges_exclude_partial_pages_and_split_once_at_wrap` (`:4100`) | Removal ranges exclude partial pages and split once at wrap | unaudited |
| `reclaimed_pages_leave_residency_and_reuse_as_zeroes` (`:4120`) | Reclaimed pages leave residency and reuse as zeroes | unaudited |
| `subpage_releases_stay_resident_until_trim` (`:4138`) | Subpage releases stay resident until trim | unaudited |
| `partial_page_reclaim_preserves_live_neighbor` (`:4165`) | Partial page reclaim preserves live neighbor | unaudited |
| `trim_preserves_bytes_of_an_uncommitted_reservation` (`:4183`) | Trim preserves bytes of an uncommitted reservation | unaudited |
| `outstanding_reservation_is_refused_without_parking` (`:4211`) | Outstanding reservation is refused without parking | unaudited |
| `page_removal_failure_quarantines_before_capacity_publication` (`:4236`) | Page removal failure quarantines before capacity publication | unaudited |
| `quarantine_survives_peer_clearing_shared_flag` (`:4257`) | Quarantine survives peer clearing shared flag | unaudited; cited by `quarantine-authority-survives-peer-writes` |
| `impossible_slot_state_quarantines_the_receiver` (`:4274`) | Impossible slot state quarantines the receiver | unaudited |
| `forged_reclaim_length_quarantines_the_producer` (`:4292`) | Forged reclaim length quarantines the producer | unaudited |
| `wrapped_errors_preserve_sources` (`:4313`) | Wrapped errors preserve sources | unaudited |

#### `crates/shm-transport/src/lease.rs` - 3 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `failed_explicit_release_is_not_retried_by_drop` (`:329`) | Failed explicit release is not retried by drop | unaudited |
| `drop_releases_exactly_once` (`:345`) | Drop releases exactly once | unaudited |
| `volatile_copy_matches_plain_copy_at_every_offset_and_length` (`:356`) | Volatile copy matches plain copy at every offset and length | unaudited |

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

#### `packages/shm-native/src/lib.rs` - 1 test

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `channel_drops_borrowing_reservations_before_the_ring` (`:1079`) | Channel drops borrowing reservations before the ring | unaudited; cited by `addon-reservations-drop-before-the-ring` |

#### `packages/shm-native/src/scheduling.rs` - 3 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `pending_callback_waits_for_acknowledgement` (`:320`) | Pending callback waits for acknowledgement | unaudited; cited by `addon-scheduling-wakes-only-on-acknowledged-readiness` |
| `setup_socket_eof_is_reactor_readiness` (`:353`) | Setup socket eof is reactor readiness | unaudited; cited by `addon-scheduling-reaches-peer-eof-and-interrupted-wait` |
| `interrupted_wait_retries_until_success_or_close` (`:374`) | Interrupted wait retries until success or close | unaudited; cited by `addon-scheduling-wakes-only-on-acknowledged-readiness` |

#### `packages/shm-native/src/setup.rs` - 3 tests

| Test | Claim asserted (from the name) | Status |
| --- | --- | --- |
| `grant_message_accepts_tagged_setup_envelope` (`:433`) | Grant message accepts tagged setup envelope | unaudited; cited by `addon-grant-decoding-is-the-shared-setup-envelope` |
| `auth_proofs_match_committed_wire_vectors` (`:452`) | Auth proofs match committed wire vectors | unaudited; cited by `setup-proof-vectors-pin-the-shared-hmac-transcript` |
| `peer_closed_reports_live_then_dropped_sentinel` (`:479`) | Peer closed reports live then dropped sentinel | unaudited; cited by `addon-grant-decoding-is-the-shared-setup-envelope` |

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
| `packages/shm-native/tests/mechanism.ts` | Runtime mechanism gate or clean omission; cleanup hook runs at exit with empty stderr; five raw-descriptor boundary suites covering non-objects, unsafe numerics, malformed grant text, throwing accessors, wrong profile, and an unresolvable descriptor | unaudited — six suites self-skip when the addon is absent or the platform is not Linux |
| `packages/shm-native/tests/runtime.ts` | Producer aliases detached before publish; receive segment has exact bounds; transfer refused; post-release reads are zeroed; double release throws; a throwing fill publishes nothing; descriptor and arena exhaustion recover; an external-view failpoint leaves the channel usable; leaked leases survive a forced GC | unaudited |

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
   macOS if the file were in the macOS command.
2. Layout and prefault arithmetic still use a compile-time page-size constant
   while residency verification was made runtime-aware. Nothing asserts the
   layout total is a multiple of the real page size.
3. The arena padding conservation term is never produced by any production path;
   its only nonzero value is a synthetic one in a test.
4. `abort_reservation`, the sole charge-return path for commit failures, aborts,
   and drops, is infallible and silent on pointer failure.
5. Peer-originated quarantine. Nothing tests it, and no check distinguishes
   self-quarantine from peer-quarantine.
6. The process-wide attach claim has no test at any level; the commit that added
   it records that in-crate tests cannot link the addon runtime.
7. Attach is only ever exercised against a ring created in the same process, so
   the lifecycle equality check is only ever fed grants that process encoded.
8. The wire-header setter has no test, though a mismatch is exactly what commit
   validation rejects.
9. Runtime-directory revalidation is never negative-tested.
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

## Eventfd delivery checks, merge `5d638e3e8` (2026-08-31)

PR #131 replaced polling with sparse doorbell delivery and added the checks
below; in this tree the doorbells are `socketpair` ends, not eventfds. All statuses are unaudited. Two corrections to the inventory above,
both verified at HEAD:

- The "In-crate unit tests: exactly one" claim is stale. The `#[cfg(test)]`
  module in `crates/shm-transport/src/backend/ring.rs` now holds seven
  tests, and `packages/shm-native/src/scheduling.rs` holds three.
- Two `tests/ring.rs` names in the table above moved:
  `sealed_object_prefault_repeated_setup_and_stress_conservation` is now
  `sealed_sparse_object_repeated_setup_and_stress_conservation` (`:309`), and
  `attach_rejects_unsealed_objects_and_tampered_grants` is now part of
  `artifact_mismatch_fails_before_mapping_and_unsealed_objects_are_rejected`
  (`:360`). Their verdicts were not re-derived.

### `crates/shm-transport/src/backend/ring.rs` unit tests — 6 new

| Test | Claim asserted | Status |
| --- | --- | --- |
| `doorbell_attachment_requires_connected_unix_stream_socket` (`:3021`) | A regular file, an eventfd, and an unconnected `AF_UNIX` stream socket are rejected as `DoorbellFailed`; a created peer end is accepted | unaudited - exercises `Doorbell::from_fd` directly, never a full `Ring::attach` |
| `doorbell_never_blocks_after_either_end_clears_nonblock` (`:3062`) | With `O_NONBLOCK` cleared on both ends, `drain`, `signal`, a bounded `wait_until`, and a million further signals complete inside two seconds | unaudited |
| `closed_peer_doorbell_fails_instead_of_blocking` (`:3088`) | After the creating side drops, `signal` and `drain` on the attached end return `DoorbellFailed` | unaudited |
| `removal_ranges_exclude_partial_pages_and_split_once_at_wrap` (`:2279`) | Across 4/16/64 KiB pages: a sub-page run removes nothing, an unaligned run removes only its interior page, a wrapping run splits into two exact ranges | unaudited |
| `reclaimed_pages_leave_residency_and_reuse_as_zeroes` (`:2300`, Linux) | After release and reclaim, residency drops to zero and reused bytes read as zeros | unaudited |
| `repeated_subpage_releases_eventually_remove_complete_pages` (`:2319`, Linux) | Sub-page releases converge to whole-page removal at exactly the page boundary | unaudited |
| `partial_page_reclaim_preserves_live_neighbor` (`:2337`, Linux) | Reclaiming one frame leaves a live lease's bytes on the shared page intact | unaudited |
| `page_removal_failure_quarantines_before_capacity_publication` (`:2355`, Linux) | A failed `madvise` quarantines with `completed` and `arena_reclaimed` still zero | unaudited — uses the test-only `FAIL_NEXT_PAGE_REMOVAL` failpoint (`:276-283`) |

(`residency_vector_tracks_runtime_page_size`, `:2272`, predates the merge and
is inventoried above.)

### `crates/shm-transport/tests/ring.rs` — rewritten by the merge

| Test | Claim asserted | Status |
| --- | --- | --- |
| `two_process_zero_copy_exchange_uses_authenticated_grant` (`:483-533`) | Now transfers three descriptors (`[OwnedFd; 3]`, mapping plus both doorbells), blocks a `reserve_until` behind the child's held lease with a 5 s deadline, and requires 25 ms minimum elapsed | unaudited — still lockstep; the release always lands mid-block, never in the arm window |
| `ring_child_exchange` (`:537-566`) | Child attaches by descriptor, blocks in `wait_for_data` on the data doorbell, verifies payload, releases | unaudited |

### `packages/shm-native/src/scheduling.rs` unit tests — 3 new

| Test | Claim asserted | Status |
| --- | --- | --- |
| `pending_callback_waits_for_acknowledgement` (`:320`) | `wait_until_handled` does not return on a control write while `pending` holds, and returns true once cleared | unaudited |
| `setup_socket_eof_is_reactor_readiness` (`:351`) | A dropped setup peer surfaces as an epoll event with the registered channel data | unaudited |
| `interrupted_wait_retries_until_success_or_close` (`:369`) | `EINTR` retries; a set `closing` flag short-circuits to `None` | unaudited |

### `crates/host-runtime/src/client.rs` bridge tests — 2 relevant

| Test | Claim asserted | Status |
| --- | --- | --- |
| `ring_bridge_drains_inbound_and_queued_writes` (`:4003`) | Eight writes queued with no per-write wake all complete within 250 ms each after one edge; the inbound frame is not starved behind them | unaudited |
| `ring_bridge_retires_when_host_drops_setup_socket` (`:4079`) | A dropped setup socket terminates the bridge | unaudited |

### `packages/shm-native/tests/mechanism.ts` — 2 new readiness suites

| Test | Claim asserted | Status |
| --- | --- | --- |
| `one channel handler failure does not starve later channels` (`:168-205`) | A throwing handler on channel 1 does not prevent delivery on channel 2, within a 1 s bound | unaudited — self-skips when the addon is absent |
| `readiness acknowledgement preserves a frame published during callback` (`:211-278`) | A frame published inside callback 1 arrives via exactly one further callback (`callbacks === 2`, `received == [1, 2]`) | unaudited — raw addon; the `NativeChannel` wrapper path is untested for this race |

### `packages/plugin/src/shared/host-client/shm-frame-channel.test.ts`

| Test | Claim asserted | Status |
| --- | --- | --- |
| `production shared-memory delivery has no timer polling` (`:35`) | The channel source contains neither `setInterval` nor `.poll(` | unaudited — a source-text grep, not a behavioral check; it cannot see polling hidden behind a helper |

### Still quiet after the merge

1. `released-charges-wake-blocked-readers` has no check at any level: nothing
   exhausts the client read budget with the bridge parked and releases a
   charge from another thread.
2. No test lands a capacity or data signal inside the arm window
   (generation-read to poll entry); every existing wake test releases
   mid-block.
3. The concurrency-tooling verdict above is unchanged by the merge: the new
   SeqCst wake protocol has no loom, shuttle, Miri, or TSan coverage.
