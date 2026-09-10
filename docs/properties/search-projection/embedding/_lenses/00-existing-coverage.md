# Existing coverage readback

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
This assessment precedes the embedding additions. It compares existing records
with current source, not with their recorded exercise claims.

## Method and evidence scope

The user supplies the settled RP2.1 plan, RP2 index, linked local parents, and
this repository. No incident reports are supplied. No tracker or external web
evidence is requested or consulted. Documented guarantees are claims under test.
The [catalog source register](../catalog.md#source-register) records why each
local source is consulted.

`colgrep` is attempted first. Its index cannot load during a concurrent update.
Exact search and read-only source inspection supply the fallback evidence.
Two requested read-only subagent passes fail at the harness depth limit before
running. The readback and lens passes therefore run sequentially in this lane.
This is a refreshed coverage assessment, not an independent evaluation.
The central independent evaluation is supplied separately by analyst session
`ses_f7623dcccffe3Y09nW2wVoABif`. Its findings and this writer's dispositions
are retained in [portfolio-evaluation.md](../portfolio-evaluation.md). This
readback and the subsequent edits are not an independent re-review.

## Reuse decisions before additions

| Existing record | Decision | Reason |
| --- | --- | --- |
| [synapse-bundle-fingerprint-covers-every-artifact](../../../host-runtime/catalog.md#synapse-bundle-fingerprint-covers-every-artifact) | Reuse exactly. | Hash participation and verified bundle loading already have an owner. The new count record adds untruncated counting from those same bytes. |
| [synapse-requests-are-validated-before-any-inference](../../../host-runtime/catalog.md#synapse-requests-are-validated-before-any-inference) | Reuse existing clauses. | Wire constraints, byte limits, hashes, and retained-key replay already belong here. RP2.1 adds exact token rejection on the product path. |
| [synapse-admission-boundaries-are-exact](../../../host-runtime/catalog.md#synapse-admission-boundaries-are-exact) | Reuse, with refreshed evidence. | JobTable count, bytes, retention, and result leases are existing mechanisms. Durable discovery and query priority are additions. |
| [synapse-degrades-to-disabled-and-keeps-the-context-routable](../../../host-runtime/catalog.md#synapse-degrades-to-disabled-and-keeps-the-context-routable) | Reuse exactly within its scope. | Optional-lane failure is not a reason to duplicate a host degradation record. Dense coverage remains a separate product concern. |
| [synapse-inference-runs-through-a-sealed-runtime-image](../../../host-runtime/catalog.md#synapse-inference-runs-through-a-sealed-runtime-image) | Reuse exactly. | Runtime-image certification is not a new embedding-driver property. |
| [tokenizer-encoding-matches-the-independent-oracle](../../../tokenizer/catalog.md#tokenizer-encoding-matches-the-independent-oracle) and the six sibling tokenizer records | Reuse for Claude accounting only. | The embedded Claude BPE vocabulary is not the Synapse model tokenizer. Its golden fixtures cannot certify EmbedTokens. |
| [scheduled-dreamer-slot-runs-once-through-lease-and-receipt](../../../daemon/handlers/catalog.md#scheduled-dreamer-slot-runs-once-through-lease-and-receipt) | Reuse the scheduled-task contract. | It covers the review-user-memories task, not durable embedding work. Reuse supervisor ownership without importing billable-attempt semantics into pure inference. |

## Harness fit

The existing deterministic engine records calls and input text and can park or
fail a call (`crates/host-runtime/tests/support/synapse.rs:40-122`). It provides
an inference boundary, not a tokenizer oracle or a durable completion oracle.
The verified bundle and independent token fixtures are needed for counting.
A reopened projection and an externally retained dispatch trace are needed for
durable work. Existing tests are all unaudited for this handoff.

## Coverage balance

Existing records emphasize host wire validation and local resource bounds.
They do not cover a durable embedding pending source, revision-aware completion,
vector persistence ordering, restart retry, or identity GC. Query FIFO has a
test, but query priority under saturated backfill has no implementation.
The additions cover those gaps and do not clone the existing guarantees.

## Implementability and stale evidence

The tokenizer catalog's no-production-caller statement is stale:
`crates/daemon/Cargo.toml:32` depends on it and
`crates/daemon/src/token_cache.rs:133` calls it in production code.
The host catalog's test-only Synapse composition statement is also stale:
`crates/daemon/src/bin/eidnara_host/serve.rs:1097-1127` composes Synapse.
Ready inference still requires a selected generation with certified artifacts
(`serve.rs:1027-1065`). That path is explicit-config-only; composition and the
disabled fallback are default-production.

The old admission record describes oldest-completion eviction. Current
`crates/host-runtime/src/synapse/jobs.rs:156-158` prefers last poll time, then
completion time. Do not copy the older ordering assertion into a new record.
Current ORT integration checks are explicitly ignored, for example
`crates/host-runtime/tests/synapse_bundle.rs:579-581`; the old inventory's
environment-missing early-return description is not current exercise evidence.

## Wildcard for the existing set

The normative wire contract deliberately permits truncation and describes a
TypeScript recovery ledger (`docs/host-wire-protocol.md:472`, `:502`). RP2.1
changes the product preflight and recovery owner. Keep that contract seam open
for document-owner reconciliation; do not silently reinterpret existing wire
behavior or add a tokenizer wire operation. The final discovery wildcard runs
after the other new lenses in [99-wildcard.md](99-wildcard.md).
