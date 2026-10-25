# wk-key-is-task-arm-and-lineage-invariant

## Discovery trigger
Parent specification #1119 C9 and D2; Part B record `wk-key-is-task-arm-and-lineage-invariant`;
implemented by #1122.

## Evidence trail
- `crates/eval-core/src/access.rs:15` `WriteKey {task, arm}`; `:22`
  `From<&SampleRecord>` copies those two fields.
- `crates/eval-core/tests/access.rs::the_write_key_is_task_and_arm_and_retries_share_it`.

## Failure scenario
A runner that keyed writes by sample id would give a retry a fresh pool, and
the retry's own earlier writes, under another id, would read as another
task's.

## Timing windows and dependencies
None.

## What a test must construct
Two records of one task under `Aged` and `Fresh`; a retry record with a
non-empty lineage.

Legal witness: `{t1, Aged, lineage: []}` and `{t1, Aged, lineage: [run_a]}`
yield one key. Smallest falsifier: a key that compares `lineage`.

## Investigation log
### Q: Does this record discharge `led-non-empty-lineage-reachable-from-a-producer`?
- Sources examined: the fixtures in `crates/eval-core/tests/access.rs`
- Findings: the lineage values are hand-built; no in-tree producer writes a
  non-empty lineage
- Missing evidence: a runner (#1007) that retries a sample.
- Conclusion: unresolved, needs a runner (#1007) that retries a sample
### Q: Catalog open question: Two history policies of one arm share one key, as #1122 resolved OQ7-4
- Sources examined: `crates/eval-core/src/access.rs:22`
  (`From<&SampleRecord>`)
- Findings: Two history policies of one arm share one key, as #1122 resolved
  OQ7-4; a `task_arm_policy` scheme would be a new `WriteKeyScheme` variant
  and a new family digest
- Missing evidence: an owner decision.
- Conclusion: needs human input
