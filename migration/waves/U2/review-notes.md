# U2 review notes

Per-file review evidence for the U2 receipt. Source is
`commons@89abb409b8c71b03146eedb5bf64cd964f2a92c0`. Every human-authored file in
the wave was read in full against the destination tree before its receipt
entry was written. Anchors below are the `review_evidence` pointers.

## Doc-rigor: Rust sources

Files: `crates/cache-stability/src/lib.rs`, `crates/lease/src/lib.rs`,
`crates/storage-types/src/lib.rs`, `crates/storage/src/lib.rs`,
`crates/cache-stability/tests/golden_vectors.rs`,
`crates/storage-types/tests/golden_vectors.rs`,
`crates/storage-types/examples/golden-vectors.rs`.

What was checked:

- Every crate path (`cortexkit_lease`, `cortexkit_store`,
  `cortexkit_store_types`, `cortexkit_cache_core`) now names the destination
  crate. The registry scan reports a renamed identity in either spelling before
  any retained substring can excuse it.
- Module and item docs describe the destination: the `storage-types` module doc
  no longer names `subc` as the resolver and says a host resolves the
  descriptor; the `StorageBackend::Postgres` doc states that no backend in this
  workspace opens it and that `storage::open_sqlite` rejects it; the
  cache-stability module doc names the golden-vector contract rather than two
  predecessor harnesses; the `fnv1a` doc explains that on-disk lease files
  outlive any one binary instead of naming the dropped PostgreSQL crate; the
  golden-vector example and test docs describe the fixture as byte-stable
  rather than as a twin of a TypeScript package that is not in scope.
- Rustdoc builds with `-D warnings`. The only change that needed was making
  `GuardedConn` and `MaintenanceConn` public re-exports: `with_conn` and
  `with_conn_fenced` already named them in their signatures, so callers could
  use them by inference but rustdoc could not link them.
- Durable bytes are unchanged: `cortexkit_fence` and `cortexkit_schema_version`
  DDL, the `cortexkit/` path component in `sqlite_store_path`, the `cortexkit_`
  database-name prefix, `LeaseKey::identity` and `fnv1a_hex`, the `.lease`
  suffix, `EPOCH_WIDTH`, and both golden fixtures.
  `fence_and_version_tables_keep_their_ddl` pins the `cortexkit_fence` and
  `cortexkit_schema_version` DDL. `lease_path_vectors_are_version_stable` pins
  `LeaseKey::identity`, the `fnv1a_hex` digest, and the `.lease` filename that
  acquisition creates.
- Comments state mechanism in the present tense. The one comment that named a
  consumer path (`llm-runner`) in a test message now describes the case
  itself.
- The four `lib.rs` files differ from the source by the review fixes and their
  tests, so line citations into them are re-derived by diff against the
  current tree rather than guessed. Relative to the source, `lease` has seven
  inserted lines inside the `invalid_epoch_states_fail_closed` table plus four
  appended tests (`lease_path_vectors_are_version_stable`,
  `epoch_read_is_bounded_regardless_of_file_size`,
  `concurrent_exclusive_acquisitions_admit_exactly_one_holder`,
  `separator_in_a_key_field_fails_closed_instead_of_aliasing`); `storage` has
  nine appended tests; `storage-types` has two; `cache-stability` has three.

Adaptations beyond renames, each recorded in the receipt as `adapted`:

| File | Change |
| --- | --- |
| `crates/cache-stability/src/lib.rs` | module doc; one test message; nested `if let` in `step_soft` collapsed into a let chain (same condition) |
| `crates/lease/src/lib.rs` | first doc line; `fnv1a` doc; six malformed-epoch cases; `LeaseKey::identity` rejects the `U+001F` separator; four appended tests |
| `crates/storage-types/src/lib.rs` | module doc; `StorageBackend::Postgres` doc; `StorageDescriptor` doc; `Debug` redacts the DSN; `sqlite_store_path` rejects path components in `module_id`; two appended tests |
| `crates/storage/src/lib.rs` | module doc first line; test temp-dir prefix; re-export of `GuardedConn` and `MaintenanceConn`; review fixes; nine appended tests |
| `crates/*/Cargo.toml` | crate names, `publish` and `rust-version` inherited, workspace dependency pins, descriptions without product branding |
| `crates/storage-types/examples/golden-vectors.rs`, `tests/golden_vectors.rs` | doc comments describe the byte-stable fixture |

