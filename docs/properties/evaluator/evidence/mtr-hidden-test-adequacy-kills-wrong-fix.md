# mtr-hidden-test-adequacy-kills-wrong-fix

## Discovery trigger
Parent specification Phase 6: "executable generated repos with fail-to-pass
hidden tests and adequacy evidence (hand-written wrong fixes; mutation is one
sufficient method, not mandatory)".

## Evidence trail
- `crates/eval-core/src/task.rs` `generate_tasks`: three wrong fixes per
  task (`absolute-first`, `saturating-at-zero`, `swapped-when-equal`), each
  naming the hidden test it fails.
- `crates/eval-core/src/task.rs` `check_adequacy` and `validate`.
- `crates/daemon/examples/eval_runner/suite_d.rs` `run` materializes the
  baseline, the correct fix, and every wrong fix into a fresh workspace and
  runs the hidden tests under the runner's authority; a refusal stops the
  campaign as `RunError::Adequacy`.
- `crates/daemon/tests/eval_suite_d.rs` `a_contained_task_is_judged_by_hidden_tests_the_agent_never_sees`
  asserts the baseline fails, the correct fix passes, and three wrong fixes
  were measured; the no-namespace test shows adequacy still runs when the
  agent cannot.
- `crates/eval-core/tests/task.rs` `adequacy_needs_fail_to_pass_and_every_wrong_fix_killed_by_its_named_test`
  and `a_task_refuses_a_missing_or_visible_oracle_and_a_text_only_fix`.

## Failure scenario
A wrong fix `a + b.abs()` passes `sum_of_a_negative` because both cases have
a positive second argument; the corpus would claim a kill it never made.

## Timing windows and dependencies
None.

## What a test must construct
A corpus whose fixes are executed by real Cargo, and mutated evidence tables
for each refusal.

## Investigation log
### Q: Does the failing test have to be the one the fix names?
- Sources examined: `check_adequacy`.
- Findings: yes; failing another test is `WrongFixSurvives`.
- Missing evidence: none.
- Conclusion: resolved with answer.
