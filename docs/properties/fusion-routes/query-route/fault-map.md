# Query-route budget fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: the `rp27/u1-identity-contract`
branch head on `main` at `8e0491225a7292ef077c675d44b94f94a24041d3`.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Cancel during a held statement | `CancellationToken` behind `CancelSignal::observing`; a long recursive scan under `read_under`. | route-sql-cancellation-is-request-local, route-permits-and-pins-outlive-client-cancellation |
| Remaining duration elapses during a scan | `RequestBudget::derive` with a short `remaining_ms`. | route-budget-is-derived-once-before-queue-wait |
| Cancel while waiting for a held connection | A reader holding the connection on another thread. | route-budget-is-derived-once-before-queue-wait |
| Late cancel before the next request | A second `RequestBudget` on the same projection after the first is cancelled. | route-sql-cancellation-is-request-local |
| Leaked progress handler | `GuardedConn::leak_progress_handler_for_test` in the storage test module. | route-sql-cancellation-is-request-local |
| Handler aborted while suspended at `run_blocking` | A real host and client; `ResponseStream::cancel`. | route-permits-and-pins-outlive-client-cancellation |
| Panic inside tracked blocking work | A closure that panics under `run_blocking`. | route-permits-and-pins-outlive-client-cancellation |

Real durations are used because `EvalBudget` reads `std::time::Instant`, which
virtual time cannot advance.
