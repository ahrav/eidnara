# il-vocabulary-literals-live-in-one-module

## Discovery trigger

#1105 Part B S2 (R6, KTD4): one module owns every tool-name and argument-key
table, and a gate keeps distinctive literals out of every other daemon
production file.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:26-55` holds the tables: `TODO_TOOL`,
  `EDIT_TOOLS`, `ZERO_VALUE_META_TOOLS`, `T1_TOOLS`, `T2_TOOLS`,
  `EXIT_CODE_TOOL`, `ZERO_VALUE_NOTE_ACTIONS`, `NOTE_ACTION_KEY`, `PATH_KEYS`,
  `DISPLAY_PATH_KEYS`, `DISPLAY_SYMBOL_KEYS`, `DIFF_KEYS`, `MCP_PREFIX`; the
  name rules at `:57-84`; the `exit` key at `:203`; the truncation pointers at
  `:214-215`.
- The gate `scripts/forbid-tool-literals.sh` runs in CI
  (`.github/workflows/ci.yml:302-303`) and exits 0 at `41d889ea4`.
- The classification ([`literals.md`](../literals.md)) finds vocabulary rows
  outside the module: `crates/daemon/src/injection.rs:21-24` (`medium`,
  `completed`, `cancelled`), `:407-409` (`content`, `status`, `priority`),
  `:465` (`todos`), and `crates/daemon/src/history_summarizer_chunk.rs:1292`
  (`description`).
- Each is a generic word, so the gate's token rule does not match it.

## Failure scenario

OpenCode renames the `todowrite` item field `priority`. The fix lands in
`injection.rs`, outside the module the specification names as the one site,
and a later reader of the module believes it holds every tool-authored key.

## Timing windows and dependencies

None; static.

## What a test must construct

A table-driven check that every vocabulary row of [`literals.md`](../literals.md)
sits in `tool_vocabulary.rs`, or a decision that records the two residues as
accepted.

## Investigation log

### Q: Are the `todowrite` input keys vocabulary?
- Sources examined: R5's rule; `injection.rs:400-470`; the OpenCode
  `todowrite` tool input in the frozen session.
- Findings: yes; a harness with no to-do tool never emits them, and the tool
  author chose the names.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Move them behind module queries, or accept them?
- Sources examined: #1105 D3; U4's PR scope (tables listed in R6).
- Findings: R6 lists the tables U4 moved; it does not list the to-do schema or
  `description`.
- Missing evidence: an owner decision.
- Conclusion: needs human input.
