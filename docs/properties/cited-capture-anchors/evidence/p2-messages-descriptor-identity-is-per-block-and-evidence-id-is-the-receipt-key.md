# p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S4, touchpoints D3, OQ2-3, and OQ2-12. The U3 citation lookup reads descriptors by this identity.

## Evidence trail

`SessionIdentity::message_unit` (`crates/daemon/src/harness_sources.rs:207`) builds one `messages` unit per text block with identity `project_id`, `harness`, `session_id`, `message_id`, and `block_index`. `evidence_request` (`:1010`) sets `evidence_id` to `srcev:` plus the unit's receipt key, and assigns `harness_capture` retention. The capture job never sees the key, so it cannot name the evidence.

## Failure scenario

An evidence id derived from the text would let a capture output compute and cite evidence directly.

## Timing windows and dependencies

None.

## What a test must construct

A Pi message with two text blocks, asserting two descriptors with consecutive `block_index` values and evidence ids equal to their receipt keys.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/source_capture_tests.rs:866`, `:1258`; `crates/kernel/tests/kernel_source_descriptors.rs:494`, `:2484`, `:2593`, `:2717`, `:2795`, `:2871`; `crates/daemon/tests/harness_sources.rs:1724`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the specification's `harness_sources.rs:1727-1736` range still apply?

- Sources examined: `crates/daemon/tests/harness_sources.rs` at the branch tip.
- Findings: Those lines sit inside `mixed_sessions_create_pending_only_for_message_text`, which starts at 1706.
- Missing evidence: none.
- Conclusion: resolved with answer: the record cites the test.
