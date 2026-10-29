# p6-direct-observation-requires-exact-evidence-digest

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S7. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`direct_observation_rests_on_live_exact_evidence` (`crates/kernel/tests/kernel_claim_causality.rs:42`) and `forged_records_and_copied_strings_grant_nothing` (`:364`) construct the cases against `record_claim_causality` (`crates/kernel/src/claim_causality.rs:218`).

## Failure scenario

A causality record claims direct observation of evidence it does not hold.

## Timing windows and dependencies

None.

## What a test must construct

Direct observations over invalidated and mismatched evidence.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
