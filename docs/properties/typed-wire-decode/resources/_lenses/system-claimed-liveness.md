# System lens: claimed liveness guarantees

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Reservations are nonwaiting at `crates/host-runtime/src/handler.rs:557-565`.
The meter halves batch requests until the exact shortfall at
`crates/daemon/src/metered_decode.rs:266-303`. Release clears charges at
`:209-217`; the existing drained-pool test retries after holder drop at
`crates/daemon/src/lib.rs:20440-20443`.

These are finite operation and lifetime checks. No bounded end-user latency
or recovery SLO is supplied for typed decode. P:L18's stop-within-noise rule
governs optimization acceptance, not runtime liveness. No artificial
`eventually` property is introduced. Fault-free retry reachability remains
with the independent witness record.
