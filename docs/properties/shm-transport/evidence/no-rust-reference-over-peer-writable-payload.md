# no-rust-reference-over-peer-writable-payload

## Discovery trigger

`crates/shm-transport/src/lease.rs` reads arena bytes three ways in one file,
and one of them differs from the other two. `read_byte` uses `read_volatile`,
`copy_to` uses `copy_nonoverlapping`, and `checksum` builds a `&[u8]` with
`std::slice::from_raw_parts`. The file's own doc comment on `ReceiveLease`
(`:88-89` (source tree; not at HEAD)) states the intent: "Raw span access avoids creating a long-lived safe
reference to memory a trusted peer could still address." `checksum` creates one.

## Evidence trail

- `crates/shm-transport/src/lease.rs:96-123` — `checksum`. Line 71 is
  `let bytes = unsafe { std::slice::from_raw_parts(self.base.as_ptr(), self.len)
  };`, then a fold over `bytes.iter()`. The slice is live across the whole fold.
- `crates/shm-transport/src/lease.rs:100-102` — the SAFETY comment justifies it
  with "R19 forbids peer writes before release", a contract term, not a
  mechanism.
- `docs/shm-transport.md:116` (source tree; not at HEAD) — the contract term the comment leans on,
  in full: "It does not protect against a malicious authenticated peer mutating
  mapped payload after publication, and tests or docs must not claim such
  immutability." The premise cited at `lease.rs:100-102` is the premise the document
  declines to guarantee.
- `crates/shm-transport/src/lease.rs:85-93` — `copy_to` uses
  `copy_nonoverlapping` (`:91`) with the same SAFETY reasoning, but no Rust
  reference is created, so a concurrent peer write is a torn value rather than
  undefined behaviour.
- `crates/shm-transport/src/lease.rs:63-82` — `read_byte` uses `read_volatile`
  at `:75-79`. Same property.
- `crates/shm-transport/src/lease.rs:330-348` — `to_vec`, the path the host
  actually uses, is built entirely on `copy_to`, so the aggregate body read is
  sound by the same argument.
- `crates/shm-transport/src/lease.rs:53-55` — `pub const fn as_mut_ptr(self)
  -> *mut u8` on a `Copy` receiver. It hands out a mutable pointer from a
  by-value `self`, so nothing in the type system limits how many live mutable
  pointers exist for one span.
- `crates/shm-transport/src/lib.rs:45` — `pub use lease::{LeaseSpan,
  ReceiveLease};`. Both the slice-building method and `as_mut_ptr` are crate
  public API, not internal helpers.
- `packages/shm-native/src/lib.rs:1431-1435` — the receive path calls
  `lease.segment(index)` then `napi_buffers::create_external_view(env,
  span.as_mut_ptr(), span.len())`.
- `packages/shm-native/src/napi_buffers.rs:60-140` — that helper calls
  `napi_create_external_arraybuffer` over the raw pointer. The result is an
  ordinary writable ArrayBuffer; nothing marks it read-only.
- `packages/shm-native/src/lib.rs:1039` and `:1121` — the same helper on the two
  produce paths, where a writable view is intended.

## Failure scenario

Two distinct exposures share one root.

The Rust one: any caller of `LeaseSpan::checksum` while the peer writes the same
bytes has a data race between a Rust shared reference and a foreign write. That
is undefined behaviour under Rust's aliasing model, not a wrong checksum. The
compiler is entitled to assume the slice is immutable for the fold's duration and
may hoist, split, or vectorize the loads accordingly.

The JavaScript one: the receive path exposes leased arena bytes as a writable
external ArrayBuffer. While the host is inside `to_vec`, JavaScript holding that
view can write the same range. `to_vec` uses `copy_nonoverlapping`, so this is a
torn body rather than undefined behaviour, but the descriptor the body was
validated against was read earlier, so a body can disagree with its own validated
length and wire header.

## Timing windows and dependencies

The `checksum` window is the duration of one fold over `len` bytes, up to
`MAX_FRAME_BYTES` (64 MiB), so it is wide. The JavaScript window spans from view
creation until the view is detached. Both windows are only reachable because
`Mapping::create` and `Mapping::attach` map the whole object
`PROT_READ | PROT_WRITE` (`ring.rs:462`, `:481`) and the required seals are
`F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL` with no `F_SEAL_WRITE`
(`crates/shm-transport/src/backend/sys.rs:24-25`). This is the same root decision behind
`quarantine-authority-survives-peer-writes` and
`reclaim-advance-bounded-by-the-producer-reservation`.

## What a test must construct

The audit form needs no fault: enumerate every method on `LeaseSpan` and
`ReceiveLease` that touches arena bytes and assert each uses `read_volatile` or
`copy_nonoverlapping`. That is a source-level or review-level check, and it fails
today at `lease.rs:71` (source tree; not at HEAD).

The impact demonstration needs a peer that writes leased bytes concurrently,
which is fault class F2 and does not exist. Under Miri or ThreadSanitizer the
`checksum` race would be reportable, and neither tool is configured anywhere in
the repository. A cheaper intermediate: run `checksum` against a span while a
second thread writes it, under `-Zsanitizer=thread`.

