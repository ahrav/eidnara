# Property lens: data integrity

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R4-R7 and latency-audit B1.

## Finding

`crates/daemon/src/wire.rs:731-736,599-617` couples exact block bytes to
persisted ingress hashes. A typed semantic comparison alone misses omitted
fields, ordering, scalar forms, and sibling drift.
`crates/daemon/src/served_json.rs:121-163` supplies the existing sorting
mechanism. The proposed block entry point must own its byte result.

## Candidates

- `plugin-block-canonical-identity-preserved`: compare frozen plugin bytes,
  projected bytes, and SHA-256, without regenerating the expected side.
- `block-byte-consumers-share-canonical-basis`: compare each fresh consumer
  with independently canonicalized typed content.
- `sibling-mutation-preserves-untouched-bytes`: mutate one cloned block and
  compare every other block and the shared source against pre-edit snapshots.

## Limits and nonapplicability

No physical corruption or fsync ordering change is proposed. Byte-order and
field-loss perturbations are sufficient fault shapes for these computations.
All candidates are unexercised; existing checks stay unaudited.
