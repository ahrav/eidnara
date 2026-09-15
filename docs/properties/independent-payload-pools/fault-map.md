# Fault-to-property map: independent payload pools

For each property, what must actually occur for a test to be non-vacuous, and
whether the harness can produce it at the tree of this catalog's introducing commit.

## Rules applied here

- Safety checks must hold while their faults are active.
- Liveness checks need a bounded fault-free window: run under load, stop the
  pressure, wait within the explicit deadline, then check.
- Coverage checks assert the independent preconditions that jointly create
  the vulnerable window. They never assert the violation.
- A skipped suite or an unreached marker is not a passing witness.

## Fault classes

| Class | Description | Available at HEAD |
| --- | --- | --- |
| F1 hostile peer slot rewrite | Store into a published descriptor slot or completion cell through the peer handle | Yes, in-process: `forged_descriptors_quarantine_before_exposing_bytes`, `stale_returns_free_nothing_and_future_completions_quarantine` (`crates/shm-transport/src/backend/ring.rs`). Not cross-process. |
| F2 doorbell loss | Close the peer's doorbell end so `send` fails or `recv` reads EOF | Yes: `wake_failure_after_publication_quarantines_but_leaves_the_frame_published`, `peer_closing_its_doorbell_quarantines_the_waiting_side` |
| F3 worker-thread return | Drop a lease on another thread while the producer is parked | Yes: `worker_final_drop_wakes_a_capacity_parked_producer_without_incoming_data`, two-process tests |
| F4 counter wrap | Place a block generation or the publication sequence at `u64::MAX` | Yes, test hooks: `set_block_generation_for_test`, `set_sequence_for_test` |
| F5 resource exhaustion | Exhaust one class, ordinary descriptor headroom, reserved depth, or one admission field | Yes: ring unit tests and `tests/profile.rs` |
| F6 wrong setup identifier or doorbell type | Present layout 3, schema 3, an eventfd, a datagram, an unconnected or non-fresh pool | Yes: `attach_rejects_eventfd_and_datagram_doorbells_and_a_second_producer`, `tests/contract.rs`, `setup_socket.rs` tests |
| F7 native detach or registration failure | Fail external-view creation, detach, or reference deletion | Partial: external-view creation failpoint exists (`napi_buffers.rs`); detach and deletion failpoints are #550 |
| F8 quarantine-accounting failure | Poison or overflow the accounting at quarantine time | Partial: `BackingAdmission::quarantine` rejects a second quarantine and `retain_uncertain` is tested; a poisoned lock is not injected |
| F9 barrier-held copy under cancel | Hold request copy work while `Cancel`, route close, or shutdown arrives | Yes: `barrier_held_copy_returns_block_and_charge_once_after_physical_completion` (`crates/host-runtime/src/ring_transport.rs`) gates a real `into_private` on the blocking barrier while every ledger closes |
| F10 publication selection under exhaustion | Ordinary exhaustion with eligible controls and terminals queued | Yes at the host: `eligible_controls_and_unrelated_terminals_publish_past_a_blocked_ordinary_ticket` empties the smallest ordinary class with held leases. Not yet at the clients: #552, #550 |

## Per-property required faults

| Property | Fault classes | Markers |
| --- | --- | --- |

