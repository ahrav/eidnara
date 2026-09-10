# Embedding property lenses

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Sources: [catalog source register](../catalog.md#source-register).
Every proposed guarantee below is a claim under test.

## Data integrity

Separate current identity, durable vector bytes, and product completion.
JobTable only checks local item count and dimension at publication
(`crates/host-runtime/src/synapse/jobs.rs:502-535`). This yields
`embedding-completion-is-identity-fenced` and
`embedding-complete-requires-durable-vector`. One does not imply the other.

## Concurrency

Hold completion after dispatch while revision, model identity, or deletion
changes. Recheck at the product commit boundary, not only before inference.
Race GC selection with dispatch and completion. Preserve this distinct lead as
`embedding-identity-gc-preserves-live-work`; existing ResultLease only protects
in-memory result bytes (`jobs.rs:62-88`).

## Failure recovery

Process restart loses JobTable. A durable row must remain discoverable through
unknown outcomes, expired polls, and retryable failures. Separate the safety
record `embedding-pending-drives-one-job-table` from bounded recovery progress
in `embedding-restart-retries-durable-pending`. A clean route close is not a
process crash (`jobs.rs:323-342`, `:597-606`).

## Protocol contracts

Reuse `synapse-requests-are-validated-before-any-inference` for current wire
validation. Add product exact-token rejection and single-artifact counting.
`docs/host-wire-protocol.md:472` documents silent truncation; RP2.1 does not
authorize a new wire method. The recovery-ledger wording at `:502` also needs
owner reconciliation when the daemon driver lands.

## Resource boundaries

Pending discovery, in-memory admission, running inference, retained pages, and
tokenizer copies are different resource owners. Product admission must respect
approved pending limits; existing JobTable limits remain their own authority.
`embedding-backfill-preserves-query-admission` adds the query-priority claim.
`embedding-supervisor-shares-budget-and-joins` covers bounded slices and cleanup.
Existing FIFO (`crates/host-runtime/src/synapse/mod.rs:206-225`) is not priority.

## Security boundaries

Use exact verified tokenizer bytes, not a separately reopened artifact with a
matching label. Retain project/occurrence scope in the pending identity; a
same-payload row in another project cannot satisfy completion. Existing bundle
and request authentication records are reused. Authorization and canonical
eligibility at retrieval boundaries belong to RP2.7 and projection owners.

## Distributed coordination

Not applicable to consensus or fleet ownership: one daemon and a process-local
Synapse component are the examined topology. Restart incarnation fencing is
applicable and is retained. No second leader, distributed lease, or routing
plane is inferred from the word "durable".

## Lifecycle transitions

Missing tokenizer identity means no submission. Shutdown stops new admission,
retains ownership of started work, and leaves unfinished durable work retryable.
Existing component shutdown joins the tracker and synchronous CPU holder
(`crates/host-runtime/src/synapse/mod.rs:1148-1161`). Reuse that ownership rather
than equating receiver cancellation with native completion.

## Idempotency and replay

Reuse canonical request-key calculation and retained-key deduplication
(`crates/host-runtime/src/synapse/protocol.rs:923-947`; `jobs.rs:407-428`).
Replay after restart may repeat pure inference. It must not create duplicate
durable completion or erase another occurrence. Count attempts separately from
committed identities; do not demand one lifetime inference per pending row.

## Version compatibility

The fingerprint binds tokenizer hashes, dimensions, and table epoch, but omits
model name (`crates/host-runtime/src/synapse/bundle.rs:652-702`). Completion must
therefore compare model explicitly as the plan requires. Same byte payloads do
not collapse occurrence identity. Bound versions and identity transitions are
inputs to campaigns, not constants invented by this catalog.

## Wildcard disposition

The final pass follows all model and property lenses. Its unique findings and
open decisions are in [99-wildcard.md](99-wildcard.md).
