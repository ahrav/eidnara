# rp21-catchup-and-authorized-recovery-converge

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

Independent central analyst `ses_f7623dcccffe3Y09nW2wVoABif` identifies gaps
G2 and G5: complete-prefix safety does not promise CatchingUp-to-Current
progress, and safe disable does not promise progress after authorized recovery.
This record disposes those findings; it is not an independent review by this
author. It adds one bounded obligation with two explicitly admitted modes.

## Evidence trail

- P, lines 106-110, names FencedBootstrap, CatchingUp, Current, Rebuilding,
  and Disabled, with replay-safe local commit/ack ordering.
- P, line 116, requires operator recovery after default disable. This does
  not authorize automatic re-enable when a timer expires or faults stop.
- P, lines 56-59 and 173-179, keeps hooks gated on coverage, freshness,
  resources, and supported harness behavior. Admission cannot bypass them.
- P, lines 118-121, and L, lines 131-136, leave numerical limits to RP2.9.
  The bound names here are oracle notation, not existing configuration APIs.
- `crates/kernel/src/outbox.rs:530-570` advances a registered consumer's
  checkpoint. It does not run a backlog worker or authorize retrieval recovery.
- `crates/kernel/src/outbox.rs:133-156` refuses pending deregistration. A
  Disabled episode may still own obligations that recovery must reconcile.
- `crates/daemon/src/kernel_routes/serving.rs:41-63` classifies lag/no-consumer
  state; classification alone does not drain work or admit recovery.
- `crates/daemon/tests/transform_canonical_memory.rs:230-302` restores a
  canonical block after a test manually acks. Status: unaudited; it is not
  production catch-up or an authorized Disabled-to-Current controller.
- That production controller and approved bounds are absent at HEAD. Hence
  `Reachability: test-only`, `Exercised: not yet`, and RP2.9-blocked execution.

## Failure scenario

An intact projection safely refuses to advance an incomplete prefix but never
schedules its next batch. It can remain CatchingUp forever while every safety
invariant passes. Deletion-after-pruning is unnecessary to expose this gap.

A Disabled controller records an accepted operator recovery request and passes
every prerequisite gate, but never resumes, registers, or bootstraps. A safety
check requiring continued unavailability before authorization also passes.
Conversely, reaching Current without authorization is not recovery coverage.

## Timing windows and dependencies

For ordinary mode, admit a healthy episode whose durable prefix trails a
finite T. For Disabled mode, record the prior Disabled state, explicit recovery
authorization, and prerequisite/gate acceptances before admission at t0.
Keep dependencies healthy and stop injected faults/new writes for the approved
interval. Freeze t0, T, mode, input envelope, and approved work/attempt caps.
Retries cannot restart the clock or change the target to avoid failure.

The deadlines are `B_catchup_ms` and `B_authorized_recovery_ms`, owned by
RP2.9. Both remain unset. At the applicable bound, require Current, compatible
selected `O(T)`, and complete durable local/reconciled kernel prefixes through T.
Re-registration/bootstrap, when required by the accepted recovery path, count
inside that episode. Accepted gates are prerequisites, not outcomes inferred
from reaching Current. Missing authorization or a failed gate is not admitted.

Source inventory and sweeps, including message cleanup and git work, remain
dependencies of [projection-source-inventory-complete](../../projection-coverage/catalog.md#projection-source-inventory-complete).
This record consumes that coverage contract rather than implement source sweeps.
Ack/lock correctness remains with this part's existing ack-order record.

## What a test must construct

1. Admit ordinary CatchingUp with positive finite lag and independently verified
   healthy dependencies, retained inputs, caps, and all prerequisite gates.
2. Separately reach Disabled, record explicit authorization and gate acceptance,
   and admit a finite recovery target; include registration/bootstrap if needed.
3. Observe selected coverage, durable prefixes, and Current by the applicable
   approved bound. Unavailable forever is not success for an admitted episode.
4. Reference `rp21_healthy_lagged_target_admitted` and
   `rp21_authorized_recovery_current_observed` from the single fault-map
   definition site. The latter positively witnesses authorization followed by
   Current, independently of whether the deadline check passes.
5. Keep unauthorized or gate-rejected recovery unavailable under the lifecycle
   safety record; do not use it to satisfy either positive progress witness.

## Investigation log

### Q: Which bounds make both admitted modes executable?

- Sources examined: P state/error/bound clauses; L protocol/approval requirement.
- Findings: Neither ordinary catch-up nor authorized recovery has an approved
  interval or finite backlog/work envelope. Serving thresholds are not substitutes.
- Missing evidence: RP2.9 per-mode bounds, row/byte/commit/attempt ceilings,
  dependency service assumptions, and the final ack-reconciliation endpoint.
- Conclusion: Needs human input from RP2.9; no fabricated numeric bound is used.

### Q: Which authority admits recovery and persists prerequisite acceptance?

- Sources examined: P operator-recovery and gating clauses; current outbox API.
- Findings: Kernel lifecycle operations exist, but no retrieval authorization/
  gate/admission controller or restart representation exists at HEAD.
- Missing evidence: Explicit recovery authority, durable acceptance evidence,
  and the implemented transition to Current after all prerequisites pass.
- Conclusion: Needs human input from daemon lifecycle/gate owners. This record
  requires that authority and does not confer permission to enable automatically.
