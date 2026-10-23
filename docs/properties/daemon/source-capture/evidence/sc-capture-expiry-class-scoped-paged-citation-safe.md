# sc-capture-expiry-class-scoped-paged-citation-safe

## Discovery trigger

The `curator_capture` expiry sweep is the template D6 names for the
`harness_capture` sweep. Part A recorded its paging, pin, citation, and class
rules. At HEAD U4 parameterized the same code by an `ExpiringClass`, so this
record now covers the shared sweep mechanism as reached through
`expire_local_file_captures`; the harness half is carried by
`sc-expiry-descriptor-first-cited-survives` and
`sc-expiry-drains-within-page-bound`.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/local_file.rs`: `MAX_EXPIRED_CAPTURES_PER_CALL = 64`;
  `ExpiringClass` with `MEMORY_REVIEWER_CAPTURES` (`curator_capture`,
  `local_file_capture` owners, producer `local-file-expiry`) and
  `HARNESS_CAPTURES` (`harness_capture`, `source_descriptor` owners,
  producer `harness-capture-expiry`).
- `KernelStore::expire_captures`: reads candidates under the reader lock,
  then commits them in `commit_with_writer` transactions of at most
  `captures_per_commit` captures: 1 for `MEMORY_REVIEWER_CAPTURES`, 64
  (`MAX_EXPIRED_CAPTURES_PER_CALL`) for `HARNESS_CAPTURES`. The transaction
  is keyed by its first capture's `expiry_intent` (reserved producer,
  `"{now}:{evidence_id}"`), and every later capture in it records its own
  receipt under the same key form. `"retired"` and `"retained"` results
  are counted; a replayed receipt is not counted; `NotFound`, `Conflict`,
  and `InvalidInput` roll the transaction back and the rest is committed
  again without the refused capture; any other error returns, keeping
  earlier retirements.
- `retire_expired_capture`: rechecks `EXPIRED_UNPINNED_SQL` under the
  writer and returns `Conflict` when the row is no longer due or is pinned;
  retires the class's own observations through `retire_capture_observation`;
  returns `"retained"` when `cited_elsewhere`; else calls `retire_evidence`.
- `EXPIRED_UNPINNED_SQL`: class equals `?1`, row live, `retain_until` not
  null and `<= now`, and no unreleased, non-purge-degraded, unexpired pin.
- `expired_captures_sql`: adds a live evidence registry object, and either a
  live owning observation or no citation; orders by
  `retain_until, evidence_id`; `LIMIT 64`.
- `crates/daemon/src/memory_reviewer/lifecycle.rs`, `kernel_slice`: runs
  `expire_local_file_captures`, then `expire_harness_captures`, then
  `maintenance_slice` over `run_staging_maintenance`. `lifecycle::run` is
  spawned from `crates/daemon/src/lib.rs` once the Memory Store opens and
  calls the slice while the Kernel is ready.

Tests read at HEAD (`crates/kernel/src/local_file.rs` unit tests):
`a_caller_cannot_seat_a_receipt_the_expiry_would_replay`,
`a_replayed_expiry_receipt_is_not_counted_as_a_retirement`,
`a_capture_pinned_after_it_was_selected_for_expiry_is_kept`,
`a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` (the
`curator_capture` sweep retires its own capture and never the harness row),
`a_purge_degraded_pin_does_not_keep_a_lapsed_capture`,
`expiry_candidate_cost_does_not_grow_with_retired_history`.
`crates/daemon/tests/memory_reviewer_project_text.rs`:
`expiry_retires_the_capture_and_its_detail_but_keeps_independent_support`
(hold blocks; canonical bytes stay readable; cited capture is `retained`
and leaves the sweep; retires once the citation goes),
`retained_captures_do_not_starve_newer_expired_captures` (65 captures, 64
cited, the uncited one retires within two calls),
`a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep` (64 rows
with a non-evidence registry object are not candidates; the capture behind
them retires in one call).

## Failure scenario

A sweep retires a pinned capture, removes evidence another row cites,
touches a `canonical` or `harness_capture` row while sweeping
`curator_capture`, or stalls behind a page of retained or refused rows.

## Timing windows and dependencies

A pin acquired between the reader query and the writer commit is caught by
the recheck (F12). A `curator_capture` page commits one capture at a time,
so a crash mid-page leaves earlier captures retired and later ones
candidates. A `harness_capture` page is one transaction, so a crash
mid-page leaves the whole page as candidates.

## What a test must construct

More than 64 expired captures (F14), a pin taken mid-sweep (F12), evidence
cited by an independent row (F13), and rows of other classes in the same
store. Coverage witnesses `expired_capture_pinned_mid_sweep` and
`expired_capture_cited_elsewhere` mark the F12 and F13 preconditions.

## Investigation log

### Q: Did the parameterization change `curator_capture` behavior?
- Sources examined: pr-1084.md; the tests above at HEAD.
- Findings: the MemoryReviewer constants are unchanged and the pre-existing
  tests pass unchanged per pr-1084; the code path is now shared.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Did the baseline citations move?
- Findings: `local_file.rs` :600 to :805 are now the named unit tests above;
  `memory_reviewer_project_text.rs` :983, :1153, :1226 are unchanged.
  `lifecycle.rs:319, 322` now point into `kernel_slice`, which also calls
  the harness sweep.
- Conclusion: resolved with answer.
