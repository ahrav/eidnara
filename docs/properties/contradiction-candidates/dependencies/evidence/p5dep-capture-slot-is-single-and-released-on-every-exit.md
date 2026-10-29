# p5dep-capture-slot-is-single-and-released-on-every-exit

HEAD `3a3b916b7`. Surface S4.

## Discovery trigger

Plan KTD2: "The publication slot's 5 s budget protects response latency; this
pass must not share it." The task names the claim "one slot". A P5 trigger
chained to the job must not extend the slot's occupancy. Lens: resource
boundaries; C9.

## Evidence trail

- `source_capture.rs:235-237`: "The daemon's capture supervisor: one
  publication slot for its one kernel store"; field `slot: Arc<Semaphore>`.
- `source_capture.rs:269`: `slot: Arc::new(Semaphore::new(1))`.
- `source_capture.rs:433-444` `ActiveSlot { capture, _permit:
  OwnedSemaphorePermit }` with `Drop` that subtracts 1 from `Count::Active`;
  doc: "Counts the slot as active while the permit lives, so a panicking job
  releases both."
- `source_capture.rs:499-504` `is_idle()` (test-support):
  `available_permits() == 1`.
- `source_capture.rs:722-765` `into_job`: `try_acquire_owned()` else
  `refuse(RefusedQueueFull)` and `return None`; on success `Active += 1` and
  the permit moves into `CaptureJob.slot`.
- `source_capture.rs:769-786` `CaptureJob` owns `slot: ActiveSlot` after
  `_retained: RetainedCharge`, the window, documents.
- `source_capture.rs:801-825` `run_settled`: `spawn_blocking(move || self.publish_settled())`
  moves the job (and the permit) into the blocking closure; "A failed join
  dropped the slot, the charges, and the window while the closure unwound"
  (`:679`).
- `source_capture.rs:813-879`: the commit gate is a separate `Mutex<()>`;
  `catch_unwind` around `publish_locked` so a panic does not poison the gate;
  `resume_unwind` after `drop(gate)`, which then unwinds through `publish`
  and drops the job.
- Tests: `source_capture_tests.rs:675`
  `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`;
  `source_capture.rs:1797` `a_settled_report_counts_the_job_that_released_the_slot`
  (acquires the slot directly at `:1799`).

## Failure scenario

A leaked permit (for example a job future dropped without running its
closure while holding the permit outside the closure) would make every later
`into_job` return `RefusedQueueFull` for the daemon's lifetime; capture would
silently stop. If a P5 pass were placed inside `publish` (before the closure
returns), its runtime would extend slot occupancy and the next pass's handoff
would be refused more often.

## Timing windows and dependencies

Slot held from `into_job` to the end of the blocking closure (or its unwind).
The commit gate is held only around `publish_locked`.

## What a test must construct

- Two handoffs in quick succession with the first job blocked (test hook
  `before_publish`); assert the second counts `RefusedQueueFull` and
  `available_permits() == 0` during, `== 1` after.
- A panic injected via `between_screen_and_commit`; assert
  `WorkerPanicked` counted, `Active == 0`, `is_idle()`.
- Shutdown cancel mid-job; assert the join completes and the slot is free.

## Investigation log

### Q: Can the `Active` gauge and the semaphore disagree?

- Sources examined: `:420-431`, `:606`, `:1588-1610`.
- Findings: `Active` is incremented at `:606` after the permit is acquired
  and decremented in `ActiveSlot::drop`; the test at `:1588` checks a report
  taken while the memo lock is held after release. A window exists between
  `try_acquire_owned` success and `Active += 1` where the semaphore is taken
  and the gauge is not; it is a few instructions on the request path.
- Missing evidence: none needed; the gauge is observability, not the bound.
- Conclusion: resolved; the semaphore is the bound, the gauge may lag by one
  for a negligible window.
