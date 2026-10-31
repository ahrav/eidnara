# p6-object-applicability-payload-fails-closed

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S3. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`unreadable_payloads_are_uncertain_rather_than_current` (`crates/kernel/tests/kernel_applicability_engine.rs:626`), `unknown_check_kinds_degrade_without_voiding_the_payload` (`:703`), `an_oversized_payload_is_uncertain_without_being_decoded` (`:1832`), and `unknown_payload_fields_are_undecodable_at_any_depth_but_an_unknown_kind_is_not` (`:2703`) construct each shape.

## Failure scenario

An undecodable payload reports `Current` and keeps an attestation served after its bytes change.

## Timing windows and dependencies

None.

## What a test must construct

Each malformed payload shape.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
