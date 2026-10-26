# uo-p-last-transform-at-monotone-per-committing-pass

## Discovery trigger

Spec #1145 C11.

Record kind: new obligation activated by #1156.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `6d91bfa92`).

- Hook: `crates/daemon/src/lib.rs:10033` in the settle step, after any rerun,
  when the final response committed.
- Update: `crates/kernel/src/use_outcome.rs:43` selects the live records with a
  null or earlier value; a session with none takes no writer.
- Outcomes: `crates/daemon/src/use_outcome_writer.rs:209` counts
  `transform_touched`, `transform_skipped_deadline`, or
  `transform_skipped_unavailable`.

- Guarantee under test: `last_transform_at` never decreases on a session's
  serving record, and only a pass whose response committed updates it.
- Existing check: `last_transform_at_moves_forward_only_on_committing_passes`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:206`);
  `a_held_writer_skips_the_update_and_the_next_committing_pass_catches_up`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:291`); Kernel half:
  `last_transform_at_advances_live_records_without_a_commit`
  (`crates/kernel/tests/kernel_use_outcome.rs:892`),
  `a_held_writer_bounds_the_advance_and_an_idle_session_skips_it`
  (`crates/kernel/tests/kernel_use_outcome.rs:951`). Status: unaudited.

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
  deadline.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.

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
