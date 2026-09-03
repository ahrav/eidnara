# Portfolio evaluation

Provenance: `primitives@89abb40`. This is the source portfolio evaluation for
the lease and fence records, carried forward verbatim in substance. The fresh
evaluation over this wave's `core` subset is in
`migration/waves/U2/property-impact.json` and the per-record evidence files.
PostgreSQL statements describe the PostgreSQL backend in the source
(`primitives@89abb40`), which is not carried.

Fresh-context evaluation ran after the initial 22-record catalog was written. It compared harness fit, coverage balance, implementability, and wildcard framing against source, then was restamped against `fa975843afd4b3122288149968ea5d6ff46322b3`.

## Gaps and disposition

| Gap | Disposition |
|---|---|
| Lease root and key were not bound to one logical store. | Added `logical-store-has-single-lease-identity`. |
| Effects performed before lock success were absent. | Added `failed-acquisition-does-not-mutate-lease-state`. |
| Drop-time release was claimed in the model but had no record. | Added `handle-drop-releases-lease`. |
| SQLite had a post-acquire, pre-fence-claim stale-write window. | Added `replacement-fence-is-claimed-before-old-writer-writes`. Open now claims a strictly greater epoch before exposure, so a stale floor cannot reissue a stored epoch; the stronger acquisition-instant property remains unresolved. |
| Unfenced write APIs made the protected write set unclear. | Added `protected-write-set-is-fence-complete` and narrowed `stale-writer-write-is-rejected`. PostgreSQL separates read-only, fenced, and explicitly unfenced maintenance callbacks; moving the SQLite consumers' unfenced mutations onto the fenced APIs remains open. |
| Steady-state mode, creation-window exposure, and symlink following were conflated. | Added `lease-file-creation-is-never-permissive` and `acquisition-does-not-follow-symlink`; narrowed the original permission records. |
| Parked dual-store migration rule was inconsistently in scope. | Explicitly excluded the unbuilt migration and linked its durability prerequisite in `relationships.md`. |

## Refinements applied

- Updated provenance after the repository advanced during analysis.
- Scoped exclusion to cooperative participants sharing one physical root and key.
- Made live-holder observation independent of the lease path under test.
- Added positive shared-coexistence coverage.
- Split returned I/O errors from non-returning crash histories.
- Made crash durability provisional on the documented but unstated crash model.
- Removed valid decimal truncation from malformed-input semantics.
- Reframed inode replacement as a fault that must not enable a competing acquisition; path divergence itself is now a coverage state.
- Replaced impossible shared-handle runtime mode checks with a source-level provenance check.
- Split positive and negative contention classifiers by their independent oracles.
- Replaced the catalog's provisional 32-byte allowance with the implementation's 20-byte decimal maximum and 21-byte bounded probe.
- Made watcher evidence require heartbeat, threshold crossing, bounded delivery, and owner acknowledgement.
- Replaced circular cross-version comparison with golden vectors.
- Corrected stale-write implication direction and separated fence completeness.
- Strengthened reachability predicates to include the exact process and timing state.
- Added exercised-state status to every fault-map row.
- Routed kernel/filesystem and crash properties to real process, deployment, or crash-consistency evidence rather than modeling away the mechanism under test.
- Replaced copied PostgreSQL lease identity/hash logic with the shared public `LeaseKey::identity` and `fnv1a` derivation, and recorded the stability-vector tests introduced across `bed0bb7` and `8da6d42` and the numeric hash API finalized in `f2107e5`.
- Recorded the move to standard-library file locking and the Rust 1.89 MSRV declaration from `bed0bb7`, plus the 0.1.1 version bump from `94c65ec`. The destination workspace pins `rust-version = "1.98"` and `rust-toolchain.toml` to 1.98; CI runs format, clippy with `-D warnings`, tests, doctests and docs, and feature configurations on that pinned toolchain, runs clippy and tests on moving stable, and checks the pinned toolchain against a regenerated lockfile of latest dependencies.

## Biases requiring human judgment

### Shared-root topology

