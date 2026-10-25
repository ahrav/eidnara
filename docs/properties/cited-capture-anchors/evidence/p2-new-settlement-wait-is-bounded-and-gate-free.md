# p2-new-settlement-wait-is-bounded-and-gate-free

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), obligation for constraint C7, mechanism D4 and OQ2-2. Activated by the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

`into_job` (`crates/daemon/src/source_capture.rs:697`) takes the publication slot, reads one `tokio::time::Instant` as `accepted_at`, seeds the job's `EvalBudget` with `accepted_at.into_std() + CAPTURE_BUDGET` (`:41`, 5 s), and records `Settlement::Running { accepted_at }` for the job's `SessionIdentity` under the supervisor's std mutex. The wait deadline and the job budget therefore share one origin.

The submit handler (`crates/daemon/src/memory_capture/native.rs:394`) calls `capture_settlement` (`:122`) after the lease and capture-enabled checks and before `kernel_routes::blocking`, the hop whose closure calls `CaptureWork::publish`, which takes the capture commit gate (`crates/daemon/src/memory_capture.rs:983`). `capture_settlement` runs `tokio::select! { biased; cancel; await_settlement }`. `SourceCapture::await_settlement` (`crates/daemon/src/source_capture.rs:579`) subscribes to the entry's `watch` channel under the mutex and releases the mutex before awaiting. `settle` (`crates/daemon/src/source_capture/settlement.rs:151`) wraps `wait_for(not Running)` in `timeout_at(accepted_at + CAPTURE_BUDGET)`, so the wait returns by the deadline whatever the job does. The `watch` version check means a transition between the subscribe and the await is observed.

No lock is held across any await of the wait: the settlement mutex guard is a temporary of the subscribe call, and the commit gate is taken only on the blocking pool after the wait returns.

## Failure scenario

A wait inside the gate critical section deadlocks the source job, which needs the gate to commit; a wait without a deadline lets a stalled job hold every submit of its session.

## Timing windows and dependencies

A source job stalled past its budget; a submit that arrives while the job runs.

## What a test must construct

- `a_running_entry_waits_for_its_end_or_its_budget_deadline` (`crates/daemon/src/source_capture/settlement.rs:228`): under `start_paused`, a waiter on a `Running` entry is pending at `accepted_at + 5 s - 1 ms` and returns `BudgetExpired` at exactly `accepted_at + 5 s`; a waiter whose job ends after 1 s returns `Settled` at 1 s.
- `a_publish_past_its_budget_releases_the_submit_at_the_deadline` (`crates/daemon/src/source_capture_tests.rs:4029`): the publish hook parks a real source job; the submit returns between 5 s and 7 s after the entry's `accepted_at`, and the parked job later ends as `Deadline` on its own budget.
- `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites` (`crates/daemon/src/source_capture_tests.rs:3961`): while the submit is parked in the wait, the test acquires `capture_commit_gate` with `try_lock`, which proves the submit holds no gate at its await point.

## Investigation log

### Q: Does the wait return by the deadline under a paused clock?

- Sources examined: `settle`, the paused-clock unit test.
- Findings: yes; the deadline is `accepted_at + CAPTURE_BUDGET` on the tokio clock, and the test advances exactly to it.
- Conclusion: resolved.

### Q: Is the gate free while a waiter is parked?

- Sources examined: `capture_settlement`, `CaptureWork::publish`, the gate assertion in `crates/daemon/src/source_capture_tests.rs:3961`.
- Findings: the gate is taken only inside the blocking hop, which starts after the wait; the test takes the gate while the submit waits.
- Conclusion: resolved.
