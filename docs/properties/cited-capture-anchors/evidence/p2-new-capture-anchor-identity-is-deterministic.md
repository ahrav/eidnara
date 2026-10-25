# p2-new-capture-anchor-identity-is-deterministic

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N2 and constraint C6, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

`anchor_spec` (`crates/daemon/src/memory_capture/cite.rs:340`) returns no anchor for a `HeadUnavailable` plan and otherwise builds `anchor_<32 hex>` and `anchorobj_<32 hex>` from the SHA-256 of `job_id:oid`, with domain `memory`, source `assistant`/`memory_capture`, revision 1, class `Normal`, and a `reachable_from` value with no captures (OQ2-9 default). `publish_captured_memories` (`crates/daemon/src/memory_capture.rs:1176`) takes the spec once and inserts it before the job's first written decision (`:1235`); a job whose memories are all `duplicate_of` writes no anchor. Every decision of the job, a superseding one included, names the same `anchor_id`. The job id is stable (`crates/memory-store/src/memory_capture.rs:112`) and the head is frozen (U2), so a retry rebuilds the same spec, and `insert_anchor` refuses a second row with one id as `Conflict`.

Citation events carry `recorded_at` from the publishing attempt's clock, so a retry's events match the rolled-back attempt's in kind and evidence id, not in `recorded_at`.

## Failure scenario

An anchor id that included a timestamp or an attempt counter would conflict on every retry, or write one row per attempt.

## Timing windows and dependencies

A retry after a rolled-back commit, or a replay after a lost reply.

## What a test must construct

One job and plan published into two fresh Kernels, comparing anchor and decision rows (`crates/daemon/src/memory_capture.rs:1701`); a publish refused after its anchor and first decision were written, then retried into the same Kernel (`:1739`); a committed publish whose completion the store refuses, then a retry that replays the receipt (`crates/daemon/src/source_capture_tests.rs:3801`); five jobs in one submission writing five anchors (`crates/daemon/src/source_capture_tests.rs:3475`).

## Investigation log

### Q: Share anchors across jobs at one oid?

- Sources examined: the specification open question and D6, resolved per job in the ticket.
- Findings: Per-job identity keeps one envelope per job and avoids shared-row liveness questions.
- Missing evidence: an owner decision for sharing.
- Conclusion: needs human input.
