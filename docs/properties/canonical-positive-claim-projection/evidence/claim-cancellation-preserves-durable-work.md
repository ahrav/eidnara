# claim-cancellation-preserves-durable-work

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/search_catchup.rs` - `cancellation_at_write_boundaries_never_quarantines_or_acknowledges`, `cancellation_in_the_second_window_preserves_the_first_acknowledged_prefix`.

## Failure scenario

A cancelled request drops durable rows or acknowledges work it never applied.

## Timing windows and dependencies

Cancellation at each `EpisodeEvent` boundary.

## What a test must construct

A cancelled episode at every boundary.

## Investigation log

No open questions at authoring time.
