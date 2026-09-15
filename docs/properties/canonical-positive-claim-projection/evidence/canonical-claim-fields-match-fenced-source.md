# canonical-claim-fields-match-fenced-source

## Discovery trigger

Specification constraint 'Canonical facts and identity' and acceptance A1: the projection oracle must match canonical fields at a fenced snapshot. Ticket #458 assigns the reader side.

## Evidence trail

- `crates/kernel/src/claim_facts.rs`: `KernelStore::claim_facts_as_of` reads registry, decision, own and lineage admission rows, and served visibility inside one `read_snapshot` transaction bound to `requested`.
- `crates/kernel/src/claim_facts.rs`: `load_admission` selects the latest row at or before the snapshot with the same lineage predicate the serving query uses.
- `crates/kernel/tests/kernel_claim_facts.rs`: `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` compares every own-admission field with the `AdmissionDecision` the writer returned and re-reads the same snapshot after later commits.

## Failure scenario

A reader that selected the newest admission row regardless of snapshot would report a later revocation at an earlier S, or an earlier admission after a revocation, and a projection built from it would disagree with the kernel's own history.

## Timing windows and dependencies

A commit landing between two reads of S; a request for S before the object's creation commit.

## What a test must construct

Write a decision and admission, publish one descriptor, capture S, read facts, commit unrelated and related writes, read S again and assert equality; read S-1 and assert the id is in `missing`.

## Investigation log

### Q: How does the materialization ticket compare exported rows against these facts at fenced S?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: see the evidence trail.
- Missing evidence: the construction named under 'What a test must construct' where it is not yet in the tree.
- Conclusion: unresolved, needs the projection-side oracle from #460
