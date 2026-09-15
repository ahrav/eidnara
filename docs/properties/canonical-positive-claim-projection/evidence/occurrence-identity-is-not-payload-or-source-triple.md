# occurrence-identity-is-not-payload-or-source-triple

## Discovery trigger

Specification 'Canonical facts and identity': occurrence identity preserves class, object identity, revision, representation, and span; payload identity never collapses occurrences.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `load_occurrences` encodes each (class, representation) with `encode_preserving_span`, looks up the descriptor by `descriptor_object_id(lineage_id, revision)` under the export's liveness rule (registry timestamps and live evidence), and re-encodes the stored detail through `reencoded_identity`; a missing descriptor is `RepresentationExclusion::NoDescriptor`.
- `crates/kernel/src/source_identity.rs`: tuple encoding and `identity_digest`.
- `crates/kernel/tests/kernel_claim_facts.rs`: `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` publishes equal bytes under `canonical_claims/decision_summary` and `promoted_memory/summary`, asserts two occurrences with distinct ids and every field equal to the writer's outcome, and one named exclusion.

## Failure scenario

Keying the inventory by payload digest would merge `decision_summary` and `promoted_memory/summary` when their bytes match.

## Timing windows and dependencies

None.

## What a test must construct

Publish two representations under two classes with equal bytes; assert both occurrence ids equal the descriptor outcomes and differ from each other, and the third representation is listed as excluded.

## Investigation log

No open questions at authoring time.
