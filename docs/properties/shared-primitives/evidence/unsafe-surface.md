# Unsafe surface: inventory and injected-defect proofs

Companion to `docs/plans/2026-09-04-0652-refactor-unsafe-surface-confinement-plan.md`. Records what unsafe remains after the refactor, which lints and checkers guard it, and the one-line edits that were made, observed to fail, and reverted to prove each guard fires.

## Remaining unsafe, by file

Counts are `unsafe { }` blocks plus `unsafe fn` declarations at the commit that added this record.

| File | Blocks + fns | Category | Guard |
|---|---|---|---|
| `crates/shm-transport/src/backend/sys.rs` | 22 + 3 | one libc call per wrapper; `munmap`, `madvise_remove`, `mincore` are `unsafe fn` with `# Safety` | `undocumented_unsafe_blocks`, `unsafe_op_in_unsafe_fn`, valgrind |
| `crates/shm-transport/src/backend/ring.rs` (before `mod tests`) | 20 + 1 | `Mapping` accessors (`ptr_at`, `shared_page` as the one `unsafe fn`, page refs, `lifecycle_snapshot`, `lifecycle_quarantined`, `initialize_page`, `arena_ptr`, `resident_pages`, `Drop`), `DescriptorSlot` volatile read/write, two private `Ring` calls into `LeaseSpan::new` and `copy_in`. Every page type names its padding as an `UnsafeCell` field with an `offset_of!` assertion, so `&Page` covers no byte outside an atomic or `UnsafeCell` | same lints, Miri (`backend::ring::miri`), valgrind |
| `crates/shm-transport/src/backend/ring.rs` (`mod tests`) | 1 + 0 | `BorrowedFd::borrow_raw` in a cloexec test | `undocumented_unsafe_blocks` |
| `crates/shm-transport/src/lease.rs` | 11 + 3 | `LeaseSpan::new`, `copy_out`, `copy_in` (`unsafe fn`); `AtomicU8`/`AtomicU64::from_ptr` on the mapping side only, at the width `AccessShape` fixes per byte; test fixtures | same lints, Miri (`lease::`) |
| `crates/shm-transport/tests/ring.rs` | 8 + 0 | `ftruncate`/`fchmod`/`memfd_create` probes and inherited-fd adoption in the child role | `undocumented_unsafe_blocks` at the test crate root, valgrind |
| `crates/shm-transport/benches/hardware_envelope.rs` | 11 + 0 | `fork`, `mmap`, `getrusage`, `sched_yield`, `waitpid`, `kill` for the bench harness | `undocumented_unsafe_blocks` at the bench crate root; not shipped |
| `crates/lease/src/lib.rs` | 8 + 0 | two `geteuid` reads, Windows `GetFileInformationByHandle` + `assume_init`, test `umask`/`mkfifo` | existing safety comments; `deny(unsafe_code)` with enumerated allows lands through wave U2, whose receipt pins this file |
| `crates/storage/src/lib.rs` | 8 + 0 | test-only `umask`/`mkfifo` | existing safety comments; `deny(unsafe_code)` with enumerated allows lands through wave U2, whose receipt pins this file |
| `crates/tokenizer` | 0 | | `forbid(unsafe_code)` |
| `crates/storage-types`, `crates/cache-stability` | 0 | | `forbid(unsafe_code)` lands through wave U2, whose receipt pins these files |

Baseline before the refactor: 103 production blocks across three files, 91 of them in `ring.rs`, 14 without a safety comment, two private `unsafe fn` without a `# Safety` section, and an `unsafe fn` release callback carrying a `*const ()` context.

### `#[allow(unsafe_code)]` items for wave U2

The four crates whose `lib.rs` the U2 receipt pins (`cache-stability`, `lease`, `storage-types`, `storage`) take their lint attributes through the wave process, so the receipt's property records and `modules_hash` are regenerated against the audited tree rather than refreshed in place. The attributes to land there:

`crates/storage-types`, `crates/cache-stability`: `#![forbid(unsafe_code)]`.

`crates/lease/src/lib.rs`: `#![deny(unsafe_code)]` with `#[allow(unsafe_code)]` on `protect_open_file`, `require_private_directory`, `FileIdentity::of_file`, `link_count`, `a_fresh_lease_root_is_owner_only_under_a_permissive_umask`, `acquisition_refuses_fifo_without_blocking`.

`crates/storage/src/lib.rs`: `#![deny(unsafe_code)]` with `#[allow(unsafe_code)]` on `a_fifo_at_the_database_path_is_refused_before_sqlite_opens_it`, `a_fifo_at_the_journal_path_is_refused_before_inspection`, `the_inspection_copy_is_owner_only_under_a_permissive_umask`, `new_database_file_is_owner_only_at_creation`, `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask`.

Once landed, a new allow anywhere in these two crates shows up in `rg '#\[allow\(unsafe_code\)\]' crates/storage crates/lease`.

## Checker commands

```sh
cargo +1.98 clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +nightly-2026-07-27 miri test -p shm-transport --lib --locked -- lease:: backend::ring::miri
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --tool=memcheck --leak-check=full --errors-for-leak-kinds=definite --trace-children=yes --error-exitcode=1" \
  EIDNARA_SHM_SKIP_TWO_PROCESS=1 cargo +1.98 test -p shm-transport --test ring --locked
```

Recorded results at this commit: clippy clean; Miri 11 passed (nightly 1.99.0 2026-07-27, default `MIRIFLAGS`); valgrind 3.19.0 `ERROR SUMMARY: 0 errors`, `definitely lost: 0 bytes`, 12 passed, 1 ignored (the child role). The CI Miri step pins that nightly and fails unless its log shows at least one test passed, because `cargo miri test` exits zero when its filter matches nothing.

