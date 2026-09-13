# Property lens: lifecycle transitions

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Meter restart preserves charges at
`crates/daemon/src/metered_decode.rs:201-217`; the handler retains the meter
through settlement at `crates/daemon/src/lib.rs:12162-12170`. Snapshots retain
request ownership after local decode (`:1803-1812`). Active projection lease
tests preserve charge across cache removal at `:23340-23353`.

Candidates: continuous peak coverage through cleanup or budget handoff, and
retained ownership after cache eviction. A completed function is not enough
if an Arc remains live. Startup readiness, transport shutdown, and blocking
worker supervision are N/A to this bounded local change.
