# Shared primitives property catalog

Records for the four shared Rust crates: `lease`, `storage`, `storage-types`,
and `cache-stability`. `index.json` is generated from this file; the record
contract is [`../METHOD.md`](../METHOD.md).

## Provenance and scope

- Source catalog: the lease property catalog at
  `primitives@89abb409b8c71b03146eedb5bf64cd964f2a92c0` (29 records covering
  the lease crate and the SQLite fence it guards). Those records enter with their source
  status; `migration/waves/U2/property-impact.json` names which are `core` and
  which are `carried-forward`.
- Discovery at U2, same source commit: 17 records for `cache-stability`,
  `storage-types`, and the non-lease behavior of `storage`, which had no
  source catalog. Their status is the status observed at discovery.
- The PostgreSQL backend in the source (`primitives@89abb40`) is not carried.
  Records that cited its checks keep the SQLite half; a PostgreSQL test named
  with the citation `primitives@89abb40` is source provenance, not a check in
  this tree. Receipts verify source blobs by hash, so no citation names a path
  or line range in the source.
- Line citations name files in this tree at the wave's destination commit.
  Test names are stable across formatting; line ranges are re-derived when a
  cited file changes.
- Observation constraint: `LeaseKey::identity`, `fnv1a`, and `fnv1a_hex` are
  public; `FileLeaseStore::lease_path` is private, so exact path checks live
  inside the `lease` crate. `LeaseKey` derives neither `Hash` nor `Ord`;
  checks index logical keys by the field tuple `(module_id, backend,
  scope_key)`.
- The parked lease-storage repack described in
  [`lease-store-density.md`](lease-store-density.md) is outside this catalog
  because that system is not built; the relationship map records its
  durability prerequisite.

Supporting artifacts:

- [System model](system-model.md)
- [Existing-check inventory](existing-checks.md)
- [Fault-to-property map](fault-map.md)
- [Relationship map](relationships.md)
- [Portfolio evaluation](portfolio-evaluation.md)
- [Durable consumer inventory](durable-consumer-inventory.md)
- [Per-property evidence](evidence/)

## Lease and fence records

### at-most-one-exclusive-holder-per-key

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet - Existing checks cover sequential same-process contention, not a concurrent exclusive race across independent processes.
Guarantee: Among cooperative participants using the same lease root and `LeaseKey`, at most one live guard returned by `FileLeaseStore::acquire` exists at every instant.
Check: `always` - `always(exclusive_live_count[(physical_root_identity, module_id, backend, scope_key)] <= 1)`, where physical root identity is canonicalized and, on Unix, confirmed by device/inode rather than raw path spelling. Each successful holder records that identity, process, epoch, acquire-return time, and release time in a witness ledger outside the lease root; the oracle rejects overlapping intervals.
Fault/timing angle: Two acquirers race from separate processes; path aliasing, inode replacement, or degraded filesystem lock semantics can let both return `Ok`.
Required faults and enabling state: Two processes must have the same lease file open concurrently and both must attempt exclusive locking. For faulted histories, also inject path aliasing, file replacement, or the deployed filesystem's lock-degradation mode.
Confidence: high - [evidence](evidence/at-most-one-exclusive-holder-per-key.md). The contract is explicit at `crates/lease/src/lib.rs:2-6`; `FileLeaseStore::acquire` delegates exclusion to `File::try_lock`.
Existing check: `acquire_then_second_holder_is_rejected`, same process and sequential, and `concurrent_exclusive_acquisitions_admit_exactly_one_holder`, same process, eight racers behind a barrier (added at U2); status **unaudited** for the record because the cross-process race is still not exercised.
Impact: Two writers can mutate one logical store. This is the crate's primary prohibited state.
Open questions:

- See the external credentials-store blocker in the [durable consumer inventory](durable-consumer-inventory.md). `(needs human input)`

### shared-exclusive-exclusion-matrix

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - on the local Unix test path; platform and filesystem variants remain unexercised.
Guarantee: At least two shared holders can coexist, but a live exclusive holder and any live shared holder never coexist for one root and key.
Check: `always` - Safety: `always(exclusive_count <= 1 && (exclusive_count == 0 || shared_count == 0))`. Availability coverage: `sometimes(shared_count >= 2)`. The first forbids mixed modes; the second proves the documented positive shared-coexistence path occurred.
Fault/timing angle: The last-of-many shared-holder drop is the discriminating transition; per-process lock semantics can release another handle's lock early.
Required faults and enabling state: At least two simultaneous shared holders, an exclusive attempt while both live, another attempt after one drops, and the reverse exclusive-then-shared history.
Confidence: high - [evidence](evidence/shared-exclusive-exclusion-matrix.md). `FileLeaseStore::acquire` and `FileLeaseStore::acquire_shared` use exclusive and shared OS locks.
Existing check: `shared_holders_coexist_but_block_exclusive`, `exclusive_holder_blocks_shared`, `concurrent_shared_first_acquisitions_coexist`, and `shared_lease_across_processes_blocks_exclusive`; status **unaudited**.
Impact: A GC can delete a resource under a live reader, or readers can enter while an exclusive mutator is active.
Open questions:

- Are Solaris or network filesystems supported, where the lock primitive may be process-scoped or host-scoped? `(needs human input)`

### dead-holder-lease-is-reclaimable

Type: liveness
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet - The cross-process test lets its child exit cleanly.
Guarantee: After a holder process dies without running `Drop`, another process can acquire the same key within the recovery bound.
Check: `always` - With a configured recovery bound `B`, after process death is confirmed and no unrelated holder exists, attempt acquisition until deadline `death_confirmed + B`; assert `always(acquired_by_deadline)`. The configured deadline makes the eventual claim exact.
Fault/timing angle: `SIGKILL`, abort, OOM kill, or equivalent termination while the handle is live.
Required faults and enabling state: A child must hold the real OS lock and be terminated without unwind; the harness must confirm process exit before starting the recovery deadline.
Confidence: high - [evidence](evidence/dead-holder-lease-is-reclaimable.md). High that this is intended (`crates/lease/src/lib.rs:4`); medium that every deployed filesystem supplies the promised behavior.
Existing check: `shared_lease_across_processes_blocks_exclusive` exits normally; status **unaudited**.
Impact: A dead writer can permanently prevent module restart.
Open questions:

- What recovery bound is operationally required? `(needs human input)`

### writer-epoch-strictly-increases

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - for malformed input, exhaustion, deterministic ordered prefix-write failures through `persist_epoch`, and SQLite sidecar loss recovered from the database fence. Exact `File` failures, arbitrary restore, process interruption, and power loss remain absent.
Guarantee: Every successful exclusive acquisition returns an epoch strictly greater than every epoch previously returned for the same physical root and key.
Check: `always` - On each successful exclusive acquisition, `always(epoch > max_returned_epoch[(physical_root_identity, module_id, backend, scope_key)])`, then update the witness. `bump_epoch_above` performs checked increment above persisted state and an optional resource floor; `persist_epoch` performs canonical persistence.
Fault/timing angle: Unsynced write loss, old-file restore, and machine power loss remain threats. Malformed input and a persisted `u64::MAX` fail closed in exercised local paths.
Required faults and enabling state: At least one prior successful acquisition, followed separately by each fault class. The maximum-value case requires the parser to observe `18446744073709551615` and repeated exclusive attempts to return `LeaseError::Io(InvalidData)` without changing bytes; counting to it is not required.
Confidence: high - [evidence](evidence/writer-epoch-strictly-increases.md). High for bounded parsing, checked increment, and ordered-prefix behavior in the injected `Write + Seek` model; exact partial-`File` I/O and power-loss behavior remain unproved.
Existing check: `exclusive_epoch_exceeds_resource_floor`, `database_epoch_survives_repeated_lease_sidecar_loss`, `invalid_epoch_states_fail_closed`, and `interrupted_persist_never_leaves_a_lower_parseable_epoch`, plus clean acquisition checks; status **unaudited**.
Impact: Reused or regressed epochs let a superseded writer pass an equal-or-older fence or can permanently reject legitimate writers.
Open questions:

- What recovery behavior is required after an older valid lease file is restored?

### returned-epoch-is-crash-durable

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet - store-instance recreation does not test device durability.
Guarantee: An epoch returned by `acquire` survives power loss and stays strictly below every later returned epoch for the same physical root and key.
Check: `always` - Witness by `(physical_root_identity, module_id, backend, scope_key)`; after each acknowledged acquisition and crash-image recovery, assert `always(reacquired_epoch > acknowledged_epoch)`. This checks the durability promise at the acknowledgement boundary.
Fault/timing angle: Power loss after file creation or epoch write but before file data and directory entry reach stable storage.
Required faults and enabling state: A first-ever acquisition to exercise directory-entry durability, a later acquisition to exercise content durability, and volatile-cache loss rather than process death alone.
Confidence: high - [evidence](evidence/returned-epoch-is-crash-durable.md). High that no `sync_data`, `sync_all`, or directory sync exists. `persist_epoch` calls `Write::flush`, which is not a durability barrier, so no power-loss atomicity or durability claim follows.
Existing check: `epoch_persists_across_store_instances`; status **unaudited**.
Impact: A post-reboot writer can reuse a superseded writer's token.
Open questions:

- Is machine-power-loss durability required? If so, the write and directory-entry protocol needs a separate design and crash test. `(needs human input)`

### invalid-epoch-fails-closed

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - existing empty content through ordinary acquisition; non-decimal UTF-8, trailing whitespace, invalid UTF-8, oversized input, 20-digit `u64` overflow, a leading `+` or `-`, surrounding spaces, hex, and a digit separator through ordinary and floor-based acquisition.
Guarantee: Ordinary and shared acquisition reject empty content. Every acquisition mode rejects nonempty content that is not a valid `u64` without issuing an epoch.
Check: `always` - For each invalid existing body, use `always(matches!(acquire(key), Err(LeaseError::Io(error))) && error.kind() == InvalidData && bytes_after == bytes_before)`. A corruption-specific public variant does not exist; `LeaseError` exposes only `Held` and `Io`.
Fault/timing angle: Valid-UTF-8 garbage, decimal overflow, foreign non-decimal writes, and future-format bytes. A torn body that remains valid decimal belongs to epoch monotonicity, not this parse-failure property.
Required faults and enabling state: Place malformed content in the exact derived lease path while no holder is live; include invalid UTF-8 and content longer than 20 bytes.
Confidence: high - [evidence](evidence/invalid-epoch-fails-closed.md). High for the exercised input classes. `read_epoch` accepts only 1-20 ASCII digits in `u64` range.
Existing check: `invalid_epoch_states_fail_closed` asserts exact `LeaseError::Io(InvalidData)` classification and unchanged bytes for twelve malformed bodies; status **audited** at U2.
Impact: Silent reset to epoch 1 reissues old fence tokens.
Open questions: None. for the current format. Empty content is recoverable only through `acquire_above` with an authoritative floor.

