# scheduled-dreamer-slot-runs-once-through-lease-and-receipt

## Discovery trigger

Issue 316 moves Dreamer scheduling into the daemon: cron schedules are read
from trusted configuration, due tasks are leased through the shared task-lease
ledger, and every run starts through the durable receipt protocol. The record
`dreamer-dispatched-attempt-always-settles-through-the-receipt` covers what a
run does once it is asked for; this record covers who asks, how often, and
under which identity, so that a scheduler restart cannot turn one slot into two
billable runs.

## Evidence trail

Verified at HEAD. References are to `crates/daemon/src/dreamer_scheduler.rs`
unless stated.

- `DreamerScheduler::tick` (`:244`) reads the clock once for due-ness, asks the
  host for its scheduled projects, and runs those due at or before that
  instant oldest first (`due_projects`, `:265`). After a run the project's
  next instant is recomputed from the clock as the run returns, floored at
  the slot that ran (`advance`, `:298`, called at `:256`), so slots
  missed while the daemon was down are not back-filled, a slot that came due
  while the run itself was in progress is not back-filled either, and a wall
  clock that steps back during a run cannot select a slot earlier than the one
  that ran: the next instant is always strictly after `due_at_ms`. A project
  not in this tick's due set whose instant elapses during another project's
  run keeps that instant and runs it once on the next tick, late; the post-run
  advance then bounds it to one run per elapsed period. When
  the host returns `Err`, `tick` returns one `TickEvent::Deferred` (`:248`)
  before the due table is reconciled, so no project is dropped and no due
  instant moves.
- `run` (`:163`) logs every `Skipped`, `Retained`, and `Deferred` event to
  stderr and, after a deferred tick or a retained slot, waits the idle poll
  instead of the distance to the earliest due instant, which would otherwise be
  zero and re-tick at once against a failing store. The tick is awaited under
  `select!` with the cancellation token (`:172`), so cancellation during
  a run drops the tick where it stands: the run in flight is abandoned to the
  receipt protocol, which recovers it on restart, and the projects still due
  behind it are not leased.
- `run_slot` (`:313`) leases before it runs:
  `acquire_dreamer_task` with acquisition id `slot_command_id(task, due_at_ms)`,
  the scheduler's instance and slot, the registration generation, the task id,
  and the due instant as the claim's `source_revision`. Every acquisition
  decision other than `Claim` is a `Skipped` event with no run, and `tick`
  advances the project past the slot. A store `Err` from the generation lookup
  or from the acquisition is not a decision: `run_slot` returns
  `TickEvent::Retained` (`:325`), `tick` leaves that project's
  due instant in place (`:254`), and `run` waits the idle poll
  before the next tick retries the same slot under the same acquisition id.
  The same holds after the lease: a `TaskRunOutcome::StoreUnavailable` from
  the host (`:377`) and a store `Err` from `complete_dreamer_task`
  (`:402`) both retain the slot with its claim live. The next
  tick's acquisition under the same id rebinds and renews that claim
  (`acquire_task_lease` replays a live claim by acquisition id), the host is
  asked for the same command id, and the receipt protocol replays a settled
  receipt or resumes an open one without a second dispatch, so the retry is
  safe whether or not the model ran. Every acquisition decision, every other
  host reply, and a completion `Conflict` consume the slot.
- The command id the run is dispatched under is derived from the returned
  claim's `source_revision` (`:367`), not from the slot that came due. The
  shared protocol rebinds a live claim held by the same instance and slot under
  an older registration generation (`crates/memory-store/src/task_lease.rs`,
  slot recovery in `acquire_task_lease`), and a rebound claim keeps its
  original `source_revision`, so a successor handed the predecessor's claim
  runs the predecessor's slot and `run_dreamer_task` finds the interrupted
  receipt under that command id.
- The registration generation is allocated on first use from
  `next_dreamer_scheduler_generation` (`crates/memory-store/src/lib.rs:3975`),
  one above the highest generation the instance has on the ledger. A live
  claim is never reclaimed, so the successor always outranks it; wall time is
  not used, so a clock step backwards cannot rank the successor below its
  predecessor.
- Lease and completion instants are read from the clock as each operation
  happens (`:342`, `:393`), so a long run for one project does not shorten the
  lease of the project behind it in the same tick.
- `SchedulerBridge::scheduled_projects` (`crates/daemon/src/lib.rs:13789`)
  takes the most recently bound binding on each route root
  (`RouteBindings::latest_per_root`, `lib.rs:277`; `RouteBindings::insert`
  stamps each bind with a sequence at `lib.rs:244-245`), with or without a
  schedule, and admits a root only
  when `memories_authority_for_route` (`lib.rs:13726`) answers
  `Module`. That helper is the same one `run_dreamer_task` uses, so a store
  `Err` is an `Err` from `scheduled_projects`, not a missing project. Roots
  that resolve to one authority project collapse to the most recently bound
  root; only then is the schedule read from that root's binding, the
  tier-merged value the route was bound under, so a newest root whose binding
  has no schedule unschedules the project even while older roots still carry
  one, and the scheduler's due table, keyed by project, sees one schedule per
  project. The schedule key is `TierClass::UserOnly`
  (`crates/daemon/src/config.rs:663-678`), so a project tier's value is
  dropped with a warning during the merge and never reaches a binding.
