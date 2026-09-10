# request-close-overlaps-live-work

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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

[start]: ../../../../crates/host-runtime/src/dispatch.rs#L857-L934
[cancel]: ../../../../crates/host-runtime/src/dispatch.rs#L937-L954
[close]: ../../../../crates/host-runtime/src/dispatch.rs#L1226-L1268
[test]: ../../../../crates/host-runtime/tests/dispatch.rs#L357-L399
[overlap]: ../../../../crates/host-runtime/tests/dispatch.rs#L832-L887
