# arena-residency-is-bounded-by-admission-and-one-punch-batch

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit charges `madvise(MADV_REMOVE)` per MiB to the hot path and proposes
deferring punches to idle time. The proposal reads the host's
`MAX_RING_RESIDENT_BYTES` as a residency promise. The constant bounds virtual
arena bytes at admission; physical residency below it is an implicit
consequence of when punching runs. Deferral changes that consequence, so the
record fixes both the admission bound and the current implicit residency
bound before either moves.

## Evidence trail

- [`MAX_RING_RESIDENT_BYTES`][max-resident] is `1 << 30` and its comment says
  "sparse ring virtual arena bytes". [`affordable_connections`][affordable]
  divides it by one connection's `arena_bytes` charge;
  [`process_limits`][process-limits] refuses `requested > affordable` with
  `ExceedsResidentBytes`. The host calls it at
  [runtime.rs:792-793][runtime-limits]; [`HostLimits::default`][config-default]
  and [`validate`][config-validate] use the same quotient. Nothing reads
  residency; [`resident_arena_pages`][resident-api] is a `mincore` probe
  documented for tests.
- The profile is [`host_test_ring_profile`][profile] with
  `arena_bytes: MIN_ARENA_BYTES`, which is [64 MiB][arena-const]; the charge
  doubles it for two directions ([profile.rs:155-158][charge]), so eight
  connections fit under 1 GiB.
- [`try_reserve`][try-reserve] calls `reclaim_completed` at
  [`:1281`][try-reserve] on every reservation.
  [`reclaim_completed_inner`][reclaim] walks released slots, and at
  [`:2129-2134`][punch-decision] punches only when
  `new_reclaimed - punched >= punch_batch_bytes()`.
  [`punch_batch_bytes`][batch] is `arena_bytes / PUNCH_BATCH_DIVISOR` with
  [divisor 4][divisor], so 16 MiB;
  [`punch_dead_pages`][punch] leaves `punched` page-aligned below `reclaimed`
  ([`:2240`][punch]), so the boundary page stays until the next batch.
- [`removal_ranges`][removal-ranges] rounds inward to whole pages and splits
  at the arena end; [`remove_pages`][remove-pages] carries the `SAFETY`
  comment "page-aligned range inside the live shared mapping with no live
  byte in it", discharged by [`madvise_remove`][madv]'s contract. The live end
  is [`live_end`][live-end], `reserved_end` or `arena_write`.
- [`abort_reservation`][abort] punches `[arena_write, reserved_end)` through
  `punch_range`, again rounded inward, so a page shared with live bytes stays.
- [`trim`][trim] punches every dead page including the partial one; its
  comment at [`:2254-2255`][trim] names the idle-ring role. Only tests call it
  ([ring.rs:3086][t-syscall] and siblings); the client crate has no caller and
  runs the same [`reserve_until`][native-reserve] path.
- [§7.7][wire77] states no timed ring poll or prefault exists; [§7.5.1][wire751]
  calls the Synapse cap an accounting boundary, not an RSS claim.
- Tests:
  [`process_limits_reject_counts_above_the_resident_byte_ceiling`][t-limits];
  [`unaligned_batch_boundaries_do_not_strand_pages`][t-batch] publishes
  `batch + 100` bytes and asserts one resident page, then removes it on the
  next batch;
  [`aborted_reservation_leaves_no_resident_pages`][t-abort];
  [`reclaimed_pages_leave_residency_and_reuse_as_zeroes`][t-reuse];
  [`subpage_releases_stay_resident_until_trim`][t-subpage];
  [`page_removal_failure_quarantines_before_capacity_publication`][t-punchfail].
  The [hardware envelope bench][bench] counts `page_removal_syscalls`.

## Failure scenario

A deferred-punch design keeps the admission arithmetic but stops punching in
`try_reserve`. The dead-byte inequality silently fails: an idle ring after a
burst holds up to its whole 64 MiB per direction resident with nothing to
observe it, because no production code reads residency. Alternatively the
design adds a residency check keyed on `MAX_RING_RESIDENT_BYTES`, a new
invariant the constant never promised, with no existing oracle and a name
that already misleads.

## Timing windows and dependencies

Punching is coupled to the next `try_reserve`, so after a burst the ring keeps
up to one batch plus a boundary page resident until the next publish; an idle
ring re-establishes nothing. `trim` would, but nothing calls it. The bound is
per producer handle and per direction; the peer's direction is governed by
the same crate in the client process, so a host-only change alters one side.
`removal_ranges` rounds inward, so the "no page of the aborted range" clause
holds for pages wholly inside the range; the page shared with live bytes is
legitimately retained.

