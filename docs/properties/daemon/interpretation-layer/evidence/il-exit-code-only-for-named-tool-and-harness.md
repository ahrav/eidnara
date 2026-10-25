# il-exit-code-only-for-named-tool-and-harness

## Discovery trigger

Issue #1105 Part B S2 (R6): the `bash` literal moves into the module's
`exit_code` query.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:35` `EXIT_CODE_TOOL`; `:200-207`
  `exit_code`.
- One production caller: `crates/daemon/src/tool_outcome.rs:33`, inside
  `opencode` (`tool_outcome.rs:23`), whose production caller is the capture
  adapter (`crates/daemon/src/harness_sources.rs:350`).
- The request digest format `source-request.v2` binds the outcome record
  (`harness_sources.rs:600`).
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:145`, `:17`, `:133`.

## Failure scenario

`exit_code` widens to every tool carrying `metadata.exit`; replayed tool units
change digest and stored receipts conflict as `IdentityReused`.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Is the record default-production, as the source labels it?

- Sources examined: every caller of `tool_outcome::opencode` and
  `tool_outcome::pi` outside tests.
- Findings: the only callers are `harness_sources.rs:350` and `:432`, inside
  the capture adapters, which run under `source_capture_enabled`.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: U4 implemented the query; `crates/daemon/src/tool_vocabulary_tests.rs:145` runs it over the trace fixture and the mutated cases.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.
