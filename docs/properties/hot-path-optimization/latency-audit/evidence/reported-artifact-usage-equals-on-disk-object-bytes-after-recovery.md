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
  [`reported_artifact_usage`][object-usage]: the count when one is held, and
  otherwise [`regular_file_bytes`][walk]; [`check_budget`][check-budget] reads
  the same count under the writer lock. Only one usage source exists.
- `KernelStore::open` runs [`recover_interrupted_work`][recover] at
  [open.rs:440][open-recover], which calls [`run_artifact_recovery`][recovery].
- Publish: the reservation row commits at [`:427`][ingest-reservation],
  the rename lands at [`:479`][ingest-publish], and the reference commits
  at [`:583`][ingest-commit]. Between the rename and the reference commit
  the bytes are on disk with a `Live` row and no reference.
- Unlink paths, each inside a fenced write with the writer lock held:
  [`cleanup_failed_reference`][cleanup] unlinks a newly published object when no
  reference or reservation protects it; [`reclaim_candidate`][reclaim-cand]
  reads the size in [`unlink_artifact`][unlink-artifact] and unlinks at
  [`:239-319`][reclaim-cand] before its row deletes commit at
  [`:239-319`][reclaim-cand]; [`complete_pending_purge_locked`][purge-unlink]
  unlinks the object and its temps before deleting the pending row. A dedup hit
  or a failed publish only [releases the row][release-res].
- Recovery: [`prepare_startup_cas_recovery`][startup] promotes `Live` rows
  with no reference to `Reclaiming`, retires `Live` rows whose bytes are gone
  ([`:133`][startup-unreachable]), and re-arms tombstones whose bytes
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
  adds the published length ([`publish_counted_object`][usage-publish]).
  Each object unlink goes through [`unlink_counted_object`][usage-unlink],
  which subtracts the `st_size` read before the unlink once the durable
  unlink succeeds; a failed unlink leaves the count unknown, and a removal
  larger than the count leaves it unknown and latches.
  [`reconcile_artifact_usage`][usage-reconcile] walks outside the writer
  lock, then takes the lock to compare the tree-change number and settle the
  result. Recovery (open and restore) runs it when the open census seeded no
  count or recovery left the count different from that seed; every
  maintenance pass runs it. With no count it seeds one. With a
  count it compares, and a disagreement latches the CAS fail-closed state
  and replaces the count with the walk, so the health sampler reports the
  bytes on disk. The drift action is the latch: the next admission is
  refused with `IngestionFailClosed`, and only a new store, which re-seeds
  from the tree, admits again. The walk follows
  [`regular_file_bytes`][walk-head]'s file set, so a stray root-level file
  counts and its later appearance or disappearance is drift. The count, the
  tree-change number, the settlement number, and the deferral counter share
  one mutex over a 40-byte value (`Option<u64>`, two `u64`, `u8`, and
  padding on x86_64) and replace the cached `Option<u64>` and generation the
  store already held, so the declared retained-resident total grows by 16
  bytes.
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
  the census counts the tree the writer lock would have seen, and the seed
  settles it through the same path as a reconciliation: a count held and
  disagreeing latches, and a failed walk leaves the count as it stood. A
  failure other than exhaustion from any worker outranks an exhaustion from
  another. The
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

### Q: Does a reconciliation that releases the writer lock during its walk keep the count equal to the walk, and what does a failed walk settle?

- Sources examined: [`reconcile_artifact_usage`][usage-reconcile-split],
  the [tree-change record][usage-tree-change],
  [`publish_counted_object`][usage-publish-counted],
  [`unlink_counted_object`][usage-unlink-change],
  [`settle_census`][usage-settle], the daemon's [memory-reviewer
  sweep][daemon-maintenance] and its [intervals][sample-interval], the
  [usage tests][t-usage], and a local probe (release build, `taskset -c
  4-7`, a store of 4,000 objects, 15 maintenance passes each beside a thread
  that ingests in a loop, three runs against the parent commit; uncommitted
  probe numbers, not approved bench evidence).
