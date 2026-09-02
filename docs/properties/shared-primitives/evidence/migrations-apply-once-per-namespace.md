# `migrations-apply-once-per-namespace`

- **Discovery:** storage migration pass.
- **Primary evidence:** `run_migrations` (`crates/storage/src/lib.rs:767-863`) reads `MAX(version)` for the namespace, sorts by version, skips versions at or below the watermark, and inserts the version record inside the same transaction as `execute_batch`.
- **Existing evidence:** `migrations_seed_once_across_reopen` (`crates/storage/src/lib.rs:1219`), `later_migration_applies_on_top_of_earlier` (`crates/storage/src/lib.rs:1543`), `independent_namespace_chains_in_one_database` (`crates/storage/src/lib.rs:1572-1603`).
- **Failure scenario:** a crash between the SQL batch and the version record; the record's transaction placement makes the migration re-run cleanly.
- **Timing window:** the crash window between batch and record is not injected; the tests cover the steady-state clauses.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. The reopen test counts seeded rows across two opens, which detects a second application; the namespace test uses two chains with overlapping version numbers.
- **Open-question log:** the crash window belongs to `/testing:crash-consistency-and-failpoint-testing`.
