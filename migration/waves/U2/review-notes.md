# U2 review notes

Per-file review evidence for the U2 receipt. Source is
`primitives@89abb409b8c71b03146eedb5bf64cd964f2a92c0`. Every human-authored
file in the wave was read in full against the destination tree before its
receipt entry was written. Anchors below are the `review_evidence` pointers.

## Doc-rigor: Rust sources

Files: `crates/cache-stability/src/lib.rs`, `crates/lease/src/lib.rs`,
`crates/storage-types/src/lib.rs`, `crates/storage/src/lib.rs`,
`crates/cache-stability/tests/golden_vectors.rs`,
`crates/storage-types/tests/golden_vectors.rs`,
`crates/storage-types/examples/golden-vectors.rs`.

What was checked:

- Every crate path names a workspace crate (`lease`, `storage`,
  `storage-types`, `cache-stability`). No source crate name appears in code,
  comments, or manifests.
- Module and item docs describe this workspace: the `storage-types` module doc
  says a host resolves the descriptor; the `StorageBackend::Postgres` doc
  states that no backend in this workspace opens it and that
  `storage::open_sqlite` rejects it; the cache-stability module doc names the
  golden-vector contract as the thing consumers pin; the `fnv1a` doc explains
  that on-disk lease files outlive any one binary; the golden-vector example
  and test docs describe the fixture as byte-stable.
- Rustdoc builds with `-D warnings`. `GuardedConn` and `MaintenanceConn` are
  public re-exports: `with_conn` and `with_conn_fenced` name them in their
  signatures, so callers can use them by inference and rustdoc can link them.
- Durable identities are pinned by tests: `fence_and_version_tables_keep_their_ddl`
  pins the `eidnara_fence` and `eidnara_schema_version` DDL;
  `helpers_reproduce_the_golden_vectors` pins the `eidnara/` path component of
  `sqlite_store_path` and the `eidnara_` database-name prefix;
  `lease_path_vectors_are_version_stable` pins `LeaseKey::identity`, the
  `fnv1a_hex` digest, the `.lease` suffix, and the filename that acquisition
  creates; `EPOCH_WIDTH` is asserted by the epoch tests.
- Comments state mechanism in the present tense. Test messages describe the
  case under test, not a consumer.
- The four `lib.rs` files differ from the source blobs by the review fixes and
  their tests, so every line citation into them is derived from the current
  tree, not from the source.

Adaptations beyond renames, each recorded in the receipt as `adapted`:

| File | Change |
| --- | --- |
| `crates/cache-stability/src/lib.rs` | module doc; one test message; nested `if let` in `step_soft` collapsed into a let chain (same condition) |
| `crates/lease/src/lib.rs` | first doc line; `fnv1a` doc; six malformed-epoch cases; `LeaseKey::identity` rejects the `U+001F` separator; appended tests |
| `crates/storage-types/src/lib.rs` | module doc; `StorageBackend::Postgres` doc; `StorageDescriptor` doc; `Debug` redacts the DSN; `sqlite_store_path` rejects path components in `module_id`; appended tests |
| `crates/storage/src/lib.rs` | module doc first line; test temp-dir prefix; re-export of `GuardedConn` and `MaintenanceConn`; review fixes; appended tests |
| `crates/*/Cargo.toml` | crate names, `publish` and `rust-version` inherited, workspace dependency pins, descriptions without product branding |
| `crates/storage-types/examples/golden-vectors.rs`, `tests/golden_vectors.rs` | doc comments describe the byte-stable fixture |

## Byte-stable fixtures

`crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` and
`crates/storage-types/tests/golden/storage_vectors.json` are authored in this
tree (`source: null`, transformation `authored`, class `new-authored`).
`storage_vectors.json` is the output of
`cargo run -p storage-types --example golden-vectors` for seven sample module
ids; `helpers_reproduce_the_golden_vectors` and
`golden_vectors_break_slug_collisions` reproduce it from the in-tree code. The
cache-stability fixture is read by `crates/cache-stability/tests/golden_vectors.rs`.
The registry lists both as `byte-stable` fixtures, so the receipt checker
accepts only `verbatim` or `authored` as their transformation, and each
receipt entry's `destination_sha256` pins the bytes. Changing either file is a
reviewed contract change under R18.

## Doc-rigor: property catalog

Files under `docs/properties/shared-primitives/`.

