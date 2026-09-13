# Property lens: idempotency and replay

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

Repeated prefix reattachment shares canonical shells in
`crates/daemon/src/wire.rs:1749-1792`. Cloning handles can keep allocations
alive after the original projection is dropped. Count physical allocations
once within the ownership graph while respecting full per-holder charges.

Candidate folds into retained accounting. Canonical replay and digest
compatibility are prerequisites from B1 and P:L44-L47, not duplicate resource
records. Durable request deduplication and exactly-once effects are N/A;
admission refusal's no-store-effect check remains in the frozen-boundary record.
