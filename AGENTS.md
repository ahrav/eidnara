# AGENTS.md

Rust workspace (`crates/*`, `packages/shm-native`) plus thin Bun/TypeScript layer. Rust 1.98 pinned by `rust-toolchain.toml`. CI run clippy + tests on both 1.98 and `stable` (stable lane skips itself when `rustc +stable --version` equals the pin), so code must build warning-free on both. Bun 1.3.14 in CI. Edition 2024, `rustfmt` style edition 2024.

## Layout

- `crates/host-runtime` = app. Directly linked host. Serve `context`, `synapse`, `broca` components over local shared-memory ring. `docs/host-wire-protocol.md` = normative wire contract. Names literals; no rename without versioned protocol change.
- `crates/shm-transport` = ring core, only crate holding `unsafe` (`#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]`, so every `unsafe` block need `// SAFETY:` comment). `host-runtime` is `deny(unsafe_code)` except Broca's `pre_exec` hook. `tokenizer` is `forbid(unsafe_code)`.
- `packages/shm-native` = N-API `cdylib` over `shm-transport` plus `index.ts`. Cargo workspace member and Bun workspace package.
- `crates/lease`, `crates/storage`, `crates/storage-types` = storage primitives. Storage baseline-only: one schema (`crates/storage/baseline.sql`), no version ledger, no upgrade path.
- `crates/tokenizer` = Claude byte-BPE port, checked against `ai-tokenizer`.
- `crates/shm-transport/fuzz` = separate workspace (excluded from root), own `Cargo.lock`.

## Commands (mirror CI exactly; always pass `--locked`)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked   # ~2.5 min warm
cargo test --workspace --doc --all-features --locked            # --all-targets skips doctests
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked
cargo check --workspace --no-default-features --locked          # storage's `sqlite` is a default feature
cargo check -p storage --no-default-features --locked           # memory-store names storage/sqlite, so the workspace check keeps it on
cargo fmt   --manifest-path crates/shm-transport/fuzz/Cargo.toml --all -- --check
cargo check --manifest-path crates/shm-transport/fuzz/Cargo.toml --locked --bins
bun install --frozen-lockfile && bun run check:repo             # root + package typecheck
```

Single test: `cargo test -p <crate> --test <file> <name>`. Unit tests: `cargo test -p <crate> --lib <name>`. Tokenizer unit tests take ~20 s (proptest).

CI also run `cargo update` in scratch copy and re-check, so code must compile against newest semver-compatible dependencies, not just lock.

## Gates you must run after touching `shm-transport` unsafe code

Files: `crates/shm-transport/src/lease.rs`, `src/backend/ring.rs`.

```sh
# Miri, ~3.5 min. CI greps for ">=1 passed", so keep tests under these module paths.
cargo +nightly-2026-07-27 miri test -p shm-transport --lib --locked -- lease:: backend::ring::miri

# valgrind. CI sets the X86_64 runner; use the triple of your machine.
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --tool=memcheck --leak-check=full --errors-for-leak-kinds=definite --trace-children=yes --error-exitcode=1" \
EIDNARA_SHM_SKIP_TWO_PROCESS=1 cargo test -p shm-transport --test ring --locked
```

## Test quirks

- `crates/host-runtime/tests/broca_subprocess.rs` has `harness = false`. Re-executes itself as fake OpenCode/Pi harness via `EIDNARA_BROCA_FIXTURE_MODE`. New test there must be added to `tests` array in `main`; `#[test]` alone does nothing. Accepts substring filter and `--exact`.
- `#[ignore]`d tests are two kinds. Do not un-ignore either:
  - Need external inputs: `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` (ONNX Runtime `.so`), `EIDNARA_SYNAPSE_PRODUCTION_BUNDLE`, `EIDNARA_SHM_SOAK_SECONDS`, U9 closure roots. Run with `-- --ignored` when you have them.
  - Child-process roles (`shm_role_client`, `ring_child_exchange`) that parent test spawns. Not skipped tests.
- `host-runtime`'s `test-support` feature only gates cross-crate re-export for downstream tests. CI runs `--all-features`, so keep it compiling.
- Set `EIDNARA_SHM_SKIP_TWO_PROCESS=1` to skip two-process ring exchange (its 5 s deadlines meaningless under valgrind).
- Criterion benches with heavy fixtures (`kernel/benches/scope_algebra.rs`, `context-core/benches/windowed.rs`) return early unless `--bench` is in argv, so `cargo test --all-targets` only compiles them. `cargo bench -- --test` still runs every cell once. Copy that `main` for any new bench whose setup takes more than a few seconds.

## Native addon (`packages/shm-native`) is Linux x86_64 only

- `build:native` refuses any other `uname`. `index.ts` refuses to load addon off `linux-x64`. On aarch64 dev box the Bun suite passes vacuously with `reason: "addon_unavailable"`. That is not evidence addon works.
- Real run (CI, x86_64): `EIDNARA_SHM_NATIVE_CLAIMED_TARGET=1 bun run --cwd packages/shm-native build:native`, then `typecheck`, `test`, `test:capability:bun`. Flag turns addon load failure into test failure instead of skip.
- `shm_native.node` and `index.js` are build outputs, gitignored.

## Docs conventions

- `docs/properties/` holds property catalogs with fixed record schema. Read `docs/properties/METHOD.md` first: verify every `file:line` against HEAD, never fabricate to close open question, never run directory-wide formatter there, treat documented guarantee as claim under test.
- `docs/runbooks/architecture-review.md` = human process, not automation.
- Rationale comments end with `commentlint: allow(JUDGE)` (146 sites). Nothing in-repo enforces it. Keep marker when editing such comments. Add it to new comments that state judgement, not mechanism.

## Git (observed, not enforced)

- Branches: `u<wave>/<n>-<topic>` (e.g. `u3r/4-host-kernel-state`). PRs merge to `main`.
- Commit subject: one imperative sentence, no type prefix, no trailing period. Body: short prose on why, then bullets naming files or records changed and what each now says.
