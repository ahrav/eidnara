# `handle-drop-releases-lease`

- **Discovery:** targeted lifecycle pass after portfolio evaluation.
- **Primary evidence:** `HeldFileLease` owns the locked file, and its `Drop` implementation calls standard-library `File::unlock`. Acquisition and read failures also call `File::unlock` before returning. Reacquisition tests cover normal drops.
- **Toolchain mechanism:** standard-library `File::try_lock`, `File::try_lock_shared`, and `File::unlock` provide acquisition and release. Descriptor close follows the best-effort explicit `File::unlock` call when `Drop` returns.
- **Failure scenario:** the best-effort explicit `File::unlock` call fails and descriptor close also does not release the lock promptly; a successor remains blocked.
- **Timing window:** last-handle drop while a competitor that has observed `Held` continues retrying.
- **Instrumentation:** retry-attempt timestamps, explicit last-handle event, and scheduler-fairness assumption.
- **Open-question log:** in the source (`primitives@89abb40`), the workspace and crate manifests declare MSRV 1.89, the release in which these standard-library file-locking APIs stabilized, while the source CI installs only the moving stable toolchain, so compatibility with the declared MSRV is unverified there. The U2 update below records this workspace's state.

## U2 update

The destination workspace pins `rust-version = "1.98"` in `Cargo.toml` and `channel = "1.98"` in `rust-toolchain.toml`. CI installs that toolchain and moving stable; on 1.98 it runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, doctests, `cargo doc` with `-D warnings`, and the default and all-features configurations, all under `--locked`; on stable it runs clippy and tests under `--locked`; and it checks the pinned toolchain against a regenerated lockfile of latest dependencies. The open question about an unverified declared MSRV is closed for this tree. The source observation above stands as provenance.
