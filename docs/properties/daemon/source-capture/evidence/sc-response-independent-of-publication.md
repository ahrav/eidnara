# sc-response-independent-of-publication

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R2 and decision D1. The
U2 ticket (#1082) resolves OQ18 (handoff after the response reaches the wire
writer) and names AC7, which compares response bytes with capture on and off
across Kernel Ready, Kernel Starting, writer busy, and slot full.

## Evidence trail

`crates/daemon/src/lib.rs`, transform pass - the pass builds a
`source_capture::Offer` from `Arc` clones of the parsed native records before
the pass runs, then attaches `capture_after_response(offer)` to the outcome
with `PreparedOutcome::after_response`. `capture_after_response` charges the
window through `SourceCapture::offer` and returns a
`dispatch::AfterResponse` closure. The closure calls
`CaptureHandoff::into_job`, which uses `try_acquire_owned` and never waits,
then spawns `CaptureJob::run` on the tracked admission.

`crates/daemon/src/lib.rs`, the transport handler - it takes the after-response
work with `take_after_response`, awaits `settle_prepared`, and runs the work
only when the settled outcome is `RequestOutcome::Response`. The comment
there states that the response bytes are encoded and about to be handed to the
wire writer. For a transform response, `settle_direct_with` reserves the
output with a boxed `DirectWrite` serializer
(`output_from_direct_write`), so at that point the response is settled and
its reservation is handed to the wire writer, and host-runtime serializes
the bytes when it emits the frame; the capture job can start before that
serialization. Publication still never delays the response, which is the
record's claim.

`crates/daemon/src/source_capture.rs` - `CaptureJob::run` moves `publish` to
`spawn_blocking`; the Kernel open check (`kernel_store`), the budget
(`EvalBudget` with `CAPTURE_BUDGET` of 5 s), and `publish_screened` all run
inside that closure, after the response.

`crates/daemon/src/dispatch.rs` - `AfterResponse` and
`PreparedOutcome::take_after_response` carry the work.

The baseline citations `lib.rs:1826-1834` (`ready_snapshot` drops
`native_messages`) and `hs.rs:493` moved; the record names functions instead.

## Failure scenario

`publish_batch` runs before the response ticket is accepted, or the handler
awaits Kernel readiness. A Kernel stall, a held writer, or a 5 s budget then
becomes a transform stall, or the response differs when the Kernel is
Starting.

## Timing windows and dependencies

The window between response construction and wire handoff holds the window
charge and the `Arc` references; it holds no slot and makes no copy. Kernel
phase, writer contention, the slot, and the budget are all read inside the
background job. A disconnect before settlement drops the after-response work
unrun.

## What a test must construct

A capture-disabled baseline handler and a capture-enabled handler serving the
same window, compared byte for byte after removing `timings` and the output
revisions, in each Kernel state: Ready, Starting (no Kernel opened), writer
held by another commit on a separate thread, and slot full (the
`before_publish` hook holds the first batch). Each comparison asserts the
response returns within `CAPTURE_TEST_RESPONSE_BOUND` (3 s) when the writer
is held.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `the_response_is_identical_whatever_capture_and_the_kernel_are_doing` builds
  all four states above and asserts equal responses; the Starting case also
  asserts `kernel_unavailable == 1`.
- `a_spent_budget_before_the_writer_commits_nothing` sleeps past
  `CAPTURE_BUDGET` in the hook and asserts the pass returned within the bound
  and `deadline == 1`.
- `document_capture_leaves_the_response_and_respects_inject_docs` asserts
  equal responses with document capture on and off.
- `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` (U3)
  asserts the Pi response is equal whether rows carry entry identity.
- `crates/daemon/src/transform_unit/host_tests.rs`
  `the_host_starts_source_capture_after_the_response_settles` drives the
  transport's own `handle` and observes publication after the response.
- `capture_latency_measurement` (ignored, measurement) records the OQ19
  latency distributions.

## Investigation log

### Q: Does the response path await anything from capture?

- Sources examined: `capture_after_response`, the transport handler's
  `settle_prepared` call, `CaptureHandoff::into_job`, `TaskAdmission::spawn`.
- Findings: `offer` is a synchronous budget charge, `into_job` a non-blocking
  `try_acquire_owned`, and the job is spawned, never awaited, after the
  response settled.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What latency-equality threshold applies to AC7 (OQ19)?

- Sources examined: `spec.md` OQ19, `ticket-1082.md`, `PROGRESS.md` U2
  measurement notes.
- Findings: U2 reports medians and spread over 10 interleaved runs per
  workload and flags a repeatable p95 regression above 5 percent. Whether
  that blocks merge is the performance owner's decision.
- Missing evidence: the owner's threshold.
- Conclusion: needs human input.

### Q: Is the `Deadline` state compared byte for byte?

- Sources examined: `a_spent_budget_before_the_writer_commits_nothing`.
- Findings: the test asserts the response time bound and the `deadline`
  counter, and does not compare the response with a capture-off baseline.
- Missing evidence: a byte comparison under `Deadline`.
- Conclusion: unresolved, needs a baseline comparison in the deadline test.
