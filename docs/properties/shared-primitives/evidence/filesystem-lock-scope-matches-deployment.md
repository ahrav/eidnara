# `filesystem-lock-scope-matches-deployment`

- **Discovery:** dependencies and distributed-coordination passes.
- **Scope:** sqlite/file backend only. PostgreSQL uses a server-side session advisory lock across processes and machines, not a filesystem lock (`commons@89abb40 crates/`commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:7-9`,242-252`).
- **Primary evidence:** `FileLeaseStore` accepts an arbitrary `base_dir` and derives one path per key; acquisition uses standard-library `File::try_lock` and `File::try_lock_shared`, with explicit `TryLockError` arms.
- **Existing evidence:** `shared_lease_across_processes_blocks_exclusive` uses one host and a local temp directory.
- **Failure scenario:** shared mount implements node-local locks, overlay replacement, or no lock support; multiple hosts each acquire.
- **Timing window:** concurrent holders on every host able to access the root.
- **Instrumentation:** missing mount identity, mount options, lock capability probe, and multi-host check.
- **Open-question log:** no sqlite deployment inventory was supplied. `storage` co-locates the file lease and sqlite database (`crates/storage/src/lib.rs:234-250`), so sqlite database placement determines this property. PostgreSQL's separate session-lock scope requires its own server/session-loss evidence, not a mount-capability check.
