# abort-lands-between-transform-commit-and-bookkeeping

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
The abort witness is included in the passing rebased blocking group. Historical
`d6060f79` gates remain separate; no green full-workspace gate is claimed.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation below retain their baseline scope. The dated
implementation section records the replacement seam and its actual witness.

## Discovery trigger

W8 is meaningful only when a pass is aborted after its transform committed
and before the handler's in-memory bookkeeping ran. On the ordinary path at
HEAD there is no such point: the commit and the bookkeeping are one
synchronous stretch of the handler task. The parent's
[portfolio evaluation](../../portfolio-evaluation.md#gaps-queued) queued a
`sometimes` witness beside the safety record so W8 cannot pass vacuously on
a campaign that never opens the window.

## Evidence trail

- The first `run_transform()` call is at [`:8202`][commit-call]; the store
  commit inside it is at [`commit_transform:4939-4971`][store-commit]. The
  first in-memory update after it is the lineage-root insert at
  [`:8209-8214`][roots-insert].
- Between those two points there is no `.await`. The next `.await` on the
  handler task is [`:8263`][await-a], then [`:8289`][await-b] and
  [`:8315`][await-c], all inside the Emergency95 branch at
  [`:8244-8334`][emergency]. Each is followed by another `run_transform()`
  and a floor reload before control reaches the projection-cache store at
  [`:8387`][pc-store] and the `guidance_dates` removal at
  [`:8398-8403`][guidance-remove].
- The only seam after the commit is the `#[cfg(test)]`
  [`between_transform_and_prepare`][hook] hook at [`:8224-8232`][hook],
  declared at [`:2908-2911`][hook-field] as a test-only interleave point. It
  runs after the roots insert and before `prepare_history_summarizer_fire`, so it can
  separate the commit from every update except the first. No `test-support`
  feature exposes it to `crates/daemon/tests/` or the benches.
- The host aborts a dispatch task on cancel at
  [`dispatch.rs:938-955`][host-cancel] (`inner.abort()` then a `cancelled`
  terminal). On route close it waits `route_close_budget`, aborts every
  tracked task, waits once more, and trips the fatal latch for a task that
  never yields ([`:1239-1259`][host-close]); the comment there says a task
  observes abort only when it yields.
- The commit is observable independently: [`MemoryStore::load`][load]
  returns `row_version`, and `commit_transform` returns the new version to
  the closure at [`:4936-4974`][store-commit].
- The parent's [E3][e3] carries the open question of which abort events a
  future worker can expose without equating waiter cancellation with
  completion.

## Failure scenario

Not a violation; a coverage gap. A relocation campaign that drives passes to
completion, or aborts them before the commit, exercises W8's four structures
on every pass while the window never opens. W8 then holds for both the
current handler and a `spawn_blocking` handler whose bookkeeping is skipped
on abort.

## Timing windows and dependencies

At HEAD: only inside the Emergency95 branch, at one of the three awaits,
which needs usage at the emergency threshold and a history_summarizer firing that is
busy or ready. Under a blocking worker: on every pass, at the await on the
worker's join handle, which a cancel, route close, or generation retirement
can hit while the worker has already committed.

## What a test must construct

A pass whose transform commits (observe `row_version` advance through a
second store handle), then an abort of the handler task before
[`:8209`][roots-insert] executes, then a marker recording both facts and
their order. At HEAD the [hook][hook] is the seam for everything after the
roots insert; the Emergency95 awaits are the only seam for the roots insert
itself. A relocation-era seam must be added by the specification. No existing
check aborts inside the hook or at an Emergency95 await and inspects the next
pass; the [emergency interleave test](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
uses the hook to inject a publish, not an abort.

## Investigation log

### Q: Which abort events can a future worker expose to the host?

The parent's E3 question: which events, without equating waiter cancellation
with completion.

- Sources examined: [`dispatch.rs:938-955`][host-cancel],
  [`:1239-1259`][host-close], the `run_transform` closure at
  [`:8139-8193`][h-run], the parent's [E3][e3].
- Findings: The host has one abort primitive, `JoinHandle::abort`, and one
  observable, the tracker emptying. A blocking worker does not observe abort;
  the handler task does, at the join await. The worker's completion, the
  store commit, and the handler's cancellation are three separate events, and
  nothing at HEAD reports the first two to the host. The witness needs the
  commit observed through the store and the abort observed through the
  handler, as separate records.
- Missing evidence: A worker design and its completion channel.
- Conclusion: needs human input.

[hook-field]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L2908-L2911
[h-run]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8139-L8193
[commit-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8202
[roots-insert]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8209-L8214
[hook]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8224-L8232
[emergency]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8244-L8334
[await-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8263
[await-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8289
[await-c]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8315
[pc-store]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8387
[guidance-remove]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8398-L8403
[store-commit]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L4936-L4974
[load]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6196-L6223
[host-cancel]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L938-L955
[host-close]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L1239-L1259
[e3]: ../../catalog.md#request-close-overlaps-live-work

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) gives abort and panic tests a
dedicated `after_transform_commit` hook in [first_transform][first-live], after
the transform returns and before lineage insertion, guidance-pin removal, and
the first publication-floor read. The separate `between_transform_and_prepare`
hook stays after the first floor read for C5, matching upstream. Both hooks are
`#[cfg(test)]`, not production fault-injection APIs.

[`aborted_waiter_preserves_commit_bookkeeping_and_worker_charges`][abort-test]
constructs the witness on a current-thread runtime. The blocking gate signals
entry, the test reads a committed and initialized row, confirms the lineage
map lacks the session, and aborts the waiter. Awaiting the waiter confirms
its cancelled `JoinError` while the independent work tracker still has one
join and the worker remains held. Only then does the test release the gate.
This establishes commit, waiter abort, and pending bookkeeping as separate
observations. It does not infer physical completion from waiter cancellation.
The test also checks W8's lineage and guidance consequences on the next pass.

The [real-host cancel and route-close tests][host-tests] hold the same hook
while observing the actual host cancellation signal and the retained route
binding. They establish production-path overlap, but explicit client cancel
locally discards the pending receiver. Its server publication hook checks an
Error frame after completion, not the decoded `cancelled` code.

The worker ownership and event questions are resolved for transform units.
W11 stays test-only because the deterministic gate is test-only, even though
the handler and host path are production code. No generation-retirement or
power-loss campaign is claimed. [The execution receipt][receipt] records the
focused results, workspace exit 101 from the two known baseline deadline
failures, their passing isolated reruns, and the passing Bun repository gate;
checks remain unaudited.

[first-live]: ../../../../../crates/daemon/src/lib.rs#L8906
[abort-test]: ../../../../../crates/daemon/src/transform_unit/tests.rs#L144
[host-tests]: ../../../../../crates/daemon/src/transform_unit/host_tests.rs#L245-L372
[receipt]: ../../existing-checks.md#transform-unit-execution-receipt-2026-09-13
