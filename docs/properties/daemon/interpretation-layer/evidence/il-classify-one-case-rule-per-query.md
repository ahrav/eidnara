# il-classify-one-case-rule-per-query

## Discovery trigger

#1105 Part B S2 (OQ20): unifying the per-caller name rules would be a behavior
change.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:57-73` `NameRule`; `:78-84`
  `normalize`.
- Selection queries normalize (`tool_vocabulary.rs:107`, `:137`, `:150-152`);
  the read-back uses `IgnoreAsciiCase` (`:157`); `exit_code` uses `Exact`
  (`:202`).
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:17` (the per-consumer
  table), `:215`, `:257`; `crates/daemon/tests/selection_differential.rs:1404-1406`
  (`MCP_READ`, `Edit`, `TodoWrite` in the generator).

## Failure scenario

One rule for every query: `MCP_TodoWrite` either stops counting under keep-one
or starts matching the read-back, and goldens that hold mixed-case names move.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Does U4 keep per-caller rules or unify them?
- Sources examined: `tool_vocabulary.rs:14-17`, `:57-73`;
  `docs/compression-fidelity.md` (Tool vocabulary).
- Findings: U4 keeps the rules as module data, one per query.
- Missing evidence: none.
- Conclusion: resolved with answer (OQ20).

### Q: Is the `mcp_` collision a guarantee?
- Sources examined: `tool_vocabulary.rs:75-77`.
- Findings: the module documents that a stripped MCP name equal to a tabled
  name takes that class; an MCP server exposing `edit` collides by design.
- Missing evidence: none.
- Conclusion: resolved with answer: a known condition, not a guarantee.

### Q: Why does confidence move from the source's `medium` to `high`?
- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: U4 implemented the rules as `NameRule` data; `crates/daemon/src/tool_vocabulary_tests.rs:17` pins the per-consumer table and `:257` every tabled name.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.
