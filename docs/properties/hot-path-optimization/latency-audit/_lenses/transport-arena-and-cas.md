# Ring arena residency and CAS usage surface

This lens records what the shared-memory ring, the host egress path, and the
kernel content-addressed store (CAS) promise about memory residency, payload
copies, and artifact byte accounting, so that deferred page punching, direct
response serialization, a zero-fill-free `to_vec`, and a durable usage counter
can be judged against preserved properties rather than against the audit's
timing numbers. Anchors are checked in `/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. It is analysis only;
nothing outside this file changed and no test ran. Punch soundness itself is
already catalogued in the shm-transport part
([reclamation-excludes-pages-with-live-wrapped-bytes][shm-punch],
[trim-removes-only-dead-pages-below-the-write-cursor][shm-trim]); request-owned
charge lifetime is [E2][e2]. This lens adds the residency bound, the copy
discipline, the direct-serialize failure contract, and the CAS usage oracle.

The ring is a sealed sparse memfd whose arena is [64 MiB per
direction][arena-const]. Punching happens in three places, all in the producer
process: [`reclaim_completed_inner`][reclaim] runs at the head of every
[`try_reserve`][try-reserve], advances `arena_reclaimed` over contiguous
released frames, and calls [`punch_dead_pages`][punch] only when the dead
run since the last punch reaches [`punch_batch_bytes`][batch], which is
`arena_bytes / PUNCH_BATCH_DIVISOR` with [divisor 4][divisor], so 16 MiB;
[`abort_reservation`][abort] punches an uncommitted reservation's dirtied pages
at once; and [`Ring::trim`][trim] punches every dead page including the partial
trailing one, but has no caller outside the crate's tests. Each pass rounds
its logical range inward to whole pages ([`removal_ranges`][removal-ranges])
and calls [`remove_pages`][remove-pages], whose `SAFETY` comment and the
underlying [`madvise_remove`][madv] contract require that no live byte occupies
the range; the ring discharges that by never advancing `arena_reclaimed` past
a frame the consumer has not released ([release protocol][release]) and by
bounding the live end with the producer-local [`reserved_end`][live-end]. The
host admits connections under [`MAX_RING_RESIDENT_BYTES`][max-resident], a
1 GiB ceiling that the doc comment calls "sparse ring virtual arena bytes":
[`affordable_connections`][affordable] divides it by one connection's arena
charge, [`process_limits`][process-limits] refuses more at startup, and the
[default configuration][config-default] and [validation][config-validate] use
the same quotient. No production code reads residency;
[`resident_arena_pages`][resident-api] is a `mincore` probe that only tests
call. The wire contract's only related statements are that the Synapse
resident cap "is an accounting boundary, not an exact process-RSS claim"
([§7.5.1][wire-751]) and that "no timed ring poll, prefault, runtime
scheduling selector, or fallback path exists" ([§7.7][wire-77]).

Every payload byte crosses the arena through [`copy_in`][copy-in] or
[`copy_out`][copy-out], which derive a per-byte atomic width from the absolute
address and length ([`AccessShape`][shape]) so a same-shape concurrent access
is stale data rather than a data race; [`LeaseSpan`][span-doc] never exposes a
`&[u8]`, its raw pointer is documented for [exclusive pre-commit
writes only][span-ptr], and [`to_vec`][to-vec] zero-fills a `Vec` of
`body_len` before copying each span into it. On the host, a handler response
is measured ([`measure`][measure]), an owned [`OutputBuffer`][outbuf] of
`len + HEADER_LEN` is reserved with an egress charge
([`reserve_output`][reserve-output] over [`reserve`][reserve]), the body is
serialized into it with an exact-length check ([`write_to`][write-to] from
[`settle_prepared`][settle-prepared]), and [`encode_split_frame`][split]
either moves the body in place to prepend the header
([`encode_owned_frame`][encode-owned], bodies under [16 KiB][split-min]) or
carries the body as `tail`; the endpoint thread then copies it into the ring
in [`publish_owned`][publish-owned]. The parallel
direct path reserves an exact length and a serializer closure
([`reserve_direct`][reserve-direct], [`output_from_writer`][from-writer]),
builds a [`DirectFrame`][direct-frame] whose header already carries that
length ([`emit_reserved_frame`][emit-reserved]), and in
[`publish_direct`][publish-direct] reserves the ring first, runs the serializer
into a [`ReservationWriter`][res-writer], and commits. Its only production-tree
callers are a [fixture arm][fixture-arm] that no test sends and one [in-crate
deadline test][t-deadline]. In the CAS, [`ingest_artifact_inner`][ingest]
writes and syncs the temp file, then takes the [writer lock][lock-writer] and
calls [`check_budget`][check-budget], which walks every shard with `statat`
([`regular_file_bytes`][walk], summing [`st_size`][stat-bytes]) and refuses
with [`ArtifactError::capacity`][cap-error] when `usage + new_bytes` would
exceed [`artifact_cap`][cap-default] (4 GiB by default); the same walk backs
[`artifact_budget_facts`][facts] through [`object_usage`][object-usage] and
the daemon's [30 s health sampler][sampler] ([loop][sampler-run]). Staging
temps under `tmp` are never counted and are swept on every
[store open][tmp-sweep].

## Candidate properties

### host-arena-admission-bounds-virtual-not-resident-bytes

Type: safety
Check: `always` - at every admission decision, the sum of `arena_bytes`
charged to admitted connections is at most
[`MAX_RING_RESIDENT_BYTES`][max-resident], and the host refuses
(`ExceedsResidentBytes` at startup, connection refusal at runtime) rather than
admitting a connection that would exceed it; the bound is evaluated on the
grant's virtual arena size, never on `mincore` residency.
`always` because the check runs on every admission and has no legal exception.
Guarantee: The host's only memory ceiling on the ring is a hard admission
bound on virtual arena bytes; physical residency below that ceiling is a
consequence of punching, not a checked invariant.
Fault/timing angle: A deferred-punch design that reads the constant's name as
an RSS promise and adds a residency check would introduce a new invariant with
no existing oracle; one that treats it as a budget for unpunched bytes would
let a single idle ring hold its full 64 MiB per direction resident, which the
current bound already permits.
Required faults and enabling state: `max_connections` set above
[`affordable_connections`][affordable]; a connection attempt when every
affordable slot is charged.
Reachability: default-production - [`process_limits`][process-limits] runs at
[host startup][runtime-limits] unconditionally, and [`HostLimits::validate`][config-validate]
refuses any configuration above the quotient.
Existing check:
[`process_limits_reject_counts_above_the_resident_byte_ceiling`][t-limits]
(unaudited) pins the quotient, the `+1` refusal, and the zero-rounds-to-one
case.
Open questions:
- Does the specification intend a physical residency bound at all, or only to
  preserve this admission bound and the per-ring dead-byte bound below? (needs
  human input)

### dead-unpunched-arena-bytes-stay-below-the-punch-batch

Type: safety
Check: `always` - after every return from
[`reclaim_completed_inner`][reclaim], `arena_reclaimed - punched <
punch_batch_bytes()` holds for the producer handle (the branch at
[`:2129-2134`][punch-decision] punches when the run reaches the batch and
[`punch_dead_pages`][punch] leaves `punched` at the page below `reclaimed`),
and after every [`abort_reservation`][abort] no page of the aborted range is
resident. `always` because this is the current implicit residency bound; if a
change defers punching to idle, the record's check becomes the replacement
bound the specification states, evaluated at every idle point of the
[endpoint loop][idle-select].
Guarantee: The bytes a producer ring keeps resident beyond its live and
pending frames are bounded by one punch batch (16 MiB at the shipped profile)
plus one boundary page, and an aborted reservation contributes nothing.
Fault/timing angle: Punching is currently coupled to the next `try_reserve`,
so an idle ring after a burst keeps up to one batch resident until the next
publish; a design that moves punching to the idle branch changes when the
bound is re-established but must not change what it is without saying so.
The client process runs the same crate for the peer-to-host direction
([`reserve_until` in shm-native][native-reserve]) and has no `trim` caller
either, so a host-only change alters one direction.
Required faults and enabling state: A released run of at least one batch on
an otherwise idle ring; a reservation written then aborted; a wrapped run
crossing the arena end.
Reachability: default-production - `reclaim_completed` runs inside every
[`try_reserve`][try-reserve]; `abort_reservation` runs from every
[`ProducerReservation` drop][res-drop] and every `write`/`commit` failure.
Existing check: [`unaligned_batch_boundaries_do_not_strand_pages`][t-batch]
(the batch threshold and boundary page),
[`aborted_reservation_leaves_no_resident_pages`][t-abort],
[`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse],
[`subpage_releases_stay_resident_until_trim`][t-subpage],
[`page_removal_failure_quarantines_before_capacity_publication`][t-punchfail];
all unaudited. None asserts the bound as an inequality over a long run.
Open questions:
- What is the replacement bound when punching is deferred: bytes, pages, a
  time since the last reserve, or "punched by the next idle point"? (needs
  human input)