| `descriptor-private-snapshot` | F1 | `pool.slot_rewritten_after_publication`, `pool.consumer_saw_published_sequence` |
| `descriptor-capacity-independent-of-payload` | F5 | `pool.ordinary_descriptors_exhausted`, `pool.payload_held_across_consumption` |
| `payload-identity-authorizes-reuse` | F1 | `pool.block_reused_with_newer_generation`, `pool.completion_cell_ahead_of_issue` |
| `released-block-reuse-preserves-held-bytes` | F3, F5 | `pool.reuse_beyond_descriptor_lap`, `pool.older_lease_live_during_reuse` |
| `class-allocation-conservation` | F5 | `pool.class_exhausted_with_larger_class_free`, `pool.reservation_aborted`, `pool.commit_underfilled`, `pool.maximum_body_reserved` |
| `completion-cell-final-owner-once` | F3 | `lease.explicit_release_then_drop`, `lease.dropped_on_worker_thread` |
| `worker-drop-forbidden-operations` | F3, F5 | `lease.drop_during_exhaustion`, `lease.drop_after_endpoint_exit` |
| `capacity-wake-progress` | F3, F5 | `pool.producer_parked_on_capacity`, `pool.transition_during_arm_window` |
| `wake-failure-preserves-published-ownership` | F2 | `pool.wake_send_failed_after_publication`, `pool.wake_would_block_token_pending` |
| `retained-mapping-lifetime` | F3 | `lease.live_after_endpoint_exit`, `lease.late_return_after_endpoint_exit` |
| `application-frame-interoperability` | F5 | `pool.maximum_frame_each_direction`, `pool.zero_body_frame` |
| `sole-identifiers-before-activation` | F6 | `setup.stale_identifier_presented`, `setup.wrong_doorbell_type_presented`, `setup.non_fresh_pool_presented` |
| `validated-setup-geometry` | F6 | `setup.grant_field_mutated`, `setup.unsealed_object_presented` |
| `authentication-transcript-boundary` | F6 | `setup.proof_mismatch_before_grant` |
| `structural-rejection-before-dispatch` | F1 | `pool.header_body_mismatch_in_block`, `pool.oversized_body_declared` |
| `shared-copy-source-access` | F1 | `lease.concurrent_same_shape_writer`, `lease.copy_at_every_alignment` |
| `owned-lease-thread-boundary` | F3 | `lease.moved_across_threads` |
| `private-decode-input-stability` | F1, F9 | `host.copy_races_peer_write` |
| `request-conversion-completion-ownership` | F9 | `host.cancel_during_barrier_held_copy` |
| `native-alias-closure-before-transfer` | F7 | `native.detach_failed`, `native.alias_survivor_before_return` |
| `partial-close-token-conservation` | F7 | `native.partial_sweep_failure`, `native.reentrant_close` |
| `environment-finalizer-confinement` | F7 | `native.environment_exit_with_aliases` |
| `response-retention-isolation` | F5 | `client.retained_response_across_close` |
| `complete-capacity-admission` | F5 | `admission.limit_one_below_charge` |
| `terminal-credit-follows-storage` | F5, F10 | `host.terminal_held_after_cancel` |
| `reserved-progress-under-data-exhaustion` | F5, F10 | `pool.ordinary_class_and_descriptors_exhausted_together`, `pool.reserved_depth_exhausted` |
| `reserved-publication-order` | F10 | `host.bypass_eligible_frame_behind_blocked_data` |
| `partial-setup-reclaims-only-unexposed-resources` | F8 | `admission.setup_failed_before_exposure`, `admission.quarantine_accounting_failed` |
| `bounded-refusal-and-recovery` | F5 | `pool.single_resource_exhausted`, `admission.field_at_limit` |
| `direct-serialization-commit-boundary` | F5 | `host.direct_frame_unreserved_on_deadline` |
| `terminal-encoding-reserve-bound` | F5 | `host.terminal_worst_case_serialized` |
| `send-outcome-no-generic-replay` | F1, F2 | `host.quarantine_between_write_and_commit` |
| `reclamation-diagnostics-meaning` | F3 | `host.generation_ended_with_live_lease` |
| `single-replacement-surface` | none (search) | `gate.sole_surface_search_run` |
| `integration-gate-dependency-selection` | none (CI) | `gate.host_runtime_only_change` |
| `unsafe-witness-selection` | none (CI) | `gate.witness_renamed` |
| `acceptance-artifact-provenance` | none (CI) | `gate.stale_artifact_present` |
| `real-process-current-layout-witness` | none (CI) | `gate.supported_runtime_present` |
| `malformed-fixture-valid-baseline` | F6 | `gate.fixture_baseline_checked` |
| `fuzz-adapter-current-contract` | none (corpus) | `gate.fuzz_corpus_replayed` |
| `capacity-model-conservation` | none (model) | `model.self_checks_run` |

## Leverage ranking

1. F1 and F5 are cheapest and cover the ownership core; every ring unit test
   constructs them without processes or threads.
2. F3 across a process boundary (two-process tests) is the only in-tree proof
   that a return from a foreign thread wakes a parked producer in another
   process; keep it out of the memcheck runner.
3. F7 and the client half of F10 need the owning tasks' seams and are the
   blocking evidence gaps for #552 and #550; F9 and the host half of F10 are
   in-process host tests that need no external process.
