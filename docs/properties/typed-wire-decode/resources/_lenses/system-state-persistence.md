# System lens: state and persistence

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Ready snapshots hold `Arc<TransformRequest>` at
`crates/daemon/src/lib.rs:1803-1812`. Snapshot accounting includes native
Values, message shells, and tail deltas at `:1745-1799`. Projection owns
canonical text and replay shells at `crates/daemon/src/wire.rs:182-198,243-365`.
These allocations outlive the local parsing call.

The plan removes only original envelope trees, not payload Values or cached
canonical text (P:L95-L110). Allocation identity matters when snapshots and
projections share shells; a smaller serialized object is not its heap size.
Storage durability protocols are N/A to this resource-only change. Refusal
must nevertheless create no store state, checked by the existing test at
`crates/daemon/src/lib.rs:20533-20580`.

Candidate: ownership-led retained accounting across cache/lease handoff.
