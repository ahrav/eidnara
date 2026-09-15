# served-sensitivity-and-artifact-policy-govern-egress

## Discovery trigger

Specification 'Echo provenance and use authority': preserve served sensitivity, destination restrictions, and artifact egress policy at every use.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `ServedFacts` is built from `ServedRow::visibility` for the three surfaces and the folded served sensitivity.
- `crates/kernel/src/eligibility.rs`: artifact egress is judged by `judge` through `egress_facts_tx`; not part of this reader.
- `crates/kernel/tests/kernel_claim_facts.rs`: served facts compared with `visible_as_of` on all three surfaces, before and after a lineage quarantine, and `ServedStanding` distinguishes a retired object from one never admitted.

## Failure scenario

A reader reporting registry sensitivity instead of the folded served class would understate sensitivity tightened by evidence or trigger classification.

## Timing windows and dependencies

None.

## What a test must construct

Compare `ServedFacts` with the serving route at one snapshot; the artifact-policy half needs the delivery ticket's eligibility path.

## Investigation log

### Q: Which egress facts does the delivery ticket join to these served facts?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs #466
