# Property lens: failure recovery

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R6 and latency-audit W5.

## Findings

`crates/daemon/src/lib.rs:16443-16461` silently skips rows that no longer
decode. Exact expected recovered IDs and ordinals are therefore necessary.
`crates/daemon/src/historian.rs:152-157,326-334` uses a length-based durable
fingerprint and rejects mismatches.

## Candidates

`historical-chunks-retain-readable-identity` checks a stored old-release
message array through the real recovery reader, then checks expected snapshot
items and the literal fingerprint. `durable-identity-domains-survive-cache-reset`
checks preserved metadata with newly constructed empty process caches.

## Narrow nonapplicability and missing evidence

No crash-consistency claim follows from reopening a store. A termination test
is required only if the later portfolio adds a crash-recovery claim.
No old database or in-flight binary-upgrade campaign is supplied. Synthetic
todo blocks are excluded from production snapshot items at
`crates/daemon/src/historian_chunk.rs:418-430`.
