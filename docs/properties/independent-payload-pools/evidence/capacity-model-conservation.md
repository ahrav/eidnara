# capacity-model-conservation

Status: invalidated. No current witness exercises this model-only claim.

## Discovery trigger

The original catalog recorded conservation self-checks for an uncalibrated
capacity research model. The model was sizing input, not a test of the
production transport.

## Evidence trail

- Commit `82a734e3` introduced the model and its catalog record.
- Commit `9d9b97b9` removed the model executable as part of the requested
  performance and research cleanup.
- Review of `df49b73e` found that the record still claimed `Exercised: yes`
  and that the results page still gave a command for the deleted executable.
- This correction invalidates the record, removes the leftover results page,
  and retires its situation marker and R12 completion status.

The previous self-check result is historical. It is not a passing check at
HEAD. No replacement model or model check is present.

## Failure scenario

A reader treats historical sizing output as a current executable guarantee.

## Timing windows and dependencies

None; the model no longer runs in this repository.

## What a test must construct

No current test must construct a model run. `model.self_checks_run` is retired,
not a required situation marker. The old `always` condition remains in the
invalidated catalog record for traceability only.

Production transport conservation belongs to the separate active
`class-allocation-conservation` record. Its ring tests are unchanged by this
retirement; they do not reactivate the model-only claim.

## Investigation log

### Q: Is the model claim exercised at HEAD?

- Sources examined: the catalog, fault map, requirement matrix, and Git history.
- Findings: the model executable was removed, but its active status survived.
- Missing evidence: no current model run exists; none is claimed or required.
- Conclusion: retire the model-only claim and its completion assertions.
