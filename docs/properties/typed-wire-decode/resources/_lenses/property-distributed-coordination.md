# Property lens: distributed coordination

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

N/A for consensus, elections, quorum loss, and partition recovery. The changed
work is a local decode and projection, backed by an in-process byte budget
(`crates/host-runtime/src/wire.rs:394-442`). It introduces no distributed
authority or replica protocol.

Cross-process input framing remains relevant only as the highest admission
observation seam (`docs/host-wire-protocol.md:304-314`). A real host fixture
does not turn the decoder property into a distributed-systems test.
No unique coordination property is added.
