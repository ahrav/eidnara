# Existing-check inventory

Statuses are **unaudited** unless a row says **audited (U2)**. Test adequacy belongs to `/testing:invariant-test-review`; production guard placement and strength belong to `/low-level-systems:defensive-assertions-and-invariant-guards`.

## Production checks and guards

No production `assert!`, `debug_assert!`, `panic!`, or equivalent invariant battery exists in `crates/lease/src/lib.rs:1-435`.

| Location | Check or branch | Semantics/message | Linked claims |
|---|---|---|---|
| `protect_file` (`crates/lease/src/lib.rs:25-54`) | Public path hardening | Unix `symlink_metadata` rejects non-regular paths; missing paths return `Ok`; `set_permissions` is path-based and does not open caller-owned files. | Static symlinks are not followed on Unix; trusted parent-directory ownership is a caller precondition. |
| `protect_open_file` (`crates/lease/src/lib.rs:56-77`) | Descriptor-relative lease-file checks | Non-regular descriptors return `InvalidInput`; Unix regular files are set to `0600`; Windows reparse points are rejected. | Lease acquisition validates and hardens its owned descriptor. |
| `lease_open_options`, `open_lease_file` (`crates/lease/src/lib.rs:89-104,106-143`) | Lease-file publication/open | Opens an existing final path first. On `NotFound`, it initializes a same-directory temporary inode to epoch zero and publishes with `persist_noclobber`; an `AlreadyExists` race reopens the winner within three attempts. | No empty final pathname is published; links, FIFOs, and reparse points fail closed. |
| `FileLeaseStore::acquire`, `FileLeaseStore::acquire_above` (`crates/lease/src/lib.rs:235-249`) | Exclusive acquisition | Uses `try_lock`, classifies contention, and issues an epoch above the persisted value and a caller-supplied floor. Ordinary acquisition rejects empty state; floor-based acquisition treats empty state as the supplied durable-resource floor. | Exclusive liveness gate, error taxonomy, and explicit durable-resource recovery. |
| `FileLeaseStore::acquire_shared` (`crates/lease/src/lib.rs:279-311`) | Shared acquisition | Uses `try_lock_shared` and reads without mutation. | Concurrent shared-first acquisition and exclusion matrix. |
| `HeldFileLease::drop` (`crates/lease/src/lib.rs:336-338`) | Guard `Drop` | Best-effort `File::unlock`; error discarded; descriptor then closes. | Drop releases lease. |
| `read_epoch` (`crates/lease/src/lib.rs:378-407`) | Bounded epoch parse | Reads at most 21 bytes into a bounded vector; existing empty state and anything except 1-20 ASCII digits in `u64` range are rejected. | Malformed, empty, oversized, and overflowing state fails closed outside floor-based acquisition. |
| `bump_epoch_above`, `persist_epoch` (`crates/lease/src/lib.rs:424-435`) | Epoch update | Checked increment above both persisted state and floor; no truncate; fixed-width decimal overwrite with invalid-marker conversion for empty and 1-19 byte variable-width states. | Exhaustion errors; ordered prefix writes cannot leave a lower parseable value in the injected model. |
| `LeaseKey::identity`, `FileLeaseStore::lease_path`, `fnv1a`, `fnv1a_hex` | Identity/path derivation | Public separator-joined identity and FNV functions feed the private `.lease` path helper. | Stable namespaced identity. |
| `HeldFileLease`, `FileLeaseStore` | Concrete public types | The guard owns the locked file; both types are `Send + Sync`. | Lock lifetime and cross-thread use compile without trait erasure. |
| `ExpectedIdentity::for_baseline`, `classify`, `apply` (`crates/storage/src/lib.rs:761-778,780-830,832-846`), called from `open_sqlite` (`crates/storage/src/lib.rs:565-661`) | Baseline identity gate | `for_baseline` applies `crates/storage/baseline.sql` plus the consumer text to an in-memory database and records the resulting `sqlite_schema` inventory (`schema_inventory`, `crates/storage/src/lib.rs:727-744`) and the text's SHA-256; an unparseable text returns `StoreError::Baseline` before the file exists. `classify` reads `application_id`, `user_version`, the inventory, and the `format_marker` row and returns `Pristine`, `Baseline`, or `StoreError::Baseline` without writing, before any pragma or transaction. `apply` runs once for a pristine file inside the open transaction and writes the whole baseline, both pragmas, and one marker row. | A file is either pristine or identical to the baseline identity; no file is upgraded, adopted, or repaired (`store-schema-identity-matches-the-baseline`). |

