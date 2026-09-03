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
- Durable identities are pinned by tests: `fresh_file_matches_the_baseline_inventory`
  pins the `fence` and `format_marker` DDL, `PRAGMA application_id`,
  `PRAGMA user_version`, and the baseline digest against
  `fixtures/schema/storage-inventory-v1.json`;
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
| `crates/storage/src/lib.rs` | module doc; one-baseline `open_sqlite(descriptor, baseline)` with the `ExpectedIdentity` gate and `StoreError::Baseline`; `fence` and `format_marker` as the infrastructure tables; test temp-dir prefix; re-export of `GuardedConn`, `MaintenanceConn`, `SchemaObject`, `schema_inventory`, `APPLICATION_ID`, and `USER_VERSION`; review fixes; tests pass `KV_BASELINE` where they need a domain table |
| `crates/*/Cargo.toml` | crate names, `publish` and `rust-version` inherited, workspace dependency pins, descriptions without product branding |
| `crates/storage-types/examples/golden-vectors.rs`, `tests/golden_vectors.rs` | doc comments describe the byte-stable fixture |

## Byte-stable fixtures

`crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` and
`crates/storage-types/tests/golden/storage_vectors.json` are authored in this
tree (`source: null`, transformation `authored`, class `new-authored`).
`storage_vectors.json` is the output of
`cargo run -p storage-types --example golden-vectors` for seven sample module
ids; `helpers_reproduce_the_golden_vectors` and
`golden_vectors_break_slug_collisions` reproduce it from the in-tree code. Its
bytes differ from the source fixture because its inputs - the `eidnara/` path
component of `sqlite_store_path`, the `eidnara_` PostgreSQL database-name
prefix, and the sample module ids - are renamed identities, which is the R18
cause recorded for this fixture. The
cache-stability fixture is read by `crates/cache-stability/tests/golden_vectors.rs`.
The registry lists both as `byte-stable` fixtures, so the receipt checker
accepts only `verbatim` or `authored` as their transformation, and each
receipt entry's `destination_sha256` pins the bytes. Changing either file is a
reviewed contract change under R18.

## Baseline open

Files: `crates/storage/baseline.sql`, `crates/storage/examples/schema-inventory.rs`,
`fixtures/schema/storage-inventory-v1.json`.

`open_sqlite(descriptor, baseline)` takes one consumer DDL text. The store's
own objects from `baseline.sql` precede it. A pristine file receives the whole
text once, together with `PRAGMA application_id = 0x4549444e` (`EIDN`),
`PRAGMA user_version = 1`, and one `format_marker` row holding the SHA-256 of
the full text. Any other file must present exactly that identity - application
id, user version, `sqlite_schema` inventory, and marker digest - or the open
returns `StoreError::Baseline` before any pragma or transaction, so the file
keeps every byte. No file is upgraded, adopted, or repaired, and no version
ledger exists.

What was checked:

- `baseline.sql` defines only `fence` and `format_marker`. Both carry
  `CHECK (id = 0)`; `fence.epoch` carries `CHECK (epoch >= 0)`;
  `format_marker.baseline_sha256` carries
  `CHECK (length(baseline_sha256) = 64)`. No object name contains
  `schema_version` or `migration`.
- The digest covers the whole text: `ExpectedIdentity::for_baseline` hashes
  `baseline.sql` followed by a newline and the consumer text, and the same
  string is what `apply` executes, so a one-byte change to either part changes
  the marker.
- The inventory comes from SQLite: `for_baseline` applies the text to an
  in-memory database and reads `sqlite_schema` through `schema_inventory`, so
  the comparison in `classify` uses SQLite's own normalization of the DDL, not
  a second parser. The root page is excluded because it varies with allocation
  order.
- `cargo run -p storage --example schema-inventory` opens a fresh store against
  an empty consumer baseline, reads the marker, reopens the file raw, and
  prints the inventory as JSON. Its output is byte-identical to
  `fixtures/schema/storage-inventory-v1.json`.
- `fresh_file_matches_the_baseline_inventory` compares the raw reopened file
  against the literal values in the fixture (`include_str!`), not against the
  constants under test, so a mutant that renames a table, drops a `CHECK`, or
  changes the application id fails the comparison.
- `is_infrastructure_table` covers both table names; the authorizer denies
  every non-read action naming either inside a callback, and
  `infrastructure_objects` compares the infrastructure-named objects in the
  main and temp schemas before and after a callback, so a rename onto either
  name is caught before commit.
- The registry lists `fence`, `format_marker`, `0x4549444e`, and
  `user_version = 1` as `frozen-durable` identities, the fixture as
  `byte-stable`, and the example as its `generator`. The U4 families follow the
  same shape: one `baseline.sql` under the owning crate and a fresh-file
  inventory captured as `fixtures/schema/kernel-inventory-v1.json` and
  `fixtures/schema/context-inventory-v1.json`.

