# uo-p-deleted-at-iff-session-delete-kernel-step

## Discovery trigger

Spec #1145 C10.

Record kind: new obligation activated by #1156.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `a9060af05`).

- Step: `mark_use_outcomes_deleted` (`crates/daemon/src/lib.rs:6842`) lists the
  live records and commits one envelope keyed by `use-outcome-delete` and a
  fresh attempt nonce (`crates/daemon/src/use_outcome_writer.rs:167`).
- Order: the step follows `crates/daemon/src/lib.rs:6977` and precedes the
  `stop_after_retirement` hook and the memory delete.
- A retry after a stop between the stores lists no live record and commits
  nothing, so the first timestamp stays.

- Guarantee under test: `deleted_at` is set exactly when a `session.delete` for
  the session committed its Kernel step, before the memory-store delete; route
  teardown leaves it null.
- Existing check:
  `a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:39`);
  `a_delete_with_nothing_live_or_no_ready_kernel_marks_nothing`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:84`);
  `a_route_torn_down_after_a_hard_fold_leaves_its_records_live`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:113`); Kernel half:
  `a_session_delete_marks_each_live_record_once`
  (`crates/kernel/tests/kernel_use_outcome.rs:843`). Status: unaudited.

## Failure scenario

A teardown that sets `deleted_at`.

## Timing windows and dependencies

Between the stores.

## What a test must construct

The tests above.

Required faults and enabling state: A delete with the Kernel available; one with
it starting; a route close without delete.

## Investigation log

### Q: A record written after the delete, by a pass on a route still bound to

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A record written after the delete, by a pass on a route still bound
  to the session (OQ3-10), stays live until a later delete marks it; each delete
  lists the live records afresh under its own attempt key.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A conflict or invalid input in the step answers `source_retirement_fai

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A conflict or invalid input in the step answers
  `source_retirement_failed` and counts no lifecycle name; the closed names
  cover the marked, nothing-live, and unavailable outcomes only.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
