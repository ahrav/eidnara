# uo-p-no-row-for-withheld-composition

## Discovery trigger

Spec #1145 C2.

Record kind: new obligation activated by #1155.

## Evidence trail

References are verified at the #1157 branch tip, the commit that carries this
revision.

- Writer guard: `record_use_outcome` (`crates/daemon/src/lib.rs:9721`); one
  write per pass covers its reruns.
- A composition read at sequence 0 precedes every commit; the handler skips its
  write, and the next fold after a commit records the serve.

- Guarantee under test: A fold that persists a withheld composition writes no
  serving record; an Available snapshot with zero rows at a tip of at least one
  commit attempts a record with zero decision rows, which exists once the write
  lands as `written` or as `replayed` against a live record; a receipt that
  outlives its expired record replays with no live row. At tip 0 the composition names no commit and
  the writer records nothing (`crates/daemon/src/lib.rs:9723`).
- Existing check: `only_an_available_composition_is_recorded`
  (`crates/daemon/src/use_outcome_writer_tests.rs:174`);
  `a_withheld_read_then_an_available_read_of_the_same_rows_records_once`
  (`crates/daemon/src/use_outcome_writer_tests.rs:514`);
  `the_real_failure_paths_count_their_outcomes_and_bound_the_wait`
  (`crates/daemon/src/use_outcome_writer_tests.rs:604`); Kernel half:
  `served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve`
  (`crates/kernel/tests/kernel_use_outcome.rs:164`). Status: unaudited.

## Failure scenario

A withheld read treated as an empty set.

## Timing windows and dependencies

A Kernel that is Starting at the pass's read and Ready by its write: the read
pins `Withheld`, and the writer must guard on that pinned value rather than on
the Kernel's readiness at the hand-off. The cited tests leave the Kernel
unopened for the whole pass, so this window is not constructed.

## What a test must construct

#1155.

Required faults and enabling state: A Kernel starting or a refusing serving
verdict at read time, then a HARD fold. The cited tests withhold the read by
leaving the Kernel unopened for the whole pass; a Kernel that is Starting at the
read and Ready by the write, which would separate guarding on the pinned
`Withheld` result from an unavailable Kernel at the write, is not constructed.

## Investigation log

### Q: OQ3-11 resolved at #1155 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-11 resolved at #1155 ticketing: an empty Canonical composition
  writes a record with zero decision rows.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
