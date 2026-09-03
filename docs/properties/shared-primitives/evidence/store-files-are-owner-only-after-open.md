# `store-files-are-owner-only-after-open`

- **Discovery:** storage hardening pass.
- **Primary evidence:** `open_sqlite` (`crates/storage/src/lib.rs:682-831`) creates a missing database file with mode `0600` and `O_NOFOLLOW` before SQLite opens it (a symlink at the database path, dangling or not, is refused before any file exists; `dangling_symlink_at_the_database_path_is_refused_without_creating_the_target` (`crates/storage/src/lib.rs:2264-2283`)), then, once `classify` has accepted the file's identity, calls `lease::protect_file` on the database path and its `-wal` and `-shm` siblings before enabling WAL or writing the fence. A file that fails the identity check keeps its mode as well as its bytes (`a_refused_foreign_file_keeps_its_permissions` (`crates/storage/src/lib.rs:2037-2067`)). SQLite gives sidecars the database file's mode, so files created later start owner-only.
- **Existing evidence:** `reopening_a_permissive_store_protects_the_database_and_its_wal` (`crates/storage/src/lib.rs:2320-2370`) covers pre-existing permissive files; `new_database_file_is_owner_only_at_creation` (`:2303-2318`) proves the created database is `0600` under umask `022` with no `chmod`; `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask` (`:2773-2801`) proves a fresh open under umask `022` leaves the database, `-wal`, and `-shm` at `0600`. All Unix only.
- **Failure scenario:** a permissive WAL exposes committed rows or lets another user forge the fence row.
- **Timing window:** files created after open inherit the database mode; the test models a WAL left by an unclean shutdown.
- **Instrumentation:** `std::fs::metadata` mode bits.
- **Audit verdict (U2): pass. Both files are set to `0644` before reopen and both are asserted `0600` after, so the assertion cannot pass on the fresh-file default.
- **Open-question log:** Windows has no equivalent mode semantics; `protect_file` is a no-op there.
