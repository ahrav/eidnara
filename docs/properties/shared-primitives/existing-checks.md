# Existing-check inventory

Statuses are **unaudited** unless a row says **audited (U2)**. Test adequacy belongs to `/testing:invariant-test-review`; production guard placement and strength belong to `/low-level-systems:defensive-assertions-and-invariant-guards`.

## Production checks and guards

No production `assert!`, `debug_assert!`, `panic!`, or equivalent invariant battery exists in `crates/lease/src/lib.rs:1-471`.

| Location | Check or branch | Semantics/message | Linked claims |
|---|---|---|---|
| `protect_file` (`crates/lease/src/lib.rs:35-54`) | Public path hardening | Unix `symlink_metadata` rejects non-regular paths; missing paths return `Ok`; `set_permissions` is path-based and does not open caller-owned files. | Static symlinks are not followed on Unix; trusted parent-directory ownership is a caller precondition. |
| `protect_open_file` (`crates/lease/src/lib.rs:56-77`) | Descriptor-relative lease-file checks | Non-regular descriptors return `InvalidInput`; Unix regular files are set to `0600`; Windows reparse points are rejected. | Lease acquisition validates and hardens its owned descriptor. |
| `lease_open_options`, `open_lease_file` (`crates/lease/src/lib.rs:89-104,106-143`) | Lease-file publication/open | Opens an existing final path first. On `NotFound`, it initializes a same-directory temporary inode to epoch zero and publishes with `persist_noclobber`; an `AlreadyExists` race reopens the winner within three attempts. | No empty final pathname is published; links, FIFOs, and reparse points fail closed. |
| `FileLeaseStore::acquire`, `FileLeaseStore::acquire_above` (`crates/lease/src/lib.rs:230-248`) | Exclusive acquisition | Uses `try_lock`, classifies contention, and issues an epoch above the persisted value and a caller-supplied floor. Ordinary acquisition rejects empty state; floor-based acquisition treats empty state as the supplied durable-resource floor. | Exclusive liveness gate, error taxonomy, and explicit durable-resource recovery. |
| `FileLeaseStore::acquire_shared` (`crates/lease/src/lib.rs:288-310`) | Shared acquisition | Uses `try_lock_shared` and reads without mutation. | Concurrent shared-first acquisition and exclusion matrix. |
| `HeldFileLease::drop` (`crates/lease/src/lib.rs:334-338`) | Guard `Drop` | Best-effort `File::unlock`; error discarded; descriptor then closes. | Drop releases lease. |
| `read_epoch` (`crates/lease/src/lib.rs:395-423`) | Bounded epoch parse | Reads at most 21 bytes into a bounded vector; existing empty state and anything except 1-20 ASCII digits in `u64` range are rejected. | Malformed, empty, oversized, and overflowing state fails closed outside floor-based acquisition. |
| `bump_epoch_above`, `persist_epoch` (`crates/lease/src/lib.rs:426-451`) | Epoch update | Checked increment above both persisted state and floor; no truncate; fixed-width decimal overwrite with invalid-marker conversion for empty and 1-19 byte legacy states. | Exhaustion errors; ordered prefix writes cannot leave a lower parseable value in the injected model. |
| `LeaseKey::identity`, `FileLeaseStore::lease_path`, `fnv1a`, `fnv1a_hex` | Identity/path derivation | Public separator-joined identity and FNV functions feed the private `.lease` path helper. | Stable namespaced identity. |
| `HeldFileLease`, `FileLeaseStore` | Concrete public types | The guard owns the locked file; both types are `Send + Sync`. | Lock lifetime and cross-thread use compile without trait erasure. |

## In-crate claim-bearing tests (22)

