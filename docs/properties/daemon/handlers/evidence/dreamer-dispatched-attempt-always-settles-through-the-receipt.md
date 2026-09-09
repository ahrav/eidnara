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

- `handle_dreamer_run_task` validates the request shape (`ClassifyRequest::parse`:
  kernel object ids in the bound project, a model chain, and a timeout; a
  host-rendered prompt or claim-lane items are refused) and hands
  `DreamerRuntime::run_dreamer_task` the parsed task plus the route facts
  (`DreamerRoute`, including the kernel project); that shared protocol passes
  the memories authority
  `MODULE` gate, takes the in-process duplicate guard, then judges the
  per-project attempt budget from `count_dreamer_attempts` before any receipt
  is written: an exhausted project with no receipt for the command answers
  `dreamer_budget_exhausted` and writes nothing; a command the ledger already
  holds proceeds to `begin_dreamer_receipt`, so it replays, refuses a changed
  request, or settles under the same rules as below, and only a takeover
  (which would dispatch) is refused over budget.
- `begin_dreamer_receipt` is the first write (`:9638`); it records the binding
  including the harness the request arrived on (`dreamer_ledger.rs`,
  `DreamerReceiptBinding.harness`, column `harness` in
  `crates/memory-store/baseline.sql:349`). Each attempt row records the
  `project_root` and `harness` it was dispatched under
  (`DreamerAttemptSpec`, `baseline.sql:374-375`), which with `child_session`
  is the run identity the runtime keys on. `Complete`
  replays; `DigestConflict` and `BindingMismatch` answer
  `dreamer_request_conflict` with no producer constructed; `InProgress`
  goes to `resume_dreamer_receipt` (`:10036`).
- Only once a dispatch is owed does `render_pool` (`:13811`) read the named
  memories' canonical rows at the kernel tip and render the prompt
  (`classify.rs`, `render_classify_prompt`, bodies escaped). The failures split
  by what a retry can change (`PoolFailure`, `:13778`): an id the bound project
  does not serve, a non-memory decision, a pool past the prompt byte bound, or
  a read the kernel truncated at its payload budget is a request failure that
  completes the receipt `failed` with an `invalid_params` or
  `payload_too_large` reply (`:9693-9706`), so a retry replays the refusal; a
  kernel that is not open, unavailable, or busy answers `kernel_unavailable`
  and returns with the receipt still open and no attempt (`:9692`), so a retry
  takes the receipt over and reads again. Replays and resumes never read the
  kernel.
- `resume_dreamer_receipt` reads `list_dreamer_attempts` and picks the newest
  attempt at the open generation whose terminal is not `not_sent`. No such
  attempt: `take_over_dreamer_receipt` moves the fence to `g + 1` (Applied) or
  the request is `dreamer_ledger_fenced`. An ended attempt: complete the
  receipt `unknown` through `complete_receipt_as_unknown` (`:14420`), leaving
  the attempt's own terminal in place. An open attempt with a handle: connect
  under the attempt's recorded `project_root` and `harness`, bind the recorded
  child session, and call `status`; `Missing` settles `unknown`,
  `Active` or `Terminal` answers `dreamer_outcome_unknown` with no write, a
  connect or status error answers the same. An open attempt with no handle:
  settle `unknown`. Settling an open attempt goes through
  `settle_dispatched_attempt_as_unknown` (`:14400`), which writes the attempt
  terminal best-effort and then completes the receipt through the same helper,
  matching `Applied`, `Fenced`, and `Err` separately.
- In the chain loop, `begin_dreamer_attempt` precedes `start` (`:9749`), and
  `record_dreamer_run_handle` follows a successful `start` (`:9795`); a handle
  write that is `Fenced` or fails purges the session and settles `unknown`.
  `finish_dreamer_attempt` records the attempt terminal; when it does not land,
  a usable result is still offered to `complete_dreamer_receipt` first, and
  otherwise the request settles `unknown`.
- Model output is accepted only by `parse_classify_output` (`classify.rs`):
  one envelope, exact coverage, closed domains for importance, scope, and
  shareable, no unknown attributes; a rejection names the rule and never the
  text. Accepted classifications are written to the kernel by
  `record_classifications` (`:13897`) before the receipt completes: one
  `memory_classification` observation per memory in the memory domain, in the
  project's scope, depending on the memory, admitted under
  `(ModelInference, DreamerInference)` from the code path, committed under
  `("dreamer.classify", <receipt operation key>, <receipt digest>)` so a repeat
  under the same receipt replays the kernel's receipt. In the same commit, a
  memory's earlier live classification is retired
  (`Envelope::live_dependent_observations`, `retire_observation`), so the live
  set is one per memory and successive runs do not grow it. A kernel write
  failure completes the receipt `failed` as `dreamer_kernel_write_failed`
  (`:9996`); a `Conflict` is classified as `kernel.commit` classifies it (a
  reused key, a reserved scope, or a storage constraint) and the failure names
  that reason.
