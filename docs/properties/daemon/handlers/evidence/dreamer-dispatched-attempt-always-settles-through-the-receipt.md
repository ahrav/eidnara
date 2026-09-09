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

Verified against the merged working tree. References are to
`crates/daemon/src/lib.rs` unless stated. The default dispatch arm at `:12719`
reaches `handle_dreamer_run_task` without a feature or configuration gate.

- `handle_dreamer_run_task` validates the request, passes the memories
  authority `MODULE` gate, takes the in-process duplicate guard, then judges the
  per-project attempt budget from `count_dreamer_attempts` before any receipt
  is written: an exhausted project with no receipt for the command answers
  `dreamer_budget_exhausted` and writes nothing; a command the ledger already
  holds proceeds to `begin_dreamer_receipt`, so it replays, refuses a changed
  request, or settles under the same rules as below, and only a takeover
  (which would dispatch) is refused over budget (`:9339-9594`, `:9930-9939`).
- The request digest includes the await timeout, output-token limit, and
  temperature along with the request, prompt, and schema inputs
  (`:9526-9539`). There is no recovery-timeout digest input or second recovery
  read after a timed-out await.
- `begin_dreamer_receipt` is the first write (`:9570`). Each attempt row records
  the `project_root` and `harness` it was dispatched under
  (`DreamerAttemptSpec`, `crates/memory-store/baseline.sql:373-374`), which with
  `child_session` is the run identity the runtime keys on; the receipt row
  carries no harness of its own, since the retry may arrive from another
  harness and the probe must use the one the run was started under. `Complete`
  replays; `DigestConflict` and `BindingMismatch` answer
  `dreamer_request_conflict` with no producer constructed; `InProgress`
  goes to `resume_dreamer_receipt` (`:9914`).
- `resume_dreamer_receipt` reads `list_dreamer_attempts` and picks the newest
  attempt at the open generation whose terminal is not `not_sent`. No such
  attempt: `take_over_undispatched_dreamer_receipt` (`:9934`) atomically
  rechecks the current generation before moving the fence to `g + 1`.
  Its `NOT EXISTS` predicate matches rows with
  `terminal_kind IS NULL OR terminal_kind != 'not_sent'`
  (`crates/memory-store/src/dreamer_ledger.rs:421-429`). No current-generation
  row or only `NotSent` rows permits takeover; any possible dispatch blocks it.
  Older-generation rows do not block it. An attempt inserted after the list
  read is therefore not missed by the fence move. `Applied` admits the
  successor; `Fenced` answers `dreamer_ledger_fenced`, and a store error answers
  `dreamer_ledger_failed`. An ended attempt: complete the
  receipt `unknown` through `complete_receipt_as_unknown` (`:13742`), leaving
  the attempt's own terminal in place. An open attempt with a handle: connect
  under the attempt's recorded `project_root` and `harness`, bind the recorded
  child session, and call `status`, all three under one `tokio::time::timeout`
  set to the request deadline's remaining duration; `Missing` and `Terminal`
  settle `unknown` (a terminal run is final for the runtime and its answer was
  never recorded, so a later retry could only repeat the probe), `Active`
  answers `dreamer_outcome_unknown` with no write, and a connect, bind, or
  status error answers the same, as does a probe the deadline cuts off
  (`:9954-10018`). An open attempt with
  no handle: settle `unknown`. Settling an open attempt goes through
  `settle_dispatched_attempt_as_unknown` (`:13722`), which writes the attempt
  terminal best-effort and then completes the receipt through the same helper,
  matching `Applied`, `Fenced`, and `Err` separately.
- In the chain loop, `begin_dreamer_attempt` precedes `start` (`:9671-9721`), and
  `record_dreamer_run_handle` follows a successful `start` (`:9733`). A fenced
  handle write skips purge; a store error still attempts purge. Both try unknown
  settlement, whose receipt write reports `Fenced` if another generation owns
  the receipt (`:9739-9750`, `:13753-13762`). The await-terminal write has the
  same no-purge rule when fenced (`:9782-9815`).
  The await runs under `classify_attempt_timeout(CLASSIFY_AWAIT_TIMEOUT,
  deadline)`; because the request deadline is clamped to that same ceiling, an
  await that times out has spent the whole request budget, the attempt ends
  `cancelled`, and nothing more is read from the run (`:9753-9773`).
  `finish_dreamer_attempt` records the attempt terminal; when it does not land,
  a usable result is still offered to `complete_dreamer_receipt` first, and
  otherwise the request tries unknown settlement (`:9775-9815`). A generation
  fence prevents that settlement from overwriting the successor's receipt.
- The exhausted-chain write (`:9862`) and the success write (`:9877`) match
  `Applied`, `Fenced`, and `Err`; only `Applied` answers with the receipt's
  recorded response, read back through `read_dream_task_response` (`:13766`).
