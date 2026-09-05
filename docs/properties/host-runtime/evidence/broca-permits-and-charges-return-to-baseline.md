# broca-permits-and-charges-return-to-baseline

## Discovery trigger

`SupervisorMetrics` exists for exactly this record. Its doc comment at
`supervisor.rs:217` says tests use the snapshot "to prove every permit and
byte charge returns exactly to baseline across all failure paths". The
supervisor holds four semaphores and one byte budget (`:206-212`), and a
permit leaked on any terminal path shrinks the admission pool until the host
restarts. The audit question was which paths release which resources, and
whether any path can release the same resource twice or not at all.

## Evidence trail

All references are at `e16e39e`.

The resources and where they are acquired:

- Command permit: `command_permit` at `supervisor.rs:311-315`, held as an RAII
  local in `send` (`:330`), `status` (`:421`), `cancel` (`:440`), and `delete`
  (`:472`). It drops on every return path of those functions.
- Run permit: `try_acquire_owned` at `:338`, stored in `RunState.run_permit`
  at `:401`. Released by `finish` at `:986-988` on any terminal, or by
  `remove_session` at `:1074-1076` when a live run is purged.
- Base charge: `try_charge` at `:339-345`, stored at `:402`. It is split three
  ways over the run's life: the request-byte excess moves to the task-local
  `_request_charge` at `:733-738` once the backend starts; `finish` releases
  the excess when `work_done` is already set (`:992-996`); `DoneGuard::drop`
  releases it when the terminal was appended first (`:795-809`). `delete`
  splits a tombstone charge out of it at `:514` and releases the rest at
  `:515-518`; `remove_session` releases the whole thing at `:1069-1072`.
- Replay frame charges: `append_event` charges each unit at `:872-884` and
  stores the charge inside the `Arc<ReplayFrame>` (`:903-906`). The frame's
  charge drops with the last holder (`:126-129`), so a subscriber mid-replay
  pins its bytes until it drops the frame.
- Subscriber permit: `try_acquire_owned` at `:570`, held as `_total` inside
  `Subscription` (`:1117`). The per-run count is decremented in
  `Subscription::drop` (`:1161-1165`). When the per-run cap rejects at
  `:587-594`, `total` was declared before `index` (`:570` vs `:574`), so it
  drops after the guards, per the comment at `:588`.
- Backend permit: `acquire_owned` in the run task at `:691`, held as
  `_backend_permit` until the task returns (`:782`).

Release ordering is handled by `Released` (`:189-197`). Every index operation
declares `released` before `index` (`:348-349`, `:422-423`, `:498-499`,
`:635-636`), so the index guard drops first and the detached charges drop
after, avoiding the waiter-lock nesting described at `:6` and `:190`.

`shutdown` at `:616-644` sets `closed` (`:621`), cancels every run (`:624-626`),
closes and drains the tracker (`:627-628`), then removes every session
(`:637-640`). Because `send` spawns while holding the index lock (`:411-414`),
no run task can register after `closed` is set, so the drain at `:628` covers
every task that will ever exist.

Existing checks, verified:

- `every_path_returns_permits_and_charges_to_baseline`
  (`tests/broca_supervisor.rs:972-1031`). With `max_active_runs: 2` and
  `max_backend_processes: 1` (`:975-980`) it drives a conflict rejection
  (`:985-988`), an active-run-cap rejection (`:992-995`), a subscriber that
  reads one frame then drops (`:998-1001`), a completion (`:1003-1008`), a
  cancel and an idempotent second cancel (`:1009-1016`), a delete (`:1018`),
  and retention expiry under paused time (`:1019`). `assert_baseline` at
  `:1023` with `sessions == 0` checks all four permit counts, the retained
  budget, live runs, and tombstones (`:75-92`). A second supervisor is shut
  down with a live gated run and checked the same way (`:1026-1030`).
- `host_shutdown_drains_the_supervisor_to_zero_state` (`:1245-1289`). A real
  loopback host with a mid-stream subscriber is shut down; the metrics show
  zero sessions, full retained budget, and full subscriber, backend, and run
  pools (`:1278-1288`). It does not check `free_command_permits`.
- `transport_detach_paths_leave_the_run_untouched` (`:1098-1243`). Request
  cancel, route goodbye, and connection loss each detach a subscriber; the
  test polls until `free_subscriber_permits == 64` (`:1204-1214`).

