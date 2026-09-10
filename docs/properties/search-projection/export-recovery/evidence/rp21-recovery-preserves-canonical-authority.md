# rp21-recovery-preserves-canonical-authority

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

P, lines 178 and 195-210, requires recovery/rollback to preserve canonical
authority and forbids mutation of truth to match a projection. I, lines 63-65,
repeats that constraint. The security and wildcard passes retain this as an
end-to-end obligation rather than assume a successful rebuild implies it.

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:147-173` returns withheld outcomes
  on unavailable kernel, failed tip/lag reads, or failed visible reads.
- `crates/daemon/src/kernel_routes/eligibility.rs:132-173` evaluates current
  retraction, supersession, revision, scope, visibility, sensitivity, and
  artifact eligibility. The function `judge` is daemon-owned at this HEAD.
- P, lines 100-103, and I, lines 40-42, describe a proposed move into shared
  kernel eligibility. That proposed API is not represented as implemented.
- `crates/kernel/src/envelope.rs:950-965` publishes an empty internal alignment
  rebuild rather than preserving obsolete rows after an empty result.
- `crates/kernel/tests/kernel_outbox.rs:333-414` checks that alignment discard
  preserves exact consumer checkpoints and receipts. Status: unaudited.
- `crates/kernel/tests/support/canonical_state.rs:1-20` supplies a table-wise
  digest with SameRoot/CrossRoot normalization. Reuse requires checking those
  exclusions against this property's allowed control-write boundary.
- `crates/kernel/tests/kernel_proofs/model.rs:183-245` compares clean and
  perturbed histories; it has no search recovery operation. Status: unaudited.
- `crates/kernel/src/outbox.rs:105-110` and `:265-282` emit legitimate control
  changes for lifecycle operations. Whole-database byte equality is therefore
  not a correct oracle for a recovery that legitimately registers a consumer.
- `crates/kernel/src/envelope.rs:395-438` changes domain-name bytes in place
  and emits `operator_remediation` using the unchanged source revision.
  Authorized remediation belongs in the independent canonical-operation ledger;
  recovery must not reverse it to reconstruct old projected bytes.
- The search recovery and egress composition does not exist. `test-only`
  reflects that absent production target, not absence of canonical authority.

## Failure scenario

A valid old projection contains an occurrence that canonical state has since
retracted. Recovery treats its projected eligibility as authoritative or
rewrites canonical revision/disposition to match the older rows. The selector
and local SQLite integrity checks can all pass while canonical truth is lost.

Another path treats an unavailable canonical read as permission to serve the
last projection. The settled plan permits prior compatible state or rebuild,
not invented authorization. This scenario is a claim to test, not an incident
deduced from a removed claim-mirror catalog.

## Timing windows and dependencies

Run the check throughout corruption handling, schema/tokenizer/model/policy/
identity mismatch, interruption, and reopen. Canonical invalidation can occur
after export S but before use; fixed-S export correctness alone is insufficient.
The source-coverage owner owns row mapping. The eligibility owner owns verdict
policy. Recovery orchestration must preserve both authorities, not copy either.

## What a test must construct

1. Maintain a fixture-owned ledger of intended canonical writes and resulting
   source facts, revisions, dispositions, and tombstones.
2. Keep a valid older projection while changing canonical revision/permission.
3. Change each of the five compatibility dimensions in separate scenarios.
4. Fail canonical reads during recovery and a later egress attempt.
5. Compare source facts to the independent ledger after each recovery mutation;
   separately reconcile authorized registration/ack/abandon control records.
6. Check that every used occurrence has canonical authorization and validation.
7. Record `rp21_recovery_stale_authority`, `rp21_recovery_contract_mismatch`, and
   `rp21_recovery_authority_unavailable` from fixture changes and fault injection.
8. Do not let the system under test label its own arbitrary writes legitimate.

## Investigation log

### Q: Which canonical tables and control mutations belong in the oracle?

- Sources examined: P rollback contract; envelope alignment discard; outbox
  control changes and the discard-preservation test.
- Findings: Source truth must not change to repair derived state, but consumer
  lifecycle legitimately writes canonical control/audit state.
- Missing evidence: An explicit table/field inventory and allowed recovery
  control-operation set supplied by kernel/source-coverage owners.
- Conclusion: Needs human input. Compare logical authority, not database bytes
  or an unreviewed list of exclusions generated by the recovery code itself.

### Q: How does recovery call the single eligibility authority?

- Sources examined: Current daemon `judge`; P/I ownership prerequisites;
  canonical memory failure path.
- Findings: A daemon policy implementation exists, while the shared kernel
  module is proposed. No search recovery adapter exists at HEAD.
- Missing evidence: Implemented shared boundary and recovery-to-egress wiring.
- Conclusion: Needs human input. Preserve the existing authority contract and
  the planned ownership handoff without inventing API names or a second policy.

### Q: Who defines the at-rest sensitivity policy for derived artifacts?

- Sources examined: P source/rollback requirements, canonical read and eligibility
  paths, and domain-name remediation above.
- Findings: Egress gates and remediation do not specify the complete at-rest
  policy for export pages, staging, selected search state, or retained old state.
- Missing evidence: Named policy owner and approved treatment of these artifacts,
  including remediated fields only if approved projection consumes them.
- Conclusion: Needs human input. No encryption, retention, permission, or new
  occurrence-generation policy is inferred or introduced by this catalog.
