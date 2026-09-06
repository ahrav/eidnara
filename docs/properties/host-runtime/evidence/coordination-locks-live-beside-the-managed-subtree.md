# coordination-locks-live-beside-the-managed-subtree

## Discovery trigger

The coordination directory was renamed at U3 to carry the product name. The
doc comment at `lifecycle.rs:52-53` states the design rule: the lock files
live outside the managed `eidnara` subtree so that replacing that subtree
cannot split the locks. The threat is a lock file inside the replaceable
tree, where a rename of `<root>/eidnara` would leave a live holder flocking
an orphaned inode while a successor creates a fresh lock at the same path and
acquires it. The audit traced both lock files from their constants to the
production acquire path and to the tests that pin the inode identity.

## Evidence trail

All references are at `572315a`.

Constants and path. `COORDINATION_DIR_NAME` is `.eidnara-coordination`
(`lifecycle.rs:33`), `TRANSACTION_LOCK_NAME` is `transaction.lock` (`:36`),
and `LIFETIME_LOCK_NAME` is `lifetime.lock` (`:39`). `coordination_dir_path`
(`:54-56`) joins the directory name to `data_dir_path`, the data root, not to
`managed_dir_path`. The managed subtree is `<root>/eidnara`
(`instance.rs:170`, `:178-180`), so the coordination directory is a hidden
sibling of it. `lifecycle_dir_path` (`lifecycle.rs:59-66`) is documented as
carrying no locks.

Creation. `open_coordination_lock_create` (`:71-85`) secures the
coordination directory with `secure_runtime_dir` (`:76`) and then
materializes both lock files on every call (`:78-83`), returning the
descriptor for the requested name. `create_validated_lock_file` (`:88-123`)
opens with `O_CREAT | O_NOFOLLOW | O_NONBLOCK`, rejects `ELOOP` and `ENOTDIR`
as `Insecure`, requires a regular owner-only single-link file
(`:110-119`), and normalizes the mode to 0600 on the descriptor (`:120-121`).

Lifetime lock. `LifetimeLock::acquire` (`:181-189`) calls
`open_coordination_lock_create` with `LIFETIME_LOCK_NAME` and takes a
non-blocking exclusive flock, mapping `EWOULDBLOCK` to `AlreadyRunning`.
`InstanceGuard::acquire` (`instance.rs:231-244`) takes this lock at `:244`
before resolving the runtime directory and before `lock_instance`. The host
entry point calls `InstanceGuard::acquire` at `runtime.rs:565-568` inside a
bounded retry loop (`:564-580`).

Transaction lock. `LifecycleTransactionLock::acquire_exclusive`
(`lifecycle.rs:456-460`) calls `open_coordination_lock_create` with
`TRANSACTION_LOCK_NAME` and `flock_exclusive_bounded`. The shared form
`acquire_shared` (`:470-486`) never creates the file and is called by
`probe_lifecycle` at `:873`. The type is exported from `lib.rs:70`.

Existing checks, verified. Unit tests in `lifecycle.rs` run under
`cargo test --workspace --all-targets` (`.github/workflows/ci.yml:118`):

- `independent_openers_see_one_stable_coordination_identity`
  (`lifecycle.rs:2001-2029`) spells `.eidnara-coordination/transaction.lock`
  as a literal (`:2004-2007`), acquires the transaction lock, records
  `(dev, ino)` (`:2011-2013`), drops it, creates `<root>/eidnara/run`, renames
  `eidnara` to `eidnara-old` (`:2017-2019`), reacquires, and asserts the same
  identity (`:2023-2028`).
- `a_replaced_lifecycle_child_cannot_mint_a_second_transaction_owner`
  (`:1981-1998`) holds the transaction lock, renames the managed `lifecycle`
  directory, and asserts a second `acquire_exclusive` is `AlreadyRunning`.
- `a_replaced_eidnara_subtree_is_not_reported_stopped_while_the_daemon_lives`
  (`:2032-2071`) holds an `InstanceGuard`, renames `eidnara` away, asserts the
  probe reports `Wedged` (`:2053`), and asserts `InstanceGuard::acquire` is
  `AlreadyRunning` (`:2056-2062`).
