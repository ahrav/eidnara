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

Verified by reading the merged working tree. Symbol anchors refer to
`crates/daemon/src/dreamer_scheduler.rs` unless another path is given;
merge-time line offsets are not retained. Scheduling is explicit-config-only:
the user-tier schedule and `MODULE` authority admit lease work, but production
has no task input builder, so scheduled model dispatch is test-only.

- `DreamerScheduler::tick` reads the clock once for due-ness, asks the
  host for its scheduled projects, and runs those due at or before that
  instant oldest first (`due_projects`). After a run the project's
  next instant is recomputed from the tick instant (`advance`), so
  slots missed while the daemon was down are not back-filled. When the host
  returns `Err`, `tick` returns one `TickEvent::Deferred` before the
  due table is reconciled, so no project is dropped and no due instant moves.
- `run` logs every `Skipped` and `Deferred` event to stderr and, after
  a deferred tick, waits the idle poll instead of the distance to the earliest
  due instant, which would otherwise be zero and re-tick at once against a
  failing store.
- `run_slot` leases before it runs: `acquire_dreamer_task` with
  acquisition id `slot_command_id(task, due_at_ms)`, the scheduler's instance
  and slot, the registration generation, the task id, and the due instant as
  the claim's `source_revision`. Every acquisition outcome other than `Claim`
  is a `Skipped` event with no run.
- The command id the run is dispatched under is derived from the returned
  claim's `source_revision`, not from the slot that came due. The
  shared protocol rebinds a live claim held by the same instance and slot under
  an older registration generation (`crates/memory-store/src/task_lease.rs`,
  slot recovery in `acquire_task_lease`), and a rebound claim keeps its
  original `source_revision`, so a successor handed the predecessor's claim
  runs the predecessor's slot and `run_dreamer_task` finds the interrupted
  receipt under that command id.
- The registration generation is allocated on first use from
  `next_dreamer_scheduler_generation` (`crates/memory-store/src/lib.rs`),
  one above the highest retained generation the instance has on the ledger. A
  live claim is never reclaimed, so the successor always outranks it; wall time is
  not used, so a clock step backwards cannot rank the successor below its
  predecessor.
- Lease and completion instants are read from the clock as each operation
  happens in `run_slot`, so a long run for one project does not shorten the
  lease of the project behind it in the same tick.
- `SchedulerBridge::scheduled_projects` (`crates/daemon/src/lib.rs`)
  takes the most recently bound binding on each route root
  (`RouteBindings::latest_per_root`; `RouteBindings::insert` stamps each bind
  with a sequence), reads the schedule from that binding's
  configuration, the tier-merged value the route was bound under, and admits a
  root only when `memories_authority_for_route` answers
  `Module`. That helper is the same one `run_dreamer_task` uses, so a store
  `Err` is an `Err` from `scheduled_projects`, not a missing project. Roots
  with schedules that resolve to one authority project collapse to the most
  recently bound root, so the scheduler's due table sees one schedule per
  project. The schedule key is `TierClass::UserOnly`
  (`crates/daemon/src/config.rs`, `tier_class`), so a project tier's value is
  dropped with a warning during the merge and never reaches a binding.
- `SchedulerBridge::run_task` (`crates/daemon/src/lib.rs`) returns `NotRunnable` when
  no live route is bound to the project or the task has no Rust-owned inputs
  (`DreamerRuntime::classify_inputs`), and
  otherwise calls `run_dreamer_task` under `SCHEDULER_LEDGER_SESSION`
  with a `ClassifyRequest` containing kernel `object_ids`, a model chain, and
  a timeout, not caller-rendered prompts. `DreamerRuntime::new` sets its input
  builder to `None` and `install_task_inputs` is `#[cfg(test)]`, so production
  slots take the not-runnable path. That reply completes the lease without
  creating a receipt or starting a model. A lease completion error is logged,
  not reported as a successful durable completion.
- `binding_for_root` uses `RouteBindings::latest_for_root`, the same newest-bind
  rule as project discovery, but performs a separate lookup. With no intervening
  rebind it uses the binding that supplied the schedule. An intervening rebind
  may change the run's harness or configuration; receipt recovery still probes
  under the attempt's recorded root, harness, and child session.
- A slot identifies one receipt, not necessarily one model call. The original
  `run_dreamer_task` chain may try several models. The receipt property prevents
  recovery from starting another attempt after any possible dispatch, and
  permits takeover only for a generation with no attempt except `NotSent` rows.

## Failure scenario