- `SchedulerBridge::run_task` (`lib.rs:13840`) refuses with `NotRunnable` when
  no live route is bound to the project or the task has no Rust-owned inputs
  (`DreamerRuntime::classify_inputs`, `lib.rs:3101`, `None` on this HEAD), and
  otherwise calls `run_dreamer_task` under `SCHEDULER_LEDGER_SESSION`
  (`lib.rs:13858`) with `leased_project` set to the project the lease
  is on. `binding_for_root` (`lib.rs:13771`) reads the newest binding on the
  scheduled root at dispatch, the same choice `scheduled_projects` made at the
  tick; a rebind of that root in between dispatches under the binding the user
  now presents, and a rebind during the run cannot be observed at all, so the
  binding is not re-validated against the tick's snapshot. The not-runnable
  reply is recorded on the lease so the slot is not retried every tick.
  `run_task` maps the protocol's two store-error codes,
  `authority_lookup_failed` and `dreamer_ledger_failed`, to
  `TaskRunOutcome::StoreUnavailable` (`lib.rs:13883`); every other
  reply, success or error, is the protocol's answer for the command and is
  recorded on the lease.
- `run_dreamer_task` resolves the route again at its authority gate
  (`lib.rs:9540`). The generation check alone does not pin the
  project: a root rebound to another `MODULE` project at an equal generation
  passes it. With `leased_project` set, a route that now resolves to another
  project is refused as `authority_project_mismatch`
  (`lib.rs:9570`) before the receipt is written, so no run is
  receipted under a project the lease does not name and the scheduler records
  the refusal on the lease like any other reply.

## Failure scenario

A scheduler leases slot `t`, `run_dreamer_task` writes the `IN_PROGRESS`
receipt and dispatches a model, and the daemon dies. The successor's first slot
is `t + period`. Without the rebind rule the successor would open a second
receipt for `t + period` and dispatch again while the first run's outcome is
unknown: two billable calls for one logical slot, and the first receipt left
open for good. With it, the successor is handed the live claim for `t`, asks
`run_dreamer_task` for `t`, and the receipt's recovery path settles or resumes
the first run with no second dispatch.

## Timing windows and dependencies

- A slot whose lease acquisition failed with a store error is retried at the
  next tick, after the idle poll, under the same acquisition id; the ledger's
  replay rules decide whether that retry leases, replays, or reports a decision
  another tick recorded.
- Recovery by rebind needs the successor's first slot to come due within
  `DREAMER_TASK_LEASE_MS` (20 min) of the interrupted slot. Past that the
  predecessor's claim is collected as `expired`, the successor leases a fresh
  claim for its own slot, and the predecessor's receipt stays `in_progress`
  until a request with its command id arrives, which no scheduler issues.
- Two schedulers on one store are refused by the store's own lease
  (`double_open_same_path_is_rejected_by_lease`), so the same instance and slot
  belong to at most one live daemon.

## What a test must construct

A real store with `MODULE` memories authority for the project, a host whose
projects and run replies are scripted, and a manual clock. For recovery: a
claim left live by a predecessor generation (acquired directly), then a
successor scheduler whose clock is behind the predecessor's, ticked at its own
first slot. For the receipt ordering: the scripted producer's `on_start` hook
reading the receipt from the store on entry to `start`. For the restart: the
tick future dropped while the producer blocks on output, then a second
`Handler` over the same store with the producer answering `status` as
`Missing`. For the deferred tick: a scripted host whose next
`scheduled_projects` fails while a slot is due, and for the bridge,
`MemoryStore::fail_next_authority_route_read_for_test`, which makes one
`authority_project_for_route` return a backend error. For the retained slot:
`MemoryStore::fail_next_dreamer_task_acquire_for_test`, which makes one
`acquire_dreamer_task` return a backend error before it touches the ledger;
`fail_next_dreamer_task_complete_for_test` does the same for
`complete_dreamer_task`, and `fail_next_authority_route_read_for_test` armed
before `run_task` makes the protocol's own authority gate fail. For the backward
clock step: a scripted host that steps the manual clock back an hour during the
run. For the refused run: a second `MODULE` authority activated on the leased root
through `activate_module_authority`, so the root resolves to the other project
at an equal generation. For cancellation mid-tick: two due projects on a host
whose runs park on a `Notify`, cancelled while the first is parked. For
binding selection:
twelve routes bound on one root with distinct schedules and harnesses, so a
pick by map order almost never matches the newest bind, then a rebind of the
oldest channel and a newest binding with no schedule. For the post-run
advance: a scripted host that moves the manual clock twelve minutes during a
run on a five-minute schedule. For the per-project
collapse: a second route root bound to the same authority project through
`bind_authority_route` with a different schedule, then a third with none.

