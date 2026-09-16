# Query-route fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: the `rp27/u1-identity-contract`
branch head on `main` at `8e0491225a7292ef077c675d44b94f94a24041d3` for the
budget rows; `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98` for the route rows.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Cancel during a held statement | `CancellationToken` behind `CancelSignal::observing`; a long recursive scan under `read_under`. | route-sql-cancellation-is-request-local, route-permits-and-pins-outlive-client-cancellation |
| Remaining duration elapses during a scan | `RequestBudget::derive` with a short `remaining_ms`. | route-budget-is-derived-once-before-queue-wait |
| Cancel while waiting for a held connection | A reader holding the connection on another thread. | route-budget-is-derived-once-before-queue-wait |
| Late cancel before the next request | A second `RequestBudget` on the same projection after the first is cancelled. | route-sql-cancellation-is-request-local |
| Leaked progress handler | `GuardedConn::leak_progress_handler_for_test` in the storage test module. | route-sql-cancellation-is-request-local |
| Handler aborted while suspended at `run_blocking` | A real host and client; `ResponseStream::cancel`; a drop-only variant whose budget observes a never-cancelled token. | route-permits-and-pins-outlive-client-cancellation |
| Panic inside tracked blocking work | A closure that panics under `run_blocking`. | route-permits-and-pins-outlive-client-cancellation |
| Cancel or lapse at one named phase | `execute`'s `before_phase` hook with a token or a sleep. | route-cancellation-is-observed-in-every-phase |
| One bound saturated | A `QueryRouteLimits` with one field shrunk over a fixed corpus. | route-bounds-are-enforced-before-protected-work |
| Foreign project, unbound session, harness mismatch | Request fields on a `KernelDaemon` route. | route-authorization-precedes-materialization, route-candidate-ids-never-widen-scope |
| Retirement after the projection snapshot | `retire_decision` plus a `ClaimMaterializer` episode. | route-final-revalidation-precedes-every-result |
| Foreign scope over the same rows | A second `ProjectScope` passed to `execute`. | route-candidate-ids-never-widen-scope, route-final-revalidation-precedes-every-result |
| Route disabled | `Handler::set_query_route_limits(None)`. | route-rollback-disables-without-mutating-canonical-truth |
| Lane failure other than the budget | Not yet injected; the lane primitives' `unavailable` reasons are mapped, no test drives one through the route. | route-required-context-failure-is-typed-and-terminal |

Real durations are used because `EvalBudget` reads `std::time::Instant`, which
virtual time cannot advance.
