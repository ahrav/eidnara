# Embedding system model

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Scope and external evidence: [source register](../catalog.md#source-register).
These are separate attention passes in one context. They are not independent
corroboration. The wildcard is retained separately and runs last.

## Architecture and data flow

The daemon host composes Synapse at `serve.rs:1097-1127` under
`crates/daemon/src/bin/eidnara_host/`. A selected generation supplies the bundle
manifest and ORT digests at `serve.rs:1027-1065`. Synapse activation loads verified
bytes and builds Backend (`crates/host-runtime/src/synapse/mod.rs:1189-1228`).
Routed batches enter JobTable; routed queries have separate admission but share
the same CPU semaphore (`mod.rs:634-902`). Product pending rows and the daemon
embedding driver are proposed, not another implemented transport component.

## State and persistence

JobTable stores two HashMaps under a mutex, an incarnation nonce, sequence,
retained vectors, and byte counters (`crates/host-runtime/src/synapse/jobs.rs:162-196`,
`:323-342`). Ready vectors are Arc-backed memory (`jobs.rs:502-557`). No database
write occurs there. RP2.1 makes durable Pending the recovery source and requires
vectors to be durable before product completion. It does not make JobTable durable.

## Concurrency model

One CPU semaphore serializes queries and batches, in semaphore registration
order (`crates/host-runtime/src/synapse/mod.rs:206-225`). A tracker owns async
workers that join blocking inference (`mod.rs:678-722`, `:843-902`). Query
admission permits remain held through physical inference. Admission order is
not host arrival order and FIFO is not a backfill-priority policy.

## Claimed safety guarantees

RP2.1 KTD3 requires one tokenizer authority and exact untruncated EmbedTokens.
U3 requires stale-result rejection and durable vectors before completion.
The existing private count runs on the truncating inference tokenizer
(`crates/host-runtime/src/synapse/inference.rs:577-589`). That is code evidence of
a missing product preflight, not a claim that the existing wire path is broken.

## Claimed liveness guarantees

RP2.1 U3 requires restart recovery and query admission under backfill saturation.
The plan leaves production limits to RP2.9. Recovery attempts, query waiting,
supervisor work, and lease/cancellation bounds therefore remain symbolic and
require approval before a campaign can claim success.

## Bug history and density

No incidents are supplied. A read-only path-scoped history inspection identifies
`1d9d4bd8`, `b1f96743`, `04141308`, and `bc007c9a` as scheduler follow-up work on
retained slots, cancellation, clock advancement, and root collapse. Their titles
are leads only, not confirmed incident mechanisms. Current scheduler code and
tests are inspected directly. No defect or exercise claim is inferred from a
commit subject.

## Existing test strategy

Synapse has a counting/gated deterministic engine, real host protocol checks,
JobTable unit checks, and ignored certified-runtime integration checks. The
scheduler uses ManualClock and a real MemoryStore. Existing catalogs and checks
are linked in [existing-checks.md](../existing-checks.md), with current coordinates.
No existing test is credited with exercising an absent product driver.

## Failure and degradation

Missing or invalid artifacts disable Synapse. Retryable inference failures
permit retained-key replacement (`crates/host-runtime/src/synapse/jobs.rs:407-458`).
Unknown, foreign-incarnation, expired, or evicted jobs return Restarted
(`jobs.rs:597-606`). RP2.1 must interpret that as a recovery trigger, not proof
that a durable vector exists. A missing exact count keeps lexical state and
marks dense work missing under the plan's failure contract.

## Dependencies

VerifiedBundle holds the four tokenizer files as verified bytes
(`crates/host-runtime/src/synapse/bundle.rs:259-278`, `:293-304`). Backend passes
them to FastEmbed with the manifest token window (`inference.rs:297-318`).
The provider-accounting tokenizer instead embeds Claude BPE
(`crates/tokenizer/src/lib.rs:1-8`, `:58`). Neither is interchangeable with the
other. Dependency internals and runtime behavior are not executed here.

## Product context

Missing or obsolete vectors reduce dense retrieval coverage; lexical presence
does not prove vector availability. Wrong-identity vectors are worse than a
missing dense lane because they silently mix revisions or embedding spaces.
Export, projection transactions, source-class coverage, and rebuild publication
belong to the other discovery lanes. This lane consumes their durable-row and
current-occurrence boundaries without duplicating their records.

## Unproven assumptions

`EvalBudget` already exists (`crates/kernel/src/applicability/checkout.rs:146-203`),
but Synapse does not consume it. The shared SQLite progress installer is
crate-private (`crates/kernel/src/open.rs:1379-1387`). The existing scheduler
starts with the opened store (`crates/daemon/src/lib.rs:3646-3660`), yet its task
body only dispatches review-user-memories (`dreamer_scheduler.rs:334-371`).
Neither a shared RP2 supervisor slice nor a public budget bridge exists.
An aborted future cannot be assumed to stop a native call. A finite physical
drain bound for an uncooperative native call remains an owner decision.
