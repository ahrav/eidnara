# uo-p-record-survives-session-delete

## Discovery trigger

Spec #1145 KTD1; activated here with its Kernel-side check (deviation confirmed
at #1154 ticketing).

Record kind: new obligation activated by #1154.

## Evidence trail

References are verified at the #1156 branch tip, the commit that carries this
revision.

- Delete triggers: `crates/kernel/src/schema.rs:188`.
- Retained read: `served_set` (`crates/kernel/src/use_outcome.rs:441`) answers
  retired records.

- Guarantee under test: After `session.delete` answers `ok`, every serving
  record and decision row of the session that was readable before it is
  readable after it, with the same ids. A serve whose write lands beside the
  delete may add a record, for a served set the session has no record of, and
  removes none (OQ3-10).
- Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
  (`crates/kernel/tests/kernel_schema.rs:2177`);
  `disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
  (`crates/kernel/tests/kernel_use_outcome.rs:400`) (a retired record stays
  readable);
  `a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`);
  `a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry`
  (`crates/daemon/src/use_outcome_lifecycle_tests.rs:86`). Status: unaudited.

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

### Q: Can the record count after a delete differ from the count before?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: Yes, upward. The serve write takes no capture gate, so a pass that
  committed before the delete can insert its record while the delete runs or
  after its Kernel step (OQ3-10, an accepted loss, gap G2). No path removes a
  row: the DDL refuses deletes and the step sets only `deleted_at`. The
  guarantee is preservation of the rows read before the delete; the cited tests
  compare before and after with no serve in flight.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - the guarantee names preservation, not
  count equality.
