# p6-new-mismatch-stale-unavailable-silent

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N5. A proposed obligation: `proposed - converts in [#1172](https://github.com/ahrav/eidnara/issues/1172)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N5. [#1172](https://github.com/ahrav/eidnara/issues/1172) lands the code and the check that converts it.

## Failure scenario

An attestation on `Mismatch`, or `MarkStale` on `Unavailable` or on the attestation.

## Timing windows and dependencies

See the fault map.

## What a test must construct

F5 and F6 verdicts driven into the write set.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