| Test | Location | Claim and exact oracle | Platform | Status |
|---|---|---|---|---|
| `fresh_exclusive_initializes_to_one` | `crates/lease/src/lib.rs:494-519` | A fresh key returns epoch 1, writes exactly 20 decimal digits, and the published Unix file is `0600`; concrete store and guard types compile as `Send + Sync`. | All | unaudited |
| `exclusive_epoch_exceeds_resource_floor` | `crates/lease/src/lib.rs:522-540` | Persisted epoch 41 with floor 100 issues 101; ordinary reacquisition then issues 102; an empty sidecar with floor 41 issues 42. | All | unaudited |
| `shared_first_initializes_canonical_zero` | `crates/lease/src/lib.rs:543-559` | Shared-first creation observes canonical zero, blocks exclusive, then permits writer epoch 1 after drop. | All | unaudited |
| `concurrent_shared_first_acquisitions_coexist` | `crates/lease/src/lib.rs:562-624` | Eight synchronized fresh-key shared acquisitions all coexist at epoch zero. Report collection and holder release are both deadline-bounded, so a holder that dies before reporting fails the check instead of hanging the suite. | All | unaudited |
| `legacy_decimal_epoch_is_canonicalized` | `crates/lease/src/lib.rs:627-639` | Variable-width decimal 41 becomes epoch 42 in fixed-width form. | All | unaudited |
| `invalid_epoch_states_fail_closed` | `crates/lease/src/lib.rs:642-694` | Empty, malformed, oversized, and overflowing states return `LeaseError::Io(InvalidData)` through ordinary acquisition; nonempty invalid states also fail through floor-based acquisition and preserve bytes. | All | audited (U2) |
| `lease_path_vectors_are_version_stable` | `crates/lease/src/lib.rs:1206-1260` | Eight externally computed identity/digest/path vectors, including both production keys; acquisition creates exactly the pinned filename. | audited (U2) |
| `epoch_read_is_bounded_regardless_of_file_size` | `crates/lease/src/lib.rs:1264-1314` | A 1 MiB epoch is rejected with at most 21 bytes read, through all three acquisition paths. | audited (U2) |
| `concurrent_exclusive_acquisitions_admit_exactly_one_holder` | `crates/lease/src/lib.rs:1316-1375` | Eight threads open independent descriptors and race exclusive acquisition behind a barrier; exactly one holds epoch 1, the rest are `Held`, and the next acquisition after release is epoch 2. Same process; the cross-process race remains unexercised. | audited (U2) |
| `epoch_errors_keep_the_underlying_os_error` | `crates/lease/src/lib.rs:697-722` | Epoch error context preserves the original `io::Error` and raw OS error through the source chain. | All | unaudited |
| `maximum_epoch_is_readable_but_exhausted` | `crates/lease/src/lib.rs:725-748` | Shared acquisition reads `u64::MAX`; exclusive acquisition reports exhaustion and preserves bytes. | All | unaudited |
| `interrupted_persist_never_leaves_a_lower_parseable_epoch` | `crates/lease/src/lib.rs:751-869` | Injected ordered prefix-write failures exercise production `persist_epoch` and `read_epoch` for empty, legacy-width, and canonical-width prior states, including a carry; any parseable aftermath is not lower, completion is fixed-width, and the count of parseable aftermaths is asserted per case. | All, in-memory `Read + Write + Seek` seam | unaudited |
| `acquisition_refuses_symlink_and_leaves_target_untouched` | `crates/lease/src/lib.rs:873-898` | Exclusive and shared acquisition fail; target content and mode remain unchanged. | Unix | unaudited |
| `acquisition_refuses_fifo_without_blocking` | `crates/lease/src/lib.rs:902-920` | Both modes reject a Unix FIFO opened with `O_NONBLOCK`. | Unix | unaudited |
| `an_acquired_lease_file_is_owner_only` | `crates/lease/src/lib.rs:926-950` | `mode == 0600`; message: lease stayed group/world writable. | Unix | unaudited |
| `protect_file_refuses_a_symlink_and_leaves_its_target_untouched` | `crates/lease/src/lib.rs:961-987` | `protect_file` returns `InvalidInput` and target remains `0644`. | Unix | unaudited |
| `protect_file_ignores_a_missing_path` | `crates/lease/src/lib.rs:994-998` | Missing path returns `Ok`. | All; trivial on non-Unix | unaudited |
| `identity_hash_derivation_is_stable` | `crates/lease/src/lib.rs:1003-1006` | Pins one public identity string and exact filename digest. | All | unaudited |
| `acquire_then_second_holder_is_rejected` | `crates/lease/src/lib.rs:1009-1023` | Second live exclusive is `Held`; re-acquired epoch is greater. | All | unaudited |
| `distinct_identity_axes_do_not_conflict` | `crates/lease/src/lib.rs:1026-1045` | Distinct scopes, modules, and backends acquire independently at epoch 1. | All | unaudited |
| `shared_holders_coexist_but_block_exclusive` | `crates/lease/src/lib.rs:1048-1078` | Two shared holders coexist; exclusive remains `Held` until last drop. | All | unaudited |
| `exclusive_holder_blocks_shared` | `crates/lease/src/lib.rs:1081-1095` | Shared is `Held` under exclusive, then succeeds after drop. | All | unaudited |
| `shared_acquisition_does_not_bump_the_write_epoch` | `crates/lease/src/lib.rs:1098-1121` | Writer 1, shared 1/1, writer 2. | All | unaudited |
| `shared_lease_across_processes_blocks_exclusive` | `crates/lease/src/lib.rs:1129-1187` | Python child holds shared lock; parent exclusive is `Held`, shared succeeds, exclusive succeeds after child exits. | Unix | unaudited |
| `epoch_persists_across_store_instances` | `crates/lease/src/lib.rs:1190-1201` | Fresh store instance observes epochs 1 then 2. | All | unaudited |

