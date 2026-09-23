# xc-suite-d-task-outcome-from-hidden-test

## Discovery trigger
Parent specification: "the post-run hidden test executes in the runner's own
process, not inside the agent's containment"; ticket #766: "Agent output
cannot select, modify, or replace the oracle. A planted hidden-test access
attempt is rejected."

## Evidence trail
- `crates/eval-core/src/task.rs` `task_terminal`: budget first, then all
  hidden tests passed, else `Fail`; empty results `Indeterminate`.
- `crates/eval-core/src/task.rs` `GeneratedTask::oracle_tamper`: hidden
  test paths, a `Cargo.toml` with `[[test]]`, `.cargo/config.toml`.
- `crates/daemon/examples/eval_runner/suite_d.rs` `hidden_results` builds
  a grading tree under the private directory (masked from the agent) from
  the corpus (the task's files, the
  candidate's regular files except `Cargo.toml`, `.cargo/`, and the
  hidden-test paths, and the hidden tests from the corpus) and runs
  `cargo test --offline --locked --test hidden_<name>` there, after the
  agent's containment exited and inside a grading containment of its own
  where the host has namespaces, with the build cache the only writable
  tree and the grade tree read-only; exit 0 with `test result: ok. 1 passed` is `passed`, exit 101
  with `test result: FAILED` is `failed`, anything else `errored`. Nothing
  in the agent's workspace is executed or written through.
- `crates/daemon/tests/eval_suite_d.rs` `grading_ignores_symlinked_hard_linked_and_undeletable_workspace_entries`:
  a `Cargo.toml` symlinked to a host file is not written through; two
  hidden-test paths hard-linked to one planted file do not share the grade;
  a `.cargo/config.toml` naming a forged runner inside a `0555` directory
  does not run; the grading `cargo` recorded the checkout's toolchain sysroot
  in `target/.rustc_info.json`.
- `crates/daemon/tests/eval_suite_d.rs` `a_bounded_run_past_its_deadline_kills_the_whole_process_tree_and_keeps_partial_output`
  and `a_bounded_run_whose_grandchild_keeps_stdout_open_still_returns_at_exit`:
  `run_bounded` kills the child's process group at the deadline and after a
  descendant holds stdout past the child's exit.
- `crates/daemon/tests/eval_suite_d.rs` `a_contained_task_is_judged_by_hidden_tests_the_agent_never_sees`:
  the agent plants `tests/hidden_sum_of_positives.rs`, replaces
  `Cargo.toml` with a symlink, copies whatever `../grade/tests` holds, and
  adds a `build.rs` that writes outside the run root and under
  `$XDG_RUNTIME_DIR`; `oracle_tamper` records the plant and the link and
  nothing copied; neither of the build script's files exists; the terminal
  is `Pass` from the corpus's tests.
- `crates/daemon/tests/eval_suite_d.rs` `a_wrong_fix_fails_a_no_fix_stays_failed_and_an_exhausted_budget_is_censored`:
  a wrong fix is `Fail` with its named test failed, although the agent's
  `build.rs` tried to rewrite that test and to plant a `rustc-wrapper` in
  Cargo's home and under the build cache, and its manifest became a
  directory (`Cargo.toml/x` recorded, the run not aborted); no fix stays `Fail`; 100
  extra tool calls are `Censored { max_tool_calls }` with no hidden test run.
- `crates/eval-core/tests/task.rs` `the_terminal_comes_from_the_hidden_tests_after_the_budget` and
  `an_agent_cannot_select_modify_or_replace_the_oracle`.

## Failure scenario
An agent writes `tests/hidden_sum_of_positives.rs` with an empty test; a
runner that ran the workspace's tests would report `Pass`. The first
version of the shell restored the oracle in place over the agent's
workspace: a `Cargo.toml` replaced by a symlink would have been written
through to its target, two hidden-test paths hard-linked together would have
both held the second test written, and a `.cargo/` made `0555` survived a
silent `remove_dir_all` with its runner config active. Grading in a tree the
runner builds closes all three.

## Timing windows and dependencies
The grading tree is built after the agent's containment exited, under the
directory that containment masked, so neither this task's tree nor an
earlier one is readable from the workspace. A grading `cargo` that outlives
its deadline is killed with its process group, so a looping test binary
does not survive the run.

## What a test must construct
A scripted agent that plants a hidden test, and a budget the script exceeds.

## Investigation log
### Q: Is a censored task's hidden test still run?
- Sources examined: `run` in the shell.
- Findings: no; the runner skips the hidden tests when the budget is
  exhausted and the terminal is `Censored`.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does a file the agent deleted reach the grade?
- Sources examined: `hidden_results`, `AgentTrace::written`.
- Findings: no. The grading tree starts from the task's files; a path the
  agent deleted from the workspace is absent from `written` and so present in
  the grade with its original contents. A fix that depended on deleting a
  source file would compile differently from the workspace.
- Missing evidence: none of the corpus's fixes delete a file.
- Conclusion: resolved with answer; a live agent's deletions are not
  reflected in the grade until the trace records them.
