# flt-shrinker-unknown-never-not-reproduced

## Discovery trigger
Parent specification C-DST: "The shrinker never classifies `OutcomeUnknown`
as `NotReproduced`; outstanding replay effects are bounded."

## Evidence trail
- `crates/eval-core/src/shrink.rs:107` `classify_replay` maps every
  `Unknown { reason }` to `CandidateVerdict::Unknown { reason }`.
- `crates/eval-core/src/shrink.rs:287` `ReplayEffects`: `issue` refuses at
  the bound and on a resolved or outstanding key; `retry` keeps the key;
  `cancel` resolves `Unknown { cancelled }`; `outcome` refuses an outstanding
  key.
- `crates/eval-core/src/shrink.rs:592` ddmin shrinks only on `Reproduced`.
- `crates/daemon/examples/eval_runner/shrink.rs:237` `Replayer::replay` issues,
  retries once on an exit before the barrier, resolves, and reads the outcome
  back; `:296` `attempt` cancels on timeout.
- `crates/eval-core/tests/shrink.rs:180` enumerates every reason with an exhaustive match.
- `crates/eval-core/tests/shrink.rs:422` keeps the stubborn element and reports
  `NotEstablished { unknown_candidates }`.
- `crates/eval-core/tests/shrink.rs:647` bound, premature `outcome`, retry then resolve, cancel.
- `crates/daemon/tests/eval_shrink.rs:307` death log holds two deaths per distinct unknown candidate.
- `crates/daemon/tests/eval_shrink.rs:374` a two-second timeout cancels and keeps the element.

## Failure scenario
A replay child is killed by the OOM killer while compiling a candidate; a
shrinker reading "no failure reported" as `NotReproduced` deletes the
elements and certifies a scenario that never reproduced.

## Timing windows and dependencies
The window between spawning the child and reading its barrier line; the
replay timeout bounds it.

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
