# uo-p-served-set-and-identity-never-rewritten

## Discovery trigger

Spec #1145 C9 and AC3.

Record kind: new obligation activated by #1154.

## Evidence trail

References were first verified at the #1157 branch tip (the commit that adds
this file; its parent is `d85de937b`) and are re-verified at the #1162 branch
tip.

- Record triggers: `use_outcomes_identity_immutable` (its column list includes
  `decision_count`),
  `use_outcomes_deleted_at_set_once`,
  `use_outcomes_last_transform_at_monotonic`, `use_outcomes_retire_once`,
  `use_outcomes_no_delete` in the `use_outcomes` component
  (`crates/kernel/src/schema.rs:193`); decision-row triggers
  `use_outcome_decisions_written_with_record` (refuses a row once the record's
  `decision_count` rows exist or once a later commit exists: a Kernel-written
  set is complete at its commit, and any record's set is sealed by the next
  commit),
  `use_outcome_decisions_name_decisions` (refuses an `object_id` whose registry
  kind is other than `decision`, or whose registry row was created after or
  retired at or before the record's `served_commit_seq`; the Kernel reports it
  as `InvalidInput`),
  `use_outcome_decisions_no_update`, and `use_outcome_decisions_no_delete` in
  the `use_outcome_decisions` component (`crates/kernel/src/schema.rs:197`).
  #1162 adds `session_seq` to the immutable record columns, makes
  `use_outcome_decisions_written_with_record` also require a decision row's
  `session_seq` and `served_commit_seq` to equal its record's, and adds
  `use_outcome_sessions_no_update` and `use_outcome_sessions_no_delete`
  (`crates/kernel/src/schema.rs:189`); a record's foreign key over
  `session_seq` and its identity columns ties it to its own session row.
- Citation rule: the `admission_decisions.use_outcome_id` foreign key, the
  `admission_decisions_use_outcome_immutable` trigger (which also freezes
  `subject_object_id` and `commit_seq` on a citing row, the two columns the
  sweep's citation predicate reads), and the
  `admission_decisions_citation_no_delete` trigger, which refuses the delete
  that `INSERT OR REPLACE` runs on a citing row
  (`crates/kernel/src/schema.rs:164`). P4 U3 writes the value with the admission
  row; a later write would cite a serve retroactively or release a cited record
  to the sweep.
- An equal rewrite of `deleted_at` or `last_transform_at` is accepted; the test
  asserts both.

- Guarantee under test: After commit, the identity columns, `served_commit_seq`,
  `session_role`, `recorded_at`, the classification columns, `decision_count`,
  `session_seq`, the session rows, and the decision rows never change;
  `deleted_at` is set once, `last_transform_at` never decreases, retirement is
  set once, no row is deleted, a record is served at a commit before the one
  that records it, and an admission citation names a recorded serve when its
  row is written, never changes, keeps its subject and commit sequence, and its
  row is never deleted.
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2210`);
  `use_outcome_classification_and_identity_shapes_are_checked`
  (`crates/kernel/tests/kernel_schema.rs:2427`);
  `an_admission_citation_names_a_recorded_use_outcome_and_never_changes`
  (`crates/kernel/tests/kernel_schema.rs:2566`). Status: unaudited.

## Failure scenario

A cleared `deleted_at` would make a deleted session read as live.

## Timing windows and dependencies

The window between the recording commit and the next commit: under a tip-only
guard a raw writer could append a decision row there, and under a count-only
guard a raw writer that recorded fewer rows than its `decision_count` could add
them in any later transaction. The trigger applies both bounds; the test
appends in the window and after a later commit and asserts both aborts.

## What a test must construct

The trigger test; the #1156 writers run against these triggers.

Required faults and enabling state: Direct SQL for each forbidden transition,
including REPLACE.

## Investigation log

### Q: OQ3-10

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-10: a session that continues after its delete reuses its
  identity at an equal composition; resolved at #1155 ticketing as an accepted
  loss (gap G2).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