## What a test must construct

A long publish-and-release run on one ring with `resident_arena_pages` read
after every `try_reserve`, asserting
`arena_reclaimed - punched < punch_batch_bytes()` and resident pages at most
one batch plus one page beyond the live span; a wrapped run so
`removal_ranges` splits; a reservation written then aborted; and a
`max_connections` above `affordable_connections()` at startup. The
[ring checks](../existing-checks.md#ring-arena-and-direct-frame) cover each
piece once; none asserts the inequality over a run or measures an idle ring.

## Investigation log

### Q: Does the specification intend a physical residency bound at all?

- Sources examined: [`MAX_RING_RESIDENT_BYTES`][max-resident] and its
  comment, [`process_limits`][process-limits], [§7.5.1][wire751], the
  absence of any production `resident_arena_pages` reader.
- Findings: Every checked bound is on virtual arena bytes. The wire contract
  makes no residency claim for the ring. The name says "resident"; the
  arithmetic does not.
- Missing evidence: The specification's stated intent.
- Conclusion: needs human input.

### Q: What is the replacement bound when punching is deferred?

- Sources examined: [`punch_batch_bytes`][batch], [`punch_dead_pages`][punch],
  [`trim`][trim], the [endpoint loop][idle-select].
- Findings: The code offers three candidate shapes: bytes since the last
  punch (the current batch), pages (what `mincore` reports), or an event
  ("punched by the next idle point" through `trim`). None is chosen.
- Missing evidence: A design decision.
- Conclusion: needs human input.

### Q: Does an idle-time punch count as a timed ring activity under §7.7?

- Sources examined: [§7.7][wire77]; the [`select!`][idle-select] in the
  endpoint loop.
- Findings: §7.7 forbids a timed ring poll and any prefault. A punch removes
  pages rather than touching them, and the loop already has an idle branch
  without a timer. Whether an idle punch counts as "timed" or as a "fallback
  path" is a reading of the contract, not of the code.
- Missing evidence: A protocol owner's reading.
- Conclusion: needs human input.

[max-resident]: ../../../../../crates/host-runtime/src/ring_transport.rs#L57-L58
[affordable]: ../../../../../crates/host-runtime/src/ring_transport.rs#L60-L65
[process-limits]: ../../../../../crates/host-runtime/src/ring_transport.rs#L96-L125
[idle-select]: ../../../../../crates/host-runtime/src/ring_transport.rs#L582-L617
[t-limits]: ../../../../../crates/host-runtime/src/ring_transport.rs#L1052-L1074
[runtime-limits]: ../../../../../crates/host-runtime/src/runtime.rs#L792-L796
[config-default]: ../../../../../crates/host-runtime/src/config.rs#L78-L83
[config-validate]: ../../../../../crates/host-runtime/src/config.rs#L127-L131
[profile]: ../../../../../crates/shm-transport/src/profile.rs#L683-L697
[charge]: ../../../../../crates/shm-transport/src/profile.rs#L155-L158
[arena-const]: ../../../../../crates/shm-transport/src/arena.rs#L4-L7
[divisor]: ../../../../../crates/shm-transport/src/backend/ring.rs#L48
[removal-ranges]: ../../../../../crates/shm-transport/src/backend/ring.rs#L375-L427
[remove-pages]: ../../../../../crates/shm-transport/src/backend/ring.rs#L433-L441
[try-reserve]: ../../../../../crates/shm-transport/src/backend/ring.rs#L1263-L1340
[resident-api]: ../../../../../crates/shm-transport/src/backend/ring.rs#L1894-L1899
[reclaim]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2070-L2151
[punch-decision]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2129-L2134
[live-end]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2153-L2156
[batch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2158-L2161
[punch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2163-L2242
[trim]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2244-L2266
[abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L2268-L2304
[t-syscall]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3086-L3087
[t-abort]: ../../../../../crates/shm-transport/src/backend/ring.rs#L3677-L3693
[t-batch]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4016-L4044
[t-reuse]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4073-L4088
[t-subpage]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4091-L4115
[t-punchfail]: ../../../../../crates/shm-transport/src/backend/ring.rs#L4243-L4258
[madv]: ../../../../../crates/shm-transport/src/backend/sys.rs#L135-L149
[native-reserve]: ../../../../../packages/shm-native/src/lib.rs#L1024
[bench]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L296-L306
[wire751]: ../../../../host-wire-protocol.md#L440
[wire77]: ../../../../host-wire-protocol.md#L666
