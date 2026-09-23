# wit-live-evidence-never-replayable

## Discovery trigger
Parent specification: "Live runs are labelled non-replayable"; ticket #765:
"Live-model evidence is not relabelled replayable."

## Evidence trail
- `crates/eval-core/src/witness.rs` `validate`: `slice == Live && replayable`
  is `LiveRelabelledReplayable`; `serialize` calls `validate` first.
- `crates/eval-core/src/failure_class.rs` `Slice`: "only a live model can be
  blamed", the same label the failure-class table uses.
- `crates/eval-core/tests/witness.rs` `live_model_evidence_is_never_relabelled_replayable`: a live
  package with `replayable: true` is refused by `validate` and by
  `serialize`, and admitted once `replayable` is false.
- The shell always packages `Slice::Cassette` with `replayable: true`; no
  live slice is shrunk today.

## Failure scenario
A live-slice failure is packaged as a replayable witness and cited as
deterministic evidence of a reasoning defect.

## Timing windows and dependencies
None.

## What a test must construct
A package with `slice: live`.

## Investigation log
### Q: Is `replayable` derivable from `slice` and therefore redundant?
- Sources examined: `WitnessPackage`; parent #749 "live runs carry
  `replayable = false`".
- Findings: the parent names the flag on the wire; the refusal keeps the two
  consistent instead of dropping one.
- Missing evidence: none.
- Conclusion: resolved with answer - keep both, refuse the contradiction.
