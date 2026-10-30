# p6-new-verifier-input-anchor-and-document-citation

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N1. A proposed obligation: `proposed - converts in [#1171](https://github.com/ahrav/eidnara/issues/1171)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N1. [#1171](https://github.com/ahrav/eidnara/issues/1171) lands the code and the check that converts it.

## Failure scenario

A `messages`-citing decision classified verifiable.

## Timing windows and dependencies

See the fault map.

## What a test must construct

A `messages`-only citation, no anchor, an `Exact` anchor, and a retired descriptor.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is `ReachableBetween.start_oid` a legal input?

- Sources examined: OQ6-10 of the specification.
- Findings: Undecided.
- Missing evidence: see the conclusion.
- Conclusion: unresolved, needs the #1173 decision on OQ6-10.
