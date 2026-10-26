# uo-p-no-row-when-memory-disabled

## Discovery trigger

Spec #1145 C2.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1155 branch tip (the commit that carries this
revision; its parent is `1fe3dfbad`).

- Writer guard: `record_use_outcome` (`crates/daemon/src/lib.rs:9669`).

- Guarantee under test: A route bound with `memory.enabled` off produces no
  serving record on any pass.
- Existing check: `only_an_available_composition_is_recorded`
  (`crates/daemon/src/use_outcome_writer_tests.rs:170`). Status: unaudited.

## Failure scenario

A writer keyed off request fields.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A route bound with `/memory/enabled: false`,
then HARD folds on both engines.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
