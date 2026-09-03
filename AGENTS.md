# AGENTS.md

## What this repo is

Eidnara = destination of staged migration from source repo aliased `primitives`. Code lands in waves (`U1`, `U2`, `U3`, `U4`, `U5`, `U7`, `U8`; no `U6`). U1, U2 landed. Wave control records live in `migration/waves/<wave>/`, validated by `scripts/eidnara-migration/check.ts`. Most tracked files pinned by receipt, so ordinary code edit usually also needs control-record edit (see "Receipts pin bytes").

Two toolchains:

- Rust workspace under `crates/`. `storage-types` (serde-only descriptor) consumed by `storage` (rusqlite baseline open, `sqlite` default feature), which uses `lease` (OS advisory lock plus epoch fencing). `cache-stability` standalone. `rust-toolchain.toml` pins 1.98; CI also runs stable.
- Bun/TypeScript under `scripts/eidnara-migration/` (checker + tests). `package.json` declares `packages/*` workspace, no members yet.

## Commands

CI runs this. Takes seconds on warm `target/`.

```sh
bun install --frozen-lockfile
bun run check:repo        # typecheck, checker tests, every control-record check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo test --workspace --doc --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked
cargo check --workspace --no-default-features --locked
```

CI repeats clippy + test with `+stable`, re-runs `cargo check` after `cargo update` in scratch copy. Change that only builds with lockfile's exact versions still fails.

Focused runs:

- One Rust test: `cargo test -p lease <name>`. Crates: `cache-stability`, `lease`, `storage`, `storage-types`.
- One checker test: `bun test scripts/eidnara-migration/check.test.ts -t "<name>"`.
- One control record:
  `bun run eidnara:check <receipt|registry|waivers|property-catalog|property-impact|architecture-impact> <json-path>`.

`check:repo` names U1 + U2 files explicitly. CI globs `migration/waves/*/`. New wave must be added to both.

## Receipt checks need the source checkout

`eidnara:check receipt` verifies each source blob against git clone of source repo. Looks for `../<alias>` next to this checkout (`../primitives`) or takes `--checkout primitives=<dir>`. Without it check fails with "no checkout is available". Clone must contain pinned commit.

## Receipts pin bytes

Every file wave lands listed in `migration/waves/<wave>/receipt.json` with `destination_sha256`. Includes `crates/*/src/lib.rs`, `crates/*/Cargo.toml`, `Cargo.toml`, `Cargo.lock`, `bun.lock`, `scripts/eidnara-migration/check.ts`, `release/registry-gate.json`. Editing any fails receipt check with "destination_sha256 is stale" until entry updated (`sha256sum <file>`). Find owning receipt: `grep -l '<path>' migration/waves/*/receipt.json`.

Other pins that go stale with same edit:

- `migration/waves/U2/architecture-impact.json` has post-integration `modules_hash` over every `git ls-files` entry under all four `crates/*` directories. Any edit under `crates/` makes it stale. Fix = re-run review per `docs/runbooks/architecture-review.md`, not editing hash.
- Receipt file with `class: new-authored` needs `authored` entry in `migration/registry.json`.
- `property-impact.json` `touched_files` must list every code file receipt changes.

## Registry scan rules

`eidnara:check registry` scans production Rust (everything Cargo compiles into lib, bin, or build script, `#[cfg(test)]` items blanked) + TypeScript under `packages/`:

- String literal ending `.db`, `.sqlite`, `.bin`, `.lock`, `.jsonl`, or `.handle` must appear in `family` entry's `literals` in `migration/registry.json`.
- Tokens rejected anywhere: `schema_migrations`, `MIGRATIONS`, `LATEST_MIGRATION_VERSION`, `BOOTSTRAP_MIGRATION_VERSION`, `ensureColumn`, `Migration {`, `fn migrate`, `run_migrations`. Each SQL family has one baseline, no upgrade path. `storage::open_sqlite` refuses file whose schema differs from baseline; never migrates.
- Every tracked file (content + path, lockfiles excluded) scanned for retired source-project names, stored as digests. If error names digest, offending token = name from source implementation. Do not name source crates or source project in code, docs, or paths.

## Generated and byte-stable files

- `docs/properties/<part>/index.json` generated from `catalog.md`. Edit `catalog.md`, then run `bun run eidnara:property-index docs/properties/<part>`. CI runs with `--check`.
- `crates/storage-types/tests/golden/storage_vectors.json` regenerates with `cargo run -p storage-types --example golden-vectors`; `fixtures/schema/storage-inventory-v1.json` with `cargo run -p storage --example schema-inventory`. Both byte-stable contracts. Change = reviewed contract change, needs receipt hash updated. `crates/cache-stability/tests/golden/*.json` hand-authored.
- `bun run eidnara:registry-audit` without `--check` calls `npm view`, rewrites `release/registry-gate.json`, which U1 receipt pins. Use `--check` to validate. Run `git checkout -- release/registry-gate.json` if ran by mistake. CI does not run this script.

## Property catalogs and evidence

`docs/properties/METHOD.md` = pinned contract for everything under `docs/properties/`. Rules that bite: verify every line reference against HEAD; never run formatter or directory-wide edit there; `Check` semantics exactly `always`, `always-or-unreached`, `sometimes`, `reachable`, or `unreachable`. Test used as check anchor must be runnable: not `#[ignore]`, not behind `cfg` false in every build. TS anchor = `test`, `it`, or `describe` call, not `.skip` or `.todo`.

## Waivers

`migration/waves/<wave>/waivers.json` can waive failing gate of kind `release`, `parity`, `repo`, or `other`. `architecture` + `property` gates cannot be waived. Waiver expires once its `expires_by_wave` wave landed.

## Conventions

- Rustdoc builds with `-D warnings`, every intra-doc link must resolve.
- `lease` + `storage` carry `cfg(windows)` paths + `windows-sys` dependency, but CI Linux only. Code not exercised.