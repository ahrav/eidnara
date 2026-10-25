# il-adapter-refusal-is-per-message

## Discovery trigger

#1105 Part A lens pass (failure and degradation): an adapter refusal arm
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
- Counting: `CaptureJob::units` (`crates/daemon/src/source_capture.rs:1011-1041`)
  counts each `Err` once (`:1033`) and adds the count to `adapter_refused`
  (`:1036-1039`), also when a later record stops the pass (`:1029-1032`).
- Pi shape: `crates/daemon/tests/harness_sources.rs:757-762` sets the second
  content item of a `toolResult` whose first item is text (`TOOL_BYTES`,
  `harness_sources.rs:433`) to an `image` item and asserts the whole record
  refuses as `UnsupportedShape("tool result item")`.
- OpenCode shape: `crates/daemon/tests/harness_sources.rs:681-686` sets
  the settled tool part's `state.output` to an object in a record whose text
  parts sit at `harness_sources.rs:388` and `:393`, and asserts the whole
  record refuses as `UnsupportedShape("tool output")`.
- U3: `crates/daemon/tests/source_corpus_replay.rs:80` asserts that the frozen
  Pi session's image-bearing tool result (entry `e0041`, a text item then an
  `image` item) is among `refused_records` and that the reader classifies it
  `non_text_item`. The reader classifies the whole record as a non-unit
  (`crates/daemon/src/source_corpus.rs:720-729`), so the pass expects no unit
  from it and the refusal reconciles as no loss, not as `adapter_refused`.

## Failure scenario

A Pi `toolResult` holds a text item and an image item. The adapter reads the
text item, reaches the image item, and refuses at `harness_sources.rs:431`. The text item's unit
is never kept; `adapter_refused` rises by one; nothing names the lost text.

## Timing windows and dependencies

Shape-driven, not time-driven. The same record refuses on every pass.

## What a test must construct

The adapter half exists for both shapes: the Pi `toolResult` with a text item
then an `image` item (`harness_sources.rs:757-762`) and the OpenCode record
with text parts beside an object `state.output` (`harness_sources.rs:681-686`).
What remains is a capture pass over each: assert the daemon counter
`adapter_refused == 1` and zero units from the record. For the OpenCode
record the reader's expected text units then reconcile as `adapter_refused`.
For the Pi record the reader classifies the whole record as a `non_text_item`
non-unit (`source_corpus.rs:720-729`), so no expected text unit exists to
reconcile; the Pi pass shows the counter and the lost text only through the
daemon counter and the adapter's zero units, and the Pi asymmetry below is the
discovery trigger, so the Pi fixture stays in the pass.

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
- Sources examined: `harness_sources.rs` at `41d889ea4`; unchanged at
  `aff5a00e6`, where `source_capture.rs` and the replay test moved and the
  references above were re-checked.
- Findings: the arms moved by at most one line; the budget-stop returns are at
  `:321`, `:354`, `:435`, `:443`, `:454`.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record.
