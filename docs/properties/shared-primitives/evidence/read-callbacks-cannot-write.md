# `read-callbacks-cannot-write`

- **Discovery:** storage callback-capability pass.
- **Primary evidence:** `CallbackScope::read_only` (`crates/storage/src/lib.rs:387-391`) sets `PRAGMA query_only = ON` and installs `deny_scope_escapes` (`crates/storage/src/lib.rs:475-493`); `GuardedConn` (`crates/storage/src/lib.rs:256-368`) forwards only `query_row`, `execute`, `prepare`, `last_insert_rowid`, and `changes`.
- **Existing evidence:** `unfenced_connection_rejects_writes` (`crates/storage/src/lib.rs:1680`), `a_read_callback_cannot_clear_the_read_only_guard` (`crates/storage/src/lib.rs:1807`), `maintenance_runs_through_the_unfenced_path` (`crates/storage/src/lib.rs:2033`).
- **Failure scenario:** a callback that clears `query_only` or that receives the raw `Connection` can write outside the fence.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. Each denial is observed as an error and followed by a row-count or pragma read that shows nothing changed.
- **Open-question log:** none.
