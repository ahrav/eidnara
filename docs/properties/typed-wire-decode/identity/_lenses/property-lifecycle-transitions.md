# Property lens: lifecycle transitions

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan KTD2 and latency-audit W5.

## Findings

`crates/daemon/src/lib.rs:3684-3689` starts process-local caches empty.
`crates/memory-store/src/lib.rs:1279-1319,1600,1736` retains synthetic
messages, ingress identities, and served receipts in durable metadata.
Restart does not make that durable state empty.

## Candidate

`durable-identity-domains-survive-cache-reset` compares warm and cold
recomputation with preserved metadata. Plugin ingress identities must remain
equal. Accepted synthetic/output byte changes must be visible in expected
durable diagnostic receipts rather than mislabeled ephemeral cache misses.

## Narrow nonapplicability

No shutdown deadline, route drain, or lease ownership change is proposed.
The lifecycle boundary here is serialization-version replacement plus cache
construction. No kill/restart or binary-upgrade result is claimed.
