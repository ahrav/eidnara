# uo-p-no-row-for-withheld-composition

## Discovery trigger

Spec #1145 C2.

Record kind: proposed obligation; its owning ticket #1155 activates it.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `438a48b57`).

- The writer distinguishes withheld from empty.

- Guarantee under test: A fold that persists a withheld composition writes no
  serving record; an Available snapshot with zero rows writes a record with zero
  decision rows.
- Existing check: Kernel half only:
  `served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve`
  (`crates/kernel/tests/kernel_use_outcome.rs:163`). The writer is proposed.
  Status: unaudited.

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
