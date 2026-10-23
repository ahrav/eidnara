# sc-capture-logs-carry-no-text

## Discovery trigger

Part B obligation from #1066 comment 2, constraint C12. Captured tool output
can carry secrets, and logs sit outside every disclosure gate. The U2 ticket
(#1082) adds the closed counter vocabulary and AC18.

## Evidence trail

`crates/daemon/src/source_capture.rs`:

- The `counts!` macro declares `Count`, a closed enum whose `name` is a
  `&'static str` literal per variant.
- `log_line(stage: &'static str, reason: Count, count: usize)` formats
  `daemon: source capture {stage}: {name} ({count})`. `log` is the module's
  only `eprintln!`. Every caller passes a string literal stage.
- `Counters` stores `AtomicU64` values only. `SourceCapture::report` emits each
  `Count` name with its number, `active`, `draining`, and
  `oldest_deferred_age_ms`.
- `kernel_end` maps every `KernelError` to `deadline` or `kernel_error`;
  `BatchRefusal` and `PublishError` values are never formatted.

`crates/daemon/src/lib.rs` - the health report inserts the capture report
under the key `source_capture`.

`crates/daemon/src/harness_sources.rs`, `tool_outcome.rs`, `git_sources.rs`,
and `project_docs.rs` contain no `eprintln!` or logging macro (grep at HEAD).
`impl std::fmt::Debug for SourceUnit` prints sizes instead of text, and also
prints identity values, revision, and role. `AdapterRefusal` variants name
static field names only.

## Failure scenario

An error path formats `{error:?}` on a `PublishError` wrapping a payload, or a
`SourceUnit` debug print reaches stderr, so tool output, a session id, or a
document path lands in a log.

## Timing windows and dependencies

None. Every refusal and Kernel error path is a code path, not a timing window.

## What a test must construct

A sentinel literal in unit text, a session id, and a message id. Drive an
identity conflict, an adapter refusal (a record naming another session whose
id is the sentinel), and a window too large. Assert the report and the health
metrics contain neither the sentinel, the message id prefix, nor the project
digest. Separately, assert every `Count` renders a log line of lowercase
ASCII, digits, and `" :_()"` only.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `the_capture_report_carries_no_unit_text` constructs the three refusals and
  checks the report and `health().metrics`.
- `crates/daemon/src/source_capture.rs` test
  `log_lines_carry_only_fixed_codes_and_counts` renders every `Count`.

## Investigation log

### Q: Does a test capture stderr on the refusal paths?

- Sources examined: both landed tests.
- Findings: the integration test checks the report and health, not stderr.
  The unit test checks `log_line` for every reason with one fixed stage. The
  signature of `log_line` admits only a static stage, a closed reason, and a
  count, so no runtime value other than the count reaches a line.
- Missing evidence: none for the module as written.
- Conclusion: resolved with answer.

### Q: Does the `SourceUnit` debug output meet the contract?

- Sources examined: `impl std::fmt::Debug for SourceUnit`.
- Findings: it omits text but prints identity values, which include session
  and message ids and, for repository documents, `relative_path`. No capture
  log formats a `SourceUnit` at HEAD.
- Missing evidence: none.
- Conclusion: resolved with answer. The impl is not a check for "no paths";
  reported to the lead.

### Q: Are `session.delete` error messages in scope?

- Sources examined: `retire_captured_sources` in `lib.rs`.
- Findings: its error response formats `captured sources not retired:
  {error}` from a `KernelError`. That is a wire response, not a capture log or
  counter; `KernelError` display text was not audited here.
- Missing evidence: an audit of `KernelError` display strings.
- Conclusion: unresolved, needs that audit if responses join the C12 scope.
