# flt-shrink-preserves-precise-failure-predicate

## Discovery trigger
Parent specification C-DST: "Shrinking preserves a precise failure predicate
(oracle, checkpoint, profile, witness class); slipped candidates are
rejected; reports state 1-minimality under named transformations." Phase 5
acceptance: "a slipped shrink candidate is rejected".

## Evidence trail
- `crates/eval-core/src/shrink.rs` `FailurePredicate` pins the four fields.
- `crates/eval-core/src/shrink.rs` `classify_replay`: an equal predicate is
  `Reproduced`; any difference is `Slipped { observed }`; `Passed` is
  `NotReproduced`; `Unknown` stays `Unknown`.
- `crates/eval-core/src/shrink.rs` `shrink` replays the original first and
  refuses `OriginalNotReproduced`; ddmin accepts only `Reproduced`.
- `crates/eval-core/src/shrink.rs` `one_minimality` tries every single
  deletion until a full pass rejects them all and names the tried
  transformations; an `Unknown` or an exhausted budget is `NotEstablished`.
- `crates/eval-core/tests/shrink.rs` `shrink_preserves_the_predicate_and_rejects_slipped_candidates`
  asserts six commits remain, every `Slipped` record observed `durable_state`,
  no slipped deletion set was accepted, and the minimized scenario
  re-evaluates to the original predicate.
- `crates/eval-core/tests/shrink.rs` `classify_keeps_unknown_unknown_for_every_reason` mutates each
  predicate field alone and expects `Slipped`.
- `crates/eval-core/tests/shrink.rs` `the_shrinker_invariants_hold_under_arbitrary_replay_answers`
  draws replay answers from the whole outcome vocabulary under six seeds and
  checks that the returned scenario has a recorded `Reproduced` and that
  `OneMinimal` rests on a recorded rejection for every single deletion.
- `crates/daemon/tests/eval_shrink.rs` `a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`
  reproduces the predicate in a fresh process and publishes the witness.

## Failure scenario
A shrinker that accepts any failing candidate deletes the sixth commit, the
class slips to `durable_state`, and the minimized witness documents a
store-refusal defect the campaign never saw. A shrinker that compares only
the class accepts a candidate failing at another cut or under another
profile.

## Timing windows and dependencies
None; the predicate is compared after the replay answers.

## What a test must construct
An oracle whose class depends on a count, an original above the boundary, a
check that every accepted candidate re-evaluates to the pinned predicate, and
a per-field mutation table for the comparator.

## Investigation log
### Q: Is the fault profile in the predicate the run profile digest or a fault-profile label?
- Sources examined: parent #749 C-DST bullet; `RunProfile::digest` in
  `crates/eval-core/src/campaign.rs`; `FaultEpisode` in
  `crates/eval-core/src/fault.rs`.
- Findings: every report carries `profile_digest`; fault episodes are
  elements of the scenario, not a label of the predicate.
- Missing evidence: none.
- Conclusion: resolved with answer - the run profile digest.
### Q: Which transformations does the report name?
- Sources examined: parent #749 shrinking order; `Transformation::ORDER`.
- Findings: a `Scenario` holds fault episodes and events; the other listed
  transformations have no representation yet and are not named.
- Missing evidence: none.
- Conclusion: resolved with answer - only the two that were tried.
