# flt-paired-worlds-shrunk-together

## Discovery trigger
Parent specification C-DST: "both worlds of a pair are shrunk together with
the pair mapping recomputed per candidate."

## Evidence trail
- `crates/eval-core/src/shrink.rs:136` `History` names the log an event is
  deleted from; the two generated histories share raw ids.
- `crates/eval-core/src/shrink.rs:192` `Scenario::without` deletes from the
  named log only.
- `crates/eval-core/src/shrink.rs:211` `Scenario::compile` calls
  `compile_pair_set` over the candidate's own logs.
- `crates/eval-core/src/shrink.rs:498` a compiler refusal is `InvalidPair`
  before any replay is issued and consumes no budget.
- `crates/eval-core/tests/shrink.rs:358` asserts the shared-id deletions, `InvalidPair` iff the
  compiler refuses, and no tagged deleted fresh event in any fresh arm.

## Failure scenario
A deletion keyed by id removes `session:session-0:0` from both histories; the
natural-fresh control loses an event the shrinker never meant to touch and
the minimized pair is not the pair the failure was observed on.

## Timing windows and dependencies
None.

## What a test must construct
A scenario whose two histories share an event id, one deletion per history,
and a per-candidate recompilation check.

## Investigation log
### Q: Must the fresh arm shrink to the natural-fresh prefix when only aged events cause the failure?
- Sources examined: parent #749; `compile_pair_set` in
  `crates/eval-core/src/pairs.rs`.
- Findings: the compiler derives the fresh arm from the natural-fresh history
  and the aged closure; nothing pins a prefix.
- Missing evidence: a maintainer decision.
- Conclusion: unresolved, needs human input.
