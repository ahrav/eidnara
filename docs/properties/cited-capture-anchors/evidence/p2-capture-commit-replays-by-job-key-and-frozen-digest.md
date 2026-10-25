# p2-capture-commit-replays-by-job-key-and-frozen-digest

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoints C6 and AC13. Refinement R7 marks its receipt-conflict marker test-only.

## Evidence trail

`publish` (`crates/daemon/src/memory_capture.rs:980`) reads the frozen plan through `prepare_memory_capture` (`crates/memory-store/src/memory_capture.rs:373`), which returns the stored `prepared_json` once frozen. `publish_captured_memories` (`crates/daemon/src/memory_capture.rs:1176`) builds the intent with producer `memory_capture`, the project's operation key for the job id (`:1188`), and the SHA-256 of the frozen plan (`:1191`). The envelope replays a receipt with an equal digest and returns `Conflict` for a different one (`crates/kernel/tests/kernel_envelope.rs:160`).

## Failure scenario

A digest computed over a re-serialized plan rather than the frozen bytes would let two different plans share one receipt.

## Timing windows and dependencies

The reply of a successful commit is lost and the job is retried.

## What a test must construct

A publish, then a second publish of the same job; a frozen plan edited under the same key in a test store.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/memory_capture.rs:1967`; `crates/kernel/tests/kernel_envelope.rs:160`; `crates/daemon/src/lib.rs:24489`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Did the daemon test move?

- Sources examined: `crates/daemon/src/lib.rs` at `a0e3f2993` and at the branch tip.
- Findings: `automatic_capture_commits_once_and_is_explicitly_retrievable` moved from line 24458 to 24489.
- Missing evidence: none.
- Conclusion: resolved with answer.
