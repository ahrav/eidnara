# `contention-is-classified-as-held`

- **Discovery:** failure-degradation and history passes.
- **Primary evidence:** `LeaseError` distinguishes `Held` from `Io`; exclusive `File::try_lock` and shared `File::try_lock_shared` map `TryLockError::WouldBlock` to `Held` and `TryLockError::Error` to `Io`. PostgreSQL's `open_postgres` maps a false `pg_try_advisory_lock` result to `Held` and query errors to `Backend` (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:355-389`).
- **History:** commit `8abefe8` names Windows contention misclassification as a prior bug class.
- **Existing evidence:** same-process exclusive and shared contention tests assert `Held`, and `shared_lease_across_processes_blocks_exclusive` does too. CI runs the workspace tests on Ubuntu, macOS, and Windows (`commons@89abb40 .github/workflows/ci.yml:21-42`) and a live PostgreSQL job on Ubuntu (`commons@89abb40 .github/workflows/ci.yml:47-73`).
- **Failure scenario:** target returns a different contention code, or unsupported-lock failure is mistaken for contention.
- **Instrumentation:** missing injection of non-contention lock errors and unsupported-filesystem behavior.
- **Open-question log:** supported targets beyond the CI matrix are not declared.
