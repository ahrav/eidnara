# Fault-to-property map

Provenance: `primitives@89abb40`, extended at U2 with faults for the discovered
records.

Safety checks apply while faults are active. Liveness checks run after the stated bounded fault-free recovery window. Coverage records verify that vulnerable preconditions occurred.

| Fault or enabling state | Threatened properties | Required witness / non-vacuity condition | Occurs today |
|---|---|---|---|
| Two independent exclusive acquirers | `at-most-one-exclusive-holder-per-key`, `contention-is-classified-as-held` | `cross-process-exclusive-race-is-reached` fires. | no |
| Two shared holders, then one drops | `shared-exclusive-exclusion-matrix` | Exclusive attempted while exactly one of at least two shared holders remains. | yes, local unit test |
| Nonzero epoch with two simultaneous shared holders | `shared-acquisition-is-epoch-neutral` | Both shared holders coexist after a writer persisted a nonzero epoch. | no; current tests split these states |
| Holder killed without unwind | `dead-holder-lease-is-reclaimable` | Child exit is confirmed before recovery deadline starts. | no |
| Process interruption during epoch rewrite | `writer-epoch-strictly-increases` | `epoch-update-interruption-window-is-reached` fires. | no; injected ordered prefix writes do not interrupt a process |
| Power loss after acquisition acknowledgement | `returned-epoch-is-crash-durable`, `writer-epoch-strictly-increases` | Volatile cache is actually discarded; process kill alone is insufficient. | no |
| `ENOSPC`, `EDQUOT`, or returned `EIO` during rewrite | `failed-acquire-preserves-prior-epoch`, `writer-epoch-strictly-increases` | A positive write prefix is followed by an error and acquisition returns `Err`. | no; byte-prefix properties do not inject `File` errors |
| Valid-UTF-8 malformed or invalid-UTF-8 epoch | `invalid-epoch-fails-closed`, `writer-epoch-strictly-increases` | File is non-empty and existed before acquisition. | yes, local unit tests through both acquisition modes |
| Persisted epoch is `u64::MAX` | `writer-epoch-strictly-increases` | Parser observes the exact maximum, then two consecutive exclusive acquisitions return `Err`. | yes, local unit test |
| Older lease file restored | `writer-epoch-strictly-increases`, `returned-epoch-is-crash-durable` | A previously acknowledged higher epoch exists before restore, then the same key is acquired. | partial; SQLite sidecar deletion is recovered from the database floor, but arbitrary lease-only consumers and power-loss restore remain untested |
| Key contains `U+001F` | `distinct-lease-keys-do-not-alias` | `LeaseKey::identity` rejects the key before any path is derived, naming the offending field; no two keys join to one identity. | yes: `separator_in_a_key_field_fails_closed_instead_of_aliasing`, one field at a time |
| FNV-1a collision | `distinct-lease-keys-do-not-alias` | Two distinct identities produce one digest; practical adversarial cost remains open. | no |
| Lease file unlinked/replaced while held | `lease-inode-remains-stable-while-held`, `at-most-one-exclusive-holder-per-key` | `live-lease-file-replacement-is-reached` fires. | no |
| Shared handle routed to write fence | `shared-epoch-never-authorizes-write`, `stale-writer-write-is-rejected` | Consumer records handle origin and a durable write attempt. | no in-repo caller; external unknown |
| Pre-existing permissive file | `unix-lease-file-is-owner-only` | Check both shared and exclusive acquisition. | partial: exclusive only |
| Permissive create-time umask | `lease-file-creation-is-never-permissive` | `NamedTempFile` creates the private inode; an observer checks it from creation. | partial: tempfile contract and post-open check, no concurrent observer |
| Symlink or reparse point planted before open | `acquisition-does-not-follow-symlink` | Assert target existence, content, and mode remain unchanged through both acquisition methods. | yes on Unix for an existing target; Windows compile-only; dangling and other non-Unix cases absent |
| Path replaced after lease descriptor open | `permission-hardening-never-follows-replacement`, `unix-lease-file-is-owner-only` | Compare the opened/locked inode with the inode hardened through the same descriptor. | code uses descriptor-relative metadata and chmod; replacement race is not directly exercised |
| Known contention with permissive incumbent file | `failed-acquisition-does-not-mutate-lease-state` | Snapshot content and metadata before and after `Held`. | no |
| Unsupported or differently scoped advisory lock | `filesystem-lock-scope-matches-deployment`, `contention-is-classified-as-held` | Real target mount and multi-host/process topology are used. | no deployment evidence |
| Oversized lease file | `epoch-input-size-is-bounded` | Both acquisition modes reject a file over the 20-byte canonical maximum after reading at most 21 bytes. | yes, local unit test |
| Sustained ephemeral keys | `lease-file-growth-trigger-is-observed` | Watcher reports size and acknowledges a threshold signal. | partial: historical measurement only |
| Old and new binaries overlap | `lease-path-format-is-version-stable`, `at-most-one-exclusive-holder-per-key` | Both versions derive and attempt the same logical key concurrently. | no |
| Same database, differing descriptors | `logical-store-has-single-lease-identity` | One logical path derives two root/key identities. | no |
| Sibling databases, equal descriptors | `logical-store-has-single-lease-identity` | Two independent files derive one root/key identity. | no |
| Last handle drops while competitor waits | `handle-drop-releases-lease` | Acquisition completes within configured bound. | partial: reacquire occurs, but no waiting competitor |
| Replacement acquired, fence not yet claimed | `replacement-fence-is-claimed-before-old-writer-writes` | Old connection attempts fenced write before replacement's first claim. | no |
| Protected mutation uses an unfenced API | `protected-write-set-is-fence-complete` | Every protected write site is inventoried and observed. | no inventory |
| Stale connection after completed fence claim | `stale-writer-write-is-rejected` | Stale attempt returns fenced and leaves state unchanged. | partial: synthetic SQLite only |

A `no` means every safety check threatened by that fault can pass without the fault ever occurring. `partial` names the missing arm so the gap remains explicit.

## Faults added at U2

| Fault | Properties |
|---|---|
| A defer pass that re-renders or re-orders frozen units | `defer-pass-replays-frozen-bytes-verbatim`, `frozen-unit-order-is-preserved` |
| A hard bust that leaves queued units pending | `hard-bust-drains-deferred-work` |
| A fresh store with the empty boundary sentinel | `never-minted-boundary-is-not-reconcile-pending` |
| A coverage-extending `Soft` while a reconcile is pending | `anchor-holds-while-reconcile-pending` |
| A run boundary with mixed durability classes | `episode-units-reset-at-run-boundary` |
| A regenerated or edited golden fixture | `cache-stability-golden-vectors-are-byte-stable`, `storage-descriptor-golden-vectors-are-byte-stable` |
| A renamed serde field or tag | `descriptor-wire-shape-round-trips` |
| A renamed or re-typed fence or version table | `fence-tables-keep-their-durable-identity` |
| A crash between a migration batch and its version record | `migrations-apply-once-per-namespace` |
| A callback that writes, lowers a pragma, or panics | `read-callbacks-cannot-write`, `callback-scope-is-restored-after-unwind`, `protected-transactions-pin-fence-durability` |
| A permissive database, WAL, or SHM file at reopen | `store-files-are-owner-only-after-open` |
| A callback error after a fence claim | `fenced-write-is-atomic` |
| A negative stored epoch or a holder epoch above `i64::MAX` | `fence-epoch-outside-sqlite-range-fails-closed` |