The lease crate requires a shared root (`crates/lease/src/lib.rs:11-15`), the in-repo SQLite consumer derives a root from each database parent in `open_sqlite` (`crates/storage/src/lib.rs:722-747`, ending at `FileLeaseStore::new(&parent)`), and the density measurement implies an external high-cardinality shared root (`lease-store-density.md:7-11`). This unresolved topology changes the impact of key aliasing, density, and filesystem-scope properties.

### Contract catalog versus current implementation

The catalog intentionally includes desired contracts that current code contradicts. Test handoff must not treat every record as expected-green.

| Disposition | Properties |
|---|---|
| **Known violated by code under the recorded enabling state** | `at-most-one-exclusive-holder-per-key` under live path replacement, `lease-inode-remains-stable-while-held` under replacement, `logical-store-has-single-lease-identity` for one database opened under descriptors that differ in module or namespace, `failed-acquisition-does-not-mutate-lease-state`, `replacement-fence-is-claimed-before-old-writer-writes`. |
| **Believed satisfied on currently exercised local paths** | `shared-exclusive-exclusion-matrix`, `shared-acquisition-is-epoch-neutral`, `contention-is-classified-as-held`, `handle-drop-releases-lease`, `invalid-epoch-fails-closed`, `epoch-input-size-is-bounded`, `distinct-lease-keys-do-not-alias` for separator-bearing fields (FNV collisions remain unhandled), `lease-file-creation-is-never-permissive` on Unix, `acquisition-does-not-follow-symlink` on Unix, descriptor-relative lease permission hardening, and `stale-writer-write-is-rejected` on the SQLite and PostgreSQL synthetic fenced paths. |
| **Unknown or deployment-dependent** | `failed-acquire-preserves-prior-epoch` under real partial `File` errors, `writer-epoch-strictly-increases` under arbitrary restore or machine power loss outside SQLite's resource-floor recovery, `returned-epoch-is-crash-durable`, `dead-holder-lease-is-reclaimable`, `shared-epoch-never-authorizes-write`, `filesystem-lock-scope-matches-deployment`, `lease-file-growth-trigger-is-observed`, `lease-path-format-is-version-stable`, `protected-write-set-is-fence-complete`, Windows reparse-point runtime behavior, and other non-Unix symlink behavior. |
| **Campaign coverage requirements, not implementation verdicts** | `cross-process-exclusive-race-is-reached`, `epoch-update-interruption-window-is-reached`, `live-lease-file-replacement-is-reached`. |

Known-violated records require an implementation decision before an expected-green test can land. Unknown records require the named deployment or authority evidence.

### External evidence boundary

The following cannot be settled from this repository:

- External shared-mode consumers.
- Per-consumer lease-root paths, mount options, and host access topology.
- External blockers and draft status are tracked in the [durable consumer inventory](durable-consumer-inventory.md).
- Deployment-owner watcher health and owner acknowledgement.
- The intended machine-crash model.
- The authoritative set of fence-protected write sites.

## Harness-fit synthesis

- Real OS lock semantics require real processes and the target filesystem; a simulated lock model would test itself.
- Power-loss, storage-tear, and returned partial-`File` errors route to crash-consistency work. Pure byte-prefix tests cover only representation invariants.
- Deployment mount and watcher properties route to production-readiness evidence.
- Pure encoding, parsing, and boundary properties remain candidates for ordinary test-strategy selection.
- Existing tests remain unaudited regardless of whether they pass.

## Implementability synthesis

- Public `LeaseKey::identity`, `fnv1a`, and `fnv1a_hex` support external derivation checks; only `FileLeaseStore::lease_path` requires an in-crate check or a manually reconstructed `.lease` path convention.
- Cross-process holder counts need an external witness ledger keyed by the logical tuple and root, never by derived path alone.
- Inode and permission races need deterministic pause points or real process coordination.
- Shared-handle write provenance cannot be checked dynamically until the API exposes mode; source-level consumer inspection is the available check.
- Fault-map `no` rows are the current vacuity backlog.

## Wildcard synthesis

Primary portfolio risk is not property count. It is the composition of three mechanisms: path/root identity, OS lock domain, and epoch enforcement at every protected write. A green result for any one layer does not establish end-to-end single-writer safety.
