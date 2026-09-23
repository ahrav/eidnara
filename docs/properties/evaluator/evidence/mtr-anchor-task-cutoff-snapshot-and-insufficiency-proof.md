# mtr-anchor-task-cutoff-snapshot-and-insufficiency-proof

## Discovery trigger
Parent specification C-VAL: "Real-history tasks carry a built cutoff snapshot
with per-task cutoff validity and a recorded current-tree-only insufficiency
run." Ticket #767 acceptance.

## Evidence trail
- `crates/eval-core/src/anchor.rs` `CutoffAudit::validate` and
  `InsufficiencyProof::validate` (`NothingExecuted`, `ReferenceDoesNotPass`,
  `TreeAlreadyPasses`).
- `crates/daemon/examples/eval_runner/anchor.rs` `prepare`: clone through the
  host seam with the store charged while the clone exists, `git show -s
  --format=%ct` for both commits, `git merge-base --is-ancestor` for the
  descent, the earliest fix-side commit for the repair's publication,
  `git archive` of the base and of the whole fix commit piped into `tar` under
  `pipe_bounded`, `git diff -z --no-renames` from the fix's parent to the fix
  for the added paths, `hidden_test_name` for files directly under `tests/`,
  `tree_digest` over bytes, executable bits, and symlink targets for the
  snapshot, the fix tree, and the fix's parent; `run` copies the snapshot into
  a fresh tree with `cp -RP`, runs the hidden tests, copies the fix tree into a
  fresh tree, and runs them again as the reference.
- `crates/daemon/examples/eval_runner/suite_d.rs` `run_hidden` under
  `contain` (tree read-only, build cache writable, throwaway `HOME`, loopback
  up) and `harness_outcome`: every summary `ok` with at least one test passed.
- `crates/daemon/tests/eval_anchor.rs` `every_anchor_task_has_an_audit_a_proof_and_a_control_and_the_pilot_never_transfers`:
  over the pilot composition of local repositories, three tasks audited
  (each fix tree unlike its parent's), proven failing, and passing on the fix tree with a
  binary file, a symlink, and an executable script read by the hidden tests,
  one of them with a module under `tests/nested/`; one with an early fix
  skipped as `missing_cutoff_evidence` with `fix_not_after_cutoff` and no run;
  one with an unfetchable issue `source_unavailable`; one whose base depends on
  a crate the offline runner cannot resolve, every test `errored` on both
  trees, `Indeterminate` with `reference_does_not_pass` and no control.
- `crates/daemon/tests/eval_anchor.rs` `grading_runs_repository_code_without_the_runners_home_or_network`:
  the graded test sees neither the runner's `HOME` nor its loopback listener
  and can use its own loopback.
- `crates/eval-core/tests/anchor.rs` `the_cutoff_audit_excludes_future_code_and_future_issue_knowledge`,
  `the_insufficiency_proof_is_an_executed_failing_run`.

## Failure scenario
A snapshot built at the fix commit passes the hidden tests before any agent
runs; a runner that recorded the corpus row without executing the tree would
report a task that does not exist. A base tree the runner cannot build errors
on every hidden test; without the reference run that error would count as the
tree failing, and every control that also cannot build would be eligible.

## Timing windows and dependencies
None beyond the commit and issue timestamps.

## What a test must construct
A repository whose fix is dated before the cutoff, a tree run on the base
commit, a base with a dependency the offline runner cannot resolve, and base
files that only a byte-exact snapshot keeps.

## Investigation log
### Q: Where do the hidden tests of a real-history task come from?
- Sources examined: `prepare`.
- Findings: the test files the fix commit added under `tests/`; the fix's
  other changed files are what a memorizing control reproduces.
- Missing evidence: a real upstream task.
- Conclusion: resolved for the fixture; open for upstream corpora.
