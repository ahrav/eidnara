# `fence-epoch-outside-sqlite-range-fails-closed`

- **Discovery:** storage fence-decoding pass.
- **Primary evidence:** `decode_fence_epoch` (`crates/storage/src/lib.rs:1324-1327`) uses `u64::try_from` and maps failure to `FenceCorrupt`; `fence_epoch_sql_value` (`crates/storage/src/lib.rs:1285-1292`) uses `i64::try_from` before any statement runs.
- **Existing evidence:** `negative_database_fence_fails_closed` (`crates/storage/src/lib.rs:2985-3013`) writes `-1` without the `CHECK` constraint and asserts the open fails and the row is unchanged; `epoch_above_sqlite_integer_range_fails` (`crates/storage/src/lib.rs:3093-3109`) constructs a store at `i64::MAX + 1` and asserts the fenced write fails with the range message.
- **Failure scenario:** an `as` cast turns `-1` into `u64::MAX` and authorizes any writer.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. Both conversions are covered from the failing side, and the negative test asserts the stored value is untouched.
- **Open-question log:** none.
