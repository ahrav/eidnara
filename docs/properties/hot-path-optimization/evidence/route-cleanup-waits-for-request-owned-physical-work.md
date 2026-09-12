# route-cleanup-waits-for-request-owned-physical-work

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

## Discovery trigger

Moving synchronous transform work off an async worker can separate waiter
cancellation from physical completion. The existing cleanup gate must survive.
The present claim concerns tracked callbacks; moving work must extend that gate.

## Evidence trail

- [dispatch.rs:873-934][fence] tracks both outer dispatch and handler callback,
  because aborting the outer task alone does not establish callback completion.
- [dispatch.rs:937-954][cancel] aborts and awaits the callback when cancellation
  wins before settling the cancelled terminal.
- [dispatch.rs:1237-1268][close] closes the tracker, waits, aborts, and waits
  again. Failure to stop trips fatal state and returns false, not cleanup.
- [The wire contract:767-781][wire] requires best-effort cancellation and
  cleanup-gated reuse, and preserves unknown outcomes after unobserved terminals.
- [daemon/lib.rs:8138-8202][transform] invokes the transform synchronously;
  no off-worker transform completion protocol is demonstrated by this site.
- [tests/dispatch.rs:832-887][overlap] starts a hanging callback, sends route
  Goodbye, then checks cancellation and one cleanup callback. It is unaudited.

## Failure scenario

A detached worker survives a cancelled handler, while the route tracker becomes
empty. Cleanup removes state that worker still uses, or the channel is reused
before stale work loses access. A terminal frame does not prove quiescence.

## Timing windows and dependencies

The relevant interval runs from physical work start through release of access
to route-owned state, not merely until a JoinHandle or waiter is dropped.
Existing close budgets permit fatal refusal when work cannot stop. This record
does not promise arbitrary work completes within a new global duration.

## What a test must construct

