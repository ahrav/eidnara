# checkout-applicability-is-revalidated-without-relevance-refresh

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/kernel/tests/kernel_applicability_engine.rs`, `crates/kernel/tests/kernel_read_repair.rs`.

## Failure scenario

A claim about code the checkout no longer has is injected as current.

## Timing windows and dependencies

A checkout change between selection and handoff.

## What a test must construct

The kernel applicability engine wired to claim candidates; a git fixture with dirty edits.

## Investigation log

### Q: Which daemon path loads bounded applicability inputs for claim candidates?

- Sources examined: the checks in the evidence trail and the specification text.
- Findings: see the evidence trail.
- Missing evidence: the construction named above where it is not yet in the tree.
- Conclusion: needs human input
