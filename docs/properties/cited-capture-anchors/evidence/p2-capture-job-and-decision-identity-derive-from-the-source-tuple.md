# p2-capture-job-and-decision-identity-derive-from-the-source-tuple

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoints C6 and D6. The U3 anchor id derives from the job id, so its stability matters.

## Evidence trail

`job_id` (`crates/memory-store/src/memory_capture.rs:136`) hashes a domain tag and the length-prefixed project, harness, session id, message id, role, and redacted text. `native_capture_fragments` (`crates/daemon/src/memory_capture.rs:1325`) replaces a fragment id with `{sha256}:{offset}` when the message exceeds 16 KiB or the id is empty, over 256 bytes, or holds control characters (`:1353`). The object id of each decision a job's extraction writes is `mem_` plus the first 32 hex characters of SHA-256 over `job_id:index` (`:4629`). A contested, settled, or ended statement's revision takes its id from the statement it supersedes and the contesting fact, the settling job, or the ending fact (`crates/daemon/src/memory_capture.rs:4117`).

## Failure scenario

A job id that hashed raw text would let a stored digest confirm a guessed secret; an id that hashed a timestamp would duplicate memories on retry.

## Timing windows and dependencies

None.

## What a test must construct

The same source enqueued twice; a secret-bearing source; an out-of-grammar id; a retried publish asserting the same object ids; a statement that ends work in progress, asserting the ended revision's id.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/memory-store/src/memory_capture.rs:882`; `crates/daemon/src/memory_capture.rs:11145`, `:11194`, `:12324`, `:12302`, `:12594`; `crates/daemon/src/lib.rs:24958`, `:26418`, read at the branch that adds this part (base `3a3b916b7`); `crates/daemon/src/memory_capture.rs:11695`, the test of an ended revision's id, read at the branch that adds it.
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
