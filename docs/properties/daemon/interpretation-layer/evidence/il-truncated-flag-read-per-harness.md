# il-truncated-flag-read-per-harness

## Discovery trigger

Issue #1105 Part A evaluation (refinement): the exit-code record hid that the Pi
truncation flag comes from a different field than the OpenCode one.

## Evidence trail

- `crates/daemon/src/tool_vocabulary.rs:212-218` `truncated_flag`: OpenCode
  reads `/metadata/truncated` under the tool part's `state`; Pi reads
  `/details/truncation/truncated` under the `toolResult` message; both with
  `as_bool`.
- Callers: `crates/daemon/src/tool_outcome.rs:37` (OpenCode, passing `state`)
  and `tool_outcome.rs:71` (Pi, passing the message).
- Tests: `crates/daemon/src/tool_vocabulary_tests.rs:185` covers both pointers,
  a string value, a missing flag, and a Pi message read as OpenCode;
  `crates/daemon/tests/harness_sources.rs:3524` covers the OpenCode trace.
- The U3 report (posted with #1109) stratifies tool-result byte lengths by
  the flag: OpenCode 2 `true`, 11 `false`; Pi 12 `unknown`.

## Failure scenario

A Pi harness version moves the flag; every Pi result reads `unknown`, and the
report's stratification silently loses the truncated stratum.

## Timing windows and dependencies

None.

## What a test must construct

A Pi session whose `bash` or `read` output passes the harness's own limit, so
the harness writes `details.truncation`; assert `truncated == Some(true)`
through the adapter.

## Investigation log

### Q: Does the frozen Pi session exercise the Pi path?

- Sources examined: the frozen Pi 0.80.2 session; the U3 report.
- Findings: no Pi tool result carries the flag, including the 70,000-byte
  result.
- Missing evidence: a harness-produced Pi record with the flag.
- Conclusion: unresolved, needs a Pi session whose output passes the
  harness's limit.

### Q: Does U4's move preserve both reads?

- Sources examined: the U4 diff of `tool_outcome.rs`; `tool_vocabulary.rs:212-218`.
- Findings: the two inline reads became one query with the same pointers and
  the same `as_bool`.
- Missing evidence: none.
- Conclusion: resolved with answer.
