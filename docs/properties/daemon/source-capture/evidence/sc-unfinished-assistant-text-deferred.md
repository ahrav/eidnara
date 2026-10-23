# sc-unfinished-assistant-text-deferred

## Discovery trigger

Part A record on settledness. The spec's Impact line said U2 must treat
deferred assistant text as pending, not refused, and U2's ticket lists this
record under "pending state". U5 added a deferral for interrupted tools.
Re-verified at HEAD `5a842d998`.

## Evidence trail

- `crates/daemon/src/harness_sources.rs` `opencode_units`:
  - `completed` is `time.completed` filtered for non-null; the message
    revision is `completed` or `time.created`.
  - The arm `Some("text") if role == "assistant" && completed.is_none() => {}`
    skips assistant text without a completion time.
  - For a tool part, `status` must be `completed` or `error`, otherwise the
    part is skipped; `crate::tool_outcome::opencode_interrupted(state) &&
    completed.is_none()` skips an interrupted call until the turn completes.
  - Other settled tool parts produce `raw_tool_spans` units at
    `state.time.end`.
- `crates/daemon/src/tool_outcome.rs` `opencode_interrupted` reads
  `state.metadata.interrupted == true`.
- Capture: `CaptureJob::units` calls `opencode_units`; deferred text yields no
  unit, so nothing reaches the memo or a counter. The next pass re-adapts the
  window.
- Tests read at HEAD (`crates/daemon/tests/harness_sources.rs`):
  - `unfinished_assistant_text_is_deferred_but_settled_tools_publish`: with
    `time.completed` null, then absent, the record yields two units, both
    `RawToolSpans`, which publish; with `info.error` set and no completion
    time, every unit is still `RawToolSpans`; after `completed` is set, the
    inventory's `messages` texts are exactly `"Hello"` and `""`, so the partial
    `"H"` never published.
  - `an_interrupted_call_waits_for_its_turn_to_complete`: the trace's aborted
    case with `completed` and `error` removed yields no unit; the settled form
    yields one unit whose outcome names `MessageAbortedError`, which publishes
    and then replays.

## Failure scenario

Partial assistant text publishes at `time.created` as its revision. When the
turn completes, the revision moves to `time.completed`, so a later completed
publication supersedes it; but a pass that observes two partial states under
the same `time.created` would publish different bytes under one key and meet
`IdentityReused`, a terminal refusal. Falsifier: any `messages` unit from an
assistant record without `time.completed`.

## Timing windows and dependencies

The window is a pass that runs mid-turn. Correct capture depends on a later
pass carrying the completed record. If the plugin never sends the record
again, the text is never captured, and no counter shows it.

## What a test must construct

Exists: adapter-level deferral for text and interrupted tools, and publication
after completion. Missing: a capture-path test that sends a mid-turn window,
then the completed window, and asserts the text publishes on the second pass
and the tool units publish on the first.

## Investigation log

### Q: Does U2 treat deferred text as pending?
- Sources examined: `ticket-1082.md` (Part A records list), `CaptureJob::units`,
  `CaptureJob::publish`, `classify`, `Memo`.
- Findings: deferred text produces no `SourceUnit`, so it is neither
  `Disposition::Pending` nor refused. It is absent until a pass carries the
  completed record.
- Missing evidence: whether the U2 owner intended a memo or counter entry.
- Conclusion: unresolved, needs a decision from the U2 owner on whether the
  capture report counts deferred text.

### Q: What changed since baseline?
- Sources examined: `opencode_units` doc comment and the U5 test.
- Findings: U5 added the interrupted-tool deferral, so the guarantee now
  covers interrupted tool parts as well as text. Baseline `hs.rs:230` is the
  assistant-text arm in `opencode_units`; baseline `hs-tests:888` is
  `unfinished_assistant_text_is_deferred_but_settled_tools_publish`.
- Conclusion: resolved with answer.

### Q: Reachability?
- Findings: the only production caller is `CaptureJob::units` behind
  `source_capture::consents`.
- Conclusion: resolved with answer: `explicit-config-only`.
