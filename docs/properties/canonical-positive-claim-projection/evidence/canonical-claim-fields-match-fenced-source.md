# canonical-claim-fields-match-fenced-source

## Discovery trigger

Specification constraint 'Canonical facts and identity' and acceptance A1: the projection oracle must match canonical fields at a fenced snapshot. Ticket #458 assigns the reader side.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `KernelStore::claim_facts_as_of` reads registry, decision, own and lineage admission rows, and served visibility inside one `read_snapshot` transaction bound to `requested`.
- `crates/kernel/src/claim_facts.rs`: `load_admission` selects the latest row at or before the snapshot with the same lineage predicate the serving query uses.
- `crates/kernel/tests/kernel_claim_facts.rs`: `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` compares every own-admission field with the `AdmissionDecision` the writer returned and re-reads the same snapshot after later commits.
- `crates/kernel/src/admission.rs`: `served_classes` folds the cited evidence's current `sensitivity_class` into the served class, and `supporting_approval_valid_sql` requires the approval's cited evidence to read `normal` today; `crates/kernel/src/cas/ingest.rs`: `MergedClassification::apply` rewrites `evidence_meta.sensitivity_class` in place. `served` and `valid_at_snapshot` therefore follow the current classification and are outside the repeatability guarantee.
- `crates/kernel/tests/kernel_claim_facts.rs`: `served_facts_follow_the_cited_evidence_class_read_today` re-ingests the cited artifact as secret after S and asserts that only `served` changes on a reread of S.

## Failure scenario

A reader that selected the newest admission row regardless of snapshot would report a later revocation at an earlier S, or an earlier admission after a revocation, and a projection built from it would disagree with the kernel's own history.

## Timing windows and dependencies

A commit landing between two reads of S; a request for S before the object's creation commit.

## What a test must construct

Write a decision and admission, publish one descriptor, capture S, read facts, commit unrelated and related writes, read S again and assert equality; read S-1 and assert the id is in `missing`. Re-ingest the cited artifact as secret and read S again: the revisioned fields are equal and `served` reports the tightened class.

## Investigation log

### Q: How does the materialization ticket compare exported rows against these facts at fenced S?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs the projection-side oracle from #460
