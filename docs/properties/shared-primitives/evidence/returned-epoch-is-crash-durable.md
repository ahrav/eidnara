# `returned-epoch-is-crash-durable`

- **Discovery:** state/persistence and crash-recovery passes.
- **Primary evidence:** persisted epochs are part of the process-level fencing design; `persist_epoch` calls `Write::flush` but no data or directory sync. No documentation promises power-loss atomicity.
- **Contradictory code evidence:** no `sync_data`, `sync_all`, or directory sync; `flush` at `crates/lease/src/lib.rs:649` is not a stable-storage barrier for `File`.
- **Existing evidence:** `epoch_persists_across_store_instances` recreates `FileLeaseStore` in a live process, which preserves page cache and directory state.
- **Failure scenario:** acknowledged epoch or newly created directory entry is lost on power failure; next writer reuses an old value.
- **Timing window:** after `acquire` returns and before kernel writeback.
- **Instrumentation:** missing crash-image or power-cut replay and acknowledgement witness keyed by physical root plus `LeaseKey` fields.
- **Open-question log:** machine-power-loss durability is unsupported unless a separate protocol and test campaign are added.
