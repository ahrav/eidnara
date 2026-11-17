# p2-decision-parents-must-be-live-at-insert

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S2, touchpoints C10 and AC1. Refinement R1 relabels its anchor clause until a production input reaches it.

## Evidence trail

`insert_decision` (`crates/kernel/src/slice/write.rs:920`) calls `require_parents` (`:1095`), which runs `require_live` for the domain and `require_optional_live` for proposition, scope, anchor, and evidence. Each query requires `invalidated_commit_seq IS NULL` inside the envelope's transaction and returns `NotFound` otherwise. The parent check runs before the registry insert, so a refusal leaves no registry row.

At U1 and U2 memory capture passed `anchor_id: None`, so no production decision reached the anchor clause. Since U3, `publish_captured_memories` (`crates/daemon/src/memory_capture.rs:3841`) inserts the job's anchor before the job's first decision and names it on every memory decision of the job (`:4079`, `:4115`); the statement record of an approved user statement is a `user-statements` decision with `anchor_id: None` (`:2041`). So every publish of a job with a readable frozen HEAD that writes a decision reaches the clause in production order. A job whose plan froze no HEAD has no anchor (`crates/daemon/src/memory_capture/cite.rs:403`) and writes its decisions with `anchor_id: None`, and a job whose memories publication all skips as duplicates inserts no capture decision and no anchor (`crates/daemon/src/memory_capture.rs:4008`); neither reaches the clause with an anchor of its own. Publication skips a memory that names `duplicate_of` unless it is the user's statement of a memory the Kernel does not serve as approved (`:4014`); that statement is written as a decision that restates the memory, so with a readable frozen HEAD its job inserts the anchor and reaches the clause as any job that writes a decision does. A contest a job records supersedes the contested statement with a row that keeps that statement's `anchor_id` (`crates/daemon/src/memory_capture.rs:3635`), and the supersession inserts that row through `insert_decision` (`crates/kernel/src/slice/write.rs:426`). So a job of either kind that contests a statement written under an anchor reaches the clause with that statement's anchor, the anchor of the job whose extraction wrote the statement, and inserts no anchor of its own. `crates/kernel/tests/kernel_anchor_write.rs:404` inserts a decision naming an anchor before the anchor in one envelope and gets `NotFound`, and the commit stores neither row.

## Failure scenario

A decision naming a retired anchor would commit a dangling reference that the engine resolves as an absent anchor.

## Timing windows and dependencies

A parent inserted later in the same envelope is not live when the child is inserted.

## What a test must construct

A decision naming a missing domain; a decision naming an anchor inserted after it in one envelope; once U3 lands, the `sometimes(decision_insert_named_an_anchor_id)` marker on a production publish.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_slice.rs:197`; `crates/kernel/tests/kernel_envelope.rs:669`; `crates/kernel/tests/kernel_anchor_write.rs:404`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the anchor clause reached in production?

- Sources examined: `publish_captured_memories` and every other `DecisionSpec` construction in `crates/daemon/src`.
- Findings: at U1 and U2 every production `DecisionSpec` set `anchor_id: None`.
  At U3 the capture publish inserts the job's anchor before the job's first
  decision and names it on every memory decision of the job
  (`crates/daemon/src/memory_capture.rs:4079`); a statement record names no
  anchor (`crates/daemon/src/memory_capture.rs:2041`). So the clause is reached on
  every publish of a job with a readable frozen HEAD that writes a decision
  (`crates/daemon/src/source_capture_tests.rs:3528`); a job whose plan froze
  no HEAD writes its decisions with `anchor_id: None`, and a job whose memories
  publication all skips as duplicates inserts no capture decision and no anchor
  (`crates/daemon/src/memory_capture.rs:4008`). A user's statement that names
  a memory the Kernel does not serve as approved under `duplicate_of` is
  written as a decision (`crates/daemon/src/memory_capture.rs:4014`), so its
  job reaches the clause when its plan froze a readable HEAD. A contest a job
  records supersedes the contested statement with a row that keeps that
  statement's anchor (`crates/daemon/src/memory_capture.rs:3635`), so a job
  with no anchor of its own, one with no frozen HEAD or one of skipped
  duplicates, reaches the clause with the contested statement's anchor when
  that statement names one.
- Missing evidence: none.
- Conclusion: resolved with answer: R1's `always-or-unreached` label is
  reversed to `always` at U3, because production reaches the clause on every
  anchored publish; a publish with no frozen HEAD inserts no anchor, and a
  contest it records names the contested statement's anchor.