- `catalog.md` is `adapted` from the source README. Every record was converted
  to the METHOD field order with enumerated values; three records needed a
  status word the source lacked (`Reachability` for all, `Exercised` heads for
  six records that started with a clause, a `Confidence` level for
  `failed-acquire-preserves-prior-epoch`). `returned-epoch-is-crash-durable`
  had `Status: unknown` and a `Question:` field; it is `active` with the
  guarantee stated and the unknown carried in `Exercised` and `Confidence`.
  The generated `index.json` passes `eidnara:check property-catalog`.
- Twenty-nine evidence files are `adapted`: paths point at destination files,
  line ranges are derived from the current tree, and PostgreSQL citations name
  `primitives@89abb40` with no path, because receipts verify source blobs by
  hash only. Three carry a `U2 audit` section (the `core` records).
- Support documents (`system-model.md`, `existing-checks.md`, `fault-map.md`,
  `relationships.md`, `portfolio-evaluation.md`,
  `durable-consumer-inventory.md`) are `adapted` with the same path rewrite, a
  provenance note, and, where U2 added records, a section for them.
  `existing-checks.md` gains rows for the four new or extended tests and marks
  the PostgreSQL rows as source-only.
- `lease-store-density.md` is `adapted` from the source's lease-store density
  note: crate name and an appended provenance section only.
- Seventeen new evidence files are `new-authored`; each records the discovery
  pass, primary and existing evidence with line citations, the failure
  scenario, and an audit verdict.

## Authored control records

| File | Purpose | Negative tests |
| --- | --- | --- |
| `migration/waves/U2/property-impact.json` | closure over the nine touched code files: 5 `core`, 41 `carried-forward` | `check.test.ts` property-impact suite (AE13, AE14, AE17, AE25) |
| `migration/waves/U2/architecture-impact.json` | pre-port and post-integration reports, four candidates, none Strong | `check.test.ts` architecture-impact suite (AE19-AE21) |
| `migration/waves/U2/waivers.json` | empty | `check.test.ts` waivers suite (AE30) |
| `Cargo.toml`, `Cargo.lock`, `crates/*/Cargo.toml` | workspace members, resolver 3, inherited `rust-version = "1.98"` and `publish = false`, dependency pins | `cargo metadata` graph check in CI; `--locked` builds on 1.98 and stable |
| `.github/workflows/ci.yml` | real workspace gates on the pinned 1.98 toolchain and stable, feature configurations, the pinned toolchain against unlocked dependencies, `cargo metadata` sibling and stub check | run locally step by step before commit |
| `scripts/eidnara-migration/check.ts`, `check.test.ts` | receipt, registry, waiver, and catalog validation; `fixture` registry kind with `byte-stable`, `generator`, and `external-record` roles; identity classes `frozen-durable`, `external-protocol`, `third-party` | `bun test scripts/eidnara-migration` |
| `migration/registry.json` | frozen DDL names, database-name prefix, store directory and `.lease` suffix, fixtures, persistent families, U2 authored paths | `eidnara:check registry` |
| `package.json` | `check:repo` covers the U2 receipt, waivers, property catalog, and impact records | `bun run check:repo` |

## Generated files

- `Cargo.lock`: `cargo +1.98 generate-lockfile`; verified by every `--locked`
  build and by the pinned-toolchain step that regenerates the lockfile against
  latest dependencies.
- `docs/properties/shared-primitives/index.json`:
  `generate-property-index.ts docs/properties/shared-primitives`; CI runs it
  with `--check`.

## Source gates not run here

The source repository's three-OS matrix, live PostgreSQL job, push-seal
version gate, and release tag resolver are recorded as `known_red` with
`not_run`. Each entry's `justification` states why: the genesis release ships
linux-x64-gnu only; the PostgreSQL backend is not carried; the version gate
belongs to a source crate that is not carried; every Eidnara crate is
`publish = false` (R21).

## METHOD template connector

`docs/properties/METHOD.md` arrived verbatim from the `host` source in U1. Its
prose rule forbids em dashes as connectors, but its own record template used
`—` between an enumerated field head and its note, and the U1 test fixture for
the index generator did the same. U2 changes the template and the fixture to a
spaced hyphen (`yes - <note>`), which the generator already accepts, and states
that rule next to the prose rule. No parsing behavior changes; the em dash is
still accepted for older content. The U1 receipt entry for `METHOD.md` is
`adapted` and points here.
