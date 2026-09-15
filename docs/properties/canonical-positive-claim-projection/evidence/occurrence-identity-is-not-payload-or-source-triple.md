# occurrence-identity-is-not-payload-or-source-triple

## Discovery trigger

Specification 'Canonical facts and identity': occurrence identity preserves class, object identity, revision, representation, and span; payload identity never collapses occurrences.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `load_occurrences` encodes each (class, representation) with `encode_preserving_span` and `span: None`, looks up the descriptor by `descriptor_object_id(lineage_id, revision)` through `load_descriptor`, which applies `Descriptors::LiveAtEnd.predicate`, the export's own liveness rule, and re-encodes the stored detail through `reencoded_identity`; a missing descriptor is `RepresentationExclusion::NoDescriptor`. The detail's `lineage_id`, `evidence_id`, `artifact_digest`, and `payload_id` must equal the joined registry, observation, and evidence columns; the registry and observation timestamps, the registry `source_kind` and the class, the registry `source_revision` and the claim's revision, and the observation and registry `sensitivity_class` must agree, or the row is `CorruptCanonicalRow`, the same gate `live_source_descriptors` and `source_export::preflight` apply.
- `crates/kernel/src/source_identity.rs`: tuple encoding and `identity_digest`; `finish` hashes the span into the lineage id, so a partial-span descriptor has an object id the reader cannot derive from the claim. `crates/daemon/src/claim_sources.rs` publishes claim occurrences with `span: None` only.
- `crates/kernel/tests/kernel_claim_facts.rs`: `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` publishes equal bytes under `canonical_claims/decision_summary` and `promoted_memory/summary`, asserts two occurrences with distinct ids and every field equal to the writer's outcome, and one named exclusion. `occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows` rewrites each of the four detail fields out of band, then the observation `sensitivity_class`, registry `source_revision`, and registry `source_kind` columns, and asserts `CorruptCanonicalRow` for each. `a_partial_span_publication_is_outside_the_whole_buffer_inventory` publishes `decision_summary` over a proper sub-span and asserts `NoDescriptor` until the whole-buffer descriptor is published.

## Failure scenario

Keying the inventory by payload digest would merge `decision_summary` and `promoted_memory/summary` when their bytes match. Reporting the detail's `evidence_id` or `artifact_digest` without comparing them with the joined columns would let a consumer fetch evidence whose liveness the query never judged.

## Timing windows and dependencies

None.

## What a test must construct

Publish two representations under two classes with equal bytes; assert both occurrence ids equal the descriptor outcomes and differ from each other, and the third representation is listed as excluded. Rewrite one detail field at a time to name another evidence row, digest, payload, or lineage, then one guarded column at a time; assert the read fails with `CorruptCanonicalRow` and succeeds again once the row is restored. Publish one representation over a proper sub-span; assert it is `NoDescriptor` and the later whole-buffer publication is the one listed.

## Investigation log

### Q: Should the inventory list partial-span descriptors?

- Sources examined: `finish` in `source_identity.rs`, `descriptor_object_id`, the `object_registry` indexes in `schema.rs`, `ClaimMaterializer` in `crates/daemon/src/claim_sources.rs`.
- Findings: the span is hashed into the lineage id and the identity lives only in the observation payload JSON, so finding a claim's partial-span descriptors means scanning every descriptor of the class at that revision number and decoding each detail. The only production producer publishes `span: None`.
- Conclusion: the record covers whole-buffer occurrences; a partial-span inventory needs an identity index first (needs human input if a producer starts publishing spans).