- The exhausted-chain write (`:9948`) and the success write (`:10002`) match
  `Applied`, `Fenced`, and `Err`; only `Applied` answers with the receipt's
  recorded response, read back through `read_dream_task_response`. The reply
  carries the commit sequence and the count written, never the model's text.
- `attempt_child_session_id` (`crates/daemon/src/classify.rs`) hashes the
  receipt generation with the request identity, attempt index, and model, so a
  successor generation cannot derive a predecessor's session.
- Bounds: `timeout_ms` is clamped to `CLASSIFY_MAX_REQUEST_TIMEOUT`
  (`classify.rs:41`, equal to the await ceiling) by `classify_request_timeout`
  at `:9558`; the chain is
  capped at `MAX_CLASSIFY_MODEL_CHAIN` at parse time; the budget is
  `DREAMER_ATTEMPT_BUDGET` per `DREAMER_ATTEMPT_BUDGET_WINDOW`
  (`classify.rs:53-54`). `count_dreamer_attempts` excludes `not_sent` rows and
  counts open, `cancelled`, and length-capped attempts alike. The
  `classify.rs` references were `:28` and `:40-41` in the prior revision and
  did not match HEAD; corrected here.
- Ledger fences: every transition is a row-predicate `UPDATE` on key, generation,
  and `in_progress` state (`dreamer_ledger.rs:565`, `guarded_transition`), so a
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
- Between `begin_dreamer_receipt` and the pool read, a kernel that is still
  opening (the memory store opens before the kernel does, `:3600-3624`) or is
  busy answers `kernel_unavailable` and leaves the same shape: an open receipt
  with no attempt, which the retry takes over once the kernel can answer. This
  window is not a recorded failure, so a restart does not burn the command id.
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
  as `missing` (`docs/host-wire-protocol.md`), and on the scripted producer's
  `status` being answerable in tests.

## What a test must construct

1. A classify run dropped while awaiting output, then a retry with `status`
   scripted `Missing`: assert one start, attempt and receipt terminal `unknown`,
   and a further retry replaying `unknown` with one start still.
2. The same with `status` `Active`: assert one start, receipt still
   `in_progress`, attempt still open.
3. A receipt forced `in_progress` with no attempt rows: assert the retry
   dispatches once at generation 2 under a generation-derived session and the
   predecessor's `finish_dreamer_attempt` is `Fenced`.
4. A blocked await under a short `timeout_ms`: assert the attempt ends
   `cancelled`, counts toward the budget, and the receipt completes `failed`.
5. A `timeout_ms` above the ceiling: assert every await the producer saw is at
   most `CLASSIFY_MAX_REQUEST_TIMEOUT`.
6. The durable attempt count filled to the budget: assert a new command answers
   `dreamer_budget_exhausted` with no receipt and no start, and a completed
   command still replays.
7. A kernel still `Starting` at the pool read: assert `kernel_unavailable`, the
   receipt `in_progress` with no attempt, no start; then the kernel opened and
   the same command retried: assert one start and one classification written
   (`dreamer_run_task_leaves_the_receipt_open_while_the_kernel_is_starting`).
8. Memories whose stored payloads pass `MAX_READ_ROW_BYTES` but whose summaries
   render inside the prompt bound: assert `payload_too_large`, not
   `invalid_params`, and no connect
   (`dreamer_run_task_refuses_a_pool_over_the_kernel_read_budget_as_too_large`).
9. Two runs over one memory under different command ids: assert one live
   classification per memory carrying the second run's values, the first run's
   rows invalidated at the second commit, and a replay of the second run
   writing nothing
   (`dreamer_run_task_retires_the_prior_classification_of_a_reclassified_memory`).

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

### Q: Does the budget check race with a concurrent request?

- Sources examined: `count_dreamer_attempts` and the check's placement before
  `begin_dreamer_receipt`.
- Findings: two requests can both read `spent = budget - 1` and both dispatch,
  so the bound can be exceeded by the number of concurrent requests. The bound
  is a host guard against runaway spend, not an exact quota.
- Conclusion: resolved with answer; recorded as a known slack.