### failed-acquire-preserves-prior-epoch

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - a deterministic injected short writer calls production `persist_epoch`; no `File` or filesystem error is injected.
Guarantee: An exclusive acquisition that returns `Err` does not lower the prior persisted epoch: any parseable aftermath is greater than or equal to the prior value, and an unparseable aftermath fails closed on the next acquisition. Byte-for-byte preservation of the prior value is not claimed; a canonical overwrite interrupted after progress can leave a higher decimal splice.
Check: `always` - Around each forced post-lock failure, `always(after_epoch_bytes parse to a value >= before_epoch)`. This checks durable state, not only that the lock was released.
Fault/timing angle: `ENOSPC`, `EDQUOT`, or returned `EIO` after a positive write prefix. Non-returning termination and power loss belong to crash recovery, not this property.
Required faults and enabling state: A prior nonzero epoch and an injected error after positive progress, with acquisition returning `Err`.
Confidence: medium - [evidence](evidence/failed-acquire-preserves-prior-epoch.md). `interrupted_persist_never_leaves_a_lower_parseable_epoch` exercises the helper's padding and fixed-width write order for variable-width and canonical-width prior states and asserts how many prefixes stay parseable, but it does not prove exact `File` behavior under a real device error.
Existing check: `interrupted_persist_never_leaves_a_lower_parseable_epoch`; status **unaudited**.
Impact: A transient storage error turns into permanent fence regression.
Open questions:

- Which real filesystem fault mechanism can exercise the same positive-prefix error through `File` without adding a production failpoint?

### distinct-lease-keys-do-not-alias

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - `separator_in_a_key_field_fails_closed_instead_of_aliasing` places `U+001F` in each field and asserts that `LeaseKey::identity` panics naming that field; `distinct_identity_axes_do_not_conflict` covers separator-free keys that differ in one axis; no check covers FNV-1a-64 collisions.
Guarantee: Distinct `LeaseKey` values produce distinct identity strings, and a field that would make the encoding ambiguous is rejected. Distinct identities map to distinct lease files except under an FNV-1a-64 collision, which no check detects; the property is unproven for colliding identities.
Check: `always` - Inside the crate, `always(k1 == k2 || lease_path(k1) != lease_path(k2))` for generated and adversarial pairs, where a key that carries the separator is rejected before it reaches a path. The file stores no identity, so collision detection by stored-key verification is a design follow-up, not an available check.
Fault/timing angle: No timing fault is needed. A field containing `U+001F` would make the tuple encoding ambiguous; `LeaseKey::identity` refuses such a field with a panic. FNV-1a-64 has no collision handling.
Required faults and enabling state: Construct keys containing the separator in different fields. A targeted FNV collision is a separate enabling state whose practical cost remains open.
Confidence: high - [evidence](evidence/distinct-lease-keys-do-not-alias.md). High that the separator is rejected in every field (`LeaseKey::identity`, `crates/lease/src/lib.rs:179-194`); high that collision handling is absent; low on practical targeted-FNV cost.
Existing check: `distinct_identity_axes_do_not_conflict` covers distinct scope, module, and backend axes; `separator_in_a_key_field_fails_closed_instead_of_aliasing` (`crates/lease/src/lib.rs:1365-1388`) covers the separator in each field; status **unaudited**.
Impact: Unrelated stores falsely contend and share one epoch sequence.
Open questions:

- Are key fields attacker-controlled in any external consumer? `(needs human input)`

### lease-inode-remains-stable-while-held

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet
Guarantee: Replacing a held lease path never permits a competing acquisition to succeed on a different inode for the same logical root and key.
Check: `always` - During replacement histories, `always(competing_acquire_succeeded => competing_inode == incumbent_inode)`. Path/inode divergence is the enabling state, not itself the forbidden outcome.
Fault/timing angle: Unlink, rename, restore, bind-mount replacement, or cleanup while the old inode remains locked through an open descriptor.
Required faults and enabling state: A live holder, external replacement of its lease path, then a second acquisition.
Confidence: high - [evidence](evidence/lease-inode-remains-stable-while-held.md). The hazard follows from descriptor-bound locks, and `lease-store-density.md:22-24` explicitly names the unlink-inode race.
Existing check: none.
Impact: Two exclusive locks can succeed on two inodes for one logical key, and both epoch sequences can restart.
Open questions:

- Which deployed actors can unlink or replace lease files? `(needs human input)`

### shared-acquisition-is-epoch-neutral

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - `shared_acquisition_does_not_bump_the_write_epoch` holds a nonzero writer epoch with sequential shared holders; `concurrent_shared_first_acquisitions_coexist` and `shared_holders_coexist_but_block_exclusive` hold simultaneous shared holders only at epoch zero; no history combines a nonzero writer epoch with simultaneous shared holders.
Guarantee: Shared acquisition over an existing valid lease file does not change its persisted writer epoch. Shared-first creation initializes canonical epoch zero and does not issue a writer epoch.
Check: `always` - For existing files, `always(epoch_bytes_after_shared_acquire == epoch_bytes_before_shared_acquire)`. For first creation, assert canonical zero and require the first exclusive acquisition to return one.
Fault/timing angle: Concurrent shared holders matter because a future refactor that writes metadata into the file can create lost updates or consume fence values.
Required faults and enabling state: A nonzero writer epoch and at least two simultaneous shared holders in the same history; no injected fault is required.
Confidence: high - [evidence](evidence/shared-acquisition-is-epoch-neutral.md). High on local tests. `open_lease_file` initializes canonical zero before publication; every shared acquirer then uses only `File::try_lock_shared` and `read_epoch`.
Existing check: `shared_first_initializes_canonical_zero`, `concurrent_shared_first_acquisitions_coexist`, and `shared_acquisition_does_not_bump_the_write_epoch`; status **unaudited**.
Impact: Readers consuming writer epochs can prematurely fence legitimate writers.
Open questions: None. This record is explicitly limited to epoch bytes; metadata effects are covered by permission and failed-acquisition records.

### shared-epoch-never-authorizes-write

Type: safety
Reachability: default-production - no production caller in this workspace uses `acquire_shared`; the property binds any consumer that does.
Status: active
Exercised: not yet - No in-repo production caller uses `acquire_shared`.
Guarantee: An epoch obtained from a shared handle is never used to authorize or stamp a durable write.
Check: `always` - No runtime mode check exists because `HeldFileLease` exposes no mode. The available check is source-level: `always(no value returned by acquire_shared reaches a durable write fence or stamp)` across every consumer. If the interface later carries mode, add `always(guard.mode == Exclusive)` at every write-fence boundary.
Fault/timing angle: No fault is needed. Both acquisition methods return `HeldFileLease`, while shared guards report the incumbent writer epoch.
Required faults and enabling state: A consumer that accepts both handle modes and routes `epoch()` to a durable write path.
Confidence: high - [evidence](evidence/shared-epoch-never-authorizes-write.md). High that `HeldFileLease` cannot distinguish modes; low that a current unseen consumer misuses it.
Existing check: `shared_acquisition_does_not_bump_the_write_epoch` pins equal epoch values but not their use; status **unaudited**.
Impact: A reader can present the live writer's epoch as write authority.
Open questions:

- Which external repository consumes shared mode, and should the interface split reader and writer handles or expose mode? `(needs human input)`

### unix-lease-file-is-owner-only

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - `an_acquired_lease_file_is_owner_only` covers an exclusive acquisition over a pre-existing `0644` file on Unix; shared acquisition over a pre-existing permissive file, replacement after descriptor open, and the creation window are not exercised.
Guarantee: After a successful Unix acquisition, the locked lease file's permission bits are exactly `0600`.
Check: `always` - `always(mode(locked_inode) & 0o777 == 0o600)` after both exclusive and shared acquisition. Platform qualification is explicit; non-Unix behavior is not imported into this claim.
Fault/timing angle: Pre-existing `0644` files, restores, and copies. Creation-window exposure is a separate property.
Required faults and enabling state: Exercise exclusive and shared acquisition against pre-existing permissive files, including replacement after descriptor open. Confirm the opened/locked inode is the inode whose mode is checked.
Confidence: high - [evidence](evidence/unix-lease-file-is-owner-only.md). High for the intended Unix outcome; commit `49bcaa2` records the observed permissive deployment state and the fix rationale.
Existing check: `an_acquired_lease_file_is_owner_only`, Unix-only; status **unaudited**.
Impact: A writable lease file allows fence-token forgery. A readable file exposes key activity, though filenames are hashed.
Open questions:

- Windows acquisition rejects reparse points but does not provide Unix owner-only mode semantics. The public `protect_file` remains a no-op on Windows. `(needs human input)`

### permission-hardening-never-follows-replacement

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - structurally, for lease acquisition through descriptor-relative metadata and chmod; a concurrent replacement history is not injected.
Guarantee: Lease acquisition changes permissions only on the regular inode opened for that acquisition. Public `protect_file` is path-based and makes no concurrent-replacement guarantee.
Check: `always` - Whenever the chmod branch executes, `always(inspected_inode == chmod_target_inode)`, and assert every unrelated target's mode is unchanged. Pre-open symlink following is a separate property.
Fault/timing angle: Replace the final path component after open and before permission hardening.
Required faults and enabling state: Directory mutation permission plus a deterministic pause after open and before descriptor-relative metadata and chmod.
Confidence: high - [evidence](evidence/permission-hardening-never-follows-replacement.md). High that acquisition metadata inspection and chmod apply to the same opened descriptor (`protect_open_file`). Path replacement can still split lock domains. Public `protect_file` documents its path race between `symlink_metadata` and `set_permissions`.
Existing check: acquisition and public-helper symlink tests cover static links only; status **unaudited**.
Impact: Permission changes can land on a file the caller never named; acquisition may also create a symlink target before refusal.
Open questions:

