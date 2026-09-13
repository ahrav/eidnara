# Property lens: idempotency and replay

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan equality/receipts section and latency-audit B1.

## Findings

`crates/daemon/src/transform.rs:167-200` chooses the first positional
candidate, or the first digest candidate only when no position exists, then
rechecks structural equality. A present unequal positional candidate forces
fresh hashing instead of searching another position.
`crates/daemon/src/wire.rs:885-903` normalizes signed zero for that equality
index, whereas served serialization preserves its sign.

## Candidate

`typed-equality-governs-receipt-reuse` preserves candidate precedence and
first-match behavior using equality over `(kind, provider_extras)` after
removal of originals. Signed-zero reuse deliberately retains the selected
projection receipt; it need not equal a fresh served-byte hash.

## Narrow nonapplicability

This is receipt reuse within transformation, not exactly-once remote effects.
No attempted/acknowledged operation accounting is needed for this local check.
