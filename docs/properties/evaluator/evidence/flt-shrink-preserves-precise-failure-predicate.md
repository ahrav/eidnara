# flt-shrink-preserves-precise-failure-predicate

## Discovery trigger
Parent specification C-DST: "Shrinking preserves a precise failure predicate
(oracle, checkpoint, profile, witness class); slipped candidates are
rejected; reports state 1-minimality under named transformations." Phase 5
acceptance: "a slipped shrink candidate is rejected".

## Evidence trail
- `crates/eval-core/src/shrink.rs:30` `FailurePredicate` pins the four fields.
- `crates/eval-core/src/shrink.rs:107` `classify_replay`: an equal predicate is
  `Reproduced`; any difference is `Slipped { observed }`.
- `crates/eval-core/src/shrink.rs:533` `shrink` replays the original first and
  refuses `OriginalNotReproduced`; ddmin accepts only `Reproduced`.
- `crates/eval-core/src/shrink.rs:644` `one_minimality` tries every single
  deletion until a full pass rejects them all and names the tried
  transformations.
- `crates/eval-core/tests/shrink.rs:234` asserts six commits remain, every `Slipped` record observed
  `durable_state`, no slipped deletion set was accepted, and the minimized
  scenario re-evaluates to the original predicate.
- `crates/eval-core/tests/shrink.rs:573` runs the invariants under arbitrary replay answers over six
  seeds.
- `crates/daemon/tests/eval_shrink.rs:175` reproduces the predicate in a fresh process and publishes the
  witness.

## Failure scenario
A shrinker that accepts any failing candidate deletes the sixth commit, the
class slips to `durable_state`, and the minimized witness documents a
store-refusal defect the campaign never saw.

## Timing windows and dependencies
None; the predicate is compared after the replay answers.

## What a test must construct
An oracle whose class depends on a count, an original above the boundary, and
a check that every accepted candidate re-evaluates to the pinned predicate.

## Investigation log
### Q: Is the fault profile in the predicate the run profile digest or a fault-profile label?
- Sources examined: parent #749 C-DST bullet; `RunProfile::digest` in
  `crates/eval-core/src/campaign.rs`.
- Findings: every report carries `profile_digest`; no separate fault-profile
  label exists.
- Missing evidence: none.
- Conclusion: resolved with answer - the run profile digest.
