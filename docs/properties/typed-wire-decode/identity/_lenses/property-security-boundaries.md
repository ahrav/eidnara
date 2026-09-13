# Property lens: security boundaries

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan field classification and normative host protocol.

## Finding

`crates/daemon/src/codec/sidecar.rs:148-174` excludes exactly the
`_eidnara_codec` provider namespace from decoded content fingerprints.
Other namespaces and opaque values are input data, not permission to erase
content. A mistaken broad strip could attach one block's native metadata to
different content.

## Candidate

`block-byte-consumers-share-canonical-basis` checks invariance under changes
only to codec stamps, plus sensitivity to a controlled noncodec change.
`typed-equality-governs-receipt-reuse` keeps equality rechecks after digest lookup.

## Narrow nonapplicability and missing evidence

No authentication or transport trust boundary changes. SHA-256 indexing is
not authorization. Existing durable redaction remains an external constraint;
tests that redact raw history are inventoried but not treated as byte-preserving
oracles for secret-bearing fixtures. No exploit report is supplied.
