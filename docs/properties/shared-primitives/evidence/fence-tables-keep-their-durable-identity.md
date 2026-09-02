# `fence-tables-keep-their-durable-identity`

- **Discovery:** storage durable-identity pass at U2, prompted by the frozen-durable registry entries for `eidnara_fence` and `eidnara_schema_version`.
- **Primary evidence:** `ensure_fence_table` (`crates/storage/src/lib.rs:711-719`) and `run_migrations` (`crates/storage/src/lib.rs:804-907`) emit the DDL; `is_infrastructure_table` (`crates/storage/src/lib.rs:548-555`) and `infrastructure_objects` (`crates/storage/src/lib.rs:358-383`) name the same two tables; `require_infrastructure_unchanged` (`:422-438`) compares the main- and temp-schema objects that carry those names before and after every callback, so a callback cannot create, drop, swap, or rename its way onto them (`a_callback_cannot_forge_the_version_table_before_the_first_migration`, `:1021-1064`).
- **Existing evidence:** `fence_and_version_tables_keep_their_ddl` (`crates/storage/src/lib.rs:2354-2409`) reads `sqlite_schema` after open and one migration and compares the recorded SQL text for both tables and the single fence row; `database_without_fence_table_uses_zero_floor` (`crates/storage/src/lib.rs:2179-2201`) shows what a missing table does.
- **Failure scenario:** a renamed table makes `read_fence_epoch` return floor zero for a database that already recorded a higher epoch, reissuing a superseded token.
- **Timing window:** none.
- **Instrumentation:** `sqlite_schema` is the oracle; SQLite records the DDL text as written.
- **Audit verdict (U2): pass. The expected strings are literals in the test, not derived from the constants under test; the mutant that renames either table or drops a `CHECK` fails.
- **Open-question log:** none.
