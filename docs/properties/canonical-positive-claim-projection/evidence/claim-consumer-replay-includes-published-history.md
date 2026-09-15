# claim-consumer-replay-includes-published-history

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `lost_and_skipped_acknowledgements_replay_from_receipts`, `unresolved_acknowledgement_blocks_and_the_next_episode_recovers`; `crates/daemon/tests/search_catchup.rs` - `lost_ack_with_cancelled_reconciliation_keeps_unknown_outcome_and_local_prefix`.

## Failure scenario

A replay that skips already-published history leaves a projection missing rows it acknowledged.

## Timing windows and dependencies

A lost acknowledgement reply; a page published but never acknowledged.

## What a test must construct

The `EpisodeFault` variants of the materializer.

## Investigation log

No open questions at authoring time.
