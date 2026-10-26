# uo-p-served-ids-equal-frozen-composition

## Discovery trigger

Spec #1145 C4.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `d5ebb3598`).

- Ids: the rendered rows of `CanonicalMemorySnapshot::new`, ascending
  (`served_ids` (`crates/daemon/src/canonical_memory.rs:80`)).
- Freeze marker: `froze_composition` set at each freeze site
  (`crates/daemon/src/transform.rs:3117`, `crates/daemon/src/transform.rs:4739`,
  `crates/daemon/src/transform.rs:4895`).

- Guarantee under test: The stored decision set equals, as a set, the object ids
  of the snapshot rows whose composition the same fold persisted.
- Existing check:
  `a_hard_fold_records_its_frozen_composition_once_per_served_set`
  (`crates/daemon/src/use_outcome_writer_tests.rs:60`);
  `a_dispute_between_folds_records_the_next_served_set_without_it`
  (`crates/daemon/src/use_outcome_writer_tests.rs:395`);
  `the_served_set_digest_names_the_rendered_ids_and_not_their_bytes`
  (`crates/daemon/src/canonical_memory.rs:335`). Status: unaudited.

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
