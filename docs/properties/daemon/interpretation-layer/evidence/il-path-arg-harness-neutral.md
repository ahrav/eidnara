# il-path-arg-harness-neutral

## Discovery trigger

#1105 Part B S2 (R6): one path query for both harnesses' spellings.

## Evidence trail

- `path_arg` (`crates/daemon/src/tool_vocabulary.rs:174-176`) reads
  `PATH_KEYS` (`:41`): OpenCode `filePath`, `file_path`, then Pi `path`.
- `display_arg` (`tool_vocabulary.rs:187-191`) reads the two display lists
  (`:43`, `:45`).
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:127` (Pi `path` and
  OpenCode `filePath` yield `src/a.rs`); `:66` (the per-site table).
- `git diff a91dbc2b6 97afdaff1 -- crates/daemon/testdata` is empty, so the
  selection, wire, and chunk goldens are unchanged.

## Failure scenario

A Pi `edit` call carries `path`; if only `filePath` were read, Pi edits would
never supersede.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Does `display_arg` adopt the shared list?
- Sources examined: `tool_vocabulary.rs:43`; `tool_vocabulary_tests.rs:66`.
- Findings: no; it keeps the summary line's list without `file_path`, and the
  test pins `{"file_path": ...}` to `None`.
- Missing evidence: none for the behavior; the intent is the open question of
  `il-path-arg-key-order-first-match`.
- Conclusion: resolved with answer.
