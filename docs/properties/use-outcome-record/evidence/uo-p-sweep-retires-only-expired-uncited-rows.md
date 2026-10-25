# uo-p-sweep-retires-only-expired-uncited-rows

## Discovery trigger

Spec #1145 C13, C14, D9; AC14, AC16.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1154 branch tip (the commit that adds this file;
its parent is `1c56ba033`).

- Selection: `expired_use_outcomes` (`crates/kernel/src/use_outcome.rs:447`)
  pages uncited expired records by `retain_until`, id, through
  `idx_use_outcomes_live_retention`, and counts cited expired records as
  retained.
- Retire: `retire_due_use_outcome` (`crates/kernel/src/use_outcome.rs:486`) sets
  `invalidated_commit_seq`; a citation found under the writer refuses with
  `Conflict`, which the sweep counts as retained.
- Registration: the Kernel maintenance slice runs the sweep after the capture
  sweeps (`crates/daemon/src/memory_reviewer/lifecycle.rs:334`); only
  retirements count as progress.
- A repeat at the same cutoff selects nothing and commits nothing; the test
  asserts the tip is unchanged.

- Guarantee under test: The expiry sweep retires a record only when
  `retain_until` is before the cutoff and no committed admission row with a live
  subject cites it, at most 64 per call, each under its own reserved receipt,
  rechecked inside the retiring commit.
- Existing check: `expiry_retires_an_expired_record_under_a_reserved_receipt`
  (`crates/kernel/tests/kernel_use_outcome.rs:374`);
  `an_expiry_backlog_drains_one_page_per_call`
  (`crates/kernel/tests/kernel_use_outcome.rs:433`);
  `an_expired_record_cited_by_a_live_report_is_retained`
  (`crates/kernel/tests/kernel_use_outcome.rs:490`);
  `a_record_cited_after_selection_is_kept`
  (`crates/kernel/src/use_outcome.rs:541`);
  `the_maintenance_slice_expires_use_outcomes_with_the_activation_gate_closed`
  (`crates/daemon/src/memory_reviewer/lifecycle.rs:790`). Status: unaudited.

## Failure scenario

A sweep that retired a cited record would leave a report's `use_outcome_id`
naming a retired serve.

## Timing windows and dependencies

Between selection under the reader and each commit under the writer.

## What a test must construct

The tests above (markers `uo_expiry_page_full`, `uo_expiry_retained_cited_row`).

Required faults and enabling state: An expired record; an expired cited record;
70 expired records; a citation inserted after selection.

## Investigation log

### Q: OQ3-8 resolved at #1154 ticketing

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: OQ3-8 resolved at #1154 ticketing: "live" means the citing row's
  subject object is unretired. Until P4 U3 writes the column, no production row
  cites a record.
- Missing evidence: none beyond what the question names.
- Conclusion: resolved with answer - see the question text.
