# uo-p-last-transform-at-monotone-per-committing-pass

## Discovery trigger

Spec #1145 C11.

Record kind: new obligation activated by #1156.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `dc07f442a`).

- Hook: `crates/daemon/src/lib.rs:10046` in the settle step, after any rerun,
  when the final response committed and the check
  (`crates/daemon/src/lib.rs:9738`) finds a live record to advance. The update
  runs on a scoped thread beside the response build, and the settle joins it
  before the response returns (`crates/daemon/src/lib.rs:10052`).
- Update: `crates/kernel/src/use_outcome.rs:43` selects the live records with a
  null or earlier value. A session with none takes no writer, because the
  check above skips it; the update itself holds only the writer.
- Outcomes: `crates/daemon/src/use_outcome_writer.rs:209` counts
  `transform_touched`, `transform_skipped_deadline`, or
  `transform_skipped_unavailable`.

- Guarantee under test: `last_transform_at` never decreases on a session's
  serving record, and only a pass whose response committed updates it.
- Existing check: `last_transform_at_moves_forward_only_on_committing_passes`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:220`);
  `a_held_writer_skips_the_update_and_the_next_committing_pass_catches_up`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:305`);
  `a_spent_pass_budget_skips_the_touch`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:353`);
  `the_serve_and_the_touch_of_one_pass_share_one_kernel_wait`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:382`);
  `a_touch_waiting_on_the_writer_overlaps_the_settle_and_lands_before_the_response`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:432`); Kernel half:
  `last_transform_at_advances_live_records_without_a_commit`
  (`crates/kernel/tests/kernel_use_outcome.rs:892`),
  `a_held_writer_bounds_the_advance_and_an_idle_session_skips_it`
  (`crates/kernel/tests/kernel_use_outcome.rs:950`),
  `an_outside_file_lock_bounds_the_advance_at_begin`
  (`crates/kernel/tests/kernel_use_outcome.rs:987`),
  `the_staleness_check_answers_for_null_or_earlier_values_and_writes_nothing`
  (`crates/kernel/tests/kernel_use_outcome.rs:1014`),
  `the_advance_takes_no_pooled_reader`
  (`crates/kernel/tests/kernel_use_outcome.rs:1046`). Status: unaudited.

## Failure scenario

An earlier clock lowering the value.

## Timing windows and dependencies

None.

## What a test must construct

The tests above.

Required faults and enabling state: A committing then a non-committing pass; a
Kernel unavailable at the write.

## Investigation log

### Q: OQ3-6 resolved at #1156 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-6 resolved at #1156 ticketing: a fenced, receipt-free monotone
  update.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: The pass budget is the 250 ms Kernel wait the pass shares across its s

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: The pass budget is the 250 ms Kernel wait the pass shares across its
  serve and touch writes (`PassWriteBudget`); the daemon carries no other pass
  deadline. The touch bounds its connection waits, its begin, and its statements
  by that budget; the serve write bounds its connection wait.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: Cost

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Cost: every committing pass of a session with a live record makes
  one fenced commit, on a scoped thread beside the response build, so its
  device flush overlaps the memory-store flush of the pass trace. Measured on
  this host (release build, one session with one record, paired runs of 240
  committing passes): with a fixed 1 ms flush on tmpfs the committing pass p50
  is 4.86 ms against 5.90 ms for the touch made before the response build; on
  XFS over EBS NVMe it is 8.68 to 8.95 ms against 9.99 to 10.17 ms. A coarser
  stale predicate would skip most of these commits and changes the
  per-committing-pass rule (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: A pass that commits and then fails its boundary read answers an error 

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A pass that commits and then fails its boundary read answers an
  error before the settle step, so it records its serve and touches nothing; the
  next committing pass catches up.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A session rebound to a route with memory disabled keeps its records un

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A session rebound to a route with memory disabled keeps its records
  untouched by those passes, because such a route takes no Kernel read (C2).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

### Q: A pass with an earlier clock selects no record, because the update fil

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: A pass with an earlier clock selects no record, because the update
  filters on a stored value before `now`; the trigger never sees a decrease, and
  the pass ignores any write failure (C11).
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
