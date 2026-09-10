# embedding-supervisor-shares-budget-and-joins

## Discovery trigger

P4 assigns embedding backfill/GC to the shared supervisor. P2 line 53 and P7
line 81 require one EvalBudget through embedding, SQLite, and dense work,
with physical ownership retained after client cancellation.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 embedding supervisor
slice or shared Synapse EvalBudget bridge exists at the pinned revision.

## Evidence trail

- `crates/daemon/src/lib.rs:3646-3660` starts the existing DreamerScheduler
  after store open with the daemon cancellation token and task admission owner.
- `crates/daemon/src/dreamer_scheduler.rs:163-220` races cancellation with the
  whole tick and sleeps after deferred/retained work rather than busy retrying.
- `dreamer_scheduler.rs:334-371` uses one review-user-memories task identity.
- `crates/kernel/src/applicability/checkout.rs:146-203` defines cloneable
  EvalBudget, sticky cancellation, absolute deadline, and BudgetExhausted.
- `crates/kernel/src/open.rs:1379-1387` installs SQLite progress privately;
  retrieval cannot simply call that installer from another crate.
- `crates/host-runtime/src/handler.rs:450-455` exposes request cancellation.
- `crates/host-runtime/src/synapse/mod.rs:669-722` retains a query permit and
  text charge in the tracked worker through its joined native call.
- `mod.rs:1148-1161` joins tracked work and the synchronous CPU holder at stop.

## Failure scenario

Each stage constructs a fresh relative deadline, extending the request beyond
its original D. Or an expired receiver drops its permit while native work
continues, allowing more resident/running work than the declared capacity.
A separate embedding timer can keep admitting work after the shared daemon
supervisor stops. An unbounded sweep can also prevent another due slice from
running despite apparently bounded individual model requests.
None of these failures requires a malformed request or an incorrect vector.

## Timing windows and dependencies

Budget derivation precedes queue wait. Counting, async inference admission,
blocking work, and later SQLite/dense checkpoints must preserve the same D
and cancellation flag. Rust Instant is local and must not cross the wire.
Query response cancellation does not prove that a started native call stops.
Current shutdown has no finite native-call completion deadline. Logical
cancellation latency and physical drain therefore need different contracts.
Maintenance uses its own approved bounded slice/lease, not a borrowed caller's
expired request. Reusing the supervisor does not mean reusing Dreamer task IDs.
The async query lane and EvalBudget adapter are cross-plan prerequisites
shared with RP2.7.U3 (P7 lines 142-147). RP2.1.U3 integrates priority admission
and embedding maintenance; RP2.7 owns the authorized full query route and its
later stages. This property observes the shared seam without moving that scope.

## What a test must construct

Observe at least two stage entries under one budget, then cross D or cancel
between them. Hold native work behind a gate and observe the cancellation
response while its task, CPU permit, and charges remain live. Release the gate
and observe physical completion before counting those resources as reclaimed.
Make another maintenance slice due while a bounded embedding slice runs.
Check work counters and admission after stop, not just a task handle's status.
`search_projection_embedding_stop_occurs_with_native_work_held` witnesses stop plus physical
work; `search_projection_embedding_budget_crosses_between_stages` witnesses deadline carry.
For a never-returning backend, report the missing physical-drain guarantee
instead of interpreting an arbitrary harness timeout as a successful join.

## Investigation log

### Q: Are shared cancellation and a supervisor entirely new primitives?

- Sources examined: EvalBudget, RequestCtx, DreamerScheduler, Synapse tracker.
- Findings: All exist separately. The missing work is the RP2.1 bridge and
  bounded embedding registration, with the query lane/EvalBudget adapter shared
  with RP2.7. It is not another timer or cancellation authority.
- Missing evidence: None for existence; production composition remains absent.
- Conclusion: Resolved. Reuse existing owners and preserve their actual scope.

### Q: What bounds physical shutdown of an uncooperative native call?

- Sources examined: `mod.rs:678-722`, `:1148-1161`; P1 bounds; P7 line 81.
- Findings: Started calls are joined, not preempted. Deadline expiry only
  stops later work and reporting; a finite native-drain bound is not established.
- Missing evidence: Approved native service/drain contract, public budget
  bridge, supervisor slice registration, and RP2.9 cancellation bounds.
- Conclusion: Needs human input. No timeout is asserted to terminate native code.
