# p2-new-capture-anchor-identity-is-deterministic

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. U1 records it as proposed (ticket deviation confirmed at approval) because its check needs capture jobs and frozen plans. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

U3 derives `anchor_id` and `object_id` from `(job_id, head_oid)` and inserts one `reachable_from` anchor per job through `insert_anchor`. The job id is stable (`crates/memory-store/src/memory_capture.rs:112`) and the oid is frozen in the plan (U2). The Kernel half holds today: a second insert of one `anchor_id` is `Conflict` (`crates/kernel/tests/kernel_anchor_write.rs:628`), and a rolled-back commit stores nothing, so a retry writes the same row once.

## Failure scenario

An anchor id that included a timestamp would conflict on every retry or duplicate per attempt.

## Timing windows and dependencies

A retry after a rolled-back commit.

## What a test must construct

`kernel_write_failed` on the first publish, a successful retry, and a count of live anchor rows per id.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

### Q: Share anchors across jobs at one oid?

- Sources examined: the specification open question and D6.
- Findings: Per-job identity avoids shared-row liveness and receipt questions.
- Missing evidence: an owner decision for sharing.
- Conclusion: needs human input.
