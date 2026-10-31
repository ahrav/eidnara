# p6-successor-keeps-identity-and-advances-revision

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`validate_successor` (`crates/kernel/src/slice/write.rs:1271`) returns `InvalidInput` when domain, source kind, or source id differ and `Conflict` when the revision does not advance. `corrections_reject_cross_domain_replacements` (`crates/kernel/tests/kernel_slice.rs:1003`) covers the first; `corrections_preserve_old_rows_and_reauthor_observation_dependencies` (`:607`) asserts `Conflict` for the unbumped replacement at `:701` to `:712`.

## Failure scenario

A verifier replaces the model decision under its own source and the lineage changes party.

## Timing windows and dependencies

None.

## What a test must construct

Replacements differing in domain, source kind, source id, and revision.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
