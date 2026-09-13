# System lens: concurrency model

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

One decode drives a meter at a time; atomics let it survive handler awaits
(`crates/daemon/src/metered_decode.rs:132-143`). `RequestCtx` reserves from
scratch at `crates/host-runtime/src/handler.rs:549-565`. `ByteBudget` shares a
semaphore and returns drop-owned charges at
`crates/host-runtime/src/wire.rs:394-442,449-459`.

A fake successful reserve with `ByteCharge::none()` can count an estimate but
cannot prove concurrency admission. Existing `TestPool` uses the real budget
at `crates/daemon/src/lib.rs:20242-20276`; `hold` constructs another owner.
The shortfall witness at `:20414-20470` does not run competing ring requests.

Candidates: independent held-byte witness and aggregate live-byte envelope.
Worker relocation is outside this plan's resource change (P:L62).
