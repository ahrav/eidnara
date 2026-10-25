# uo-p-served-set-and-identity-never-rewritten

## Discovery trigger

Spec #1145 C9 and AC3.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- Trigger set: `use_outcomes_identity_immutable`,
  `use_outcomes_deleted_at_set_once`,
  `use_outcomes_last_transform_at_monotonic`, `use_outcomes_retire_once`,
  `use_outcomes_no_delete`, `use_outcome_decisions_no_update`,
  `use_outcome_decisions_no_delete` (`crates/kernel/src/schema.rs:187`).
- An equal rewrite of `deleted_at` or `last_transform_at` is accepted; the test
  asserts both.

- Guarantee under test: After commit, the identity columns, `served_commit_seq`,
  `session_role`, `recorded_at`, the classification columns, and the decision
  rows never change; `deleted_at` is set once, `last_transform_at` never
  decreases, retirement is set once, and no row is deleted.
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2171`). Status: unaudited.

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