- Which deployment actors can replace lease or SQLite/WAL/SHM paths during hardening? Lease acquisition retains its descriptor, while public `protect_file` intentionally avoids opening caller-owned files.

### contention-is-classified-as-held

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - the positive arm is covered by same-process, cross-process, and eight-way racing contention tests on the local platform; the negative arm has no test, because no injected non-contention lock error (`EACCES`, `ENOLCK`, `EOPNOTSUPP`) reaches `TryLockError::Error`, and other supported targets are not exercised.
Guarantee: A contended try-lock returns `LeaseError::Held`, while every other lock failure returns `LeaseError::Io`.
Check: `always` - Positive arm: while a known live holder exists, `always(matches!(result, Err(LeaseError::Held { .. })))`. Negative arm: for each injected non-contention lock error, `always(matches!(result, Err(LeaseError::Io(_))))`. The arms use different ground-truth mechanisms and are not collapsed into one biconditional.
Fault/timing angle: Platform-specific raw OS codes and filesystems that report unsupported/exhausted lock resources rather than the normal contention code.
Required faults and enabling state: Genuine contention for the positive arm; injected `EACCES`, `ENOLCK`, `EOPNOTSUPP`, or target equivalents for the negative arm.
Confidence: high - [evidence](evidence/contention-is-classified-as-held.md). High on Linux/macOS/Windows ordinary contention; lower on unsupported targets and filesystems.
Existing check: same-process and cross-process contention tests; status **unaudited**.
Impact: Callers can mistake a live holder for storage failure or a broken lock facility for ordinary contention.
Open questions:

- Is the CI matrix the complete supported platform set? `(needs human input)`

### filesystem-lock-scope-matches-deployment

Type: safety
Reachability: default-production - holds for whichever filesystem hosts the lease root; only deployment evidence can decide it.
Status: active
Exercised: not yet - cannot be exercised from this repository; mount and host topology are deployment evidence.
Guarantee: Every cooperative writer using the lease protocol and able to access a shared lease root participates in one lock domain for that root.
Check: `always` - For each deployed mount configuration, `always(acquire_on_B == Held)` while A holds the same key; run B on another host whenever the mount is shared.
Fault/timing angle: Node-local advisory locking, unsupported locking, overlay replacement, or process-scoped emulation.
Required faults and enabling state: The real deployment filesystem and mount options, with concurrent acquirers in every host/process topology that can access it.
Confidence: medium - [evidence](evidence/filesystem-lock-scope-matches-deployment.md). The crate accepts arbitrary paths and documents no filesystem contract.
Existing check: local-temp-directory cross-process shared contention in `shared_lease_across_processes_blocks_exclusive`; status **unaudited**.
Impact: Both hosts can believe they exclusively own one store.
Open questions:

- Where does each external consumer place its lease root, and with what mount options? `(needs human input)`

### epoch-input-size-is-bounded

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - a 21-byte file through exclusive and shared acquisition, and a 1 MiB file through all three acquisition paths with a counting reader that observes at most 21 bytes read.
Guarantee: Acquisition reads at most 21 epoch bytes and rejects any state longer than the 20-byte decimal maximum, independent of file size.
Check: `always` - `always(bytes_read_for_epoch <= 21)` and reject files larger than 20 bytes without proportional allocation. Whitespace is not part of the format.
Fault/timing angle: A corrupt, restored, or hostile multi-gigabyte lease file.
Required faults and enabling state: Replace a key's lease file with progressively oversized content while no holder is live; exercise both exclusive and shared acquisition paths.
Confidence: high - [evidence](evidence/epoch-input-size-is-bounded.md). `read_epoch` applies `Read::take(21)` and allocates capacity for 21 bytes before rejecting lengths above 20.
Existing check: `invalid_epoch_states_fail_closed` (21-byte file) and `epoch_read_is_bounded_regardless_of_file_size` (1 MiB file, counting reader, all three acquisition paths); status **audited** at U2.
Impact: Opening a store can exhaust process memory before the database is opened.
Open questions:

- A future versioned format must revise the 20-byte bound deliberately.

### lease-file-growth-trigger-is-observed

Type: liveness
Reachability: default-production - the watcher and its threshold live with the deployment owner, not in this repository.
Status: active
Exercised: not yet - cannot be exercised from this repository; watcher and acknowledgement evidence live with the deployment owner.
Guarantee: When a lease directory crosses the configured physical-size trigger, the assigned owner receives and acknowledges a re-open signal within a configured bound.
Check: `always` - Convergence is the property: `always(owner_acknowledged_reopen_signal)` after crossing a configurable campaign threshold within a bounded signal window with no injected watcher fault. Coverage: `reachable(watcher_evaluated_and_reported_size)` once per monitoring interval. Production uses 1 GiB; campaigns use a smaller constructible threshold.
Fault/timing angle: Long-running growth from ephemeral identities, watcher failure, ownership drift, and inode exhaustion before the byte threshold.
Required faults and enabling state: Sustained unique-key creation through an actual configured-threshold crossing, with the watcher healthy for the bounded acknowledgement check. Injected watcher failure is evaluated separately by heartbeat coverage.
Confidence: medium - [evidence](evidence/lease-file-growth-trigger-is-observed.md). The measurement and ownership assignment are documented at `lease-store-density.md:3-51`; watcher operation is outside this repository.
Existing check: none in the crate.
Impact: Unbounded file and inode growth can exhaust the filesystem; an unsafe cleanup can then trigger the unlink-inode race.
Open questions:

- Is the watcher still armed, and who watches inode availability? `(needs human input)`

### lease-path-format-is-version-stable

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: partial - six identity/hash/path vectors computed outside the crate pin the `identity()` encoding, the FNV-1a-64 digest, and the `.lease` filename, and acquisition is shown to create the pinned file; cross-version overlap and the advisory-key vector of the source's PostgreSQL backend remain unchecked.
Guarantee: Binaries that may overlap in one deployment derive the same lease path for the same key, or reject mixed-version operation before either acquires.
Check: `always` - Expand the checked-in vectors to representative keys, including empty and non-ASCII fields, and assert `always(derived_filename == golden_filename)`; a `U+001F` field is rejected by `LeaseKey::identity` and cannot carry a vector. Pin the PostgreSQL advisory bigint from the same public `LeaseKey::identity` and `fnv1a` derivation.
Fault/timing angle: Changing field order, separator, hash, suffix, or normalization while old and new processes overlap.
Required faults and enabling state: Two versions running concurrently against one lease root, including rolling restart and rollback.
Confidence: high - [evidence](evidence/lease-path-format-is-version-stable.md). High that `FileLeaseStore::lease_path` is a de facto persisted protocol. Crate version `0.3.0` changes only the source API; the path and 0.2 state format remain unchanged, and compatibility remains a manual convention rather than an automated gate.
Existing check: `identity_hash_derivation_is_stable` pins one identity and filename hash; `lease_path_vectors_are_version_stable` pins six sample-key vectors, with digests computed by an independent FNV-1a implementation, and the created filename; status **audited** at U2. The PostgreSQL `advisory_key_derivation_is_stable` vector belongs to the source's PostgreSQL backend (`primitives@89abb40`).
Impact: Old and new binaries lock different files and can both write.
Open questions:

- Is mixed-version overlap supported for all consumers? `(needs human input)`

### stale-writer-write-is-rejected

Type: safety
Reachability: default-production - every `SqliteStore` write path is fence-checked or explicitly unfenced.
Status: active
Exercised: partial - SQLite and live PostgreSQL tests reject synthetic stale stores and preserve domain state. Real retained-connection handover and unfenced SQLite paths remain missing.
Guarantee: On write paths declared fence-protected, after a replacement writer claims epoch `n`, every write attempt from epoch `< n` is rejected before effects.
Check: `always` - `always(effects_begin => holder_epoch >= authoritative_epoch)`. For every stale attempt, assert an explicit fenced result and unchanged application state.
Fault/timing angle: A stale connection remains usable after its lease is released and a replacement acquires a newer epoch.
Required faults and enabling state: Real handover, retained old connection, replacement fence claim, then a late old-writer mutation. Run for every path declared fence-protected; fence-coverage completeness is a separate property.
Confidence: high - [evidence](evidence/stale-writer-write-is-rejected.md). High that both concrete fenced callbacks compare the persisted epoch and bind the comparison and callback effects to one transaction.
Existing check: `superseded_writer_is_fenced_out_after_handover`, `crates/storage/src/lib.rs:2063-2119`, and `superseded_writer_is_rejected_after_reopen`, `primitives@89abb40`; both use synthetic stale stores and remain **unaudited**.
Impact: A superseded process can overwrite state owned by its replacement.
Open questions:

- Which external write sites remain outside the concrete fenced callbacks? See `protected-write-set-is-fence-complete`.

### logical-store-has-single-lease-identity

Type: safety
Reachability: default-production - `open_sqlite` derives the lease root from the database path.
Status: active
Exercised: partial - sibling databases in one directory and symlink aliases are covered; same-database descriptor disagreement and hardlink aliases are not.
Guarantee: All cooperative writers for one logical store derive the same `(base_dir, LeaseKey)`, while distinct stores that must write independently derive different identities.
Check: `always` - `always(same_logical_store => lease_identity_a == lease_identity_b)` and `always(independent_stores => lease_identity_a != lease_identity_b)`, where identity includes canonical root plus all three key fields.
Fault/timing angle: The lease key includes the database file name and the root is the database parent, so sibling files with equal descriptors get distinct leases. Every open refuses a symlink anywhere in the database path (`SQLITE_OPEN_NOFOLLOW` plus `O_NOFOLLOW` at creation), so neither a directory alias nor a file alias reaches the bytes. One database opened with differing module or namespace values, or through a hardlink alias, still splits into independent locks.
Required faults and enabling state: Open the same SQLite database through descriptors differing in module or namespace; open it through a hardlink alias; open it through directory or file symlink aliases; open two sibling database files under one parent with equal key fields.
Confidence: high - [evidence](evidence/logical-store-has-single-lease-identity.md). High on derivation facts (`crates/storage/src/lib.rs:62-74,565-661`); unknown whether descriptor authority prevents these combinations in deployment.
Existing check: `distinct_databases_do_not_falsely_contend` (`crates/storage/src/lib.rs:1599-1608`) uses different parent directories; `distinct_databases_in_one_directory_do_not_falsely_contend` (`:1610-1621`) proves sibling files in one directory do not contend; `symlinked_database_paths_are_refused_never_aliased` (`:1111-1167`) proves both a directory-symlink alias and a file-symlink alias are refused, with and without a live holder. No test covers descriptor disagreement or hardlink aliases; status **unaudited**.
Impact: One store can have two writers, or independent stores can falsely block each other.
Open questions:

