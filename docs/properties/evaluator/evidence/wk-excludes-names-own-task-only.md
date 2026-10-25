# wk-excludes-names-own-task-only

## Discovery trigger
Parent specification #1119 C10; Part B record `wk-excludes-names-own-task-only`; implemented by
#1122.

## Evidence trail
- `crates/eval-core/src/access.rs:33` `excludes` compares `key.task` with
  `target.task` only.
- `crates/eval-core/tests/access.rs::excludes_names_the_own_task_under_every_arm_and_nothing_else`
  runs the full two-task by three-arm table.

## Failure scenario
A predicate that compared `(task, arm)` would let the fresh arm's writes for
task t1 serve t1's aged sample, which is the same task's knowledge.

## Timing windows and dependencies
None.

## What a test must construct
Key `(t1, Fresh)` against target `(t1, Aged)`; key `(t2, Aged)` against the
same target.

Legal witness: `excludes({t1, Aged}, (t1, Fresh))` is true and `excludes({t1,
Aged}, (t2, Aged))` is false. Smallest falsifier: `excludes({t1, Aged}, (t1,
Fresh))` false.

## Investigation log
No open questions were raised while verifying this record.
