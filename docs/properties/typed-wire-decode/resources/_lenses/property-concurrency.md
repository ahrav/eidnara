# Property lens: concurrency

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

`TestPool::hold` at `crates/daemon/src/lib.rs:20261-20265` supplies independent
pressure. `ShortfallMarker` records needed, charged, and capacity at
`crates/daemon/src/metered_decode.rs:294-302`, but not another owner's held
allocation identity. A returned `queue_full` alone is not a witness.

Candidates: frozen admission and independent resource situations. Record the
holder's acquired bytes before dispatch, keep the holder live through the
attempt, and retry after drop. Add the aggregate projection ledger, because
single-request capacity checks can miss concurrent uncharged peaks.
