# dreamer-dispatched-attempt-always-settles-through-the-receipt

## Discovery trigger

The N1 specification (issue 306) requires that a Dreamer retry after an unknown
outcome never creates a second billable attempt, and that a daemon restart
mid-run resolves to exactly one recorded attempt or a fail-closed unknown
outcome. The receipt ledger (`crates/memory-store/src/dreamer_ledger.rs`) made
the route's writes checkable; this record states the invariant the route must
keep over that ledger, and replaces
`h4c-dreamer-failure-path-ledger-write-is-unchecked`, whose subject (an ignored
write on the failure path) no longer exists.

## Evidence trail

Verified at HEAD. References are to `crates/daemon/src/lib.rs` unless stated.

- `handle_dreamer_run_task` validates the request, passes the memories
  authority `MODULE` gate, takes the in-process duplicate guard, then judges the
  per-project attempt budget from `count_dreamer_attempts` before any receipt
  is written: an exhausted project with no receipt for the command answers
  `dreamer_budget_exhausted` and writes nothing; a command the ledger already
  holds proceeds to `begin_dreamer_receipt`, so it replays, refuses a changed
  request, or settles under the same rules as below, and only a takeover
  (which would dispatch) is refused over budget.
- `begin_dreamer_receipt` is the first write (`:9570`). Each attempt row records
  the `project_root` and `harness` it was dispatched under
  (`DreamerAttemptSpec`, `crates/memory-store/baseline.sql:373-374`), which with
  `child_session` is the run identity the runtime keys on; the receipt row
  carries no harness of its own, since the retry may arrive from another
  harness and the probe must use the one the run was started under. `Complete`
  replays; `DigestConflict` and `BindingMismatch` answer
  `dreamer_request_conflict` with no producer constructed; `InProgress`
  goes to `resume_dreamer_receipt` (`:9868`).
- `resume_dreamer_receipt` reads `list_dreamer_attempts` and picks the newest
  attempt at the open generation whose terminal is not `not_sent`. No such
  attempt: `take_over_dreamer_receipt` moves the fence to `g + 1` (Applied) or
  the request is `dreamer_ledger_fenced`. An ended attempt: complete the
  receipt `unknown` through `complete_receipt_as_unknown` (`:13673`), leaving
  the attempt's own terminal in place. An open attempt with a handle: connect
  under the attempt's recorded `project_root` and `harness`, bind the recorded
  child session, and call `status`; `Missing` and `Terminal` settle `unknown`
  (a terminal run is final for the runtime and its answer was never recorded,
  so a later retry could only repeat the probe), `Active` answers
  `dreamer_outcome_unknown` with no write, and a connect or status error answers
  the same. An open attempt with no handle:
  settle `unknown`. Settling an open attempt goes through
  `settle_dispatched_attempt_as_unknown` (`:13653`), which writes the attempt
  terminal best-effort and then completes the receipt through the same helper,
  matching `Applied`, `Fenced`, and `Err` separately.
- In the chain loop, `begin_dreamer_attempt` precedes `start` (`:9653`), and
  `record_dreamer_run_handle` follows a successful `start` (`:9699`); a handle
  write that is `Fenced` or fails purges the session and settles `unknown`.
  The await runs under `classify_attempt_timeout(CLASSIFY_AWAIT_TIMEOUT,
  deadline)`; because the request deadline is clamped to that same ceiling, an
  await that times out has spent the whole request budget, the attempt ends
  `cancelled`, and nothing more is read from the run.
  `finish_dreamer_attempt` records the attempt terminal; when it does not land,
  a usable result is still offered to `complete_dreamer_receipt` first, and
  otherwise the request settles `unknown`.
- The exhausted-chain write (`:9820`) and the success write (`:9835`) match
  `Applied`, `Fenced`, and `Err`; only `Applied` answers with the receipt's
  recorded response, read back through `read_dream_task_response`.
- `attempt_child_session_id` (`crates/daemon/src/classify.rs`) hashes the
  receipt generation with the request identity, attempt index, and model, so a
  successor generation cannot derive a predecessor's session.
- Bounds: `timeout_ms` is clamped to `CLASSIFY_MAX_REQUEST_TIMEOUT`
  (`classify.rs:23`, equal to the await ceiling) by `classify_request_timeout`
  at `:9491`; the chain is
  capped at `MAX_CLASSIFY_MODEL_CHAIN` at parse time; the budget is
  `DREAMER_ATTEMPT_BUDGET` per `DREAMER_ATTEMPT_BUDGET_WINDOW`
  (`classify.rs:30-31`). `count_dreamer_attempts` excludes `not_sent` rows and
  counts open, `cancelled`, and length-capped attempts alike.
- Ledger fences: every transition is a row-predicate `UPDATE` on key, generation,
  and `in_progress` state (`dreamer_ledger.rs:568`, `guarded_transition`), so a
  predecessor generation's write after takeover is `Fenced`, not applied
  (`crates/memory-store/tests/dreamer_ledger.rs`,
  `a_higher_generation_takes_over_and_the_predecessor_is_fenced_on_every_transition`).

## Failure scenario

