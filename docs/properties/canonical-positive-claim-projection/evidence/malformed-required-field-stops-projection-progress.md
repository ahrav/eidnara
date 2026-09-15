# malformed-required-field-stops-projection-progress

## Discovery trigger

Specification 'Projection, progress and recovery': missing or malformed required canonical data quarantines the row and stops checkpoint advancement; Unknown causal metadata is the only exception.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `interpret_admission` and `sensitivity_field` map any unrecognized enum or class value to `ClaimFactsError::MalformedRequiredField`, `load_decision` compares the decision row's stored class text and creation commit with the registry row's and maps a disagreement to `CorruptCanonicalRow`, then decodes the agreed class strictly so an unreadable registry class is `MalformedRequiredField` rather than the `Secret` default `ObjectRow` carries, and `claim_facts_as_of` fails the whole request.
- `crates/kernel/src/claim_causality.rs`: malformed causal detail maps to `Unknown(Malformed)`, the specified exception.
- `crates/kernel/tests/kernel_claim_facts.rs`: `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` corrupts `sensitivity_class`, `disposition`, and `maturity` in turn, then a decision row's class, and asserts each error. `a_registry_class_this_build_cannot_read_is_an_error_not_a_secret_default` inserts registry rows with an unreadable class and asserts `CorruptCanonicalRow` when the decision row says `secret` and `MalformedRequiredField` when it repeats the unreadable value.

## Failure scenario

Treating an unreadable maturity as `Candidate` or skipping the claim would let a projection advance past a row it cannot interpret. Comparing the decision class with the registry's decoded class would let an unreadable registry value pass as `Secret` whenever the decision row happens to say `secret`.

## Timing windows and dependencies

None.

## What a test must construct

Corrupt one enum column out of band and read; assert `Err(MalformedRequiredField)` rather than a claim with defaults. Insert a registry row whose class no build reads alongside a `secret` decision row; assert an error rather than a claim labeled `Secret`.

## Investigation log

### Q: How does the materialization ticket surface this error as a quarantine that stops the checkpoint?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs #460
