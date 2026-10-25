# il-reduced-tool-call-carries-extracted-path

## Discovery trigger

#1105 Part A evaluation (gap): the path key list is visible in compaction
output through the reduced call's `path`, and no record named it.

## Evidence trail

- `flatten_block` (`crates/daemon/src/wire.rs:520`) computes
  `extract_file_path(input)` for a `ToolCall` (`wire.rs:540`) and stores it in
  the flat block's `file_path` (`wire.rs:568`, the field at `wire.rs:131`).
- `extract_file_path` is `tool_vocabulary::path_arg` (`wire.rs:646-648`).
- The transform passes `block.file_path` to `reduced_block`
  (`crates/daemon/src/transform.rs:11890-11896`).
- `reduced_block` (`wire.rs:445`) writes `reduced: true`, `summary`, and, when
  a path is present, `path` into the reduced call's input (`wire.rs:484-489`).
- No test asserts the `path` key against the original input; `wire.rs:1260`
  reduces a tool result with no path.

## Failure scenario

A change to `PATH_KEYS` order makes `{path: "a", filePath: "b"}` reduce to
`path: "a"` instead of `"b"`; the compaction output changes for every such
call and no golden holds one.

## Timing windows and dependencies

None.

## What a test must construct

A `reduced_block` test over a flattened call with an input under each key of
`PATH_KEYS`, two keys at once, and none; assert the `path` key or its absence.

## Investigation log

### Q: Does the reduced call read the same list as wire extraction?
- Sources examined: `wire.rs:445-489`, `:520-570`, `:646-648`;
  `transform.rs:11880-11896`.
- Findings: yes; the path is computed once by `path_arg` during flattening and
  carried to the reduction.
- Missing evidence: none.
- Conclusion: resolved with answer; confidence raised from the source's medium
  to high.

### Q: Does the record run under `DaemonConfig::default()`, as the source's label says?
- Sources examined: `crates/daemon/src/config.rs:147`, `:187-191`;
  `crates/daemon/src/transform.rs:3350-3351`, `:3364-3370`, `:5196`, `:11890`.
- Findings: no. The default `model_chain` is empty, so `eidnara_folds()` is
  false and `apply_once` hands the request to `apply_additive_only`, which
  projects the window (`transform.rs:2852`) and composes m0
  (`transform.rs:3061`) without selecting, injecting, or rendering a
  reduced call. A switched-lineage subagent request passes the gate and
  returns a passthrough. Flattening runs on both paths, so every flat block carries its path,
  but a reduced call renders only in `build_output_with_tags`
  (`transform.rs:11890`), whose production caller is `apply_once`
  (`transform.rs:5196`).
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.