## Adjacent in-repo checks

These are outside the target crate but explicitly exercise or consume its contract.

The PostgreSQL backend is not migrated. Its rows stay as archived provenance; the paths point into `commons@89abb40`, not into this tree.

| Test | Location | Claim | Status |
|---|---|---|---|
| `reopening_a_permissive_store_protects_the_database_and_its_wal` | `crates/storage/src/lib.rs:829-883` | Database and WAL are `0600` on reopen. | unaudited |
| `open_claims_fence_before_return` | `crates/storage/src/lib.rs:922-934` | Open stamps the lease epoch before exposing the store. | unaudited |
| `open_claim_rejects_an_epoch_the_database_already_stores` | `crates/storage/src/lib.rs:941-977` | The open claim rejects an epoch equal to the stored fence; `claim_fence` still authorizes it. | unaudited |
| `migrations_seed_once_across_reopen` | `crates/storage/src/lib.rs:980-1001` | Migrations and seeds run once; clean reopen issues a greater epoch. | unaudited |
| `database_epoch_survives_repeated_lease_sidecar_loss` | `crates/storage/src/lib.rs:1004-1030` | Two repeated sidecar losses each issue an epoch above the database fence. | unaudited |
| `second_live_writer_is_rejected` | `crates/storage/src/lib.rs:1045-1054` | Second same-process store open is rejected as a lease error. | unaudited |
| `distinct_databases_do_not_falsely_contend` | `crates/storage/src/lib.rs:1057-1065` | Distinct database paths coexist. | unaudited |
| `unfenced_connection_rejects_writes` | `crates/storage/src/lib.rs:1172-1195` | `with_conn` rejects a write with `SQLITE_READONLY`, leaves no row, and still permits a later fenced write. | unaudited |
| `open_pins_full_synchronous` | `crates/storage/src/lib.rs:1198-1206` | Open pins `synchronous=FULL`, so a committed fence epoch survives power loss in WAL mode. | unaudited |
| `a_panicking_read_does_not_strand_the_connection_read_only` | `crates/storage/src/lib.rs:1209-1229` | A panicking `with_conn` callback still clears `query_only`, so later fenced writes and maintenance remain authorized. | unaudited |
| `a_read_callback_cannot_lower_fence_durability` | `crates/storage/src/lib.rs:1232-1296` | A read callback is denied lowering `synchronous`, and a fenced write and a migration re-pin `FULL` and a WAL journal after the maintenance path changes them. | unaudited |
| `a_read_callback_cannot_clear_the_read_only_guard` | `crates/storage/src/lib.rs:1299-1352` | A read callback is denied every pragma write, in any letter case, and writes nothing. | unaudited |
| `a_callback_cannot_damage_the_fence_row_it_is_checked_against` | `crates/storage/src/lib.rs:1414-1522` | A fenced callback or migration that lowers, deletes, or forges the fence and version rows is denied, and the row keeps its epoch. | unaudited |
| `a_callback_cannot_end_the_fence_checked_transaction` | `crates/storage/src/lib.rs:1355-1411` | `COMMIT`, `ROLLBACK`, `SAVEPOINT`, and `BEGIN` are denied inside a fenced callback and a migration, and nothing commits or is created unfenced. | unaudited |
| `maintenance_runs_through_the_unfenced_path` | `crates/storage/src/lib.rs:1525-1538` | `VACUUM` fails the read-only guard and succeeds through `with_conn_unfenced`. | unaudited |
| `fenced_write_rolls_back_on_error` | `crates/storage/src/lib.rs:1541-1574` | Callback failure rolls back both domain mutation and a newer fence claim. | unaudited |
| `legacy_database_without_fence_table_uses_zero_floor` | `crates/storage/src/lib.rs:1577-1599` | A pre-fence-table database opens at floor zero and receives epoch 1. | unaudited |
| `legacy_negative_database_fence_fails_closed` | `crates/storage/src/lib.rs:1602-1629` | A pre-constraint negative fence is rejected and remains unchanged. | unaudited |
| `superseded_writer_is_fenced_out_after_handover` | `crates/storage/src/lib.rs:1632-1670` | Synthetic epoch-1 writer cannot overwrite epoch-2 state. | unaudited |
| `superseded_writer_cannot_migrate` | `crates/storage/src/lib.rs:1673-1703` | Synthetic stale migration is fenced before its schema SQL executes. | unaudited |
| `equal_epoch_writer_is_not_fenced` | `crates/storage/src/lib.rs:1706-1726` | Equal epoch can continue writing. | unaudited |
| `epoch_above_sqlite_integer_range_fails` | `crates/storage/src/lib.rs:1729-1744` | Epochs above SQLite's signed integer range fail instead of wrapping. | unaudited |
| `fence_and_version_tables_keep_their_ddl` | `crates/storage/src/lib.rs:1755` | `sqlite_schema` records the pinned names and DDL of both infrastructure tables and one fence row. | audited (U2) |
| `open_migrate_and_single_writer` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:613-644` | Live PostgreSQL covers migration and session exclusion. Requires `CORTEXKIT_TEST_PG_DSN`; CI has a required live job. | unaudited |
| `read_only_callback_rejects_mutation_without_rows` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1051-1083` | Read-only mutation reports SQLSTATE `25006` and leaves rows unchanged. | unaudited |
| `open_verifies_the_stored_epoch_matches_the_issued_one` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1002-1026` | Open re-reads the committed lease row and requires it to carry the epoch it issued. | unaudited |
| `a_suppressed_epoch_increment_is_rejected` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:982-999` | Issuing an epoch that did not advance past the stored row is rejected. | unaudited |
| `a_callback_cannot_damage_the_lease_row_it_is_fenced_against` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:935-979` | A fenced callback that deletes or lowers its own lease row is rejected and rolled back. | unaudited |
| `a_callback_that_ends_the_transaction_is_rejected` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:647-689` | A fenced callback or migration that sends `COMMIT` is rejected instead of reporting success. | unaudited |
| `a_read_callback_cannot_escape_read_only_mode` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:795-854` | A read callback that sends `COMMIT` or `SET TRANSACTION READ WRITE` is rejected; only the autocommitted write survives. | unaudited |
| `a_regressed_positive_epoch_fails_closed` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:890-932` | A stored epoch below the one stamped at open fails closed for the holder and for a superseded writer above it. | unaudited |
| `a_negative_epoch_fails_closed` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:857-887` | The lease table rejects a negative epoch, and a negative epoch reaching the fence returns `FenceCorrupt`. | unaudited |
| `unfenced_callback_runs_statements_a_transaction_forbids` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1029-1048` | `VACUUM` reports SQLSTATE `25001` inside a fenced transaction and succeeds through the autocommit callback. | unaudited |
| `fenced_callback_error_rolls_back_rows` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1086-1119` | Callback failure rolls back domain rows. | unaudited |
| `repeated_fenced_writes_at_current_epoch_succeed` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1122-1141` | Repeated writes at the current lease epoch succeed. | unaudited |
| `superseded_writer_is_rejected_after_reopen` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1144-1175` | Synthetic stale callback is rejected after reopen. | unaudited |
| `superseded_writer_cannot_migrate` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1178-1213` | Synthetic stale migration is fenced before its schema SQL executes. | unaudited |
| `independent_namespace_chains` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1216-1245` | Independent migrations both apply. | unaudited |
| `advisory_key_derivation_is_stable` | `commons@89abb40 crates/cortexkit-store-postgres/src/lib.rs:1250-1253` | Pins one advisory bigint derived through public `LeaseKey::identity` and `fnv1a`. | unaudited |

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
