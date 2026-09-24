# flt-coverage-witnesses-fire-only-on-observed-behaviour

## Discovery trigger
Parent specification C-DST: "Coverage markers are constant, globally unique
names in an evaluator-owned Rust registry with a uniqueness test and an
env-gated completeness proof."

## Evidence trail
- `crates/eval-core/src/markers.rs` `MARKERS` names the three shrink markers
  and the tests that fire them.
- `crates/daemon/examples/eval_runner/shrink.rs` `run` records each marker
  behind the condition it names.
- `crates/daemon/tests/eval_shrink.rs` `a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`:
  fresh-process and slipped markers fired, unknown asserted absent.
- `crates/daemon/tests/eval_shrink.rs` `a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept`:
  the unknown marker fired beside the slipped one.
- `crates/eval-core/tests/render.rs`
  `the_marker_registry_is_unique_and_incomplete_until_every_marker_fires`
  asserts every marker name is unique.

## Failure scenario
A run with no slipped candidate reports the slipped marker fired and the
coverage signature overstates what the shrink observed.

## Timing windows and dependencies
None.

## What a test must construct
Runs whose reports do and do not contain each condition, each asserting the
absent marker as well as the present one.

## Investigation log
### Q: Is the completeness proof gated for the shrink markers?
- Sources examined: `Coverage::complete`; the eval_shrink tests.
- Findings: `flt_shrink_fresh_process_reproduced` fires on every run that
  reaches the shrinker, so the dying-child run fires all three;
  `a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept`
  asserts `Coverage::complete` over the suite prefix on that run, and the
  clean run asserts the unknown marker absent.
- Missing evidence: none.
- Conclusion: resolved with answer - one run proves completeness.
