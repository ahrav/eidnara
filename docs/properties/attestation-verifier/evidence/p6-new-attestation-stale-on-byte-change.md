# p6-new-attestation-stale-on-byte-change

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N8. A proposed obligation: `proposed - converts in [#1174](https://github.com/ahrav/eidnara/issues/1174)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N8. [#1174](https://github.com/ahrav/eidnara/issues/1174) lands the code and the check that converts it.

## Failure scenario

Bytes changed at a descendant and `Current` reported because `reachable_from` still holds.

## Timing windows and dependencies

See the fault map.

## What a test must construct

A descendant commit that changes the cited file.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which mechanism carries applicability?

- Sources examined: OQ6-4.
- Findings: Four alternatives; none chosen.
- Missing evidence: see the conclusion.
- Conclusion: needs human input