## Investigation log

### Q: Should a successor sweep receipts left `in_progress` by a slot whose lease expired before the successor's first slot?

- Sources examined: `run_slot`, `acquire_task_lease` slot recovery,
  `collect_ledgers_tx`, `resume_dreamer_receipt`.
- Findings: the receipt's attempt row still records the dispatch, so nothing
  is double-billed; the receipt is simply never asked about again.
- Missing evidence: none; the behaviour is as implemented.
- Conclusion: needs human input

### Q: What happens to a project's pending slot when the store fails during a tick?

- Sources examined: `SchedulerBridge::scheduled_projects`,
  `DreamerScheduler::tick`, `due_projects`, `run`, and the wire route's
  authority gate in `run_dreamer_task`.
- Findings: an earlier shape of `scheduled_projects` mapped every store `Err`
  through `.ok().flatten()`, so a failed read looked like a project with no
  schedule; `due_projects` then evicted the project's due entry through
  `retain`, and the next successful tick recomputed its next instant from
  `now`, losing the slot with no event or log. The wire route distinguished the
  same failure as `authority_lookup_failed`. Both paths now share
  `memories_authority_for_route`; the bridge propagates `Err`, `tick` defers
  without touching the due table, and `run` logs the deferral and waits the
  idle poll.
- Missing evidence: none.
- Conclusion: resolved; covered by
  `a_failed_project_lookup_defers_the_tick_and_keeps_the_pending_slot`,
  `run_waits_the_idle_poll_after_a_deferred_tick`, and
  `dreamer_scheduler_bridge_reports_a_store_failure_instead_of_no_projects`.

### Q: Which binding speaks for a root, and which root for a project, when they disagree?

- Sources examined: `RouteBindings`, `SchedulerBridge::scheduled_projects`,
  `SchedulerBridge::binding_for_root`, `Handler::bind`, `due_projects`,
  `authority_route_bindings` in `crates/memory-store/baseline.sql`.
- Findings: bindings freeze configuration at bind, so sessions bound before
  and after a user edits the cron disagree. An earlier shape took the first
  binding `HashMap::values()` yielded, so the schedule and harness could flip
  between ticks and each flip reset the project's next instant; and because
  `route_project_root` is the primary key of `authority_route_bindings` with
  `project` non-unique, two roots on one project with different schedules reset
  each other every tick and the project never came due. `RouteBindings` now
  stamps each bind with a sequence; the bridge takes the newest binding per
  root and the newest root per project, and `binding_for_root` follows the same
  choice.
- Missing evidence: none.
- Conclusion: resolved; covered by
  `dreamer_scheduler_bridge_follows_the_most_recent_binding_on_a_root` and
  `dreamer_scheduler_bridge_reports_a_project_once_across_its_roots`.

### Q: Does a newest root without a schedule unschedule a project whose older roots still have one?

- Sources examined: `SchedulerBridge::scheduled_projects`,
  `RouteBindings::latest_per_root`, the per-root and per-project tests.
- Findings: an earlier shape dropped every root without a schedule before the
  per-project collapse, so an older root's frozen schedule won and the project
  stayed scheduled after the user removed the schedule and bound a new
  session. The collapse now runs over every root's newest binding and the
  winner's schedule decides.
- Missing evidence: none.
- Conclusion: resolved; covered by the third root in
  `dreamer_scheduler_bridge_reports_a_project_once_across_its_roots`.

### Q: Is a slot lost when the lease ledger fails transiently?

- Sources examined: `run_slot`, `tick`, `run`, `acquire_task_lease`.
- Findings: an earlier shape returned `Skipped` for a store `Err` and `tick`
  advanced the project unconditionally, so a momentary store failure consumed
  the slot without a lease or a run. Store errors before the lease is held now
  return `Retained`, the project is not advanced, and the loop waits the idle
  poll.
- Missing evidence: none.
- Conclusion: resolved; covered by
  `a_failed_lease_acquisition_retains_the_slot_for_the_next_tick` and
  `run_waits_the_idle_poll_after_a_deferred_tick_or_a_retained_slot`.

### Q: Can a run execute under a project other than the one the lease names?

- Sources examined: `SchedulerBridge::run_task`, `run_dreamer_task`'s
  authority gate, `bind_authority_route`, `authority_route_bindings`.
- Findings: `bind_authority_route` upserts the route's project, so a root can
  move to another project between `scheduled_projects` and `run_task`. The gate
  re-resolved the route and compared generations only, so another project at an
  equal generation accepted inputs built for the leased project and receipted
  the run under itself. `DreamerRunRequest::leased_project` now names the
  leased project and the gate refuses a mismatch before any receipt is written.
- Missing evidence: none.
- Conclusion: resolved; covered by
  `dreamer_scheduler_bridge_refuses_a_root_that_moved_to_another_project`.
