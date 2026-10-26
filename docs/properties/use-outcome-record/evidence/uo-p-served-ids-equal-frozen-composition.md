# uo-p-served-ids-equal-frozen-composition

## Discovery trigger

Spec #1145 C4.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `d85de937b`).

- Ids: the rendered rows of `CanonicalMemorySnapshot::new`, ascending
  (`served_ids` (`crates/daemon/src/canonical_memory.rs:81`)).
- Freeze marker: every freeze site calls `freeze_project_memory`
  (`freeze_project_memory` (`crates/daemon/src/transform.rs:613`)), which
  assigns the composition and reports the freeze
  (`crates/daemon/src/transform.rs:3122`, `crates/daemon/src/transform.rs:4743`,
  `crates/daemon/src/transform.rs:4898`).

- Guarantee under test: The stored decision set equals, as a set, the object ids
  of the snapshot rows whose composition the same fold persisted.
- Existing check:
  `a_hard_fold_records_its_frozen_composition_once_per_served_set`
  (`crates/daemon/src/use_outcome_writer_tests.rs:64`);
  `a_dispute_between_folds_records_the_next_served_set_without_it`
  (`crates/daemon/src/use_outcome_writer_tests.rs:465`);
  `the_served_set_digest_names_the_rendered_ids_and_not_their_bytes`
  (`crates/daemon/src/canonical_memory.rs:336`);
  `rows_past_the_memory_budget_are_neither_injected_nor_digested`
  (`crates/daemon/src/canonical_memory.rs:471`);
  `a_fold_whose_boundary_read_fails_after_its_commit_records_its_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:765`); Kernel half:
  `duplicate_and_malformed_serves_are_refused_without_rows`
  (`crates/kernel/tests/kernel_use_outcome.rs:214`),
  `session_roles_round_trip_and_the_served_set_digest_is_order_free`
  (`crates/kernel/tests/kernel_use_outcome.rs:750`). Status: unaudited.

## Failure scenario

Ids drawn from a later read.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A read trimmed by the token budget; a CAS
retry.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
