# eligible-positive-and-unknown-claims-remain-reachable

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` (the quiet and anti claims before their transitions); `crates/retrieval/tests/claims.rs`.

## Failure scenario

An all-Unknown corpus would deliver nothing, failing the useful-path requirement.

## Timing windows and dependencies

None.

## What a test must construct

A served claim with no causality record.

## Investigation log

No open questions at authoring time.
