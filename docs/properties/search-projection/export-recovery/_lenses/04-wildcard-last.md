# Wildcard passes, last

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
User-supplied evidence scope: plan, linked parent/index/research, and local repo.
No incident logs are supplied. These passes follow the named model and property
passes and the existing-catalog assessment. They are not independent evidence.

## System-model wildcard

1. An outbox retention checkpoint is not an artifact pin. Artifact GC consults
   evidence references, capture pins, and reservations
   (`crates/kernel/src/cas/gc.rs:437-534`). The proposed export fence must prove
   coverage or abort when source bytes disappear, including an explicit purge.
2. A complete canonical commit can have no outbox rows. Existing tests ack an
   empty commit (`crates/kernel/tests/kernel_outbox.rs:114-121`). Exhausting a
   row page cannot identify the full commit-log tip by itself.
3. Deletion barriers capture the consumer set when recorded
   (`crates/kernel/src/cas/deletion.rs:1002-1017`). A replacement consumer
   registered later is not automatically the old consumer's obligation owner.
4. The current history reader masks invalidation after S
   (`crates/kernel/src/envelope.rs:699-711`), whereas the live-only reader omits
   invalidated rows. Rebuild tombstones cannot be validated by a row-count-only
   comparison or by calling the live-only reader as its own oracle.

## Property wildcard

1. A page-size limit can cut one large commit before its final ordinal. Repeatedly
   enlarging the limit violates a hard cap; repeatedly reading the same prefix
   makes no progress. Keep oversize-commit handling unresolved and separately
   observed from oversize-row handling.
2. Ack success may be lost after kernel COMMIT. Count attempts and read stored
   checkpoints; do not require one ack response per durable effect. Conversely,
   a monotonic ack can skip work, so it cannot be the oracle for completeness.
3. A missing projection after pruning starts from a fresh S, not a promise to
   restore bytes deliberately removed by canonical retention. The positive
   convergence case requires source coverage; the loss case must abort safely.
4. A valid old selector is insufficient after a policy/identity change. The
   prior projection is admissible only while compatible and canonically gated.
   Rebuild success never authorizes rewriting canonical facts.
5. The numeric lag thresholds measure serving policy, not a recovery deadline.
   The bounded convergence property remains unexercised until RP2.9 supplies a
   recovery interval, finite input envelope, and attempt/work ceilings.
6. Verification of reused test seams identifies an existing clean/perturbed
   history model (`crates/kernel/tests/kernel_proofs/model.rs:183-245`). Reuse
   it without relabeling clean rollback/reopen as process death; the harness
   expressly excludes that claim at `crates/kernel/tests/kernel_proofs/harness.rs:1-13`.
   Its normalized digest also cannot replace exact export byte/identity checks.

## Synthesis disposition

- Preserve ten distinct obligations: export identity, retention, byte admission,
  complete commits, ack ordering, selection, bounded rebuild, ordinary/authorized
  recovery progress, disable lifecycle, and canonical authority.
- Use independent constant situation markers for each vulnerable window.
- Link exact existing generic generation and canonical-withholding properties.
- Keep source-row mapping and embedding work with their assigned owners.
- Independent central review by `ses_f7623dcccffe3Y09nW2wVoABif` is complete;
  [dispositions](../portfolio-evaluation.md) retain remaining implementation and
  owner questions. This author's corrections are not an independent re-review.
- Central G1 is narrowed to verified in-place domain-name remediation
  (`crates/kernel/src/envelope.rs:395-438`), not an established RP2.1 field
  dependency. Mutable required S bytes must remain valid or cause abort.
- Central G3 reuses the existing CAS child/barrier/kill/reap pattern at
  `crates/kernel/tests/cas_fault_injection.rs:1046-1094`; concrete search hooks
  remain missing. Central R2 names the absent live decoded-heap observer and
  preserves the kernel unsafe-code boundary.
