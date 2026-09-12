# artifact-byte-decrement-paths-are-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

A durable usage counter must be decremented on every path that removes object
bytes. Two of those paths run in production only under a failure, and the
other two have no production caller at all, so a counter validated against the
walk in production traffic would ship having never been decremented by a
GC reclaim or a purge. This record asks a campaign to reach each path with
bytes at stake, not to check the accounting.

## Evidence trail

- The five unlink sites, each removing regular-file bytes under `objects`:
  - [`cleanup_failed_reference`][cleanup] unlinks at
    [`:831-841`][cleanup-unlink] when `published_new` is true and no reference
    or reservation protects the digest. It runs after a
    [`PublishOutcome::Published`][ingest-publish] when the directory sync,
    `verify_object`, the injected `after_directory_sync` fault, the CAS latch,
    or the reference commit fails ([`:528-572`][ingest-postpublish],
    [`:645-652`][ingest-commit-fail]).
  - [`run_artifact_recovery`][recovery] promotes orphan `Live` rows through
    [`prepare_startup_cas_recovery`][startup] and runs `reclaim_candidate`
    over them, which reaches [`unlink_artifact`][unlink-artifact].
  - [`reclaim_candidate`][reclaim-cand] unlinks at [`:260`][reclaim-cand]
    after `prepare_reclaim` accepts a candidate past its grace; it is reached
    from [`run_artifact_gc`][run-gc], which
    [`run_staging_maintenance`][maintenance] calls after its own transaction.
  - [`complete_pending_purge_locked`][purge-unlink] unlinks a purged digest
    and sweeps its temps; it is reached from [`delete_artifact`][delete].
  - A GC unlink that fails: [`unlink_artifact`][unlink-artifact] maps a
    `durable_unlink` error through `map_gc_storage_error`, and
    [`run_artifact_gc`][run-gc] counts it in `failed_candidates` and
    continues; the candidate stays for the next pass.
- Daemon callers: [`ingest_artifact`][route-ingest] is the only CAS mutation
  the daemon issues. `run_staging_maintenance` and `delete_artifact` are
  called from kernel tests, the daemon's benches, and the daemon's tests, not
  from `crates/daemon/src`.
- Faults and windows the tests already drive:
  [`ArtifactIngestFault::AfterEvents`][t-faults] fails the reference commit
  after a publish; [`INGEST_CRASH_POINT`][t-crash-point] is
  `ingest.070.object.rename.after` and the [crash test][t-crash] kills a child
  there and at the post-reservation and post-purge-commit barriers;
  [`ArtifactGcFault::Unlink`][t-gcfaults] is the `gc.020.object.unlink.before`
  point; [`reclaim_frees_capacity_for_next_write`][t-reclaim] ages an
  invalidated reference past [`RESERVATION_MS`][reservation-ms] (one hour).
- Every one of those tests asserts state convergence through
  [`assert_semantic_oracle`][t-oracle]; none records usage before and after a
  single decrement path.

## Failure scenario

Not a violation; a coverage gap. A counter decremented in `cleanup` and in
recovery, and validated in production, passes every observation while the GC
and purge paths, which no production code reaches, never decrement it. The
first operator-run GC then leaves the counter above the disk, and
`check_budget` refuses valid ingests.

## Timing windows and dependencies

The five paths are reached from different states: a publish followed by a
commit failure, a crash after the rename, an invalidated reference aged past
its grace, a purge request, and an unlink failure with a retry. Each unlink
must remove a non-zero `st_size`; `unlink_artifact` returns `(false, 0)` when
the object is already gone, and that run exercises no accounting.

## What a test must construct

For each path, an object of known non-zero size, a usage reading before, the
path's enabling fault or state, and a usage reading after with the writer
lock released, asserting the delta equals the object's size:
`AfterEvents` after a new publish; a SIGKILL child at `INGEST_CRASH_POINT`
followed by a reopen; an invalidated reference aged past `RESERVATION_MS`
then `run_staging_maintenance`; a purge through `delete_artifact`; and
`ArtifactGcFault::Unlink` followed by a second pass that succeeds. The marker
records the path name and the delta. The
[CAS fault tables](../existing-checks.md#cas-usage-accounting) drive the
faults and windows; none records a per-path delta.

The usage reading is the independent `st_size` sum over `objects`
(`regular_file_bytes`, `crates/kernel/src/cas/ingest.rs:1228-1288`), never a
counter under test, and each path has its own constant marker:
`artifact-byte-decrement-paths-are-exercised-cleanup`, `-recovery`,
`-reclaim`, `-purge`, and `-retry`.

## Investigation log

The catalog record lists no open questions. One question was checked while
enumerating the paths.

### Q: Are there unlink sites for object bytes beyond the five listed?

- Sources examined: `durable_unlink` call sites in `crates/kernel/src/cas/`
  ([`cleanup_failed_reference`][cleanup-unlink],
  [`unlink_artifact`][unlink-artifact], [`unlink_purged_artifact`][purge-object]
  reached from [`complete_pending_purge_locked`][purge-unlink]), plus the temp
  unlinks in [`ingest_artifact_inner`][ingest-publish] and the staged-object
  drop.
- Findings: The remaining `durable_unlink` calls remove files under `tmp`,
  which [`regular_file_bytes`][walk] never counts. Recovery unlinks through
  `reclaim_candidate`, so the five paths reduce to three unlink functions
  reached from five operational states.
- Missing evidence: None at HEAD.
- Conclusion: resolved with answer - the five paths are complete for
  `objects` bytes; temp unlinks are outside the counted set.

[ingest-publish]: ../../../../../crates/kernel/src/cas/ingest.rs#L476-L526
[ingest-postpublish]: ../../../../../crates/kernel/src/cas/ingest.rs#L528-L572
[ingest-commit-fail]: ../../../../../crates/kernel/src/cas/ingest.rs#L645-L652
[cleanup]: ../../../../../crates/kernel/src/cas/ingest.rs#L779-L849
[cleanup-unlink]: ../../../../../crates/kernel/src/cas/ingest.rs#L831-L841
[walk]: ../../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
[reservation-ms]: ../../../../../crates/kernel/src/cas/ingest.rs#L36
[startup]: ../../../../../crates/kernel/src/cas/gc.rs#L78-L180
[run-gc]: ../../../../../crates/kernel/src/cas/gc.rs#L187-L210
[reclaim-cand]: ../../../../../crates/kernel/src/cas/gc.rs#L212-L289
[recovery]: ../../../../../crates/kernel/src/cas/gc.rs#L300-L341
[unlink-artifact]: ../../../../../crates/kernel/src/cas/gc.rs#L403-L419
[purge-unlink]: ../../../../../crates/kernel/src/cas/deletion.rs#L532-L556
[purge-object]: ../../../../../crates/kernel/src/cas/deletion.rs#L589-L599
[delete]: ../../../../../crates/kernel/src/cas/deletion.rs#L237
[maintenance]: ../../../../../crates/kernel/src/retention.rs#L186-L211
[route-ingest]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L577
[t-oracle]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-faults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-crash-point]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L34
[t-crash]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L924-L990
[t-reclaim]: ../../../../../crates/kernel/tests/kernel_gc.rs#L535-L554