All three are in the default-harness `broca_supervisor` binary, which CI runs
via `cargo test --workspace --all-targets` (`.github/workflows/ci.yml:118`).

## Failure scenario

The leak that would matter most is a backend permit, because the pool is 8
(`config.rs:140`) and a single leak drops it to 7 for the process lifetime.

1. A run acquires its backend permit at `:691` and starts the backend.
2. The backend future never returns.
3. `_backend_permit` is owned by the task future, so it cannot drop until the
   task does. `shutdown` waits on the tracker at `:628` and would hang.

As written, the subprocess runner bounds every path: `terminate_group`
(`subprocess.rs:670-702`) bounds `child.wait()` by `termination_grace`
(`:691`) and returns `Err`, which becomes `TeardownUnconfirmed` (`:544-551`)
and then `FailedUnresolved`. So the backend returns, the permit drops, and the
supervisor records `work_unresolved` (`supervisor.rs:770-772`). The record's
`Exercised` line is accurate: this path is covered only through those timers.

## Timing windows and dependencies

Two windows are deliberately closed by ordering rather than by a lock.

The request-byte excess is released only after both the terminal commit and
`work_done`. `finish` releases it if `work_done` is already true (`:992`);
otherwise `DoneGuard::drop` releases it once `terminal_appended` is true
(`:798`). If neither condition holds at either point, the run is still live
and the charge stays attached. Both sites call `split_excess`, so a second
call on an already-split charge releases nothing extra. This is what keeps a
cancelled-while-queued run from admitting a replacement before its parked
task has dropped `backend_request` (comment at `:989-990`).

The `delete` path takes a tombstone reservation before waiting (`:495`) so
that eviction during `wait_work_done` cannot consume the budget the tombstone
needs; the reservation is released at `:506-508` when the run is still owned
and the tombstone is carved from `base_charge` instead.

## What a test must construct

The existing tests cover every in-process terminal. The uncovered case is a
backend that never returns and ignores its cancellation token.
`ScriptedBackend::gated_ignoring_cancel` (`tests/support/broca.rs:117-134`)
exists and is used at `tests/broca_supervisor.rs:431`, `:605`, and `:800`,
but in each case the gate is eventually opened. A test that calls `shutdown`
without opening the gate would need a bound on the tracker wait, which the
supervisor does not have; it would hang. That absence is a design fact worth
recording, not a test to write: the supervisor relies on the backend
implementation to be bounded.

For the real runner, `sigkill_escalation_when_term_ignored`
(`tests/broca_subprocess.rs:2553-2592`) shows the bound holds with a child that
ignores `SIGTERM`, but it asserts the terminal shape, not the supervisor's
permit counts. A version that runs through a `Supervisor` and asserts
`free_backend_permits` after the run would close the gap.

## Investigation log

### Q: Does every terminal path release the run permit exactly once?

- Sources examined: `supervisor.rs:338`, `:401`, `:938-1001`, `:1059-1081`;
  every call to `finish` at `:457`, `:487`, `:684`, `:688`, `:694`, `:706`,
  `:716`, `:781`, `:922`.
- Findings: `finish` returns early at `:943-945` if a terminal is already
  appended or the run is purged, so only the first terminal reaches
  `state.run_permit.take()` at `:986`. `remove_session` also uses `take()` at
  `:1074`, so a purge after a terminal finds `None`. The two release sites are
  mutually exclusive by `Option::take`.
- Missing evidence: none.
- Conclusion: resolved. Exactly-once release is enforced by `take()` on both
  sites, not by the caller's discipline.

### Q: Can the tracker drain in `shutdown` be bypassed by a late `send`?

- Sources examined: `supervisor.rs:349-352`, `:411-414`, `:619-628`.
- Findings: `send` checks `index.closed` under the lock (`:350`) and spawns
  under the same lock (`:414`). `shutdown` sets `closed` under the lock
  (`:621`) before `tracker.close()` (`:627`). A `send` that took the lock
  before `shutdown` has spawned before releasing it; one that took it after
  sees `closed` and returns. No task can register between `close` and `wait`.
- Missing evidence: none.
- Conclusion: resolved. The drain is complete by construction.
