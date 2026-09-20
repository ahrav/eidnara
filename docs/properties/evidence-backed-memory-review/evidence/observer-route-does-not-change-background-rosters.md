# observer-route-does-not-change-background-rosters

## Discovery trigger

Specification U3 acceptance: an observer route across explicit worker and
scheduler passes leaves a dormant project's Ready job unclaimed, a live
scheduled harness keeps its binding, and rosters are unchanged by observer
open and close. Owner decision OQ8 on #596: `harness == "cli"` selects
observation, filtered before newest-per-root in one shared participating view.

## Evidence trail

`crates/daemon/src/lib.rs` - `SessionBinding::is_observational`,
`OBSERVATIONAL_HARNESS`, `RouteBindings::participating`, `latest_for_root`,
`latest_per_root`, `bound_projects`; `SchedulerBridge::scheduled_projects` and
`binding_for_root`.

`crates/daemon/src/memory_reviewer/worker.rs` - `module_projects` reads
`latest_per_root`.

`crates/daemon/src/lib.rs` search lifecycle owner roster reads `bound_projects`.

Tests named in the catalog record.

## Failure scenario

The review command binds a route on a project whose sessions are all closed.
The worker's next pass sees the root, enrolls the project, and runs its Ready
jobs; or the scheduler takes the observer as the newest binding on the root
and reads its configuration for the schedule.

## Timing windows and dependencies

Any worker or scheduler pass while the observer is open; the pass after it
closes.

## What a test must construct

MODULE authority with the start-up binding closed; the four views snapshotted;
an observer bound and closed with the views compared to the snapshot; a live
scheduled `pi` binding, then an observer on the same root and on a second
bound root, with the views compared to the scheduled snapshot; both close
orders with each session still resolvable on its own route; a Ready job with
the gate open and two worker passes over an empty view.

## Investigation log

### Q: Is the worker's view the same one the scheduler and roster read?

- Sources examined: every caller of `latest_per_root`, `latest_for_root`,
  and `RouteBindings::values`.
- Findings: the three background consumers read `latest_per_root` or
  `latest_for_root` only. `values()` is read by bind and unbind for
  session-purge and note-capability decisions, which are route-local and must
  see observers.
- Missing evidence: none.
- Conclusion: resolved with answer.
