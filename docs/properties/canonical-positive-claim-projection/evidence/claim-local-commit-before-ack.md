# claim-local-commit-before-ack

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/search_catchup.rs` - `crash_cuts_recover_to_the_ledger_after_two_reopens_and_never_acknowledge_early`, `released_before_every_acknowledgement`.

## Failure scenario

An acknowledgement ahead of durable local progress loses rows on restart.

## Timing windows and dependencies

The window between local commit and acknowledgement.

## What a test must construct

A process kill between the two.

## Investigation log

No open questions at authoring time.
