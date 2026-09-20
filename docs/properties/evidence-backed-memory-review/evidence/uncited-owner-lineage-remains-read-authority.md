# uncited-owner-lineage-remains-read-authority

## Discovery trigger

Specification R6 and U1 acceptance: selected reads revalidate the full persisted
cited and uncited policy lineage. Decision record Q25: the union member key
includes owner and owner revision.

## Evidence trail

`crates/daemon/src/memory_reviewer/settlement.rs` -
`read_selected_proposal_inner` calls `dependencies::revalidate` under the review
hold for `ArtifactDestination::Local` after the hold lookup.

`crates/daemon/src/memory_reviewer/dependencies.rs` - `expectation` rebuilds a
canonical member through `resolve_descriptor` and requires the recorded owner
and owner revision to be the live decision's.

`crates/daemon/tests/memory_reviewer_settlement.rs` - tests named in the
catalog record.

## Failure scenario

An uncited source disclosed to the model is retired, or a canonical member's
decision is re-revised, after selection. A read that checks only cited evidence
ids serves the proposal as still supported.

## Timing windows and dependencies

Between selection and any later read; the hold is live throughout.

## What a test must construct

A published proposal over two disclosed sources with one uncited; retire the
uncited source; read and expect `dependency_refused`; assert the receipt is
unchanged. A staged record with a fabricated canonical member; read and expect
the same refusal.

## Investigation log

No open questions at authoring time.
