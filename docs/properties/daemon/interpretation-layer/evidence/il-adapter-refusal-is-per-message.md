# il-adapter-refusal-is-per-message

## Discovery trigger

Issue #1105 Part A lens pass (failure and degradation): an adapter refusal arm
returns before any unit of the record is kept, so one unsupported part costs
every sibling unit of its message.

## Evidence trail

- OpenCode adapter: `crates/daemon/src/harness_sources.rs:272-362`. The `?`
  arms inside the part loop are `:313` (text not a string), `:327` (tool part
  without `state`), `:340` (output not a string), `:343` (no `callID`), `:345-348`
  (no `state.time.end`), and `:351` (outcome refused). Each returns `Err`
  before `Ok(Some(kept.units))` at `:361`.
- Pi adapter: `harness_sources.rs:381-461`. Tool results: `:424` (no boolean
  `isError`), `:431` (a non-text item), `:433` (no `toolName`). Content items:
  `:452` (text item without string text).
- Budget stop: `Kept::keep` (`harness_sources.rs:463-482`) returns `false`
  when the copy budget refuses; the adapter returns `Ok(None)` at `:321`,
  `:354`, `:435`, `:443`, or `:454`.
- Counting: `CaptureJob::units` (`crates/daemon/src/source_capture.rs:1116-1146`)
  counts each `Err` once (`:1138`) and adds the count to `adapter_refused`
  (`:1141-1144`), also when a later record stops the pass (`:1134-1137`).
- U3: `crates/daemon/tests/source_corpus_replay.rs:93` asserts that the frozen
  Pi session's image-bearing tool result is among `refused_records` and that
  the reader classifies it `non_text_item`.

## Failure scenario

A Pi `toolResult` holds a text item and an image item. The adapter reads the
text item, reaches the image item, and refuses at `harness_sources.rs:431`. The text item's unit
is never kept; `adapter_refused` rises by one; nothing names the lost text.

## Timing windows and dependencies

Shape-driven, not time-driven. The same record refuses on every pass.

## What a test must construct

An OpenCode assistant message with two text parts and a settled tool part whose
`state.output` is a number; assert zero units, `adapter_refused == 1`, and the
reader's expected units all reconciled as `adapter_refused`.

## Investigation log

### Q: Is the per-message blast radius intended?

- Sources examined: the adapter doc comments (`harness_sources.rs:258`,
  `:367`); #1066; the corpus README.
- Findings: the doc comments list the refusals; none states that sibling units
  are lost. A Pi user-message `image` item is skipped (`:448`) while a Pi
  tool-result `image` item refuses the record.
- Missing evidence: a design decision on the asymmetry.
- Conclusion: needs human input.

### Q: Do the line references of the source hold at HEAD?

- Sources examined: `harness_sources.rs` at `41d889ea4`.
- Findings: the arms moved by at most one line; the budget-stop returns are at
  `:321`, `:354`, `:435`, `:443`, `:454`.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record.