- Findings: The earlier entries state that the daemon calls no maintenance.
  It does: the memory-reviewer sweep calls `run_staging_maintenance` every
  30 s when idle and every 5 s after a pass that advanced, up to four calls
  a slice, so the reconciliation and its latch run in production. At the
  parent commit each such pass held the writer lock for a whole walk, and a
  walk that failed for any reason other than exhaustion latched the store
  closed. Now the walk runs without the writer lock. Every publication and
  every object unlink holds the writer lock from its filesystem change
  through its tree-change record, so once the reconciliation takes the
  writer lock no change is half-applied; it compares only when the
  tree-change number equals the one it read before walking, and otherwise
  leaves the comparison to the next pass. A publication that finds the
  object already present changes nothing and records nothing; a failed
  publication records a change, because the fallback link can place and
  remove the object inside its window. A census or reconciliation walk that
  fails settles nothing: a held count stays, and an admission that holds no
  count walks under the writer lock and fails closed itself. The count and
  the tree-change number share one mutex, the same shape as the parent's
  count and generation, so the retained-resident total is unchanged.
  [`a_reconciliation_walks_without_the_writer_and_skips_a_count_that_moved_under_it`][t-usage-raced]
  finds the writer lock free after the walk, lands a publication and then
  an unlink there, keeps usage equal to the walk and admission open, and
  shows the next pass still latches on a stray file.
  [`a_reconciliation_walk_that_fails_leaves_admission_on_the_held_count`][t-usage-failed-walk]
  and [`an_open_whose_census_fails_admits_once_a_later_walk_succeeds`][t-usage-failed-census]
  fail a walk with an unreadable directory. Negative controls, run locally
  and reverted: comparing regardless of the tree-change number, dropping the
  publication's record, dropping the unlink's record, and holding the writer
  lock across the walk each fail the race test; latching on a failed walk
  fails both failed-walk tests. Probe: concurrent ingests completed during
  the 15 passes rose from 627, 629, and 618 to 787, 800, and 919; ordinary
  ingest p50 was 488, 496, and 474 us before and 494, 542, and 505 us
  after, inside the run-to-run spread; a pass's own duration is dominated by
  GC's per-candidate writer acquisitions, which this change leaves alone.
- Missing evidence: A failed walk caused by `EMFILE` or `ENFILE`, which no
  fault seam injects; the tests use `EACCES` and skip when the process
  ignores mode bits.
- Conclusion: resolved with answer - the reconciliation compares only a walk
  no publication or unlink overlapped, holds the writer lock only for the
  comparison, and a failed walk is no evidence against the count.

### Q: Does a writer that overlaps every maintenance pass defer drift detection without bound, and does a census that runs the descriptor table out latch the store?

- Sources examined: [`reconcile_artifact_usage`][usage-reconcile-bounded]
  and its [deferral counter][usage-raced-counter],
  [`regular_file_bytes_concurrently`][walk-concurrent] and its
  [descriptor fallback][walk-descriptors], [`check_budget`][check-budget],
  the two review findings on PR #1043, and the [usage tests][t-usage].
- Findings: Both findings held against the code as reviewed. A pass whose
  tree-change number moved during its walk settled nothing and kept no record
  of having deferred, so with a stray file on disk and an ingest landing
  inside every pass's window, four passes in a row left admission open
  ([`drift_latches_once_raced_reconciliations_reach_their_bound`][t-usage-raced-bound]
  failed with `unwrap_err()` on an admitted handle before the change). The
  count now carries the number of consecutive deferred comparisons; after
  three, the next reconciliation takes the writer lock before its snapshot
  and holds it across the walk, so that comparison runs and the stray file
  latches on the fourth pass. The census workers each hold one shard
  descriptor at a time, so a soft `RLIMIT_NOFILE` within sixty-four of the
  descriptors in use made the admission walk fail with `EMFILE`, which
  `check_budget` maps to `IngestionFailClosed` and latches
  ([`an_admission_walk_short_of_descriptors_counts_the_tree_one_shard_at_a_time`][t-usage-descriptors]
  failed with `artifact ingestion is fail-closed until the store is reopened`
  before the change). `EMFILE` or `ENFILE` from a worker now repeats the walk
  on the calling thread, which holds one shard descriptor, and the walk
  becomes the count. The deferral counter is one `u8` beside the count and
  the tree-change number under the same mutex, which grows the value from 24
  to 32 bytes (40 with the settlement number added below).
