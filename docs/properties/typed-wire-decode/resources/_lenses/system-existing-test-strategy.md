# System lens: existing test strategy

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

The parse-peak integration binary uses a process-wide requested-layout
counter and a local serialization mutex
(`crates/daemon/tests/parse_charge_covers_typed_decode.rs:25-88`). It measures
both tree stages together for native scalars at `:110-123`.
The text-heavy case measures only direct decoding at `:178-199`.

Library tests compare outcomes and needed bytes at
`crates/daemon/src/lib.rs:20188-20229`, with capacities recomputed from the
same footprint function. `FixtureProcess` supports a real unary transform
(`crates/daemon/tests/direct_host.rs:48-69`). Source presence is not proof
that any new resource case passed. Every check remains unaudited in the
inventory. Allocation thresholds and measurement manifests have no new run.
