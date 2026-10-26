# uo-disposition-events-append-rows-and-newest-by-commit-seq-wins

## Discovery trigger

Spec #1145 D3.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `a9060af05`).

- Append: `crates/kernel/src/admission.rs:1748`.
- Newest wins: `crates/kernel/src/admission.rs:675`.
- Correction: the spec's `kernel_admission.rs:2571` and `:2832` test retirement
  and time travel, not append or newest-wins.

- Guarantee under test: Disposition events append admission rows; the newest
  committed row by (`commit_seq`, `admission_decision_id`) decides the subject's
  disposition.
- Existing check: `both_latest_decision_formulations_choose_the_same_row`
  (`crates/kernel/tests/kernel_admission.rs:2252`);
  `an_event_that_names_a_disposition_keeps_its_own_outcome`
  (`crates/kernel/src/admission.rs:5245`). Status: unaudited.

## Failure scenario

A later approval hiding an earlier dispute from the join would erase a harm
signal; the join reads events, so it does not.

## Timing windows and dependencies

Two events in one commit share a sequence.

## What a test must construct

The existing test; marker `uo_two_disposition_events_one_commit` in #1154.

Required faults and enabling state: A `mark_stale` and a `mark_disputed`
committed together.

## Investigation log

### Q: A decision disputed then re-approved still counts as disputed after se

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A decision disputed then re-approved still counts as disputed after
  serve: event existence counts (resolved at #1154 ticketing).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
