# reported-artifact-usage-equals-on-disk-object-bytes-after-recovery

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

A durable usage counter replaces a walk that is correct by construction with
a figure that can drift. Every path that changes object bytes is a filesystem
operation beside a SQLite transaction, not inside one, so a counter written in
the transaction is wrong in one crash window or the other until recovery
reconciles it. The record fixes the oracle: at quiescence, whatever the store
reports equals the disk.

## Evidence trail

- Reported usage is [`artifact_budget_facts`][facts], which reaches
  [`object_usage`][object-usage] and [`regular_file_bytes`][walk]; the same
  walk backs [`check_budget`][check-budget]. Only one usage source exists.
- `KernelStore::open` runs [`recover_interrupted_work`][recover] at
  [open.rs:406][open-recover], which calls [`run_artifact_recovery`][recovery].
- Publish: the reservation row commits at [`:424-471`][ingest-reservation],
  the rename lands at [`:476-526`][ingest-publish], and the reference commits
  at [`:580-595`][ingest-commit]. Between the rename and the reference commit
  the bytes are on disk with a `Live` row and no reference.
- Unlink paths, each inside a fenced write with the writer lock held:
  [`cleanup_failed_reference`][cleanup] unlinks a newly published object when no
  reference or reservation protects it; [`reclaim_candidate`][reclaim-cand]
  reads the size in [`unlink_artifact`][unlink-artifact] and unlinks at
  [`:212-289`][reclaim-cand] before its row deletes commit at
  [`:212-289`][reclaim-cand]; [`complete_pending_purge_locked`][purge-unlink]
  unlinks the object and its temps before deleting the pending row. A dedup hit
  or a failed publish only [releases the row][release-res].
- Recovery: [`prepare_startup_cas_recovery`][startup] promotes `Live` rows
  with no reference to `Reclaiming`, retires `Live` rows whose bytes are gone
  ([`:109-136`][startup-unreachable]), and re-arms tombstones whose bytes
  remain; [`run_artifact_recovery`][recovery] then runs `reclaim_candidate`
  over `Reclaiming` rows and pending unlinks.
- Two sources bypass any counter: the [restore path][restore] installs a
  backup and runs recovery, and
  [`orphan_mtime_grace_and_budget_facts_are_reconciled_from_objects`][t-orphan]
  writes objects directly under `objects` and asserts usage 8 of cap 10 with
  `warn` set.
- The sampler's [`facts_unless`][sampler-facts] walks without the writer
  lock and reports a value that can include a published-but-uncommitted
  object.
- [`assert_semantic_oracle`][t-oracle] reopens the store, reads
  `artifact_budget_facts().usage_bytes`, scans the tree independently, and
  asserts equality plus no stray entries and no resurrected tombstone;
  [`recover_twice`][t-recover-twice] asserts a second recovery changes nothing.
  The [ingest fault table][t-faults] drives six fault points, the [purge and GC
  table][t-gcfaults] four, and
  [`crash_windows_recover_idempotently_and_match_no_crash_execution`][t-crash]
  kills children at the post-rename, post-purge-commit, and post-reservation
  barriers.

## Failure scenario

A counter incremented in the reference-commit transaction under-reports
between the rename and the commit; if the process dies there, recovery
unlinks the orphan and the counter never saw the bytes, which is correct by
accident. A counter decremented in the unlink transaction over-reports if the
process dies after the unlink and before the commit; recovery retires the row
but has no counter to fix, so the store refuses valid ingests forever. A
counter that never sees the restore path or an object written directly to
disk diverges permanently. In each case `check_budget` admits over the cap or
refuses under it.

## Timing windows and dependencies

Quiescence is the writer lock released after `open`, an ingest, a cleanup, a
GC pass, or a purge. Inside a window the disk and any counter legitimately
disagree; the walk is the reconciliation, so the equality is claimed only at
quiescence. The sampler is excluded because it does not hold the lock.

## What a test must construct

Keep the walk as the oracle. Run the six ingest and three GC fault points
and the three crash barriers, and after each recovery compare the counter,
`artifact_budget_facts().usage_bytes`, and an independent `st_size` sum over
`objects`; add an object written beside the store and a restore, and compare
again. The [CAS checks](../existing-checks.md#cas-usage-accounting) already
assert the walk against an independent scan; none compares two store-side
sources because only one exists.

## Investigation log

### Q: Is the walk retained, and what is the fail-closed action on drift?

- Sources examined: [`check_budget`][check-budget], [`facts`][facts],
  [`latch_cas_failure`][latch], the [restore path][restore].
- Findings: The code has one source and no drift action because there is
  nothing to drift from. Three actions are available in the code shape:
  refuse ingest, latch CAS ingestion closed, or adopt the walk value.
- Missing evidence: A design decision.
- Conclusion: needs human input.

### Q: Which file set does a counter follow: the walk's or `scan_objects`'s?

- Sources examined: [`regular_file_bytes`][walk] and
  [`scan_objects`][scan-objects].
- Findings: The walk counts a root-level regular file and every regular file
  in every immediate subdirectory, whatever its name. `scan_objects` skips
  shards whose names are not two hex digits. A stray file counts toward the
  cap but is never a GC candidate; [`assert_semantic_oracle`][t-oracle] flags
  such entries as anomalies.
- Missing evidence: A decision on which definition a counter follows.
- Conclusion: needs human input.

[ingest-reservation]: ../../../../../crates/kernel/src/cas/ingest.rs#L424-L471
[ingest-publish]: ../../../../../crates/kernel/src/cas/ingest.rs#L476-L526
[ingest-commit]: ../../../../../crates/kernel/src/cas/ingest.rs#L580-L595
[check-budget]: ../../../../../crates/kernel/src/cas/ingest.rs#L665-L682
[release-res]: ../../../../../crates/kernel/src/cas/ingest.rs#L756-L772
[cleanup]: ../../../../../crates/kernel/src/cas/ingest.rs#L779-L849
[walk]: ../../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
[startup]: ../../../../../crates/kernel/src/cas/gc.rs#L78-L180
[startup-unreachable]: ../../../../../crates/kernel/src/cas/gc.rs#L109-L136
[reclaim-cand]: ../../../../../crates/kernel/src/cas/gc.rs#L212-L289
[recovery]: ../../../../../crates/kernel/src/cas/gc.rs#L300-L341
[unlink-artifact]: ../../../../../crates/kernel/src/cas/gc.rs#L403-L419
[scan-objects]: ../../../../../crates/kernel/src/cas/gc.rs#L579
[object-usage]: ../../../../../crates/kernel/src/cas/gc.rs#L637-L645
[purge-unlink]: ../../../../../crates/kernel/src/cas/deletion.rs#L532-L556
[latch]: ../../../../../crates/kernel/src/cas/mod.rs#L569
[open-recover]: ../../../../../crates/kernel/src/open.rs#L406
[recover]: ../../../../../crates/kernel/src/open.rs#L419-L422
[facts]: ../../../../../crates/kernel/src/facts.rs#L144-L164
[restore]: ../../../../../crates/kernel/src/backup.rs#L424
[sampler-facts]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L226
[t-oracle]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-recover-twice]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L384-L389
[t-faults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-crash]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L902-L969
[t-orphan]: ../../../../../crates/kernel/tests/kernel_gc.rs#L593-L634
