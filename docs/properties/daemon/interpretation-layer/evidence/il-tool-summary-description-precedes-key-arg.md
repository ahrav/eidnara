# il-tool-summary-description-precedes-key-arg

## Discovery trigger

Issue #1105 Part A evaluation (gap): the summary line's precedence and truncation
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
  `crates/daemon/src/tool_outcome.rs:254`, `:273`.
- Reachability: the summarizer path needs a model chain (`catalog.md`,
  Reachability classes).
- Tests: `history_summarizer_chunk.rs:3312` (description and key-argument
  arms, on separate inputs), `:3251`;
  `crates/daemon/src/lib.rs:19653`
  `history_summarizer_boundary_construction_matches_owned_reference`, which
  expects `TC: bash` (`lib.rs:19927-19932`) for a call whose input holds only
  `command` (`lib.rs:26292`), the bare-name arm;
  `crates/daemon/src/tool_vocabulary_tests.rs:66` (the cut).

## Failure scenario

`display_arg` drops the marker or counts bytes instead of scalar values; every
long-path summary line changes and the chunk golden moves.

## Timing windows and dependencies

None.

## What a test must construct

One input holding both a non-empty `description` and a `filePath`, asserting
`TC: {description}`, so the precedence between the first two arms is pinned.

## Investigation log

### Q: Does the description arm include the tool name?

- Sources examined: `history_summarizer_chunk.rs:1291-1296`;
  `history_summarizer_chunk.rs:3331`.
- Findings: no; the arm renders `TC: {description}`. The source record stated
  `name(description)`.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the source left the locations unverified; this pass read `format_tool_summary` and every caller at `41d889ea4`, and `history_summarizer_chunk.rs:3312`, `crates/daemon/src/lib.rs:19653` (`:19651` at `41d889ea4`), and `crates/daemon/src/tool_vocabulary_tests.rs:66` exercise all three arms on separate inputs.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.
