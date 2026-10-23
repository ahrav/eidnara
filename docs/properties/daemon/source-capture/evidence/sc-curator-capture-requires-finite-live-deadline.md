# sc-curator-capture-requires-finite-live-deadline

## Discovery trigger

D6 gives harness captures a finite deadline modelled on the MemoryReviewer
`curator_capture` class. Part A recorded the existing ingest rule for that
class so U4 could mirror it, and flagged that liveness reads the wall clock.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/cas/mod.rs`, `MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS`
  is `"curator_capture"`; `HARNESS_CAPTURE_RETENTION_CLASS` is
  `"harness_capture"`. The first doc comment states a request must carry a
  finite `retain_until`, a new reference commits only while the deadline is
  ahead of the store clock, and an identical committed request replays.
- `crates/kernel/src/cas/ingest.rs`, request validation in the ingest entry
  (the block that returns `ArtifactErrorKind::InvalidInput`): a
  `curator_capture` or `harness_capture` request with `retain_until: None`
  is refused before staging, as is a negative deadline.
- `crates/kernel/src/cas/ingest.rs`, `insert_reference`: after the
  re-admission check, a `curator_capture` request whose `retain_until` is
  not greater than `crate::current_time_ms()` returns
  `KernelError::InvalidInput`. The comment states a replayed receipt never
  reaches this point. The live-deadline check names only
  `MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS`.

Tests read at HEAD:

- `crates/kernel/tests/kernel_memory_reviewer_holds.rs`,
  `memory_reviewer_captures_must_carry_a_finite_acquisition_reference`:
  `None` is `InvalidInput`; `Some(now_ms() - 1)` is `InvalidInput` with the
  message "an already-dead reference is refused"; `now_ms() + HOUR_MS`
  ingests.
- `crates/kernel/tests/kernel_memory_reviewer_holds.rs`,
  `a_committed_capture_replays_after_its_retention_deadline`: ingests with
  `now_ms() + 200`, sleeps 300 ms, and the identical request returns the
  same handle.
- `crates/kernel/src/local_file.rs` unit test
  `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`: an
  undated `harness_capture` ingest is `InvalidInput` with `tip()` unchanged;
  the same test ingests a `harness_capture` row with
  `retain_until: Some(NOW)` where `NOW` is 10 000 ms after the epoch, which
  succeeds.

## Failure scenario

A `curator_capture` row with no deadline, or with a deadline already past,
gains a live reference and never enters the expiry sweep's candidate set, or
an identical committed request is refused after its deadline and the run
loses its handle.

## Timing windows and dependencies

The live-deadline check runs inside the reference commit against the wall
clock, with no injected clock. The replay test therefore depends on a
200 ms window; PR #1081 and PR #1084 record it failing under host load and
passing alone.

## What a test must construct

F11: `None`, `now - 1`, and a committed request replayed after its deadline,
each for `curator_capture`. A clock seam would remove the sleep.

## Investigation log

### Q: Is the replay-after-deadline arm tested?
- Sources examined: `git grep` at baseline `08de9051a` and HEAD.
- Findings: `a_committed_capture_replays_after_its_retention_deadline`
  exists at both, at line 1793. The source's "untested" note was wrong.
- Missing evidence: none.
- Conclusion: resolved with answer; `Exercised` is `yes`.

### Q: Does `harness_capture` mirror both halves?
- Sources examined: `insert_reference`; the local_file unit test above.
- Findings: `harness_capture` shares the finite-deadline refusal only. A
  past deadline commits, and the expiry sweep then treats the row as
  already due.
- Missing evidence: a statement of intent in ticket-1084 or pr-1084; neither
  mentions the live-deadline half.
- Conclusion: resolved with answer by code reading; recorded for the lead as
  a contract-versus-code note against the spec's "U4 mirrors this".

### Q: Should the deadline check use an injected clock?
- Sources examined: spec.md Part A Impact; ticket-1084 (seam uses the
  maintenance entry's `now`, not ingest).
- Findings: ingest still reads `current_time_ms()`.
- Missing evidence: a test-strategy decision.
- Conclusion: unresolved, needs a `/testing:test-strategy` decision on an
  ingest clock seam.
