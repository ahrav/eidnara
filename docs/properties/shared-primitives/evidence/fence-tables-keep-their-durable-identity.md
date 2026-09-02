# `fence-tables-keep-their-durable-identity`

- **Discovery:** storage durable-identity pass at U2, prompted by the frozen-durable registry entries for `cortexkit_fence` and `cortexkit_schema_version`.
- **Primary evidence:** `ensure_fence_table` (`crates/storage/src/lib.rs:668-675`) and `run_migrations` (`crates/storage/src/lib.rs:778-786`) emit the DDL; `is_infrastructure_table` (`crates/storage/src/lib.rs:511-514`) and `require_no_shadow` (`crates/storage/src/lib.rs:395-415`) name the same two tables.
- **Existing evidence:** `fence_and_version_tables_keep_their_ddl` (`crates/storage/src/lib.rs:2234`) reads `sqlite_schema` after open and one migration and compares the recorded SQL text for both tables and the single fence row; `legacy_database_without_fence_table_uses_zero_floor` (`crates/storage/src/lib.rs:2056`) shows what a missing table does.
- **Failure scenario:** a renamed table makes `read_fence_epoch` return floor zero for a database that already recorded a higher epoch, reissuing a superseded token.
- **Timing window:** none.
- **Instrumentation:** `sqlite_schema` is the oracle; SQLite records the DDL text as written.
- **Audit verdict (U2): pass. The expected strings are literals in the test, not derived from the constants under test; the mutant that renames either table or drops a `CHECK` fails.
- **Open-question log:** none.