- `attempt_child_session_id` (`crates/daemon/src/classify.rs:206-227`) hashes the
  receipt generation with the request identity, attempt index, and model, so a
  successor generation cannot derive a predecessor's session.
- Bounds: `timeout_ms` is clamped to `CLASSIFY_MAX_REQUEST_TIMEOUT`
  (`crates/daemon/src/classify.rs:23-27`, equal to the await ceiling) by
  `classify_request_timeout` at `:9491`; the chain is
  capped at `MAX_CLASSIFY_MODEL_CHAIN` at parse time (`:9464-9468`); the budget is
  `DREAMER_ATTEMPT_BUDGET` per `DREAMER_ATTEMPT_BUDGET_WINDOW`
  (`crates/daemon/src/classify.rs:30-31`), read through
  `dreamer_attempt_budget_exhausted` (`:13667`) once before the receipt is
  written and again before every model after the first (`:9627-9638`), so one
  admitted chain overshoots the budget by at most the attempt it was admitted
  for. `count_dreamer_attempts` excludes
  `not_sent` rows and counts open, `cancelled`, and length-capped attempts alike
  (`crates/memory-store/src/dreamer_ledger.rs:692-708`).
- Ledger fences: takeover, attempt insertion, handle recording, attempt finish,
  and receipt completion each predicate their write on the receipt key,
  generation, and `in_progress` state
  (`crates/memory-store/src/dreamer_ledger.rs:410-572`). The shared
  `guarded_transition` executes its statement at `dreamer_ledger.rs:605-621`;
  receipt completion executes its guarded update at `dreamer_ledger.rs:555-572`.
  A predecessor generation's write after takeover is `Fenced`, not applied
  (`crates/memory-store/tests/dreamer_ledger.rs:437-554`,
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
  receipt with no marker; an admitted successor takes over and may dispatch.
- Between the resume list read and takeover, a predecessor may insert an attempt.
  The atomic predicate sees that possible dispatch and fences the takeover;
  if takeover wins first, the predecessor's attempt insert is fenced
  (`crates/memory-store/src/dreamer_ledger.rs:421-429`, `:456-469`). A generation
  containing only `NotSent` attempts remains eligible.
- Between `start` returning and `record_dreamer_run_handle` a crash leaves an
  open attempt with no handle; the successor settles `unknown` because the
  dispatch cannot be disproved.
- Between `record_dreamer_run_handle` and `finish_dreamer_attempt` a crash
  leaves an open attempt with a handle; the successor resolves it.
- A generation change during `start` or `await_output` fences the returning
  predecessor's write. It skips purge and cannot complete the generation-2
  receipt (`:9739-9750`, `:9782-9815`).
- Between `finish_dreamer_attempt` and `complete_dreamer_receipt` a crash leaves
  an ended attempt under an open receipt; the successor settles `unknown` rather
  than replaying a result it does not hold.
- The budget check and the attempt write are two reads apart, so concurrent
  requests for different commands can each exceed the budget by one attempt;
  the re-read before every later model keeps a single chain from adding its
  length to that slack. The bound is a guard against runaway spend, not an
  exact quota.
- The recovery probe shares the request deadline. A runtime that accepts the
  `status` call and never answers is cut off at the deadline and the retry
  answers `dreamer_outcome_unknown` with no write; the receipt stays open for a
  later retry with more budget.
- The property depends on the runtime reporting a restarted host's old run ids
  as `missing` (`docs/host-wire-protocol.md:384`), on a terminal run status never
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
4. A receipt stranded `in_progress` by an aborted attempt insert: assert the
   retry dispatches once at generation 2 (`:27524-27577`). Also reopen a receipt
   with a `NotSent` attempt: assert one successor dispatch under a
   generation-derived session and a fenced predecessor `finish_dreamer_attempt`
   (`:28103-28186`).
5. A timed-out await with a late answer queued behind it: assert the attempt
   ends `cancelled`, no second read of the run happens, the attempt counts
   toward the budget, and the receipt completes `failed`.
6. A `timeout_ms` above the ceiling: assert every await the producer saw is at
   most `CLASSIFY_MAX_REQUEST_TIMEOUT`.
7. The durable attempt count filled to the budget: assert a new command answers
    `dreamer_budget_exhausted` with no receipt and no start, and a completed
    command still replays.
8. No current-generation attempt rows, then an open row, then a `NotSent` row:
   assert atomic takeover permits the first and third states and rejects the
   second. Older-generation rows do not block takeover; a failed
   current-generation attempt does
   (`crates/memory-store/tests/dreamer_ledger.rs:281-347`).
9. Move the receipt to generation 2 in each of the scripted producer's
   `on_start` and `on_await_output` hooks. Assert `dreamer_ledger_fenced`, one
   start, no purge, the generation-2 receipt still `in_progress`, and the
   generation-1 attempt still open. The start window performs no await; the
   await window performs exactly one (`:27580-27646`).
10. The durable attempt count at `budget - 1` and a three-model chain whose
    every model fails: assert one start, one `failed` attempt row, the receipt
    `complete` as `failed` with a budget message, the count at exactly the
    budget, and a retry replaying the failure with no new start.
11. A dispatched attempt whose retry finds the runtime accepting `status` and
    never answering, under a `timeout_ms` both legs share: assert the retry
    returns `dreamer_outcome_unknown` within the deadline, one `status` call,
    no start, and the receipt and attempt still open.

The tests named in the catalog record construct these states. The cleanup test
is `dreamer_run_task_does_not_purge_the_child_session_once_it_is_fenced`; the
ledger predicate test is
`an_undispatched_receipt_can_be_taken_over_only_without_a_possible_dispatch`.
The latter checks the allowed and blocked states, not a concurrent schedule
between the daemon's list read and takeover. That interleaving's exclusion is
established here by the single guarded statement, not by a race test.

## Investigation log

### Q: Why is a retry of an ended attempt settled `unknown` rather than resumed?

- Sources examined: the chain loop's `finish_dreamer_attempt` and
  `complete_dreamer_receipt` order; the historian's reattach path
   (`crates/daemon/src/historian.rs:1366-1418`, `reattach_historian_producer`).
- Findings: the model's answer is not durable until `complete_dreamer_receipt`,
  and the crashed daemon may have purged the child session, so the answer
  cannot be re-read. Dispatching again would bill twice. The historian refires
  because its work is idempotent summarization; a classify run is a paid call
  the specification says must never be repeated on an unknown outcome.
- Conclusion: resolved with answer; the asymmetry is deliberate.

### Q: Why does a `Terminal` status settle rather than wait for a later retry?

- Sources examined: the Broca supervisor's run status transitions and terminal
  retention (`crates/host-runtime/src/broca/supervisor.rs:472-485`,
  `crates/host-runtime/src/broca/config.rs:148`, `TERMINAL_RETENTION`).
- Findings: a run reported terminal stays terminal until the runtime forgets it
  after the retention window, when `status` answers `missing`. Leaving the
  receipt open would repeat the connect, bind, and status probe on every retry
  for that window and then settle `unknown` anyway. The spec accepts a
  fail-closed `unknown` for a restart mid-run, so the receipt settles at the
  first probe that proves the run can never become active.
- Conclusion: resolved with answer; `Terminal` shares the `Missing` arm's write.

### Q: Why is there no second read of the run after a timed-out await?

- Sources examined: `classify_request_timeout`, `classify_attempt_timeout`, and
  the await call in the chain loop (`:9491`, `:9753-9761`).
- Findings: the request deadline is clamped to `CLASSIFY_AWAIT_TIMEOUT`, and the
  await runs under `min(CLASSIFY_AWAIT_TIMEOUT, deadline - now)`, which is the
  remaining request budget. An await that times out has therefore reached the
  deadline, and any further read would run under a zero window. The attempt
  ends `cancelled` and cleanup attempts to purge the session unless its terminal
  write is fenced (`:9764-9815`, `:9835-9850`).
- Conclusion: resolved with answer; the deadline is the single bound.

### Q: Does the budget check race with a concurrent request?

- Sources examined: `dreamer_attempt_budget_exhausted`, its call before
  `begin_dreamer_receipt`, and its call at the top of the chain loop.
- Findings: two requests can both read `spent = budget - 1` and both dispatch,
  so the bound can be exceeded by the number of concurrent requests. Within one
  request the count is re-read before each model after the first, so a chain
  admitted at `budget - 1` dispatches once and then fails closed through its
  receipt; without that re-read a single eight-model chain could add seven
  attempts past the budget with no concurrency at all. The bound is a host guard
  against runaway spend, not an exact quota.
- Conclusion: resolved with answer; recorded as a known slack of one attempt
  per concurrent request.

### Q: Why does the recovery probe run under the request deadline?

- Sources examined: `resume_dreamer_receipt`'s connect, bind, and `status`
  calls; `HistorianProducer::status` and `DEFAULT_REQUEST_TIMEOUT`
  (`crates/daemon/src/historian_producer.rs:32`, `:903-912`).
- Findings: the producer's `status` call runs under its own 30 s request
  timeout, and the connect and bind before it have no timeout of their own. A
  retry with a small `timeout_ms` against a runtime that accepted the probe and
  stalled would occupy the request for the producer's timeout rather than the
  caller's, contradicting the whole-request bound. The three calls now run
  under one `tokio::time::timeout` set to the deadline's remaining duration; a
  cut-off probe answers `dreamer_outcome_unknown` with no write, the same shape
  as a runtime whose status cannot be read.
- Conclusion: resolved with answer; the deadline bounds recovery as well as
  dispatch.
