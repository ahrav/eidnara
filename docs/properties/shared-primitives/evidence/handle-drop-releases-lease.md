# `handle-drop-releases-lease`

- **Discovery:** targeted lifecycle pass after portfolio evaluation.
- **Primary evidence:** `HeldFileLease` owns the locked file, and its `Drop` implementation calls standard-library `File::unlock`. Acquisition and read failures also call `File::unlock` before returning. Reacquisition tests cover normal drops.
- **Toolchain mechanism:** standard-library `File::try_lock`, `File::try_lock_shared`, and `File::unlock` provide acquisition and release. Descriptor close follows the best-effort explicit unlock when `Drop` returns.
- **Failure scenario:** best-effort explicit unlock fails and descriptor close also does not release the lock promptly; a successor remains blocked.
- **Timing window:** last-handle drop while a competitor that has observed `Held` continues retrying.
- **Instrumentation:** retry-attempt timestamps, explicit last-handle event, and scheduler-fairness assumption.
- **Open-question log:** workspace and crate manifests declare MSRV 1.89 (`Cargo.toml:9-15`; `crates/lease/Cargo.toml:1-9`), when these standard-library file-locking APIs stabilized. CI installs only the moving stable toolchain (`commons@89abb40 .github/workflows/ci.yml:21-42`,64-73`), so compatibility with the declared MSRV is unverified.

## U2 update

The destination workspace pins `rust-version = "1.98"` in `Cargo.toml` and `channel = "1.98"` in `rust-toolchain.toml`. CI installs that toolchain and moving stable, runs the suite on both under `--locked`, and checks the pinned toolchain against unlocked latest dependencies, so the open question about an unverified declared MSRV is closed for this tree. The source-repository observation above stands as provenance.
