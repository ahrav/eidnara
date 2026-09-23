# flt-paired-worlds-shrunk-together

## Discovery trigger
Parent specification C-DST: "both worlds of a pair are shrunk together with
the pair mapping recomputed per candidate."

## Evidence trail
- `crates/eval-core/src/shrink.rs` `History` names the log an event is
  deleted from; the two generated histories share raw ids such as
  `session:session-0:0`.
- `crates/eval-core/src/shrink.rs` `Scenario::without` deletes from the
  named log only.
- `crates/eval-core/src/shrink.rs` `Scenario::compile` calls
  `compile_pair_set` over the candidate's own logs.
- `crates/eval-core/src/shrink.rs` `Driver::replay_candidate`: a compiler
  refusal is `InvalidPair`
  before any replay is issued and consumes no budget.
- `crates/eval-core/tests/shrink.rs` `pair_validity_is_recomputed_and_both_worlds_are_shrunk_together`
  asserts the shared-id deletions, `InvalidPair` iff the compiler refuses, no
  tagged deleted fresh event in any fresh arm, and that natural-fresh
  deletions were tried.
- `crates/eval-core/tests/shrink.rs` `the_shrinker_invariants_hold_under_arbitrary_replay_answers`
  repeats the `InvalidPair` iff refused check under every seed.

## Failure scenario
A deletion keyed by id removes `session:session-0:0` from both histories; the
natural-fresh control loses an event the shrinker never meant to touch and
the minimized pair is not the pair the failure was observed on. A mapping
carried over from the original would let a fresh arm keep an event whose
aged counterpart is gone.

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
