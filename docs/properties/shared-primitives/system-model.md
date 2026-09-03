# System model

Provenance: `primitives@89abb40`. Crate names are the workspace names; the
PostgreSQL backend in the source (`primitives@89abb40`) is described where the
source described it and is not part of this workspace.

System path: `crates/lease` at revision `9e871ce`, plus the U2 working-tree changes documented here.

## Architecture and data flow

`LeaseKey` contains `(module_id, backend, scope_key)` (`LeaseKey`). The fields are joined with `U+001F`, hashed with FNV-1a-64, and mapped to `<base_dir>/<16hex>.lease` (`LeaseKey::identity`, `fnv1a`, `fnv1a_hex`, and `FileLeaseStore::lease_path`).

`open_lease_file` first opens an existing final path with Unix `O_NOFOLLOW | O_NONBLOCK` or Windows `FILE_FLAG_OPEN_REPARSE_POINT`. On `NotFound`, it creates a same-directory `NamedTempFile`, writes canonical epoch zero, and calls `persist_noclobber`; an `AlreadyExists` race reopens the winner within three attempts. A successful publication returns the already-open temporary-file inode. Descriptor metadata rejects nonregular files and Windows reparse points (`crates/lease/src/lib.rs:56-90,119-156,244-324`). Exclusive and shared acquisition then use only `File::try_lock` or `File::try_lock_shared`. Both methods classify `TryLockError::WouldBlock` as `LeaseError::Held` and unwrap `TryLockError::Error` into `LeaseError::Io` (`crates/lease/src/lib.rs:244-324`).

The crate has no network or database boundary. Its authority boundaries are the filesystem path and kernel lock table. `storage` reads the SQLite fence as a resource floor, acquires above it, claims a strictly greater epoch before exposure, and rechecks it in every fenced write, DDL included (`crates/storage/src/lib.rs:170-216,617-740,1074-1111`).

## State and persistence

One file per derived key stores a decimal `u64`. Published files start with canonical epoch zero; ordinary acquisition rejects existing empty files. `acquire_above` treats an empty sidecar as its caller-supplied resource floor, which must cover every durable epoch previously authorized for the key. Existing nonempty content must contain 1-20 ASCII decimal digits; any longer, non-decimal, or out-of-range state fails closed (`read_epoch` (`crates/lease/src/lib.rs:391-420`)). Existing variable-width decimal files remain readable. Successful updates write exactly 20 decimal digits and use checked increment above the persisted epoch and optional resource floor, so `u64::MAX` is terminal (`bump_epoch_above` and `persist_epoch` (`crates/lease/src/lib.rs:437-448`)). There is no magic, key binding, checksum, format version, or generation.

The update does not truncate. An empty or 1-19 byte variable-width input is extended to 20 bytes with non-decimal markers before the canonical overwrite. For canonical 20-byte values, every prefix splice from the next epoch is either equal to or greater than the prior value. `interrupted_persist_never_leaves_a_lower_parseable_epoch` injects ordered prefix-write failures through `persist_epoch` for all three widths and parses aftermath through production `read_epoch`; the canonical case is where every prefix stays parseable, so the count of parseable aftermaths is asserted to keep the monotonicity oracle non-vacuous. It does not prove `File`, device, process-interruption, or power-loss behavior (`crates/lease/src/lib.rs:392-448,748-866`).

`flush` is not `sync_data` or `sync_all`. The crate makes no claim about exact partial-`File` I/O outcomes, process interruption, machine power loss, storage-cache loss, torn sectors, or filesystem reordering.

Lease files are not removed by production code. `lease-store-density.md:22-24` says this avoids an unlink-inode race; the source does not enforce the assumption against external actors.

## Concurrency model

There is no internal shared mutable state. Inter-process coordination is entirely the OS advisory lock held by `HeldFileLease.file`. The crate uses standard-library `File::try_lock`, `File::try_lock_shared`, and `File::unlock`; lock semantics still depend on the target OS and backing filesystem.

The epoch read-modify-write runs only after the exclusive lock succeeds. Shared readers each use a separate file descriptor and perform no epoch write.

## Claimed safety guarantees

- No two live writers for one logical store (`crates/lease/src/lib.rs:2`).
- Persisted, monotonically increasing writer epochs; stale writes are rejected (`crates/lease/src/lib.rs:4-6`, `bump_epoch_above`).
- Distinct module/backend/scope keys do not collide on a shared root (`LeaseKey::identity`, `FileLeaseStore::lease_path`).
- Shared and exclusive holder modes exclude each other (`FileLeaseStore::acquire`, `FileLeaseStore::acquire_shared`).
- Unix existing-file opens use `O_NOFOLLOW | O_NONBLOCK`, and permission changes apply through the opened descriptor. Windows existing-file opens use `FILE_FLAG_OPEN_REPARSE_POINT` and reject reparse-point metadata. New files are private `NamedTempFile` inodes initialized before no-clobber publication (`crates/lease/src/lib.rs:56-90,102-117,119-156`). Public `protect_file` remains path-based and Unix-only (`crates/lease/src/lib.rs:35-54`).
- Genuine contention maps to `LeaseError::Held` through `TryLockError::WouldBlock` (`crates/lease/src/lib.rs:256-324`).

These remain claims under test. Code disagreements are retained in the catalog.

## Claimed liveness guarantees

- Process death releases the advisory lock and permits reclaim without stale-PID cleanup (`crates/lease/src/lib.rs:4`).
- Dropping a guard releases its lease (`HeldFileLease::drop`).
- The parked density watcher is claimed to trigger when physical size crosses 1 GiB (`lease-store-density.md:39-51`).

## Bug history and density

