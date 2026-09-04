# Unsafe surface: inventory and injected-defect proofs

Companion to `docs/plans/2026-09-04-0652-refactor-unsafe-surface-confinement-plan.md`. Records what unsafe remains after the refactor, which lints and checkers guard it, and the one-line edits that were made, observed to fail, and reverted to prove each guard fires. Counts are of source lines containing the `unsafe` keyword, by file, at the commit that added this record.

## Remaining unsafe, by file

| File | Lines | Category | Guard |
|---|---:|---|---|
| `crates/shm-transport/src/backend/sys.rs` | 17 | one libc call per wrapper; `munmap`, `madvise_remove`, `mincore` are `unsafe fn` with `# Safety` | `undocumented_unsafe_blocks`, `unsafe_op_in_unsafe_fn`, valgrind |
| `crates/shm-transport/src/backend/ring.rs` (before `mod tests`) | 21 | `Mapping` accessors (`ptr_at`, `shared_page`, page refs, `lifecycle_snapshot`, `lifecycle_quarantined`, `initialize_page`, `arena_ptr`, `resident_pages`, `Drop`), `DescriptorSlot` volatile read/write, two private `Ring` calls into `LeaseSpan::new` and `atomic_copy` | same lints, Miri (`backend::ring::miri`), valgrind |
| `crates/shm-transport/src/backend/ring.rs` (`mod tests`) | 1 | `BorrowedFd::borrow_raw` in a cloexec test | `undocumented_unsafe_blocks` |
| `crates/shm-transport/src/lease.rs` | 9 | `LeaseSpan::new` (`unsafe fn`), `atomic_copy` (`unsafe fn`), per-byte `AtomicU8::from_ptr` loads, test fixtures | same lints, Miri (`lease::`) |
| `crates/shm-transport/tests/ring.rs` | 8 | `ftruncate`/`fchmod`/`memfd_create` probes and inherited-fd adoption in the child role | `undocumented_unsafe_blocks` at the test crate root, valgrind |
| `crates/shm-transport/benches/hardware_envelope.rs` | 11 | `fork`, `mmap`, `getrusage`, `sched_yield`, `waitpid`, `kill` for the bench harness | `undocumented_unsafe_blocks` at the bench crate root; not shipped |
| `crates/lease/src/lib.rs` | 8 | two `geteuid` reads, Windows `GetFileInformationByHandle` + `assume_init`, test `umask`/`mkfifo` | `deny(unsafe_code)` with `#[allow(unsafe_code)]` on six named items |
| `crates/storage/src/lib.rs` | 8 | test-only `umask`/`mkfifo` | `deny(unsafe_code)` with `#[allow(unsafe_code)]` on five test functions |
| `crates/storage-types`, `crates/cache-stability`, `crates/tokenizer` | 0 | | `forbid(unsafe_code)` |

Baseline before the refactor: 103 production blocks across three files, 91 of them in `ring.rs`, 14 without a safety comment, two private `unsafe fn` without a `# Safety` section, and an `unsafe fn` release callback carrying a `*const ()` context.

### `#[allow(unsafe_code)]` items

`crates/lease/src/lib.rs`: `protect_open_file`, `require_private_directory`, `FileIdentity::of_file`, `link_count`, `a_fresh_lease_root_is_owner_only_under_a_permissive_umask`, `acquisition_refuses_fifo_without_blocking`.

`crates/storage/src/lib.rs`: `a_fifo_at_the_database_path_is_refused_before_sqlite_opens_it`, `a_fifo_at_the_journal_path_is_refused_before_inspection`, `the_inspection_copy_is_owner_only_under_a_permissive_umask`, `new_database_file_is_owner_only_at_creation`, `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask`.

A new allow anywhere in these two crates shows up in `rg '#\[allow\(unsafe_code\)\]' crates/storage crates/lease`.

## Checker commands

```sh
cargo +1.98 clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +nightly miri test -p shm-transport --lib --locked -- lease:: backend::ring::miri
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --tool=memcheck --leak-check=full --errors-for-leak-kinds=definite --trace-children=yes --error-exitcode=1" \
  EIDNARA_SHM_SKIP_TWO_PROCESS=1 cargo +1.98 test -p shm-transport --test ring --locked
```

Recorded results at this commit: clippy clean; Miri 9 passed (nightly 1.99.0 2026-07-27, default `MIRIFLAGS`); valgrind 3.19.0 `ERROR SUMMARY: 0 errors`, `definitely lost: 0 bytes`, 12 passed, 1 ignored (the child role).

## Injected-defect proofs

Each row is a one-line edit made, observed, and reverted. A guard that no edit turns red is decorative; every guard below was shown red once.

| Guard | Edit | Observed |
|---|---|---|
| `forbid(unsafe_code)` on `tokenizer` | append `fn __probe() { unsafe { std::hint::unreachable_unchecked() } }` | `error: usage of an unsafe block`, pointing at `#![forbid(unsafe_code)]` |
| `deny(unsafe_code)` on `storage` | same probe appended outside any allowed item | `error: usage of an unsafe block`, pointing at `#![deny(unsafe_code)]` |
| `clippy::undocumented_unsafe_blocks` | delete the `// SAFETY:` above `geteuid` in `sys.rs` | `error: unsafe block missing a safety comment --> sys.rs:85:5` |
| `SharedDescriptor` layout assertions | move `span_count` above `allocation_start` | `error[E0080]: evaluation panicked: assertion failed: offset_of!(SharedDescriptor, allocation_start) == 64` |
| `Layout::slot_offset` depth bound | delete the `index >= self.depth` check | `miri::slot_index_past_depth_is_refused_before_any_dereference` fails |
| `ParkGuard` clears `parked` on every exit | replace the `Drop` store with a no-op | `reserve_until_deadline_leaves_the_capacity_wake_unparked` fails |
| `Doorbell::from_fd` socket-type check | delete the `socket_type == SOCK_STREAM` branch | `doorbell_attachment_requires_connected_unix_stream_socket` fails on the `UnixDatagram::pair` case |
| valgrind gate | `std::mem::forget(Box::new([7u8; 64]))` in an integration test | `64 bytes in 1 blocks are definitely lost`, `ERROR SUMMARY: 1 errors`, exit 1 |
| Miri concurrent-writer test | word-wide `AtomicUsize` copy against byte-wide writer stores (tried during U5) | `Undefined Behavior: Race condition detected between (1) 1-byte atomic store ... and (2) 8-byte atomic load`; this is why every span access is one byte wide |

## Assumptions the tools cannot check

- The peer process writes shared memory with whole-byte stores. Every access this crate makes through a `LeaseSpan` is a one-byte relaxed atomic, and every access to a control page is an `AtomicU64`/`AtomicU8` operation, so a peer using ordinary store instructions cannot create a mixed-size race. A peer that tears a byte is a protocol violation; descriptor validation already treats every shared value as untrusted.
- Miri models the writer as a Rust thread with atomic stores. It cannot observe a foreign process. valgrind memcheck observes memory errors in this process and the spawned child, not data races.
- The two-process exchange test is skipped under valgrind (`EIDNARA_SHM_SKIP_TWO_PROCESS`) because its 5 s peer deadlines assume native speed; it runs in the ordinary test step.

## Measured

`atomic_copy` per-byte relaxed atomics vs the previous `[u8; 8]` volatile copy, release build, this host: 256 B 52 ns vs 141 ns; 4 KiB 1.2 us vs 2.2 us; 64 KiB 17 us vs 36 us. A word-wide atomic path measured 30 GB/s but requires the peer to match access width; recorded as follow-up work in the plan.
