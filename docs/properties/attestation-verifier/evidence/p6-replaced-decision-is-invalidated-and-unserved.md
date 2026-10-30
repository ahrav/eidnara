# p6-replaced-decision-is-invalidated-and-unserved

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`supersede_decision` (`crates/kernel/src/admission.rs:1368`) validates the successor and invalidates the predecessor in the same envelope. `supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor` (`crates/kernel/tests/kernel_slice.rs:1096`) and `replacement_preserves_predecessor_history_and_leaves_successor_unadmitted` (`crates/kernel/tests/kernel_admission.rs:393`) cover it.

## Failure scenario

A replaced decision keeps serving.

## Timing windows and dependencies

None.

## What a test must construct

A replacement of a live decision.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