- What component guarantees descriptor uniqueness and canonical database paths? `(needs human input)`

### failed-acquisition-does-not-mutate-lease-state

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet
Guarantee: An attempt rejected as `Held` does not change the incumbent lease file's bytes, mode, owner, or modification time.
Check: `always` - Around every known-contended attempt, `always(after_state == before_state)` for content and metadata of the incumbent inode.
Fault/timing angle: Both acquisition paths create/open and call `protect_open_file` before try-lock, so a non-holder can chmod the file before learning it is contended.
Required faults and enabling state: A live holder plus a competing process that sees a deliberately permissive mode before attempting acquisition.
Confidence: high - [evidence](evidence/failed-acquisition-does-not-mutate-lease-state.md). High on operation order in `open_lease_file`, `FileLeaseStore::acquire`, and `FileLeaseStore::acquire_shared`.
Existing check: none.
Impact: A rejected actor mutates state it never owned; foreign-owned or read-only roots also collapse into undifferentiated `Io` failures.
Open questions:

- Is single-UID ownership a supported precondition or merely a deployment habit? `(needs human input)`

### handle-drop-releases-lease

Type: liveness
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - on current CI-style local filesystems; toolchain and target variants remain partial.
Guarantee: After the last handle for a root and key is dropped, a retrying cooperative acquirer succeeds within configured bound `B`.
Check: `always` - A competitor first observes `Held`, then retries on a fixed campaign schedule. Drop the last handle and assert `always(acquired_by(drop_time + B))`, under the stated scheduler-fairness assumption.
Fault/timing angle: `Drop` discards errors from standard-library `File::unlock`; descriptor close is the final release mechanism.
Required faults and enabling state: A competitor that has observed `Held` and continues retrying, last-handle drop, injected `File::unlock` error where possible, and every supported target/toolchain family.
Confidence: high - [evidence](evidence/handle-drop-releases-lease.md). High for current Linux/macOS/Windows close semantics. The destination pins Rust 1.98, above the 1.89 release that stabilized `File::try_lock`, `File::try_lock_shared`, and `File::unlock`; CI formats, lints, tests, and documents the workspace on the pinned 1.98 toolchain and lints and tests it on moving stable.
Existing check: the contention and cross-process tests reacquire after clean drop/exit; status **unaudited**.
Impact: A cleanly stopped module can leave its successor unable to start.
Open questions:

- Resolved at U2: the workspace pins `rust-version = "1.98"` and `rust-toolchain.toml` to 1.98; CI formats, lints, tests, and documents the workspace on that pinned toolchain, lints and tests it on moving stable, and checks the pinned toolchain against a regenerated lockfile of latest dependencies. `File::try_lock` and `File::try_lock_shared` (stable since 1.89) are inside the pinned version.

### replacement-fence-is-claimed-before-old-writer-writes

Type: safety
Reachability: default-production - every `open_sqlite` claims the fence before returning.
Status: active
Exercised: partial - `open_claims_fence_before_return` verifies that `open_sqlite` does not return before stamping its epoch, and `open_claim_rejects_an_epoch_the_database_already_stores` verifies that the open claim refuses an epoch equal to the stored fence. No retained old connection races the interval between lease acquisition and the internal claim.
Guarantee: Once a replacement store has committed its fence claim, no old-epoch writer can commit an effect against the database: every old-epoch fenced attempt or in-flight transaction aborts as `Fenced` and leaves application state unchanged. The interval between the replacement's lease acquisition and its committed claim is not covered by this guarantee and is unverified.
Check: `always` - `always(old_epoch_effect_commits => replacement_claim_not_yet_committed)`. After the replacement's claim commits, every old-epoch attempt or in-flight transaction must abort as fenced and leave application state unchanged.
Fault/timing angle: `open_sqlite` acquires the file lease before it obtains the SQLite `IMMEDIATE` transaction used to claim the database fence. A retained old transaction can race inside that internal interval, although no replacement store is exposed before the claim commits. The floor is also read before the lease is held, so a fence advance in that interval makes the issued epoch equal to the stored one; `claim_fence_strict` fails the open rather than duplicating an epoch.
Required faults and enabling state: Retain an old connection after releasing its lease, pause replacement open after lease acquisition, and race an old transaction against the replacement's `IMMEDIATE` claim.
Confidence: high - [evidence](evidence/replacement-fence-is-claimed-before-old-writer-writes.md). High that every returned store has claimed a strictly greater epoch (`crates/storage/src/lib.rs:565-661,880-895`); the stronger acquisition-instant guarantee remains unproved.
Existing check: `open_claims_fence_before_return`, `crates/storage/src/lib.rs:1200-1215`, observes the claim before domain setup, and `open_claim_rejects_an_epoch_the_database_already_stores`, `:1217-1252`, pins the strict-advance rule at the helper rather than through two racing opens; status **unaudited**.
Impact: A superseded writer can commit during the handover window the fence is meant to close.
Open questions:

- Does the guarantee begin at internal file-lease acquisition or when `open_sqlite` returns? `(needs human input)`

### protected-write-set-is-fence-complete

Type: safety
Reachability: default-production - applies to the SQLite backend; the PostgreSQL backend in the source (`primitives@89abb40`) is not carried.
Status: active
Exercised: partial - Both backends reject a write through their ordinary callback and reject a callback that ends the fence-checked transaction, and each retains one deliberately unfenced maintenance surface. Consumer-side completeness is still unproved, and the enumerated protected write-site set does not exist.
Guarantee: Every durable mutation declared protected by lease fencing commits only after at least one authoritative fence check atomically bound to that mutation.
Check: `always` - For the enumerated protected write-site set, `always(protected_effect_commits => authoritative_atomic_fence_checks >= 1)`, with a source-level inventory proving no protected write path bypasses the checked transaction.
Fault/timing angle: `storage` runs `with_conn` under `PRAGMA query_only`, so an unfenced write fails `SQLITE_READONLY` instead of committing; `with_conn_unfenced` carries the maintenance statements the guard and the fenced transaction both reject. The PostgreSQL backend in the source (`primitives@89abb40`) mirrors this with a read-only transaction plus `with_client_unfenced`. On SQLite a consumer's schema is one baseline text applied inside the open transaction, and any later DDL runs through `with_conn_fenced`, so schema SQL is fenced exactly like DML. Four effects escape a naive binding. Enforcement is connection state rather than statement state, so a read scope that ends by unwinding must still clear the pragma. The callback can also reach that state itself: A callback holding `&Connection` can call `Connection::authorizer` and remove any guard installed for it, so no statement rule survives on its own; SQLite therefore hands guarded callbacks a `GuardedConn` that omits authorizer control, pragma writes, statement batches, and transaction control, and additionally denies at the statement level pragma writes, transaction control, savepoints, `ATTACH`/`DETACH`, and every non-read action naming the `fence` or `format_marker` tables, including the triggers, indexes, virtual tables, and views that reach those rows without naming them in DML. A rename is authorized on its source name alone, so a temporary table renamed onto an infrastructure name is caught after the callback and before commit. Authorizer control is absent from the maintenance handle as well, so releasing a scope cannot discard a policy a caller installed. Protected transactions re-pin `synchronous=FULL` and a WAL journal behind the unrestricted maintenance path. A callback that sends `COMMIT`, or on PostgreSQL `SET TRANSACTION READ WRITE`, escapes its transaction, after which its statements run unfenced; SQLite denies the statement, while PostgreSQL, lacking an authorizer, can only verify afterwards. A persisted epoch that cannot be reconciled with the holder's - negative, or on PostgreSQL below the epoch stamped at open - authorizes a superseded writer under a one-sided comparison, so both backends fail closed instead. Nothing reserves the PostgreSQL lease table against callback SQL, so a callback can delete or lower the very row its write was fenced against.
Required faults and enabling state: Exercise or inspect every public durable-write boundary and classify whether fencing is required by its contract. Include a panicking read callback, a read callback that sets `query_only` or `synchronous`, a callback that sends transaction-control SQL or changes the access mode, and a negative persisted epoch.
Confidence: high - [evidence](evidence/protected-write-set-is-fence-complete.md). High that the SQLite and PostgreSQL ordinary callbacks reject writes, that a callback cannot lift the SQLite guard or lower fence durability, that each unfenced maintenance surface is reachable only by name, that a callback escaping either the checked or the read-only transaction is rejected rather than reported as success, and that a negative epoch fails closed on both backends. Both limits are PostgreSQL-only, because SQLite withdraws the capability and denies the escape before it executes: on PostgreSQL a callback that ends the transaction and opens a replacement is not detected, and an escape that autocommits before the check can only be reported, while a mode switch that keeps the transaction open rolls back. A PostgreSQL equivalent of the SQLite capability withdrawal does not exist, so narrowing what its callback receives remains the structural fix there. The authoritative protected write set and the consumer upgrade to the fenced APIs still need external ownership.
Existing check: backend tests cover fenced callbacks, fenced DDL, SQLite and PostgreSQL read-only rejection, and both autocommit maintenance paths. `a_panicking_read_does_not_strand_the_connection_read_only` (`crates/storage/src/lib.rs:1805-1825`) leaves later fenced writes authorized; `a_read_callback_cannot_lower_fence_durability` (`:1827-1893`) pins `synchronous=FULL` and a WAL journal; `a_read_callback_cannot_clear_the_read_only_guard` (`:1895-1943`) denies every pragma write; `a_callback_cannot_end_the_fence_checked_transaction` (`:1945-1990`) denies the transaction escape and `a_callback_that_ends_the_transaction_is_rejected` (`primitives@89abb40`) reports it; `a_read_callback_cannot_escape_read_only_mode` (`primitives@89abb40`) covers ending and switching a read transaction; `a_negative_epoch_fails_closed` (`primitives@89abb40`) and `a_regressed_positive_epoch_fails_closed` (`primitives@89abb40`) fail closed on an unreconcilable epoch; `a_callback_cannot_damage_the_lease_row_it_is_fenced_against` (`primitives@89abb40`), `a_suppressed_epoch_increment_is_rejected` (`primitives@89abb40`), and `open_verifies_the_stored_epoch_matches_the_issued_one` (`primitives@89abb40`) protect the lease row and its issuance; `a_callback_cannot_damage_the_fence_row_it_is_checked_against` (`crates/storage/src/lib.rs:1992-2095`) refuses DML, triggers, indexes, views, and renames onto the `fence` and `format_marker` tables; and `a_fenced_callback_cannot_rewrite_the_format_marker` (`:2097-2153`) refuses an update of, or a rename onto, the marker row. The [durable consumer inventory](durable-consumer-inventory.md) records source receipts; no source-level completeness gate exists.
Impact: Enforcement converts a silent unfenced commit into a loud failure on SQLite, where the capability can be withdrawn. On PostgreSQL the residual risk is unbounded by validation: callback SQL retains the privilege to attach triggers to the infrastructure tables and to release this session's advisory lease, so an effect can always be scheduled after the last check and single-writer itself is reachable, and consumers may also break on upgrade or reroute a mutation through the unfenced surface.
Open questions:

