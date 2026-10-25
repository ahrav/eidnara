# il-dedup-selector-never-fires-on-harness-names

## Discovery trigger

Issue #1105 Part A lens pass (bug history and unproven assumptions): the dedup
selector compared raw tool names against a list of `mcp_`-prefixed names that
no pinned harness sends.

## Evidence trail

- At `a0e3f2993`, `DEDUP_SAFE_TOOLS` listed `mcp_grep`, `mcp_read`, and
  similar names, and `select_tool_dedup` compared `arc.dedup_name`, the raw
  name, against it. Part A cites the list, `dedup_name`, the selector, and its
  call site in `select_reductions_with_outcome`.
- `git diff a91dbc2b6 97afdaff1 -- crates/daemon/src/selection.rs` deletes the
  list, `dedup_name`, and `select_tool_dedup`; `97afdaff1` is the merge of U4
  (#1117).
- At `41d889ea4` the only per-name selector is `select_supersession`
  (`crates/daemon/src/selection.rs:630-677`), whose arms are `Edit`,
  `TodoWrite`, `EidnaraReduce`, `ZeroValueMeta`, `EidnaraNote`, and a no-drop
  arm for `Tabled` and `Unknown` (`selection.rs:659-670`).
- `selection.rs:2828` `identical_calls_produce_no_dedup_decision_for_any_tool_name`
  runs two identical calls under `read`, `mcp_read`, and `mcp_foo` and asserts
  an empty outcome.
- The frozen corpus sessions (`crates/daemon/testdata/corpus/sessions/`) hold
  no tool name beginning `mcp_`; the OpenCode MCP probe registers as
  `corpus_probe`.

## Failure scenario

Before U4: a harness exposing a tool literally named `mcp_read` sends two
identical calls under one owner; the selector drops the older arc as a
duplicate. At HEAD no code path can produce this.

## Timing windows and dependencies

None. The selector was a pure function of the window.

## What a test must construct

Nothing further. The successor record
`il-dedup-removal-leaves-goldens-byte-identical` holds the check that the
removal changed no output.

## Investigation log

### Q: Does any selector at HEAD drop an arc as a duplicate of another?

- Sources examined: `selection.rs:630-677`, `selection.rs:893` and `:905`
  (`select_reductions` and `select_reductions_with_outcome`), the U4 diff.
- Findings: no. Supersession drops by class (`todowrite` keep-one,
  `eidnara_reduce` keep-K, zero-value meta, zero-value note actions) and marks
  older edits; none compares argument equality.
- Missing evidence: none.
- Conclusion: resolved with answer: the record is invalidated.

### Q: Which names does each pinned harness register?

- Sources examined: the frozen OpenCode 1.18.22 and Pi 0.80.2 sessions; the
  census in `packages/e2e-tests/src/corpus-sessions/census.ts`.
- Findings: the frozen sessions name `bash`, `read`, `edit`, `write`, `glob`,
  `grep`, `todowrite`, `task`, and `corpus_probe` (OpenCode), and the Pi
  built-in tools; no `mcp_` name.
- Missing evidence: a registry listing, which no harness exposes to the daemon.
- Conclusion: resolved for the frozen closures; OQ9 owns the general question.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the deletion is a read of the U4 diff (`97afdaff1`), not a premise about harness names, so the record's claim about HEAD rests on code alone.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: Does the record run under `DaemonConfig::default()`, as the source's label says?
- Sources examined: `crates/daemon/src/config.rs:147`, `:187-191`;
  `crates/daemon/src/transform.rs:3350-3351`, `:3364-3370`; `transform.rs` and `selection.rs` at `a0e3f2993`.
- Findings: no. The default `model_chain` is empty, so `eidnara_folds()` is
  false and `apply_once` hands the request to `apply_additive_only`, which
  projects the window (`transform.rs:2852`) and composes m0
  (`transform.rs:3061`) without selecting, injecting, or rendering a
  reduced call. A switched-lineage subagent request passes the gate and
  returns a passthrough. At `a0e3f2993` the same gate stood before `apply_once`, and
  `select_tool_dedup` ran only from `select_reductions_with_outcome`,
  unconditionally within selection.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.
