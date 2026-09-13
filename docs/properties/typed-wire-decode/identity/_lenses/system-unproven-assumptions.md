# System lens: unproven assumptions

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External lead: plan KTD2, R6, and Appendix A.3.

## Discriminating findings

- The plan's 25-byte omission counts the member but not its delimiter.
  `len(',"provider_executed":false'.encode('utf-8')) == 26`. Every typed
  tool kind has other members (`crates/memory-store/src/lib.rs:340-352`).
- `crates/daemon/src/historian_chunk.rs:418-430` excludes synthetic blocks
  from production snapshot items. R6's synthetic todo fingerprint example
  therefore needs another demonstrated path or a test-only classification.
- `crates/daemon/src/transform.rs:13825-13830,13889-13892` explicitly shows
  equal signed zeros with different bytes and different fresh/reused receipts.
- Frozen synthetic messages are durable (`crates/memory-store/src/lib.rs:1273-1319`),
  even though synthetic ingress identities are excluded.

## Open questions and handoffs

The plan is accepted as prospective intent; its asserted implementation facts
need these corrections in the parent specification. No interview occurs here.
Ask the specification owner to distinguish permitted upgrade byte changes
from universal frozen-byte promises and to classify the synthetic chunk example.

## Narrow nonapplicability

No supplied corpus demonstrates a production 25-byte delta. Appendix A.3's
26 differing blocks out of 53 is a corpus count, not a byte-length proof.