- The PostgreSQL infrastructure tables must be reserved from PostgreSQL callback DDL rather than validated afterward (SQLite callbacks already receive a `GuardedConn` with no authorizer, pragma, or transaction control), and this is the blocking decision rather than one option among several. Validation has been moved after the callback, after the increment, and to the committed row, and each position was defeated by moving the effect one step later: a `DEFERRABLE INITIALLY DEFERRED` constraint trigger fires at `COMMIT`, after any in-transaction re-read, and the store's own version-record insert runs after the last lease-row check. No check placed inside the transaction can be last. Reserving the tables needs a decision this catalog cannot make, because the candidates - running callback SQL under a restricted role through `SET LOCAL ROLE`, moving the tables to a schema the connection role cannot create objects in, or reaching them only through a `SECURITY DEFINER` function - all require role or schema provisioning outside the crate and change what a consumer's own DDL may do. Also unresolved: which SQLite writes are contractually fence-protected, do maintenance statements through `with_conn_unfenced` and `with_client_unfenced` count as protected writes, should the callbacks receive a capability-narrowed handle rather than one that can execute arbitrary SQL and pragmas, should the infrastructure tables be schema-qualified so a callback's session `search_path` cannot redirect the fence check, and who owns moving the host source's module-store mutations onto the fenced APIs, given that the mutations fail rather than commit? `(needs human input)`

### lease-file-creation-is-never-permissive

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: not yet
Guarantee: A newly created Unix lease file is never observable with permission bits wider than `0600`.
Check: `always` - `always(mode_observed_from_creation & 0o077 == 0)` and, structurally, assert the create operation requests exactly `0600`; a numeric comparison or post-acquisition check is insufficient.
Fault/timing angle: First creation uses `NamedTempFile` in the target directory and publishes its initialized inode with `persist_noclobber`.
Required faults and enabling state: First acquisition under permissive umasks plus a concurrent observer that opens during the create-before-chmod window.
Confidence: high - [evidence](evidence/lease-file-creation-is-never-permissive.md). High that `NamedTempFile` creates a private file and publication occurs only after initialization (`open_lease_file`); the dependency contract is part of this claim.
Existing check: T1 checks only post-acquisition steady state; status **unaudited**.
Impact: A racing process can retain access acquired before chmod.
Open questions:

- What umasks do supported deployments use? `(needs human input)`

### acquisition-does-not-follow-symlink

Type: safety
Reachability: default-production - every kernel and module-store open acquires an exclusive file lease through this crate.
Status: active
Exercised: yes - on Unix through both acquisition paths for a symlink to an existing target. Windows is compile-checked only; dangling-link and Windows runtime tests remain absent.
Guarantee: Exclusive and shared acquisition never create, open, lock, write, or chmod a symlink target as the lease file.
Check: `always` - With the derived lease path replaced by symlinks to existing and absent targets, assert `always(acquire_returns_error)`, `always(target_content_mode_and_existence_unchanged)`, and `unreachable("acquisition-owned-fd-resolved-to-target-inode")` using syscall or descriptor tracing.
Fault/timing angle: Unix uses `O_NOFOLLOW`; Windows opens the reparse point itself and rejects reparse metadata. Other non-Unix targets have no explicit no-follow flag.
Required faults and enabling state: Existing-target and dangling-target symlinks through both shared and exclusive methods on every supported platform.
Confidence: high - [evidence](evidence/acquisition-does-not-follow-symlink.md). High on Unix from `O_NOFOLLOW` and compile-checked on Windows from `FILE_FLAG_OPEN_REPARSE_POINT` plus attribute rejection (`lease_open_options`, `protect_open_file`); Windows runtime behavior remains untested.
Existing check: `acquisition_refuses_symlink_and_leaves_target_untouched` exercises exclusive and shared acquisition; status **unaudited**.
Impact: The lease can protect and overwrite an attacker-chosen inode or create an unintended file.
Open questions:

- Is Windows a supported deployment target? `(needs human input)`

### cross-process-exclusive-race-is-reached

Type: reachability
Reachability: test-only - campaign coverage witness for the exclusive race.
Status: active
Exercised: not yet
Guarantee: Every campaign for exclusive exclusion executes at least one history where two independent processes have the same lease file open and concurrently attempt exclusive acquisition.
Check: `sometimes` - `sometimes(distinct_processes >= 2 && same_root_and_logical_key && same_inode && both_waiting_at_pre_lock_barrier_before_either_try_lock)`. This is situation coverage, so `sometimes` fits.
Fault/timing angle: Scheduler ordering can otherwise serialize every attempt and let the safety check pass vacuously.
Required faults and enabling state: A barrier after both opens and before both try-lock calls, then concurrent release of the barrier.
Confidence: high - [evidence](evidence/cross-process-exclusive-race-is-reached.md). High that this is reachable; the current test machinery already spawns a child for shared mode.
Existing check: none; `shared_lease_across_processes_blocks_exclusive` reaches cross-process shared contention only.
Impact: Without this witness, `at-most-one-exclusive-holder-per-key` can pass without exercising its primary contention state.
Open questions: None.

### epoch-update-interruption-window-is-reached

Type: reachability
Reachability: test-only - campaign coverage witness for crash-recovery histories.
Status: active
Exercised: not yet
Guarantee: Every crash-recovery campaign interrupts at least one acquisition after epoch-update work begins and before the canonical value is fully written.
Check: `reachable` - `reachable("process-terminated-during-epoch-update")`. The event fires only after the harness confirms termination occurred inside `persist_epoch`.
Fault/timing angle: Random process kills are unlikely to land inside the short update sequence.
Required faults and enabling state: A deterministic process boundary during `persist_epoch`, followed by non-unwinding termination.
Confidence: high - [evidence](evidence/epoch-update-interruption-window-is-reached.md). High that the point is reachable in code; the injected short-writer test does not inject process death.
Existing check: `interrupted_persist_never_leaves_a_lower_parseable_epoch` covers ordered prefix-write outcomes only; status **unaudited**.
Impact: Without this witness, crash-recovery properties can pass vacuously. Returned-I/O-error preservation needs a separate injected error witness.
Open questions: None.

### live-lease-file-replacement-is-reached

Type: reachability
Reachability: test-only - campaign coverage witness for inode-stability histories.
Status: active
Exercised: not yet
Guarantee: Every inode-stability campaign executes at least one history where a lease path is replaced while its old inode remains locked by a live holder.
Check: `sometimes` - `sometimes(holder_live && path_identity != holder_inode_identity)`. This asserts the vulnerable precondition, not the forbidden outcome of two successful writers.
Fault/timing angle: Cleanup and restore actions may otherwise occur only before or after the holder lifetime.
Required faults and enabling state: A live holder, external unlink or rename of the path, and creation of a new file at the same path before holder release.
Confidence: high - [evidence](evidence/live-lease-file-replacement-is-reached.md). High that the state is constructible on local Unix filesystems; deployment reachability remains open.
Existing check: none.
Impact: Without this witness, inode-stability checks say nothing about the race documented in `lease-store-density.md:22-24`.
Open questions:

- Which production actor can create this state? `(needs human input)`

## Records discovered at U2

Cache-stability state machine, storage descriptor types, and the non-lease
behavior of the SQLite store.

### defer-pass-replays-frozen-bytes-verbatim

