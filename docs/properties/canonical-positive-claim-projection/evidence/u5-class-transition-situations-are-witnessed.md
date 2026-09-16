# u5-class-transition-situations-are-witnessed

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`, `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees`.

## Failure scenario

A summary marker counts a cell no test constructed.

## Timing windows and dependencies

Transitions landing between classification and validation.

## What a test must construct

The frozen RP2.9 manifest of cells.

## Investigation log

### Q: Which cells does the RP2.9 manifest require?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: needs human input