## In-crate claim-bearing tests (26)

| Test | Location | Claim and exact oracle | Platform | Status |
|---|---|---|---|---|
| `fresh_exclusive_initializes_to_one` | `crates/lease/src/lib.rs:477-503` | A fresh key returns epoch 1, writes exactly 20 decimal digits, and the published Unix file is `0600`; concrete store and guard types compile as `Send + Sync`. | All | unaudited |
| `exclusive_epoch_exceeds_resource_floor` | `crates/lease/src/lib.rs:505-524` | Persisted epoch 41 with floor 100 issues 101; ordinary reacquisition then issues 102; an empty sidecar with floor 41 issues 42. | All | unaudited |
| `shared_first_initializes_canonical_zero` | `crates/lease/src/lib.rs:526-543` | Shared-first creation observes canonical zero, blocks exclusive, then permits writer epoch 1 after drop. | All | unaudited |
| `concurrent_shared_first_acquisitions_coexist` | `crates/lease/src/lib.rs:545-608` | Eight synchronized fresh-key shared acquisitions all coexist at epoch zero. Report collection and holder release are both deadline-bounded, so a holder that dies before reporting fails the check instead of hanging the suite. | All | unaudited |
| `variable_width_decimal_epoch_is_canonicalized` | `crates/lease/src/lib.rs:610-623` | Variable-width decimal 41 becomes epoch 42 in fixed-width form. | All | unaudited |
| `invalid_epoch_states_fail_closed` | `crates/lease/src/lib.rs:625-678` | Empty, malformed, oversized, and overflowing states return `LeaseError::Io(InvalidData)` through ordinary acquisition; nonempty invalid states also fail through floor-based acquisition and preserve bytes. | All | audited (U2) |
| `lease_path_vectors_are_version_stable` | `crates/lease/src/lib.rs:1193-1249` | Six identity/digest/path vectors for sample keys, with digests computed by an FNV-1a implementation outside the crate; acquisition creates exactly the pinned filename. | All | audited (U2) |
| `epoch_read_is_bounded_regardless_of_file_size` | `crates/lease/src/lib.rs:1251-1303` | A 1 MiB epoch is rejected with at most 21 bytes read through a counting in-memory reader, and through all three acquisition paths against a real file. | All | audited (U2) |
| `concurrent_exclusive_acquisitions_admit_exactly_one_holder` | `crates/lease/src/lib.rs:1305-1364` | Eight threads open independent descriptors and race exclusive acquisition behind a barrier; exactly one holds epoch 1, the rest are `Held`, and the next acquisition after release is epoch 2. Same process; the cross-process race remains unexercised. | All | audited (U2) |
| `separator_in_a_key_field_fails_closed_instead_of_aliasing` | `crates/lease/src/lib.rs:1365-1388` | `LeaseKey::identity` panics when `module_id`, `backend`, or `scope_key` contains `U+001F`, and the message names the field and the separator. | All | unaudited |
| `epoch_errors_keep_the_underlying_os_error` | `crates/lease/src/lib.rs:680-706` | Epoch error context preserves the original `io::Error` and raw OS error through the source chain. | All | unaudited |
| `maximum_epoch_is_readable_but_exhausted` | `crates/lease/src/lib.rs:708-732` | Shared acquisition reads `u64::MAX`; exclusive acquisition reports exhaustion and preserves bytes. | All | unaudited |
| `interrupted_persist_never_leaves_a_lower_parseable_epoch` | `crates/lease/src/lib.rs:734-853` | Injected ordered prefix-write failures exercise production `persist_epoch` and `read_epoch` for empty, variable-width, and canonical-width prior states, including a carry; any parseable aftermath is not lower, completion is fixed-width, and the count of parseable aftermaths is asserted per case. | All, in-memory `Read + Write + Seek` seam | unaudited |
| `acquisition_refuses_symlink_and_leaves_target_untouched` | `crates/lease/src/lib.rs:855-882` | Exclusive and shared acquisition fail; target content and mode remain unchanged. | Unix | unaudited |
| `acquisition_refuses_fifo_without_blocking` | `crates/lease/src/lib.rs:884-904` | Both modes reject a Unix FIFO opened with `O_NONBLOCK`. | Unix | unaudited |
| `an_acquired_lease_file_is_owner_only` | `crates/lease/src/lib.rs:906-934` | `mode == 0600`; message: lease stayed group/world writable. | Unix | unaudited |
| `protect_file_refuses_a_symlink_and_leaves_its_target_untouched` | `crates/lease/src/lib.rs:936-971` | `protect_file` returns `InvalidInput` and target remains `0644`. | Unix | unaudited |
| `protect_file_ignores_a_missing_path` | `crates/lease/src/lib.rs:973-982` | Missing path returns `Ok`. | All; trivial on non-Unix | unaudited |
| `identity_hash_derivation_is_stable` | `crates/lease/src/lib.rs:984-990` | Pins one public identity string and exact filename digest. | All | unaudited |
| `acquire_then_second_holder_is_rejected` | `crates/lease/src/lib.rs:992-1007` | Second live exclusive is `Held`; re-acquired epoch is greater. | All | unaudited |
| `distinct_identity_axes_do_not_conflict` | `crates/lease/src/lib.rs:1009-1029` | Distinct scopes, modules, and backends acquire independently at epoch 1. | All | unaudited |
| `shared_holders_coexist_but_block_exclusive` | `crates/lease/src/lib.rs:1031-1062` | Two shared holders coexist; exclusive remains `Held` until last drop. | All | unaudited |
| `exclusive_holder_blocks_shared` | `crates/lease/src/lib.rs:1064-1079` | Shared is `Held` under exclusive, then succeeds after drop. | All | unaudited |
| `shared_acquisition_does_not_bump_the_write_epoch` | `crates/lease/src/lib.rs:1081-1105` | Writer 1, shared 1/1, writer 2. | All | unaudited |
| `shared_lease_across_processes_blocks_exclusive` | `crates/lease/src/lib.rs:1111-1177` | Python child holds shared lock; parent exclusive is `Held`, shared succeeds, exclusive succeeds after child exits. | Unix | unaudited |
| `epoch_persists_across_store_instances` | `crates/lease/src/lib.rs:1179-1191` | Fresh store instance observes epochs 1 then 2. | All | unaudited |

