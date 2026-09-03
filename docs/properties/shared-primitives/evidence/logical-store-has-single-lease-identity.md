# `logical-store-has-single-lease-identity`

- **Discovery:** targeted consumer-boundary pass after portfolio evaluation.
- **Primary evidence:** `lease_key` includes the database file name (`crates/storage/src/lib.rs:62-74`); `open_sqlite` derives the root from the database parent (`:652-663`); `open_sqlite` opens with `SQLITE_OPEN_NOFOLLOW` and creates with `O_NOFOLLOW`, which refuses a symlink anywhere in the database path.
- **Existing evidence:** `distinct_databases_do_not_falsely_contend` (`crates/storage/src/lib.rs:2015-2024`) uses different parent directories; `distinct_databases_in_one_directory_do_not_falsely_contend` (`:2026-2037`) proves sibling files with equal key fields do not contend; `symlinked_database_paths_are_refused_never_aliased` (`:1527-1583`) proves a directory-symlink alias and a file-symlink alias are both refused, with and without a live holder.
- **Failure scenario:** one SQLite database opened under differing module/namespace descriptors gets split leases. A hardlink alias is refused before any open: `refuse_unfit_store_files` rejects a database, `-wal`, or `-shm` path that is not a regular file or whose link count exceeds one (`a_hard_linked_database_is_refused` (`crates/storage/src/lib.rs:1205-1232`)).
- **Timing window:** concurrent opens are needed for writer overlap; false contention needs no timing.
- **Instrumentation:** missing authoritative logical-store ID and canonical `(root,key)` observation.
- **Open-question log:** no validation binds one database path to one immutable descriptor. Deployment authority remains external.
