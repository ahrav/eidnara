# occurrence-identity-is-not-payload-or-source-triple

## Discovery trigger

Specification 'Canonical facts and identity': occurrence identity preserves class, object identity, revision, representation, and span; payload identity never collapses occurrences.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `load_occurrences` encodes each (class, representation) with `encode_preserving_span`, looks up the descriptor by `descriptor_object_id(lineage_id, revision)` through `load_descriptor`, which applies `Descriptors::LiveAtEnd.predicate`, the export's own liveness rule, and re-encodes the stored detail through `reencoded_identity`; a missing descriptor is `RepresentationExclusion::NoDescriptor`. The detail's `lineage_id`, `evidence_id`, `artifact_digest`, and `payload_id` must equal the joined registry, observation, and evidence columns, and the registry and observation timestamps must agree, or the row is `CorruptCanonicalRow`, the same gate `live_source_descriptors` and `source_export::preflight` apply.
- `crates/kernel/src/source_identity.rs`: tuple encoding and `identity_digest`.
- `crates/kernel/tests/kernel_claim_facts.rs`: `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` publishes equal bytes under `canonical_claims/decision_summary` and `promoted_memory/summary`, asserts two occurrences with distinct ids and every field equal to the writer's outcome, and one named exclusion. `occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows` rewrites each of the four detail fields out of band and asserts `CorruptCanonicalRow`.

## Failure scenario

Keying the inventory by payload digest would merge `decision_summary` and `promoted_memory/summary` when their bytes match. Reporting the detail's `evidence_id` or `artifact_digest` without comparing them with the joined columns would let a consumer fetch evidence whose liveness the query never judged.

## Timing windows and dependencies

None.

## What a test must construct

Publish two representations under two classes with equal bytes; assert both occurrence ids equal the descriptor outcomes and differ from each other, and the third representation is listed as excluded. Rewrite one detail field at a time to name another evidence row, digest, payload, or lineage; assert the read fails with `CorruptCanonicalRow` and succeeds again once the payload is restored.

## Investigation log

No open questions at authoring time.
