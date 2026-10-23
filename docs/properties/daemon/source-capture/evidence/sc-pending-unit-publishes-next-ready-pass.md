# sc-pending-unit-publishes-next-ready-pass

## Discovery trigger

Part B liveness obligation from #1066 comment 2, requirement R1, constraints
C1 and C7, and the U2 retry policy. The U2 ticket (#1082) states the bound: a
pending unit with unchanged input reaches `publish_batch` or a terminal
disposition on the first later pass with the Kernel Ready and the slot free.

## Evidence trail

`crates/daemon/src/source_capture.rs`:

- `CaptureJob::publish` ends `kernel_unavailable` when `kernel_store()`
  fails, before any memo write. Units of that pass stay unknown to the memo.
- Candidate selection orders memo `Pending` units first, then units the memo
  does not know; only an equal-digest `Published` or `Terminal` entry is
  skipped.
- `classify` records `Deferred` and retryable refusals as
  `Disposition::Pending { since_ms }`; `Memo::record` keeps the first
  `since_ms`, which `report` exposes as `oldest_deferred_age_ms`.
- `publish_locked`, error arm: siblings of a conflicting unit are not
  recorded, so they remain candidates.
- A `session_invalidated`, `deadline`, `kernel_error`, or
  `refused_queue_full` end records nothing, so every unit of that window
  stays a candidate for the next pass that carries it.
- Module doc: a window refused at a full slot and never offered again is lost
  to capture (C7).

`crates/daemon/src/harness_sources.rs` - `publish_screened` admits in order up
to 32 units and 64 KiB; replayed units count toward the bound, which is why
pending units are offered first.

## Failure scenario

The pending set is dropped with the window reference on a refused handoff, so
a window observed once while the Kernel was Starting is never captured and no
signal shows it.

## Timing windows and dependencies

Pass N meets Kernel `Starting`; pass N+1 meets Kernel Ready and a free slot.
The bound is one pass only when the pending and unknown units ahead of the
unit fit one batch; more than 32 candidates, or more than 64 KiB of candidate
text, defers the excess again, pending units first. A retryable refusal such
as `RevisionAhead` is re-offered on every pass and stays pending until its
condition clears. Units are re-derived from each pass's window, so a unit
reaches the next pass only when that pass's window carries it.

## What a test must construct

A handler with no Kernel opened, one pass, then `KernelOpenCoordinator::open`
and a second pass with the same window. Assert `kernel_unavailable == 1` after
pass N and one live descriptor after pass N+1.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass`
  constructs the above.
- `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`
  asserts the deferred unit publishes on the next pass and
  `oldest_deferred_age_ms` returns to null.
- `a_revision_lead_leaves_the_unit_pending` (F25) asserts a retryable unit is
  offered again (`units_refused_retryable == 2`, `units_memo_skipped == 0`).
- `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window`
  asserts the next pass captures an invalidated window.
- `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
  asserts the next pass after a panic publishes.

## Investigation log

### Q: Pending set per unit or per window?

- Sources examined: `CaptureJob::publish`, `Memo`, the module doc.
- Findings: pending state is per unit, keyed by the memo key (incarnation,
  project, harness, session, receipt key) with its digest. Units not in the
  memo are candidates by default. The window itself is never retained past
  its job.
- Missing evidence: none.
- Conclusion: resolved with answer, in U2. Windows older than the submitted
  head are outside the guarantee, as C7 states.

### Q: Does a slot-full refusal followed by a free pass publish?

- Sources examined: the slot-full section of
  `the_response_is_identical_whatever_capture_and_the_kernel_are_doing`.
- Findings: the test releases the first job and settles, and does not send a
  third pass to observe the refused window's units published.
- Missing evidence: a pass after the refusal.
- Conclusion: unresolved, needs a third pass in the slot-full test.
