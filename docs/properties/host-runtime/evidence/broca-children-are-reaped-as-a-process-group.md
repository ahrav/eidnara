# broca-children-are-reaped-as-a-process-group

## Discovery trigger

The runner's module doc at `subprocess.rs:3-4` names the process group as the
cancellation unit, and the comment at `:327-331` states the limit of the other
mechanism: `pdeathsig` "applies only to the leader", so provider and extension
grandchildren can outlive it. A grandchild that survives its parent still has
the provider credential in its environment. The audit traced the three
teardown paths (cancel, delete, shutdown) from the supervisor into the runner
and then into the startup sweep for the crash case.

## Evidence trail

All references are at `e16e39e`.

Spawn. `run` at `subprocess.rs:300` configures the child with
`.process_group(0)` (`:324`) and `.kill_on_drop(true)` (`:326`). The
`pre_exec` closure sets `PR_SET_PDEATHSIG` to `KILL` on Linux (`:343-344`) and
aborts the exec if the parent is no longer the host (`:345-348`), closing the
window where the host dies between `fork` and the pdeathsig call. With
`process_group(0)` the leader's PID is the group ID (`:369-373`). A registry
record is written at `:376-377` before the prompt is delivered; on registry
failure the group is killed and the run fails before any prompt bytes flow
(`:378-396`).

Teardown on cancel. The run loop selects `cancel.cancelled()` first under
`biased` (`:434-436`), breaks with `SubprocessEnd::Cancelled`, and calls
`terminate_group` (`:510-512`). `terminate_group` at `:670-702` sends `TERM`
to the group (`:675`), waits up to `grace` for the leader to exit without
reaping it (`:679`), escalates to `KILL` if the leader is still running
(`:680-685`), sends a fenced `KILL` (`:686`), waits for other members to be
gone (`:689`), and bounds the final `child.wait()` by `grace` (`:691-695`). If
members are not confirmed gone it returns `Err` (`:696-700`), which the caller
maps to `TeardownUnconfirmed` (`:544-551`) and then `FailedUnresolved`
(`:983-985`), so the supervisor records `work_unresolved`
(`supervisor.rs:770-772`) and `cancel` and `delete` return
`teardown_unconfirmed` (`:557-566`).

The leader is kept unreaped while members are checked so its zombie pins the
PGID (`:596-597`, `:649-651`). `kill_group_fenced` at `:631-647` refuses to
signal a PGID whose leader was already reaped unless a member is still found,
because a reaped leader's PGID may already belong to an unrelated group
(`:625-630`).

Supervisor entry points. `cancel` fires the run token at `supervisor.rs:456`,
`delete` at `:486`, and `shutdown` cancels every run at `:624-626`. The token
reaches the runner as `backend_cancel` (`:747`, `:750`). Both `cancel` and
`delete` wait for `work_done` (`:464`, `:496`), which `DoneGuard` sets only
when the task returns (`:797`), so the lifecycle call resolves after the
leader is reaped.

Startup sweep. `sweep_orphaned_groups` at `subprocess.rs:1479-1539` runs from
`BrocaComponent::initialize` (`mod.rs:341`). It skips records from another
boot (`:1501-1504`), skips records whose owner is alive with the recorded start
time (`:1505-1507`), treats a zombie owner as dead (`:1280-1282`), decides
group liveness by leader PID and start time or by surviving members
(`:1508-1518`), sends `KILL` to the group (`:1521`), and waits for the group to
empty before removing the record (`:1530-1536`).

