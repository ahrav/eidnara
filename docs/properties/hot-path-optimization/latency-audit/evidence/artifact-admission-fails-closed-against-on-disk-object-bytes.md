# artifact-admission-fails-closed-against-on-disk-object-bytes

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit charges a full walk of the object tree to every ingest and proposes
a durable usage counter. The walk is not a cost to remove without replacing
what it computes: the admission decision reads the filesystem under the same
lock that publishes, so it cannot admit against a stale figure. A counter must
preserve that, or state what it trades away.

## Evidence trail

- [`ingest_artifact_inner`][ingest] writes and syncs the payload to a temp
  under `tmp` ([`:379-405`][ingest-temp]) before any lock; a
  [`StagedObject`][staged] unlinks it on drop unless consumed.
- It then takes the exclusive [writer lock][lock-writer] and calls
  [`check_budget`][check-budget] at [`:407-410`][ingest-lock], before the
  shard is created ([`:411-416`][ingest-lock]), before the reservation row
  ([`:424-471`][ingest-reservation]), and before the publish rename
  ([`:476-526`][ingest-publish]).
- [`check_budget`][check-budget] calls [`regular_file_bytes`][walk] with an
  uncancellable predicate, adds `byte_length` unless
  [`object_is_present`][present] finds a regular file at the digest's path,
  and refuses with [`ArtifactError::capacity(usage, cap)`][cap-error] when
  `projected > artifact_cap`.
- [`regular_file_bytes`][walk] reads `objects` and each immediate shard
  directory with `statat(..., SYMLINK_NOFOLLOW)`, sums [`st_size`][stat-bytes]
  of regular files, skips entries that vanish mid-walk, and saturates. It
  reads no table, so an invalidated reference whose bytes remain still
  counts.
- [`DEFAULT_ARTIFACT_CAP`][cap-default] is 4 GiB.
- The daemon's route calls [`store.ingest_artifact`][route-ingest] and maps a
  retriable error through [`KernelOutcome::from`][busy], where `Capacity`
  becomes `StoreBusy`; [`is_retriable`][retriable] lists `Capacity`.
- The health sampler reads usage through
  [`facts_unless`][sampler-facts] every [30 s][sampler], which reaches
  [`object_usage`][object-usage] and the same walk without the writer lock.
- Tests: [`cap_error_reports_usage_and_cap_without_poisoning_reads`][t-cap],
  [`invalidated_retained_object_still_consumes_cap`][t-retained],
  [`reclaim_frees_capacity_for_next_write`][t-reclaim],
  [`payload_limit_is_inclusive_at_the_artifact_cap`][t-payload], and the
  daemon's [`ingest_route_accepts_a_payload_at_the_artifact_cap`][t-route-cap].

## Failure scenario

A counter maintained outside the writer lock lags the disk by whatever
publishes or unlinks landed since its last update, and `check_budget` admits
over the cap by that lag. A counter that ignores invalidated-but-retained
objects admits the bytes GC has not yet reclaimed. A refusal that runs after
the reservation insert leaves a `Live` row for a digest with no bytes, which
startup recovery then has to retire. A dedup hit at exactly the cap must be
admitted with zero added bytes; a counter that adds `byte_length` before
checking presence refuses it.

## Timing windows and dependencies

Two ingests serialize on the writer lock, so the locked walk cannot race a
concurrent publish or a GC unlink, both of which also hold the lock. It does
race the sampler's lock-free walk, which is why the sampler's figure is
informational. A refused ingest has already paid the temp write and sync; the
refusal costs one unlink. A crash after the publish rename and before the
reference commit leaves bytes the walk counts, so a second ingest before
recovery is refused against usage that includes the orphan, which is the
fail-closed direction.

## What a test must construct

A store at `cap - 1` receiving a two-byte payload, asserting `Capacity` with
`usage` and `cap`, no reservation row, no shard entry, and no temp; the same
digest re-ingested at exactly the cap, asserting admission; an invalidated
reference whose bytes remain; a crash between publish and reference commit
followed by a second ingest before recovery. The
[CAS checks](../existing-checks.md#cas-usage-accounting) cover the cap error,
retained bytes, reclaim, and the inclusive limit; none covers dedup at the cap
or a refusal after an unrecovered orphan publish.

## Investigation log

### Q: Is `StoreBusy` the intended classification for a cap refusal?

- Sources examined: [`KernelOutcome::from`][busy], [`is_retriable`][retriable],
  the callers of [`run_staging_maintenance`][maintenance] and
  [`delete_artifact`][delete].
- Findings: `Capacity` is retriable and maps to `StoreBusy`. The only
  production paths that lower usage are failed-ingest cleanup and startup
  recovery; `run_staging_maintenance` and `delete_artifact` are called from
  kernel tests and the daemon's benches and tests, not from the daemon's
  source. A cap reached in production is relieved by a restart or an
  operator, not by waiting.
- Missing evidence: A stated policy on the retry class.
- Conclusion: needs human input.

[ingest]: ../../../../../crates/kernel/src/cas/ingest.rs#L361-L663
[staged]: ../../../../../crates/kernel/src/cas/ingest.rs#L38-L56
[ingest-temp]: ../../../../../crates/kernel/src/cas/ingest.rs#L379-L405
[ingest-lock]: ../../../../../crates/kernel/src/cas/ingest.rs#L407-L416
[ingest-reservation]: ../../../../../crates/kernel/src/cas/ingest.rs#L424-L471
[ingest-publish]: ../../../../../crates/kernel/src/cas/ingest.rs#L476-L526
[check-budget]: ../../../../../crates/kernel/src/cas/ingest.rs#L665-L682
[stat-bytes]: ../../../../../crates/kernel/src/cas/ingest.rs#L1206-L1208
[walk]: ../../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
[present]: ../../../../../crates/kernel/src/cas/ingest.rs#L1290-L1296
[object-usage]: ../../../../../crates/kernel/src/cas/gc.rs#L637-L645
[cap-default]: ../../../../../crates/kernel/src/cas/mod.rs#L24
[retriable]: ../../../../../crates/kernel/src/cas/mod.rs#L299-L307
[cap-error]: ../../../../../crates/kernel/src/cas/mod.rs#L318-L325
[lock-writer]: ../../../../../crates/kernel/src/open.rs#L433-L442
[maintenance]: ../../../../../crates/kernel/src/retention.rs#L186-L211
[delete]: ../../../../../crates/kernel/src/cas/deletion.rs#L237
[busy]: ../../../../../crates/daemon/src/kernel_routes/state.rs#L290-L295
[route-ingest]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L577
[sampler]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L17-L22
[sampler-facts]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L226
[t-cap]: ../../../../../crates/kernel/tests/kernel_cas.rs#L418-L433
[t-retained]: ../../../../../crates/kernel/tests/kernel_cas.rs#L436-L450
[t-payload]: ../../../../../crates/kernel/tests/kernel_cas.rs#L214-L234
[t-reclaim]: ../../../../../crates/kernel/tests/kernel_gc.rs#L535-L554
[t-route-cap]: ../../../../../crates/daemon/tests/kernel_routes.rs#L3600-L3629