- `a_replaced_eidnara_subtree_cannot_admit_an_overlapping_incarnation`
  (`tests/lifecycle.rs:1739-1795`) does the same through a real `TestHost`,
  asserting `!observed.lifetime_lock_free` (`:1766`) and
  `HostError::Instance(AlreadyRunning)` for the blocked successor
  (`:1773-1781`).
- `successive_incarnations_lock_the_same_coordination_inodes`
  (`tests/lifecycle.rs:1798-1831`) derives the path from the exported
  constants (`:1802-1805`), so it does not independently pin the spelling,
  but it does pin the lifetime lock's `(dev, ino)` across two host starts.
- `hostile_shapes_at_the_lock_names_fail_closed` (`lifecycle.rs:1635`)
  iterates both lock names (`:1638`) against planted symlinks and FIFOs.

The registry family `host-locks` (`migration/registry.json:311-325`) records
the same two paths under `$XDG_DATA_HOME/.eidnara-coordination/`.

## Failure scenario

1. A lock file at `<root>/eidnara/run/lifetime.lock` fences a live daemon.
2. An operator or installer renames `<root>/eidnara` to stage a replacement.
3. A successor resolves the same path, finds no file, creates one, and takes
   the flock. Two hosts now each hold a fence on distinct inodes.

As written, both lock names resolve under `<root>/.eidnara-coordination`,
which no supported code renames (`:166`, `:424`). The rename in step 2 does
not move the lock inode, so the successor's flock contends with the holder.

## Timing windows and dependencies

The rename in the identity test happens between two acquisitions, after the
first lock is dropped (`:2014`). The subtree tests hold the lock across the
rename. Neither exercises a rename of `.eidnara-coordination` itself; the doc
at `:172` and `:193-194` names that as unsupported and as a fence split.

The exclusion holds only among coordination-aware releases. The comment at
`:435-438` states that a release predating `transaction.lock` serializes on
the `eidnara/lifecycle` directory inode instead, so its transactions do not
contend with one taken here.

## What a test must construct

The property as stated is covered for the transaction lock by the identity
test and for the lifetime lock by the two replaced-subtree tests plus the
inode test at `tests/lifecycle.rs:1798`. A version of the identity test for
`lifetime.lock` that spells the path as a literal, rather than through the
exported constants, would remove the one remaining dependence on the crate's
own spelling.

## Investigation log

### Q: Which lock does the production path exercise?

- Sources examined: `runtime.rs:564-580`; `instance.rs:231-244`;
  `lifecycle.rs:78-83`, `:181-189`, `:456-460`, `:470-486`, `:873`;
  `lib.rs:70`; `docs/properties/README.md:52`; a grep for
  `acquire_exclusive` across `crates/host-runtime/src` and `tests`.
- Findings: `InstanceGuard::acquire` takes the lifetime lock on every
  incarnation (`instance.rs:244`, via `runtime.rs:565`).
  `LifecycleTransactionLock::acquire_exclusive` has no caller outside
  `lifecycle.rs` unit tests in this tree; the daemon that mutates the
  lifecycle namespace is scheduled for U4 (`docs/properties/README.md:52`).
  The transaction lock file is still created on every incarnation, because
  `open_coordination_lock_create` materializes both names (`:78-83`), and it
  is shared-locked by `probe_lifecycle` (`:873`). The identity test asserts
  the transaction lock's `(dev, ino)`, which is the lock production does not
  exclusively acquire in this tree.
- Missing evidence: a production caller of `acquire_exclusive`.
- Conclusion: resolved with a correction to the record. The record's
  reachability note says every incarnation takes both locks; in this tree
  every incarnation takes the lifetime lock and creates the transaction lock
  file, but only tests take the transaction lock exclusively. The path
  property holds for both names because both go through
  `coordination_dir_path`, and the lifetime lock's identity is pinned by
  `successive_incarnations_lock_the_same_coordination_inodes`.

### Q: How does the cutover isolation probe treat this directory?

- Sources examined: `migration/registry.json:311-325`; `lifecycle.rs:33`.
- Findings: the directory is a hidden sibling of the managed subtree, outside
  `<root>/eidnara`. The predecessor's coordination directory carried a
  different name. The registry entry lists the current paths with
  `mismatch_behavior: recreate`.
- Missing evidence: the predecessor directory name and whether the cutover
  probe digests the two directories separately.
- Conclusion: needs human input. The probe must digest the current
  coordination directory separately from the predecessor's; this tree does
  not record the predecessor name.
