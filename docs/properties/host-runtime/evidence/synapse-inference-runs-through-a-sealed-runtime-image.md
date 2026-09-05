# synapse-inference-runs-through-a-sealed-runtime-image

## Discovery trigger

`inference.rs:24` says `OrtIdentity` "binds a library path to certified
SHA-256 bytes, so `ensure_ort` rejects a different build before calling
`ort::init_from`". The bundle certification pins model and tokenizer bytes,
but the ONNX Runtime library is a separate file on disk that the loader
`dlopen`s. If the runtime were loaded by path, a library swapped after the
hash check but before `dlopen` would change every embedding while the lane
still advertised a certified identity. The audit traced how the verified
bytes reach the loader.

## Evidence trail

All references are at `e16e39e`, paths relative to `crates/host-runtime/`.

Verification. `verify_ort_library` (`src/synapse/inference.rs:105-161`)
validates the expected digest string (`:106-108`), opens the library with
`open_regular_file` (`:109-116`; `NOFOLLOW`, regular-file check at
`src/synapse/bundle.rs:641-664`), bounds the length by
`MAX_ORT_LIBRARY_BYTES` (`:22`, 512 MiB; check at `:117-121`), reads through
the checked descriptor (`:122-124`), and compares `sha256_hex(&bytes)` to the
identity (`:125-129`).

Sealing. The same `bytes` buffer is written to a fresh memfd named
`host-onnxruntime` created with `CLOEXEC | ALLOW_SEALING | EXEC` (`:131-134`),
falling back without `EXEC` on `EINVAL` for kernels before `MFD_EXEC`
(`:135-144`). After the write (`:149-150`) the buffer is dropped (`:151`) and
`fcntl_add_seals` applies `SHRINK | GROW | WRITE | SEAL` (`:152-159`). The
record's `Check` names the write and grow seals; the code applies all four.

Loading. `ensure_ort` (`:71-95`) calls `verify_ort_library` first (`:73`),
then `ort::init_from(verified.load_path())` (`:85`), where `load_path` is
`/proc/self/fd/<n>` (`:63-67`). The on-disk path is never handed to `ort`.
`ORT_COMMITTED` (`:53-54`) makes the identity process-global and first-wins:
a matching identity returns `Ok` (`:77-80`), a different one is an `Artifact`
error (`:81-83`), and a `commit()` that returns false because `ort` was
already initialised is also an error (`:87-91`). `Backend::load` calls
`ensure_ort` before constructing the model (`:170-171`), and `activate` calls
`Backend::load` once per activation (`src/synapse/mod.rs:1035-1036`).

Non-Linux. `ensure_ort` returns `Artifact("secure ONNX Runtime staging
requires Linux")` (`:97-102`), so the lane is `Disabled` rather than loaded
by path.

Existing check. `source_replacement_cannot_change_verified_loader_bytes`
(`:341-384`, `#[cfg(target_os = "linux")]`) writes a fake library, verifies
it, and asserts: the memfd carries all four seals (`:355-361`); a write
through a cloned descriptor fails (`:362-363`); after renaming a replacement
over the source (`:365-366`) the load path is under `/proc/self/fd` and
differs from the source (`:368-370`); the bytes read from the load path equal
the verified bytes and hash to the identity (`:371-376`); the source now holds
the replacement (`:378-381`). It never calls `ort::init_from`. The companion
`oversized_sparse_ort_library_fails_before_reading_or_allocating_its_length`
(`:386-411`) asserts the size bound rejects a sparse 512 MiB + 1 file.

Both are unit tests in the crate, run by CI via `cargo test --workspace
--all-targets` (`.github/workflows/ci.yml:118`) on `ubuntu-latest` (`:14`).
The full load into ONNX Runtime is exercised only by tests gated on
`EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` (`tests/synapse_bundle.rs:29-41`,
`tests/synapse_roundtrip.rs:27-38`), which CI does not set.

## Failure scenario

1. A bundle is certified against `libonnxruntime.so` with digest D.
2. An attacker with write access to the library directory replaces the file
   after `activate` hashes it but before the loader maps it.
3. A path-based `dlopen` would map the replacement; the lane would report
   `Ready` under fingerprint F while producing vectors from uncertified code.

As written, the hashed buffer and the written buffer are the same `Vec`
(`:122-149`), and the loader path names the memfd, so the window in step 2
does not exist. A replacement on disk is only observed at the next process
start, where it fails `:125-129` and disables the lane.

## Timing windows and dependencies

There is no time-of-check to time-of-use gap between hashing and staging,
because both operate on one in-memory buffer. The remaining dependency is
`dlopen` semantics: `VerifiedOrtLibrary` is a local in `ensure_ort` and its
descriptor closes when the function returns (`:71-95`). The loaded mapping
must therefore outlive the descriptor. Linux keeps a memfd's pages alive
while any mapping references them, so closing the fd after `dlopen` is safe,
but nothing in this crate asserts that `ort` has finished mapping before the
drop; it relies on `init_from` completing synchronously.

`ORT_COMMITTED` is checked after `verify_ort_library`, so two concurrent
`ensure_ort` calls both stage a memfd and only one commits; the loser's
memfd drops with its `VerifiedOrtLibrary`. This costs memory, not
correctness.

## What a test must construct

1. A load-path assertion with the real runtime: after `Backend::load`
   succeeds, read `/proc/self/maps` and assert the `onnxruntime` mapping's
   backing file is `/memfd:host-onnxruntime` and no mapping names the
   on-disk path. This needs `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY`.
2. A post-load replacement: with the real runtime, rename a different
   library over the source after `Backend::load`, embed, and assert the
   vectors still match the corpus.
3. A `MFD_EXEC` fallback case on a kernel with `vm.memfd_noexec=1`, where a
   memfd without `EXEC` cannot be mapped executable; today the fallback at
   `:135-144` is exercised only on kernels that return `EINVAL`.

## Investigation log

### Q: Is the loaded image's digest re-checked at load time?

- Sources examined: `src/synapse/inference.rs:71-95`, `:105-161`, `:343-384`.
- Findings: production code hashes the source bytes once (`:125`) and writes
  those bytes to the memfd (`:149`). There is no second hash of the memfd
  contents or of what `ort` mapped. The equality the record's `Check` asserts
  is established by construction (same buffer) and observed only in the unit
  test, which reads `/proc/self/fd/<n>` back and hashes it (`:371-376`).
- Missing evidence: none for the buffer identity; the test is the only
  place the memfd bytes are re-hashed.
- Conclusion: resolved with that scoping. The guarantee holds by
  construction, not by a runtime re-check.

### Q: Does `ort::init_from` load from the given path and nothing else?

- Sources examined: `src/synapse/inference.rs:85-91`; `Cargo.toml:21`
  (`ort = "=2.0.0-rc.13"`, `load-dynamic`).
- Findings: the crate passes the `/proc/self/fd` path and treats a false
  `commit()` as failure. I did not read `ort`'s `init_from` to confirm it
  does not consult `ORT_DYLIB_PATH` or a default search path when the given
  path fails, nor whether a failed `dlopen` of the memfd could fall back.
- Missing evidence: the `ort` 2.0.0-rc.13 `init_from` source, or a test
  with the real library that inspects the resulting mapping.
- Conclusion: unresolved, needs the `ort` source or the `/proc/self/maps`
  test described above.
