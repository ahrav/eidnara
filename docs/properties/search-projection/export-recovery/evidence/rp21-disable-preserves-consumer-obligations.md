# rp21-disable-preserves-consumer-obligations

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

P, line 116, distinguishes pause, deregister, and audited abandonment, choosing
deregistration as default disable with retrieval unavailable until recovery.
The failure/lifecycle passes check whether this transition can happen while
lagging. D R6 is a lead; its prose is not imported as an implemented contract.

## Evidence trail

- `crates/kernel/src/outbox.rs:133-135` returns `ConsumerPending` when the
  checkpoint is below the pre-operation tip. This is not a silent removal.
- `crates/kernel/src/outbox.rs:137-156` completes satisfied barriers before
  deleting a caught-up consumer and emitting its control change.
- `crates/kernel/src/outbox.rs:177-205` validates abandonment facts and an
  optional recorded barrier; `:215-283` persists audit before consumer removal.
- `crates/kernel/src/outbox.rs:578-588` refuses prune when no consumers remain.
  Removing one consumer changes the minimum; removing the last does not create
  an unconstrained pruning horizon.
- `crates/kernel/src/facts.rs:203-223` counts published positions after the
  minimum checkpoint and ages all unconsumed rows, including unpublished rows.
- `crates/daemon/src/kernel_routes/serving.rs:41-63` distinguishes no-consumer
  gated reads from daemon-internal direct canonical tip reads.
- `crates/kernel/tests/kernel_outbox.rs:247-330` checks pending refusal and
  durable abandonment facts. Status: unaudited.
- `crates/kernel/tests/kernel_deletion.rs:212-292` checks recorded-consumer
  barriers and explicit empty-set abandonment. Status: unaudited.
- No retrieval disable/pause/recovery driver exists. The subject is its
  proposed orchestration, so the record is `test-only`, not a kernel-only claim.

## Failure scenario

A lag threshold triggers disable. Deregistration returns `ConsumerPending`.
If the daemon ignores the error, it can claim release while still pinning
history and degrading gated reads. If it advances ack without applying work
or silently substitutes abandonment, it destroys the retention obligation.

Another error treats zero consumers as fresh on every surface. Existing code
permits direct tip reads without a consumer but reports gated reads unavailable.
Neither blanket availability nor blanket unavailability matches that behavior.

## Timing windows and dependencies

Exercise caught-up and pending removal, and a new commit between a prior tip
read and the deregistration transaction. Kernel checks its own pre-operation
tip, so an earlier observation is insufficient. Pause keeps registration and
checkpoint; it is not an abandonment. An old deletion barrier keeps its recorded
consumer identity even if a replacement consumer is registered later.

## What a test must construct

1. Request disable with pending work and observe durable consumer state/error.
2. Separately remove a caught-up last consumer and a non-last consumer.
3. Verify retrieval stays unavailable; compare gated and direct tip surfaces.
4. Request authorized abandonment with a recorded incomplete deletion barrier.
5. Reopen after lifecycle interruption and reconcile durable audit/checkpoint
   state before any retry or enablement.
6. Record `rp21_disable_pending_consumer`, `rp21_disable_last_consumer`, and
   `rp21_abandon_request_with_barrier` independently of transition success.
7. Reuse the transform catalog's withheld-versus-empty property and its real
   daemon test at `crates/daemon/tests/transform_canonical_memory.rs:230-302`.

## Investigation log

### Q: How does default disable resolve a lagging consumer?

- Sources examined: P line 116; D R6; kernel deregistration/pruning above.
- Findings: Default deregistration cannot succeed while pending. D's suggestion
  that removal lets pruning resume needs qualification for the last consumer.
- Missing evidence: Chosen bounded drain/recovery policy when work cannot finish.
- Conclusion: Needs human input. Keep retrieval disabled and report the pending
  obligation; do not invent ack progress or silently authorize abandonment.

### Q: Who authorizes re-enable, and what survives restart?

- Sources examined: P state/error/rollback clauses and existing lifecycle APIs.
- Findings: P requires operator recovery; no durable retrieval-disabled state
  or recovery authorization boundary exists at HEAD.
- Missing evidence: State owner, restart representation, and explicit recovery
  transition consistent with canonical consumer audit.
- Conclusion: Needs human input from the daemon lifecycle owner.

Bounded progress after authorization belongs to
[rp21-catchup-and-authorized-recovery-converge](../catalog.md#rp21-catchup-and-authorized-recovery-converge).
It requires every prerequisite/gate accepted and explicit authorization before
admission. It does not weaken this record's prohibition on automatic enablement.