Type: safety
Reachability: default-production - every `SoftPlus` pass in the observer path takes this branch.
Status: active
Exercised: yes - a defer pass with no rendered units against a matching boundary, a defer against an absent boundary, a `run_started` defer over a lineage and an episode unit, and the eleven golden vectors, which assert the cached prefix after every pass.
Guarantee: A `SoftPlus` pass never re-renders, never changes `version`, and never changes the `frozen_payload` bytes or relative order of any frozen unit it retains, whatever units are queued and whether or not the boundary is present. When `run_started` is false, `cached_prefix_bytes()` is unchanged. When `run_started` is true, `CoreState::step` removes `DurabilityClass::Episode` units from `frozen_units` and `pending_changes` before any arm runs, so `cached_prefix_bytes()` can shrink by exactly the removed episode payloads while every retained lineage payload stays byte-identical.
Check: `always` - Without `run_started`: `always(bytes_after == bytes_before && version_after == version_before)` around every `step` whose `proposed` is `SoftPlus`. With `run_started`: `always(version_after == version_before && retained_lineage_payloads_after == lineage_payloads_before)`, where the retained set is the pre-step lineage units in their pre-step order. The state machine proves it structurally because `step_defer` touches only `pending_changes` and `reconcile_pending`, and the run-boundary filter in `step` only removes whole units.
Fault/timing angle: A defer pass that lost its boundary must keep replaying rather than rebuild in the same pass; a `run_started` defer drops episode units but must leave lineage bytes untouched.
Required faults and enabling state: A frozen set with at least one unit; a defer with the boundary present, one with it absent, and one with `run_started` set over a frozen set that holds both durability classes.
Confidence: high - [evidence](evidence/defer-pass-replays-frozen-bytes-verbatim.md). `step_defer` (`crates/cache-stability/src/lib.rs:191-217`) never reads `rendered_units` and mutates no frozen unit; the run-boundary filter in `CoreState::step` (`crates/cache-stability/src/lib.rs:168-189`) is the only path that removes units on a defer. `defer_does_not_mutate_frozen_bytes_or_render` (`crates/cache-stability/src/lib.rs:329-348`), `defer_boundary_absent_keeps_bytes_and_sets_reconcile_pending` (`crates/cache-stability/src/lib.rs:385-401`), and `run_started_keeps_lineage_resets_episode` (`crates/cache-stability/src/lib.rs:636-658`) pin the three arms, and every golden vector compares `cached_prefix_bytes()` after each pass.
Existing check: `defer_does_not_mutate_frozen_bytes_or_render`, `defer_boundary_absent_keeps_bytes_and_sets_reconcile_pending`, `run_started_keeps_lineage_resets_episode`, `all_golden_vectors_pass` (`crates/cache-stability/tests/golden_vectors.rs:159-166`); audited at U2 as an independent oracle: the byte and version assertions do not derive from the action under test.
Impact: A defer pass that re-renders breaks provider prefix-cache stability on every turn, which is the cost the whole mechanism exists to avoid.
Open questions: None.

### hard-bust-drains-deferred-work

Type: safety
Reachability: default-production - every `Hard` pass takes this branch.
Status: active
Exercised: yes - a queued drop followed by a `Hard` bust whose rendered baseline omits the drop.
Guarantee: After a `Hard` pass, the frozen set is exactly the rendered units followed by the drained units whose keys the render did not produce (the rendered bytes win, and a key the render no longer produces leaves the set); `pending_changes` is empty; when the pass carries `run_started`, only lineage units are retained across the boundary; and `boundary_id` equals the minted id when one is supplied. `reconcile_pending` is false when the pass mints a boundary, matches the existing boundary, or has no current boundary; a `Hard` that mints nothing while a minted anchor is absent keeps it true.
Check: `always` - `always(pending_changes.is_empty() && frozen_keys == rendered_keys ++ (queued_lineage_keys \\ rendered_keys))` after every `Hard` step, `boundary_id == new_boundary_id` when the input supplies one, and `!reconcile_pending` when `new_boundary_id.is_some() || boundary_match || boundary_id.is_empty()`.
Fault/timing angle: A hard bust from any cause must drain deferred work; a bust that only froze its rendered units would leave queued drops invisible until a later bust.
Required faults and enabling state: At least one unit queued through a `SoftPlus` pass before the `Hard` pass, and a rendered baseline that does not itself include the queued unit.
Confidence: high - [evidence](evidence/hard-bust-drains-deferred-work.md). `step_hard` (`crates/cache-stability/src/lib.rs:254-283`) appends `pending_changes` into the rendered set before `apply_units` and clears `reconcile_pending` only when it minted or the anchor is present; `hard_drains_pending_changes_into_the_bust` (`crates/cache-stability/src/lib.rs:455-488`) asserts the drain, the mint, and the cleared flag, and `hard_without_mint_on_absent_boundary_keeps_reconcile_pending` (`crates/cache-stability/src/lib.rs:719-754`) asserts the flag stays set when nothing reanchors.
Existing check: `hard_drains_pending_changes_into_the_bust`, `hard_prefers_rendered_units_over_deferred_copies_of_the_same_key`, `hard_drops_frozen_keys_the_render_no_longer_produces`, `hard_without_mint_on_absent_boundary_keeps_reconcile_pending`, golden vectors with `queued` units; audited at U2.
Impact: A dropped compartment reappears in the cached prefix or never leaves it, so the rendered context and the recorded state disagree.
Open questions: None.

### never-minted-boundary-is-not-reconcile-pending

Type: safety
Reachability: default-production - every fresh store starts with `boundary_id == ""`.
Status: active
Exercised: yes - three defers against an empty `boundary_id`, then a real mint followed by an absent-boundary defer.
Guarantee: A defer pass sets `reconcile_pending` only when a non-empty boundary id is absent from the live array; the empty id is the never-minted sentinel and never reads as a revert.
Check: `always` - `always(reconcile_pending == (!boundary_match && !boundary_id.is_empty()))` after every `SoftPlus` step.
Fault/timing angle: Without the guard an unseeded session oscillates `Hard` → defer → `Hard` forever, cache-neutral but with a permanently dishonest flag.
Required faults and enabling state: A state with `boundary_id == ""` and a defer whose `boundary_present` is any other token; then a `Hard` that mints a non-empty id and a defer that omits it.
Confidence: high - [evidence](evidence/never-minted-boundary-is-not-reconcile-pending.md). The guard is one expression in `step_defer` (`crates/cache-stability/src/lib.rs:191-217`); `defer_on_never_minted_boundary_is_stable_not_reconcile_pending` (`crates/cache-stability/src/lib.rs:350-383`) covers both the vacuous and the non-vacuous arm.
Existing check: `defer_on_never_minted_boundary_is_stable_not_reconcile_pending`, `defer_boundary_absent_keeps_bytes_and_sets_reconcile_pending` (`crates/cache-stability/src/lib.rs:385-401`); audited at U2.
Impact: Every fresh store busts hard on alternate passes, or a real revert is never reconciled.
Open questions: None.

### anchor-holds-while-reconcile-pending

Type: safety
Reachability: default-production - every `Soft` pass evaluates the guard.
Status: active
Exercised: yes - a coverage-extending `Soft` after a revert, and a coverage-extending `Soft` at a live anchor.
Guarantee: A `Soft` pass advances `boundary_id` only when the prior anchor is present and no reconcile is pending; it never clears `reconcile_pending`.
Check: `always` - `always(boundary_after == boundary_before || (boundary_match && !reconcile_pending_before))` and `always(reconcile_pending_after == reconcile_pending_before)` after every `Soft` step.
Fault/timing angle: A misclassified coverage-extending `Soft` while m0 is stale would strand the stale baseline under a fresh anchor, so the needed `Hard` never fires.
Required faults and enabling state: A revert that sets `reconcile_pending`, then a `Soft` with `new_boundary_id` set; and, separately, a `Soft` with `new_boundary_id` at a live anchor.
Confidence: high - [evidence](evidence/anchor-holds-while-reconcile-pending.md). The guard in `step_soft` (`crates/cache-stability/src/lib.rs:219-252`) is a let chain over both conditions; `soft_does_not_advance_anchor_while_reconcile_pending` (`crates/cache-stability/src/lib.rs:597-634`) and `coverage_extending_soft_advances_anchor_keeps_m0_frozen` (`crates/cache-stability/src/lib.rs:527-595`) pin both outcomes.
Existing check: `soft_does_not_advance_anchor_while_reconcile_pending`, `coverage_extending_soft_advances_anchor_keeps_m0_frozen`; audited at U2.
Impact: The stale m0 is never rematerialized, so the cached prefix summarizes content that the live array no longer contains.
Open questions: None.

### frozen-unit-order-is-preserved

Type: safety
Reachability: default-production - every bust applies units through `apply_units`.
Status: active
Exercised: yes - a `Soft` that replaces one existing key and appends one new key.
Guarantee: On a `Soft` pass, applying rendered units replaces a unit with the same key in its existing slot and appends new keys in input order; on a `Hard` pass the set is rebuilt in input order. Either way the cached-prefix byte order is the frozen-set order.
Check: `always` - `always(keys_after == keys_before ++ new_keys_in_input_order && replaced_units_keep_index)` after every `apply_units` call.
Fault/timing angle: None; the order is a pure function of the prior set and the input.
Required faults and enabling state: A frozen set with two units and a rendered set that replaces the second and adds a third.
Confidence: high - [evidence](evidence/frozen-unit-order-is-preserved.md). `apply_units` (`crates/cache-stability/src/lib.rs:285-297`) is a find-or-push loop; `soft_replaces_by_key_keeps_slot_appends_new` (`crates/cache-stability/src/lib.rs:490-525`) asserts the resulting key order and the untouched first unit.
Existing check: `soft_replaces_by_key_keeps_slot_appends_new`; the golden vectors compare `cached_prefix_bytes()`, which is order-sensitive; audited at U2.
Impact: A reordered prefix is a cache bust at the first moved byte on every later pass.
Open questions: None.

### episode-units-reset-at-run-boundary

Type: safety
Reachability: default-production - every `run_started` defer evaluates the durability filter.
Status: active
Exercised: yes - one lineage and one episode unit through a `run_started` defer.
Guarantee: On a `run_started` pass, every `Episode` unit leaves the frozen set and every `Lineage` unit keeps its bytes.
Check: `always` - `always(frozen_units_after == frozen_units_before.filter(Lineage))` after every step with `run_started`.
Fault/timing angle: The cache set is all-lineage in current consumers, so the filter is a no-op in practice and only the test exercises the `Episode` arm.
Required faults and enabling state: A frozen set containing at least one `Episode` unit and a `SoftPlus` pass with `run_started` set.
Confidence: high - [evidence](evidence/episode-units-reset-at-run-boundary.md). `CoreState::step` (`crates/cache-stability/src/lib.rs:168-189`) retains by `durability_class` before dispatching any action; `run_started_keeps_lineage_resets_episode` (`crates/cache-stability/src/lib.rs:636-658`) and `cross_episode_lineage_reproduces_byte_identical` (`crates/cache-stability/tests/golden_vectors.rs:274-320`) cover both classes.
Existing check: `run_started_keeps_lineage_resets_episode`, `cross_episode_lineage_reproduces_byte_identical`; audited at U2.
Impact: A run-scoped unit survives into the next episode, or a lineage unit is dropped and the prefix un-compacts.
Open questions: None.

### cache-stability-golden-vectors-are-byte-stable

