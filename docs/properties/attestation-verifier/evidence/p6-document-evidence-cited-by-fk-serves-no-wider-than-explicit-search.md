# p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S5, S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

Source capture publishes through a `SourcePublisher` with `sensitivity: Sensitivity::Sensitive` (`crates/daemon/src/source_capture.rs:922`). The slice fold raises a citing row to its evidence's class (`a_decision_citing_evidence_is_classified_no_lower_than_that_evidence`, `crates/kernel/tests/kernel_slice.rs:1454`; `a_decision_serves_no_lower_than_its_cited_evidence_reads_today`, `:1527`). `validate_trigger` (`crates/kernel/src/admission.rs:2807`) folds backing evidence into the trigger class, and `surface_visibility` (`:4221`) hides `Sensitive` on the automatic surfaces. The specification cited `source_capture_tests.rs:1962-1979`, lines that now begin at `:1990`.

## Failure scenario

A `Verified` attestation is hidden on `AutoInject` because its witness cites document evidence.

## Timing windows and dependencies

None.

## What a test must construct

A trusted subject promoted through a trigger whose observation cites document evidence.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
