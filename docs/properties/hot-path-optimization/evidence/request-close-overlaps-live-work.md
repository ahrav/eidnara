# request-close-overlaps-live-work

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
The final blocking group reports eleven passes and one ignored child role that
its parent executes. Final Bun passes; the earlier full-workspace run failed
three tests. Historical `d6060f79` gates remain separate.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation below describe that baseline. The implementation
section records the resolved topology and does not rewrite the historical gap.

## Discovery trigger

Cleanup tests are vacuous for execution relocation if all work finishes before
close starts. The witness must describe overlap, not a forbidden outcome.

## Evidence trail

- [dispatch.rs:857-934][start] installs pending identity before dispatch and
  starts the tracked handler with cancellation and charged input.
- [dispatch.rs:937-954][cancel] requests abort and waits for callback completion.
- [dispatch.rs:1226-1268][close] initiates cancellation before waiting for route
  quiescence, so an active-request close is a production lifecycle situation.
- [tests/dispatch.rs:357-399][test] waits for dispatch before cancellation and
  checks the terminal; it is not a future transform-worker completion witness.
- [tests/dispatch.rs:832-887][overlap] overlaps a hanging callback with route
  Goodbye and checks cancellation plus one cleanup callback; it is unaudited.

## Failure scenario

A test sends a request, waits for its response, then closes the route. Its
cleanup assertion passes even when a worker would outlive an earlier close.
Another weak marker observes cancellation alone and never establishes live work.

## Timing windows and dependencies

The required order is work-start, close-start, then physical completion for
one route identity. Hold a barrier that prevents completion until the close
start is observed. Route close and generation retirement are distinct schedules.
An async handler's start is not proof that a separately queued worker started.

## What a test must construct

Record independent start and completion events, initiate close during the held
interval, and use this record's constant slug as the situation marker. Pair
the witness with E1 and E2; never require early cleanup, stale effects, or a
duplicate terminal to satisfy it. A missed marker is a construction/reachability
gap, not proof of liveness failure. No witness runs here; checks are unaudited.

## Investigation log

### Q: What events identify live physical work in the proposed topology?

- Sources examined: [Tracked handler construction][start] and
  [the close gate][close].
- Findings: Existing general requests support an active-callback overlap. A
  future worker needs its own start/completion observations, independent of
  cancellation and pending-table removal.
- Missing evidence: Worker execution topology and instrumentation are not
  supplied. No transform-specific overlap has been exercised.
- Conclusion: FUTURE topology needs human input; worker-specific test readiness
  is BLOCKED. Default-production reachability describes the existing callback
  lifecycle, whose witness must extend to owned physical work if execution moves.

[start]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L857-L934
[cancel]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L937-L954
[close]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L1226-L1268
[test]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/tests/dispatch.rs#L357-L399
[overlap]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/tests/dispatch.rs#L832-L887

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) resolves the topology question:
the production `UnitRunner` delegates to `RequestCtx::run_blocking`. The host
owns physical joins, not the daemon waiter. [E1's implementation evidence][e1]
describes the gate and its unchanged fatal-close limit.

[`route_close_keeps_binding_and_scratch_until_transform_finishes`][close-test]
runs the real daemon handler through a host and client. Its test-only
`after_transform_commit` gate reports entry after the store commit but before
the lineage
insert. The test observes a committed row, sends route close, and awaits the
request's actual `CancelSignal`. With the gate still held, the route binding
exists, one unit permit is held, the full scratch pool cannot be reacquired,
and neither route-gone nor a server Error publication has occurred. Releasing
the gate permits cleanup, all four permits, exact scratch reacquisition, exact
ingress baseline return, and unchanged committed core state. Before interruption,
the ingress observer reads baseline minus the held body charge. Callback permit
observations are sent to the test for assertions outside the callbacks. They
measure available permits at one callback, not callback frequency or duplicates.
The host
shuts down cleanly under the fixture's two-second route-close budget and
five-second shutdown deadline, with a ten-second test watchdog.

[`request_cancel_waits_for_committed_transform_and_releases_scratch`][cancel-test]
uses explicit request cancellation at the same live gate. It checks a server
Error publication after unit completion and retains the binding until a later
route close. `ResponseStream::cancel` drops its pending receiver locally, so
this test does not decode a server `cancelled` code. [The older raw host
oracle][raw] establishes that code separately.

These tests establish the E3 situation from independent gate entry, committed
store state, and host cancellation observations. They do not require a defect
to make the witness fire. The handler-unit tests additionally abort ordinary
and paged waiters while the worker remains held. Generation retirement and a
full-cap paged request on slow storage are not equivalent tested schedules.
The [execution receipt][receipt] reports the focused run, not a full-gate pass.

[e1]: route-cleanup-waits-for-request-owned-physical-work.md#implementation-evidence-2026-09-13
[close-test]: ../../../../crates/daemon/src/transform_unit/host_tests.rs#L365
[cancel-test]: ../../../../crates/daemon/src/transform_unit/host_tests.rs#L370
[raw]: route-cleanup-waits-for-request-owned-physical-work.md#request-work-join-evidence
[receipt]: ../existing-checks.md#transform-unit-execution-receipt-2026-09-13
