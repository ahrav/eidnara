# credential-lifetime-covers-dispatch

## Discovery trigger

Specification #860, record N2: a refreshable row authorizes a spawn only under
`always(expiry > run deadline + 60s && run deadline never grows)`, witnessed at
threshold minus one, equal, and plus one, with clock jumps and queue or setup
delay. D4 fixes the 60-second skew. The M4 ticket (#872) lands the record with
its runnable checks.

## Evidence trail

- `crates/host-runtime/src/model_execution/aws_refresh.rs:38` sets
  `EXPIRY_SKEW` to 60 seconds.
- `aws_refresh.rs:363` `Inner::covers` requires
  `lease.usable_until > deadline + EXPIRY_SKEW` on the monotonic clock and
  `lease.expires_at_wall > wall + remaining + EXPIRY_SKEW` on the wall clock,
  where `remaining` is the deadline's distance from now. Both halves must hold.
- `aws_refresh.rs:1105` `lifetime_must_exceed_the_deadline_plus_skew_strictly`
  pushes rows of `model + skew - 1`, `model + skew`, and `model + skew + 1`
  seconds; only the last acquires. A refused row yields
  `SourceError::InsufficientLifetime` and the next acquisition yields
  `SourceError::Cooldown`. A row exactly at the skew is refused for a
  one-second demand.
- `crates/host-runtime/tests/model_execution_subprocess.rs:4073`
  `profile_rows_without_lifetime_start_no_child` returns a row `needed - 5`
  seconds long, then one with no lifetime, for both the OpenCode and Pi
  adapters, and asserts a `Permanent` `SourceFailed` terminal with no
  `argv.json` written. `needed` is `RUN_DEADLINE_SECS + EXPIRY_SKEW`.
- `model_execution_subprocess.rs:4140`
  `a_wall_jump_after_acquisition_refuses_the_spawn` installs a `ShiftedClock`
  that jumps the wall 3,600 seconds at its second read, between acquisition
  and the spawn recheck, for both adapters, and asserts no child starts.
- Both subprocess tests are registered by name in the manual test list at
  `model_execution_subprocess.rs:308` and `:316`.

## Failure scenario

A row whose lifetime equals the deadline plus skew is accepted, or a wall jump
after acquisition goes unnoticed. The child starts, its credentials expire
mid-run, and the run fails on the provider side after the model was billed.

## Timing windows and dependencies

The window is the interval between acquisition and the spawn recheck: setup
work, queueing, and a wall-clock jump can all land there. The monotonic bound
survives a wall rollback; the wall bound catches a forward jump. The check
depends on the owner's `ShiftedClock` seam in tests and on `SystemClock` in
production (`crates/daemon/src/bin/eidnara_host/serve.rs:1000`).

## What a test must construct

A row at, below, and just above the deadline plus skew; a wall jump injected
at the read between acquisition and the spawn recheck; both adapters, because
each carries its own spawn path; an assertion that no child artifact exists
rather than only that an error was returned.

## Investigation log

### Q: Does the predicate hold at every spawn, or only at acquisition?

- Sources examined: `aws_refresh.rs:208` (the acquisition contract), `:358`
  (`covers` at acquisition), `:612` `recheck_before_spawn` (`covers` again
  before the child starts, returning a `Transient` `SourceFailed` when the
  lease stopped covering the deadline).
- Findings: `covers` runs at acquisition and again in `recheck_before_spawn`;
  the wall-jump test fails only if the recheck is missing.
- Missing evidence: none for the three checks read.
- Conclusion: resolved with answer - the predicate is evaluated at both points.
