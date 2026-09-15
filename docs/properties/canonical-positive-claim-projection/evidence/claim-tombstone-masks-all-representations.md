# claim-tombstone-masks-all-representations

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` and `correction_retirement_and_replay_cannot_resurrect_stale_rows`; `crates/daemon/tests/embedding_publication.rs` - `the_projection_itself_obsoletes_tombstoned_or_replaced_inputs`.

## Failure scenario

A lexical or vector candidate of a retired claim keeps ranking after the kernel withdrew it.

## Timing windows and dependencies

The window between a kernel correction and the projection's catch-up.

## What a test must construct

A corrected and a retired decision with three and two representations; a projection frozen before catch-up.

## Investigation log

No open questions at authoring time.
