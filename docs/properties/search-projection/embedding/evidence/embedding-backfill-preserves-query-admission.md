# embedding-backfill-preserves-query-admission

## Discovery trigger

P1 line 149 requires saturated backfill to preserve declared query admission.
P7 line 81 includes queue wait and embedding inside one absolute query budget.
This is a planned service guarantee, not a claim that current FIFO is defective.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 priority driver exists.

## Evidence trail

- `crates/host-runtime/src/synapse/mod.rs:206-225` has separate query admission
  capacity and batch JobTable bounds but one shared FIFO CPU semaphore.
- `mod.rs:634-668` computes the query deadline before nonblocking admission.
- `mod.rs:678-722` waits for CPU within the tracked query worker and checks the
  deadline again before native work.
- `mod.rs:843-871` registers batch workers on the same CPU semaphore.
- `crates/host-runtime/tests/synapse_protocol.rs:186-233` constructs mixed
  query/batch order and asserts query, query, batch, query. It tests FIFO.
- `crates/host-runtime/src/synapse/bundle.rs:582-604` checks coexistence of
  query memory, queue metadata, and scratch. It does not impose query priority.
- Existing admission and request records are reused for these local gates.

## Failure scenario

Backfill admits enough batch workers to occupy the shared CPU wait queue.
A query obtains its separate query slot but spends its remaining deadline
behind that work. Continuing backfill can reproduce the delay every time.
Admission-slot isolation alone therefore cannot prove useful query service.
A different failure rejects an in-envelope query because background work
consumed shared resident capacity that the declared query contract requires.
Both must be evaluated against an approved workload and service envelope.

## Timing windows and dependencies

Arrival, query admission, CPU registration, native start, and response are
different events. Record them separately rather than inferring one from another.
An already started native call is nonpreemptive in the existing design; query
priority cannot be interpreted as immediate CPU ownership while it runs.
The allowed work ahead of a query, native service envelope, query capacity,
and maximum wait must come from the admission owner and RP2.9.
Cancellation and expired D permit explicit non-success, not fabricated service.
P7 allows dense-lane unavailability to degrade retrieval explicitly.
The async query lane and EvalBudget bridge are shared prerequisites with
RP2.7.U3 (P7 lines 142-147). RP2.1.U3 integrates priority with exact preflight
and JobTable; RP2.7 owns the authorized full query path. Do not schedule that
whole path inside RP2.1 merely because this property observes its admission.

## What a test must construct

Use the existing gated engine and make backfill reach its declared limit before
offering a query while query occupancy is below its cap. Hold and release
native calls under the approved service bound. Replenish background work long
enough to expose service ordering, then stop pressure for recovery observation.
Assert both admission preservation and bounded start/completion where D permits.
Run above-envelope queries separately to check explicit overload disposition.
`search_projection_embedding_query_arrives_during_backfill_saturation` records occupancy and
arrival before any admission decision, so rejection is not needed to fire it.
Do not reuse the FIFO test's wall-clock values as production service limits.
The handoff chooses the cheapest admission/service oracle before load testing.
Evaluate each admitted episode whose premises are independently constructed,
not only queries the implementation admits successfully. RP2.9-unapproved
bounds block acceptance; optional/unreached inference cannot satisfy the
[shared acceptance check](../../projection-coverage/catalog.md#projection-acceptance-situations-witnessed).

## Investigation log

### Q: Does the current FIFO test establish query priority?

- Sources examined: `mod.rs:206-225`, `:843-851`; mixed FIFO test at line 186.
- Findings: Query and batch workers share registration order; the test expects
  a batch to run before a later query. It does not construct saturated backfill.
- Missing evidence: None for this distinction between FIFO and priority.
- Conclusion: Resolved. Reuse FIFO evidence only for the existing contract.

### Q: What precise service promise should the new campaign enforce?

- Sources examined: P1 U3, P2 bounds, P7 deadline and degradation contracts.
- Findings: Query-priority admission is required but capacities, queueing
  allowance, and service measurements are intentionally deferred to RP2.9.
- Missing evidence: Approved L and a declared nonpreemptive-work allowance.
- Conclusion: Needs human input. The catalog does not choose strict priority,
  weighted scheduling, a reserve size, or a starvation timeout.
  The shared RP2.7 query-lane prerequisite and RP2.1.U3 priority integration
  must both remain explicit in the implementation handoff.
