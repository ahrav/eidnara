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
  re-bound read-only.
- `canary_main` reads the secret and credential files under the private
  directory, connects to the runner's loopback listener, and starts the
  escapee under `setsid`; `run_canaries` samples the alive file twice after
  the child exited and kills a surviving escapee.
- `crates/eval-core/src/task.rs` `ContainmentReport::validate`.
- `crates/daemon/tests/eval_suite_d.rs` `a_contained_task_is_judged_by_hidden_tests_the_agent_never_sees`:
  every canary `denied` inside and `allowed` under the control.
- `crates/daemon/tests/eval_suite_d.rs` `a_host_without_namespaces_skips_every_task_with_no_containment`:
  `Containment::Skipped { no_containment }`, every task skipped, no agent
  run, adequacy still measured.
- `crates/eval-core/tests/task.rs` `every_canary_must_be_denied_inside_and_allowed_under_the_inverted_control`.

## Failure scenario
A control that is itself denied (the listener never bound, the escapee
binary missing) would make an inside denial vacuous; `ControlDenied`
refuses it.

## Timing windows and dependencies
The 300 ms samples of the alive file after the canary child exited.

## What a test must construct
A host seam reporting no namespaces, and the real namespaces where present.

## Investigation log
### Q: Is every path outside the workspace read-only inside?
- Sources examined: `MOUNTS`; a probe of `/var/tmp` before it was added.
- Findings: the named trees are; other world-writable paths on a host are
  not covered without a `pivot_root` into a read-only root.
- Missing evidence: a maintainer decision on the containment depth.
- Conclusion: unresolved, needs human input.