- Missing evidence: A census at open that runs the descriptor table out; the
  descriptor test lowers the limit after open and exercises the admission
  walk. A walk whose single-threaded repeat also fails with `EMFILE`, which
  still latches.
- Conclusion: resolved with answer - a writer defers drift detection by at
  most three maintenance passes, and a census short of descriptors counts the
  tree one shard at a time.

### Q: Can an older reconciliation replace a newer census, and can the census at open starve the database open of descriptors?

- Sources examined: [`reconcile_artifact_usage`][usage-reconcile-bounded],
  [`settle_census`][usage-settle], [`regular_file_bytes_on_workers`][walk-concurrent]
  and its [descriptor budget][walk-descriptor-budget], the [open
  census][open-census], and the [usage tests][t-usage].
- Findings: The ordering finding held. Out-of-band drift records no tree
  change, so two reconciliations whose walks straddled a drift snapshot the
  same tree-change number; the one that walked later settled the drifted
  total and latched, and the older one then passed the same check and
  replaced that total with its stale walk
  ([`a_census_settled_during_an_older_walk_is_kept`][t-usage-settled-order]
  reported 4 bytes against 9 on disk before the change). The count now
  carries a settlement number that every settled walk advances; a
  reconciliation whose snapshot predates the current number leaves the count
  as the newer walk settled it, and the latch from that walk stands. The
  descriptor finding was not reproduced: with the soft limit lowered to the
  descriptors in use plus twelve, two above what the database open takes,
  the store opened and its census became the count in six runs of six
  ([`a_store_opens_while_its_census_shares_a_small_descriptor_table`][t-usage-open-descriptors]).
  The overlap is possible in principle, so the worker count is now also
  bounded by one worker per sixteen descriptors of the soft limit: 64 under
  the 1,024 default and one under the test's limit, so the census holds at
  most a sixteenth of the table while the database opens.
- Missing evidence: A run in which the census workers held the descriptors
  the database open needed; the test shows the open succeeding, so the
  worker bound rests on the arithmetic, which the test exercises at one
  worker.
- Conclusion: resolved with answer - a settled census is replaced only by a
  walk that started after it, and the census takes at most a sixteenth of the
  descriptor table.

