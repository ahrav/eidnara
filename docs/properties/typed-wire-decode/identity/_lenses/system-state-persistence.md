# System lens: state and persistence

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External lead: plan KTD2 and latency-audit B1/W5.

## Observations

- `crates/daemon/src/wire.rs:599-617` excludes synthetic blocks and messages
  from `identity_by_mid`.
- `crates/daemon/src/transform.rs:5163-5244,5269-5298` checks and adopts
  ingress identities into metadata. Covered or frozen identity drift can reject.
- `crates/memory-store/src/lib.rs:1600` stores `block_identity_by_mid`.
  Lines 1273-1319 also persist and decode the frozen synthetic todo messages.
  Exclusion from ingress identity is not exclusion from all durable state.
- `crates/daemon/src/lib.rs:3684-3689` initializes snapshot, output, native,
  and projection caches empty. References use `git show HEAD` for this dirty file.
- `crates/daemon/src/history_summarizer_chunk.rs:665-675` serializes nonsynthetic raw
  messages. `lib.rs:16443-16461` decodes recovered rows as `IngressMessages`.

## Contract and candidate

Preserve persisted plugin identities across a cold start. Permit the plan's
synthetic false omission without pretending that frozen synthetic bytes are
not persisted. Check historical raw-row readability separately.

## Narrow nonapplicability and missing evidence

This is an upgrade compatibility boundary, not a new durability protocol.
No kill/restart campaign or old-release database artifact is supplied.
