# flt-coverage-witnesses-fire-only-on-observed-behaviour

## Discovery trigger
Parent specification C-DST: "Coverage markers are constant, globally unique
names in an evaluator-owned Rust registry with a uniqueness test and an
env-gated completeness proof."

## Evidence trail
- `crates/eval-core/src/markers.rs:12` `MARKERS` names the three shrink
  markers and the tests that fire them.
- `crates/daemon/examples/eval_runner/shrink.rs:432` `run` records each
  marker behind the condition it names.
- `crates/daemon/tests/eval_shrink.rs:175` fresh-process and slipped markers fired, unknown not.
- `crates/daemon/tests/eval_shrink.rs:307` the unknown marker fired.
- `crates/eval-core/tests/render.rs` asserts every marker name is unique.

## Failure scenario
A run with no slipped candidate reports the slipped marker fired and the
coverage signature overstates what the shrink observed.

## Timing windows and dependencies
None.

## What a test must construct
Runs whose reports do and do not contain each condition.

## Investigation log
### Q: Is the completeness proof gated for the shrink markers?
- Sources examined: `Coverage::complete`; the eval_shrink tests.
- Findings: the three markers fire across two tests; no single run fires all
  three, so `complete` is not asserted per run.
- Missing evidence: none.
- Conclusion: resolved with answer - union across the suite.
