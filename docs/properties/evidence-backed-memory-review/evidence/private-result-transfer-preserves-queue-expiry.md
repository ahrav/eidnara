# private-result-transfer-preserves-queue-expiry

## Discovery trigger

Decision record Q27: preserve the original job queue deadline for unselected
results; Kernel result persistence and hold transfer precede the Memory Store's
fenced completion; only the completed receipt selects publication; a result
validly selected before queue expiry remains resolvable afterward only within
its live review hold and current policy. Specification U1 acceptance: a crash
between private Kernel result or hold transfer and receipt selection preserves
Q27 unselected expiry.

## Evidence trail

`crates/kernel/src/memory_reviewer_hold.rs` - `transfer_execution_to_review` no longer
updates `candidates.lease_expires_at` or `extraction_runs.lease_expires_at`;
`list_active_review_holds` and `MemoryReviewerHoldBinding::from_owner_id` expose live
review holds to the reconciler.

`crates/kernel/src/review_staging.rs` - `load_staged_review` takes the instant
the row's deadline must be ahead of: `read_selected_review_input` passes
`selected_at`; `read_review_input` and `staged_review_binding` pass the later of
the caller's and the store's clocks.

`crates/memory-store/src/memory_reviewer_ledger.rs` - completion requires
`queue_deadline_ms > now`; `MemoryReviewerReceipt::completed_at_ms` exposes the
completion instant, which no later write changes;
`memory_reviewer_result_is_selected` and `in_progress_memory_reviewer_receipts` answer the
reconciler.

`crates/memory-store/src/memory_reviewer_jobs.rs` - `expire_memory_reviewer_work` closes an
in-progress receipt at the earlier of `run_deadline_ms` and the job's
`queue_deadline_ms`.

`crates/daemon/src/memory_reviewer/settlement.rs` - `read_selected_proposal_inner`
reads through `read_selected_review_input` with the receipt's completion time
and maps a row `Expired` under that clock to `selection_mismatch`.

`crates/daemon/src/memory_reviewer/lifecycle.rs` - `reconcile_review_holds` runs each
pass after the Memory Store sweep and before Kernel maintenance.

Tests named in the catalog record.

## Failure scenario

The worker commits the Kernel result and the hold transfer, then dies before
the Memory Store completion. The previous transfer moved the row's deadline to
the seven-day review expiry, so the unselected row stayed readable by identity
for seven days and its hold retained evidence for as long, while the job's
24-hour queue had lapsed.

## Timing windows and dependencies

Between the Kernel envelope commit and the Memory Store completion; between a
sweep at the deadline and a completion racing it; a run deadline later than the
queue deadline when the job was claimed in its last two minutes.

## What a test must construct

A sealed proposal row and a transferred hold with the receipt still in
progress; a read by identity before and after the queue deadline; the sweep at
the earlier deadline; a public read and list after the sweep; the reconciler
pass; a late completion under the original claim. Separately, a selected result
read after the queue deadline with a live hold and at the hold's expiry.

## Investigation log

### Q: Does the selected read need the row deadline at all once the receipt is complete?

- Sources examined: `memory_reviewer_ledger.rs` completion statement,
  `review_staging.rs` `load_staged_review`.
- Findings: completion is fenced on the job's live queue, so a complete
  selected receipt already proves selection before the deadline. The row check
  against `selected_at` is defense in depth against a receipt row whose
  completion time disagrees with the row it selects.
- Missing evidence: none.
- Conclusion: resolved with answer; keep the check, map its refusal to
  `selection_mismatch`.

### Q: Should the capture `retain_until` promotion also stay at the queue deadline?

- Sources examined: `memory_reviewer_hold.rs` transfer envelope, decision record Q3.
- Findings: `retain_until` is the finite acquisition reference for
  MemoryReviewer-captured evidence, a retention floor separate from the row deadline
  and the hold. Moving it forward at transfer does not make an unselected row
  readable; the reconciler releases the hold, and capture expiry follows its own
  sweep.
- Missing evidence: an owner statement that the floor may outlive the queue
  for unselected results.
- Conclusion: needs human input; left unchanged.
