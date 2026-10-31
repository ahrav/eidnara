# p6-object-subject-provenance-passes-trusted-classes

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`validate_provenance` (`crates/kernel/src/admission.rs:2650`) accepts `None` unconditionally, refuses explicit-user classes for `repository` provenance, and refuses explicit-user and trusted classes for `unclassified` provenance. `record_admission` calls it at `:1622`.

## Failure scenario

An attestation subject carries `unclassified` provenance and the trusted admission is refused.

## Timing windows and dependencies

None.

## What a test must construct

A decision subject without provenance admitted under trusted classes.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