## Adjacent in-repo checks

These are outside the target crate but explicitly exercise or consume its contract.

The PostgreSQL backend in the source (`primitives@89abb40`) is not carried. Its rows stay as source provenance: the location is the source alias and commit only, because receipts verify source blobs by hash and docs cite no paths into the source tree.

| Test | Location | Claim | Status |
|---|---|---|---|
| `reopening_a_permissive_store_protects_the_database_and_its_wal` | `crates/storage/src/lib.rs:1070-1120` | Database, WAL, and SHM are `0600` on reopen. | unaudited |
| `open_claims_fence_before_return` | `crates/storage/src/lib.rs:1200-1215` | Open stamps the lease epoch before exposing the store. | unaudited |
| `open_claim_rejects_an_epoch_the_database_already_stores` | `crates/storage/src/lib.rs:1217-1252` | The open claim rejects an epoch equal to the stored fence; `claim_fence` still authorizes it. | unaudited |
| `fresh_file_matches_the_baseline_inventory` | `crates/storage/src/lib.rs:1254-1318` | A fresh file reopened raw carries `APPLICATION_ID`, `USER_VERSION`, exactly one `format_marker` row, and a `sqlite_schema` inventory equal, object for object, to the literal values in `fixtures/schema/storage-inventory-v1.json`; no object name contains `schema_version` or `migration`. | audited (U2) |
| `a_consumer_baseline_is_applied_once_and_verified_on_reopen` | `crates/storage/src/lib.rs:1320-1360` | The first open applies `KV_BASELINE` and issues epoch 1; a reopen under the same text keeps the committed row and issues epoch 2; a reopen under a different text returns `StoreError::Baseline` and leaves the file bytes identical. | audited (U2) |
| `a_baseline_that_does_not_apply_is_rejected_before_the_file_is_touched` | `crates/storage/src/lib.rs:1362-1377` | An unparseable baseline text returns `StoreError::Baseline` and the database file is never created. | audited (U2) |
| `a_file_with_foreign_objects_is_refused_without_mutation` | `crates/storage/src/lib.rs:1379-1405` | A file holding a foreign table returns `StoreError::Baseline`; its bytes are identical before and after, and no `-wal` or `-shm` sidecar appears. | audited (U2) |
| `database_epoch_survives_repeated_lease_sidecar_loss` | `crates/storage/src/lib.rs:1407-1430` | Two repeated sidecar losses each issue an epoch above the database fence. | unaudited |
| `second_live_writer_is_rejected` | `crates/storage/src/lib.rs:1432-1442` | Second same-process store open is rejected as a lease error. | unaudited |
| `distinct_databases_do_not_falsely_contend` | `crates/storage/src/lib.rs:1444-1453` | Distinct database paths coexist. | unaudited |
| `unfenced_connection_rejects_writes` | `crates/storage/src/lib.rs:1614-1637` | `with_conn` rejects a write with `SQLITE_READONLY`, leaves no row, and still permits a later fenced write. | unaudited |
| `open_pins_full_synchronous` | `crates/storage/src/lib.rs:1639-1648` | `PRAGMA synchronous` reads back `2` (`FULL`) on a freshly opened store. The test observes the pragma value only; whether a committed fence epoch survives machine power loss in WAL mode is unverified (see `writer-epoch-strictly-increases.md`). | unaudited |
| `a_panicking_read_does_not_strand_the_connection_read_only` | `crates/storage/src/lib.rs:1650-1670` | A panicking `with_conn` callback still clears `query_only`, so later fenced writes and maintenance remain authorized. | unaudited |
| `a_read_callback_cannot_lower_fence_durability` | `crates/storage/src/lib.rs:1672-1738` | A read callback is denied lowering `synchronous`, and a fenced write and a fenced schema change re-pin `FULL` and a WAL journal after the maintenance path changes them. | unaudited |
| `a_read_callback_cannot_clear_the_read_only_guard` | `crates/storage/src/lib.rs:1740-1788` | A read callback is denied every pragma write, in any letter case, and writes nothing. | unaudited |
| `a_callback_cannot_damage_the_fence_row_it_is_checked_against` | `crates/storage/src/lib.rs:1837-1929` | A fenced callback that lowers, deletes, or forges the `fence` or `format_marker` rows, drops or indexes those tables, attaches a trigger or a shadowing view to them, or renames a temporary table onto `fence` in any letter case is denied, and the row keeps its epoch. | unaudited |
| `a_fenced_callback_cannot_rewrite_the_format_marker` | `crates/storage/src/lib.rs:1931-1987` | A fenced `UPDATE` of `format_marker` is denied and a temporary table renamed onto `format_marker` is rejected before commit; the single marker row keeps its digest. | audited (U2) |
| `a_callback_cannot_end_the_fence_checked_transaction` | `crates/storage/src/lib.rs:1790-1835` | `COMMIT`, `ROLLBACK`, `SAVEPOINT`, and `BEGIN` are denied inside a fenced callback, and nothing commits or is created unfenced. | unaudited |
| `maintenance_runs_through_the_unfenced_path` | `crates/storage/src/lib.rs:1989-2002` | `VACUUM` fails the read-only guard and succeeds through `with_conn_unfenced`. | unaudited |
| `fenced_write_rolls_back_on_error` | `crates/storage/src/lib.rs:2004-2031` | Callback failure rolls back both domain mutation and a newer fence claim. | unaudited |
| `negative_database_fence_fails_closed` | `crates/storage/src/lib.rs:2033-2061` | A negative fence written through `ignore_check_constraints` is rejected with `FenceCorrupt` and remains unchanged. | unaudited |
| `superseded_writer_is_fenced_out_after_handover` | `crates/storage/src/lib.rs:2063-2119` | Synthetic epoch-1 writer cannot overwrite epoch-2 state, and its fenced DDL is rejected before it creates a table. | unaudited |
| `equal_epoch_writer_is_not_fenced` | `crates/storage/src/lib.rs:2121-2139` | Equal epoch can continue writing. | unaudited |
| `epoch_above_sqlite_integer_range_fails` | `crates/storage/src/lib.rs:2141-2157` | Epochs above SQLite's signed integer range fail instead of wrapping. | unaudited |
| `distinct_databases_in_one_directory_do_not_falsely_contend` | `crates/storage/src/lib.rs:1455-1466` | Two database files in one directory, with equal module and namespace, open concurrently; the lease key carries the file name. | audited (U2) |
| `symlinked_database_paths_contend_or_are_refused_never_aliased` | `crates/storage/src/lib.rs:958-1010` | A directory-symlink alias of a held database returns `Held`; a file-symlink alias is refused as a non-regular file, with and without a holder. | audited (U2) |
| `protection_failure_aborts_open_before_the_fence_write` | `crates/storage/src/lib.rs:1122-1151` | A sidecar path that `protect_file` rejects aborts `open_sqlite` before any fence byte is written. | audited (U2) |
| `new_database_file_is_owner_only_at_creation` | `crates/storage/src/lib.rs:1053-1068` | Under umask `022`, the database file the open path creates is `0600` with no `chmod`. | audited (U2) |
| `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask` | `crates/storage/src/lib.rs:1522-1550` | Under umask `022`, a fresh open leaves the database, `-wal`, and `-shm` files at `0600`; the sidecars inherit the database mode. | audited (U2) |
| `suppressed_fence_update_is_an_error_not_a_silent_success` | `crates/storage/src/lib.rs:1468-1493` | A `BEFORE UPDATE ... RAISE(IGNORE)` trigger makes the fence upsert affect zero rows; the claim fails instead of returning success. | audited (U2) |
| `undone_fence_update_is_an_error_not_a_silent_success` | `crates/storage/src/lib.rs:1495-1520` | An `AFTER UPDATE` trigger that restores the old epoch leaves the change count at one; the read-back rejects the claim. | audited (U2) |
| `store_error_source_preserves_the_underlying_errno` | `crates/storage/src/lib.rs:1552-1577` | `StoreError::source` reaches the `io::Error` and its errno through `Io` and through `Lease(LeaseError::Io)`. | audited (U2) |
| `dangling_symlink_at_the_database_path_is_refused_without_creating_the_target` | `crates/storage/src/lib.rs:1012-1033` | A dangling symlink at the database path is refused with `ELOOP` and its target is never created. | audited (U2) |
| `a_read_callback_cannot_checkpoint_the_wal` | `crates/storage/src/lib.rs:1035-1051` | `PRAGMA wal_checkpoint` is denied inside a read callback while a pragma read still succeeds. | audited (U2) |
| `open_migrate_and_single_writer` | `primitives@89abb40` | Live PostgreSQL covers migration and session exclusion. Requires a live PostgreSQL DSN from the environment; the source CI has a required live job. | unaudited |
| `read_only_callback_rejects_mutation_without_rows` | `primitives@89abb40` | Read-only mutation reports SQLSTATE `25006` and leaves rows unchanged. | unaudited |
| `open_verifies_the_stored_epoch_matches_the_issued_one` | `primitives@89abb40` | Open re-reads the committed lease row and requires it to carry the epoch it issued. | unaudited |
| `a_suppressed_epoch_increment_is_rejected` | `primitives@89abb40` | Issuing an epoch that did not advance past the stored row is rejected. | unaudited |
| `a_callback_cannot_damage_the_lease_row_it_is_fenced_against` | `primitives@89abb40` | A fenced callback that deletes or lowers its own lease row is rejected and rolled back. | unaudited |
| `a_callback_that_ends_the_transaction_is_rejected` | `primitives@89abb40` | A fenced callback or migration that sends `COMMIT` is rejected instead of reporting success. | unaudited |
| `a_read_callback_cannot_escape_read_only_mode` | `primitives@89abb40` | A read callback that sends `COMMIT` or `SET TRANSACTION READ WRITE` is rejected; only the autocommitted write survives. | unaudited |
| `a_regressed_positive_epoch_fails_closed` | `primitives@89abb40` | A stored epoch below the one stamped at open fails closed for the holder and for a superseded writer above it. | unaudited |
| `a_negative_epoch_fails_closed` | `primitives@89abb40` | The lease table rejects a negative epoch, and a negative epoch reaching the fence returns `FenceCorrupt`. | unaudited |
| `unfenced_callback_runs_statements_a_transaction_forbids` | `primitives@89abb40` | `VACUUM` reports SQLSTATE `25001` inside a fenced transaction and succeeds through the autocommit callback. | unaudited |
| `fenced_callback_error_rolls_back_rows` | `primitives@89abb40` | Callback failure rolls back domain rows. | unaudited |
| `repeated_fenced_writes_at_current_epoch_succeed` | `primitives@89abb40` | Repeated writes at the current lease epoch succeed. | unaudited |
| `superseded_writer_is_rejected_after_reopen` | `primitives@89abb40` | Synthetic stale callback is rejected after reopen. | unaudited |
| `superseded_writer_cannot_migrate` | `primitives@89abb40` | Synthetic stale migration is fenced before its schema SQL executes. | unaudited |
| `independent_namespace_chains` | `primitives@89abb40` | Independent migrations both apply. | unaudited |
| `advisory_key_derivation_is_stable` | `primitives@89abb40` | Pins one advisory bigint derived through public `LeaseKey::identity` and `fnv1a`. | unaudited |

The handover checks use synthetic stores that bypass real lease acquisition. They check fence logic against real database transactions, not an end-to-end retained-connection handover.

## Explicitly absent checks

- Process death without unwind.
- Power-loss durability or directory-entry durability.
- Real `File` I/O failure after a positive write prefix.
- Runtime Windows reparse-point and lock-conversion behavior; Windows is compile-checked only.
- Restored older valid epoch files.
- Live lease-file unlink or replacement.
- Cross-process exclusive-versus-exclusive contention (same-process concurrent contention is exercised).
- Adversarial key fields or hash collisions.
- Deployed network/overlay filesystem semantics.
- Shared-handle epoch use at consumer write sites.
- Cross-version lease-path overlap (the golden vectors themselves exist).
- Situation-coverage assertions (`sometimes`/`reachable` equivalents).
- Property, fuzz, model-checking, Miri, or failpoint harnesses.
