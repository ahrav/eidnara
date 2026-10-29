# p6-new-attestation-classes-and-cited-fields

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N3. A proposed obligation: `proposed - converts in [#1172](https://github.com/ahrav/eidnara/issues/1172)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N3. [#1172](https://github.com/ahrav/eidnara/issues/1172) lands the code and the check that converts it.

## Failure scenario

No citation event, or a different evidence row, at `Verified`.

## Timing windows and dependencies

See the fault map.

## What a test must construct

F14.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
