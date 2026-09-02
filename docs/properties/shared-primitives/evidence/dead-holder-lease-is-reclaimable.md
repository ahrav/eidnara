# `dead-holder-lease-is-reclaimable`

- **Discovery:** claimed-liveness and failure-recovery passes.
- **Primary evidence:** the file-backend contract says the kernel releases the advisory lock on process death (`crates/lease/src/lib.rs:4-5`). PostgreSQL instead holds a session advisory lock, which the server releases when the connection drops (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:7-9`); its `Drop` impl also attempts `pg_advisory_unlock` before closing the connection (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:332-340`).
- **Existing evidence:** `shared_lease_across_processes_blocks_exclusive` lets the child exit normally once the parent closes its stdin; every in-process file release runs `Drop`. `superseded_writer_is_rejected_after_reopen` shows PostgreSQL reopening only after a normal drop (`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1144-1175`).
- **Failure scenario:** a killed file-lock holder leaves a lock that a replacement cannot reclaim on the deployed filesystem, or a dead PostgreSQL holder's connection remains live at the server and keeps its session advisory lock.
- **Timing window:** kill while handle is live; recovery check starts after process exit is confirmed.
- **Instrumentation:** missing non-unwinding child termination for the file backend, forced holder or connection termination for PostgreSQL, and bounded recovery deadlines for both.
- **Open-question log:** the mechanisms differ from the bound: file-lock release follows OS process/descriptor teardown, while PostgreSQL release follows server observation of session loss and can depend on transport failure detection. No operational recovery bound for either backend is stated in crate docs, in-repo consumer docs, or history.
