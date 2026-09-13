# System lens: wildcard

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
This pass follows the eleven named system lenses. It tests their framing.
External lead: the plan's claim that changed output hashes are process-local.

## Unique finding: served receipts are durable too

`crates/memory-store/src/lib.rs:1330-1334,1733-1736` defines and persists
`ServedBlockFingerprint { block_id, content_hash, serialized_len }`.
`crates/daemon/src/transform.rs:2048-2089` includes synthetic messages in
that vector and assigns todo call/result IDs. Lines 4923-4930 compare it with
the stored vector and assign the new vector into metadata.

This refutes the broad claim that every changed daemon-built block hash lives
only in process-local memos. It does not refute synthetic exclusion from
`block_identity_by_mid`; these are different identity domains.
`crates/daemon/src/divergence.rs:42-95` compares whole receipt records, so a
changed hash or length can produce `ContentChanged` across an upgrade.

## Candidate and handoff

Keep durable ingress identity, durable served diagnostics, frozen synthetic
messages, and ephemeral caches separate in the catalog. Preserve exact
expected diagnostic changes instead of resetting durable state to hide them.
The parent specification must resolve the claimed cache-only rationale.

## Limits

The discovery proves a source path, not an executed upgrade outcome.
This initial wildcard did not perform independent portfolio evaluation. The
separate evaluator's completed findings are in `../portfolio-evaluation.md`.
Parallel discovery dispatch was attempted but the harness denied nested
subagents at depth one; the controller preserves that initial limitation.
