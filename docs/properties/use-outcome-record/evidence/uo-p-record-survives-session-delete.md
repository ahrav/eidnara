# uo-p-record-survives-session-delete

## Discovery trigger

Spec #1145 KTD1; activated here with its Kernel-side check (deviation confirmed
at #1154 ticketing).

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `a9060af05`).

- Delete triggers: `crates/kernel/src/schema.rs:187`.
- Retained read: `served_set` (`crates/kernel/src/use_outcome.rs:375`) answers
  retired records.

- Guarantee under test: After `session.delete` answers `ok`, the session's
  serving records and decision rows are readable, equal in count and ids to
  before.
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2175`);
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:341`) (a retired record stays
  readable);
  `a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:39`);
  `a_delete_with_nothing_live_or_no_ready_kernel_marks_nothing`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:84`). Status: unaudited.

## Failure scenario

A delete path that removed Kernel rows would erase the served set.

## Timing windows and dependencies

None.

## What a test must construct

The `session.delete` route test above (marker
`uo_delete_stopped_between_stores`).

Required faults and enabling state: A session with records, then a delete
(Kernel arm: direct SQL deletes).

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
