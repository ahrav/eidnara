# il-path-arg-key-order-first-match

## Discovery trigger

Issue #1105 Part A lens pass (architecture and data flow): three sites each held an
ordered key list, and the lists diverged.

## Evidence trail

- After U4 the lists are module constants: `PATH_KEYS`
  (`crates/daemon/src/tool_vocabulary.rs:41`), `DISPLAY_PATH_KEYS` (`:43`),
  `DISPLAY_SYMBOL_KEYS` (`:45`).
- `first_string` (`tool_vocabulary.rs:166-170`) returns the first key whose
  value is a string.
- `path_arg` (`tool_vocabulary.rs:174-176`) serves wire extraction
  (`crates/daemon/src/wire.rs:646-648`, called at `wire.rs:540`) and edit
  supersession (`crates/daemon/src/selection.rs:649`).
- `display_arg` (`tool_vocabulary.rs:187-191`) serves `extract_key_arg`
  (`crates/daemon/src/chunk_text.rs:122-127`), which truncates a path-like
  value to 60 scalar values plus `…` (`chunk_text.rs:129-137`).
- `crates/daemon/src/tool_vocabulary_tests.rs:66` pins fourteen inputs at both
  queries, including `{"path": "p", "file_path": "f"}` (path `f`, display `p`)
  and `{"file_path": "src/a.rs"}` (display `None`).

## Failure scenario

An OpenCode or Pi tool passes `file_path`. Edit supersession and the wire see
the path; the summary line shows `TC: name` with no argument.

## Timing windows and dependencies

None.

## What a test must construct

Covered by `tool_vocabulary_tests.rs:66`.

## Investigation log

### Q: Did U4 change any site's list?

- Sources examined: the U4 diff of `selection.rs`, `wire.rs`,
  `chunk_text.rs`; `tool_vocabulary.rs:41-45`; the empty testdata diff
  `git diff a91dbc2b6 97afdaff1 -- crates/daemon/testdata`.
- Findings: the selection and wire lists were equal and became `PATH_KEYS`;
  the chunk lists became the two display lists unchanged.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the display list's omission of `file_path` intended?

- Sources examined: `72f35b64d`, which introduced the lists together; #1072.
- Findings: no commit message or comment states a reason; U4 preserved it.
- Missing evidence: an owner decision.
- Conclusion: needs human input.
