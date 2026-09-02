# `protected-transactions-pin-fence-durability`

- **Discovery:** storage durability pass.
- **Primary evidence:** `pin_fence_durability` (`crates/storage/src/lib.rs:557-572`) runs at the start of `with_conn_fenced` and `run_migrations`; `open_sqlite` (`crates/storage/src/lib.rs:574-654`) calls the same function before the fence claim, so an open on a VFS that cannot switch to WAL fails instead of committing a claim every later fenced write would reject.
- **Existing evidence:** `open_pins_full_synchronous` (`crates/storage/src/lib.rs:1800-1809`), `a_read_callback_cannot_lower_fence_durability` (`crates/storage/src/lib.rs:1834-1899`).
- **Failure scenario:** WAL with `synchronous = NORMAL` can lose the most recent commits after power loss, rolling the fence epoch back.
- **Timing window:** power loss after a fence claim; not injected.
- **Instrumentation:** `PRAGMA synchronous` and `PRAGMA journal_mode` reads.
- **Audit verdict (U2): pass. The test lowers the settings through the maintenance path, then observes the re-pinned values after a fenced write and after a migration.
- **Open-question log:** power-loss evidence belongs to `/testing:crash-consistency-and-failpoint-testing`.
