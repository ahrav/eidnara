# route-permits-and-pins-outlive-client-cancellation

## Discovery trigger

RP2.7 requires owned permits, pins, connections, and scratch to stay charged
until blocking work physically completes and is joined, even after client
cancellation, and a cancelled request to return a typed terminal rather than a
falsely complete ranking.

## Evidence trail

- `crates/host-runtime/src/handler.rs` `WorkLedgers::run_blocking` registers
  the request, route, and host trackers before the route-closed check,
  catches unwind on the blocking thread, and reports `Panicked`,
  `RuntimeStopped`, or `RouteClosing`.
- `crates/host-runtime/src/dispatch.rs` aborts the handler future on cancel,
  waits for the request ledger, and only then settles the request.
- `crates/daemon/src/request_budget/host_tests.rs` runs a real host and
  client: the handler derives a `RequestBudget`, runs a held projection read
  through `run_blocking`, and awaits it with no `select!` arm. The blocking
  closure records when the read returned; the publish hook records when the
  error terminal was published.

## Failure scenario

A request settled while its read still runs would release the connection to
the next request while the first still holds it, or would report a ranking
that never finished. A panic inside the closure that escaped the boundary
would take the runtime down instead of settling one request.

## Timing windows and dependencies

The host's abort of the handler future while the blocking read runs. With the
request's own token the stop predicate also sees the cancellation directly;
the drop-only variant derives the budget from a token nobody cancels, so the
guard's drop is the only path that raises the interrupt in that window.

## What a test must construct

- A held read on the blocking pool, observable by a second reader failing to
  acquire the connection within a short bound.
- A client cancel frame while the handler is suspended at the await, once
  with the request's token and once with a token the test never cancels.
- Read-return time compared with error-publication time.
- A panicking closure and the client's single terminal error.
- After U3b and U3c: the route's permits and dense pins in the same census.

## Investigation log

### Q: How is "joined before settle" observed without host internals?

- Sources examined: `run_with_publish_hook`; the transform host tests'
  `error_published` pattern.
- Findings: the publish hook fires when the error terminal is emitted for the
  channel, which follows the ledger drain; the blocking closure can stamp its
  own return time.
- Missing evidence: none for the bridge clauses.
- Conclusion: resolved with answer - compare the two instants.