A scheduler leases slot `t`, `run_dreamer_task` writes the `IN_PROGRESS`
receipt and dispatches a model, and the daemon dies. The successor's first slot
is `t + period`. Without the rebind rule the successor would open a second
receipt for `t + period` and dispatch again while the first run's outcome is
unknown: the interrupted logical slot is abandoned and fresh work is dispatched.
With it, the successor is handed the live claim for `t`, asks
`run_dreamer_task` for `t`, and the receipt's recovery path settles or resumes
the first run with no second dispatch, provided the receipt's digest and
authority binding still match. This model-dispatch scenario uses scripted task
inputs because production has no input builder.

## Timing windows and dependencies

- Recovery by rebind needs acquisition before the predecessor's lease expires,
  `DREAMER_TASK_LEASE_MS` (20 min, `crates/memory-store/src/lib.rs`)
  after acquisition, not after the nominal due instant. Past that the
  predecessor's claim is collected as `expired`, the successor leases a fresh
  claim for its own slot, and the predecessor's receipt stays `in_progress`
  until a request with its command id arrives, which no scheduler issues.
- Two schedulers on one store are refused by the store's own lease
  when they open it independently (`double_open_same_path_is_rejected_by_lease`
  in `crates/memory-store/src/lib.rs`, `mod tests`). This does not prove mutual
  exclusion for two scheduler objects sharing one already-open `MemoryStore`.
- `SchedulerBridge` rebuilds inputs before it enters the receipt protocol. A
  changed object set, model chain, timeout, or authority binding can therefore
  produce a conflict instead of resuming an interrupted receipt. The same slot
  id alone is not sufficient to guarantee settlement.

## What a test must construct

A real store with `MODULE` memories authority for the project, a host whose
projects and run replies are scripted, and a manual clock. For recovery: a
claim left live by a predecessor generation (acquired directly), then a
successor scheduler whose clock is behind the predecessor's, ticked at its own
first slot. For the receipt ordering: the scripted producer's `on_start` hook
reading the receipt from the store on entry to `start`. For the restart:
`dreamer_scheduled_run_writes_one_receipt_and_a_restart_adds_no_attempt` in
`crates/daemon/src/lib.rs` uses `tokio::select!` with
`wait_for_count(&producer.await_outputs, 1)` to observe entry into the output
wait, then drops the tick while the producer blocks. It does not infer that
state from 200 ms elapsed. A second `Handler` over the same store retries with
the producer answering `status` as
`Missing`. For the deferred tick: a scripted host whose next
`scheduled_projects` fails while a slot is due, and for the bridge,
`MemoryStore::fail_next_authority_route_read_for_test`, which makes one
`authority_project_for_route` return a backend error. For binding selection:
twelve routes bound on one root with distinct schedules and harnesses, so a
pick by map order almost never matches the newest bind, then a rebind of the
oldest channel and a newest binding with no schedule. For the per-project
collapse: a second route root bound to the same authority project through
`bind_authority_route` with a different schedule.

The three bridge tests named below use the async object-id harness through
`DreamerHarness::start(&producer).await` in `crates/daemon/src/lib.rs`. The main
thread's observed results are recorded in
[the catalog](../catalog.md#dreamer-dispatched-attempt-always-settles-through-the-receipt):
50 Dreamer tests passed, followed by 50 further runs of those 50 tests at
default concurrency with no retries; the broader nextest run passed 2,325
tests and skipped 5. A single-context static review found no actionable
findings. The checks' adequacy status remains `unaudited`. The slot oracle must
distinguish
`NotRunnable` from receipt-backed work and allow the original chain's fallback
attempts while rejecting a second start for any recorded attempt identity.

## Investigation log

### Q: Should a successor sweep receipts left `in_progress` by a slot whose lease expired before the successor's first slot?

- Sources examined: `run_slot`, `acquire_task_lease` slot recovery,
  `collect_ledgers_tx`, `resume_dreamer_receipt`.
- Findings: the attempt row still protects the old receipt from redispatch if
  it is retried, but the scheduler asks for a new slot. It does not settle the
  old receipt. This is not a guarantee against model work for a later slot.
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
  root and the newest scheduled root per project. `binding_for_root` follows
  the same newest-bind rule, but the two lookups are not one snapshot.
- Missing evidence: none.
- Conclusion: resolved; covered by
  `dreamer_scheduler_bridge_follows_the_most_recent_binding_on_a_root` and
  `dreamer_scheduler_bridge_reports_a_project_once_across_its_roots`.
