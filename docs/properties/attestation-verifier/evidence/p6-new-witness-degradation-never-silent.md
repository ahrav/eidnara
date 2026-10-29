# p6-new-witness-degradation-never-silent

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N4. A proposed obligation: `proposed - converts in [#1172](https://github.com/ahrav/eidnara/issues/1172)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N4. [#1172](https://github.com/ahrav/eidnara/issues/1172) lands the code and the check that converts it.

## Failure scenario

A committed attestation at `Candidate`.

## Timing windows and dependencies

See the fault map.

## What a test must construct

F1.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should repeated degradation stop the task?

- Sources examined: OQ6-11.
- Findings: Undecided.
- Missing evidence: see the conclusion.
- Conclusion: needs human input