- `8abefe8` extracted the lease and names a prior Windows contention-classification bug class.
- `16aed47` added shared mode.
- `49bcaa2` hardens file modes after a measured deployment had permissive files. Its commit message records an initial adjacent `storage` WAL test that could not fail, direct evidence that vacuity has occurred in this subsystem's check history.
- `bed0bb7` moved file locking to the standard library, declared Rust 1.89 as the workspace MSRV, and added the lease identity/hash stability vector.
- `8da6d42` made lease identity and hexadecimal hashing public so PostgreSQL could share the derivation, then added the advisory-key stability vector.
- `f2107e5` exposed numeric `fnv1a`, retained `fnv1a_hex` for filenames, and removed PostgreSQL's hex format/parse round trip.
- `94c65ec` bumped `lease` to 0.1.1 for the public API and MSRV change.
- `lease-store-density.md:7-13` reports 20,484 lease files and 80 MiB physical use from about 20 KiB logical content.

No issue or incident tracker was supplied, so history cannot establish additional reported defects.

## Existing test strategy

Twenty-six inline unit tests cover ordinary exclusivity, a same-process eight-way exclusive race, resource-floor issuance, synchronized concurrent shared-first acquisition, shared/exclusive behavior, simple key separation, separator-bearing key fields failing closed, new and variable-width epoch initialization, fail-closed epoch states, bounded epoch reads, injected ordered prefix-write failure, Unix symlink/FIFO refusal, permissions, two identity/hash/path stability vector sets, and one cross-process shared-lock case. There are no crash-image tests, fuzz targets, model checkers, or situation-coverage assertions. See [existing-checks.md](existing-checks.md).

## Failure and degradation

- Replacing a locked path creates a new lock domain.
- Unsupported or differently scoped filesystem locks can fail closed or fail open depending on behavior.
- Machine-power-loss and storage-tear behavior remain untested and unspecified.
- Windows acquisition rejects reparse points, but has compile-only coverage in this change; other non-Unix targets have no explicit no-follow flag.
- Shared acquisition needs write access because it creates and hardens the lease file.

## Dependencies

The crate uses `tempfile::NamedTempFile::persist_noclobber` for initialized no-clobber publication and depends on `libc` on Unix for `O_NOFOLLOW`, `O_NONBLOCK`, and the FIFO test. Its lock API needs Rust 1.89 or later; the workspace pins `rust-version = "1.98"` in `Cargo.toml` and `channel = "1.98"` in `rust-toolchain.toml`, and CI (`.github/workflows/ci.yml`) installs both 1.98 and moving stable, runs format, clippy with `-D warnings`, tests, doctests and docs, and the default and all-features configurations on 1.98 under `--locked`, runs clippy and tests on stable under `--locked`, and checks 1.98 against a regenerated lockfile of latest dependencies. The test suite also assumes `python3` on Unix for the cross-process test. The standard-library lock implementation and `TryLockError` behavior remain platform contracts.

## Product context

The lease guards module-owned durable stores. SQLite holds the exclusive handle for the store lifetime and exposes an epoch-fenced write API. Shared mode's documented example protects a blob reader against exclusive GC, but no production in-repo caller uses `acquire_shared`.

The density decision explicitly prioritizes single-writer correctness over reclaiming small files (`lease-store-density.md:15-34`).

## Unproven assumptions

- Lease roots are on filesystems whose advisory-lock scope matches all writers.
- Lease paths and inodes are not replaced while held.
- Key fields never contain the tuple separator and hash collisions need no handling.
- The intended epoch crash model is narrower than machine power loss.
- Regular-file writes are observed in issued byte order within the stated process model.
- Shared-handle epochs are never routed to writes despite no mode distinction in the type.
- External consumers preserve the lease-path format across overlapping versions.
- One OS account owns and can write each lease root.

Targeted portfolio follow-up found two additional boundary assumptions: every logical store has one canonical `(lease root, LeaseKey)`, and every durable mutation claimed as fence-protected reaches the fenced write API.

## Wildcard findings

- File-backed acquisition returns the concrete `HeldFileLease`; PostgreSQL remains a separate native session-lock implementation with no shared mode.
- `HeldFileLease::epoch` serves both modes, so shared guards still expose an observability-only value through the same concrete type.
- The source repository declared Rust 1.89 as the MSRV without compiling or testing with it. The destination pins Rust 1.98 and CI compiles and tests on that pinned toolchain and on moving stable, so this lead is closed here.
- External blocker and draft status are tracked in the [durable consumer inventory](durable-consumer-inventory.md).

## Property-lens coverage

| Lens | Result |
|---|---|
| Data integrity | Epoch monotonicity, crash durability, malformed state, failed-write preservation, key binding, and input bounds cataloged. |
| Concurrency | Exclusive uniqueness, mode matrix, inode stability, contention classification, and coverage races cataloged. |
| Failure recovery | Dead-holder reclaim, epoch interruption, and file replacement cataloged. |
| Protocol contracts | Fence use, key/path format, and backend write fencing cataloged. |
| Resource boundaries | Unbounded reads and lease-file growth cataloged. |
| Security boundaries | File mode and symlink/TOCTOU properties cataloged. |
| Distributed coordination | Consensus/quorum/election are not present. Filesystem lock scope is cataloged because it controls cross-host exclusion. |
| Lifecycle transitions | Acquire, held, drop, process death, restart, and rolling-version overlap represented. |
| Idempotency and replay | No request/replay protocol exists. Failed-attempt state preservation is cataloged. |
| Version compatibility | Lease-path persistence and shared cross-crate identity/FNV-1a derivation represented. |
| Wildcard | Guard-mode ambiguity, external backend divergence, and stale README claim retained as leads; the source repository's untested-MSRV lead is closed by the pinned 1.98 CI gate. |
