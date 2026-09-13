# System lens: architecture and data flow

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

`Handler::handle` caps bytes, creates the meter, reserves admission scratch,
probes the lane, dispatches, and settles at `crates/daemon/src/lib.rs:12153-12170`.
The direct path walks `SkippedValue` before typed decoding; invalid typed
decoding restarts into a metered `Value` parse at `:12921-12946`.
Tree conversion calls `from_value` at `:8115-8125`.

Wire envelope deserializers build and clone trees at
`crates/memory-store/src/lib.rs:126-143,250-264`. Projection can clone a shell
at `crates/daemon/src/wire.rs:540-559` and allocates canonical text at `:731`.
P:L38 and P:L88 propose removing envelope trees and reducing string charge.
The resulting combined peak is an obligation, not a source-proven saving.

Candidates: combined decode footprint, retained ownership, whole projection
envelope. Do not treat `from_value` alone as the tree lane.
