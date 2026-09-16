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
| Retirement after the projection snapshot | `retire_decision` plus a `ClaimMaterializer` episode. | route-final-revalidation-precedes-every-result, route-candidate-ids-never-widen-scope |
| Kernel commit between two revalidation slices | `validation_batch = 1` and a `retire_decision` commit placed by the `before_phase` hook at the second revalidation check. | route-final-revalidation-precedes-every-result |
| Projection read while the lanes are judged | A second budgeted `read_under` placed by the `before_phase` hook at the admission, fusion, and revalidation phases. | route-cancelled-work-drains-within-approved-envelope |
| Stored identifier outside the contract spelling | An occurrence row and its lexical row copied under a non-hex identifier through `SearchProjection::write`. | route-required-context-failure-is-typed-and-terminal |
| Foreign scope over the same rows | A second `ProjectScope` passed to `execute`. | route-candidate-ids-never-widen-scope, route-final-revalidation-precedes-every-result |
| Route disabled | `Handler::set_query_route_limits(None)`. | route-rollback-disables-without-mutating-canonical-truth |
| Lane failure other than the budget | A projection with an identity but no applied batch, so the exact lane has no checkpoint; `probes = 1` against two prose atoms. | route-required-context-failure-is-typed-and-terminal, route-bounds-are-enforced-before-protected-work |
| Unapproved limit set | `validation_batch` over `MAX_ELIGIBILITY_CANDIDATES` passed to `set_query_route_limits`; `response_bytes` one below `QueryRouteLimits::response_floor` passed to `validate`. | route-bounds-are-enforced-before-protected-work, route-rollback-disables-without-mutating-canonical-truth |
| Envelope over the response bound | `response_bytes` equal to the floor with a degraded lane, so the envelope alone exceeds it. | route-bounds-are-enforced-before-protected-work |
| Embedding lane busy, starting, disabled, failing, or faulted | A `QueryEmbedder` installed through `set_query_embedder_for_test`; `DenseLane::Unavailable` at the `execute` level. | route-dense-unavailable-degrades-typed-within-deadline |
| Inference declares its artifact unusable | `TestEngine::fail_next(InferenceError::Artifact(..))` on the daemon's engine before one request. | route-dense-unavailable-degrades-typed-within-deadline |
| Request without prose against a ready dense lane | A selector-only query with `DenseLane::Ready` at the `execute` level; a counting embedder through the handler. | route-dense-unavailable-degrades-typed-within-deadline |
| Deadline lapses during the embedding await | A scripted embedder that sleeps past `remaining_ms`. | route-query-embedding-completes-before-blocking-scan, route-cancellation-is-observed-in-every-phase |
| Corrupt stored vector | A zero-norm vector stored under the fixture generation. | route-dense-unavailable-degrades-typed-within-deadline |
| Cancellation inside the dense producer | A `DenseProducer` that cancels the token before delegating to the exhaustive oracle. | route-cancellation-is-observed-in-every-phase |

Real durations are used because `EvalBudget` reads `std::time::Instant`, which
virtual time cannot advance.

## Coverage checks to add

None. Every fault row above is constructed by a named test in the catalog's
`Exercised:` field, and every record is `always`, so no `sometimes` or
`reachable` marker is needed to prove a window was reached. The permit and pin
census of `route-permits-and-pins-outlive-client-cancellation` needs new rows
only once U3b and U3c add the state they count.

## Leverage ranking

1. The storage negative control (`a_leaked_progress_handler_interrupts_the_next_read_on_the_connection`)
   is the cheapest oracle: it proves the untouched-later-read check can fail,
   so the request-local record's passing test is evidence rather than silence.
2. The file-backed `request_budget_reads.rs` tests cover three of the seven
   fault rows with one fixture and no host.
3. The host witnesses are the most expensive and the only ones that exercise
   the abort-while-suspended window; run them last.
