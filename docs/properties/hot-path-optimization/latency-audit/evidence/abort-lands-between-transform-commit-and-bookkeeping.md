# abort-lands-between-transform-commit-and-bookkeeping

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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
  runs after the roots insert and before `prepare_historian_fire`, so it can
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
  the closure at [`:4944-4982`][store-commit].
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
which needs usage at the emergency threshold and a historian firing that is
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

[hook-field]: ../../../../../crates/daemon/src/lib.rs#L2916-L2919
[h-run]: ../../../../../crates/daemon/src/lib.rs#L8190-L8252
[commit-call]: ../../../../../crates/daemon/src/lib.rs#L8264
[roots-insert]: ../../../../../crates/daemon/src/lib.rs#L8271-L8276
[hook]: ../../../../../crates/daemon/src/lib.rs#L8309-L8317
[emergency]: ../../../../../crates/daemon/src/lib.rs#L8253-L8343
[await-a]: ../../../../../crates/daemon/src/lib.rs#L8324
[await-b]: ../../../../../crates/daemon/src/lib.rs#L8350
[await-c]: ../../../../../crates/daemon/src/lib.rs#L8324
[pc-store]: ../../../../../crates/daemon/src/lib.rs#L8439
[guidance-remove]: ../../../../../crates/daemon/src/lib.rs#L8450-L8455
[store-commit]: ../../../../../crates/daemon/src/transform.rs#L4947-L4985
[load]: ../../../../../crates/memory-store/src/lib.rs#L6236-L6263
[host-cancel]: ../../../../../crates/host-runtime/src/dispatch.rs#L938-L955
[host-close]: ../../../../../crates/host-runtime/src/dispatch.rs#L1239-L1259
[e3]: ../../catalog.md#request-close-overlaps-live-work
