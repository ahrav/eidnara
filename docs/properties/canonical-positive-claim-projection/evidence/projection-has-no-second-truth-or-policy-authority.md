# projection-has-no-second-truth-or-policy-authority

## Discovery trigger

Specification 'Canonical facts and identity': neither projection nor retrieval may become a second claim database, truth engine, or policy authority. Acceptance A4.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `admission_sql` selects rows through `served_own_decision_sql` and `served_lineage_decision_sql`, wrappers over the digest-guarded serving definitions; `load_admission` copies the columns; `load_served` calls `admission::served_classes`, the same query `visible_as_of` uses; `load_occurrences` applies the export's liveness rule.
- `crates/kernel/tests/kernel_claim_facts.rs`: served visibility is compared with `visible_as_of(ExplicitSearch)` and own admission with the writer's `AdmissionDecision`.

## Failure scenario

A reader that re-ran `evaluate_admission` on copied inputs would drift from stored decisions whenever the policy table changed.

## Timing windows and dependencies

None.

## What a test must construct

Compare reader output with an independent kernel read at the same snapshot.

## Investigation log

### Q: Where does the A4 production identifier check live?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs a repository-level test for deleted claim machinery names