## Byte-stable fixtures

`crates/cache-stability/tests/golden/cache-stability-golden-vectors.json` and
`crates/storage-types/tests/golden/storage_vectors.json` are `verbatim`:
receipt verification compares the destination bytes to `git cat-file` of the
source blob. The registry lists both as `byte-stable` fixtures, so the identity
scan skips them and the receipt checker refuses any transformation other than
`verbatim`. Their provenance strings name the predecessor repositories; that is
fixture content under R18.

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
  line ranges were re-derived from a line-level diff of each ported file, and
  PostgreSQL citations point into `commons@89abb40` as archived provenance.
  Three carry a `U2 audit` section (the `core` records).
- Support documents (`system-model.md`, `existing-checks.md`, `fault-map.md`,
  `relationships.md`, `portfolio-evaluation.md`,
  `durable-consumer-inventory.md`) are `adapted` with the same path rewrite, a
  provenance note, and, where U2 added records, a section for them.
  `existing-checks.md` gains rows for the four new or extended tests and marks
  the PostgreSQL rows as archived.
- `lease-store-density.md` is `adapted` from `docs/lease-store-density.md`: crate
  name and an appended provenance section only, so its cited line numbers
  (1-60) still hold.
- Seventeen new evidence files are `new-authored`; each records the discovery
  pass, primary and existing evidence with line citations, the failure
  scenario, and an audit verdict.

## Authored control records

| File | Purpose | Negative tests |
| --- | --- | --- |
| `migration/waves/U2/property-impact.json` | closure over the nine touched code files: 5 `core`, 41 `carried-forward` | `check.test.ts` property-impact suite (AE13, AE14, AE17, AE25) |
| `migration/waves/U2/architecture-impact.json` | pre-port and post-integration reports, four candidates, none Strong | `check.test.ts` architecture-impact suite (AE19-AE21) |
| `migration/waves/U2/waivers.json` | empty | `check.test.ts` waivers suite (AE30) |
| `migration/waves/U2/source-crate-dispositions.json` | five unmigrated crates and the source CI, publication, fuzz, probe, and catalog artifacts, with measured registry state | none; reviewed against `GET crates.io/api/v1/crates/<name>` responses whose digests it records |
| `Cargo.toml`, `Cargo.lock`, `crates/*/Cargo.toml` | workspace members, resolver 3, inherited `rust-version = "1.98"` and `publish = false`, dependency pins | `cargo metadata` graph check in CI; `--locked` builds on 1.98 and stable |
| `.github/workflows/ci.yml` | real workspace gates on the pinned 1.98 toolchain and stable, feature configurations, the pinned toolchain against unlocked dependencies, `cargo metadata` sibling and stub check | run locally step by step before commit |
| `scripts/eidnara-migration/check.ts`, `check.test.ts` | renamed-identity-first scan with underscore spelling and range containment; `fixture` registry kind | two new tests; 68 tests pass |
| `migration/registry.json` | frozen DDL names, database-name prefix, quoted production lease-key literals, fixtures, `store.db` template literal, U2 authored paths | `eidnara:check registry` |
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
`not_run`; `source-crate-dispositions.json` explains each.

## METHOD template connector

`docs/properties/METHOD.md` arrived verbatim from magic-context in U1. Its
prose rule forbids em dashes as connectors, but its own record template used
`—` between an enumerated field head and its note, and the U1 test fixture for
the index generator did the same. U2 changes the template and the fixture to a
spaced hyphen (`yes - <note>`), which the generator already accepts, and states
that rule next to the prose rule. No parsing behavior changes; the em dash is
still accepted for older content. The U1 receipt entry for `METHOD.md` is now
`adapted` and points here.
