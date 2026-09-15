# Property catalog: independent payload pools

Scope: `crates/shm-transport`, `crates/host-runtime/src/ring_transport.rs` and
its callers, `packages/shm-native`, and the TypeScript host client where a
transport property is only observable there. Domain: the payload-pool transport
that replaced the FIFO ring (specification: GitHub issue #524; implementation
tasks #546, #548, #552, #550).

Provenance: the record set is the 41 canonical slugs the Independent Payload
Pools specification (#524, "Property catalog") enumerates by acceptance section:
37 safety, 3 liveness, 1 reachability. Check semantics here are 39 `always`,
1 `unreachable` (the forbidden-operation observers), and 1 `reachable`; the
specification's `always-or-unreached` and `sometimes` records are expressed as
`always` checks whose situation markers carry the enabling-state obligation,
because every optional path they described is now unconditional in the
replacement. 64 constant situation markers and the three instrumented
forbidden-operation observers are preserved; the two further forbidden
operations are design guarantees without a code point (see below). Records were authored against the tree of this catalog's introducing commit on
branch `payload-pools/546-owned-pool-transport`; every `file:line` was resolved
by search at authoring time and re-resolved against the branch head after later
commits on the branch moved code (every reference was checked to name the same
line text, or a named replacement where the cited code changed). Read
[../METHOD.md](../METHOD.md) before changing this directory.

A documented guarantee is a claim under test. `Exercised` names the witness
that constructs the record's enabling state at HEAD; `not yet` and `partial`
records name the implementation task that owns the missing witness. Nothing
here is performance evidence; the capacity model under `capacity-model/` is
uncalibrated sizing input.

## Reachability classes

- `default-production`: reached by the shipped daemon, native addon, or clients
  with no special configuration.
- `test-only`: obligations of the verification apparatus itself (CI selection,
  fixtures, fuzz adapters, the capacity model).

No record is `explicit-config-only`.

## Forbidden-operation observers

Three code points must never execute inside a lease's final drop. They are
implemented as `unreachable` observers in `crates/shm-transport/src/lease.rs`
(`lease::observers`) and asserted by `worker-drop-forbidden-operations`:

| Observer | Forbidden operation |
| --- | --- |
| `ring_call` | any `Ring` entry point |
| `slot_wait` | parking for a descriptor slot |
| `free_list_mutation` | any free-list push or pop |

Two further forbidden operations, allocating a completion node and crossing
the N-API boundary, have no code point in this crate. The pool publishes a
completion into a fixed cell, so no node exists to allocate, and the N-API
boundary lives in the native addon. They are stated as guarantees of the
design, not observed as code points; `environment-finalizer-confinement`
covers the addon side.

## Situation markers

64 constant markers name independent enabling states. Each is listed
in its record's `Required faults and enabling state` line as `marker:<name>`.
A marker fires on a correct implementation; none asserts a violation.

## Index

| Slug | Type | Reachability | Check | Exercised |
| --- | --- | --- | --- | --- |
| `descriptor-private-snapshot` | safety | default-production | `always` | partial |
| `descriptor-capacity-independent-of-payload` | safety | default-production | `always` | yes |
| `payload-identity-authorizes-reuse` | safety | default-production | `always` | yes |
| `released-block-reuse-preserves-held-bytes` | safety | default-production | `always` | yes |
| `class-allocation-conservation` | safety | default-production | `always` | yes |
| `completion-cell-final-owner-once` | safety | default-production | `always` | yes |
| `worker-drop-forbidden-operations` | safety | default-production | `unreachable` | yes |
| `capacity-wake-progress` | liveness | default-production | `always` | yes |
| `wake-failure-preserves-published-ownership` | safety | default-production | `always` | yes |
| `retained-mapping-lifetime` | safety | default-production | `always` | yes |
| `application-frame-interoperability` | safety | default-production | `always` | yes |
| `sole-identifiers-before-activation` | safety | default-production | `always` | yes |
| `validated-setup-geometry` | safety | default-production | `always` | yes |
| `authentication-transcript-boundary` | safety | default-production | `always` | yes |
| `structural-rejection-before-dispatch` | safety | default-production | `always` | partial |
| `shared-copy-source-access` | safety | default-production | `always` | partial |
| `owned-lease-thread-boundary` | safety | default-production | `always` | yes |
| `private-decode-input-stability` | safety | default-production | `always` | yes |
| `request-conversion-completion-ownership` | safety | default-production | `always` | partial |
| `native-alias-closure-before-transfer` | safety | default-production | `always` | partial |
| `partial-close-token-conservation` | safety | default-production | `always` | partial |
| `environment-finalizer-confinement` | safety | default-production | `always` | partial |
| `response-retention-isolation` | safety | default-production | `always` | not yet |
| `complete-capacity-admission` | safety | default-production | `always` | yes |
| `terminal-credit-follows-storage` | safety | default-production | `always` | yes |
| `reserved-progress-under-data-exhaustion` | liveness | default-production | `always` | partial |
| `reserved-publication-order` | safety | default-production | `always` | partial |
| `partial-setup-reclaims-only-unexposed-resources` | safety | default-production | `always` | partial |
| `bounded-refusal-and-recovery` | liveness | default-production | `always` | yes |
| `direct-serialization-commit-boundary` | safety | default-production | `always` | yes |
| `terminal-encoding-reserve-bound` | safety | default-production | `always` | yes |
| `send-outcome-no-generic-replay` | safety | default-production | `always` | partial |
| `reclamation-diagnostics-meaning` | safety | default-production | `always` | yes |
| `single-replacement-surface` | safety | test-only | `always` | yes |
| `integration-gate-dependency-selection` | safety | test-only | `always` | yes |
| `unsafe-witness-selection` | safety | test-only | `always` | yes |
| `acceptance-artifact-provenance` | safety | test-only | `always` | partial |
| `real-process-current-layout-witness` | reachability | test-only | `reachable` | partial |
| `malformed-fixture-valid-baseline` | safety | test-only | `always` | yes |
| `fuzz-adapter-current-contract` | safety | test-only | `always` | yes |
| `capacity-model-conservation` | safety | test-only | `always` | yes |

## Ownership, framing, and progress

### descriptor-private-snapshot

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/shm-transport/src/backend/ring.rs:2147` copies the four fields out under Miri and shows a later peer rewrite changes nothing the receiver holds; `crates/shm-transport/src/backend/ring.rs:2540` forges every field through the peer handle. No test races a rewrite against the copy itself.
Guarantee: The receiver validates only a private copy of the four descriptor words taken once under the Acquire on `published`; no check rereads shared memory and no lease references the slot.
Check: `always` - after `try_receive_inner` returns, the lease's block, generation, and body length equal the values `PoolDescriptor::from_untrusted` captured, whatever the slot holds now; the semantics are `always` because the property must hold on every receive, not only when a rewrite occurs.
Fault/timing angle: The window between the four relaxed loads and the compare-exchange on `consumed`, during which a peer may rewrite the slot.
Required faults and enabling state: A peer rewrite of a slot field after publication; the consumer must have observed `published` past that slot. Markers: marker:`pool.slot_rewritten_after_publication`, marker:`pool.consumer_saw_published_sequence`.
Confidence: high - [evidence](evidence/descriptor-private-snapshot.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1453`; `crates/shm-transport/src/descriptor.rs:222`; `crates/shm-transport/src/backend/ring.rs:1501`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2147` copies the four fields out under Miri and shows a later peer rewrite changes nothing the receiver holds; `crates/shm-transport/src/backend/ring.rs:2540` forges every field through the peer handle. No test races a rewrite against the copy itself.
Impact: A receiver that rereads the slot could validate one value and lease another, exposing bytes outside the block or a body longer than the block holds.
Open questions:

- Handoff: unsafe-review for the copy-out contract; invariant-test-review for the forged-field test's discriminating power.

### descriptor-capacity-independent-of-payload

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2322` publishes past a one-slot ordinary depth after the consumer acknowledges while still holding the payload; `crates/shm-transport/tests/ring.rs:486` does the same across processes.
Guarantee: Acknowledging a descriptor makes its queue slot reusable by the producer without the payload having returned and without the request having completed (R1).
Check: `always` - immediately after `try_receive` returns a lease, the producer's `descriptors_outstanding` is one lower than before and a reservation that was `Exhausted` on descriptor headroom alone now succeeds, while `outstanding_returns` still counts the held lease.
Fault/timing angle: None; the property is structural. The interesting window is a producer parked on descriptor exhaustion when the acknowledgement arrives.
Required faults and enabling state: Ordinary descriptor headroom exhausted with every published payload still held by its lease. Markers: marker:`pool.ordinary_descriptors_exhausted`, marker:`pool.payload_held_across_consumption`.
Confidence: high - [evidence](evidence/descriptor-capacity-independent-of-payload.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1270`; `crates/shm-transport/src/backend/ring.rs:1497`; `crates/shm-transport/src/backend/retained.rs:623`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2322` publishes past a one-slot ordinary depth after the consumer acknowledges while still holding the payload; `crates/shm-transport/tests/ring.rs:486` does the same across processes.
Impact: Retained payloads would stop publication regardless of unused blocks, the FIFO coupling the replacement removes.
Open questions:

- Handoff: test-strategy for a seeded schedule that interleaves acknowledgement and return.

### payload-identity-authorizes-reuse

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2635` and `crates/shm-transport/src/backend/ring.rs:2540`; `crates/shm-transport/tests/contract.rs:118` covers the pure validator.
Guarantee: A block is reused only when its completion cell holds exactly the generation the producer issued; a stale cell frees nothing, a cell ahead of any issued generation quarantines, and no pointer or peer offset is ever consulted (KTD2).
Check: `always` - in `reclaim_completions`, every block pushed to a free list satisfies `cell == ledger.generations[block]`, and every observed `cell > generation` ends in quarantine before any free-list mutation.
Fault/timing angle: A stale return landing after the block was reused with a newer generation; a forged cell value above the issued generation.
Required faults and enabling state: A block reused at least once (generation >= 2) with a late return of the earlier generation; a cell written past the issued generation. Markers: marker:`pool.block_reused_with_newer_generation`, marker:`pool.completion_cell_ahead_of_issue`.
Confidence: high - [evidence](evidence/payload-identity-authorizes-reuse.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1171`; `crates/shm-transport/src/backend/retained.rs:597`; `crates/shm-transport/src/descriptor.rs:222`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2635` and `crates/shm-transport/src/backend/ring.rs:2540`; `crates/shm-transport/tests/contract.rs:118` covers the pure validator.
Impact: A stale or forged return could free a block a newer occupant still reads, the early-reuse stop condition.
Open questions:

- Handoff: unsafe-review for the Acquire/Release pairing on the cell.

### released-block-reuse-preserves-held-bytes

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2287` holds A for `2*depth+1` reuses of B; `crates/shm-transport/tests/ring.rs:347` repeats it across a process boundary with returns from a worker thread.
Guarantee: Releasing payload B permits reuse of B's block while an older payload A remains byte-identical and live (R2); B's reuse never touches A's block.
Check: `always` - after each B cycle, `A.to_vec()` equals the bytes captured at A's receive and no B reservation returned A's block id.
Fault/timing angle: None structural; the enabling state is reuse of a block while another block's lease is live across more than one descriptor lap.
Required faults and enabling state: At least `2 * descriptor_depth + 1` publications after A while A is held; B's block id repeats. Markers: marker:`pool.reuse_beyond_descriptor_lap`, marker:`pool.older_lease_live_during_reuse`.
Confidence: high - [evidence](evidence/released-block-reuse-preserves-held-bytes.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/pool.rs:285`; `crates/shm-transport/src/backend/ring.rs:1053`; `crates/shm-transport/src/lease.rs:308`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2287` holds A for `2*depth+1` reuses of B; `crates/shm-transport/tests/ring.rs:347` repeats it across a process boundary with returns from a worker thread.
Impact: Reuse that overlapped a live block would corrupt bytes a reader is decoding.
Open questions:

- Handoff: deterministic-simulation for a longer randomized hold/reuse schedule.

### class-allocation-conservation

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2356`, `crates/shm-transport/src/backend/ring.rs:2385`, and `crates/shm-transport/src/backend/ring.rs:2420`.
Guarantee: Each class's free, reserved, and published counts sum to its block count; an ordinary bound takes the smallest fitting class and never spills; abort and underfill return the block; a short commit retains it (KTD1).
Check: `always` - `PoolInventory::conserves` holds after every reserve, abort, commit, and reclaim, and `class_for` returns the smallest fitting class or `None`.
Fault/timing angle: Abort and underfill paths; a bound exactly at, one below, and one above each class boundary.
Required faults and enabling state: A class exhausted while a larger class has free blocks; an aborted reservation; an underfilled commit; a body of exactly `MAX_FRAME_BYTES` and of `MAX_FRAME_BYTES + 1`. Markers: marker:`pool.class_exhausted_with_larger_class_free`, marker:`pool.reservation_aborted`, marker:`pool.commit_underfilled`, marker:`pool.maximum_body_reserved`.
Confidence: high - [evidence](evidence/class-allocation-conservation.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/pool.rs:273`; `crates/shm-transport/src/backend/ring.rs:1288`; `crates/shm-transport/src/backend/ring.rs:555`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2356`, `crates/shm-transport/src/backend/ring.rs:2385`, and `crates/shm-transport/src/backend/ring.rs:2420`.
Impact: A lost or double-counted block either strands capacity forever or hands one block to two frames.
Open questions:

- Handoff: invariant-test-review for whether the inventory checks are placed after every mutation.

### completion-cell-final-owner-once

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/lease.rs:544`, `crates/shm-transport/src/lease.rs:573`, and `crates/shm-transport/src/lease.rs:588` run under Miri.
Guarantee: Each block has one completion cell; the final lease owner publishes its captured generation exactly once with Release and never lowers a newer value (KTD3).
Check: `always` - after `release` or drop, the cell holds `max(previous, generation)`, `returned` is set before publication, and a second `release` is `DuplicateRelease` with no second publication.
Fault/timing angle: Explicit release followed by drop; a late stale return after reuse; a return from another thread.
Required faults and enabling state: Explicit `release` then drop of the same lease; a lease dropped on a non-receiving thread; a stale lease returning after its block was republished. Markers: marker:`lease.explicit_release_then_drop`, marker:`lease.dropped_on_worker_thread`.
Confidence: high - [evidence](evidence/completion-cell-final-owner-once.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/lease.rs:346`; `crates/shm-transport/src/backend/retained.rs:584`; `crates/shm-transport/src/backend/retained.rs:352`.
Existing check: `crates/shm-transport/src/lease.rs:544`, `crates/shm-transport/src/lease.rs:573`, and `crates/shm-transport/src/lease.rs:588` run under Miri.
Impact: A double publication or a lowered cell could free a block twice or hide a real return.
Open questions:

- Handoff: unsafe-review for the `Send`/`Sync` justification on `Retained`.

### worker-drop-forbidden-operations

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/lease.rs:621` and `crates/shm-transport/src/backend/ring.rs:3243` assert the three observers stay unreached through saturated drops on worker threads.
Guarantee: The final drop of a lease performs no `Ring` call, allocates no completion node, waits for no slot, makes no N-API call, and mutates no free list (KTD3).
Check: `unreachable` - the three observer code points in `lease::observers` (`ring_call`, `slot_wait`, `free_list_mutation`) are never entered while `in_final_drop` is set; `unreachable` because each is a specific code location that must not execute. Completion-node allocation and the N-API boundary have no code point in this crate and are guaranteed by construction, not observed.
Fault/timing angle: Drops under class exhaustion, after endpoint exit, and from several worker threads at once.
Required faults and enabling state: Leases dropped on worker threads while the producer is `Exhausted`; leases dropped after both endpoint handles are gone. Markers: marker:`lease.drop_during_exhaustion`, marker:`lease.drop_after_endpoint_exit`.
Confidence: high - [evidence](evidence/worker-drop-forbidden-operations.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/lease.rs:378`; `crates/shm-transport/src/backend/retained.rs:586`; `crates/shm-transport/src/backend/ring.rs:1052`.
Existing check: `crates/shm-transport/src/lease.rs:621` and `crates/shm-transport/src/backend/ring.rs:3243` assert the three observers stay unreached through saturated drops on worker threads.
Impact: A drop that reached the ring or a free list would race the endpoint thread or need it alive, re-coupling lease lifetime to the endpoint.
Open questions:

- Handoff: assertion-guard owner for whether the observers should also exist in release builds.

### capacity-wake-progress

Type: liveness
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2764`, `crates/shm-transport/src/backend/ring.rs:2804`, and both two-process tests in crates/shm-transport/tests/ring.rs.
Guarantee: Both capacity transitions, descriptor acknowledgement and final payload return, wake a producer parked on the capacity doorbell within the bounded `reserve_until` deadline, without incoming data or polling (KTD3).
Check: `always` - a `reserve_until` parked on exhaustion returns `Ok` before its deadline once either transition happens, with `parks >= 1` in `syscall_counters`; bounded by the test deadline, never an open-ended eventually.
Fault/timing angle: Return before arm, return after arm, and a coalesced token covering both transitions.
Required faults and enabling state: Producer parked (`parked != 0`) when the transition happens; a transition landing between `try_reserve` and `ParkGuard::arm`. Markers: marker:`pool.producer_parked_on_capacity`, marker:`pool.transition_during_arm_window`.
Confidence: high - [evidence](evidence/capacity-wake-progress.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1111`; `crates/shm-transport/src/backend/retained.rs:623`; `crates/shm-transport/src/backend/ring.rs:95`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2764`, `crates/shm-transport/src/backend/ring.rs:2804`, and both two-process tests in crates/shm-transport/tests/ring.rs.
Impact: A lost wake leaves the producer parked to its deadline although capacity exists.
Open questions:

- Handoff: deterministic-simulation for controlled interleavings at the arm window.

### wake-failure-preserves-published-ownership

Type: safety
Reachability: default-production
Status: active
Exercised: yes - publish side: `wake_failure_after_publication_quarantines_but_leaves_the_frame_published`; consumption side: `a_failed_consumption_wake_quarantines_the_consumer_and_returns_the_block`; return side: `a_failed_return_wake_reports_wake_failed_and_keeps_the_completion`, which arms `parked`, closes the producer's doorbell end, asserts `WakeFailed` from `release`, and reads the completion cell and return flag back. All three are in `crates/shm-transport/src/backend/ring.rs`.
Guarantee: A doorbell failure after publication or consumption quarantines the handle that rang it, or surfaces as `WakeFailed` to a lease's explicit `release` caller, but never rolls back the published descriptor, the consumption, or the completion; `WouldBlock` is success (KTD3).
Check: `always` - after a failed wake, `published` still holds the new sequence or the completion cell still holds the generation, and no free-list mutation followed the failure.
Fault/timing angle: Peer doorbell end closed before the wake; a full socket buffer (`WouldBlock`).
Required faults and enabling state: `parked` set with the peer's doorbell end closed so `send` fails with `EPIPE`. Markers: marker:`pool.wake_send_failed_after_publication`, marker:`pool.wake_would_block_token_pending`.
Confidence: high - [evidence](evidence/wake-failure-preserves-published-ownership.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1365`; `crates/shm-transport/src/backend/retained.rs:628`; `crates/shm-transport/src/backend/retained.rs:644`.
Existing check: publish side: `wake_failure_after_publication_quarantines_but_leaves_the_frame_published`; consumption side: `a_failed_consumption_wake_quarantines_the_consumer_and_returns_the_block`; return side: `a_failed_return_wake_reports_wake_failed_and_keeps_the_completion`, which arms `parked`, closes the producer's doorbell end, asserts `WakeFailed` from `release`, and reads the completion cell and return flag back. All three are in `crates/shm-transport/src/backend/ring.rs`.
Impact: Rolling back a publication the peer may already hold would reuse bytes a reader is decoding.
Open questions:

- Handoff: none.

### retained-mapping-lifetime

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/lease.rs:606` (Miri) and `crates/shm-transport/src/backend/ring.rs:2740`.
Guarantee: Backing, completion cells, wake handle, and the backing charge stay alive while any lease exists; the mapping unmaps only when the last holder drops; endpoint exit refunds worker count only (KTD5).
Check: `always` - with a lease live, `Weak::upgrade` on the backing succeeds after both `Ring` handles drop and `to_vec` returns the original bytes; after the lease drops, the upgrade fails.
Fault/timing angle: Endpoint close and reconnect with a reader still holding a lease.
Required faults and enabling state: Both endpoint handles dropped while a lease is live; a late return after the drop. Markers: marker:`lease.live_after_endpoint_exit`, marker:`lease.late_return_after_endpoint_exit`.
Confidence: high - [evidence](evidence/retained-mapping-lifetime.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/lease.rs:248`; `crates/shm-transport/src/backend/retained.rs:270`; `crates/host-runtime/src/ring_transport.rs:427`.
Existing check: `crates/shm-transport/src/lease.rs:606` (Miri) and `crates/shm-transport/src/backend/ring.rs:2740`.
Impact: Unmapping under a reader is a use-after-unmap; refunding its charge early lets admission oversubscribe.
Open questions:

- Handoff: unsafe-review for the `Arc<Retained>` ownership argument.

### application-frame-interoperability

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/tests/ring.rs:74` and `crates/shm-transport/src/backend/ring.rs:2420`; application vectors stay frozen in `crates/host-runtime/tests/protocol_vectors.rs`.
Guarantee: The 21-byte header, JSON bodies, direct serializers, send outcomes, and the exact 67,108,864-byte maximum are unchanged; both directions accept one maximum frame on an admitted connection and refuse maximum-plus-one (R7).
Check: `always` - every published body of length `n <= MAX_FRAME_BYTES` is received with identical bytes and header, and `MAX_FRAME_BYTES + 1` is `BoundExceedsClass` before any block is taken.
Fault/timing angle: None; boundary values are the enabling state.
Required faults and enabling state: A body of exactly `MAX_FRAME_BYTES` in each direction on an otherwise empty connection. Markers: marker:`pool.maximum_frame_each_direction`, marker:`pool.zero_body_frame`.
Confidence: high - [evidence](evidence/application-frame-interoperability.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/pool.rs:310`; `crates/shm-transport/src/profile.rs:676`; `crates/shm-transport/src/descriptor.rs:26`.
Existing check: `crates/shm-transport/tests/ring.rs:74` and `crates/shm-transport/src/backend/ring.rs:2420`; application vectors stay frozen in `crates/host-runtime/tests/protocol_vectors.rs`.
Impact: A smaller effective maximum would break the interoperability promise of the wire contract.
Open questions:

- Handoff: none.

### sole-identifiers-before-activation

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/tests/contract.rs:167`, `crates/shm-transport/src/backend/ring.rs:3061`, `crates/shm-transport/tests/profile.rs:261`, and `stale_wire_or_descriptor_schema_is_invalid_identity` in `crates/host-runtime/src/setup_socket.rs`.
Guarantee: Only descriptor schema 4, layout version 4, and profile `host-payload-pool-v1` are accepted; any other identifier, an eventfd or datagram doorbell, or a pool with traffic in flight fails before application traffic (R8, KTD8).
Check: `always` - every mismatch path returns an error before `Mapping::attach` or before `activate` commits, and no code path decodes another layout.
Fault/timing angle: None; this is fail-closed identity checking.
Required faults and enabling state: A grant with layout version 3; a setup message with schema 3; an eventfd in a doorbell position; a non-fresh pool at attach. Markers: marker:`setup.stale_identifier_presented`, marker:`setup.wrong_doorbell_type_presented`, marker:`setup.non_fresh_pool_presented`.
Confidence: high - [evidence](evidence/sole-identifiers-before-activation.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:190`; `crates/shm-transport/src/backend/ring.rs:726`; `crates/host-runtime/src/ring_transport.rs:1483`; `packages/shm-native/src/lib.rs:305`.
Existing check: `crates/shm-transport/tests/contract.rs:167`, `crates/shm-transport/src/backend/ring.rs:3061`, `crates/shm-transport/tests/profile.rs:261`, and `stale_wire_or_descriptor_schema_is_invalid_identity` in `crates/host-runtime/src/setup_socket.rs`.
Impact: An accepted stale identifier would decode another layout's bytes as this one's.
Open questions:

- Handoff: none.

### validated-setup-geometry

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:3148`, `crates/shm-transport/tests/ring.rs:104`, `crates/shm-transport/src/pool.rs:623`, and `crates/shm-transport/tests/fuzz_corpus.rs:92`.
Guarantee: Every block offset and capacity comes from geometry both peers validated; the grant's total must equal the computed layout; unsealed, resized, or non-regular objects are refused before mapping (KTD2).
Check: `always` - `PoolGrant::decode` accepts only a geometry `PoolGeometry::new` accepts whose layout total matches, and `Ring::attach` refuses a mapping whose lifecycle page disagrees with the grant in any field.
Fault/timing angle: Mutated grant fields; an unsealed or wrongly sized object; a lifecycle page that disagrees with the grant.
Required faults and enabling state: Each grant field mutated one at a time; an unsealed memfd of the right size; a lifecycle lane rewritten after creation. Markers: marker:`setup.grant_field_mutated`, marker:`setup.unsealed_object_presented`.
Confidence: high - [evidence](evidence/validated-setup-geometry.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:341`; `crates/shm-transport/src/backend/retained.rs:761`; `crates/shm-transport/src/backend/retained.rs:293`.
Existing check: `crates/shm-transport/src/backend/ring.rs:3148`, `crates/shm-transport/tests/ring.rs:104`, `crates/shm-transport/src/pool.rs:623`, and `crates/shm-transport/tests/fuzz_corpus.rs:92`.
Impact: A peer-supplied offset or size would let a forged descriptor address bytes outside its block.
Open questions:

- Handoff: none.

### authentication-transcript-boundary

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/setup_auth.rs` vector tests and `crates/host-runtime/src/setup_socket.rs` activation tests; carried forward unchanged from the host-runtime setup-identity catalog.
Guarantee: Grants are decoded only after the authenticated transcript verifies, and the grant carries no profile id of its own: the profile is checked by the setup layer before any grant byte is interpreted.
Check: `always` - `begin_connect` and `run_connection` verify the proof before `receive_grant`, and `PoolGrant` has no profile field.
Fault/timing angle: None new; the transcript is unchanged by this replacement.
Required faults and enabling state: A proof mismatch before the grant message. Markers: marker:`setup.proof_mismatch_before_grant`.
Confidence: medium - [evidence](evidence/authentication-transcript-boundary.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/src/setup.rs:116`; `crates/shm-transport/src/backend/ring.rs:306`.
Existing check: `crates/shm-transport/src/setup_auth.rs` vector tests and `crates/host-runtime/src/setup_socket.rs` activation tests; carried forward unchanged from the host-runtime setup-identity catalog.
Impact: A grant decoded before authentication would let an unauthenticated peer drive geometry validation.
Open questions:

- Handoff: none.

### structural-rejection-before-dispatch

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/shm-transport/src/backend/ring.rs:2540` covers descriptor and header structure; host header validation stays in `validate_inbound_header` tests in `crates/host-runtime/src/frame_channel.rs`.
Guarantee: A structurally illegal descriptor, header, or body length closes the generation before any application dispatch and exposes no byte.
Check: `always` - `try_receive` returns `Err` and quarantines for every validation failure, and `receive_one` maps a header failure to `ReadClose::Corrupt` before delivering an `InboundEvent`.
Fault/timing angle: Header/body mismatch written into the block; oversized declared length.
Required faults and enabling state: A block header whose declared length differs from the descriptor's body length. Markers: marker:`pool.header_body_mismatch_in_block`, marker:`pool.oversized_body_declared`.
Confidence: high - [evidence](evidence/structural-rejection-before-dispatch.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1487`; `crates/host-runtime/src/ring_transport.rs:1003`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2540` covers descriptor and header structure; host header validation stays in `validate_inbound_header` tests in `crates/host-runtime/src/frame_channel.rs`.
Impact: Dispatching a structurally illegal frame would let a peer steer application decoding with unchecked lengths.
Open questions:

- Handoff: none.

## Runtime and native lifetime

### shared-copy-source-access

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/shm-transport/src/lease.rs:631` and `crates/shm-transport/src/lease.rs:737` under Miri prove same-shape access; cross-process hostile writers are not provable here (recorded limitation).
Guarantee: Every access to shared bytes is a fixed-width atomic or an `AccessShape` copy; no `&[u8]` over the arena exists; a copy stabilizes destination bytes only, and consumers decode from private copies (R6, KTD4).
Check: `always` - `rg` over `crates/shm-transport/src` finds no `slice::from_raw_parts` over arena memory, and every raw pointer escape is one of `ProducerReservation::segment` and `PayloadLease::body`.
Fault/timing angle: A same-shape concurrent writer; a shifted overlapping writer is outside the contract and documented.
Required faults and enabling state: A writer thread storing through `copy_in` while a span is read. Markers: marker:`lease.concurrent_same_shape_writer`, marker:`lease.copy_at_every_alignment`.
Confidence: medium - [evidence](evidence/shared-copy-source-access.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/lease.rs:142`; `crates/shm-transport/src/lease.rs:188`; docs/payload-pool-protocol.md section 10.
Existing check: `crates/shm-transport/src/lease.rs:631` and `crates/shm-transport/src/lease.rs:737` under Miri prove same-shape access; cross-process hostile writers are not provable here (recorded limitation).
Impact: A Rust slice over peer-writable memory is undefined behavior the moment the peer writes.
Open questions:

- Handoff: safe-over-unsafe then unsafe-review for the shifted-overlap and JavaScript-writer exclusions.

### owned-lease-thread-boundary

Type: safety
Reachability: default-production
Status: active
Exercised: yes - compile-time: the `compile_fail` doctests at the top of crates/shm-transport/src/backend/ring.rs and the `assert_send::<PayloadLease>` in crates/shm-transport/src/lease.rs; runtime: `crates/shm-transport/src/lease.rs:573` and `crates/shm-transport/src/backend/ring.rs:3187`.
Guarantee: `PayloadLease` is `Send`; `Ring`, `ProducerReservation`, and `LeaseSpan` are `!Send`; `Retained` is `Send + Sync` by explicit justification (R5, KTD4).
Check: `always` - the positive and negative `Send` assertions compile as written; no other `unsafe impl Send/Sync` exists in the crate.
Fault/timing angle: None; type-level.
Required faults and enabling state: A build of the doctests. Markers: marker:`lease.moved_across_threads`.
Confidence: high - [evidence](evidence/owned-lease-thread-boundary.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:25`; `crates/shm-transport/src/backend/retained.rs:354`; `crates/shm-transport/src/lease.rs:21`.
Existing check: compile-time: the `compile_fail` doctests at the top of crates/shm-transport/src/backend/ring.rs and the `assert_send::<PayloadLease>` in crates/shm-transport/src/lease.rs; runtime: `crates/shm-transport/src/lease.rs:573` and `crates/shm-transport/src/backend/ring.rs:3187`.
Impact: A `Send` `Ring` would let two threads drive one endpoint's non-atomic ledger.
Open questions:

- Handoff: unsafe-review.

### private-decode-input-stability

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/host-runtime/src/ring_transport.rs:2336` shows the ring slot released once the body is private; `InboundFrame::into_private` (`crates/host-runtime/src/frame_channel.rs:107`) copies, releases the lease, then checks the copied length against the header, and `decode_control_frame` (`crates/host-runtime/src/connection.rs:532`) parses channel-0 bodies only from that private copy. Oversized channel-0 requests are refused before any lease (`crates/host-runtime/src/ring_transport.rs:1004`).
Guarantee: Rust decoding reads only stable private bytes: routed and channel-0 bodies are copied out of the lease before any parser sees them, and the lease is released after the last copy (KTD4).
Check: `always` - no `InboundFrame` or control decoder holds a `LeaseSpan`; `lease.release()` precedes `deliver`.
Fault/timing angle: A peer rewriting a published block during the copy.
Required faults and enabling state: A copy racing a peer write of the same block. Markers: marker:`host.copy_races_peer_write`.
Confidence: high - [evidence](evidence/private-decode-input-stability.md). Verified against the tree of this catalog's introducing commit: `crates/host-runtime/src/frame_channel.rs:107`; `crates/host-runtime/src/connection.rs:532`; `crates/host-runtime/src/dispatch.rs:993`.
Existing check: `crates/host-runtime/src/ring_transport.rs:2336` shows the ring slot released once the body is private; `InboundFrame::into_private` (`crates/host-runtime/src/frame_channel.rs:107`) copies, releases the lease, then checks the copied length against the header, and `decode_control_frame` (`crates/host-runtime/src/connection.rs:532`) parses channel-0 bodies only from that private copy. Oversized channel-0 requests are refused before any lease (`crates/host-runtime/src/ring_transport.rs:1004`).
Impact: Decoding shared bytes would let a peer change a message under the parser.
Open questions:

- Handoff: #552 and #550 for client-side decoding of host output.

### request-conversion-completion-ownership

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/host-runtime/src/ring_transport.rs:3544` holds a real `into_private` copy on the blocking barrier while the request, route, and host ledgers close, and shows `outstanding_returns` and the ingress charge unchanged until the copy joins, then each returned once. `crates/host-runtime/tests/dispatch.rs:749` and `crates/host-runtime/tests/dispatch.rs:805` drive the production Cancel and route-close paths against handler blocking work (`blocking_hold`), which starts after `dispatch_request` has already completed the inbound copy; no test pauses the production copy itself under Cancel, route close, or shutdown.
Guarantee: Cancellation, route close, and shutdown cannot return a block or its charge before the barrier-held copy/decode physically completes.
Check: `always` - a lease moved into blocking work returns only after that work joins; `Cancel` observed mid-copy leaves `outstanding_returns` unchanged until the join. A pure-header body copies inline in `dispatch_request` (`crates/host-runtime/src/dispatch.rs:993`) with no worker and nothing to read, so no window exists there; a closed route still settles it as cancelled.
Fault/timing angle: Cancel, route close, and shutdown during copy.
Required faults and enabling state: A barrier holding copy work while `Cancel` arrives. Markers: marker:`host.cancel_during_barrier_held_copy`.
Confidence: high - [evidence](evidence/request-conversion-completion-ownership.md). Verified against the tree of this catalog's introducing commit: `crates/host-runtime/src/handler.rs:617`; `crates/host-runtime/src/dispatch.rs:993`.
Existing check: `crates/host-runtime/src/ring_transport.rs:3544` holds a real `into_private` copy on the blocking barrier while the request, route, and host ledgers close, and shows `outstanding_returns` and the ingress charge unchanged until the copy joins, then each returned once. `crates/host-runtime/tests/dispatch.rs:749` and `crates/host-runtime/tests/dispatch.rs:805` drive the production Cancel and route-close paths against handler blocking work (`blocking_hold`), which starts after `dispatch_request` has already completed the inbound copy; no test pauses the production copy itself under Cancel, route close, or shutdown.
Impact: An early return would reuse a block a worker is still copying.
Open questions:

- Handoff: none for this task.

### native-alias-closure-before-transfer

Type: safety
Reachability: default-production
Status: active
Exercised: partial - producer aliases detach before commit (`packages/shm-native/src/lib.rs:380`) and consumer aliases detach before return (`packages/shm-native/src/lib.rs:353`); `runNativeLifecycle` in packages/shm-native/tests/runtime.ts asserts subarray, DataView, and Buffer aliases read zero after release, but only when the runtime reports the detachment capability.
Guarantee: Every JavaScript alias of a block detaches before the block is published or returned; a failed detach quarantines the direction and retains the alias record (R9, KTD6).
Check: `always` - `commit_reservation` and `release` reach the ring only after `detach_all` succeeded; a detach failure calls `enter_quarantine` and keeps the entry.
Fault/timing angle: `napi_detach_arraybuffer` failure; alias survivors through `subarray`/`DataView`.
Required faults and enabling state: An external-view failpoint firing on detach; a `subarray` created before release. Markers: marker:`native.detach_failed`, marker:`native.alias_survivor_before_return`.
Confidence: medium - [evidence](evidence/native-alias-closure-before-transfer.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/src/lib.rs:353`; `packages/shm-native/src/napi_buffers.rs:142`; `packages/shm-native/tests/runtime.ts:146`.
Existing check: producer aliases detach before commit (`packages/shm-native/src/lib.rs:380`) and consumer aliases detach before return (`packages/shm-native/src/lib.rs:353`); `runNativeLifecycle` in packages/shm-native/tests/runtime.ts asserts subarray, DataView, and Buffer aliases read zero after release, but only when the runtime reports the detachment capability.
Impact: A live alias after return would let JavaScript write a block the peer has reused.
Open questions:

- Handoff: #550 owns the detach-failure injection witnesses; the Bun 1.3.14 `markAsUntransferable` gap is a recorded unsupported capability.

### partial-close-token-conservation

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `packages/shm-native/src/lib.rs:407` sweeps every alias and reports the first failure; `finish_close` retains alias-holding channels; mechanism tests in packages/shm-native/tests/mechanism.ts cover repeated release and close.
Guarantee: Partial close sweeps, reentrant callbacks, repeated release/close, and environment termination conserve tokens and one-shot return authority; uncertain aliases quarantine the owning backing once.
Check: `always` - every token is released or retained exactly once across a partial sweep, and a channel with any alias outstanding is never removed from the registry.
Fault/timing angle: A detach failure mid-sweep; a callback that closes the channel reentrantly.
Required faults and enabling state: Injected detach failure on the second of three aliases; a `deliver` callback calling `close`. Markers: marker:`native.partial_sweep_failure`, marker:`native.reentrant_close`.
Confidence: medium - [evidence](evidence/partial-close-token-conservation.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/src/lib.rs:407`; `packages/shm-native/src/lib.rs:1656`.
Existing check: `packages/shm-native/src/lib.rs:407` sweeps every alias and reports the first failure; `finish_close` retains alias-holding channels; mechanism tests in packages/shm-native/tests/mechanism.ts cover repeated release and close.
Impact: A lost token strands a block; a double return frees a newer occupant.
Open questions:

- Handoff: #550.

### environment-finalizer-confinement

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `packages/shm-native/src/lib.rs:469` closes channels on the environment cleanup hook and `mem::forget`s alias-holding channels; the owned lease's drop is the only finalizer-adjacent return and reaches no N-API (`crates/shm-transport/src/backend/retained.rs:584`).
Guarantee: Finalizers and cleanup hooks own only their declared context: no ring call, allocator mismatch, unwind across C, or arbitrary N-API; uncertain cleanup quarantines rather than unmapping (KTD6).
Check: `always` - `cleanup_env` never calls `Ring` methods other than `enter_quarantine`, and a final drop never reaches the N-API boundary; no observer instruments that boundary in `shm-transport`, so the check is on the addon's detach-before-return path.
Fault/timing angle: Environment teardown with aliases outstanding.
Required faults and enabling state: An environment exit while a channel holds a stranded alias. Markers: marker:`native.environment_exit_with_aliases`.
Confidence: medium - [evidence](evidence/environment-finalizer-confinement.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/src/lib.rs:493`; `crates/shm-transport/src/lease.rs:378`.
Existing check: `packages/shm-native/src/lib.rs:469` closes channels on the environment cleanup hook and `mem::forget`s alias-holding channels; the owned lease's drop is the only finalizer-adjacent return and reaches no N-API (`crates/shm-transport/src/backend/retained.rs:584`).
Impact: A finalizer calling N-API off-thread or unmapping under an alias is a crash or a use-after-unmap.
Open questions:

- Handoff: #550 for late-finalizer witnesses.

### response-retention-isolation

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - retained binary unary quotas and stream private copies belong to #550 (native/TypeScript) and #552 (Rust client); the transport side is proved by `released-block-reuse-preserves-held-bytes`.
Guarantee: A retained response A never pins B's block; retained binary responses consume a separate per-connection quota until native release; stream items are private copies.
Check: `always` - holding A leaves `descriptors_outstanding` and B's class free count unaffected by A; retained-quota refusals recover independently.
Fault/timing angle: Retention across close and reconnect.
Required faults and enabling state: A retained after its connection closes while B cycles. Markers: marker:`client.retained_response_across_close`.
Confidence: low - [evidence](evidence/response-retention-isolation.md). Verified against the tree of this catalog's introducing commit: `packages/opencode-plugin/src/shared/host-client/connection.ts:1057`; `packages/opencode-plugin/src/shared/host-client/connection.ts:1088`.
Existing check: retained binary unary quotas and stream private copies belong to #550 (native/TypeScript) and #552 (Rust client); the transport side is proved by `released-block-reuse-preserves-held-bytes`.
Impact: Retention that pinned unrelated storage would reintroduce the FIFO coupling at the client layer.
Open questions:

- Handoff: #550 and #552.

## Reserves, accounting, and publication

### complete-capacity-admission

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/tests/profile.rs:261` checks the charge equals the created object size; `crates/shm-transport/tests/profile.rs:131` and `process_limits_reject_counts_above_the_resident_byte_ceiling` in crates/host-runtime/src/ring_transport.rs. On the host side, `HostLimits::checked_aggregate` (`crates/host-runtime/src/config.rs:209`) states transport, resident, and terminal ceilings as distinct checked quantities and `crates/host-runtime/src/config.rs:615` refuses an unstatable total; `host.status` exposes the aggregate (`crates/host-runtime/src/connection.rs:643`).
Guarantee: Full capacity is charged before activation from the complete layout: mapping bytes, ledger bytes, descriptors, blocks, mappings, file descriptors, wake handles, and the instance; `affordable_connections` divides the byte ceiling by the complete committed charge (KTD7).
Check: `always` - `charges().mapping_bytes == 2 * Ring::object_size()` and `ledger_bytes == 2 * ledger_bytes(geometry)` for the production profile, and every `HostLimits` field is checked in field order.
Fault/timing angle: Each limit one below the requested charge.
Required faults and enabling state: A limit tightened one unit below one connection's charge, per field. Markers: marker:`admission.limit_one_below_charge`.
Confidence: high - [evidence](evidence/complete-capacity-admission.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/profile.rs:166`; `crates/host-runtime/src/ring_transport.rs:72`; `crates/host-runtime/src/config.rs:209`; `crates/host-runtime/src/ring_transport.rs:1199`.
Existing check: `crates/shm-transport/tests/profile.rs:261` checks the charge equals the created object size; `crates/shm-transport/tests/profile.rs:131` and `process_limits_reject_counts_above_the_resident_byte_ceiling` in crates/host-runtime/src/ring_transport.rs. On the host side, `HostLimits::checked_aggregate` (`crates/host-runtime/src/config.rs:209`) states transport, resident, and terminal ceilings as distinct checked quantities and `crates/host-runtime/src/config.rs:615` refuses an unstatable total; `host.status` exposes the aggregate (`crates/host-runtime/src/connection.rs:643`).
Impact: An under-charged connection oversubscribes the process ceiling.
Open questions:

- Handoff: #552 for Rust-client retention; #550 for native/TypeScript retention.

### terminal-credit-follows-storage

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/host-runtime/src/ring_transport.rs:3375` publishes a terminal carrying a credit and shows the credit outstanding until `Ring::take_reclaimed` observes the block's return; `crates/host-runtime/tests/dispatch.rs:1649` admits 63 unsettled requests, refuses the 64th with `server_busy`/`terminal capacity exhausted` and zero dispatch while pending slots remain, then dispatches again only after the cancelled terminal's block is consumed.
Guarantee: One terminal credit is reserved before request admission and released only when its block returns or its generation retires, not when a callback completes.
Check: `always` - the count of admitted requests never exceeds terminal credits, and a credit is released exactly once at the physical return point.
Fault/timing angle: Cancellation and foreign retention of a terminal block; a peer return that lands between one pump's settlement scan and the same pump's reservation, so the next terminal reuses the block.
Required faults and enabling state: A cancelled request whose terminal block is still held by the peer. Markers: marker:`host.terminal_held_after_cancel`. Reuse of a returned block before the owner drains its return: `crates/shm-transport/src/backend/ring.rs:2513` shows `take_reclaimed` reporting nothing for a block reserved again since its return, and `crates/host-runtime/src/ring_transport.rs:3409` shows the credit on the reused block held until the peer releases the new publication.
Confidence: high - [evidence](evidence/terminal-credit-follows-storage.md). Verified against the tree of this catalog's introducing commit: `crates/host-runtime/src/dispatch.rs:652`; `crates/host-runtime/src/connection.rs:98`; `crates/shm-transport/src/backend/ring.rs:1653`; `crates/host-runtime/src/ring_transport.rs:1352`.
Existing check: `crates/host-runtime/src/ring_transport.rs:3375` publishes a terminal carrying a credit and shows the credit outstanding until `Ring::take_reclaimed` observes the block's return; `crates/host-runtime/tests/dispatch.rs:1649` admits 63 unsettled requests, refuses the 64th with `server_busy`/`terminal capacity exhausted` and zero dispatch while pending slots remain, then dispatches again only after the cancelled terminal's block is consumed.
Impact: A credit refunded on callback completion lets terminals exceed the reserved inventory.
Open questions:

- Handoff: none for this task.

### reserved-progress-under-data-exhaustion

Type: liveness
Reachability: default-production
Status: active
Exercised: partial - `crates/shm-transport/src/backend/ring.rs:2462` proves control and terminal reservations succeed while ordinary descriptor headroom is exhausted; `crates/host-runtime/src/ring_transport.rs:3263` shows the host publisher publishing an eligible Ping and an unrelated terminal past a blocked ordinary ticket with the smallest ordinary class empty, then resuming admission order as blocks return. Client publication selection belongs to #552 and #550.
Guarantee: With ordinary blocks and descriptors exhausted, eligible reserved control and terminal frames still publish within the bounded attempt, and returns still complete (R11).
Check: `always` - `try_reserve_in(Inventory::Control | Terminal, ..)` succeeds while `try_reserve_in(Ordinary, ..)` is `Exhausted`, until the reserved depth itself is full.
Fault/timing angle: Ordinary exhaustion by block class and by descriptor headroom, separately and together.
Required faults and enabling state: Ordinary descriptors at 32 outstanding; every ordinary class empty; both at once. Markers: marker:`pool.ordinary_class_and_descriptors_exhausted_together`, marker:`pool.reserved_depth_exhausted`.
Confidence: high - [evidence](evidence/reserved-progress-under-data-exhaustion.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1042`; `crates/shm-transport/src/pool.rs:31`; `crates/host-runtime/src/ring_transport.rs:1307`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2462` proves control and terminal reservations succeed while ordinary descriptor headroom is exhausted; `crates/host-runtime/src/ring_transport.rs:3263` shows the host publisher publishing an eligible Ping and an unrelated terminal past a blocked ordinary ticket with the smallest ordinary class empty, then resuming admission order as blocks return. Client publication selection belongs to #552 and #550.
Impact: A draining peer that cannot exchange controls under data backpressure never recovers.
Open questions:

- Handoff: #552, #550 for client publication selection.

### reserved-publication-order

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/host-runtime/src/ring_transport.rs:3263` checks the host publisher: a blocked ordinary head lets an eligible Ping and an unrelated terminal through, a terminal whose stream prefix is blocked waits, and Goodbye waits for every earlier frame; `crates/host-runtime/src/ring_transport.rs:3497` pins that a channel-0 Request is never a bypass control. Client publishers belong to #552 and #550.
Guarantee: Cross-class bypass preserves increasing consumer Request correlations, ordinary FIFO, per-stream data before terminal, and drain before Goodbye; a channel-0 Request is not a bypass control.
Check: `always` - the sequence of published headers per direction satisfies the four order predicates.
Fault/timing angle: A blocked ordinary ticket first in queue with eligible Ping and unrelated terminal behind it.
Required faults and enabling state: Ordinary exhaustion with a Ping and a terminal queued behind a data frame. Markers: marker:`host.bypass_eligible_frame_behind_blocked_data`.
Confidence: medium - [evidence](evidence/reserved-publication-order.md). Verified against the tree of this catalog's introducing commit: docs/payload-pool-protocol.md section 11; `docs/host-wire-protocol.md:314`; `crates/host-runtime/src/ring_transport.rs:1088`; `crates/host-runtime/src/ring_transport.rs:1115`.
Existing check: `crates/host-runtime/src/ring_transport.rs:3263` checks the host publisher: a blocked ordinary head lets an eligible Ping and an unrelated terminal through, a terminal whose stream prefix is blocked waits, and Goodbye waits for every earlier frame; `crates/host-runtime/src/ring_transport.rs:3497` pins that a channel-0 Request is never a bypass control. Client publishers belong to #552 and #550.
Impact: A reordered terminal or correlation breaks the application contract.
Open questions:

- Handoff: #552, #550 for client publishers.

### partial-setup-reclaims-only-unexposed-resources

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/shm-transport/tests/profile.rs:215` covers worker/backing settlement, quarantine, and uncertain retention; `crates/host-runtime/src/ring_transport.rs:473` refunds on a pre-exposure failure, and `crates/host-runtime/src/ring_transport.rs:3666` shows a refused admission charges nothing and the released charge admits the next connection. A failure injected between descriptor duplication and grant transfer is not yet exercised.
Guarantee: Setup failure refunds only resources proved unexposed; a quarantine-accounting failure leaves the backing charge counted forever (KTD5, KTD7).
Check: `always` - after a setup failure before the grant is sent, `snapshot().active` returns to its prior value; after a quarantine failure, `BackingAdmission::is_active()` is false and no refund occurs on drop.
Fault/timing angle: Failure after ring creation before the grant is sent; quarantine accounting failure.
Required faults and enabling state: A `DuplexRing::create` failure; a poisoned accounting lock at quarantine time. Markers: marker:`admission.setup_failed_before_exposure`, marker:`admission.quarantine_accounting_failed`.
Confidence: medium - [evidence](evidence/partial-setup-reclaims-only-unexposed-resources.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/profile.rs:622`; `crates/host-runtime/src/ring_transport.rs:525`.
Existing check: `crates/shm-transport/tests/profile.rs:215` covers worker/backing settlement, quarantine, and uncertain retention; `crates/host-runtime/src/ring_transport.rs:473` refunds on a pre-exposure failure, and `crates/host-runtime/src/ring_transport.rs:3753` shows a refused admission charges nothing and the released charge admits the next connection. A failure injected between descriptor duplication and grant transfer is not yet exercised.
Impact: Refunding storage a peer may have mapped lets a later connection map over it.
Open questions:

- Handoff: the last implementation task injects a descriptor-duplication failure in the combined matrix.

### bounded-refusal-and-recovery

Type: liveness
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/src/backend/ring.rs:2356` and `crates/shm-transport/tests/profile.rs:107`; `crates/host-runtime/src/ring_transport.rs:3666` shows the host names the exhausted resource in `exhaustion.by_resource`, charges nothing, and admits again after release; `crates/host-runtime/src/ring_transport.rs:3484` bounds a stalled peer by the frame deadline.
Guarantee: Every refusal is bounded and named by resource (class, descriptor headroom, admission field), and recovery follows the resource's own release within one reservation attempt.
Check: `always` - after the exhausting resource is released, the next `try_reserve_in` or `admit` succeeds; refusals charge nothing.
Fault/timing angle: Exhaustion of each resource in isolation.
Required faults and enabling state: One class empty; descriptor headroom full; one admission field at its limit. Markers: marker:`pool.single_resource_exhausted`, marker:`admission.field_at_limit`.
Confidence: high - [evidence](evidence/bounded-refusal-and-recovery.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1044`; `crates/shm-transport/src/profile.rs:416`; `crates/host-runtime/src/ring_transport.rs:279`.
Existing check: `crates/shm-transport/src/backend/ring.rs:2356` and `crates/shm-transport/tests/profile.rs:107`; `crates/host-runtime/src/ring_transport.rs:3666` shows the host names the exhausted resource in `exhaustion.by_resource`, charges nothing, and admits again after release; `crates/host-runtime/src/ring_transport.rs:3484` bounds a stalled peer by the frame deadline.
Impact: An unbounded or unrecoverable refusal is a hang the peer cannot diagnose.
Open questions:

- Handoff: #552 for Rust-client refusal recovery; #550 for native.

### direct-serialization-commit-boundary

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/host-runtime/src/ring_transport.rs:3404` counts serializer invocations: zero while the class is exhausted through retirement, exactly one once a block is reserved; `crates/host-runtime/src/ring_transport.rs:1382` serializes through `ReservationWriter` only after reservation, and `crates/shm-transport/src/backend/ring.rs:2385` covers abort and short commit.
Guarantee: A direct serializer runs only into a reserved block whose span is the logical body bound, is never consumed without a reservation, and a short result keeps its block (KTD1).
Check: `always` - `ProducerReservation::capacity()` equals the caller's bound, `segment(0)` has that length, and an unreserved `DirectFrame` is never invoked.
Fault/timing angle: Reservation failure before serialization.
Required faults and enabling state: `reserve_until` returning `Deadline` with a `DirectFrame` queued. Markers: marker:`host.direct_frame_unreserved_on_deadline`.
Confidence: high - [evidence](evidence/direct-serialization-commit-boundary.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/ring.rs:1736`; `crates/host-runtime/src/ring_transport.rs:1422`.
Existing check: `crates/host-runtime/src/ring_transport.rs:3404` counts serializer invocations: zero while the class is exhausted through retirement, exactly one once a block is reserved; `crates/host-runtime/src/ring_transport.rs:1382` serializes through `ReservationWriter` only after reservation, and `crates/shm-transport/src/backend/ring.rs:2385` covers abort and short commit.
Impact: Serializing into class slack or without a reservation would write bytes no descriptor accounts for.
Open questions:

- Handoff: none for this task.

### terminal-encoding-reserve-bound

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/shm-transport/tests/contract.rs:187` checks the 32 KiB terminal block holds 25,406 body and 25,427 frame bytes; `crates/host-runtime/src/dispatch.rs:1594` serializes the worst-escaped 128-byte code, 4,096-byte message, and `u64::MAX` retry hint and shows the frame is exactly `TERMINAL_FRAME_BYTES` (25,427) and fits the terminal class body; terminal bodies charge `HostShared::terminal_budget`, sized from `TERMINAL_RESERVED_BYTES_PER_CONNECTION`, never the ordinary egress budget.
Guarantee: The worst-escaped 128-byte code, 4,096-byte message, and maximum retry hint fit one 32 KiB terminal block unchanged, and dedicated encoding bytes never consume the maximum-frame egress floor.
Check: `always` - `terminal.body_capacity() >= 25_406` and the serializer output for the worst case is `<= 25_406` bytes.
Fault/timing angle: None; boundary arithmetic.
Required faults and enabling state: The maximum-length worst-escaped terminal serialized with ordinary egress exhausted. Markers: marker:`host.terminal_worst_case_serialized`.
Confidence: high - [evidence](evidence/terminal-encoding-reserve-bound.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/tests/contract.rs:217`; `crates/host-runtime/src/config.rs:33`; `crates/host-runtime/src/runtime.rs:107`.
Existing check: `crates/shm-transport/tests/contract.rs:187` checks the 32 KiB terminal block holds 25,406 body and 25,427 frame bytes; `crates/host-runtime/src/dispatch.rs:1594` serializes the worst-escaped 128-byte code, 4,096-byte message, and `u64::MAX` retry hint and shows the frame is exactly `TERMINAL_FRAME_BYTES` (25,427) and fits the terminal class body; terminal bodies charge `HostShared::terminal_budget`, sized from `TERMINAL_RESERVED_BYTES_PER_CONNECTION`, never the ordinary egress budget.
Impact: A terminal that does not fit its reserve is truncated or replaced, hiding the real error.
Open questions:

- Handoff: none for this task.

### send-outcome-no-generic-replay

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/host-runtime/src/ring_transport.rs:1594` classifies `Deadline`/`Unreserved` as zero-byte and `Reserved` as unknown; `a_client_send_past_its_frame_deadline_publishes_nothing` in crates/host-runtime/src/ring_transport.rs; `crates/host-runtime/src/ring_transport.rs:3565` retires a host ticket that missed its deadline as `not_sent` with nothing published. Stop/restart witnesses for the clients belong to #552 and #550.
Guarantee: Failure before publication is `not_sent`; failure after publication is `outcome_unknown`; no layer replays an uncertain request (R7).
Check: `always` - every `SendFailure` maps to exactly one of the two outcomes and no code path resubmits a frame after `commit` returned `Err`.
Fault/timing angle: Quarantine between the pre-commit check and `commit`.
Required faults and enabling state: A quarantine landing after `write` and before `commit`. Markers: marker:`host.quarantine_between_write_and_commit`.
Confidence: medium - [evidence](evidence/send-outcome-no-generic-replay.md). Verified against the tree of this catalog's introducing commit: `crates/host-runtime/src/ring_transport.rs:1531`; `crates/shm-transport/src/backend/ring.rs:1019`.
Existing check: `crates/host-runtime/src/ring_transport.rs:1594` classifies `Deadline`/`Unreserved` as zero-byte and `Reserved` as unknown; `a_client_send_past_its_frame_deadline_publishes_nothing` in crates/host-runtime/src/ring_transport.rs; `crates/host-runtime/src/ring_transport.rs:3565` retires a host ticket that missed its deadline as `not_sent` with nothing published. Stop/restart witnesses for the clients belong to #552 and #550.
Impact: A replayed uncertain request executes twice.
Open questions:

- Handoff: #552, #550.

### reclamation-diagnostics-meaning

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `crates/host-runtime/src/ring_transport.rs:3738` takes one snapshot of live backings, outstanding leases, and released backing bytes while the endpoint runs and again after it ends, and shows `reclamation.completed` advancing for the generation end without advancing released backing; `RingTransport::return_snapshot` (`crates/host-runtime/src/ring_transport.rs:262`) reads both quantities under one lock and `diagnostics()` reports `reclamation.meaning`, `returns`, and `exhaustion.by_resource` as distinct objects under the existing wire names.
Guarantee: Diagnostics report outstanding return obligations, quarantined commitment, and actually released backing as distinct quantities, each sampled independently rather than as one atomic snapshot.
Check: `always` - `outstanding_returns` counts live leases exactly, and the host counter's meaning is corrected without changing wire names.
Fault/timing angle: A held reader across a connection generation end.
Required faults and enabling state: A generation ends while a lease is live. Markers: marker:`host.generation_ended_with_live_lease`.
Confidence: high - [evidence](evidence/reclamation-diagnostics-meaning.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/backend/retained.rs:342`; `crates/host-runtime/src/ring_transport.rs:262`; `crates/host-runtime/src/ring_transport.rs:378`.
Existing check: `crates/host-runtime/src/ring_transport.rs:3738` takes one snapshot of live backings, outstanding leases, and released backing bytes while the endpoint runs and again after it ends, and shows `reclamation.completed` advancing for the generation end without advancing released backing; `RingTransport::return_snapshot` (`crates/host-runtime/src/ring_transport.rs:262`) reads both quantities under one lock and `diagnostics()` reports `reclamation.meaning`, `returns`, and `exhaustion.by_resource` as distinct objects under the existing wire names.
Impact: A counter read as released storage misleads operators about reclaimable capacity.
Open questions:

- Handoff: none for this task.

## Completion evidence

### single-replacement-surface

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `rg` over the tree finds no `SpanPlan`, `SamplePrefix`, `MADV_REMOVE`, `ReleaseSink`, `RingGrant`, `host-test-ring-v1`, or `arena_bytes` in production sources; `crates/shm-transport/tests/contract.rs:167` pins the identifiers.
Guarantee: One transport, one layout reader, one receive representation; no FIFO allocator, slot-coupled release, setup decoder, compatibility wrapper, selection flag, or legacy fixture remains (R13).
Check: `always` - the search list above returns nothing outside `docs/properties` history and this catalog.
Fault/timing angle: None.
Required faults and enabling state: A tree-wide search after every PR. Markers: marker:`gate.sole_surface_search_run`.
Confidence: high - [evidence](evidence/single-replacement-surface.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/src/arena.rs` and `backend/sample.rs` deleted; `crates/shm-transport/src/harness.rs:31`.
Existing check: `rg` over the tree finds no `SpanPlan`, `SamplePrefix`, `MADV_REMOVE`, `ReleaseSink`, `RingGrant`, `host-test-ring-v1`, or `arena_bytes` in production sources; `crates/shm-transport/tests/contract.rs:167` pins the identifiers.
Impact: A second transport or reader is an untested path a peer can select.
Open questions:

- Handoff: #548, #552, #550 rerun the sweep; the last task owns the final one.

### integration-gate-dependency-selection

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `.github/workflows/ci.yml:95` adds host-runtime to the `native` filter and `.github/workflows/ci.yml:71` adds the protocol document, which `crates/shm-transport/tests/contract.rs:364` reads.
Guarantee: Every CI gate is selected by the inputs it consumes: host-runtime changes select the native/E2E job, and the protocol document selects the transport gates that read it.
Check: `always` - each path a gate's tests read appears in that gate's filter.
Fault/timing angle: None.
Required faults and enabling state: A host-runtime-only pull request. Markers: marker:`gate.host_runtime_only_change`.
Confidence: high - [evidence](evidence/integration-gate-dependency-selection.md). Verified against the tree of this catalog's introducing commit: `.github/workflows/ci.yml:95`; `crates/shm-transport/tests/contract.rs:366`.
Existing check: `.github/workflows/ci.yml:95` adds host-runtime to the `native` filter and `.github/workflows/ci.yml:71` adds the protocol document, which `crates/shm-transport/tests/contract.rs:364` reads.
Impact: A change to a consumed input that skips its gate is an untested change.
Open questions:

- Handoff: every later PR rechecks.

### unsafe-witness-selection

Type: safety
Reachability: test-only
Status: active
Exercised: yes - the Miri step greps each required witness by name (`.github/workflows/ci.yml:653`), the Valgrind step likewise (`.github/workflows/ci.yml:701`), and the `two-process` job runs the child-process witnesses without the memcheck runner.
Guarantee: Each required unsafe witness is individually selected and reported by the Miri and Valgrind gates; two-process witnesses run separately; a nonzero aggregate count discharges nothing.
Check: `always` - the CI logs contain `test <witness> ... ok` for every listed witness.
Fault/timing angle: A module move that drops a witness from the prefix filter.
Required faults and enabling state: A renamed or moved witness. Markers: marker:`gate.witness_renamed`.
Confidence: high - [evidence](evidence/unsafe-witness-selection.md). Verified against the tree of this catalog's introducing commit: `.github/workflows/ci.yml:642`; `.github/workflows/ci.yml:734`.
Existing check: the Miri step greps each required witness by name (`.github/workflows/ci.yml:653`), the Valgrind step likewise (`.github/workflows/ci.yml:701`), and the `two-process` job runs the child-process witnesses without the memcheck runner.
Impact: An empty gate passes while proving nothing.
Open questions:

- Handoff: none.

### acceptance-artifact-provenance

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the native job builds the addon from source before every test run (`.github/workflows/ci.yml:785`); `nativeWireConstants` (`packages/shm-native/index.ts:12`) compares the loaded addon's identifiers with the wrapper's. Recorded artifact identity is #550's.
Guarantee: Acceptance runs load a wrapper and addon built from the tested source at the current layout.
Check: `always` - the addon's `descriptorSchemaVersion()` and `qualifiedTestProfile()` equal the wrapper constants in every run.
Fault/timing angle: A stale prebuilt `shm_native.node`.
Required faults and enabling state: A run against a stale artifact. Markers: marker:`gate.stale_artifact_present`.
Confidence: medium - [evidence](evidence/acceptance-artifact-provenance.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/tests/mechanism.ts:56`; `.github/workflows/ci.yml:785`.
Existing check: the native job builds the addon from source before every test run (`.github/workflows/ci.yml:785`); `nativeWireConstants` (`packages/shm-native/index.ts:12`) compares the loaded addon's identifiers with the wrapper's. Recorded artifact identity is #550's.
Impact: A stale artifact tests the old layout while reporting the new one.
Open questions:

- Handoff: #550.

### real-process-current-layout-witness

Type: reachability
Reachability: test-only
Status: active
Exercised: partial - `crates/shm-transport/tests/ring.rs:347` records a completed real cross-process exchange at layout 4; the direct-host E2E and native suites still skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and on Node (`node_detachment_unavailable`), which are recorded limitations, not passes.
Guarantee: Every required real-process witness records current-layout activation and a completed daemon request on a supported runtime; a skip discharges nothing.
Check: `reachable` - the code point that reports a completed real exchange (`EIDNARA_SHM_CHILD_DONE`) executes in the two-process job; `reachable` because a specific code location must execute.
Fault/timing angle: None.
Required faults and enabling state: A supported runtime. Markers: marker:`gate.supported_runtime_present`.
Confidence: medium - [evidence](evidence/real-process-current-layout-witness.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/tests/ring.rs:429`; `packages/e2e-tests/src/rust-runner/hermetic-host.ts:275`.
Existing check: `crates/shm-transport/tests/ring.rs:347` records a completed real cross-process exchange at layout 4; the direct-host E2E and native suites still skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and on Node (`node_detachment_unavailable`), which are recorded limitations, not passes.
Impact: A skipped suite counted as passing hides an unexercised layout.
Open questions:

- Handoff: #548, #552, #550 add daemon-level real-process witnesses.

### malformed-fixture-valid-baseline

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `packages/shm-native/tests/mechanism.ts:939` proves the unmutated fixture decodes before mutation cases; `crates/shm-transport/tests/fuzz_corpus.rs:78` asserts each `valid` seed is accepted.
Guarantee: Every malformed-input fixture passes unmutated under the current layout before its mutation cases count.
Check: `always` - `grantDecodes(fixture) == true` and each corpus `valid` seed is accepted.
Fault/timing angle: None.
Required faults and enabling state: A fixture built from the current geometry. Markers: marker:`gate.fixture_baseline_checked`.
Confidence: high - [evidence](evidence/malformed-fixture-valid-baseline.md). Verified against the tree of this catalog's introducing commit: `packages/shm-native/src/lib.rs:276`; `crates/shm-transport/tests/fuzz_corpus.rs:60`.
Existing check: `packages/shm-native/tests/mechanism.ts:939` proves the unmutated fixture decodes before mutation cases; `crates/shm-transport/tests/fuzz_corpus.rs:78` asserts each `valid` seed is accepted.
Impact: A stale fixture makes every rejection assertion pass for the wrong reason.
Open questions:

- Handoff: none.

### fuzz-adapter-current-contract

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/shm-transport/src/harness.rs:31`, `crates/shm-transport/src/harness.rs:79`, and `provider_grant` decode the current contract; the corpus `valid` seeds decode and the dormant sample reader is deleted.
Guarantee: Fuzz adapters decode the current descriptor, grant, and completion encodings, accept at least one valid seed each, and no dormant old reader remains.
Check: `always` - `cargo check --bins` of the fuzz workspace succeeds and every `valid` seed is accepted.
Fault/timing angle: None.
Required faults and enabling state: A corpus replay. Markers: marker:`gate.fuzz_corpus_replayed`.
Confidence: high - [evidence](evidence/fuzz-adapter-current-contract.md). Verified against the tree of this catalog's introducing commit: `crates/shm-transport/fuzz/fuzz_targets/{pool_descriptor,provider_grant,payload_completion}.rs`; `crates/shm-transport/tests/fuzz_corpus.rs:80`.
Existing check: `crates/shm-transport/src/harness.rs:31`, `crates/shm-transport/src/harness.rs:79`, and `provider_grant` decode the current contract; the corpus `valid` seeds decode and the dormant sample reader is deleted.
Impact: A fuzz target that rejects everything finds nothing.
Open questions:

- Handoff: none.

### capacity-model-conservation

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `python3 docs/properties/independent-payload-pools/capacity-model/simulate.py` passes its self-checks and prints the ten recorded scenarios; the outputs are recorded in `capacity-model/results.md` and labeled uncalibrated.
Guarantee: The capacity model conserves leases across every scenario, classifies refusals disjointly, and drains completely; its results are sizing input, not performance evidence.
Check: `always` - self-checks pass and every scenario reports `unfinished_after_drain == 0` and `descriptor_refusals + sum(class_refusals) == would_block`.
Fault/timing angle: None.
Required faults and enabling state: A model run. Markers: marker:`model.self_checks_run`.
Confidence: high - [evidence](evidence/capacity-model-conservation.md). Verified against the tree of this catalog's introducing commit: `docs/properties/independent-payload-pools/capacity-model/simulate.py`; `docs/properties/independent-payload-pools/capacity-model/results.md`.
Existing check: `python3 docs/properties/independent-payload-pools/capacity-model/simulate.py` passes its self-checks and prints the ten recorded scenarios; the outputs are recorded in `capacity-model/results.md` and labeled uncalibrated.
Impact: A model that loses leases misleads the initial sizing.
Open questions:

- Handoff: none.

## Relationship map

- `descriptor-private-snapshot`, `validated-setup-geometry`, and
  `structural-rejection-before-dispatch` gate every byte exposure;
  `shared-copy-source-access` bounds how exposed bytes are read.
- `descriptor-capacity-independent-of-payload`,
  `released-block-reuse-preserves-held-bytes`, and
  `payload-identity-authorizes-reuse` together are R1-R2; all three depend on
  `class-allocation-conservation`.
- `completion-cell-final-owner-once`, `worker-drop-forbidden-operations`,
  `capacity-wake-progress`, and `wake-failure-preserves-published-ownership`
  are the return path (KTD3); `retained-mapping-lifetime` and
  `owned-lease-thread-boundary` are its ownership preconditions.
- The reserves group depends on `Inventory::Control` and `Inventory::Terminal`
  from this task and on the publisher selection of #548, #552, and #550.
- The completion-evidence group checks the checks; it depends on nothing in
  the transport and every transport record depends on it for its witness to
  count.
