# uo-p-deleted-at-iff-session-delete-kernel-step

## Discovery trigger

Spec #1145 C10.

Record kind: new obligation activated by #1156.

## Evidence trail

References are verified at the #1156 branch tip, the commit that carries this
revision.

- Step: `delete_kernel_step` (`crates/daemon/src/lib.rs:6883`) retires the
  captured sources, then `mark_use_outcomes_deleted`
  (`crates/daemon/src/lib.rs:6908`) commits one envelope keyed by
  `use-outcome-delete` and a fresh attempt nonce
  (`crates/daemon/src/use_outcome_writer.rs:226`); an envelope that marks
  nothing rolls back.
- Order: the step runs at `crates/daemon/src/lib.rs:7007`, before the
  `stop_after_retirement` hook and the memory delete; a `kernel_unavailable`
  from either part counts `delete_kernel_unavailable`.
- A retry after a stop between the stores, or after an unknown commit outcome,
  selects no live record and commits nothing, so the first timestamp stays.

- Guarantee under test: `deleted_at` is set exactly when a `session.delete` for
  the session committed its Kernel step, before the memory-store delete; route
  teardown leaves it null.
- Existing check:
  `a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`);
  `a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:86`);
  `a_delete_step_with_an_unknown_outcome_converges_on_retry`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:136`);
  `a_second_delete_marks_records_written_after_the_first`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:172`);
  `a_route_torn_down_after_a_hard_fold_leaves_its_records_live`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:203`); Kernel half:
  `a_session_delete_marks_each_live_record_once`
  (`crates/kernel/tests/kernel_use_outcome.rs:935`). Status: unaudited.

## Failure scenario

A teardown that sets `deleted_at`.

## Timing windows and dependencies

Between the stores.

## What a test must construct

The tests above.

Required faults and enabling state: A delete with the Kernel available; one with
it starting; a route close without delete.

## Investigation log

### Q: A record written after the delete commits, by a pass on a route still bound to the session (OQ3-10), stays live until a later delete marks it; each delete selects the live records afresh under its own attempt key

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A record written after the delete commits, by a pass on a route
  still bound to the session (OQ3-10), stays live until a later delete marks it;
  each delete selects the live records afresh under its own attempt key. This
  includes a pass whose Memory Store commit preceded the delete and whose serve
  write landed after it: the harness received that composition, and the serve
  writer takes no capture gate, so the record counts as a continued session.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A fold after a delete over a composition the deleted record already holds replays that record, so the continued session gains no live record for it until its composition changes (needs human input)

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A fold after a delete over a composition the deleted record already
  holds replays that record, so the continued session gains no live record for
  it until its composition changes (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: A conflict or invalid input in the step answers `source_retirement_failed` and counts no lifecycle name; the closed names cover the marked, nothing-live, and unavailable outcomes only

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A conflict or invalid input in the step answers
  `source_retirement_failed` and counts no lifecycle name; the closed names
  cover the marked, nothing-live, and unavailable outcomes only.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
