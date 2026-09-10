# callback-batching-preserves-observation-boundaries

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Batching repeated guarded calls can amortize setup while accidentally changing
which commits a caller observes or which effects roll back together.

## Evidence trail

- [storage/lib.rs:220-245][read] gives each read callback one deferred-transaction
  snapshot and finishes that transaction when the callback ends.
- [290-316][write] places each fenced callback inside an immediate transaction
  with its own claim and commit decision.
- [3946-3981][test] reads `1` twice around a second connection's commit, then
  reads `2` in the next callback. It checks both stability and freshness.
- [4116][rollback] checks rollback on a fenced callback error.
- [memory-store/lib.rs:5532-5563][caller] demonstrates independent reads before
  a prepared fenced operation; adjacency does not itself make one transaction.

## Failure scenario

Two independent reads are merged into one transaction and the second retains
an old snapshot despite an intervening commit. Alternatively, two independent
writes become one rollback unit, so failure of the second erases the first.
Splitting an existing multi-read callback can create the opposite inconsistency.

## Timing windows and dependencies

Use a controlled writer schedule, including a commit inside a callback and
another between callbacks. SQLite snapshot timing is tied to reads, not just
function entry. The reference must preserve the original operation boundaries
and supported concurrency; it must not force unrelated calls to share a snapshot.

## What a test must construct

Compare the observable sequence of read values and successful/refused write
effect sets with the baseline under that schedule. One operation must fail next
to an independent successful operation. Keep fence durability and schema identity
unchanged. Existing snapshot and rollback tests remain
[unaudited](../existing-checks.md#guarded-store); no new trace is exercised.

## Investigation log

### Q: Which observations may the optimization legally batch?

- Sources examined: [The callback boundaries][read], [fenced writes][write], and
  [the existing freshness test][test].
- Findings: A callback has a snapshot contract, and the next callback can
  observe a newer commit. Existing adjacency does not imply shared atomicity.
- Missing evidence: A proposed batching plan identifying logical observations
  and preserved commit points is not supplied.
- Conclusion: The preservation obligation is resolved; selecting batching
  boundaries needs human input rather than imposing a new global snapshot.

[read]: ../../../../crates/storage/src/lib.rs#L220-L245
[write]: ../../../../crates/storage/src/lib.rs#L290-L316
[test]: ../../../../crates/storage/src/lib.rs#L3946-L3981
[rollback]: ../../../../crates/storage/src/lib.rs#L4116
[caller]: ../../../../crates/memory-store/src/lib.rs#L5532-L5563
