# flt-shrinker-unknown-never-not-reproduced

## Discovery trigger
Parent specification C-DST: "The shrinker never classifies `OutcomeUnknown`
as `NotReproduced`; outstanding replay effects are bounded." Ticket #765:
"preserve receipt-key recovery semantics through retries and cancellation. A
deliberately premature classification fails."

## Evidence trail
- `crates/eval-core/src/shrink.rs:107` `classify_replay` maps every
  `Unknown { reason }` to `CandidateVerdict::Unknown { reason }`.
- `crates/eval-core/src/shrink.rs:287` `ReplayEffects`: `issue` refuses at
  the bound and on a resolved or outstanding key; `retry` keeps the key;
  `cancel` resolves `Unknown { cancelled }`; `outcome` refuses an outstanding
  key.
- `crates/eval-core/src/shrink.rs:592` ddmin shrinks only on `Reproduced`;
  `one_minimality` counts `Unknown` single deletions and refuses to certify.
- `crates/daemon/examples/eval_runner/shrink.rs` `Replayer::replay` issues,
  retries once on an exit before the barrier, resolves, and reads the outcome
  back through `ReplayEffects::outcome`; `wait_for_barrier` cancels on
  timeout and answers `read_back_failed` on a malformed line.
- `crates/eval-core/tests/shrink.rs` `classify_keeps_unknown_unknown_for_every_reason` enumerates
  every reason with an exhaustive match, so a new variant fails to compile
  there.
- `crates/eval-core/tests/shrink.rs` `an_unknown_replay_is_kept_and_never_becomes_not_reproduced`
  keeps the stubborn element and reports
  `NotEstablished { unknown_candidates }`.
- `crates/eval-core/tests/shrink.rs` `replay_effects_are_bounded_and_a_premature_verdict_is_refused`:
  bound, premature `outcome`, reissue of an outstanding key, retry then
  resolve under the same key, cancel, refusals on resolved and unknown keys.
- `crates/daemon/tests/eval_shrink.rs` `a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept`:
  the death log holds two deaths per distinct unknown candidate.
- `crates/daemon/tests/eval_shrink.rs` `a_child_that_never_answers_is_cancelled_and_unknown`: a
  two-second timeout cancels and keeps the element.

## Failure scenario
A replay child is killed by the OOM killer while compiling a candidate; a
shrinker reading "no failure reported" as `NotReproduced` deletes the
elements and certifies a scenario that never reproduced.

## Timing windows and dependencies
The window between spawning the child and reading its barrier line; the
replay timeout and the profile's elapsed bound both cap it.

## What a test must construct
A spawn function that maps a chosen candidate to a dying or sleeping child,
and an assertion that the affected element survives with a non-`NotReproduced`
verdict.

## Investigation log
### Q: Does the in-core driver need the ledger?
- Sources examined: `shrink` and `Driver` in `crates/eval-core/src/shrink.rs`.
- Findings: the driver calls its replay callback synchronously and never has
  two effects outstanding; the ledger is the shell's contract.
- Missing evidence: none.
- Conclusion: resolved with answer - the shell owns the ledger.
### Q: Does a retry count as a new effect?
- Sources examined: `ReplayEffects::retry`; `Replayer::replay`.
- Findings: the key is kept and the attempt count rises; the resolution is
  the last attempt's answer.
- Missing evidence: none.
- Conclusion: resolved with answer - one effect, several attempts.