- Does an idle-time punch count as a "timed ring" activity under [§7.7][wire-77],
  which forbids a timed ring poll and any prefault? (needs human input)

### arena-payload-copies-keep-the-address-derived-atomic-shape

Type: safety
Check: `always` - every byte moved between the arena and process memory goes
through [`copy_in`][copy-in], [`copy_out`][copy-out], `read_byte`, or
`checksum`, each of which partitions the range with
[`AccessShape::of(address, len)`][shape]; no `&[u8]` or `&mut [u8]` is formed
over arena bytes; and a `to_vec` that skips the zero-fill still returns a
`Vec` whose every byte in `0..body_len` was written by `copy_out` before the
`Vec` is observable, or returns `Err` without exposing the buffer. `always`
because the soundness argument in the [`LeaseSpan::new` contract][span-safety]
and the crate's [AGENTS.md][agents] treat this as a verification boundary.
Guarantee: A concurrent peer store of the same shape yields stale bytes, never
a mixed-size data race, and a receiver never reads uninitialized process
memory as payload.
Fault/timing angle: The zero-fill at [`to_vec:331`][to-vec-fill] is what makes
the `Vec` initialized before any early `Err` return at [`:328-348`][to-vec];
replacing it with capacity plus `set_len` or `MaybeUninit` moves the
initialization proof onto the span-length checks, which run per span. A
`memcpy` replacement would form a reference over peer-writable memory, which
[`no-rust-reference-over-peer-writable-payload`][shm-noref] forbids.
Required faults and enabling state: A peer writing the same span while the
host copies (the [Miri thread test][t-concurrent] constructs it); a lease
whose span lengths do not sum to `body_len`; a span starting at each of the
eight word offsets.
Reachability: default-production - [`receive_one`][receive-to-vec] calls
`to_vec` on every inbound frame; `publish_owned` and `ReservationWriter` call
`copy_in` through [`write_reservation`][write-res] on every outbound frame.
Existing check:
[`copy_in_then_copy_out_round_trips_at_every_alignment_and_length`][t-roundtrip],
[`span_reads_tolerate_a_concurrent_writer`][t-concurrent],
[`read_byte_agrees_with_copy_to_at_every_alignment`][t-readbyte], all run under
the [Miri CI job][ci-miri] and natively; the [Valgrind job][ci-valgrind] runs
the `ring` integration tests. All unaudited. None covers `to_vec` with
mismatched span lengths.
Open questions: None.

