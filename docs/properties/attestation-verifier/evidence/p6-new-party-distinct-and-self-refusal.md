# p6-new-party-distinct-and-self-refusal

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N6. A proposed obligation: `proposed - converts in [#1172](https://github.com/ahrav/eidnara/issues/1172) (plugin half) and [#1171](https://github.com/ahrav/eidnara/issues/1171) (`self_source` half)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N6. [#1172](https://github.com/ahrav/eidnara/issues/1172) (plugin half) and [#1171](https://github.com/ahrav/eidnara/issues/1171) (`self_source` half) lands the code and the check that converts it.

## Failure scenario

The verifier attests its own attestation.

## Timing windows and dependencies

See the fault map.

## What a test must construct

A store holding an attestation; a plugin write under `verifier`.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does anything beyond the plugin route reserve `verifier`?

- Sources examined: OQ6-15.
- Findings: No; a plugin can still `MarkStale` or retire an attestation (gap G4).
- Missing evidence: see the conclusion.
- Conclusion: needs human input
