# credential-lifetime-covers-dispatch

- Predicate: `Inner::covers` in
  `crates/host-runtime/src/model_execution/aws_refresh.rs:363` requires
  `lease.usable_until > deadline + EXPIRY_SKEW` on the monotonic clock and
  `lease.expires_at_wall > wall + remaining + EXPIRY_SKEW` on the wall clock.
- Unit check: `lifetime_must_exceed_the_deadline_plus_skew_strictly` at
  `aws_refresh.rs:1105` drives lifetimes around the bound.
- Subprocess checks: `profile_rows_without_lifetime_start_no_child` at
  `crates/host-runtime/tests/model_execution_subprocess.rs:4073` returns a row
  `needed - 5` seconds and `0` seconds long for both adapters and asserts a
  `Permanent` `SourceFailed` terminal with no `argv.json` written.
  `a_wall_jump_after_acquisition_refuses_the_spawn` at `:4140` jumps the wall
  between acquisition and the spawn recheck.