### direct-frame-publishes-exactly-the-declared-length-or-nothing

Type: safety
Check: `always` - for every [`DirectFrame`][direct-frame] handed to
[`publish_direct`][publish-direct], either the serializer writes exactly
`body_len` bytes and `commit(body_len)` publishes one frame whose header `len`
equals `body_len`, or no frame becomes visible to the peer: a short write
fails `commit` with `Underfill` ([`:2533-2570`][commit-underfill]), an
over-write fails [`ReservationWriter::write`][res-writer] through
[`ProducerReservation::write`][res-write] with `Overflow`, a serializer `Err`
or panic drops the reservation, and each path runs
[`abort_reservation`][abort]. `always` because the ring's
[`prepare_commit`][prepare-commit] checks the header's declared length against
`body_len` on every commit and there is no partial-frame state.
Guarantee: The direct path never publishes a frame whose body differs from
its header's declared length, and a mid-write failure leaves no hole, no
partial frame, and no leaked slot.
Fault/timing angle: The serializer runs on the [endpoint thread][idle-select]
after `reserve_until` returns, so its CPU time holds a ring reservation and
blocks inbound receives for its duration; a serializer that finishes after
`frame_deadline` is refused at [`commit_before`][commit-before]. A failure
after partial write triggers a synchronous `MADV_REMOVE` of the dirtied pages
inside `abort_reservation`, and a failing punch quarantines the ring.
Every `publish_direct` error is then reported through
[`publish_one`][publish-one] as [`ReadClose::Corrupt("shared-memory publish
failed")`][publish-fail], which closes the whole connection; the owned path
turns the same serializer failure into a request-scoped `encode_failed`
terminal in [`settle_prepared_with`][settle-with] before any frame is queued.
Required faults and enabling state: A serializer that writes `body_len - 1`,
`body_len + 1`, returns `Err` after some bytes, or panics; a body that wraps
the arena end so both spans are touched; a deadline that expires during
serialization.
Reachability: test-only - [`reserve_direct`][reserve-direct] is reached only
through [`output_from_writer`][from-writer], whose sole caller outside the
crate is the [`direct_fill` fixture arm][fixture-arm], and no test sends that
mode; the [in-crate deadline test][t-deadline] calls `publish_direct`
directly. The specification would move it to default-production for transform
responses.
Existing check: [`a_commit_past_the_write_deadline_is_refused`][t-deadline]
(unaudited) covers the deadline arm and asserts the ring holds no frame
afterwards. No test covers underfill, overflow, serializer error, or panic on
the direct path; [`commit_after_quarantine_is_refused_and_aborts`][t-commitq]
covers the quarantine arm at ring level.
Open questions:
- Is a connection close the intended outcome for a serializer failure on a
  response that already won settlement, or must the direct path preserve the
  owned path's request-scoped `encode_failed` terminal? The change is visible
  to the peer as every in-flight request on the connection failing. (needs
  human input)
- [`req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame`][hr-terminal]
  holds trivially on the failure arm because nothing is emitted; the record
  should state that the settled `Response` is then never delivered, which
  [`req-a-a-response-publication-failure-never-reaches-the-settling-path`][hr-pubfail]
  already covers for owned frames.

### direct-frame-charge-and-captures-outlive-the-handler-until-publication

Type: safety
Check: `always` - a Direct [`OutputBuffer`][outbuf] holds an egress
`ByteCharge` of exactly `exact_len + HEADER_LEN`
([`reserve_direct:528`][reserve-direct]), [`into_parts`][into-parts] passes it
through unshrunk, the charge is dropped only after `commit` in
[`publish_one:784`][publish-one], and any request-owned bytes the serializer
closure captures (the transform segments, their scratch charge) are counted as
retained until that same point. `always` because [E2][e2] requires each charge
to cover its resource's lifetime and the closure extends the resource's
lifetime past `handle` returning.
Guarantee: Deferring serialization does not release the egress charge before
the bytes reach the ring, and does not let the source bytes outlive the
scratch charge that admitted them.
Fault/timing angle: On the owned path the parse charge is held until `handle`
returns and the encoded `Vec` carries its own charge; on the direct path the
closure holds the source tree after `handle` returns, so a scratch charge
released at return undercounts until the endpoint thread serializes. A
cancelled or retired generation drops the queued `OutboundFrame` with its
closure, which must release both.
Required faults and enabling state: A direct response queued behind a slow
egress so the closure is alive while the handler future is gone; generation
retirement or `discard` with a direct frame queued; the egress budget near
capacity so the exact charge is observable.
Reachability: test-only at HEAD for the same reason as the record above;
the charge bookkeeping in `reserve_direct` and `into_parts` is default-
production code with no default-production caller.
Existing check: [`into_parts_returns_the_unused_output_reservation`][t-parts],
[`into_parts_never_grows_a_charge_below_the_encoded_size`][t-parts2],
[`into_parts_does_not_leave_a_large_allocation_behind_a_small_charge`][t-parts3]
cover the owned arm only; all unaudited. None found for the direct arm.
Open questions:
- Which charge class covers the captured source bytes between `handle`
  returning and `publish_one` completing: the egress charge already taken, the
  request scratch charge, or a new class? (needs human input)

