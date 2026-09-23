# mtr-suite-d-canaries-denied-before-generated-code

## Discovery trigger
Parent specification Operational: "Suite D containment is Linux user,
mount, PID, and network namespaces (bubblewrap or `unshare`) with the case
workspace as the only writable mount; four canaries ... must report `denied`
from outside the containment, and an inverted control with containment
disabled must report `allowed` for each".

## Evidence trail
- `crates/daemon/examples/eval_runner/suite_d.rs` `contain`: `unshare
  --user --map-root-user --mount --pid --net --fork --kill-child`, then
  `MOUNTS`: an empty read-only tmpfs over the private directory, the
  workspace bound writable, `/tmp`, `/var/tmp`, `/dev/shm`, and `$HOME`
  re-bound read-only, `cd` into the workspace by its absolute path, then
  `exec setpriv --no-new-privs --inh-caps=-all --ambient-caps=-all
  --bounding-set=-all`. Any failing step exits 97 and the run is refused.
- `canary_main`, started in the workspace, reads the secret and credential
  files under the private directory, connects to the runner's loopback
  listener, starts the escapee under `setsid` and waits for its first write
  (`escapee_ready`), writes `../escaped.write`, runs `umount` on the private
  directory, and reads the secret again; `run_canaries` refuses when
  `escapee_ready`, `umount_ran`, or `proc_namespaced` (the PID `/proc/self`
  names is the canary's own) is false, when the contained canary connects to
  the runner's Unix socket under `$XDG_RUNTIME_DIR` or the control cannot,
  or when all 200 of the contained canary's forks succeed, then samples the alive file twice, 300 ms apart,
  after the child exited. The escapee exits on its own after three seconds.
- `crates/eval-core/src/task.rs` `ContainmentReport::validate` over the six
  `Canary` variants.
- `crates/daemon/tests/eval_suite_d.rs` `a_contained_task_is_judged_by_hidden_tests_the_agent_never_sees`:
  every canary `denied` inside and `allowed` under the control.
- `crates/daemon/tests/eval_suite_d.rs` `the_containment_denies_relative_writes_and_mask_removal_that_the_control_allows`:
  `run_canaries` alone; nothing written outside the workspace remains.
- `crates/daemon/tests/eval_suite_d.rs` `an_escapee_that_never_starts_refuses_the_canaries_instead_of_reading_as_denied`:
  a `Host` whose escapee is a missing binary refuses in both modes.
- `crates/daemon/tests/eval_suite_d.rs` `a_mask_removal_probe_that_never_ran_umount_refuses_the_canaries`:
  a canary whose `PATH` has no `umount` refuses instead of reading as denied.
- `crates/daemon/tests/eval_suite_d.rs` `a_host_without_namespaces_skips_every_task_with_no_containment`:
  `Containment::Skipped { no_containment }`, every task skipped, no agent
  run, adequacy still measured.
- `crates/eval-core/tests/task.rs` `every_canary_must_be_denied_inside_and_allowed_under_the_inverted_control`.

## Failure scenario
A control that is itself denied (the listener never bound, the escapee
binary missing) would make an inside denial vacuous; `ControlDenied`
refuses it, and a canary child that never saw the escapee's first write
refuses before any verdict is read.

Two escapes the first version of the shell allowed and the canaries did not
probe: a working directory inherited from before the mounts still names the
writable mount underneath the read-only rebind of `/tmp`, so `../x` was
writable while its absolute path was read-only (reproduced with the mount
script; this reached the runner's `target/` and a parent `.cargo/config.toml`
the grading `cargo` would have read), and the mapped root kept
`CAP_SYS_ADMIN` in its namespace, so `umount` of the private directory
exposed the secret (reproduced). The `cd` and `setpriv` in `MOUNTS` close
them, and the two new canaries hold the closure.

## Timing windows and dependencies
The 300 ms samples of the alive file after the canary child exited, which
itself waited up to five seconds for the escapee's first write.

## What a test must construct
A host seam reporting no namespaces, and the real namespaces where present.

## Investigation log
### Q: Is every path outside the workspace read-only inside?
- Sources examined: `MOUNTS`; a probe of `/var/tmp` before it was added.
- Findings: the named trees are; other world-writable paths on a host are
  not covered without a `pivot_root` into a read-only root.
- Missing evidence: a maintainer decision on the containment depth.
- Conclusion: unresolved, needs human input.
### Q: Can a slow host read a live escapee as denied?
- Sources examined: the escape verdict in `run_canaries` (`suite_d.rs`): two
  samples of the alive file 300 ms apart after the canary child exited,
  `allowed` when the second is non-empty and differs from the first; the
  escapee's 50 ms rewrite loop and `ESCAPEE_LIFETIME`.
- Findings: the verdict is a timing measurement. An escapee that survived but
  was descheduled for the whole 300 ms window, or whose writes had not yet
  reached the file the runner reads, reads as `denied`; the inverted control
  catches a host where the escapee never runs at all, and the canary child's
  refusal of an escapee that never wrote covers a start failure, not one that
  runs late.
- Missing evidence: a bound on the sampling window the maintainer accepts,
  or a verdict that waits for the escapee's own exit rather than sampling.
- Conclusion: unresolved, needs human input.

### Q: Can the agent read the host user's home directory inside?
- Sources examined: `MOUNTS`; `$HOME` is re-bound read-only, not covered.
- Findings: yes. Reads of dotfiles and credentials under `$HOME` succeed;
  the network namespace denies exfiltration by socket, but the workspace and
  stdout leave the containment with the run.
- Missing evidence: a maintainer decision on masking the home directory.
- Conclusion: unresolved, needs human input.
