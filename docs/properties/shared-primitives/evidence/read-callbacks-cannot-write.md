# `read-callbacks-cannot-write`

- **Discovery:** storage callback-capability pass.
- **Primary evidence:** `CallbackScope::read_only` (`crates/storage/src/lib.rs:379-387`) sets `PRAGMA query_only = ON` and installs `deny_scope_escapes` (`crates/storage/src/lib.rs:464-508`); `GuardedConn` (`crates/storage/src/lib.rs:219-331`) forwards only `query_row`, `execute`, `prepare`, `last_insert_rowid`, and `changes`.
- **Existing evidence:** `unfenced_connection_rejects_writes` (`crates/storage/src/lib.rs:1862-1885`), `a_read_callback_cannot_clear_the_read_only_guard` (`crates/storage/src/lib.rs:1988-2036`), `maintenance_runs_through_the_unfenced_path` (`crates/storage/src/lib.rs:2248-2261`).
- **Failure scenario:** a callback that clears `query_only` or that receives the raw `Connection` can write outside the fence.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. Each denial is observed as an error and followed by a row-count or pragma read that shows nothing changed. `is_side_effecting_pragma` (`crates/storage/src/lib.rs:510-521`) denies the argumentless pragmas that `query_only` does not stop (`wal_checkpoint`, `incremental_vacuum`, `optimize`, `shrink_memory`); `a_read_callback_cannot_checkpoint_the_wal` (`:1283-1299`) observes the denial and that a pragma read still works.
- **Open-question log:** none.
