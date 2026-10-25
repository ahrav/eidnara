# il-synthetic-todowrite-round-trips-under-one-name

## Discovery trigger

#1105 Part A evaluation (gap): injection writes, reads back, and selection
counts one tool name through three spellings.

## Evidence trail

- `TODO_TOOL` (`crates/daemon/src/tool_vocabulary.rs:26`) is `todowrite`.
- Emission: `crates/daemon/src/injection.rs:158` (call name) and `:173`
  (result tool name).
- Read-back: `injection.rs:429-457` `newest_todowrite_state_json` skips calls
  `is_injected_todo_tool` rejects (`injection.rs:435`; the rule at
  `tool_vocabulary.rs:156-158`).
- Selection: `classify(TODO_TOOL)` is `TodoWrite` (`tool_vocabulary.rs:111`);
  keep-one at `crates/daemon/src/selection.rs:660-663` and the two-pass
  exemption at `selection.rs:693`.
- Exclusion: ingress marks any message carrying a `synthetic_todo_` id
  synthetic (`crates/daemon/src/transform.rs:2597-2615`), and `live` drops
  synthetic blocks (`transform.rs:3467-3471`) before `tail_for_selection` is
  built (`transform.rs:4063-4064`). Selection and the read-back read that
  tail, so neither sees the echoed synthetic pair.
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:17` (the `todowrite`
  row); `injection.rs:553`, `:1047`, `:1052`;
  `crates/daemon/src/transform.rs:29645`
  `warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`
  (the echoed pair beside a native `todowrite` call, no duplicate tool-use
  id).

## Failure scenario

The emitted name diverges from `TODO_TOOL`. The echoed pair is still excluded
by id, so selection does not drop it; the injected to-do state now sits under
a tool name that differs from the name the harness's own to-do calls carry,
which is the name selection's keep-one and the read-back match.

## Timing windows and dependencies

None.

## What a test must construct

A fold-path transform holding the echoed synthetic pair and one native
`todowrite` call: assert the pair is excluded by id before selection, and that
keep-one and the read-back act on the native call alone.

## Investigation log

### Q: Do the three uses resolve one constant at HEAD?
- Sources examined: `injection.rs:10` imports `TODO_TOOL` and
  `is_injected_todo_tool`; `selection.rs` reaches the name through `classify`.
- Findings: yes; U4 replaced the separate spellings with `TODO_TOOL`.
- Missing evidence: a transform-level test that keep-one and the read-back
  act on the native call alone beside the echoed pair.
- Conclusion: resolved with answer; the test gap stays open.

### Q: Can selection drop the synthetic call as an older `todowrite`?
- Sources examined: `transform.rs:2597-2615`, `:3467-3471`, `:4063-4064`;
  `injection.rs:164-165`, `:181-182` (the emitted messages carry
  `synthetic: true`).
- Findings: no. The synthetic pair is excluded by its `synthetic_todo_` id and
  its synthetic flag before the selection tail is built. The source's impact
  line assumed selection saw the pair.
- Missing evidence: none.
- Conclusion: resolved with answer: corrected in the record's enabling state
  and impact.

### Q: Why does confidence move from the source's `medium` to `high`?
- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the source left the three uses unverified; this pass read all three at `41d889ea4` and found one constant, `TODO_TOOL` (`crates/daemon/src/tool_vocabulary.rs:26`), behind them, and `crates/daemon/src/tool_vocabulary_tests.rs:17` asserts both queries on it.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: Does the record run under `DaemonConfig::default()`, as the source's label says?
- Sources examined: `crates/daemon/src/config.rs:147`, `:187-191`;
  `crates/daemon/src/transform.rs:3350-3351`, `:3364-3370`, `:4067`, `:4451`.
- Findings: no. The default `model_chain` is empty, so `eidnara_folds()` is
  false and `apply_once` hands the request to `apply_additive_only`, which
  projects the window (`transform.rs:2852`) and composes m0
  (`transform.rs:3061`) without selecting, injecting, or rendering a
  reduced call. A switched-lineage subagent request passes the gate and
  returns a passthrough. Emission and the read-back run from `transform.rs:4067` and `:4451`,
  inside `apply_once` past the gate.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.
