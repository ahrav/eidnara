# Property lens: wildcard

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
This pass follows all ten named property lenses and the full system model.
External leads: plan U2 and the canonical-byte consumer contract.

## Unique findings

`crates/daemon/src/codec/sidecar.rs:407-410` accepts a `Value`, serializes it,
and hashes the serialization. Passing canonical text as `Value::String`
hashes JSON quotes and escapes, not the canonical block bytes. The plan's
phrase "stable_hash over same bytes" needs a raw-byte seam; the existing
`crates/daemon/src/wire.rs:865-868` already hashes raw string bytes.

`typed-equality-governs-receipt-reuse` must not assert digest equality implies
structural equality. The production equality recheck is part of the contract.
Similarly, a common fresh basis does not make reused signed-zero receipts
equal to freshly hashed served bytes.

## Independent reachability candidate

`identity-edge-states-are-exercised` records constructed inputs: emitted shape
partitions, shifted receipt positions, codec stamps, old rows, cold caches,
and a selected sibling mutation. It does not assert occurrence of corruption,
identity rejection, or missing history. Marker names are fixed literals.

## Limits and handoff

The portfolio is safety-heavy because the slice is deterministic encoding.
The separate completed evaluation and dispositions are in
`../portfolio-evaluation.md`. This report remains discovery's wildcard, not
that independent evaluation. No tests were run.
