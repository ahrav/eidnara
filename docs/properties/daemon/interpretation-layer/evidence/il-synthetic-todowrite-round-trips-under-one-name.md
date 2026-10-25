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
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:17` (the `todowrite`
  row); `injection.rs:553`, `:1047`, `:1052`.

## Failure scenario

If the emitted name and the counted name diverged, selection would treat the
synthetic call as an older `todowrite` and drop it, or the read-back would miss
the synthetic state.

## Timing windows and dependencies

None.

## What a test must construct

A selection input holding one synthetic and one native `todowrite` call;
assert keep-one keeps the newer and the read-back reads the newer state.

## Investigation log

### Q: Do the three uses resolve one constant at HEAD?
- Sources examined: `injection.rs:10` imports `TODO_TOOL` and
  `is_injected_todo_tool`; `selection.rs` reaches the name through `classify`.
- Findings: yes; U4 replaced the separate spellings with `TODO_TOOL`.
- Missing evidence: a selection-level test of the round trip.
- Conclusion: resolved with answer; the test gap stays open.
