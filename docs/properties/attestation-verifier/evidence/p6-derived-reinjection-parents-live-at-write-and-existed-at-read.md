# p6-derived-reinjection-parents-live-at-write-and-existed-at-read

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S7. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`record_claim_causality` (`crates/kernel/src/claim_causality.rs:218`) validates the parent list, bounded by `MAX_DERIVATION_PARENTS` (`:38`), against live registry rows. `derived_reinjection_rests_on_exact_live_parents` (`crates/kernel/tests/kernel_claim_causality.rs:229`) and `detail_operation_and_parent_list_must_agree_with_the_registry` (`:710`) cover it. A later read uses `parents_existed` (`crates/kernel/src/claim_causality.rs:719`), which requires each parent row to have existed at the requested snapshot with the named revision and ignores later invalidation; `derived_reinjection_rests_on_exact_live_parents` retires a parent after the write and asserts the class stays `DerivedReinjection` (`crates/kernel/tests/kernel_claim_causality.rs:315`).

## Failure scenario

An attestation links to a superseded model decision.

## Timing windows and dependencies

The parent superseded between selection and commit.

## What a test must construct

A parent superseded in an earlier envelope.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
