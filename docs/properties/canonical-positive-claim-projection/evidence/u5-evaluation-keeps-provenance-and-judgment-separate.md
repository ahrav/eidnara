# u5-evaluation-keeps-provenance-and-judgment-separate

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`; `crates/retrieval/tests/claims.rs` - `causal_class_changes_no_state`.

## Failure scenario

Provenance leaks into relevance or authorization.

## Timing windows and dependencies

None.

## What a test must construct

A genuine and an Unknown claim with equal policy.

## Investigation log

### Q: How are relevance judgments and external-application outcomes recorded beside these counts?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: unresolved, needs the RP2.9 accounting protocol
