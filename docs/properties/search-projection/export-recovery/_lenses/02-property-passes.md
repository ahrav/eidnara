# Export and recovery: property-mining passes

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The user-supplied evidence scope and source locators are in
[catalog.md](../catalog.md). These passes use source inspection, not runtime
experiments. Wildcard runs after the named passes.

Independent central review by `ses_f7623dcccffe3Y09nW2wVoABif` is complete.
[Dispositions](../portfolio-evaluation.md) add ordinary/authorized recovery
progress, remediation qualification, and observation/harness corrections.
Those edits are not another independent lens pass by this author.

## Data integrity

Hold S fixed while constructing multiple pages. Compare exact keyed source
records, not counts or the same exporter called twice. Existing decision and
observation reads apply created/invalidation predicates but collect full
vectors (`crates/kernel/src/slice/read.rs:194-215`, `:298-341`). Preserve the
fixed-S exactness lead separately from byte admission and retention continuity.

## Concurrency

Local transaction release must precede every attempted kernel ack acquisition,
not merely its return. Kernel ack has no access to local search durability
(`crates/kernel/src/outbox.rs:530-570`). Do not infer the guarantee from a
monotonic checkpoint. Instrument resource ownership and attempted/committed
ack events separately. Preserve the lock-order record; the other owner keeps
row/checkpoint/job atomicity.

## Failure recovery

U5 specifically deletes the projection after historical outbox pruning. A
replay-only fixture with a full outbox misses the claim. Record bounded
convergence from canonical state, with a finite post-fault target T and RP2.9
parameters still unset. Replacement safety must also hold before convergence.

## Protocol contracts

`pending_outbox` can stop mid-commit and excludes published rows
(`crates/kernel/src/outbox.rs:420-426`, `:451-453`). Consumer ack accepts an
existing committed sequence; it does not verify that the consumer applied all
ordinals. Include published retained rows, empty commits, duplicate delivery,
and a commit larger than a batch in the complete-prefix lead. Do not confuse
publication position with commit sequence.
Include `operator_remediation`: its domain-name update is in place with
unchanged source revision (`crates/kernel/src/envelope.rs:395-438`). Whether
export consumes that field remains a source-mapping question; required S bytes
must be supplied or the attempt aborted without reversing canonical remediation.

## Resource boundaries

Encoded payload admission must precede materialization. Existing size lookup
reads SQL `length(decision_payload)` (`crates/kernel/src/slice/read.rs:254-270`),
but row loaders allocate bytes before JSON parsing (`:275-295`, `:315-340`).
Require independent decode-entry and high-water observations. SQL cache,
encoded bytes, decoded heap bytes, and local-transaction bytes are distinct
charges. Numeric caps and decoded-memory accounting remain RP2.9 decisions.

## Security boundaries

Recovery may replace derived state, not rewrite canonical truth to match it.
An obsolete schema/tokenizer/model/policy/identity bundle must not regain
authority by falling back to a stale projection. The existing canonical reader
returns a withheld verdict on store or lag failure
(`crates/daemon/src/canonical_memory.rs:147-173`). Publication validation and
canonical authority therefore remain different properties.

## Distributed coordination

No multi-node consensus or remote owner transfer is in this surface. Skip
Raft/quorum properties. The relevant coordination is a single daemon across
two local SQLite durability domains plus filesystem selector publication.
Do not introduce a distributed transaction or another queue.

## Lifecycle transitions

Registration, pause, safe deregistration, and audited abandonment have different
retention consequences. `ConsumerPending` blocks lagging deregistration
(`crates/kernel/src/outbox.rs:133-135`); audit insertion precedes consumer
deletion on abandonment (`:215-264`). Default disable cannot quietly bypass
this restriction. This is a contract integration gap, not an observed defect
in a projection supervisor that does not exist.

## Idempotency and replay

Publisher retry after pruning has a durable publication watermark
(`crates/kernel/src/outbox.rs:480-517`); that is not a local projection replay
receipt. Consumer replay needs a complete-prefix target keyed by commit and
ordinal. Repeated ack is permitted, and unknown outcome must be reconciled
from stored checkpoint rather than return values alone. Projection row/job
deduplication belongs to the other discovery owner.

## Version compatibility

RP2.1 R5 requires rebuild on five contract mismatches. Host payload generation
validation is an existing lower mechanism (`crates/host-runtime/src/generation.rs:473-546`),
not a search contract validator. Reuse the generic selector-validity property;
catalog only search-specific completeness/compatibility and authority deltas.
No predecessor migration or wire-name change is proposed.
