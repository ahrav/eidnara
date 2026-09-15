# unknown-echo-state-is-policy-neutral

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/retrieval/tests/claims.rs` - `causal_class_changes_no_state`; `crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees`.

## Failure scenario

Unknown lineage would earn or lose standing it has no evidence for.

## Timing windows and dependencies

None.

## What a test must construct

Claims with equal facts and different causal classes.

## Investigation log

No open questions at authoring time.
