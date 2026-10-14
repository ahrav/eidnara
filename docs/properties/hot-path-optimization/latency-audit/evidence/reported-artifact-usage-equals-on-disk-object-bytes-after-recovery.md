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
- Conclusion: needs human input. Superseded by the decision entry below.

### Q: Which file set does a counter follow: the walk's or `scan_objects`'s?

- Sources examined: [`regular_file_bytes`][walk] and
  [`scan_objects`][scan-objects].
- Findings: The walk counts a root-level regular file and every regular file
  in every immediate subdirectory, whatever its name. `scan_objects` skips
  shards whose names are not two hex digits. A stray file counts toward the
  cap but is never a GC candidate; [`assert_semantic_oracle`][t-oracle] flags
  such entries as anomalies.
- Missing evidence: A decision on which definition a counter follows.
- Conclusion: needs human input. Superseded by the decision entry below:
  the count follows the walk's file set.

### Q: Where does the count live, and what happens on drift? (decision at the usage-count head)

- Sources examined: [`usage.rs`][usage-count], the [open seed][open-seed],
  [`recover_interrupted_work`][recover-reconcile], the
  [maintenance pass][maintenance-reconcile], and the
  [usage tests][t-usage].
- Findings: The kernel store has no spare mutable column, and a new table or
  column is a format break, so the specification's durable counter updated in
  the same transaction is replaced by a process-local count. The count is an
  `Option<u64>` under its own mutex and is changed only while the writer lock
  is held. The open walk seeds it before startup recovery. Each publication
  adds the published length ([`count_published_artifact`][usage-publish]).
  Each object unlink goes through [`unlink_counted_object`][usage-unlink],
  which subtracts the `st_size` read before the unlink once the durable
  unlink succeeds; a failed unlink leaves the count unknown, and a removal
  larger than the count leaves it unknown and latches.
  [`reconcile_artifact_usage`][usage-reconcile] walks under the writer lock
  at the end of every recovery (open and restore) and every maintenance
  pass. With no count it seeds one. With a
  count it compares, and a disagreement latches the CAS fail-closed state
  and replaces the count with the walk, so the health sampler reports the
  bytes on disk. The drift action is the latch: the next admission is
  refused with `IngestionFailClosed`, and only a new store, which re-seeds
  from the tree, admits again. The walk follows
  [`regular_file_bytes`][walk-head]'s file set, so a stray root-level file
  counts and its later appearance or disappearance is drift. The count is a
  `Mutex<Option<u64>>` of about 24 bytes and replaces the cached
  `Option<u64>` and generation the store already held, so the declared
  retained-resident total is unchanged.
- Missing evidence: None for the decision. Drift between a restore's
  install and its reconciliation is detected only at that reconciliation.
- Conclusion: resolved with answer - a writer-lock count seeded by the open
  walk, reconciled by recovery and maintenance walks, and latched closed on
  disagreement.

### Q: Does the count equal the walk on every path?

- Sources examined: the [usage tests][t-usage] and
  [`assert_semantic_oracle`][t-oracle-head].
- Findings: The usage tests compare `artifact_budget_facts().usage_bytes`
  with an independent `std::fs` walk after publish, dedup at the cap,
  refusal, cleanup, an orphan publish, startup recovery, reclaim, purge, a
  failed unlink, and its retry, and they assert that a file added or removed
  outside the store makes the next maintenance pass latch so the next ingest
  is `IngestionFailClosed`. A batch publish, a refused batch, an
  underflowing purge, and a restore are compared the same way. Negative
  controls, run locally and reverted: dropping the unlink subtraction fails six tests, dropping the publication
  addition fails nine, dropping the drift latch fails both drift tests,
  dropping the underflow latch fails the underflow test, and dropping the
  batch publication addition fails the batch test. A clean open takes one
  walk and each maintenance pass one, as the next entry describes.
- Missing evidence: None.
- Conclusion: resolved with answer - yes, at every quiescent point the tests
  reach.

### Q: What does the census cost at open, and does a concurrent census keep the count equal to the walk?

- Sources examined: the [open census][open-census],
  [`seed_artifact_usage`][usage-seed], the [census thread][usage-census],
  [`regular_file_bytes_concurrently`][walk-concurrent], the
  [usage tests][t-usage], and a local probe (release build, a store of
  100,000 objects in 256 shards, database on tmpfs and objects on XFS over
  EBS, interleaved against the parent commit; uncommitted probe numbers,
  not approved bench evidence).
- Findings: Each object costs one `statat`. Single-threaded, that is about
  2.1 us per object warm on the probe host, whose syscall path carries audit
  and SELinux hooks, and one device read per inode cluster cold (about
  5,500 reads for the store, at about 610 us each on EBS). The census now
  divides the shard directories among up to 64 threads, a count sized for
  reads in flight, and at open it runs on its own thread beside the database
  open. Until `open` returns, the opening thread is the store's only user, so
  the census counts the tree the writer lock would have seen, and the seed settles it through the same path as a
  reconciliation: a count held and disagreeing latches, an exhaustion
  leaves the count unknown, and any other failure latches. A failure other
  than exhaustion from any worker outranks an exhaustion from another. The
  cancellable fallback walk the sampler takes with no count held stays on
  the caller's thread and polls before every entry as before. Probe: warm
  open fell from about 186 ms to about 31 ms and cold open from about
  3,030 ms to about 131 ms on 32 processors (about 836 ms to 222 ms on
  four processors against the 16-worker intermediate); the fallback walk,
  ingest, and the held-count read stayed within noise. On one processor a
  warm open of a 4,000-object store pays about 3 ms more for the thread
  starts. The census threads and their 16 KiB directory buffers exist only
  while the walk runs, so the retained-resident total is unchanged. Negative
  controls, run locally and reverted: dropping the workers' sums, dropping
  the calling thread's share, skipping every other shard, and dropping
  top-level files each fail
  `the_open_census_sums_every_shard_when_shards_outnumber_its_workers`
  with other usage tests.
- Missing evidence: Measurements on local NVMe and on hosts without audit;
  the probe host is the only one measured.
- Conclusion: resolved with answer - the concurrent census counts the same
  file set as the walk it replaces and settles it through the same latch, so
  every equality the usage tests assert still holds.

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
[usage-count]: ../../../../../crates/kernel/src/cas/usage.rs#L25-L32
[usage-publish]: ../../../../../crates/kernel/src/cas/usage.rs#L36-L40
[usage-unlink]: ../../../../../crates/kernel/src/cas/usage.rs#L48-L73
[usage-reconcile]: ../../../../../crates/kernel/src/cas/usage.rs#L81-L121
[usage-reported]: ../../../../../crates/kernel/src/cas/usage.rs#L99-L107
[open-seed]: ../../../../../crates/kernel/src/open.rs#L432
[recover-reconcile]: ../../../../../crates/kernel/src/open.rs#L459-L464
[maintenance-reconcile]: ../../../../../crates/kernel/src/retention.rs#L222-L223
[t-usage]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs
[walk-head]: ../../../../../crates/kernel/src/cas/ingest.rs#L1520-L1684
[walk-concurrent]: ../../../../../crates/kernel/src/cas/ingest.rs#L1542-L1594
[open-census]: ../../../../../crates/kernel/src/open.rs#L319
[usage-seed]: ../../../../../crates/kernel/src/cas/usage.rs#L88-L108
[usage-census]: ../../../../../crates/kernel/src/cas/usage.rs#L169-L190
[t-oracle-head]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