[ingest-reservation]: ../../../../../crates/kernel/src/cas/ingest.rs#L427
[ingest-publish]: ../../../../../crates/kernel/src/cas/ingest.rs#L479
[ingest-commit]: ../../../../../crates/kernel/src/cas/ingest.rs#L583
[check-budget]: ../../../../../crates/kernel/src/cas/ingest.rs#L947-L962
[release-res]: ../../../../../crates/kernel/src/cas/ingest.rs#L1036-L1052
[cleanup]: ../../../../../crates/kernel/src/cas/ingest.rs#L1059-L1123
[walk]: ../../../../../crates/kernel/src/cas/ingest.rs#L1517-L1537
[startup]: ../../../../../crates/kernel/src/cas/gc.rs#L102-L204
[startup-unreachable]: ../../../../../crates/kernel/src/cas/gc.rs#L133
[reclaim-cand]: ../../../../../crates/kernel/src/cas/gc.rs#L239-L319
[recovery]: ../../../../../crates/kernel/src/cas/gc.rs#L330-L371
[unlink-artifact]: ../../../../../crates/kernel/src/cas/gc.rs#L442-L460
[scan-objects]: ../../../../../crates/kernel/src/cas/gc.rs#L663
[object-usage]: ../../../../../crates/kernel/src/cas/usage.rs#L208-L216
[purge-unlink]: ../../../../../crates/kernel/src/cas/deletion.rs#L535-L558
[latch]: ../../../../../crates/kernel/src/cas/mod.rs#L631
[open-recover]: ../../../../../crates/kernel/src/open.rs#L440
[recover]: ../../../../../crates/kernel/src/open.rs#L458-L468
[facts]: ../../../../../crates/kernel/src/facts.rs#L144
[restore]: ../../../../../crates/kernel/src/backup.rs#L424
[sampler-facts]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L226
[t-oracle]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-recover-twice]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L384-L389
[t-faults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-gcfaults]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L497-L582
[t-crash]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L903-L969
[t-orphan]: ../../../../../crates/kernel/tests/kernel_gc.rs#L634-L676
[usage-count]: ../../../../../crates/kernel/src/cas/usage.rs#L52-L59
[usage-publish]: ../../../../../crates/kernel/src/cas/usage.rs#L66-L88
[usage-unlink]: ../../../../../crates/kernel/src/cas/usage.rs#L96-L120
[usage-reconcile]: ../../../../../crates/kernel/src/cas/usage.rs#L135-L204
[usage-reported]: ../../../../../crates/kernel/src/cas/usage.rs#L208-L216
[open-seed]: ../../../../../crates/kernel/src/open.rs#L435
[recover-reconcile]: ../../../../../crates/kernel/src/open.rs#L462-L467
[maintenance-reconcile]: ../../../../../crates/kernel/src/retention.rs#L222-L223
[t-usage]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs
[walk-head]: ../../../../../crates/kernel/src/cas/ingest.rs#L1517-L1701
[walk-concurrent]: ../../../../../crates/kernel/src/cas/ingest.rs#L1539-L1608
[open-census]: ../../../../../crates/kernel/src/open.rs#L320
[usage-seed]: ../../../../../crates/kernel/src/cas/usage.rs#L170-L190
[usage-census]: ../../../../../crates/kernel/src/cas/usage.rs#L278-L299
[t-oracle-head]: ../../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[usage-reconcile-split]: ../../../../../crates/kernel/src/cas/usage.rs#L122-L168
[usage-tree-change]: ../../../../../crates/kernel/src/cas/usage.rs#L22-L45
[usage-publish-counted]: ../../../../../crates/kernel/src/cas/usage.rs#L61-L88
[usage-unlink-change]: ../../../../../crates/kernel/src/cas/usage.rs#L90-L120
[usage-settle]: ../../../../../crates/kernel/src/cas/usage.rs#L192-L204
[daemon-maintenance]: ../../../../../crates/daemon/src/memory_reviewer/lifecycle.rs#L311-L325
[sample-interval]: ../../../../../crates/daemon/src/memory_reviewer/lifecycle.rs#L16-L22
[t-usage-raced]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs#L532
[t-usage-failed-walk]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs#L644
[t-usage-failed-census]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs#L666
[usage-reconcile-bounded]: ../../../../../crates/kernel/src/cas/usage.rs#L122-L168
[usage-raced-counter]: ../../../../../crates/kernel/src/cas/usage.rs#L22-L39
[walk-descriptors]: ../../../../../crates/kernel/src/cas/ingest.rs#L1548-L1561
[t-usage-raced-bound]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs#L570
[t-usage-descriptors]: ../../../../../crates/kernel/tests/kernel_artifact_usage_descriptors.rs#L67
[walk-descriptor-budget]: ../../../../../crates/kernel/src/cas/ingest.rs#L1703-L1713
[t-usage-settled-order]: ../../../../../crates/kernel/tests/kernel_artifact_usage.rs#L609
[t-usage-open-descriptors]: ../../../../../crates/kernel/tests/kernel_artifact_usage_descriptors.rs#L103
