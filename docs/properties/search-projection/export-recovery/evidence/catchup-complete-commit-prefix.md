# catchup-complete-commit-prefix

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

The protocol and replay passes distinguish consumer catch-up from global
publication. P KTD2 requires complete commits after S before publication.
P U5 requires catch-up of concurrent commits before coverage comparison.
Neither requirement can be proved from a monotonic consumer ack alone.

## Evidence trail

- `crates/kernel/src/outbox.rs:28-43` separates outbox position, commit
  sequence, ordinal, and the complete-commit boundary flag.
- `crates/kernel/src/outbox.rs:420-426` documents that a limited publisher
  batch can end mid-commit, even when it contains no complete boundary.
- `crates/kernel/src/outbox.rs:442-453` computes the last row against all
  stored rows but returns only globally unpublished rows.
- `crates/kernel/src/outbox.rs:499-517` checks a publication boundary for
  new positions and persists an independent publication watermark.
- `crates/kernel/src/outbox.rs:551-570` accepts a consumer ack for an
  existing committed sequence; it has no applied-ordinal input.
- `crates/kernel/tests/kernel_outbox.rs:114-121` explicitly acknowledges
  a commit that contains no outbox events. Status: unaudited.
- `crates/kernel/tests/kernel_outbox.rs:622-698` checks a split publisher
  batch and the disappearance of marked rows from `pending_outbox`.
- `crates/kernel/src/envelope.rs:395-438` emits `operator_remediation` after
  an in-place domain-name rewrite without changing source revision. Required
  control-event accounting cannot rely on revision increments alone.
- Source search finds no consumer-specific catch-up reader or daemon driver.
  The new complete-prefix record is `test-only` because that path is absent.

## Failure scenario

A commit has more rows than one batch. The driver applies the first batch
and acknowledges its commit sequence even though later ordinals are missing.
Kernel accepts that existing sequence, and pruning can erase the remainder.
This is a proposed driver defect, not a defect demonstrated in kernel ack.

Alternatively, another publisher marks retained rows before a new consumer
reads them. `pending_outbox` no longer returns those rows. Treating an empty
publisher result as caught up can skip required work while all ack guards pass.
Empty commits are the opposite case: progress exists without any event row.

## Timing windows and dependencies

Hold a target T and ledger of canonical commits. Stage partial commit work
without advancing the durable prefix until all required ordinals complete.
The projection-row owner defines atomic application and replay identity.
This record defines which complete history the local checkpoint may represent.
Outbox publication position and consumer commit sequence must not be compared
as if they used the same units. The ack-order record consumes this prefix.

## What a test must construct

1. Create multi-row canonical commits spanning a declared batch boundary.
2. Mark some post-S rows published while retaining them for the consumer.
3. Include an empty commit at or before T, deletion/control events, and
   `operator_remediation`. Projection effects for the latter depend on whether
   approved mapping consumes domain names; the event still belongs in the ledger.
4. Repeat a delivered batch and interrupt between two parts of one commit.
5. Compare applied logical commit/ordinal history with an independent ledger.
6. Require no durable prefix advancement on a partial commit or read failure.
7. Record `search_projection_catchup_split_commit`, `search_projection_catchup_published_retained`, and
   `search_projection_catchup_empty_commit` from inputs/delivery, independently of success.
   The shared `search_projection_export_operator_remediation_during_snapshot` witness
   constructs a remediation event for later catch-up without assuming its
   field is projected or mandating a new occurrence generation.

## Investigation log

### Q: Does the existing pending reader supply consumer replay after S?

- Sources examined: pending SQL, publication watermark, consumer ack, tests above.
- Findings: It filters on publication, takes no consumer/cursor argument, and
  does not expose commit-log-only entries. Those facts discriminate it from
  the proposed complete-history reader even if both return outbox rows.
- Missing evidence: Bounded retained-history admission and empty-commit handling.
- Conclusion: Needs human input for the integration contract; reuse the kernel
  ownership and decoding rather than assume a proposed API already exists.

### Q: What happens when one commit exceeds an approved batch or transaction cap?

- Sources examined: P KTD2 and Bounds; pending-reader documentation.
- Findings: The publisher documentation suggests a larger limit when no complete
  boundary fits. An unbounded increase cannot satisfy RP2.1 hard byte caps.
- Missing evidence: A bounded accumulation/admission or explicit failure policy
  consistent with local atomic application and complete-prefix progress.
- Conclusion: Needs human input from kernel, projection-row, and RP2.9 owners.
