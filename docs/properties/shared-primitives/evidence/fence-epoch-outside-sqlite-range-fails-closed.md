# `fence-epoch-outside-sqlite-range-fails-closed`

- **Discovery:** storage fence-decoding pass.
- **Primary evidence:** `decode_fence_epoch` (`crates/storage/src/lib.rs:1397-1400`) uses `u64::try_from` and maps failure to `FenceCorrupt`; `fence_epoch_sql_value` (`crates/storage/src/lib.rs:1358-1365`) uses `i64::try_from` before any statement runs.
- **Existing evidence:** `negative_database_fence_fails_closed` (`crates/storage/src/lib.rs:3162-3190`) writes `-1` without the `CHECK` constraint and asserts the open fails and the row is unchanged; `epoch_above_sqlite_integer_range_fails` (`crates/storage/src/lib.rs:3270-3286`) constructs a store at `i64::MAX + 1` and asserts the fenced write fails with the range message.
- **Failure scenario:** a fence at `i64::MAX` reaching the lease, which would persist `i64::MAX + 1` in the sidecar and leave the store unopenable after the row is repaired; `open_sqlite` refuses such a floor with `FenceExhausted` before `acquire_above` runs (`a_fence_at_the_integer_maximum_is_refused_before_the_lease_advances`).  an `as` cast turns `-1` into `u64::MAX` and authorizes any writer.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U2): pass. Both conversions are covered from the failing side, and the negative test asserts the stored value is untouched.
- **Open-question log:** none.
