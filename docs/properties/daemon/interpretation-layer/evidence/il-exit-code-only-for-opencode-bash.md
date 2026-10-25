# il-exit-code-only-for-opencode-bash

## Discovery trigger

#1105 Part A lens pass (claimed guarantees): `tool_outcome` reads
`state.metadata.exit`, a tool-authored field, for one tool name only.

## Evidence trail

- `crates/daemon/src/tool_outcome.rs:21-48` `opencode`: `exit_code` comes from
  `crate::tool_vocabulary::exit_code(Harness::OpenCode, tool_name, metadata)`
  (`:31`).
- `crates/daemon/src/tool_vocabulary.rs:200-207`: OpenCode with a name equal
  to `EXIT_CODE_TOOL` (`tv.rs:35`, `"bash"`) under `NameRule::Exact` reads
  `metadata.exit` with `as_i64`; every other case is `None`.
- `tool_outcome.rs:59-76` `pi`: `exit_code: None` at `:72`.
- Callers: `crates/daemon/src/harness_sources.rs:350` and `:432`, both inside
  the capture adapters.
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:145` over the OpenCode
  1.19.0 trace fixture (`bash` exit 1 and 0, four `bash` cases without an
  integer exit, one `read`) and the string, float, `read`, and Pi cases;
  `tool_vocabulary_tests.rs:17` over 25 names; `crates/daemon/tests/harness_sources.rs:3619`.
- Frozen OpenCode 1.18.22 session: four `bash` parts, three with
  `metadata.exit == 0` and one with `metadata.exit == null`.

## Failure scenario

A tool other than `bash` writes `metadata.exit` with another meaning; reading
it would put a false exit code into the outcome record and the request digest.

## Timing windows and dependencies

None.

## What a test must construct

Covered: the trace cases and the mutated cases in `tool_vocabulary_tests.rs:145`.

## Investigation log

### Q: Does OpenCode 1.18.22 write `metadata.exit` for `bash`?
- Sources examined: `crates/daemon/testdata/corpus/sessions/opencode-1.18.22/mix.json`.
- Findings: yes, as an integer on three of four `bash` parts; one carries
  `null`, which `as_i64` maps to `None`.
- Missing evidence: none.
- Conclusion: resolved with answer: both pinned OpenCode versions write it.

### Q: Does the digest emit the exit code?
- Sources examined: `tool_outcome.rs:156-176` `outcome_digest`.
- Findings: the digest holds status, bytes, head, and tail; no exit field.
- Missing evidence: none.
- Conclusion: resolved with answer: lead L3 (#1066 D9's `[; exit=N]`) stands.