Existing checks, verified, all in the `harness = false` binary
(`Cargo.toml:36-38`) which CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`):

- `cancel_reaps_group_with_sigterm_first`
  (`tests/broca_subprocess.rs:2519-2551`). The `grandchild_hang` fixture forks
  a grandchild (`:594-618`), which writes `grandchild-ready` and then
  `grandchild-sigterm` on `SIGTERM` (`:661-672`). The test cancels after
  readiness, asserts `Failed` (`:2543`), asserts the leader is already gone
  with a zero-retry probe (`:2544`, helper at `:1093-1101`), polls up to four
  seconds for the grandchild (`:2545`, helper at `:1079-1090`), and requires
  the `grandchild-sigterm` marker (`:2547-2550`), which proves group-wide
  `SIGTERM` rather than leader-only delivery.
- `sigkill_escalation_when_term_ignored` (`:2553-2592`). The `hang_ignore_term`
  fixture installs a `SIGTERM` handler and never exits (`:574-591`). With
  `termination_grace: 500 ms` (`:2557`), the test asserts a bounded elapsed
  time (`:2585-2588`), the `got-sigterm` marker (`:2590`), and a reaped
  leader (`:2591`).
- `supervisor_delete_reaps_group` (`:2608-2635`) and
  `supervisor_shutdown_reaps_group` (`:2637-2663`) drive the same fixture
  through a real `Supervisor` and assert leader reaped and grandchild gone.
- `group_registry_sweep_kills_only_dead_owner_groups` (`:3045-3176`) covers a
  dead owner (`:3070-3086`), a live owner whose group must survive
  (`:3078-3091`), a zombie owner (`:3100-3128`), and a leaderless group whose
  grandchild must die (`:3131-3175`).
- `timeout_reaps_leader_and_grandchild` (`:2131-2156`) covers the same
  teardown on the timeout path; the record does not list it.

## Failure scenario

1. A harness child forks a provider process, which inherits the credential
   row in its environment.
2. The user cancels. If the host signalled only the direct child, the leader
   dies and the grandchild is reparented to `init`, holding the credential and
   possibly still executing a billable request.
3. As written, `kill(-pgid, SIGTERM)` reaches both, and the grandchild's
   marker proves it.

The sweep's failure shape is the inverse: signalling a group whose owner is
alive would kill another live host's run. The owner-start-time check at
`:1505-1507` is the guard; the survivor case in the sweep test is its proof.

## Timing windows and dependencies

Every wait is bounded by `termination_grace` (default 5 s, `:232`). A member
in uninterruptible kernel state can outlast `SIGKILL`; then `terminate_group`
returns `Err`, the run becomes `FailedUnresolved`, and the registry record is
retained (`:547-549`) for the next host's sweep. That path is asserted in the
supervisor tests only indirectly, through
`unproven_teardown_fails_cancel_and_delete` in
`tests/broca_supervisor.rs:685`, which uses a scripted backend rather than
a real stuck process.

The sweep runs once, at initialize (`mod.rs:341`). It is not periodic, so a
group orphaned by a host crash survives until the next host starts.

## What a test must construct

The gap is in the shutdown test. `supervisor_shutdown_reaps_group` discards
the `usize` that `shutdown` returns (`tests/broca_subprocess.rs:2659`), and
`assert_process_gone` polls for up to four seconds (`:1083-1090`). If teardown
were unconfirmed and the grandchild died late, the test would still pass. A
strengthened version asserts `shutdown().await == 0` and probes the grandchild
with the zero-retry `assert_leader_already_reaped` helper. The cancel and
delete variants already discriminate: `Failed` at `:2543` is not
`FailedUnresolved`, and `expect("delete succeeds")` at `:2631` would fail on
`teardown_unconfirmed`.

## Investigation log

### Q: Does the sweep ever signal a group whose owner is alive?

- Sources examined: `subprocess.rs:1274-1282`, `:1505-1507`, `:1508-1518`;
  `tests/broca_subprocess.rs:3078-3095`.
- Findings: the owner check compares the live start time from
  `/proc/<pid>/stat` with the recorded value and `continue`s on a match. A
  recycled owner PID with a different start time is treated as dead, which is
  correct. A zombie owner is treated as dead by `proc_live_start_time`. The
  survivor case in the test spawns a leader, records it in-process, sweeps,
  and asserts the leader is still running.
- Missing evidence: none.
- Conclusion: resolved. Live-owner groups are skipped before any liveness or
  kill decision is made.

### Q: Does the shutdown test prove the grandchild died before the terminal?

- Sources examined: `tests/broca_subprocess.rs:2637-2663`, `:1079-1101`;
  `supervisor.rs:616-644`.
- Findings: no. The returned unresolved count is discarded and the grandchild
  probe tolerates four seconds of lag. The cancel and delete tests do
  discriminate through the terminal variant and the `Result`.
- Missing evidence: an assertion on `shutdown`'s return value and a zero-retry
  grandchild probe in the shutdown test.
- Conclusion: unresolved, needs the strengthened shutdown test described
  above. The mechanism is the same code path as cancel and delete, so the
  property most likely holds, but the shutdown test as written cannot refute
  a late kill.
