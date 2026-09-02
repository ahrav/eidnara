# `shared-exclusive-exclusion-matrix`

- **Discovery:** concurrency and lifecycle passes.
- **Primary evidence:** `FileLeaseStore::acquire` uses `File::try_lock`; `FileLeaseStore::acquire_shared` uses `File::try_lock_shared`; both map `WouldBlock` to `Held`.
- **Existing evidence:** `shared_holders_coexist_but_block_exclusive`, `exclusive_holder_blocks_shared`, and `shared_lease_across_processes_blocks_exclusive`, including the discriminating step where one of two shared holders drops and exclusive remains blocked. `concurrent_shared_first_acquisitions_coexist` covers synchronized publication/open races on a fresh key.
- **Failure scenario:** process-scoped lock emulation or premature unlock lets exclusive coexist with a remaining shared holder.
- **Timing window:** exclusive attempt after the first shared holder drops but before the last drops.
- **Instrumentation:** partial; tests observe API outcomes but not live-holder counts or inode identity.
- **Open-question log:** locking uses the standard library's `File::try_lock` and `File::try_lock_shared`, with contention and other failures classified through `TryLockError`. These APIs set the workspace MSRV to Rust 1.89 (`Cargo.toml:14-15`). Deployment filesystem support beyond exercised platforms needs human confirmation.