Type: safety
Reachability: default-production - every consumer pins the same fixture.
Status: active
Exercised: yes - the fixture's bytes match the receipt hash, and all eleven vectors, the schema-3 empty wire format, and the cross-episode lineage vector pass in this workspace.
Guarantee: `tests/golden/cache-stability-golden-vectors.json` keeps the exact bytes its receipt entry pins, and every vector in it reproduces: after each pass the executed action, `cached_prefix_bytes()`, `boundary_id`, `reconcile_pending`, and the pending-change count equal the fixture.
Check: `always` - `always(sha256(fixture) == receipt.destination_sha256)` at receipt verification, and `always(observed_after_pass == expected_after_pass)` for every pass of every vector.
Fault/timing angle: The fixture is the cross-harness contract; a regenerated fixture would pass its own test, so byte identity to the receipt hash is checked outside the test.
Required faults and enabling state: A receipt entry whose `destination_sha256` pins the fixture, plus the test run against the in-tree crate.
Confidence: high - [evidence](evidence/cache-stability-golden-vectors-are-byte-stable.md). Receipt verification recomputes the fixture hash and compares it to the receipt; `all_golden_vectors_pass` (`crates/cache-stability/tests/golden_vectors.rs:159-166`) drives every vector through `CoreState::step`. The action-equality assertion is near-tautological because the harness feeds the expected action as `proposed`; the byte, boundary, flag, and queue assertions are independent.
Existing check: `golden_fixture_is_schema_v3_with_eleven_vectors` (`crates/cache-stability/tests/golden_vectors.rs:96-105`), `core_state_schema_v3_empty_wire_format_is_stable` (`crates/cache-stability/tests/golden_vectors.rs:107-157`), `all_golden_vectors_pass`, `cross_episode_lineage_reproduces_byte_identical`; audited at U2 with the tautology noted.
Impact: Two consumers pinned to different fixtures render different prefixes for the same history.
Open questions: None.

### storage-descriptor-golden-vectors-are-byte-stable

Type: safety
Reachability: default-production - the module-store descriptor is built from these helpers.
Status: active
Exercised: yes - the fixture's bytes match the receipt hash; `postgres_database_name`, `sqlite_store_path`, and descriptor reserialization reproduce all seven vectors in this workspace.
Guarantee: `tests/golden/storage_vectors.json` keeps the exact bytes its receipt entry pins, and for every vector `postgres_database_name(id)`, `sqlite_store_path(data_home, id)`, and the reserialized `sqlite_descriptor` equal the fixture.
Check: `always` - `always(sha256(fixture) == receipt.destination_sha256)` at receipt verification and `always(derived == fixture)` for each of the three derivations per vector.
Fault/timing angle: The generator example reproduces the fixture from the same code, so it cannot detect drift on its own; the checked-in bytes are the oracle.
Required faults and enabling state: A receipt entry whose `destination_sha256` pins the fixture, plus `helpers_reproduce_the_golden_vectors` against the in-tree crate.
Confidence: high - [evidence](evidence/storage-descriptor-golden-vectors-are-byte-stable.md). Receipt verification compares the fixture bytes to the receipt hash; `helpers_reproduce_the_golden_vectors` (`crates/storage-types/tests/golden_vectors.rs:11-42`) and `golden_vectors_break_slug_collisions` (`crates/storage-types/tests/golden_vectors.rs:44-61`) assert every derivation. The `eidnara_` prefix and the `eidnara/` path component are frozen identities in the registry.
Existing check: `helpers_reproduce_the_golden_vectors`, `golden_vectors_break_slug_collisions`; audited at U2 as an independent oracle.
Impact: A renamed prefix or path component makes a store open under a new path with an empty database while the old one keeps the data.
Open questions: None.

### descriptor-wire-shape-round-trips

Type: safety
Reachability: default-production - descriptors cross the host-to-module boundary as JSON.
Status: active
Exercised: yes - the SQLite descriptor against an exact JSON string and the PostgreSQL descriptor through a round trip.
Guarantee: A `StorageDescriptor` serializes with field names `module_id`, `storage_namespace`, `isolation`, `backend`, the internal tags `kind` and `backend` in snake_case, and deserializes back to an equal value.
Check: `always` - `always(from_json(to_json(d)) == d)` for every descriptor, and `always(to_json(sqlite_descriptor) == pinned_string)` for the SQLite shape.
Fault/timing angle: None; serde attributes fix the shape at compile time.
Required faults and enabling state: A descriptor of each backend variant.
Confidence: high - [evidence](evidence/descriptor-wire-shape-round-trips.md). `#[serde(rename_all = "snake_case", tag = ...)]` on `Isolation` and `StorageBackend` (`crates/storage-types/src/lib.rs:25-51`); `sqlite_descriptor_golden_json` (`crates/storage-types/src/lib.rs:210-227`) pins the exact string and `postgres_descriptor_golden_json` (`crates/storage-types/src/lib.rs:229-243`) the round trip; the golden fixture reserialization covers eight more.
Existing check: `sqlite_descriptor_golden_json`, `postgres_descriptor_golden_json`, `helpers_reproduce_the_golden_vectors`; audited at U2.
Impact: A host and a module built from different revisions disagree on the descriptor and the module opens the wrong database or none.
Open questions: None.

### store-schema-identity-matches-the-baseline

