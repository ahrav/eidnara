# p2-decision-parents-must-be-live-at-insert

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S2, touchpoints C10 and AC1. Refinement R1 relabels its anchor clause until a production input reaches it.

## Evidence trail

`insert_decision` (`crates/kernel/src/slice/write.rs:920`) calls `require_parents` (`:1095`), which runs `require_live` for the domain and `require_optional_live` for proposition, scope, anchor, and evidence. Each query requires `invalidated_commit_seq IS NULL` inside the envelope's transaction and returns `NotFound` otherwise. The parent check runs before the registry insert, so a refusal leaves no registry row.

Memory capture passes `anchor_id: None` (`crates/daemon/src/memory_capture.rs:1202`), so at the branch tip no production decision reaches the anchor clause. `crates/kernel/tests/kernel_anchor_write.rs:399` inserts a decision naming an anchor before the anchor in one envelope and gets `NotFound`, and the commit stores neither row.

## Failure scenario

A decision naming a retired anchor would commit a dangling reference that the engine resolves as an absent anchor.

## Timing windows and dependencies

A parent inserted later in the same envelope is not live when the child is inserted.

## What a test must construct

A decision naming a missing domain; a decision naming an anchor inserted after it in one envelope; once U3 lands, the `sometimes(decision_insert_named_an_anchor_id)` marker on a production publish.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_slice.rs:196`; `crates/kernel/tests/kernel_envelope.rs:669`; `crates/kernel/tests/kernel_anchor_write.rs:399`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the anchor clause reached in production?

- Sources examined: `publish_captured_memories` and every other `DecisionSpec` construction in `crates/daemon/src`.
- Findings: at U1 and U2 every production `DecisionSpec` set `anchor_id: None`.
  At U3 the capture publish inserts the job's anchor before the job's first
  decision and names it on every decision of the job
  (`crates/daemon/src/memory_capture.rs:1261`), so the clause is reached on
  every anchored publish (`crates/daemon/src/source_capture_tests.rs:3496`).
- Missing evidence: none.
- Conclusion: resolved with answer: R1's `always-or-unreached` label is
  reversed to `always` at U3.
