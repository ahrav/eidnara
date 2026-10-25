# il-tool-summary-description-precedes-key-arg

## Discovery trigger

#1105 Part A evaluation (gap): the summary line's precedence and truncation
are golden-visible and `display_arg` had to reproduce them.

## Evidence trail

- `format_tool_summary` (`crates/daemon/src/history_summarizer_chunk.rs:1290-1302`):
  a non-empty string `description` renders `TC: {description}`; otherwise
  `extract_key_arg` renders `TC: {name}({key_arg})`; otherwise `TC: {name}`;
  then `normalize_text`.
- `extract_key_arg` (`crates/daemon/src/chunk_text.rs:122-127`) uses
  `display_arg`; path-like values pass through `truncate_arg`
  (`chunk_text.rs:129-137`), 60 scalar values plus `…`.
- Callers: `history_summarizer_chunk.rs:1254`, `:1259` (through
  `call_summary`), `crates/daemon/src/boundary.rs:1860`, `:1867`,
  `crates/daemon/src/tool_outcome.rs:251`, `:270`.
- Reachability: the summarizer path needs a model chain (`catalog.md`,
  Reachability classes).
- Tests: `history_summarizer_chunk.rs:3312` (description and key-argument
  arms), `:3251`; `crates/daemon/src/tool_vocabulary_tests.rs:66` (the cut).

## Failure scenario

`display_arg` drops the marker or counts bytes instead of scalar values; every
long-path summary line changes and the chunk golden moves.

## Timing windows and dependencies

None.

## What a test must construct

A bare-name case (`{}` input) asserting `TC: name`.

## Investigation log

### Q: Does the description arm include the tool name?
- Sources examined: `history_summarizer_chunk.rs:1291-1296`;
  `history_summarizer_chunk.rs:3331`.
- Findings: no; the arm renders `TC: {description}`. The source record stated
  `name(description)`.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record.
