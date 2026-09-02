# `logical-store-has-single-lease-identity`

- **Discovery:** targeted consumer-boundary pass after portfolio evaluation.
- **Primary evidence:** `lease_key` includes the database file name (`crates/storage/src/lib.rs:60-72`); `open_sqlite` derives the root from the database parent (`:606-619`); `protect_file` refuses a database path that is not a regular file, which rejects file-symlink aliases (`:622-628`).
- **Existing evidence:** `distinct_databases_do_not_falsely_contend` (`crates/storage/src/lib.rs:1421-1430`) uses different parent directories; `distinct_databases_in_one_directory_do_not_falsely_contend` (`:1432-1445`) proves sibling files with equal key fields do not contend; `symlinked_database_paths_contend_or_are_refused_never_aliased` (`:925-977`) proves a directory-symlink alias returns `Held` and a file-symlink alias is refused as a non-regular file, with and without a live holder.
- **Failure scenario:** one SQLite database opened under differing module/namespace descriptors, or through a hardlink alias, gets split leases. Hardlinks share an inode but no pathname, so no path-derived root can detect them.
- **Timing window:** concurrent opens are needed for writer overlap; false contention needs no timing.
- **Instrumentation:** missing authoritative logical-store ID and canonical `(root,key)` observation.
- **Open-question log:** no validation binds one database path to one immutable descriptor. Deployment authority remains external.
