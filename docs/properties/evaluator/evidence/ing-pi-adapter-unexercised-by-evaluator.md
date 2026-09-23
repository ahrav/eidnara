# ing-pi-adapter-unexercised-by-evaluator

## Discovery trigger
Ticket #765: promote the executed properties "including ingestion
reachability gaps, the Pi coverage gap, and coverage witnesses".

## Evidence trail
- `crates/daemon/src/harness_sources.rs:314` `pi_units` is the Pi adapter.
- `crates/daemon/examples/eval_runner/campaign.rs` renders one OpenCode
  session per arm; no Pi session is rendered anywhere under
  `crates/daemon/examples/eval_runner/` or `crates/daemon/tests/eval_*.rs`.
- `docs/evaluator.md` mentions the Pi runner only for the cassette oracle's
  JSONL reader.

## Failure scenario
Pi sessions age differently from OpenCode sessions and the evaluator's
claims are read as covering both.

## Timing windows and dependencies
None.

## What a test must construct
A rendered Pi session ingested through `pi_units` with adapter accounting.

## Investigation log
### Q: Does any evaluator path reach `pi_units`?
- Sources examined: grep over the shells and evaluator tests.
- Findings: none.
- Missing evidence: a Pi arm.
- Conclusion: unresolved, needs human input on scope.
