# malformed-required-field-stops-projection-progress

## Discovery trigger

Specification 'Projection, progress and recovery': missing or malformed required canonical data quarantines the row and stops checkpoint advancement; Unknown causal metadata is the only exception.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `interpret_admission` and `sensitivity_field` map any unrecognized enum or class value to `ClaimFactsError::MalformedRequiredField`, `load_decision` maps a decision row disagreeing with its registry row to `CorruptCanonicalRow`, and `claim_facts_as_of` fails the whole request.
- `crates/kernel/src/claim_causality.rs`: malformed causal detail maps to `Unknown(Malformed)`, the specified exception.
- `crates/kernel/tests/kernel_claim_facts.rs`: `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` corrupts `sensitivity_class`, `disposition`, and `maturity` in turn, then a decision row's class, and asserts each error.

## Failure scenario

Treating an unreadable maturity as `Candidate` or skipping the claim would let a projection advance past a row it cannot interpret.

## Timing windows and dependencies

None.

## What a test must construct

Corrupt one enum column out of band and read; assert `Err(MalformedRequiredField)` rather than a claim with defaults.

## Investigation log

### Q: How does the materialization ticket surface this error as a quarantine that stops the checkpoint?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs #460
