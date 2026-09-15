# claim-replay-preserves-newest-canonical-state

## Discovery trigger

Specification 'Projection, progress and recovery': duplicate commits and older-after-newer replay must yield the same final state as a once-only ordered run.

## Evidence trail

- `crates/kernel/src/claim_causality.rs`: `record_causality_inner` replaces the live record through `correct_observation_inner`, so one record is live per subject and older snapshots keep theirs.
- `crates/kernel/src/envelope.rs`: `commit_prepared_with_writer` returns the stored receipt for a repeated intent without running the operation.
- `crates/kernel/tests/kernel_claim_facts.rs`: `replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown`.

## Failure scenario

A replayed recording commit that re-ran would create a second live record and turn the class `Unknown(Conflicting)`; an older record surviving a newer one would revive a withdrawn class.

## Timing windows and dependencies

Duplicate commit; two records in successive commits.

## What a test must construct

Commit a record twice with one intent, assert `replayed`; record a second class, assert the first is replaced and the earlier snapshot still reads the first.

## Investigation log

No open questions at authoring time.
