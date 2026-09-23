# mtr-anchor-task-cutoff-snapshot-and-insufficiency-proof

## Discovery trigger
Parent specification C-VAL: "Real-history tasks carry a built cutoff snapshot
with per-task cutoff validity and a recorded current-tree-only insufficiency
run." Ticket #767 acceptance.

## Evidence trail
- `crates/eval-core/src/anchor.rs` `CutoffAudit::validate` and
  `InsufficiencyProof::validate`.
- `crates/daemon/examples/eval_runner/anchor.rs` `prepare`: clone through the
  host seam, `git show -s --format=%ct` for both commits, `git archive` of the
  base into the snapshot, `git diff --name-only --diff-filter=A` for the
  fix's added paths, the snapshot digest over its files; `run` writes the
  snapshot into a fresh tree and runs the fix's added tests with no agent.
- `crates/daemon/tests/eval_anchor.rs` `every_anchor_task_has_an_audit_a_proof_and_a_control_and_the_pilot_never_transfers`:
  three tasks audited and proven failing, one with an early fix skipped as
  `missing_cutoff_evidence` with `fix_not_after_cutoff` and no run, one with
  an unfetchable issue `source_unavailable`.
- `crates/eval-core/tests/anchor.rs` `the_cutoff_audit_excludes_future_code_and_future_issue_knowledge`,
  `the_insufficiency_proof_is_an_executed_failing_run`.

## Failure scenario
A snapshot built at the fix commit passes the hidden tests before any agent
runs; a runner that recorded the corpus row without executing the tree would
report a task that does not exist.

## Timing windows and dependencies
None beyond the commit and issue timestamps.

## What a test must construct
A repository whose fix is dated before the cutoff, and a tree run on the
base commit.

## Investigation log
### Q: Where do the hidden tests of a real-history task come from?
- Sources examined: `prepare`.
- Findings: the test files the fix commit added under `tests/`; the fix's
  other changed files are what a memorizing control reproduces.
- Missing evidence: a real upstream task.
- Conclusion: resolved for the fixture; open for upstream corpora.
