# projection-has-no-second-truth-or-policy-authority

## Discovery trigger

Specification 'Canonical facts and identity': neither projection nor retrieval may become a second claim database, truth engine, or policy authority. Acceptance A4.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `admission_sql` selects rows through `served_own_decision_sql` and `served_lineage_decision_sql`, wrappers over the digest-guarded serving definitions; `load_admission` copies the columns; `load_served` calls `admission::served_classes`, the same query `visible_as_of` uses; `load_descriptor` selects descriptors through `Descriptors::LiveAtEnd.predicate`, the export's own liveness predicate, rather than restating it.
- `crates/kernel/tests/kernel_claim_facts.rs`: served visibility is compared with `visible_as_of(ExplicitSearch)` and own admission with the writer's `AdmissionDecision`.

## Failure scenario

A reader that re-ran `evaluate_admission` on copied inputs would drift from stored decisions whenever the policy table changed.

## Timing windows and dependencies

None.

## What a test must construct

Compare reader output with an independent kernel read at the same snapshot.

## Investigation log

### Q: Where does the A4 production identifier check live?

- Sources examined: the files in the evidence trail, the specification text, the ticket bodies, and `.github/workflows/ci.yml`.
- Findings: the `gates` job's `Retired memory-plane identifiers` step greps production content, tracked pathnames, and whole index blobs for the retired names (`claim_mirror`, `claim_operation`, and the rest of its token list) and fails the job on a match; crate `tests/`, `*.test.ts`, `docs/`, `NOTICE`, and `*.md` are excluded.
- Missing evidence: an audit that the token list names every deleted claim-machinery identifier.
- Conclusion: the check exists as a CI gate; its coverage is unaudited.