## Investigation log

### Q: Is `checksum` reachable from any non-bench caller? If it is bench-only, gating it removes the finding; if it is part of the intended read API, the slice needs to go.

- Sources examined: `grep -rn "checksum" crates/shm-transport/` and
  `packages/shm-native/`; `grep -rn "\.checksum()" crates/ packages/`
  excluding `node_modules`, `dist`, and `target`;
  `crates/shm-transport/src/lib.rs` for the export surface.
- Findings: exactly one call site exists in the entire tree —
  `crates/shm-transport/benches/hardware_envelope.rs:768`,
  `black_box(span.checksum())`. There are no callers in
  `crates/shm-transport/src`, no callers in any `tests/` directory, no callers
  in `packages/shm-native`, and no callers in any other crate. The other
  `checksum` matches in the repository are unrelated: evidence-manifest sidecar
  hashing in `crates/host-runtime/benches/support/evidence.rs`, authority seed
  checksums in `crates/daemon`, and store columns in `crates/context-store`. The
  method is nonetheless `pub` on a type re-exported at `lib.rs:45`, so it is part
  of the crate's public API and a downstream caller is admissible today.
- Missing evidence: none for reachability. The catalog's parenthetical "the only
  observed call sites are the bench and tests" overstates it — there is no test
  caller.
- Conclusion: resolved with answer. The finding is **gated, not live**: no
  non-bench, non-test caller exists at `9c1eb4d1`, and in fact no test caller
  exists either. It is not dead, because the method is public API. The record
  stays active as an audit-form property, with severity reduced from "a production
  read path is unsound" to "a public API method invites an unsound read, and one
  benchmark takes it". The sibling finding about `as_mut_ptr` and the writable
  receive-path ArrayBuffer is independent of this answer and is live.

### Q: Does `checksum` still form a slice at HEAD? (added 2026-09-05)

- Checked: `LeaseSpan::checksum` (`crates/shm-transport/src/lease.rs:96-123`) folds one `read_volatile` per byte and its comment says no `&[u8]` is formed; `copy_to` (`:85-93`) delegates to `volatile_copy`; `rg from_raw_parts crates/shm-transport/src/lease.rs` returns nothing. `LeaseSpan::as_mut_ptr` is still present.
- Conclusion: no. The finding the trail above describes is resolved; the residual impact is `as_mut_ptr` only.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 9, `:88-89`: All three readers now load through relaxed atomics whose width `AccessShape` fixes per byte, so none of them forms a Rust reference over arena memory.
  - line 14, `crates/shm-transport/src/lease.rs:69-75` now `crates/shm-transport/src/lease.rs:96-123`: `checksum` forms no slice at HEAD: it sums per-byte `AtomicU8` and per-word `AtomicU64` relaxed loads partitioned by `AccessShape`, so there is no `from_raw_parts` and no fold over a `&[u8]`.
  - line 17, `crates/shm-transport/src/lease.rs:70` now `crates/shm-transport/src/lease.rs:100-102`: The SAFETY comment no longer leans on a contract term; `R19` appears nowhere in the crate, and the justification is that `AccessShape` partitions the range and no Rust reference is formed.
  - line 26, `:62-64` now `:91`: `copy_to` delegates to `copy_out` (`crates/shm-transport/src/lease.rs:186-206`), which copies through relaxed atomic loads rather than `copy_nonoverlapping`.
  - line 30, `:53` now `:75-79`: `read_byte` loads through `AtomicU64::from_ptr` or `AtomicU8::from_ptr` with `Ordering::Relaxed`, not `read_volatile`.
  - line 73, `ring.rs:227` now `ring.rs:462`: Both mappings go through `sys::mmap_shared` (`crates/shm-transport/src/backend/sys.rs:86-104`), which is where `PROT_READ | PROT_WRITE` now lives.
  - line 101, `crates/shm-transport/benches/hardware_envelope.rs:406` now `crates/shm-transport/benches/hardware_envelope.rs:768`: The single call site is `checksum = checksum.wrapping_add(span.checksum())` in the leased-receiver peer loop, not a `black_box` call.
  - line 123, `crates/shm-transport/src/lease.rs:70-77` now `crates/shm-transport/src/lease.rs:96-123`: `checksum` sums relaxed `AtomicU8` and `AtomicU64` loads rather than one `read_volatile` per byte, and still forms no `&[u8]`.
  - line 123, `:60-67` now `:85-93`: `copy_to` delegates to `copy_out`, not to a `volatile_copy` helper.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 9, `:88-89` (the ReceiveLease doc comment about long-lived safe references): That sentence is gone; the same intent now sits in the `LeaseSpan` doc comment at `crates/shm-transport/src/lease.rs:10-13`, which states there is no `&[u8]` accessor.
  - line 20, `docs/shm-transport.md:116` (the mutable-payload disclaimer): `docs/shm-transport.md` is 98 lines at HEAD and carries no immutability disclaimer of that wording.
  - line 84, `lease.rs:71` (the from_raw_parts slice in checksum): No `from_raw_parts` remains in `lease.rs`, so the audit form of this check passes at HEAD.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
