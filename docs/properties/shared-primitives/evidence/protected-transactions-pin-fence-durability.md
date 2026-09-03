# `protected-transactions-pin-fence-durability`

- **Discovery:** storage durability pass.
- **Primary evidence:** `pin_fence_durability` (`crates/storage/src/lib.rs:650-665`) runs at the start of `with_conn_fenced`; `open_sqlite` (`crates/storage/src/lib.rs:667-799`) calls the same function before the transaction that applies the baseline and claims the fence, so an open on a VFS that cannot switch to WAL fails instead of committing a claim every later fenced write would reject.
- **Existing evidence:** `open_pins_full_synchronous` (`crates/storage/src/lib.rs:2580-2589`), `a_read_callback_cannot_lower_fence_durability` (`crates/storage/src/lib.rs:2613-2679`).
- **Failure scenario:** WAL with `synchronous = NORMAL` can lose the most recent commits after power loss, rolling the fence epoch back.
- **Timing window:** power loss after a fence claim; not injected.
- **Instrumentation:** `PRAGMA synchronous` and `PRAGMA journal_mode` reads.
- **Audit verdict (U2): pass. The test lowers the settings through the maintenance path, then observes the re-pinned values after a fenced write and after a fenced schema change.
- **Open-question log:** power-loss evidence belongs to `/testing:crash-consistency-and-failpoint-testing`.
