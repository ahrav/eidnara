# il-tool-name-matching-differs-across-consumers

## Discovery trigger

#1105 Part A lens pass (unproven assumptions): consumers compared tool names
by different rules, so one spelling could mean two tools.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:57-73` `NameRule` (`IgnoreAsciiCase`,
  `Exact`) and `:78-84` `normalize` (lowercase, strip one `mcp_`).
- Selection: `classify` normalizes at `tool_vocabulary.rs:107`; `tier` at
  `:137`; `selection_tier` normalizes before `tier` (`:150-152`), so a name is
  normalized twice for the tier. Callers: `crates/daemon/src/selection.rs:610`,
  `:645`, `:693`, `:750`.
- Injection read-back: `is_injected_todo_tool` (`tool_vocabulary.rs:156-158`)
  under `IgnoreAsciiCase`, called at `crates/daemon/src/injection.rs:435`.
- Outcome: `exit_code` under `Exact` (`tool_vocabulary.rs:202`).
- Summary line: `history_summarizer_chunk.rs:1290-1302` shows the raw name.
- The dedup selector's raw-name rule is gone with the selector (U4).
- `crates/daemon/src/tool_vocabulary_tests.rs:17` pins 25 names at each
  consumer; `:257` holds every tabled name under case flips and one prefix.

## Failure scenario

`MCP_TodoWrite` is `todowrite` for selection's keep-one, but the injection
read-back does not accept it; a session using that spelling keeps its to-do
calls under keep-one while injection never reads their state.

## Timing windows and dependencies

None.

## What a test must construct

Covered by `tool_vocabulary_tests.rs:17`.

## Investigation log

### Q: Which rule does `classify` apply, and did U4 unify the rules?
- Sources examined: `tool_vocabulary.rs:1-18` module doc, `:57-158`; U4's
  `docs/compression-fidelity.md` paragraph.
- Findings: `classify` normalizes; each query keeps its HEAD rule as module
  data.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the double normalization in the tier lookup a behavior change?
- Sources examined: `tool_vocabulary.rs:147-152`; `tool_vocabulary_tests.rs:58-60`.
- Findings: no; at `a0e3f2993` selection normalized at grouping and again in
  the tier lookup. `mcp_mcp_edit` is `Unknown` for `classify` and tier 2 for
  `selection_tier`, and U4 documents it.
- Missing evidence: none.
- Conclusion: resolved with answer.