## Injected-defect proofs

Each row is a one-line edit made, observed, and reverted. A guard that no edit turns red is decorative; every guard below was shown red once.

| Guard | Edit | Observed |
|---|---|---|
| `forbid(unsafe_code)` on `tokenizer` | append `fn __probe() { unsafe { std::hint::unreachable_unchecked() } }` | `error: usage of an unsafe block`, pointing at `#![forbid(unsafe_code)]` |
| `deny(unsafe_code)` on `storage` (shown red on the pre-wave draft; the attribute lands through wave U2) | same probe appended outside any allowed item | `error: usage of an unsafe block`, pointing at `#![deny(unsafe_code)]` |
| `clippy::undocumented_unsafe_blocks` | delete the `// SAFETY:` above `geteuid` in `sys.rs` | `error: unsafe block missing a safety comment --> sys.rs:85:5` |
| `SharedDescriptor` layout assertions | move `span_count` above `allocation_start` | `error[E0080]: evaluation panicked: assertion failed: offset_of!(SharedDescriptor, allocation_start) == 64` |
| `Layout::slot_offset` depth bound | delete the `index >= self.depth` check | `miri::slot_index_past_depth_is_refused_before_any_dereference` fails |
| `ParkGuard` clears `parked` on every exit | replace the `Drop` store with a no-op | `reserve_until_deadline_leaves_the_capacity_wake_unparked` fails |
| `Doorbell::from_fd` socket-type check | delete the `socket_type == SOCK_STREAM` branch | `doorbell_attachment_requires_connected_unix_stream_socket` fails on the `UnixDatagram::pair` case |
| valgrind gate | `std::mem::forget(Box::new([7u8; 64]))` in an integration test | `64 bytes in 1 blocks are definitely lost`, `ERROR SUMMARY: 1 errors`, exit 1 |
| Miri concurrent-writer test, matched widths | replace the writer's `copy_in` with a per-byte `AtomicU8` store loop over the same range | `Undefined Behavior: Race condition detected between (1) 8-byte atomic load ... and (2) 1-byte atomic store`; this is why every party accesses a span through `AccessShape`, which fixes one width per byte from the absolute address |
| `AccessShape` partition | `access_shape_partitions_the_range_on_aligned_words` and `read_byte_agrees_with_copy_to_at_every_alignment` cover every alignment residue; a wrong `head` makes the word loads unaligned and Miri reports it | tests pass; guard is the alignment check Miri performs on every `AtomicU64::from_ptr` |
| Page padding is `UnsafeCell` | delete `_padding` from `ProducerPage` | `error[E0609]: no field _padding on type ProducerPage` at `assert!(offset_of!(ProducerPage, _padding) == 16)`; with the assertion also deleted, `&ProducerPage` would again span 112 bytes outside any `UnsafeCell` |
| `AtomicU8::from_ptr` read-write precondition | review finding: the first `atomic_copy` passed a `&[u8]`-derived pointer to `from_ptr`, which requires write validity | split into `copy_out`/`copy_in` so only mapping-side pointers reach `from_ptr`; `copy_out` takes `*mut u8` and every `# Safety` section states read-and-write validity, so the signature carries the requirement |

## Assumptions the tools cannot check

- A peer that writes a span while this side holds a lease or reservation on it violates the protocol; the honest producer writes only inside its reservation and the honest receiver reads only after the acquire on the slot state, so the two never race on arena bytes. If a peer does race, the Rust abstract machine cannot observe a foreign process; on the hardware this crate targets an aligned 8-byte load racing a byte store returns some mix of old and new bytes, which descriptor validation and checksums treat as untrusted data. Within one process (tests, Miri) both sides use `AccessShape`, so every racing pair has one width. Every access to a control page is an `AtomicU64`/`AtomicU8` operation.
- Miri models the writer as a Rust thread with atomic stores. It cannot observe a foreign process. valgrind memcheck observes memory errors in this process and the spawned child, not data races.
- The two-process exchange test is skipped under valgrind (`EIDNARA_SHM_SKIP_TWO_PROCESS`) because its 5 s peer deadlines assume native speed; it runs in the ordinary test step.

## Measured

Measured at the call sites through the public `Ring` API (`ProducerReservation::write` and `ReceiveLease::to_vec`, depth-7 ring, release build, `taskset` pinned, best of 5 x 3 runs, aarch64 Graviton, this host), against `origin/main` at `3ec87c1`:

| payload | phase | `origin/main` (`[u8; 8]` volatile copy) | per-byte `AtomicU8` (first draft of this branch) | `AccessShape` (shipped) |
|---|---|---|---|---|
| 256 B | write | 151 ns | 276 ns | 149 ns |
| 256 B | to_vec | 105 ns | 192 ns | 102 ns |
| 4 KiB | write | 1923 ns | 3444 ns | 1767 ns |
| 4 KiB | to_vec | 512 ns | 1757 ns | 351 ns |
| 64 KiB | write | 30278 ns | 54125 ns | 27274 ns |
| 64 KiB | to_vec | 6959 ns | 26871 ns | 4211 ns |

The per-byte draft was a 1.8x to 3.9x regression on both copy paths; LLVM never merges atomic accesses, so it compiled to one byte per instruction where the old inlined `[u8; 8]` volatile copy compiled to an 8-byte `ldr`/`str` pair. An earlier record of that comparison measured the old path through a non-inlined call, where the `[u8; 8]` round-trips through a stack slot, and understated its speed by 4x to 5x at 4 KiB and above; call-site numbers replace it. `try_reserve`, `commit`, `try_receive`, and `release` are unchanged within noise across all three columns.
