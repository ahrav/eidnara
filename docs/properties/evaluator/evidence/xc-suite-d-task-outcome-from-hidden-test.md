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
- `crates/daemon/examples/eval_runner/suite_d.rs` `hidden_results` writes
  each hidden test from the corpus and runs `cargo test --offline --test
  hidden_<name>` outside the containment; exit 0 is `passed`, exit 101 with
  `test result: FAILED` is `failed`, anything else `errored`.
- `crates/daemon/tests/eval_suite_d.rs` `a_contained_task_is_judged_by_hidden_tests_the_agent_never_sees`:
  the agent plants `tests/hidden_sum_of_positives.rs`; `oracle_tamper`
  records it; the terminal is `Pass` from the corpus's tests.
- `crates/daemon/tests/eval_suite_d.rs` `a_wrong_fix_fails_a_no_fix_stays_failed_and_an_exhausted_budget_is_censored`:
  a wrong fix is `Fail` with its named test failed; no fix stays `Fail`; 100
  extra tool calls are `Censored { max_tool_calls }` with no hidden test run.
- `crates/eval-core/tests/task.rs` `the_terminal_comes_from_the_hidden_tests_after_the_budget` and
  `an_agent_cannot_select_modify_or_replace_the_oracle`.

## Failure scenario
An agent writes `tests/hidden_sum_of_positives.rs` with an empty test; a
runner that ran the workspace's tests would report `Pass`.

## Timing windows and dependencies
The hidden tests are written after the agent's containment exited.

## What a test must construct
A scripted agent that plants a hidden test, and a budget the script exceeds.

## Investigation log
### Q: Is a censored task's hidden test still run?
- Sources examined: `run` in the shell.
- Findings: no; the runner skips the hidden tests when the budget is
  exhausted and the terminal is `Censored`.
- Missing evidence: none.
- Conclusion: resolved with answer.
### Q: Does grading run candidate code with the runner's authority?
- Sources examined: `hidden_results` in the shell (`suite_d.rs`), which runs
  `cargo test --offline --test hidden_<name>` "under the runner's own
  authority, outside any containment"; the module doc at `suite_d.rs:3`.
- Findings: yes. The hidden tests compile and run the agent's tree outside
  the namespaces, so a build script or a test body written by the agent runs
  as the runner. The oracle restore covers the manifest, `.cargo/`, and the
  hidden test files, not what `src/` may do at build or test time.
- Missing evidence: a maintainer decision on grading inside its own
  restricted worker, and what that worker may keep (the target directory,
  the network).
- Conclusion: unresolved, needs human input.
