# claim-enablement-requires-approved-evidence

## Discovery trigger

Specification acceptance 'Enablement': current approvals, prerequisites, RP2.9 evidence, and actual backend support are required together.

## Evidence trail

- `crates/daemon/src/projection_gates.rs`: `ProjectionHook` lists the hooks the evidence gate admits; none names claim retrieval or causality.
- `crates/kernel/src/claim_facts.rs` and `crates/kernel/src/claim_causality.rs` have no caller outside `crates/kernel` at authoring time (`rg claim_facts_as_of record_claim_causality crates/`).

## Failure scenario

A daemon route calling the reader or writer without an admitted hook would enable claim behavior on unapproved limits.

## Timing windows and dependencies

None.

## What a test must construct

A repository test that lists callers of the two APIs and asserts each sits behind `HookGate::admit`; cannot be written until a caller exists.

## Investigation log

### Q: Which `ProjectionHook` variant gates claim retrieval and causality writes?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: needs human input