Type: safety
Reachability: default-production - every `open_sqlite` classifies the file before any pragma or transaction touches it.
Status: active
Exercised: yes - a fresh file compared to the inventory fixture; a file with a foreign table, a file opened under a different consumer baseline, and a fence row driven negative through a constraint bypass, each refused with the file bytes unchanged.
Guarantee: A store has exactly one schema. A pristine file receives the complete baseline text (`crates/storage/baseline.sql` followed by the consumer's DDL) once, with `application_id = 0x4549444e`, `user_version = 1`, and one `format_marker` row holding the SHA-256 of that text. Any other file opens only if its `application_id`, `user_version`, `sqlite_schema` inventory, and marker digest equal the baseline's; otherwise `open_sqlite` returns `StoreError::Baseline` and the file keeps every byte. No file is upgraded, adopted, or repaired.
Check: `always` - `always(fresh_file.identity == fixtures/schema/storage-inventory-v1.json)` for the storage-owned part, and `always(open(non_pristine) == Ok ⇔ identity(non_pristine) == identity(baseline))` with `always(bytes_after_refusal == bytes_before)`.
Fault/timing angle: The identity check runs on the opened connection before `journal_mode`, `synchronous`, or the fence transaction, so a refused file is never switched to WAL and never gains a sidecar. The inventory comes from applying the same text to an in-memory database, so SQLite's own DDL normalization is the comparator.
Required faults and enabling state: A pre-existing file with a foreign object, a second baseline text for the same path, and a bypassed `CHECK` on the fence row.
Confidence: high - [evidence](evidence/store-schema-identity-matches-the-baseline.md). `ExpectedIdentity::for_baseline` (`crates/storage/src/lib.rs:796-824`), `classify` (`:826-876`), and `apply` (`:878-910`) implement the contract inside `open_sqlite` (`:589-693`); `fresh_file_matches_the_baseline_inventory` (`:1409-1473`) pins the fixture, and `a_file_with_foreign_objects_is_refused_without_mutation` (`:1534-1560`), `a_consumer_baseline_is_applied_once_and_verified_on_reopen` (`:1475-1515`), `a_baseline_that_does_not_apply_is_rejected_before_the_file_is_touched` (`:1517-1532`), and `negative_database_fence_fails_closed` (`:2199-2227`) cover the refusals.
Existing check: `fresh_file_matches_the_baseline_inventory`, `a_file_with_foreign_objects_is_refused_without_mutation`, `a_consumer_baseline_is_applied_once_and_verified_on_reopen`, `a_baseline_that_does_not_apply_is_rejected_before_the_file_is_touched`, `a_baseline_that_hooks_an_infrastructure_table_is_rejected`, `fenced_callbacks_cannot_change_the_schema_so_the_store_stays_reopenable`, `negative_database_fence_fails_closed`; audited at U2.
Impact: A foreign or half-formed file is written to as if it were this store, or a renamed table makes the fence read return floor zero and reissue a superseded epoch.
Open questions: None.

### read-callbacks-cannot-write

Type: safety
Reachability: default-production - `with_conn` is the ordinary read path of every consumer.
Status: active
Exercised: yes - an `INSERT`, every pragma write in mixed case, and `VACUUM` through `with_conn`.
Guarantee: A `with_conn` callback cannot commit a durable write: writes fail with `SQLITE_READONLY`, pragma writes are denied by the authorizer, and the handle exposes no batch, pragma, or transaction control.
Check: `always` - `always(with_conn(f) ⇒ database_bytes_unchanged)` for every callback `f`, with the denial observed as an error rather than a silent no-op.
Fault/timing angle: The guard is connection state (`query_only` plus an authorizer), so it must be installed before the callback and cleared after it, including on unwind.
Required faults and enabling state: A callback that attempts DML, one that attempts `PRAGMA query_only = OFF`, and one that runs `VACUUM`.
Confidence: high - [evidence](evidence/read-callbacks-cannot-write.md). `CallbackScope::read_only` (`crates/storage/src/lib.rs:379-387`) sets `query_only` and installs `deny_scope_escapes` (`crates/storage/src/lib.rs:464-508`); `GuardedConn` (`crates/storage/src/lib.rs:219-331`) omits the escapes; `unfenced_connection_rejects_writes` (`crates/storage/src/lib.rs:1769-1792`), `a_read_callback_cannot_clear_the_read_only_guard` (`crates/storage/src/lib.rs:1895-1943`), and `maintenance_runs_through_the_unfenced_path` (`crates/storage/src/lib.rs:2155-2168`) pin the three denials.
Existing check: `unfenced_connection_rejects_writes`, `a_read_callback_cannot_clear_the_read_only_guard`, `maintenance_runs_through_the_unfenced_path`; audited at U2.
Impact: A consumer mutation bypasses the fence and commits while a newer writer owns the database.
Open questions: None.

### callback-scope-is-restored-after-unwind

Type: safety
Reachability: default-production - any callback can panic.
Status: active
Exercised: yes - a panicking read callback followed by a fenced write and a maintenance statement on the same connection.
Guarantee: When a guarded callback unwinds, the connection it ran on is returned to its pre-callback state: `query_only` off and no authorizer installed, so later fenced writes and maintenance succeed.
Check: `always` - `always(scope_dropped ⇒ query_only == OFF && authorizer == None)` for every `CallbackScope`, including the unwinding path.
Fault/timing angle: A poisoned mutex is recovered and hands the same connection to the next caller, so a leaked `query_only` would strand every later write.
Required faults and enabling state: A callback that panics inside `with_conn`, then a `with_conn_fenced` and a `with_conn_unfenced` on the same store.
Confidence: high - [evidence](evidence/callback-scope-is-restored-after-unwind.md). `CallbackScope::drop` (`crates/storage/src/lib.rs:456-461`) restores when `release` did not run; `a_panicking_read_does_not_strand_the_connection_read_only` (`crates/storage/src/lib.rs:1805-1825`) exercises the unwind.
Existing check: `a_panicking_read_does_not_strand_the_connection_read_only`; audited at U2.
Impact: One panicking read makes the store permanently read-only for the rest of the process lifetime.
Open questions: None.

### protected-transactions-pin-fence-durability

Type: safety
Reachability: default-production - every fenced write, including fenced DDL, re-pins the journal settings.
Status: active
Exercised: yes - open, a read callback that is denied lowering `synchronous`, and a fenced write plus a fenced schema change that re-pin `FULL` and WAL after the maintenance path changed them.
Guarantee: Every fence-checked transaction runs with `synchronous = FULL` and a WAL journal, whatever the maintenance path set between transactions.
Check: `always` - `always(synchronous == FULL && journal_mode == wal)` at the start of `open_sqlite`'s fence transaction and every `with_conn_fenced` transaction.
Fault/timing angle: With WAL and `synchronous = NORMAL`, power loss can roll back a committed fence claim, which reissues a superseded epoch.
Required faults and enabling state: A maintenance callback that lowers `synchronous` or changes the journal mode, followed by a fenced write.
Confidence: high - [evidence](evidence/protected-transactions-pin-fence-durability.md). `pin_fence_durability` (`crates/storage/src/lib.rs:572-587`) runs before each protected transaction; `open_pins_full_synchronous` (`crates/storage/src/lib.rs:1794-1803`) and `a_read_callback_cannot_lower_fence_durability` (`crates/storage/src/lib.rs:1827-1893`) observe the pinned values. Power loss itself is not injected.
Existing check: `open_pins_full_synchronous`, `a_read_callback_cannot_lower_fence_durability`; audited at U2.
Impact: A fence claim that looked committed disappears after power loss and two writers hold equal epochs.
Open questions: None.

### store-files-are-owner-only-after-open

Type: safety
Reachability: default-production - every `open_sqlite` hardens the database and its sidecars on Unix.
Status: active
Exercised: yes - a reopen of a database and leftover WAL and SHM files set to `0644`, and a fresh open under umask `022`.
Guarantee: After `open_sqlite` returns on Unix, the database file and any existing `-wal` and `-shm` sidecars have mode `0600`.
Check: `always` - `always(mode(path) & 0o777 == 0o600)` for each of `path`, `path-wal`, and `path-shm` that exists after open.
Fault/timing angle: The database file is created owner-only before SQLite opens it and pre-existing files are narrowed before WAL setup and the fence write; WAL and SHM files created later inherit the database mode.
Required faults and enabling state: A pre-existing permissive database, WAL, and SHM, then a reopen; a fresh open under a permissive umask.
Confidence: high - [evidence](evidence/store-files-are-owner-only-after-open.md). `open_sqlite` (`crates/storage/src/lib.rs:589-693`) creates the database owner-only and calls `protect_file` on all three paths before WAL setup; `reopening_a_permissive_store_protects_the_database_and_its_wal` (`crates/storage/src/lib.rs:1225-1275`) asserts all three modes on reopen and `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask` (`crates/storage/src/lib.rs:1677-1705`) asserts them on a fresh open.
Existing check: `reopening_a_permissive_store_protects_the_database_and_its_wal`, `new_database_file_is_owner_only_at_creation`, `fresh_open_creates_owner_only_sidecars_under_a_permissive_umask` (Unix only); audited at U2.
Impact: Another local user can read committed rows from the WAL or forge the fence row.
Open questions: None.

### fenced-write-is-atomic

Type: safety
Reachability: default-production - every fenced write commits through one `IMMEDIATE` transaction.
Status: active
Exercised: yes - a callback that mutates and then returns an error, on a store whose epoch is ahead of the stored fence.
Guarantee: A `with_conn_fenced` callback that returns an error leaves the database as it was before the call, including the fence row the transaction would have advanced.
Check: `always` - `always(callback_returns_err ⇒ database_after == database_before)` for the domain rows and the fence row.
Fault/timing angle: The fence claim and the callback run in one transaction, so a rolled-back callback must also roll back the claim.
Required faults and enabling state: A holder epoch above the stored fence, a callback that inserts a row and returns `Err`.
Confidence: high - [evidence](evidence/fenced-write-is-atomic.md). `with_conn_fenced` (`crates/storage/src/lib.rs:170-216`) claims and runs inside one transaction and commits only on `Ok`; `fenced_write_rolls_back_on_error` (`crates/storage/src/lib.rs:2170-2197`) checks both the row and the fence.
Existing check: `fenced_write_rolls_back_on_error`, `fenced_write_commits_and_persists` (`crates/storage/src/lib.rs:1752-1767`); audited at U2.
Impact: A failed write leaves half of a domain mutation committed, or advances the fence without the write it authorized.
Open questions: None.

### fence-epoch-outside-sqlite-range-fails-closed

Type: safety
Reachability: default-production - every fence read decodes the stored integer.
Status: active
Exercised: yes - a negative stored epoch on open and a holder epoch above `i64::MAX` on a fenced write.
Guarantee: A stored fence epoch below zero refuses the open with `FenceCorrupt`, and a holder epoch above `i64::MAX` refuses the write before any database access; neither wraps into a valid epoch.
Check: `always` - `always(stored < 0 ⇒ open == Err(FenceCorrupt))` and `always(holder > i64::MAX ⇒ fenced_write == Err(Backend) && database_unchanged)`.
Fault/timing angle: A wrapped conversion would turn a corrupt negative row into a huge valid epoch and let any writer pass.
Required faults and enabling state: A database whose fence row was written without the `CHECK (epoch >= 0)` constraint, and a store constructed with a holder epoch above `i64::MAX`.
Confidence: high - [evidence](evidence/fence-epoch-outside-sqlite-range-fails-closed.md). `decode_fence_epoch` (`crates/storage/src/lib.rs:1000-1003`) and `fence_epoch_sql_value` (`crates/storage/src/lib.rs:961-968`) use checked conversions; `negative_database_fence_fails_closed` (`crates/storage/src/lib.rs:2199-2227`) and `epoch_above_sqlite_integer_range_fails` (`crates/storage/src/lib.rs:2307-2323`) cover both directions.
Existing check: `negative_database_fence_fails_closed`, `epoch_above_sqlite_integer_range_fails`; audited at U2.
Impact: A corrupt fence row authorizes a superseded writer.
Open questions: None.

## Handoff list

Active records go to `/testing:test-strategy` for test-form, oracle, and
boundary selection. Additional routing:

| Property | Additional handoff |
| --- | --- |
| `at-most-one-exclusive-holder-per-key` | `/testing:test-strategy` for a real multi-process barrier race; a simulation must not replace the kernel lock under test |
| `shared-exclusive-exclusion-matrix` | `/testing:invariant-test-review` for T9-T12 |
| `dead-holder-lease-is-reclaimable` | `/testing:crash-consistency-and-failpoint-testing` for non-unwinding termination and restart |
| `writer-epoch-strictly-increases` | `/testing:crash-consistency-and-failpoint-testing`; `/testing:invariant-test-review` for T5/T13 |
| `returned-epoch-is-crash-durable` | `/testing:crash-consistency-and-failpoint-testing` for power-loss and crash-image evidence |
| `failed-acquire-preserves-prior-epoch` | `/testing:crash-consistency-and-failpoint-testing` for real `File` errors and process interruption beyond the injected ordered-prefix model |
| `distinct-lease-keys-do-not-alias` | `/testing:invariant-test-review` for T6-T8 and the separator rejection test; FNV collision handling needs an interface decision before an expected-green test |
| `lease-inode-remains-stable-while-held` | `/testing:test-strategy` for a real two-process replacement ordering test |
| `shared-epoch-never-authorizes-write` | `/testing:test-strategy` at the consumer boundary; interface design is a separate follow-up |
| `permission-hardening-never-follows-replacement` | `/testing:invariant-test-review` for descriptor-relative hardening and the static symlink test |
| `filesystem-lock-scope-matches-deployment` | `/operational-resilience:production-readiness-review`; needs deployment mount and host evidence |
| `lease-file-growth-trigger-is-observed` | `/operational-resilience:production-readiness-review` for watcher and inode-headroom evidence |
| `lease-path-format-is-version-stable` | cross-version overlap policy is a release decision; SemVer tooling is a separate follow-up |
| `stale-writer-write-is-rejected` | `/testing:test-strategy` for real handover histories |
| `logical-store-has-single-lease-identity` | `/testing:test-strategy` at the descriptor-to-store boundary |
| `failed-acquisition-does-not-mutate-lease-state` | `/testing:test-strategy`; production enforcement routes to defensive assertions |
| `replacement-fence-is-claimed-before-old-writer-writes` | `/testing:test-strategy` at the SQLite handover boundary |
| `protected-write-set-is-fence-complete` | `/testing:test-strategy` for a source-level write-site inventory of every consumer |
| `lease-file-creation-is-never-permissive` | `/testing:test-strategy` for a real creation-window observer |
| `acquisition-does-not-follow-symlink` | `/testing:test-strategy` for non-Unix behavior |
| `epoch-update-interruption-window-is-reached` | `/testing:crash-consistency-and-failpoint-testing` |
| Other reachability records | `/testing:test-strategy` for real process and filesystem scheduling |
| `protected-transactions-pin-fence-durability` | `/testing:crash-consistency-and-failpoint-testing` for the power-loss window the record names but does not inject |

Records whose `Existing check` says **unaudited** keep that status until
`/testing:invariant-test-review` returns a verdict; the ones marked **audited
at U2** carry that verdict in their evidence files.
