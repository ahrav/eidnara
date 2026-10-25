# p2-capture-job-and-decision-identity-derive-from-the-source-tuple

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoints C6 and D6. The U3 anchor id derives from the job id, so its stability matters.

## Evidence trail

`job_id` (`crates/memory-store/src/memory_capture.rs:112`) hashes a domain tag and the length-prefixed project, harness, session id, message id, role, and redacted text. `native_capture_fragments` (`crates/daemon/src/memory_capture.rs:318`) replaces a fragment id with `{sha256}:{offset}` when the message exceeds 16 KiB or the id is empty, over 256 bytes, or holds control characters (`:346`). Each written decision's object id is `mem_` plus the first 32 hex characters of SHA-256 over `job_id:index` (`:1241`).

## Failure scenario

A job id that hashed raw text would let a stored digest confirm a guessed secret; an id that hashed a timestamp would duplicate memories on retry.

## Timing windows and dependencies

None.

## What a test must construct

The same source enqueued twice; a secret-bearing source; an out-of-grammar id; a retried publish asserting the same object ids.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/memory-store/src/memory_capture.rs:755`; `crates/daemon/src/memory_capture.rs:1499`, `:1548`, `:1853`, `:1875`, `:1969`; `crates/daemon/src/lib.rs:24489`, `:25800`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
