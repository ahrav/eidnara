# il-classify-total-with-unknown

## Discovery trigger

Issue #1105 Part B S2 (R7): `classify` is total, with `Unknown` as a legal value.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:106-124` `classify`: normalizes, then
  a chain of `contains` and equality checks ending in `ToolClass::Unknown`.
- `normalize` (`tool_vocabulary.rs:78-84`) uses `to_lowercase` and `drain` on
  a prefix whose byte length it has just matched, so it cannot split a
  character.
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:248` (proptest over
  `any::<String>()`, determinism, `Unknown` implies `tier` 3, the
  single-normalization query), `:232` (empty, `mcp_foo`, 10 KiB, NUL,
  `mcp_\u{e9}dit`), `:133`, `:215`.

## Failure scenario

A lookup indexes the name or unwraps a table hit; a harness's new tool panics
the transform.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Does `classify` normalize before lookup?

- Sources examined: `tool_vocabulary.rs:106-108`.
- Findings: yes; `mcp_read` classifies as `read`, and `MCP_READ` as `Tabled`
  (`tool_vocabulary_tests.rs:243`).
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: U4 implemented `classify`; `crates/daemon/src/tool_vocabulary_tests.rs:248` runs it over arbitrary strings and `:232` over the edge names.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: Does the record run under `DaemonConfig::default()`, as the source's label says?
- Sources examined: `crates/daemon/src/config.rs:152`, `:194-198`;
  `crates/daemon/src/transform.rs:3352-3353`, `:3366-3372`, `:4135`.
- Findings: no. The default `model_chain` is empty, so `eidnara_folds()` is
  false and `apply_once` hands the request to `apply_additive_only`, which
  projects the window (`transform.rs:2854`) and composes m0
  (`transform.rs:3063`) without selecting, injecting, or rendering a
  reduced call. A switched-lineage subagent request passes the gate and
  returns a passthrough. `classify` has production callers only in selection
  (`crates/daemon/src/selection.rs:610`, `:645`, `:693`), which runs from
  `transform.rs:4135` inside `apply_once`.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.
