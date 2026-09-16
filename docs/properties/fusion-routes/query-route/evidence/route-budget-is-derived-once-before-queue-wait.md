# route-budget-is-derived-once-before-queue-wait

## Discovery trigger

RP2.7 KTD3 fixes one local absolute budget derived before embedding and before
any queue wait from request cancellation and a clamped remaining-duration
field. The cancellation lens adds that the host aborts a cancelled handler
future, so any budget that depends on an awaited `select!` arm is never
cancelled at all.

## Evidence trail

- `crates/host-runtime/src/handler.rs` `RequestCtx` carries a cancellation
  token and no deadline; `dispatch.rs` aborts the handler future on cancel
  and then drains the request ledger.
- `crates/kernel/src/applicability/checkout.rs` `EvalBudget` clones share one
  interrupt flag and copy one `Instant` deadline; `cancel` is irreversible.
- `crates/daemon/src/request_budget.rs` `RequestBudget::derive` refuses a
  missing ceiling, a missing or zero `remaining_ms`, and an already-cancelled
  request, then mints the deadline once; `SharedBudget` clones and the stop
  predicate carry that deadline; `Drop` cancels. The `EvalBudget` is private
  to `SharedBudget`: host cancellation is folded into its flag only when
  `exhaustion` or the stop predicate polls, so a callee holding the
  `EvalBudget` alone would run to the deadline after a host cancel. Handing it
  out waits for the route that owns the async side and can bridge the token's
  edge into the flag once.
- `crates/storage/src/lib.rs` `lock_conn_until` polls the same stop predicate
  during acquisition, so the wait ends on cancellation.

## Failure scenario

A route that computes `Instant::now() + remaining` again at the blocking stage
grants the scan a fresh window after the async wait consumed the original one.
A route that reads a transport timeout instead of the caller's field lets the
host's admission budget stand in for the caller's remaining duration.

## Timing windows and dependencies

The window between `derive` and the first blocking phase, and the acquisition
wait while another reader holds the connection.

## What a test must construct

- A `remaining_ms` above the ceiling, asserting the clamped deadline.
- Missing ceiling, missing and zero `remaining_ms`, and a pre-cancelled signal.
- A clone carried to another thread, compared for deadline equality.
- A held connection with a waiter whose budget is cancelled mid-wait.

## Investigation log

### Q: Should connection acquisition observe cancellation?

- Sources examined: `lock_conn_within` (deadline-only poll loop); the parent
  Q4 wording; the acceptance criterion offering either behavior.
- Findings: the poll loop already wakes every millisecond, so checking the
  stop predicate there costs nothing and closes the gap for every caller of
  `with_conn_interruptible`.
- Missing evidence: none.
- Conclusion: resolved with answer - acquisition observes the stop predicate
  in storage; the storage surface carries it.
