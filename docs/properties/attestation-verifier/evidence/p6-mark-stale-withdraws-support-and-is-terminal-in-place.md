# p6-mark-stale-withdraws-support-and-is-terminal-in-place

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`event_effect` (`crates/kernel/src/admission.rs:4245`) returns a disposition only for the disposition events; the comment above it states that a `None` disposition leaves the prior one in place. `an_event_that_names_a_disposition_keeps_its_own_outcome` (`:5437`) pins the `MarkStale` row. `same_envelope_subject_decision_after_admission_uses_the_recorded_prior` (`crates/kernel/tests/kernel_admission.rs:1047`) applies `MarkStale` after an admission in one envelope; the specification's `:1070` is a line inside it. `dispositions_and_sensitivity_only_reduce_visibility` (`:2735`) and `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition` (`crates/daemon/tests/kernel_routes.rs:4455`) cover the route.

## Failure scenario

A stale attestation is re-promoted by a later witness and served again.

## Timing windows and dependencies

A witness event after `MarkStale` in the same or a later envelope.

## What a test must construct

`MarkStale` then a validated witness on one subject.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: After `MarkStale`, does a validated `CodeObserved` leave effective maturity and visibility unchanged?

- Sources examined: `event_effect` and the serving fold; refinement R2.
- Findings: The disposition half holds from `event_effect`. No test in this part's inventory asserts the maturity and visibility after that sequence.
- Missing evidence: see the conclusion.
- Conclusion: unresolved, needs a Kernel test that applies `MarkStale` then a validated `CodeObserved` and reads effective maturity and visibility.
