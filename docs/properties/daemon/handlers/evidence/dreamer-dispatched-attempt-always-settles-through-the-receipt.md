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

Verified by reading the merged working tree. Symbol anchors below refer to
`crates/daemon/src/lib.rs` unless another path is given; merge-time line offsets
are not retained. `dispatch_value_with_inbound_bytes` reaches
`handle_dreamer_run_task` without a feature or configuration gate. This is
default-production reachability, subject to the route's authority checks.

- `handle_dreamer_run_task` validates the request shape (`ClassifyRequest::parse`:
  kernel object ids, a model chain, and a timeout; a
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
- The digest in `run_dreamer_task` includes the sorted `object_ids`, model
  chain, requested timeout, await ceiling, output-token limit, temperature,
  template and schema versions, and system-prompt hash. It does not include
  the canonical memory bodies or a recovery timeout. A completed receipt with
  matching inputs and binding replays its outcome without reading changed
  memory text.
- `begin_dreamer_receipt` is the first write. The receipt binding has no
  harness field. Each `DreamerAttemptSpec` records `project_root`, `harness`,
  and `child_session`, so recovery uses the dispatch identity even when the
  retry arrives under another harness. These fields are defined in
  `crates/memory-store/src/dreamer_ledger.rs` and the `dreamer_attempts` table
  in `crates/memory-store/baseline.sql`. `Complete` replays; `DigestConflict`
  and `BindingMismatch` answer `dreamer_request_conflict`; `InProgress` goes
  to `resume_dreamer_receipt`, all before a new producer is constructed.
- Only a fresh or successfully taken-over generation calls `render_pool`.
  It reads the named memories at the kernel tip, checks visibility, memory
  domain membership, and the serving view's folded sensitivity, and renders
  escaped bodies through
  `render_classify_prompt` in `crates/daemon/src/classify.rs`. `PoolFailure`
  separates recorded request failures (`invalid_params`, `sensitive_remote`,
  `payload_too_large`, or `kernel_read_failed`) from `kernel_unavailable`, which
  leaves an open receipt with no attempt. A memory served above normal is
  `sensitive_remote`: the surface serves it, but the prompt goes to a model
  provider. A truncated kernel read is `payload_too_large`,
  not a missing-object error. A retry may take over that undispatched receipt
  and read again; replay and recovery of a possible dispatch do not read it.
- `resume_dreamer_receipt` selects the newest current-generation attempt that
  is not `NotSent`. Without one, `take_over_undispatched_dreamer_receipt` in
  `crates/memory-store/src/dreamer_ledger.rs` atomically checks
  `NOT EXISTS (... terminal_kind IS NULL OR terminal_kind != 'not_sent')`
  before advancing to `g + 1`. No row or only `NotSent` rows permits takeover;
  an open or ended possible dispatch blocks it, including one inserted after
  the list read. Older-generation rows do not block it. `Applied`, `Fenced`,
  and store errors are distinct outcomes.
- An ended marker completes the receipt `unknown` through
  `complete_receipt_as_unknown`, preserving the attempt's terminal. An open
  marker without a handle also settles `unknown`. With a handle, recovery
  connects under the recorded root and harness, binds the recorded child
  session, and asks `status`. `Missing` and `Terminal` settle `unknown`;
  `Active`, connection errors, bind errors, and status errors answer
  `dreamer_outcome_unknown` without writing. Open-attempt settlement uses
  `settle_dispatched_attempt_as_unknown`: attempt finish is best-effort, but
  receipt completion checks `Applied`, `Fenced`, and `Err` separately.
- In `run_dreamer_task`, `begin_dreamer_attempt` precedes the start call and
  `record_dreamer_run_handle` follows a successful start. A fenced handle
  write skips purge; a store error still attempts purge. Both try unknown
  settlement, which cannot overwrite a successor's receipt. The attempt
  terminal write follows the same no-purge rule when fenced. On a store error,
  a usable result is first parsed, committed through `record_classifications`,
  and offered to receipt completion; otherwise the request tries unknown
  settlement. A fence excludes that known-result fallback's kernel write.
- The await uses `classify_attempt_timeout(CLASSIFY_AWAIT_TIMEOUT, deadline)`.
  The request deadline is clamped to that ceiling, so a timed-out await ends
  the attempt `cancelled` without a second read of the run. The normal model
  chain may try another model after a failed or rejected result; this property
  forbids redispatch during recovery, not fallback within the original chain.
- `parse_output` rejects length-capped output before `parse_classify_output`
  in `crates/daemon/src/classify.rs` accepts one envelope with exact coverage
  and closed importance, scope, and shareable domains. Parser errors name the
  rule, not the raw output. `record_classifications` writes one
  `memory_classification` observation per memory, scoped to the project and
  dependent on that memory, with `(ModelInference, DreamerInference)` admission.
  The model's `shareable` is floored by `Envelope::served_rows_for` in the same
  commit: a memory served above normal is recorded `shareable=false`.
  Its kernel receipt uses producer `dreamer.classify`, a project-namespaced
  receipt operation key, and the receipt digest. A replay writes nothing new.
  The same commit retires this project's prior live classifications through
  `Envelope::live_dependent_observations` in `crates/kernel/src/envelope.rs`
  and `retire_observation` in `crates/kernel/src/slice/write.rs`; a row that
  `scoped_object_state` places outside the project is skipped, since the
  `classifies` kind is not projected and any project may cite the memory.
- Normal kernel-write failure is recorded as `dreamer_kernel_write_failed`;
  kernel conflicts retain the `kernel.commit` reason classification. Both
  that completion and the exhausted-chain and success completions distinguish
  `Applied`, `Fenced`, and `Err`. `read_dream_task_response` reads the durable
  reply after `Applied`; read errors, a missing terminal receipt, or malformed
  JSON fail closed.
  `classify_success_response` carries the commit sequence and count, never
  model text. The kernel commit and receipt completion are separate writes.
- `attempt_child_session_id` in `crates/daemon/src/classify.rs` hashes the
  generation with the project, ledger session, command, attempt index, and
  model to separate successor sessions. That module defines the 600-second
  request/await ceiling, the eight-model chain cap, and the budget of 200
  attempts per project per 24 hours. `count_dreamer_attempts` in
  `crates/memory-store/src/dreamer_ledger.rs` excludes only `NotSent` rows.
- Takeover, attempt insertion, handle recording, attempt finish, and receipt
  completion predicate writes on receipt key, generation, and `in_progress`
  state in `crates/memory-store/src/dreamer_ledger.rs`. Predecessor writes
  after takeover are `Fenced`, as exercised in
  `crates/memory-store/tests/dreamer_ledger.rs` by
  `a_higher_generation_takes_over_and_the_predecessor_is_fenced_on_every_transition`.

## Failure scenario

A daemon dispatches a classify run and is killed while awaiting the model. On
restart the client retries the same `command_id`. Without this property the
route would either replay a success it never recorded or start the chain again
and bill the model twice. With it, the retry finds the open attempt's run handle,
asks the runtime, and settles the command `unknown` when the runtime reports
the run missing or terminal; every matching retry replays that `unknown` and
the producer is never started again.

## Timing windows and dependencies

- Between `begin_dreamer_receipt` and `begin_dreamer_attempt` a crash leaves a
  receipt with no marker; an admitted successor takes over and may dispatch.
- Between `begin_dreamer_receipt` and the pool read, a kernel that is still
  opening or is busy answers `kernel_unavailable` and leaves an open receipt
  with no attempt, which the retry takes over once the kernel can answer. This
  window is not a recorded failure, so a restart does not burn the command id.
- Between the resume list read and takeover, a predecessor may insert an attempt.
  The atomic predicate sees that possible dispatch and fences the takeover;
  if takeover wins first, the predecessor's attempt insert is fenced
  (`take_over_undispatched_dreamer_receipt` and `begin_dreamer_attempt` in
  `crates/memory-store/src/dreamer_ledger.rs`). A generation
  containing only `NotSent` attempts remains eligible.
- Between `start` returning and `record_dreamer_run_handle` a crash leaves an
  open attempt with no handle; the successor settles `unknown` because the
  dispatch cannot be disproved.
- Between `record_dreamer_run_handle` and `finish_dreamer_attempt` a crash
  leaves an open attempt with a handle; the successor resolves it.
- A generation change during `start` or `await_output` fences the returning
  predecessor's write. It skips purge and cannot complete the generation-2
  receipt (`run_dreamer_task`).
- Between `finish_dreamer_attempt` and `complete_dreamer_receipt` a crash leaves
  an ended attempt under an open receipt; the successor settles `unknown` rather
  than replaying a result it does not hold.
- Between `record_classifications` and receipt completion, a crash can leave
  canonical classifications committed but the receipt open. Recovery settles
  `unknown` without another dispatch or kernel write; `unknown` does not mean
  that no canonical effect occurred.
- The budget is checked before the chain, not reserved per attempt. Concurrent
  requests and fallback attempts in an admitted chain can exceed it; it is a
  spend guard, not an exact quota.
- The property depends on the runtime reporting a restarted host's old run ids
  as `missing` (`docs/host-wire-protocol.md`, Section 7.2, `run.status`), on a
  terminal run status never returning to active within one runtime incarnation,
  and on the scripted producer's `status` being answerable in tests.

## What a test must construct

1. A classify run dropped while awaiting output, then a retry with `status`
   scripted `Missing`: assert one start, attempt and receipt terminal `unknown`,
   and a further retry replaying `unknown` with one start still.
   `DreamerHarness::crash_after_dispatch` in `crates/daemon/src/lib.rs` uses
   `tokio::select!` with `wait_for_count(&producer.await_outputs, 1)` to observe
   entry into the output wait before dropping the request. It does not infer
   that state from 200 ms elapsed.
2. The same with `status` `Active`: assert one start, receipt still
   `in_progress`, attempt still open.
3. The same with `status` `Terminal`: assert one start, attempt and receipt
   terminal `unknown`, and a further retry replaying `unknown` with no second
   `status` call.
4. A receipt stranded `in_progress` by an aborted attempt insert: assert the
   retry dispatches once at generation 2. Also reopen a receipt
   with a `NotSent` attempt: assert one successor dispatch under a
   generation-derived session and a fenced predecessor `finish_dreamer_attempt`
   in `dreamer_run_task_takes_over_an_undispatched_receipt_and_dispatches_once`.
5. A timed-out await with a late answer queued behind it: assert the attempt
   ends `cancelled`, no second read of the run happens, the attempt counts
   toward the budget, and the receipt completes `failed`.
6. A `timeout_ms` above the ceiling: assert every await the producer saw is at
   most `CLASSIFY_MAX_REQUEST_TIMEOUT`.
7. The durable attempt count filled to the budget: assert a new command answers
   `dreamer_budget_exhausted` with no receipt and no start, and a completed
   command still replays.
8. A kernel still `Starting` at the pool read: assert `kernel_unavailable`, the
   receipt `in_progress` with no attempt, no start; then the kernel opened and
   the same command retried: assert one start and one classification written
   (`dreamer_run_task_leaves_the_receipt_open_while_the_kernel_is_starting`).
9. Memories whose stored payloads exceed `MAX_READ_ROW_BYTES` but whose summaries
   render inside the prompt bound: assert `payload_too_large`, not
   `invalid_params`, and no connect
   (`dreamer_run_task_refuses_a_pool_over_the_kernel_read_budget_as_too_large`).
10. Two runs over one memory under different command ids: assert one live
   classification per memory carrying the second run's values, the first run's
   rows invalidated at the second commit, and a replay of the second run
   writing nothing
   (`dreamer_run_task_retires_the_prior_classification_of_a_reclassified_memory`).
11. No current-generation attempt rows, then an open row, then a `NotSent` row:
   assert atomic takeover permits the first and third states and rejects the
   second. Older-generation rows do not block takeover; a failed
   current-generation attempt does
   (`crates/memory-store/tests/dreamer_ledger.rs`,
   `an_undispatched_receipt_can_be_taken_over_only_without_a_possible_dispatch`).
12. Move the receipt to generation 2 in each of the scripted producer's
   `on_start` and `on_await_output` hooks. Assert `dreamer_ledger_fenced`, one
   start, no purge, the generation-2 receipt still `in_progress`, and the
   generation-1 attempt still open. The start window performs no await; the
   await window performs exactly one
    (`dreamer_run_task_does_not_purge_the_child_session_once_it_is_fenced`).
13. A memory admitted `Sensitive` in the bound project, shown served at
   `ExplicitSearch`: assert `sensitive_remote`, no start, no classification,
   the receipt completed `failed`, and a retry replaying the refusal with no
   start
   (`dreamer_run_task_refuses_a_memory_served_above_normal_sensitivity`).
14. `record_classifications` called directly with `shareable=true` for a
   `Sensitive` memory and a normal one: assert the sensitive row is recorded
   `shareable=false` and the normal row `shareable=true`
   (`record_classifications_never_records_a_sensitive_memory_as_shareable`).
15. Another project's `memory_classification` observation citing the memory
   through `classifies`, then a classify of that memory: assert `classified`
   is 1, the foreign row still live, and this project's row written
   (`dreamer_run_task_leaves_another_projects_classification_of_the_memory_live`).

The stranded-receipt, fenced-cleanup, and terminal-status tests use the async
object-id harness:
`classify_payload`, `classify_manifest` where output is needed, and
`DreamerHarness::start(&producer).await` in `crates/daemon/src/lib.rs`.
The main thread's observed results are recorded in
[the catalog](../catalog.md#dreamer-dispatched-attempt-always-settles-through-the-receipt):
50 Dreamer tests passed, followed by 50 further runs of those 50 tests at
default concurrency with no retries; the broader nextest run passed 2,325
tests and skipped 5. A single-context static review found no actionable
findings. Test adequacy remains `unaudited`. The cleanup test
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
   (`crates/daemon/src/historian.rs`, `reattach_historian_producer`).
- Findings: the reply to replay is not durable until `complete_dreamer_receipt`,
  even if `record_classifications` already committed its canonical effects.
  The crashed daemon may also have purged the child session. Recovery therefore
   does not depend on re-reading the answer. Dispatching again would bill twice.
   The historian's missing-run branch returns `RefireEligible`; Dreamer instead
   settles unknown. The code establishes this difference, not a general claim
   that repeating summarization has no cost or side effects.
- Conclusion: resolved with answer; the asymmetry is deliberate.

### Q: Why does a `Terminal` status settle rather than wait for a later retry?

- Sources examined: the Broca supervisor's run status transitions and terminal
  retention (`crates/host-runtime/src/broca/supervisor.rs`, `status`,
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
  the await call in `run_dreamer_task`.
- Findings: the request deadline is clamped to `CLASSIFY_AWAIT_TIMEOUT`, and the
  await runs under `min(CLASSIFY_AWAIT_TIMEOUT, deadline - now)`, which is the
  remaining request budget. An await that times out has therefore reached the
  deadline, and any further read would run under a zero window. The attempt
  ends `cancelled` and cleanup attempts to purge the session unless its terminal
   write is fenced.
- Conclusion: resolved with answer; the deadline is the single bound.

### Q: Does the budget check race with a concurrent request?

- Sources examined: `count_dreamer_attempts` and the check's placement before
  `begin_dreamer_receipt`.
- Findings: two requests can both read `spent = budget - 1` and both dispatch.
  Each admitted chain can also try several models without checking the budget
  again. The bound is a host guard against runaway spend, not an exact quota.
- Conclusion: resolved with answer; recorded as a known slack.
