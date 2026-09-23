# wit-live-evidence-never-replayable

## Discovery trigger
Parent specification: "Live runs are labelled non-replayable"; ticket #765:
"Live-model evidence is not relabelled replayable."

## Evidence trail
- `crates/eval-core/src/witness.rs:125` the refusal.
- `crates/eval-core/tests/witness.rs:227` a live package with `replayable: true` is refused and admitted
  once `replayable` is false.

## Failure scenario
A live-slice failure is packaged as a replayable witness and cited as
deterministic evidence of a reasoning defect.

## Timing windows and dependencies
None.

## What a test must construct
A package with `slice: live`.

## Investigation log
### Q: None.
- Sources examined: the serializer.
- Findings: a single refusal covers the property.
- Missing evidence: none.
- Conclusion: resolved with answer.
