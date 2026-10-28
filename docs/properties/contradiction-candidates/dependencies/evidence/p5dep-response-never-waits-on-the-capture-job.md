# p5dep-response-never-waits-on-the-capture-job

HEAD `3a3b916b7`. Surface S4.

## Discovery trigger

Plan R1 and KTD2: the P5 pass "never delays the transform response or the
publish" and must not share the publication slot's 5 s budget. The module
header at `crates/daemon/src/source_capture.rs:3` claims "The response bytes
never wait on the kernel or the job." Lens: concurrency; C9.

## Evidence trail

- `source_capture.rs:3`: the handler offers records to `SourceCapture::offer`
  once the pass has built its response; "When the response is handed to the
  wire writer, `CaptureHandoff::into_job` takes the store's single publication
  slot"; the 5 s budget's origin is "the handoff's acceptance".
- `crates/daemon/src/lib.rs:9220-9237` `capture_after_response(offer)`:
  `self.source_capture.offer(offer)?` runs synchronously in the handler
  (this is the charge of the retained window to the supervisor's byte budget,
  not a kernel or job wait); returns `AfterResponse::new(move || { if let
  Some(job) = handoff.into_job(kernel, gate, cancel) &&
  admission.spawn(job.run()).is_none() { capture.refuse(RefusedShuttingDown)
  } })`.
- `crates/daemon/src/dispatch.rs:24-35` `AfterResponse`: "Work deferred until
  a response's bytes are encoded and handed to the wire writer. Dropping it
  unrun is the outcome for a response that never settled."
- `dispatch.rs:445-460`: `after_response` attaches only to
  `PreparedOutcome::Response`; `take_after_response` detaches it.
- `lib.rs:13490-13502`: `dispatch_body` -> `take_after_response()` ->
  charges -> `settle_prepared(&ctx, outcome).await` -> `if let
  (RequestOutcome::Response { .. }, Some(after)) = (&settled, after_response)
  { after.run(); }`. Comment at `:13478`: "The response bytes are encoded and
  about to be handed to the wire writer; work that waits for that point
  starts now and never delays them."
- `source_capture.rs:608-617` `into_job`: `try_acquire_owned()` (non-blocking)
  or `RefusedQueueFull`.
- `source_capture.rs:665-691` `run`: `spawn_blocking(self.publish())`; the
  handler's task is not awaited by the request path (spawned through
  `admission.spawn`).
- Tests: `source_capture_tests.rs:330`
  `the_response_is_identical_whatever_capture_and_the_kernel_are_doing`;
  `:938` `capture_latency_measurement`; `:298`
  `capture_off_memory_off_or_a_subagent_session_runs_no_adapter`.
- Reachability: `crates/daemon/src/config.rs:154` `source_capture_enabled:
  false` by default; `:2268` JSON default; the module header's gate also
  requires `memory.enabled` and a non-subagent session.

## Failure scenario

If `after.run()` moved before `settle_prepared`, or if `into_job` awaited the
semaphore, a kernel that is starting or a job holding the slot would delay
the response by up to the acquisition wait. If a P5 trigger were inserted
inside the `AfterResponse` closure and did synchronous work before
`job.run()` was spawned, that work would run on the request path after the
bytes are handed off but before the handler returns; the response bytes would
not wait, but the connection's next request might.

## Timing windows and dependencies

`offer` runs before the response settles and charges the window; a refusal
there costs nothing. `after.run()` runs after `settle_prepared` returns and
before the handler returns `settled`.

## What a test must construct

- Capture on; kernel held in `Starting` or `Unavailable`; a prior job holding
  the slot; measure response bytes and latency against capture off. The
  existing identity test does the byte comparison; the latency test measures
  the pass.
- A probe recording the order of `settle_prepared` return and `after.run()`.

## Investigation log

### Q: Does `offer` itself ever block on the kernel?

- Sources examined: `source_capture.rs:445-496` `offer`.
- Findings: `offer` charges bytes against the supervisor's `ByteBudget` and
  builds the handoff; it reads no store and takes no slot.
- Missing evidence: none.
- Conclusion: resolved; `offer` is in-memory accounting.
