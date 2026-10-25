# il-unknown-tool-never-dropped-deduped-or-edit-marked

## Discovery trigger

Issue #1105 Part B S2 (R7, AE6 second half): an unknown tool takes every default.

## Evidence trail

- Edit marking requires `ToolClass::Edit` (`crates/daemon/src/selection.rs:645-658`).
- The drop decision maps `Edit`, `Tabled`, and `Unknown` to no drop
  (`selection.rs:669`).
- No dedup selector exists (U4).
- Tier: `tier` returns `T3` for any untabled name (`crates/daemon/src/tool_vocabulary.rs:136-145`).
- Display: `display_arg` reads the generic lists for any name
  (`tool_vocabulary.rs:187-191`).
- Exit code: `exit_code` requires the exact name `bash`
  (`tool_vocabulary.rs:200-207`).
- Tests: `selection.rs:2828` (identical `mcp_foo` calls with `filePath` and
  `oldString`, no decision, against an `edit` positive control);
  `crates/daemon/src/tool_vocabulary_tests.rs:133`.

## Failure scenario

`mcp_foo` with an `oldString` argument becomes an `edit_marker`, and the agent
loses the output of a custom tool it still needs.

## Timing windows and dependencies

The emergency tiered drop (`selection.rs:789`) may drop tier-3 arcs under
budget pressure; that decision is by budget, not class.

## What a test must construct

Covered for class decisions.

## Investigation log

### Q: Does "never dropped" include budget-driven drops?

- Sources examined: #1105 R7; `selection.rs:789` `select_emergency`.
- Findings: the plan text does not separate class decisions from budget
  decisions; the emergency pass drops by tier.
- Missing evidence: an owner decision.
- Conclusion: needs human input; the record scopes the guarantee to class
  decisions.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: U4 implemented the class arms; `crates/daemon/src/selection.rs:2828` runs an `Unknown` tool through selection beside an `edit` positive control.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: Why is the check `always(!X)` when the source's `Check` line opens with `always`?

- Sources examined: the source record (#1105 Part B S2) and METHOD's
  check-semantics rules.
- Findings: the source's own rationale says "`always(!X)` for the forbidden
  drop/dedup/mark; the forbidden states have no dedicated code point". METHOD
  assigns a forbidden state with no detection point to `always(!X)`.
- Missing evidence: none.
- Conclusion: resolved with answer: the label follows the source's rationale.
