# p5dep-capture-job-ends-in-one-closed-vocabulary-count

HEAD `3a3b916b7`. Surface S4.

## Discovery trigger

Plan U2 step 1: "On publish settlement with the flag on, enqueue one bounded
pass." The task asks where "publish settlement" is observable: the job's
terminal `Count` is the only completion signal and no post-publish hook
exists. The header at `source_capture.rs:7` claims a closed vocabulary with
no unit text. Lenses: lifecycle transitions; protocol contracts; C10.

## Evidence trail

- `source_capture.rs:64-82` `counts!` macro generates `enum Count`,
  `Count::ALL`, and `name()`.
- `source_capture.rs:86-130` the list: `admitted`, `refused_queue_full`,
  `refused_resident_short`, `refused_window_too_large`,
  `refused_shutting_down`, `completed`, `discarded`, `session_invalidated`,
  `kernel_unavailable`, `deadline`, `kernel_error`, `worker_panicked`,
  `adapter_refused`, `units_*`, `active`, `retained_bytes`, `publish_ms_*`,
  `documents_*`, `expiry_retired`, `expiry_retained`.
- `source_capture.rs:800-824` `run_settled() -> (Count, Option<SettledPublication>)`:
  join -> `Ok(settled) => return settled` (already counted inside `publish`
  via `end()`); `Err(panic) => WorkerPanicked`, `Err(_) => Discarded`, each
  counted once at `:820` and logged.
- `source_capture.rs:873-881` `end(&self, end) -> Count`: `counters.add(end,
  1)`; logs unless `Completed`.
- `source_capture.rs:827-871` `publish_settled`: calls `publish` once and
  hands its `Count` out with the kept texts.
- `source_capture.rs:884-1006` `publish`: every exit is `return self.end(..)`
  (`Discarded` `:886`, `KernelUnavailable` `:889`, `Deadline` `:892`,
  `RefusedResidentShort` `:900`, `:930`, `Completed` `:903`, `:977`,
  `kernel_end(error)` `:907`, `Discarded` `:982`) or the result of
  `publish_locked` returned through `catch_unwind` (`:989-1005`).
- `source_capture.rs:1009-1094` `publish_locked`: `SessionInvalidated`
  `:1019`, `kernel_end` `:1022`, `Deadline` `:1025`, the document-only end
  `:1029`. Each path returns one `Count` that `publish` passes to `end()`
  once.
- `source_capture.rs:1660-1665` `run_hook` and the `cfg(test)` hooks at
  `:980` (`before_publish`), `:1040` (`between_screen_and_commit`), `:1154`,
  `:1239`, `:1270`. No non-test hook after publish.
- `lib.rs:9303-9320`: the `AfterResponse` task awaits `job.run_settled()` and
  discards its `Count`; `admission.try_spawn(task)` refused counts
  `RefusedShuttingDown` through `refuse_handoff`.
- `source_capture.rs:666-669` `record_expiry` adds the sweep's counts, so
  the vocabulary also carries S6 progress.
- Tests: `source_capture_tests.rs:807`
  `the_capture_report_carries_no_unit_text`; `:861`
  `the_capture_report_reaches_host_status_field_for_field`.

## Failure scenario

A path that returned without `end()` (or called it twice) would make the
counters disagree with the number of jobs, and a P5 trigger keyed on
`Completed` would fire zero or two times for one publish. A hook added after
publish that received unit text or object ids would widen the closed
vocabulary; the header's privacy promise (no unit text, paths, identities,
digests in logs/counters) would then depend on the hook's discipline.

## Timing windows and dependencies

`run` returns after the blocking closure physically completes (`:653`
doc). A trigger placed after `job.run().await` inside the spawned task sees
the terminal `Count` and the kernel commit already durable.

## What a test must construct

- Force each exit (cancel before start, kernel unavailable, budget spent,
  delete epoch moved, refusal in the commit, panic via hook) and assert the
  terminal counters' total rose by exactly 1 per job and the returned
  `Count` matches the incremented counter.
- Assert every log line's reason string is in `Count::ALL`.

## Investigation log

### Q: Where would a P5 trigger attach?

- Sources examined: `lib.rs:9229-9236`, `source_capture.rs:665-691`,
  `publish_locked` return values.
- Findings: `Published` values (object ids, `evidence_object_id`) are
  consumed inside `publish_locked` to classify dispositions (`:852-858`) and
  are not returned from `run`. A trigger needing descriptor ids would have to
  be threaded out of `publish_locked` or re-read from the Kernel by
  `observed_at`/scope.
- Missing evidence: none; this is a design input for the P5 domain model.
- Conclusion: resolved as fact; the design belongs to the other pass.
