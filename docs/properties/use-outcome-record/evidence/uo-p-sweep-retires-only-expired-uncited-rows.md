# uo-p-sweep-retires-only-expired-uncited-rows

## Discovery trigger

Spec #1145 C13, C14, D9; AC14, AC16.

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `742896941`).

- Selection: `expired_page` (`crates/kernel/src/use_outcome.rs:606`) reads one
  statement, so one snapshot, in `retain_until`, id order through
  `idx_use_outcomes_live_retention` (`crates/kernel/src/use_outcome.rs:219`); it
  pages up to 64 uncited records and counts the cited ones it reads before the
  page fills. The scan reads cited rows ahead of the page, so its read work
  grows with the cited expired records while its writes stay bounded by the
  page.
- Commit: one page commit keyed by the cutoff and a digest of the page's ids
  (`page_key` (`crates/kernel/src/use_outcome.rs:564`), `expire_use_outcomes`
  (`crates/kernel/src/use_outcome.rs:516`)); inside it `retire_due_use_outcome`
  (`crates/kernel/src/use_outcome.rs:630`) rechecks retirement and citation
  (`crates/kernel/src/use_outcome.rs:227`), sets `invalidated_commit_seq`, and
  records the record's own reserved receipt. The deadline is immutable, so the
  recheck reads only liveness and citation. A citation found under the writer
  keeps the record and is counted `cited`; a page that retires nothing rolls
  back and leaves no receipt.
- Registration: the Kernel maintenance slice runs the sweep after the capture
  sweeps (`crates/daemon/src/memory_reviewer/lifecycle.rs:334`); only
  retirements count as progress, because a cited record is counted again on
  every pass while its citation holds.
- A repeat at the same cutoff selects nothing and commits nothing; the test
  asserts the tip is unchanged.

- Guarantee under test: The expiry sweep retires a record only when
  `retain_until` is before the cutoff and no committed admission row with a live
  subject cites it, at most 64 per call in one commit keyed by a digest of the
  page, each under its own reserved receipt keyed by cutoff and id, rechecked
  inside that commit; a page that retires nothing commits nothing; the Kernel
  maintenance slice calls it once per pass whenever the Kernel is Ready.
- Existing check: `expiry_retires_an_expired_record_under_a_reserved_receipt`
  (`crates/kernel/tests/kernel_use_outcome.rs:474`);
  `an_expiry_backlog_drains_one_page_per_call`
  (`crates/kernel/tests/kernel_use_outcome.rs:534`);
  `an_expired_record_cited_by_a_live_report_is_retained`
  (`crates/kernel/tests/kernel_use_outcome.rs:604`);
  `a_record_cited_after_selection_is_kept`
  (`crates/kernel/src/use_outcome.rs:771`);
  `a_page_another_sweep_committed_at_the_same_cutoff_replays`
  (`crates/kernel/src/use_outcome.rs:799`);
  `a_record_retired_after_selection_at_another_cutoff_is_skipped`
  (`crates/kernel/src/use_outcome.rs:862`);
  `a_record_kept_at_a_cutoff_retires_at_that_cutoff_once_uncited`
  (`crates/kernel/src/use_outcome.rs:883`);
  `the_cited_count_covers_the_rows_read_before_the_page_fills`
  (`crates/kernel/tests/kernel_use_outcome.rs:799`);
  `the_sweep_and_the_reads_use_their_indexes`
  (`crates/kernel/src/use_outcome.rs:900`);
  `the_maintenance_slice_expires_use_outcomes_with_the_activation_gate_closed`
  (`crates/daemon/src/memory_reviewer/lifecycle.rs:829`);
  `a_cancelled_slice_stops_before_the_use_outcome_sweep`
  (`crates/daemon/src/memory_reviewer/lifecycle.rs:928`);
  `a_cited_use_outcome_kept_by_the_sweep_is_not_progress`
  (`crates/daemon/src/memory_reviewer/lifecycle.rs:859`). Status: unaudited.

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
