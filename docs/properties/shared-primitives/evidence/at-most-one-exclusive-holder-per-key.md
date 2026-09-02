# `at-most-one-exclusive-holder-per-key`

- **Discovery:** architecture, concurrency, claimed-safety, and failure-recovery passes.
- **Primary evidence:** headline contract at `crates/lease/src/lib.rs:2-6`; `FileLeaseStore::lease_path` derives one per-key path and `FileLeaseStore::acquire` calls `File::try_lock`, mapping `TryLockError::WouldBlock` to `Held` and other lock errors to `Io`. PostgreSQL derives one advisory key and treats a false `pg_try_advisory_lock` result as `Held` (`commons@89abb40 crates/`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:65-78`,332-359`).
- **Existing evidence:** `acquire_then_second_holder_is_rejected` and PostgreSQL's `open_migrate_and_single_writer` are same-process and sequential. `README.md:12-13` claims a real-daemon two-process check, but none exists in this repository.
- **Failure scenario:** independent processes race; path aliasing, replacement, or lock-scope mismatch creates separate lock domains; both return `Ok`.
- **Timing window:** both contenders reach backend lock acquisition before either holder releases.
- **Instrumentation:** missing live-exclusive-holder identity and cross-process exclusive-versus-exclusive race barrier.
- **Open-question log:** searched the target crate, both store backends, README, CI, and the pinned external inventory. The canonical Claustrum blocker is recorded in the [durable consumer inventory](../durable-consumer-inventory.md).
