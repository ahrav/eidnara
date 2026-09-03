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
| A `Soft` whose anchor is absent with no prior defer | `anchor-holds-while-reconcile-pending`; the pass records the loss (`soft_with_an_absent_anchor_marks_reconcile_pending`) |
| A run boundary with mixed durability classes | `episode-units-reset-at-run-boundary` |
| A regenerated or edited golden fixture | `cache-stability-golden-vectors-are-byte-stable`, `storage-descriptor-golden-vectors-are-byte-stable` |
| A renamed serde field or tag | `descriptor-wire-shape-round-trips` |
| A file whose `application_id`, `user_version`, `sqlite_schema` inventory, or format-marker digest differs from the baseline, or a baseline text that does not apply | `store-schema-identity-matches-the-baseline` |
| A callback that writes, lowers a pragma, panics, or creates a temp object that shadows a baseline table | `read-callbacks-cannot-write`, `callback-scope-is-restored-after-unwind`, `protected-transactions-pin-fence-durability` |
| A permissive database, WAL, or SHM file at reopen | `store-files-are-owner-only-after-open` |
| A callback error after a fence claim | `fenced-write-is-atomic` |
| A negative stored epoch or a holder epoch above `i64::MAX` | `fence-epoch-outside-sqlite-range-fails-closed` |
| A baseline text that attaches a database, writes a pragma, opens a transaction, or writes a `fence` or `format_marker` row | `store-schema-identity-matches-the-baseline` |
| An initialized store whose `fence` row holds `i64::MAX` | `fence-epoch-outside-sqlite-range-fails-closed`; refused with `FenceExhausted` before the lease sidecar advances (`a_fence_at_the_integer_maximum_is_refused_before_the_lease_advances`) |
| An initialized store whose `fence` row is gone | `store-schema-identity-matches-the-baseline`, `stale-writer-write-is-rejected`; refused with `FenceMissing` (`an_initialized_store_without_a_fence_row_is_refused`) |
| A rollback-mode store whose writer died with spilled pages and a hot `-journal` | `store-schema-identity-matches-the-baseline`; the inspection copy rolls the journal back before classifying (`a_store_with_a_hot_rollback_journal_is_classified_after_rollback`) |
| A lease path hard-linked to another file | `unix-lease-file-is-owner-only`, `distinct-lease-keys-do-not-alias`; `protect_open_file` refuses a file with more than one name (`acquisition_refuses_a_hard_linked_lease_file_and_leaves_the_other_name_untouched`) |
| A lease directory writable by group or other, or owned by another user | `at-most-one-exclusive-holder-per-key`; refused before any lease file exists (`acquisition_refuses_a_group_or_world_writable_lease_directory`) |
| A lease directory whose ancestor another principal can rename in without the sticky bit | `at-most-one-exclusive-holder-per-key`; refused (`acquisition_refuses_a_lease_directory_under_a_writable_non_sticky_ancestor`) |
| A rename onto the lease path between an acquirer's open and its lock | `at-most-one-exclusive-holder-per-key`; the post-lock identity check refuses the lease (`a_lease_path_replaced_after_open_is_detected_before_the_lease_is_returned`) |
| A relative lease root or a relative SQLite path | `at-most-one-exclusive-holder-per-key`, `logical-store-has-single-lease-identity`; the lease root is resolved once at construction and a relative database path is refused (`a_relative_root_is_resolved_when_the_store_is_built`, `a_relative_sqlite_path_is_refused`) |
| A different regular file renamed onto the store path between inspection and open | `store-schema-identity-matches-the-baseline`; refused before the first statement, with the `open(2)`-to-`stat(2)` interval recorded as open (`file_identity_follows_the_inode_not_the_bytes`) |
| A superseded writer reaching the durability pin after maintenance changed the journal mode | `stale-writer-write-is-rejected`; the read-only precheck refuses it before the pragmas (`a_superseded_writer_does_not_change_the_journal_mode_before_being_fenced`) |
| A symlink on the lease path owned by another user, or a user-owned symlink in a directory others can write to | `at-most-one-exclusive-holder-per-key`; refused (`a_lease_path_through_a_foreign_owned_symlink_is_refused`) |
| A store whose last writer left frames in the WAL and no lease sidecar | `store-schema-identity-matches-the-baseline`; the inspection copy replays the WAL for the floor (`a_store_left_with_wal_frames_reopens_above_the_wal_epoch`) |
| A stored cache `version` of `u64::MAX` before a rebuild | none; `CoreState::step` returns `StepError::VersionExhausted` with the state unchanged (`an_exhausted_version_refuses_a_rebuild_and_leaves_the_state_unchanged`) |
| Two frozen units under one key in loaded cache state | none; `CoreState::step` returns `StepError::DuplicateFrozenKey` with the state unchanged (`duplicate_frozen_keys_in_loaded_state_are_refused_unchanged`) |
| A foreign database with committed WAL frames, or a foreign `fence` row, at the store path | `store-schema-identity-matches-the-baseline` |