Hold live work at a barrier, begin close, and record completion, route-gone, and
reuse independently for the full route identity. Cleanup requires no live
request-owned state access. Reconcile durable operations using existing CAS
and receipts; do not demand exactly one effect per transform or infer no effect
from cancellation. [Existing checks](../existing-checks.md#execution-lifecycle)
are unaudited, and no new physical-completion trace runs here.

## Investigation log

### Q: Who owns and joins a proposed off-worker transform?

- Sources examined: [The transform call][transform] and [host fence][fence].
- Findings: The host tracks the handler task, not an independently proposed
  worker that the handler might spawn. Current source cannot prove that design.
- Missing evidence: The worker topology, completion handoff, and cleanup witness
  are not supplied.
- Conclusion: FUTURE execution topology needs human input. Worker-specific test
  readiness is BLOCKED, not implementation-ready. Default-production reachability
  covers existing callbacks only; it does not establish an off-worker proof or
  prescribe a new route-reuse epoch mechanism.


## Request-work join evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/437)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).
Join-owner decision: the host owns the join. A completion guard carried by the
request context enters blocking work in the request's own ledger and in the
route's; the daemon keeps no per-route drain of its own.

[`RequestCtx::run_blocking`][run-blocking] is the seam. It submits the closure
to the blocking pool at the call, under [`redact_sync`][redact-sync], so a panic
on that thread reaches the redacting hook at depth one and prints the fixed
diagnostic rather than its payload. The closure's result travels back over a
one-shot channel and is handed over, or dropped when no one waits, under the
same guard, so a destructor panic in the value or in a panic payload is
redacted too; a panic surfaces to the handler as
[`BlockingWorkFailed::Panicked`][failed], a runtime stop as `RuntimeStopped`,
and an offer made once the route's fence has closed as `RouteClosing`: such
work is refused rather than run outside every join. Registration takes a route
tracker token before checking whether the fence has closed. An accepted offer
therefore holds the drain open before submitting its closure. The join itself
runs in a task of its own, entered in
[two ledgers][work-ledgers]: the request tracker that
[`dispatch_request` creates][ctx-work] for each request, and the route tracker
that is the existing handler completion fence. Dropping the future the handler
awaits, as an aborted handler does, detaches only the result: the join task
keeps both trackers non-empty until the thread finishes.

The cancel arm aborts and joins the handler task as before and then
[closes and waits on the request ledger][cancel-join] before it settles the
cancelled terminal, so a cancellation cannot report before the request's
blocking work has stopped. Route close keeps its shape: the
[close gate][close-gate-live] closes the route tracker, waits the route-close
budget, aborts the dispatch tasks, waits the budget again, and trips fatal
rather than running route-gone if work is still live; the join task is entered
in that tracker, so held blocking work, awaited or not, is inside the gate.
The abort drops the dispatch task at its wait on the request ledger, before
that task's own `settle`, so when the work then finishes inside the post-abort
budget the [route drain settles][close-fallback] each collected pending entry
that is still unsettled as `cancelled` before route-gone; `settle` is
first-terminal-wins, so a request the task settled earlier is left alone.
Neither budget moves. The host cannot stop a thread that has started, so the
seam's contract asks the work to terminate on its own or to observe the
request's [cancel signal][cancel-signal] at its safe points; the signal is an
observation-only view of the token, so a handler cannot cancel its own request
through it; work held past the budget follows the fatal path, which is E1's
stated outcome for unquiesced work. Values the closure captures, charges
included, are released when it returns or unwinds, on the blocking thread,
whichever side stopped waiting, so a charge returns exactly once. Two
consequences are recorded rather than changed: a cancelled request keeps its
pending permit while the cancel arm waits on its work, and work entered from a
task that outlives the handler task, after the cancel arm has waited, is joined
by route close alone.

The seam covers only work submitted through it. The daemon's
[`kernel_routes::blocking`][daemon-blocking] is a bare `spawn_blocking` whose
`JoinError` maps to an unavailable store; it enters neither ledger and raises
no redaction guard, and its ten call sites (`read.rs`, `ingest.rs`,
`eligibility.rs`, `egress.rs`, `commit.rs`, `lib.rs`) are the production
submitters of request-owned store work. An aborted handler drops such a
worker's result unread, as [`ingest.rs`][ingest-detached] states, so route-gone
can run while that worker is live. The catalog records this as an open
question; the `always` check holds for `run_blocking` work and not yet for
kernel routes.

The [test handler modes][t-hold] run a closure that waits on a condition
variable and drops a counted resident charge when released, or panics while
holding one. The [cancel test][t-cancel] cancels the request while the closure
is held, sees no terminal for three hundred milliseconds and no charge
released, releases the closure, then sees one `cancelled` terminal, one charge
release, no second terminal, and a served request on the same route. The
[route-close test][t-close] sends `Goodbye` while the closure is held, sees no
terminal, no route-gone and no release, releases, then sees the `cancelled`
terminal, exactly one route-gone, and one release. The [detached test][t-detached]
has the handler answer without awaiting its work, so only the route ledger
holds it, and shows route-gone waiting for the release all the same. The
[budget test][t-fatal] holds the work past a shortened route-close budget and
shows no route-gone and a lifecycle-fatal shutdown, the refusal path rather
than cleanup. The [late-release test][t-late] shortens the budget to four
hundred milliseconds, releases the work at six hundred, after the close has
aborted the dispatch task, and sees the `cancelled` terminal, exactly one
route-gone, one release, and a graceful shutdown; before the route drain
settled unsettled entries this test timed out waiting for the terminal. The
[panic test][t-panic] shows a panic inside the closure settling as one
`internal_error` terminal with the held charge released once, and the
[stderr test][t-stderr] re-runs the binary as a child that panics inside the
closure with the canary payload and shows stderr carrying the fixed
diagnostic, not the canary, with an unrelated panic still reaching the prior
hook.

### Focused execution, 2026-09-12

`cargo test -p host-runtime --locked` passed every suite, `tests/dispatch.rs`
with 27 tests including the seven above.

[run-blocking]: ../../../../crates/host-runtime/src/handler.rs#L593-L619
[work-ledgers]: ../../../../crates/host-runtime/src/handler.rs#L452-L455
[failed]: ../../../../crates/host-runtime/src/handler.rs#L460-L468
[cancel-signal]: ../../../../crates/host-runtime/src/handler.rs#L495-L530
[redact-sync]: ../../../../crates/host-runtime/src/panic_boundary.rs#L52-L55
[ctx-work]: ../../../../crates/host-runtime/src/dispatch.rs#L915
[cancel-join]: ../../../../crates/host-runtime/src/dispatch.rs#L950
[close-gate-live]: ../../../../crates/host-runtime/src/dispatch.rs#L1234-L1298
[close-fallback]: ../../../../crates/host-runtime/src/dispatch.rs#L1274-L1289
[daemon-blocking]: ../../../../crates/daemon/src/kernel_routes/mod.rs#L462-L468
[ingest-detached]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L793-L797
[t-hold]: ../../../../crates/host-runtime/tests/support/mod.rs#L617-L660
[t-cancel]: ../../../../crates/host-runtime/tests/dispatch.rs#L725-L776
[t-close]: ../../../../crates/host-runtime/tests/dispatch.rs#L781-L825
[t-detached]: ../../../../crates/host-runtime/tests/dispatch.rs#L830-L866
[t-fatal]: ../../../../crates/host-runtime/tests/dispatch.rs#L871-L898
[t-late]: ../../../../crates/host-runtime/tests/dispatch.rs#L901-L944
[t-panic]: ../../../../crates/host-runtime/tests/dispatch.rs#L950-L984
[t-stderr]: ../../../../crates/host-runtime/tests/dispatch.rs#L610-L612

[fence]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L873-L934
[cancel]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L937-L954
[close]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L1237-L1268
[wire]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L765-L781
[transform]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8138-L8202
[overlap]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/tests/dispatch.rs#L832-L887
