# claim-rebuild-incremental-parity

## Discovery trigger

Specification section 'Projection, progress and recovery' and acceptance A3 and
Recovery; implementation ticket for claim materialization assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees`; `crates/retrieval/tests/lexical_projection.rs` (rebuild from one snapshot equals incremental) and `crates/daemon/tests/search_replacement/recovery.rs` for the RP2.1 rebuild path.

## Failure scenario

A rebuild that silently classifies a claim differently from the projection it replaces.

## Timing windows and dependencies

None.

## What a test must construct

An incremental projection that has applied a catch-up window and a fresh bootstrap at a later snapshot.

## Investigation log

### Q: Which tombstone and job identities must a claim-specific parity check compare beyond live rows?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: unresolved, needs the normalization rule the specification leaves to the RP2.1 owner
