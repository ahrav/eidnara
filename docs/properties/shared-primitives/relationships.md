# Relationship map

Provenance: `primitives@89abb40`, extended at U2 with the records discovered for
`cache-stability`, `storage-types`, and the non-lease behavior of `storage`.
The PostgreSQL backend named below is the one in the source
(`primitives@89abb40`) and is not carried; its mechanisms stay listed so the
SQLite-side records keep their context.

## Shared mechanisms

### OS-lock mechanism

- `at-most-one-exclusive-holder-per-key`
- `shared-exclusive-exclusion-matrix`
- `dead-holder-lease-is-reclaimable`
- `contention-is-classified-as-held`
- `filesystem-lock-scope-matches-deployment`
- `lease-inode-remains-stable-while-held`
- `handle-drop-releases-lease`
- `failed-acquisition-does-not-mutate-lease-state`

`filesystem-lock-scope-matches-deployment` and `lease-inode-remains-stable-while-held` are prerequisites for interpreting an acquired OS lock as ownership of the logical key.

### Epoch-file mechanism

- `writer-epoch-strictly-increases`
- `returned-epoch-is-crash-durable`
- `invalid-epoch-fails-closed`
- `failed-acquire-preserves-prior-epoch`
- `epoch-input-size-is-bounded`
- `shared-acquisition-is-epoch-neutral`

Suspected dominance: if `writer-epoch-strictly-increases` holds over all acknowledged acquisitions, several externally visible consequences of crash loss, malformed input, failed writes, and saturation are excluded. It does not prove bounded allocation or fail-closed diagnostics, so those records remain independent.

### Key/path mechanism

- `distinct-lease-keys-do-not-alias`
- `lease-path-format-is-version-stable`
- `lease-inode-remains-stable-while-held`
- `logical-store-has-single-lease-identity`

Distinct-key injectivity prevents false sharing. Cross-version stability and inode stability prevent one key from splitting into multiple lock domains. Neither dominates the other.

### Fence-consumer mechanism

- `shared-epoch-never-authorizes-write`
- `stale-writer-write-is-rejected`
- `writer-epoch-strictly-increases`
- `replacement-fence-is-claimed-before-old-writer-writes`
- `protected-write-set-is-fence-complete`

Suspected dominance: stale-write rejection relies on both trustworthy monotonic epochs and correct provenance at the write boundary. A correct consumer cannot repair duplicated tokens; correct token issuance cannot stop an unfenced write API.

### Permission mechanism

- `unix-lease-file-is-owner-only`
- `permission-hardening-never-follows-replacement`
- `lease-file-creation-is-never-permissive`
- `acquisition-does-not-follow-symlink`

The steady-state mode property does not dominate path-target stability. A test can observe `0600` on the wrong inode.

### Lease-root topology (unresolved shared dependency)

`distinct-lease-keys-do-not-alias`, `lease-file-growth-trigger-is-observed`, `filesystem-lock-scope-matches-deployment`, and `logical-store-has-single-lease-identity` depend on whether a consumer places many logical stores under one root. The lease crate describes a shared root (`crates/lease/src/lib.rs:11-15`); the in-repo SQLite consumer derives one root per database parent in `open_sqlite` (`crates/storage/src/lib.rs:606-619`, ending at `FileLeaseStore::new(&parent)`); the density measurement implies at least one external consumer uses a high-cardinality shared root (`lease-store-density.md:7-11`). Impact remains conditional until deployment topology is supplied.

### Resource mechanism

- `epoch-input-size-is-bounded`
- `lease-file-growth-trigger-is-observed`
- `lease-inode-remains-stable-while-held`

The never-unlink rule protects exclusion but causes accumulation. Cleanup pressure therefore increases the chance of introducing the exact replacement fault that inode stability forbids.

The parked dual-store migration at `lease-store-density.md:53-60` is out of scope because it is not implemented. Its stated dual-durability premise depends on `returned-epoch-is-crash-durable`, which current code contradicts.

## Coverage relationships

- `cross-process-exclusive-race-is-reached` prevents vacuous evidence for `at-most-one-exclusive-holder-per-key`.
- `epoch-update-interruption-window-is-reached` prevents vacuous evidence for process-interruption behavior. Returned-error state preservation requires an injected I/O error instead.
- `live-lease-file-replacement-is-reached` prevents vacuous evidence for inode stability.

Coverage checks assert enabling states only. None requires the prohibited outcome, such as two successful exclusive holders or a regressed epoch.

## Mechanisms added at U2

### Cache-stability state machine

- `defer-pass-replays-frozen-bytes-verbatim`
- `hard-bust-drains-deferred-work`
- `never-minted-boundary-is-not-reconcile-pending`
- `anchor-holds-while-reconcile-pending`
- `frozen-unit-order-is-preserved`
- `episode-units-reset-at-run-boundary`
- `cache-stability-golden-vectors-are-byte-stable`

The golden-vector record depends on the six mechanism records: a vector reproduces only if every arm behaves as recorded. The mechanism records do not depend on the fixture.

### Descriptor derivation

- `storage-descriptor-golden-vectors-are-byte-stable`
- `descriptor-wire-shape-round-trips`
- `logical-store-has-single-lease-identity` (the lease key is derived from descriptor fields)

### SQLite fence and callback scope

- `fence-tables-keep-their-durable-identity`
- `migrations-apply-once-per-namespace`
- `read-callbacks-cannot-write`
- `callback-scope-is-restored-after-unwind`
- `protected-transactions-pin-fence-durability`
- `store-files-are-owner-only-after-open`
- `fenced-write-is-atomic`
- `fence-epoch-outside-sqlite-range-fails-closed`

`fence-tables-keep-their-durable-identity` is a prerequisite for `stale-writer-write-is-rejected`, `replacement-fence-is-claimed-before-old-writer-writes`, and `protected-write-set-is-fence-complete`: every fence read and claim names those two tables. `read-callbacks-cannot-write` and `callback-scope-is-restored-after-unwind` are the SQLite half of `protected-write-set-is-fence-complete`.

