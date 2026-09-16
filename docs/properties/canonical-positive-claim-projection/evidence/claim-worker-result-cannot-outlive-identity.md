# claim-worker-result-cannot-outlive-identity

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/embedding_publication.rs` - `canonical_mutations_wait_behind_the_guard_and_stale_inputs_become_obsolete`, `the_projection_itself_obsoletes_tombstoned_or_replaced_inputs`.

## Failure scenario

A vector for a retired revision serves as if current.

## Timing windows and dependencies

A tombstone landing between dispatch and publication.

## What a test must construct

A job dispatched before a correction.

## Investigation log

No open questions at authoring time.