A daemon dispatches a classify run and is killed while awaiting the model. On
restart the client retries the same `command_id`. Without this property the
route would either replay a success it never recorded or start the chain again
and bill the model twice. With it, the retry finds the open attempt's run handle,
asks the runtime, and settles the command `unknown` when the runtime no longer
knows the run; every later retry replays that `unknown` and the producer is
never started again.

## Timing windows and dependencies

- Between `begin_dreamer_receipt` and `begin_dreamer_attempt` a crash leaves a
  receipt with no marker; the successor takes over and dispatches once.
- Between `start` returning and `record_dreamer_run_handle` a crash leaves an
  open attempt with no handle; the successor settles `unknown` because the
  dispatch cannot be disproved.
- Between `record_dreamer_run_handle` and `finish_dreamer_attempt` a crash
  leaves an open attempt with a handle; the successor resolves it.
- Between `finish_dreamer_attempt` and `complete_dreamer_receipt` a crash leaves
  an ended attempt under an open receipt; the successor settles `unknown` rather
  than replaying a result it does not hold.
- The budget check and the receipt write are two reads apart, so concurrent
  requests for different commands can exceed the budget by their number; the
  bound is a guard against runaway spend, not an exact quota.
- The property depends on the runtime reporting a restarted host's old run ids
  as `missing` (`docs/host-wire-protocol.md`), on a terminal run status never
  returning to active within one runtime incarnation, and on the scripted
  producer's `status` being answerable in tests.

## What a test must construct

1. A classify run dropped while awaiting output, then a retry with `status`
   scripted `Missing`: assert one start, attempt and receipt terminal `unknown`,
   and a further retry replaying `unknown` with one start still.
2. The same with `status` `Active`: assert one start, receipt still
   `in_progress`, attempt still open.
3. The same with `status` `Terminal`: assert one start, attempt and receipt
   terminal `unknown`, and a further retry replaying `unknown` with no second
   `status` call.
4. A receipt forced `in_progress` with no attempt rows: assert the retry
   dispatches once at generation 2 under a generation-derived session and the
   predecessor's `finish_dreamer_attempt` is `Fenced`.
5. A timed-out await with a late answer queued behind it: assert the attempt
   ends `cancelled`, no second read of the run happens, the attempt counts
   toward the budget, and the receipt completes `failed`.
6. A `timeout_ms` above the ceiling: assert every await the producer saw is at
   most `CLASSIFY_MAX_REQUEST_TIMEOUT`.
7. The durable attempt count filled to the budget: assert a new command answers
   `dreamer_budget_exhausted` with no receipt and no start, and a completed
   command still replays.

The tests named in the catalog record construct exactly these.

## Investigation log

### Q: Why is a retry of an ended attempt settled `unknown` rather than resumed?

- Sources examined: the chain loop's `finish_dreamer_attempt` and
  `complete_dreamer_receipt` order; the historian's reattach path
  (`crates/daemon/src/historian.rs`, `reattach_historian_producer`).
- Findings: the model's answer is not durable until `complete_dreamer_receipt`,
  and the crashed daemon may have purged the child session, so the answer
  cannot be re-read. Dispatching again would bill twice. The historian refires
  because its work is idempotent summarization; a classify run is a paid call
  the specification says must never be repeated on an unknown outcome.
- Conclusion: resolved with answer; the asymmetry is deliberate.

### Q: Why does a `Terminal` status settle rather than wait for a later retry?

- Sources examined: the Broca supervisor's run status transitions and terminal
  retention (`crates/host-runtime/src/broca/supervisor.rs`,
  `crates/host-runtime/src/broca/config.rs`, `TERMINAL_RETENTION`).
- Findings: a run reported terminal stays terminal until the runtime forgets it
  after the retention window, when `status` answers `missing`. Leaving the
  receipt open would repeat the connect, bind, and status probe on every retry
  for that window and then settle `unknown` anyway. The spec accepts a
  fail-closed `unknown` for a restart mid-run, so the receipt settles at the
  first probe that proves the run can never become active.
- Conclusion: resolved with answer; `Terminal` shares the `Missing` arm's write.

### Q: Why is there no second read of the run after a timed-out await?

- Sources examined: `classify_request_timeout`, `classify_attempt_timeout`, and
  the await call in the chain loop.
- Findings: the request deadline is clamped to `CLASSIFY_AWAIT_TIMEOUT`, and the
  await runs under `min(CLASSIFY_AWAIT_TIMEOUT, deadline - now)`, which is the
  remaining request budget. An await that times out has therefore reached the
  deadline, and any further read would run under a zero window. The attempt
  ends `cancelled` and the session is purged.
- Conclusion: resolved with answer; the deadline is the single bound.

### Q: Does the budget check race with a concurrent request?

- Sources examined: `count_dreamer_attempts` and the check's placement before
  `begin_dreamer_receipt`.
- Findings: two requests can both read `spent = budget - 1` and both dispatch,
  so the bound can be exceeded by the number of concurrent requests. The bound
  is a host guard against runaway spend, not an exact quota.
- Conclusion: resolved with answer; recorded as a known slack.
