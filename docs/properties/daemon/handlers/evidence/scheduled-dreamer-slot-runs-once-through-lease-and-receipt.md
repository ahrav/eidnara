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

- `DreamerScheduler::tick` (`:186`) reads the clock once for due-ness, asks the
  host for its scheduled projects, and runs those due at or before that
  instant oldest first (`due_projects`, `:200`). After a run the project's
  next instant is recomputed from the tick instant (`advance`, `:233`), so
  slots missed while the daemon was down are not back-filled.
- `run_slot` (`:248`) leases before it runs: `acquire_dreamer_task` with
  acquisition id `slot_command_id(task, due_at_ms)`, the scheduler's instance
  and slot, the registration generation, the task id, and the due instant as
  the claim's `source_revision`. Every acquisition outcome other than `Claim`
  is a `Skipped` event with no run.
- The command id the run is dispatched under is derived from the returned
  claim's `source_revision` (`:297`), not from the slot that came due. The
  shared protocol rebinds a live claim held by the same instance and slot under
  an older registration generation (`crates/memory-store/src/task_lease.rs`,
  slot recovery in `acquire_task_lease`), and a rebound claim keeps its
  original `source_revision`, so a successor handed the predecessor's claim
  runs the predecessor's slot and `run_dreamer_task` finds the interrupted
  receipt under that command id.
- The registration generation is allocated on first use from
  `next_dreamer_scheduler_generation` (`crates/memory-store/src/lib.rs:3939`),
  one above the highest generation the instance has on the ledger. A live
  claim is never reclaimed, so the successor always outranks it; wall time is
  not used, so a clock step backwards cannot rank the successor below its
  predecessor.
- Lease and completion instants are read from the clock as each operation
  happens (`:272`, `:318`), so a long run for one project does not shorten the
  lease of the project behind it in the same tick.
- `SchedulerBridge::scheduled_projects` (`crates/daemon/src/lib.rs:14030`)
  reads the schedule from each bound route's configuration, the tier-merged
  value the route was bound under, and admits a project only when
  `authority_project_for_route`, `module_authority_for_project`, and
  `authority_status` agree its memories authority is `MODULE`. The schedule key
  is `TierClass::UserOnly` (`crates/daemon/src/config.rs:663-678`), so a
  project tier's value is dropped with a warning during the merge and never
  reaches a binding.
- `SchedulerBridge::run_task` (`lib.rs:14066`) refuses with `NotRunnable` when
  no live route is bound to the project or the task has no Rust-owned inputs
  (`DreamerRuntime::classify_inputs`, `lib.rs:3053`, `None` on this HEAD), and
  otherwise calls `run_dreamer_task` under `SCHEDULER_LEDGER_SESSION`
  (`lib.rs:14088`). The not-runnable reply is recorded on the lease so the slot
  is not retried every tick.

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
`Missing`.

## Investigation log

### Q: Should a successor sweep receipts left `in_progress` by a slot whose lease expired before the successor's first slot?

- Sources examined: `run_slot`, `acquire_task_lease` slot recovery,
  `collect_ledgers_tx`, `resume_dreamer_receipt`.
- Findings: the receipt's attempt row still records the dispatch, so nothing
  is double-billed; the receipt is simply never asked about again.
- Missing evidence: none; the behaviour is as implemented.
- Conclusion: needs human input