### artifact-admission-fails-closed-against-on-disk-object-bytes

Type: safety
Check: `always` - for every ingest, `check_budget` runs under the exclusive
[writer lock][lock-writer] before the reservation row, the shard creation, and
the publish rename ([`ingest.rs:477-490`][ingest-lock]); it refuses with
`Capacity` carrying `usage` and `cap` when `usage + byte_length >
artifact_cap`, adds zero for a digest already present
([`object_is_present`][present]), and counts invalidated-but-retained objects
because the walk reads the filesystem, not `evidence_meta`. `always` because
the cap is a hard promise with no admission on the failure arm.
Guarantee: No ingest publishes bytes that would raise the on-disk regular-file
sum under `objects` above `artifact_cap`, and a refused ingest leaves no
reservation row and no published object.
Fault/timing angle: The temp file is written and synced under `tmp`
([`:379-405`][ingest-temp]) before the lock and before the check, so `tmp`
bytes are never counted and a refused ingest still cost one full write; two
ingests serialize on the writer lock, so the walk cannot race a concurrent
publish, but it does race the health sampler's lock-free walk. A counter
consulted here instead of the walk must be updated inside the same lock scope
that publishes or unlinks, or it admits over the cap by whatever it lags.
Required faults and enabling state: A store at `cap - 1` receiving a
two-byte payload; the same digest re-ingested at the cap; an invalidated
reference whose bytes remain; a crash between publish and reference commit
followed by a second ingest before recovery.
Reachability: default-production - every daemon [`ingest_artifact`][route-ingest]
call reaches `check_budget`; `artifact_cap` is the [4 GiB default][cap-default]
in `KernelStore::open`.
Existing check:
[`cap_error_reports_usage_and_cap_without_poisoning_reads`][t-cap],
[`invalidated_retained_object_still_consumes_cap`][t-retained],
[`reclaim_frees_capacity_for_next_write`][t-reclaim],
[`payload_limit_is_inclusive_at_the_artifact_cap`][t-payload],
[`ingest_route_accepts_a_payload_at_the_artifact_cap`][t-route-cap]; all
unaudited. None found for dedup at exactly the cap or for a refusal after an
unrecovered orphan publish.
Open questions:
- The daemon maps `Capacity` to [`StoreBusy`][busy], a retryable class, while
  the only production paths that lower usage are failed-ingest cleanup and
  startup recovery ([`run_staging_maintenance`][maintenance] and
  [`delete_artifact`][delete] have no daemon caller). Is "busy" the intended
  classification of a cap that only a restart or an operator relieves? (needs
  human input)

### reported-artifact-usage-equals-on-disk-object-bytes-after-recovery

