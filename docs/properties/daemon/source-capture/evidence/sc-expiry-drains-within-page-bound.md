# sc-expiry-drains-within-page-bound

## Discovery trigger

Part B obligation from #1066 comment 2, decision D6. The Part A record
`sc-capture-expiry-class-scoped-paged-citation-safe` states the across-call
liveness half for `curator_capture` and links it here. The U4 ticket (#1084)
names the record with fault F14 (more than 64 expired captures with one cited
row) and the AC11 clause that 65 expired captures drain in two calls.

## Evidence trail

`crates/kernel/src/local_file.rs` - `MAX_EXPIRED_CAPTURES_PER_CALL = 64` is
shared by both sweeps. `expired_captures` binds it as the `LIMIT` of
`expired_captures_sql`, which orders by `e.retain_until, e.evidence_id`, so
the page is the oldest deadlines first with a total tie order. The query's
`WHERE` clause filters before the limit: pinned rows fail
`EXPIRED_UNPINNED_SQL`, rows whose registry object is not a live evidence
object fail the registry `EXISTS`, and a retained row (no live owner
descriptor and cited elsewhere) fails the last disjunct. None of them takes a
page slot.

`expire_captures` counts `retired` and `retained` from each commit's receipt
result, skips a replayed receipt without counting it, and skips
`NotFound`, `Conflict`, and `InvalidInput` from one capture's commit so the
rest of the page proceeds. Any other error returns and leaves earlier
retirements committed.

`crates/daemon/src/memory_reviewer/lifecycle.rs` - `kernel_slice` calls
`expire_local_file_captures`, then `expire_harness_captures`, stores the
result in `harness_expiry`, and runs `run_staging_maintenance` after both, so
the artifact walk sees the retired references. A call that retires or retains
anything counts as `advanced`. `crates/daemon/src/source_capture.rs` -
`SourceCapture::record_expiry` feeds `expiry_retired` and `expiry_retained`
into the capture report.

Landed checks:
`crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
publishes 65 OpenCode messages at `NOW`, cites the first in page order, and
after the hold lapses asserts the first call returns `(63, 1)`, the second
`(1, 0)`, every uncited capture's evidence is retired, and a third call at
the same cutoff returns the default. Expiry receipts are keyed by cutoff, so
the test retires the kept capture at `after_hold + 1` once the citer is
retired.
`crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress`
asserts one slice at the deadline retires one row and the capture report
shows `expiry_retired = 1`.

## Failure scenario

A page without `ORDER BY retain_until` that refills with cited or held rows
retires nothing while newer expired captures wait. A refusal that aborts the
whole call stalls every row behind it. Either way the store grows past its
retention bound.

## Timing windows and dependencies

The bound is in calls, not time: after `ceil(N / 64)` calls with no new
expiries, N expired, unpinned, uncited captures are retired. The bound
assumes each candidate's commit either retires, retains, or fails with a
transient refusal. A row whose commit refused persistently while it still
passed the candidate query would hold a page slot on every call; the
candidate predicate excludes the registry cases the code comment names, and
no test constructs a persistent refusal. Read work per call grows with the
retained and held rows the query scans before filling its page; the
`expired_captures` doc comment states this, and the bound covers writes.

## What a test must construct

More than 64 expired captures, one cited among them, and a live hold over all
of them first. Stop new expiries, call the sweep, and assert after
`ceil(N / 64)` calls that zero live expired, uncited, unpinned
`harness_capture` evidence remains, with per-call `retired + retained <= 64`.
The legal witness is 65 expired captures draining in two calls.

## Investigation log

### Q: Descriptor and receipt rows outlive retirement (OQ6)
- Sources examined: `ticket-1084.md`; spec C13 and OQ6; `retire_slice_object`
  and `retire_evidence_inner`, which invalidate rows rather than delete them.
- Findings: retention bounds bytes, not rows.
- Missing evidence: the capacity owner's row allowance per store and the
  admission behavior at it.
- Conclusion: needs human input.

### Q: Can a persistently refused candidate starve the page?
- Sources examined: `expire_captures` error arm; `expired_captures_sql`;
  `retire_expired_capture`.
- Findings: refused rows are skipped within a call and stay candidates on the
  next call; the candidate predicate mirrors the recheck and the retirement
  preconditions, so no persistent refusal was found.
- Missing evidence: a test with 64 rows whose commits refuse at the head of
  the order.
- Conclusion: unresolved, needs a test that forces a persistent per-row
  refusal.
