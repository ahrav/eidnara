# p2-capture-quote-is-an-exact-substring-of-its-own-batch-source

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoints C9 and AC5. Refinement R8 corrected the prompt offset.

## Evidence trail

`parse_capture_batch` (`crates/daemon/src/memory_capture.rs:266`) refuses the batch when a row's `message_id` names no batch message or repeats one. `validate_capture_memories` (`:701`) refuses a memory whose quote is empty, over `MAX_CAPTURE_MEMORY_BYTES`, or not contained in its own source's text (`:720`). The system prompt states the rule at `:180`. A refusal fails only the affected source's jobs. For a user message under its own id, `join_sentence_runs` (`:391`) runs before that validation and gives quotations that follow one another the slice of the same message that spans them, so the quotation validation checks is the one the plan freezes.

## Failure scenario

A quote taken from a neighbouring message would cite evidence for a statement the cited source never made.

## Timing windows and dependencies

None.

## What a test must construct

A batch where one memory quotes another message; an invented `message_id`; a duplicate `message_id`; an empty quote; a user message whose quotations join into one statement, whose stored quotation is a slice of that message.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/memory_capture.rs:4456`, `:4489`, `:4521`, `:4661`, `:7662`; `crates/daemon/src/lib.rs:24876`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