Type: safety
Check: `always` - at every quiescent point (after `KernelStore::open`
completes [`recover_interrupted_work`][recover], and after each ingest,
cleanup, GC pass, or purge returns with the writer lock released), the usage
the store reports through [`artifact_budget_facts`][facts] and the usage
`check_budget` admits against both equal an independent sum of `st_size` over
regular files under `objects` and its shard directories. `always` because the
[fault-injection oracle][t-oracle] asserts exactly this equality after every
fault point and after a second recovery, and a durable counter turns the
equality from a tautology into the drift check.
Guarantee: Reported artifact usage never drifts from the bytes actually on
disk once recovery has run, whatever crash window preceded it.
Fault/timing angle: Every path that changes on-disk object bytes is a
filesystem operation beside, not inside, a SQLite transaction: publish by
rename after the [reservation row][ingest-reservation] commits and before the
reference commits ([`:476-526`][ingest-publish], [`:580-595`][ingest-commit]);
unlink inside a fenced write transaction in
[`cleanup_failed_reference`][cleanup], [`reclaim_candidate`][reclaim-cand]
(reading the size it removes in [`unlink_artifact`][unlink-artifact]), and
[`complete_pending_purge_locked`][purge-unlink], which also
[sweeps the digest's temps][sweep-temps]; a dedup hit or a failed publish
only [releases the row][release-res]. A crash after the rename and
before the reference commit leaves bytes the walk counts and a `Live`
reservation that [`prepare_startup_cas_recovery`][startup] promotes to
`Reclaiming` and [`run_artifact_recovery`][recovery] unlinks; a crash after an
unlink and before its transaction commits leaves a row without bytes that
recovery retires as unreachable ([`:109-136`][startup-unreachable]). A counter
written in the transaction would over-report in the second window and
under-report in the first until recovery reconciles it; the walk is correct in
both because it is the reconciliation. Two further sources bypass any counter:
the [restore path][restore] verifies referenced artifacts against the current
tree, and the tests write objects [directly to disk][t-orphan]. The health
sampler walks without the writer lock, so its value may include a published-
but-uncommitted object; equality is claimed only at quiescence.
Required faults and enabling state: The six ingest fault points and three GC
fault points in [`cas_fault_injection.rs`][t-faults]; SIGKILL at the
[post-reservation and post-publish crash barriers][t-crash]; an object added
or removed under `objects` by something other than the store.
Reachability: default-production for ingest publish, failed-ingest cleanup,
and startup recovery; explicit-config-only for GC and purge reclamation,
which only tests and benches invoke
([`run_staging_maintenance`][maintenance], [`delete_artifact`][delete]).
Existing check: [`assert_semantic_oracle`][t-oracle] and
[`recover_twice`][t-recover-twice] in the fault-injection suite,
[`orphan_mtime_grace_and_budget_facts_are_reconciled_from_objects`][t-orphan],
[`facts_unless_abandons_the_artifact_walk_once_cancelled`][t-cancel],
[`startup_retires_a_live_reservation_whose_bytes_are_already_gone`][t-gone];
all unaudited. None found that compares two independent usage sources,
because only one exists at HEAD.
Open questions:
- Is the walk retained as a periodic or startup reconciliation, and what is
  the fail-closed action on `counter != walk`: refuse ingest, latch the CAS
  failure ([`latch_cas_failure`][latch]), or adopt the walk value? (needs
  human input)
- [`regular_file_bytes`][walk] counts a root-level regular file and any file in
  any subdirectory, while GC's [`scan_objects`][scan-objects] skips
  non-canonical shards; which definition does a counter follow? (needs human
  input)

### artifact-byte-decrement-paths-are-exercised

Type: reachability
Check: `sometimes` - a preservation campaign observes usage before and after
each path that removes object bytes, and each of the following occurs at least
once with a non-zero byte delta: `cleanup_failed_reference` after a
[`PublishOutcome::Published`][ingest-publish] followed by a reference-commit
failure; `run_artifact_recovery` unlinking an orphan publish after a crash;
`reclaim_candidate` reclaiming an invalidated object past its grace;
`complete_pending_purge_locked` unlinking a purged digest; and a GC unlink
that fails and is retried. `sometimes` because these are operational states,
and executing the unlink lines with a zero-byte candidate would not exercise
the accounting.
Guarantee: Every decrement path a durable counter must observe is reached
with bytes at stake before the counter ships.
Fault/timing angle: At HEAD only cleanup and startup recovery run in
production; GC and purge decrements are reached only when a caller is added,
so a counter validated against the walk in production traffic would never see
them.
Required faults and enabling state: `ArtifactIngestFault::AfterEvents` after a
new publish; a SIGKILL child at `INGEST_CRASH_POINT`; an invalidated reference
aged past `RESERVATION_MS`; a purge request; `ArtifactGcFault::Unlink`.
Reachability: test-only for the GC and purge arms; default-production for
cleanup and recovery, though only under a failure.
Existing check: [`return_value_fault_table_latches_eio_and_never_publishes_a_reference`][t-faults],
[`purge_and_gc_fault_table_preserves_pending_work_and_converges`][t-gcfaults],
[`crash_windows_recover_idempotently_and_match_no_crash_execution`][t-crash],
[`reclaim_frees_capacity_for_next_write`][t-reclaim]; all unaudited, and all
assert state convergence rather than a usage delta per path.
Open questions: None.

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [`process_limits_reject_counts_above_the_resident_byte_ceiling`][t-limits] | `affordable = MAX_RING_RESIDENT_BYTES / arena_bytes`; `affordable + 1` refused; zero rounds to one | unaudited |
| [`unaligned_batch_boundaries_do_not_strand_pages`][t-batch] | a run of `batch + 100` bytes leaves one boundary page; the next batch removes it | unaudited |
| [`aborted_reservation_leaves_no_resident_pages`][t-abort] | two written pages, abort, zero resident, conservation intact | unaudited |
| [`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse] | full-arena publish and release; next reserve sees zero resident and reads zero bytes | unaudited |
| [`subpage_releases_stay_resident_until_trim`][t-subpage] | sub-page releases keep one page until `trim` | unaudited |
| [`trim_reclaims_pending_releases_before_punching`][t-trim-order] | `trim` reclaims then punches; conservation intact | unaudited |
| [`trim_preserves_bytes_of_an_uncommitted_reservation`][t-trim-res] | `reserved_end` protects an uncommitted reservation from `trim` | unaudited |
| [`page_removal_failure_quarantines_before_capacity_publication`][t-punchfail] | injected `madvise` failure quarantines; cursors unchanged | unaudited |
| [`retained_oldest_lease_enforces_fifo_reclamation`][t-fifo] | releasing the oldest lease makes completed pages removable | unaudited |
| [`sealed_sparse_object_repeated_setup_and_stress_conservation`][t-sparse] | a fresh ring has zero resident arena pages | unaudited |
| syscall counter test at [`ring.rs:3086-3087`][t-syscall] | one `trim` is one `page_removals` | unaudited |
| [`copy_in_then_copy_out_round_trips_at_every_alignment_and_length`][t-roundtrip] | 16 source offsets x 16 shifts x 40 lengths round-trip; bytes before the shift untouched | unaudited |
| [`span_reads_tolerate_a_concurrent_writer`][t-concurrent] | concurrent same-shape `copy_in` never tears a byte through `read_byte`, `copy_to`, or `checksum` | unaudited |
| [`read_byte_agrees_with_copy_to_at_every_alignment`][t-readbyte] | per-byte and bulk reads agree | unaudited |
| [Miri job][ci-miri] | `lease::` and `backend::ring::miri` tests under Miri; fails unless at least one passes | unaudited |
| [Valgrind job][ci-valgrind] | `shm-transport --test ring` under memcheck | unaudited |
| [`a_commit_past_the_write_deadline_is_refused`][t-deadline] | `publish_direct` with a slow serializer publishes nothing | unaudited |
| [`commit_after_quarantine_is_refused_and_aborts`][t-commitq] | `commit` on a quarantined ring aborts | unaudited |
| [`into_parts_returns_the_unused_output_reservation`][t-parts] | owned charge shrinks to `len + HEADER_LEN` | unaudited |
| [`into_parts_never_grows_a_charge_below_the_encoded_size`][t-parts2] | a smaller charge stays unchanged | unaudited |
| [`into_parts_does_not_leave_a_large_allocation_behind_a_small_charge`][t-parts3] | `Vec` capacity is at most the charge | unaudited |
| [`cap_error_reports_usage_and_cap_without_poisoning_reads`][t-cap] | `Capacity` carries `usage` and `cap`; reads still work | unaudited |
| [`invalidated_retained_object_still_consumes_cap`][t-retained] | invalidated bytes count against the cap | unaudited |
| [`reclaim_frees_capacity_for_next_write`][t-reclaim] | GC reclaim lowers usage so the next ingest fits | unaudited |
| [`payload_limit_is_inclusive_at_the_artifact_cap`][t-payload] | 64 MiB accepted, 64 MiB + 1 refused with no reservation or temp | unaudited |
| [`ingest_route_accepts_a_payload_at_the_artifact_cap`][t-route-cap] | daemon route at and over `MAX_PAYLOAD_BYTES` | unaudited |
| [`orphan_mtime_grace_and_budget_facts_are_reconciled_from_objects`][t-orphan] | usage is read from objects written outside the store; `warn` at 80 percent | unaudited |
| [`facts_unless_abandons_the_artifact_walk_once_cancelled`][t-cancel] | the walk polls cancellation before every entry | unaudited |
| [`assert_semantic_oracle`][t-oracle] | `artifact_budget_facts().usage_bytes` equals an independent scan after every fault | unaudited |
| [`recover_twice`][t-recover-twice] | a second recovery changes nothing | unaudited |
| [`return_value_fault_table_latches_eio_and_never_publishes_a_reference`][t-faults] | six ingest fault points | unaudited |
| [`purge_and_gc_fault_table_preserves_pending_work_and_converges`][t-gcfaults] | purge unlink fault and three GC fault points | unaudited |
| [`crash_windows_recover_idempotently_and_match_no_crash_execution`][t-crash] | SIGKILL children at reservation, ingest, and purge barriers | unaudited |
| [`startup_retires_a_live_reservation_whose_bytes_are_already_gone`][t-gone] | recovery deletes a reservation with no bytes | unaudited |

None found:

- A test that asserts `arena_reclaimed - punched < punch_batch_bytes` as an
  inequality over a long publish and release sequence, or that measures
  resident pages on an idle host ring.
- Any production reader of `resident_arena_pages` or of process RSS.
- A test of `to_vec` with span lengths that do not sum to `body_len`.
- A test of the direct path with underfill, overflow, a serializer `Err`
  after partial write, or a serializer panic; the `direct_fill` fixture arm
  has no sender.
- A test of the egress charge held by a Direct `OutputBuffer`, or of a queued
  direct frame dropped by generation retirement.
- A test that dedup at exactly the cap is admitted, or that a second ingest
  before recovery of an orphan publish is refused.
- A comparison of two independent usage sources, because only the walk exists.
- Any daemon caller of `run_staging_maintenance` or `delete_artifact`, so the
  GC and purge decrement paths run only in kernel tests and benches.
- A test that the health sampler's lock-free walk and `check_budget`'s locked
  walk agree, or that the sampler's 300 s staleness bound is reached by a slow
  walk.

Suspiciously quiet: the [hardware envelope bench][bench] counts
`page_removal_syscalls` and `body_copies` per operation, so the audit's
per-MiB cost has a measurement channel, but no test consumes those counters
as an oracle.

## Contract-versus-code disagreements

None found in the normative wire contract, which makes no residency,
sparseness, or copy-count claim for the ring and leaves artifact accounting to
the kernel. Five adjacent observations, not disagreements:

- [`MAX_RING_RESIDENT_BYTES`][max-resident] and
  `ProcessLimitsError::ExceedsResidentBytes` say "resident"; the doc comment
  and the arithmetic bound virtual arena bytes. The audit's "resident-bytes
  bound" language inherits the name. [§7.5.1][wire-751] makes the same
  distinction explicit for the Synapse pool.
- [§7.7][wire-77] states that no prefault exists. A deferred-punch design that
  re-touches pages ahead of a write to avoid the re-fault would contradict it;
  one that punches later does not.
- [`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse] asserts
  that a reservation over reclaimed pages reads zero. No contract requires it:
  [`commit`][commit-underfill] refuses unless `cursor == body_len`, so every
  published byte was written, and the memfd is private to one connection. The
  test pins a side effect of eager punching that deferral would change.
- The comment at [`trim:2254-2255`][trim] describes an idle-ring role that no
  shipped code performs; [`trim-removes-only-dead-pages-below-the-write-cursor`][shm-trim]
  already records the absence of a caller.
- The owned path classifies a serializer failure as a request-scoped
  `encode_failed` terminal ([`settle_prepared_with`][settle-with]); the direct
  path at HEAD classifies the same failure as a connection close
  ([`publish_one`][publish-one] to [`fail`][publish-fail]). Neither is
  documented in the wire contract; the specification must pick one before the
  direct path carries transform responses.

## Anchors

[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
[shm-punch]: ../../../shm-transport/catalog.md#reclamation-excludes-pages-with-live-wrapped-bytes
[shm-trim]: ../../../shm-transport/catalog.md#trim-removes-only-dead-pages-below-the-write-cursor
[shm-noref]: ../../../shm-transport/catalog.md#no-rust-reference-over-peer-writable-payload
[hr-terminal]: ../../../host-runtime/catalog.md#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame
[hr-pubfail]: ../../../host-runtime/catalog.md#req-a-a-response-publication-failure-never-reaches-the-settling-path
[agents]: ../../../../../crates/shm-transport/AGENTS.md
[arena-const]: ../../../../../crates/shm-transport/src/arena.rs#L4-L7
[divisor]: ../../../../../crates/shm-transport/src/backend/ring.rs#L48
[removal-ranges]: ../../../../../crates/shm-transport/src/backend/ring.rs#L375-L427
[remove-pages]: ../../../../../crates/shm-transport/src/backend/ring.rs#L433-L441
[try-reserve]: ../../../../../crates/shm-transport/src/backend/ring.rs#L1263-L1340
[release]: ../../../../../crates/shm-transport/src/backend/ring.rs#L1528-L1600
[resident-api]: ../../../../../crates/shm-transport/src/backend/ring.rs#L1894-L1899
[reclaim]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2070-L2151
[punch-decision]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2129-L2134
[live-end]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2153-L2156
[batch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2158-L2161
[punch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2163-L2242
[trim]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2244-L2266
[abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2268-L2304
[prepare-commit]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2306-L2343
[write-res]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2388-L2419
[res-write]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2519-L2531
[commit-underfill]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2533-L2570
[res-drop]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2587-L2594
[madv]: ../../../../../crates/shm-transport/src/backend/sys.rs#L135-L149
[span-doc]: ../../../../../crates/shm-transport/src/lease.rs#L10-L13
[span-safety]: ../../../../../crates/shm-transport/src/lease.rs#L25-L33
[span-ptr]: ../../../../../crates/shm-transport/src/lease.rs#L49-L52
[shape]: ../../../../../crates/shm-transport/src/lease.rs#L134-L156
[copy-out]: ../../../../../crates/shm-transport/src/lease.rs#L176-L206
[copy-in]: ../../../../../crates/shm-transport/src/lease.rs#L208-L235
[to-vec]: ../../../../../crates/shm-transport/src/lease.rs#L328-L348
[to-vec-fill]: ../../../../../crates/shm-transport/src/lease.rs#L331
[max-resident]: ../../../../../crates/host-runtime/src/ring_transport.rs#L57-L58
[affordable]: ../../../../../crates/host-runtime/src/ring_transport.rs#L60-L65
[process-limits]: ../../../../../crates/host-runtime/src/ring_transport.rs#L96-L125
[idle-select]: ../../../../../crates/host-runtime/src/ring_transport.rs#L582-L617
[publish-fail]: ../../../../../crates/host-runtime/src/ring_transport.rs#L622-L646
[receive-to-vec]: ../../../../../crates/host-runtime/src/ring_transport.rs#L731-L736
[publish-one]: ../../../../../crates/host-runtime/src/ring_transport.rs#L749-L786
[publish-direct]: ../../../../../crates/host-runtime/src/ring_transport.rs#L788-L800
[publish-owned]: ../../../../../crates/host-runtime/src/ring_transport.rs#L802-L812
[commit-before]: ../../../../../crates/host-runtime/src/ring_transport.rs#L814-L825
[res-writer]: ../../../../../crates/host-runtime/src/ring_transport.rs#L827-L843
[runtime-limits]: ../../../../../crates/host-runtime/src/runtime.rs#L792-L796
[config-default]: ../../../../../crates/host-runtime/src/config.rs#L78-L83
[config-validate]: ../../../../../crates/host-runtime/src/config.rs#L127-L131
[outbuf]: ../../../../../crates/host-runtime/src/handler.rs#L320-L335
[into-parts]: ../../../../../crates/host-runtime/src/handler.rs#L387-L400
[reserve-output]: ../../../../../crates/host-runtime/src/handler.rs#L461-L463
[from-writer]: ../../../../../crates/host-runtime/src/handler.rs#L465-L472
[emit-reserved]: ../../../../../crates/host-runtime/src/dispatch.rs#L282-L332
[reserve]: ../../../../../crates/host-runtime/src/dispatch.rs#L484-L515
[reserve-direct]: ../../../../../crates/host-runtime/src/dispatch.rs#L517-L554
[direct-frame]: ../../../../../crates/host-runtime/src/frame_channel.rs#L166-L200
[encode-owned]: ../../../../../crates/host-runtime/src/wire.rs#L569-L583
[split-min]: ../../../../../crates/host-runtime/src/wire.rs#L585-L587
[split]: ../../../../../crates/host-runtime/src/wire.rs#L589-L599
[native-reserve]: ../../../../../packages/shm-native/src/lib.rs#L1024
[measure]: ../../../../../crates/daemon/src/dispatch.rs#L129-L148
[write-to]: ../../../../../crates/daemon/src/dispatch.rs#L236-L262
[settle-with]: ../../../../../crates/daemon/src/lib.rs#L12072-L12128
[settle-prepared]: ../../../../../crates/daemon/src/lib.rs#L12130-L12145
[ingest]: ../../../../../crates/kernel/src/cas/ingest.rs#L361-L663
[ingest-temp]: ../../../../../crates/kernel/src/cas/ingest.rs#L379-L405
[ingest-lock]: ../../../../../crates/kernel/src/cas/ingest.rs#L407-L416
[ingest-reservation]: ../../../../../crates/kernel/src/cas/ingest.rs#L424-L471
[ingest-publish]: ../../../../../crates/kernel/src/cas/ingest.rs#L476-L526
[ingest-commit]: ../../../../../crates/kernel/src/cas/ingest.rs#L580-L595
[check-budget]: ../../../../../crates/kernel/src/cas/ingest.rs#L665-L682
[release-res]: ../../../../../crates/kernel/src/cas/ingest.rs#L756-L772
[cleanup]: ../../../../../crates/kernel/src/cas/ingest.rs#L779-L849
[stat-bytes]: ../../../../../crates/kernel/src/cas/ingest.rs#L1206-L1208
[walk]: ../../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
[present]: ../../../../../crates/kernel/src/cas/ingest.rs#L1290-L1296
[startup]: ../../../../../crates/kernel/src/cas/gc.rs#L78-L180
[startup-unreachable]: ../../../../../crates/kernel/src/cas/gc.rs#L109-L136
[reclaim-cand]: ../../../../../crates/kernel/src/cas/gc.rs#L212-L289
[recovery]: ../../../../../crates/kernel/src/cas/gc.rs#L300-L341
[scan-objects]: ../../../../../crates/kernel/src/cas/gc.rs#L579
[unlink-artifact]: ../../../../../crates/kernel/src/cas/gc.rs#L403-L419
[object-usage]: ../../../../../crates/kernel/src/cas/gc.rs#L637-L645
[purge-unlink]: ../../../../../crates/kernel/src/cas/deletion.rs#L532-L556
[sweep-temps]: ../../../../../crates/kernel/src/cas/deletion.rs#L607-L630
[cap-default]: ../../../../../crates/kernel/src/cas/mod.rs#L24
[cap-error]: ../../../../../crates/kernel/src/cas/mod.rs#L318-L325
[tmp-sweep]: ../../../../../crates/kernel/src/cas/mod.rs#L486-L512
[recover]: ../../../../../crates/kernel/src/open.rs#L419-L422
[lock-writer]: ../../../../../crates/kernel/src/open.rs#L433-L442
[facts]: ../../../../../crates/kernel/src/facts.rs#L144-L164
[maintenance]: ../../../../../crates/kernel/src/retention.rs#L190-L211
[delete]: ../../../../../crates/kernel/src/cas/deletion.rs#L237
[restore]: ../../../../../crates/kernel/src/backup.rs#L424
[latch]: ../../../../../crates/kernel/src/cas/mod.rs#L569
[sampler]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L17-L22
[sampler-run]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L214-L263
[busy]: ../../../../../crates/daemon/src/kernel_routes/state.rs#L290-L295
[route-ingest]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L577
[wire-751]: ../../../../host-wire-protocol.md#L440
[wire-77]: ../../../../host-wire-protocol.md#L666
[ci-miri]: ../../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../../.github/workflows/ci.yml#L637-L669
[bench]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L296-L306
[fixture-arm]: ../../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
[t-limits]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1052-L1074
[t-deadline]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1849-L1879
[t-parts]: ../../../../../crates/host-runtime/src/handler.rs#L596-L626
[t-parts2]: ../../../../../crates/host-runtime/src/handler.rs#L628-L645
[t-parts3]: ../../../../../crates/host-runtime/src/handler.rs#L647-L673
[t-syscall]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3086-L3087
[t-commitq]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3198
[t-trim-order]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3319-L3335
[t-abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3667-L3685
[t-batch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4018-L4046
[t-reuse]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4075-L4091
[t-subpage]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4093-L4118
[t-trim-res]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4138-L4164
[t-punchfail]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4245-L4261
[t-roundtrip]: ../../../../../crates/shm-transport/src/lease.rs#L475-L506
[t-readbyte]: ../../../../../crates/shm-transport/src/lease.rs#L543
[t-concurrent]: ../../../../../crates/shm-transport/src/lease.rs#L581-L619
[t-fifo]: ../../../../../crates/shm-transport/tests/ring.rs#L124-L174
[t-sparse]: ../../../../../crates/shm-transport/tests/ring.rs#L230-L273
[t-cap]: ../../../../../crates/kernel/tests/kernel_cas.rs#L418-L433
[t-retained]: ../../../../../crates/kernel/tests/kernel_cas.rs#L436-L450
[t-payload]: ../../../../../crates/kernel/tests/kernel_cas.rs#L214-L234
[t-reclaim]: ../../../../../crates/kernel/tests/kernel_gc.rs#L535-L554
[t-cancel]: ../../../../../crates/kernel/tests/kernel_gc.rs#L556-L591
[t-orphan]: ../../../../../crates/kernel/tests/kernel_gc.rs#L593-L634
[t-oracle]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-recover-twice]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L384-L389
[t-faults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-gone]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L838
[t-crash]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L902-L969
[t-route-cap]: ../../../../../crates/daemon/tests/kernel_routes.rs#L3600-L3629
