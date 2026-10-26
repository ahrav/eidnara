# uo-p-served-set-and-identity-never-rewritten

## Discovery trigger

Spec #1145 C9 and AC3.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `ed75b9e5b`).

- Record triggers: `use_outcomes_identity_immutable`,
  `use_outcomes_deleted_at_set_once`,
  `use_outcomes_last_transform_at_monotonic`, `use_outcomes_retire_once`,
  `use_outcomes_no_delete` in the `use_outcomes` component
  (`crates/kernel/src/schema.rs:187`); decision-row triggers
  `use_outcome_decisions_written_with_record`,
  `use_outcome_decisions_no_update`, and `use_outcome_decisions_no_delete` in
  the `use_outcome_decisions` component (`crates/kernel/src/schema.rs:191`).
- Citation rule: the `admission_decisions.use_outcome_id` foreign key and the
  `admission_decisions_use_outcome_immutable` trigger
  (`crates/kernel/src/schema.rs:163`). P4 U3 writes the value with the admission
  row; a later write would cite a serve retroactively or release a cited record
  to the sweep.
- An equal rewrite of `deleted_at` or `last_transform_at` is accepted; the test
  asserts both.

- Guarantee under test: After commit, the identity columns, `served_commit_seq`,
  `session_role`, `recorded_at`, the classification columns, and the decision
  rows never change; `deleted_at` is set once, `last_transform_at` never
  decreases, retirement is set once, no row is deleted, a record is served at a
  commit before the one that records it, and an admission citation names a
  recorded serve when its row is written and never changes.
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2175`);
  `use_outcome_classification_and_identity_shapes_are_checked`
  (`crates/kernel/tests/kernel_schema.rs:2297`);
  `an_admission_citation_names_a_recorded_use_outcome_and_never_changes`
  (`crates/kernel/tests/kernel_schema.rs:2376`). Status: unaudited.

## Failure scenario

A cleared `deleted_at` would make a deleted session read as live.

## Timing windows and dependencies

None.

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
