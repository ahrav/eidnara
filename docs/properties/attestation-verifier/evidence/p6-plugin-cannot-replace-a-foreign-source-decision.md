# p6-plugin-cannot-replace-a-foreign-source-decision

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2, S6. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

The route builds the replacement spec with `plan.source_kind` and calls `supersede_decision` (`crates/daemon/src/kernel_routes/commit.rs:1002`); `validate_successor` (`crates/kernel/src/slice/write.rs:1271`) refuses a different source kind or id. `refuse_succession_events` (`crates/kernel/src/admission.rs:2640`) refuses `Correct`, `Replace`, and `ApprovalRevoked` submitted as admission events.

## Failure scenario

A plugin replaces an attestation with model text under the attestation's lineage.

## Timing windows and dependencies

None.

## What a test must construct

A plugin route replacement naming a decision of another source kind.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
