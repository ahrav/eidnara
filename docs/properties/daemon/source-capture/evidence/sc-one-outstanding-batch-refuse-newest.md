# sc-one-outstanding-batch-refuse-newest

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R14, decision D3, and
constraints C1 to C4. The U2 ticket (#1082) adds the slot, the resident
charges, the F22 test hook, and AC8's last clause.

## Evidence trail

`crates/daemon/src/source_capture.rs`:

- `SourceCapture::default` builds `slot: Arc::new(Semaphore::new(1))`, one
  per daemon, which serves one Kernel store.
- `SourceCapture::offer` charges the window against `budget`
  (`MAX_RETAINED_WINDOW_BYTES`, 32 MiB) through `RetainedCharge::take`, and
  refuses `refused_window_too_large` or `refused_resident_short` with no
  handoff.
- `CaptureHandoff::into_job` calls `try_acquire_owned` on the slot. On
  failure it counts `refused_queue_full` through `SourceCapture::refuse` and
  returns `None`, which drops the handoff, its `Arc` window references, and
  its `RetainedCharge`.
- The accepted `CaptureJob` owns `ActiveSlot` (the permit) and the
  `RetainedCharge`; `CaptureJob::run` moves the job into `spawn_blocking`,
  so both drop only when `publish` physically returns or unwinds. On
  cancellation, `run` sets the budget interrupt and still awaits the join.
- `RetainedCharge::drop` and `ActiveSlot::drop` reverse the
  `retained_bytes` and `active` gauges.

`crates/daemon/src/harness_sources.rs` - `publish_screened` admits at most
`MAX_SOURCE_BATCH_UNITS` (32, from `memory_capture::MAX_CAPTURE_MESSAGES`)
units and `MAX_SOURCE_BATCH_BYTES` (64 KiB, from
`memory_capture::MAX_CAPTURE_INPUT_BYTES`) of text; the rest are `Deferred`.

`CaptureJob::publish_locked` runs the window's batch, then
`capture_documents` runs the documents' own `publish_screened` batch in the
same job. The two batches run serially under one slot.

## Failure scenario

The slot is released when the async task is cancelled while the blocking
thread still holds the batch, so a second batch starts and resident bytes grow
without bound. Or a refused handoff keeps its window reference.

## Timing windows and dependencies

The charge is taken when the pass's response is built
(`capture_after_response`) and the slot when the response settles. Between
those points a refused handoff holds its charge and its `Arc` references; the
charge drops at refusal. Nothing is copied before the slot is held: the
adapters run inside the job. A window refused at a full slot is lost unless a
later pass carries it (C7).

## What a test must construct

A capture handler whose `before_publish` hook blocks the first job inside
publication, then a second pass while the first holds the slot. Assert
`refused_queue_full == 1`, `retained_bytes` unchanged by the second pass,
`active == 1`, and after release `retained_bytes == 0` and `completed == 1`.
A legal witness: a 33-unit window publishes 32 and reports one `deferred`.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `the_response_is_identical_whatever_capture_and_the_kernel_are_doing`, slot
  full section, asserts exactly the above; its comment names the coverage
  witness `handoff_refused_slot_busy`.
- `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`
  asserts 32 live descriptors and `units_deferred == 1`.
- `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
  asserts a panicking job leaves `retained_bytes == 0` and `active == 0`, and
  that shutdown waits for the in-flight batch.
- `a_window_past_the_remaining_budget_is_refused_resident_short` asserts a
  budget refusal retains nothing.
- `crates/daemon/tests/harness_sources.rs`
  `capture_batch_defers_units_beyond_its_bounds` pins 32 units and 64 KiB.

## Investigation log

### Q: Is the slot acquired before any window reference is retained?

- Sources examined: `ticket-1082.md` scope (one outstanding batch),
  `capture_after_response`, `SourceCapture::offer`,
  `CaptureHandoff::into_job`.
- Findings: the ticket says the slot is acquired before any window reference
  is retained past the response. The code charges and holds `Arc` references
  from response construction to settlement, then takes the slot. A refused
  handoff releases both at refusal, so the net retention is zero and no copy
  is made.
- Missing evidence: none; this is a wording difference reported to the lead.
- Conclusion: resolved with answer. The record's check reads "a refused
  handoff retained 0 bytes after refusal".

### Q: Can the task cancel release the slot early?

- Sources examined: `CaptureJob::run`.
- Findings: the permit lives in the job moved into `spawn_blocking`; `run`
  awaits the join on both arms of its `select!`.
- Missing evidence: none.
- Conclusion: resolved with answer.
