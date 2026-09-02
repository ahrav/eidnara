# `store-files-are-owner-only-after-open`

- **Discovery:** storage hardening pass.
- **Primary evidence:** `open_sqlite` (`crates/storage/src/lib.rs:610-616`) calls `lease::protect_file` on the database path and its `-wal` and `-shm` siblings after enabling WAL.
- **Existing evidence:** `reopening_a_permissive_store_protects_the_database_and_its_wal` (`crates/storage/src/lib.rs:1037`), Unix only.
- **Failure scenario:** a permissive WAL exposes committed rows or lets another user forge the fence row.
- **Timing window:** files created after open inherit the database mode; the test models a WAL left by an unclean shutdown.
- **Instrumentation:** `std::fs::metadata` mode bits.
- **Audit verdict (U2): pass. Both files are set to `0644` before reopen and both are asserted `0600` after, so the assertion cannot pass on the fresh-file default.
- **Open-question log:** Windows has no equivalent mode semantics; `protect_file` is a no-op there.
