# route-cleanup-waits-for-request-owned-physical-work

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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
- [daemon/lib.rs:8196-8271][transform] invokes the transform synchronously;
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

[fence]: ../../../../crates/host-runtime/src/dispatch.rs#L873-L934
[cancel]: ../../../../crates/host-runtime/src/dispatch.rs#L937-L954
[close]: ../../../../crates/host-runtime/src/dispatch.rs#L1237-L1268
[wire]: ../../../host-wire-protocol.md#L765-L781
[transform]: ../../../../crates/daemon/src/lib.rs#L8196-L8271
[overlap]: ../../../../crates/host-runtime/tests/dispatch.rs#L832-L887