Negative tests, all in `crates/storage/src/lib.rs` and run by
`cargo test --workspace --all-targets --all-features --locked` on Rust 1.98
and stable:

- `fresh_file_matches_the_baseline_inventory`: fixture equality, object for
  object, plus exactly one marker row.
- `a_consumer_baseline_is_applied_once_and_verified_on_reopen`: the same text
  reopens and keeps its rows; a different text is refused and the file bytes
  are identical before and after.
- `a_baseline_that_does_not_apply_is_rejected_before_the_file_is_touched`: an
  unparseable text is refused and the database file is never created.
- `a_file_with_foreign_objects_is_refused_without_mutation`: a file holding a
  foreign table is refused with identical bytes and no `-wal` or `-shm`
  sidecar.
- `a_fenced_callback_cannot_rewrite_the_format_marker`: an `UPDATE` of the
  marker is denied and a temporary table renamed onto `format_marker` is
  rejected before commit.
- `a_callback_cannot_damage_the_fence_row_it_is_checked_against` and
  `negative_database_fence_fails_closed` cover the `fence` half: DML, triggers,
  indexes, views, drops, and renames onto `fence` are denied, and a negative
  epoch written through `ignore_check_constraints` fails closed.

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
  hash only. Three carry a `U2 audit` section (the lease `core` records); the
  three other `core` records carry their verdict in an `Audit verdict (U2)`
  line.
- Support documents (`system-model.md`, `existing-checks.md`, `fault-map.md`,
  `relationships.md`, `portfolio-evaluation.md`,
  `durable-consumer-inventory.md`) are `adapted` with the same path rewrite, a
  provenance note, and, where U2 added records, a section for them.
  `existing-checks.md` lists every `storage` test with a line range derived
  from the current tree, carries a production-guard row for the baseline
  identity gate, and marks the PostgreSQL rows as source-only.
- `lease-store-density.md` is `adapted` from the source's lease-store density
  note: crate name and an appended provenance section only.
- Sixteen new evidence files are `new-authored`; each records the discovery
  pass, primary and existing evidence with line citations, the failure
  scenario, and an audit verdict.

## Authored control records

| File | Purpose | Negative tests |
| --- | --- | --- |
| `migration/waves/U2/property-impact.json` | closure over the nine touched code files: 6 `core`, 39 `carried-forward` | `check.test.ts` property-impact suite (AE13, AE14, AE17, AE25) |
| `migration/waves/U2/architecture-impact.json` | pre-port and post-integration reports, four candidates, none Strong | `check.test.ts` architecture-impact suite (AE19-AE21) |
| `migration/waves/U2/waivers.json` | empty | `check.test.ts` waivers suite (AE30) |
| `Cargo.toml`, `Cargo.lock`, `crates/*/Cargo.toml` | workspace members, resolver 3, inherited `rust-version = "1.98"` and `publish = false`, dependency pins | `cargo metadata` graph check in CI; `--locked` builds on 1.98 and stable |
| `.github/workflows/ci.yml` | real workspace gates on the pinned 1.98 toolchain and stable, feature configurations, the pinned toolchain against unlocked dependencies, `cargo metadata` sibling and stub check | run locally step by step before commit |
| `scripts/eidnara-migration/check.ts`, `check.test.ts` | receipt, registry, waiver, and catalog validation; `fixture` registry kind with `byte-stable`, `generator`, and `external-record` roles; identity classes `frozen-durable`, `external-protocol`, `third-party`; registry verification rejects migration-machinery tokens (`schema_migrations`, `MIGRATIONS`, `LATEST_MIGRATION_VERSION`, `BOOTSTRAP_MIGRATION_VERSION`, `ensureColumn`, `Migration {`, `fn migrate`, `run_migrations`) and unregistered persistent literals in every file under `crates/*/src` and `packages/*/src`, with `#[cfg(test)]` items blanked out rather than files skipped by a test-like name | `bun test scripts/eidnara-migration` |
| `migration/registry.json` | frozen `fence` and `format_marker` table names, `application_id` and `user_version`, database-name prefix, store directory and `.lease` suffix, fixtures and generators, persistent families with their `baseline_source`, U2 authored paths, and `retired-identity` entries that carry the SHA-256 of each source-implementation name in normalized form (lower-case, `-` and `_` removed) so the registry gate refuses any tracked text file in which a token, or any contiguous run of a token's `-`/`_`-separated parts, normalizes to one, without the registry spelling the name | `eidnara:check registry` |
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
