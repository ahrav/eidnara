# uo-p-no-row-for-withheld-composition

## Discovery trigger

Spec #1145 C2.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1155 branch tip (the commit that adds this file;
its parent is `d5ebb3598`).

- Writer guard: `record_use_outcome` (`crates/daemon/src/lib.rs:9628`).
- An Available composition read at sequence 0 names no commit and is refused by
  the Kernel, counted `refused`.

- Guarantee under test: A fold that persists a withheld composition writes no
  serving record; an Available snapshot with zero rows writes a record with zero
  decision rows.
- Existing check: `only_an_available_composition_is_recorded`
  (`crates/daemon/src/use_outcome_writer_tests.rs:170`);
  `a_withheld_read_then_an_available_read_of_the_same_rows_records_once`
  (`crates/daemon/src/use_outcome_writer_tests.rs:431`). Status: unaudited.

## Failure scenario

A withheld read treated as an empty set.

## Timing windows and dependencies

None.

## What a test must construct

#1155.

Required faults and enabling state: A Kernel starting or a refusing serving
verdict at read time, then a HARD fold.

## Investigation log

### Q: OQ3-11 resolved at #1155 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-11 resolved at #1155 ticketing: an empty Canonical composition
  writes a record with zero decision rows.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
