use context_core::claim_operation::is_lower_hex;
use lease::{FileLeaseStore, HeldFileLease, LeaseError, LeaseKey, protect_file};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError, TryLockError};
use std::time::Instant;

use super::schema::{
    KERNEL_APPLICATION_ID, KERNEL_FORMAT_EPOCH, apply_kernel_schema, kernel_schema_digest,
    kernel_schema_object_inventory,
};
use crate::current_time_ms;
use crate::sqlite_runtime::{
    SqliteEngineIdentity, compute_marker_digest, evaluate_sqlite_runtime_gate,
    probe_sqlite_engine_identity_off_path,
};

const BUSY_TIMEOUT_MS: i64 = 5_000;
const PREPARED_STATEMENT_CACHE_CAPACITY: usize = 128;
const READ_POOL_SIZE: usize = 2;
const WRITER_ACQUIRE_POLL: std::time::Duration = std::time::Duration::from_millis(1);
const RESTORE_MARKER_SUFFIX: &str = ".restore";
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Redacted failure classes returned by kernel-store operations.
///
/// Variants intentionally omit paths, SQL text, and underlying error details.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KernelError {
    #[error("kernel store is held by another writer")]
    Held,
    #[error("SQLite engine is unsupported for the kernel store")]
    EngineUnsupported,
    /// The file is not SQLite, or its `application_id` is not the Eidnara id.
    /// Every Eidnara store shares that id, so a sibling family such as the
    /// memory store passes this check and is refused as `Inconclusive` when its
    /// schema lacks `kernel_format_marker`.
    #[error("kernel store path contains a foreign database family")]
    Foreign,
    #[error("kernel store identity could not be established safely")]
    Inconclusive,
    #[error("kernel store I/O failed")]
    Io,
    #[error("kernel store lock was not acquired before the busy timeout")]
    Busy,
    #[error("kernel store identity does not match this build")]
    IdentityMismatch,
    #[error("kernel store writer fence was lost")]
    FenceLost,
    #[error("kernel operation conflicts with an existing receipt")]
    Conflict,
    #[error("kernel canonical row is corrupt or missing")]
    CorruptCanonicalRow,
    #[error("kernel operation input is invalid")]
    InvalidInput,
    #[error("kernel admission classification or transition is invalid")]
    AdmissionPolicy,
    /// A preview refuses a later admission that depends on authority an earlier admission in the same preview changed. A commit runs the authority cascade over that change before judging the later admission; the preview does not simulate the cascade, so it refuses rather than judge against authority the commit would already have revised. commentlint: allow(JUDGE)
    #[error("kernel preview rests on an authority an earlier previewed admission changed")]
    PreviewAuthorityChanged,
    #[error("kernel snapshot is newer than the committed tip")]
    FutureSnapshot,
    #[error("kernel object was not found")]
    NotFound,
    #[error("outbox checkpoint is invalid")]
    InvalidCheckpoint,
    #[error("outbox pruning requires at least one consumer")]
    NoRequiredConsumers,
    #[error("outbox consumer has not reached the commit-log tip")]
    ConsumerPending,
    #[error("kernel operation hit an injected fault")]
    Fault,
    #[error("kernel operation exceeded its deadline")]
    Deadline,
    #[error("backup destination is not a private local directory")]
    UnsafeDestination,
    #[error("backup artifact failed verification")]
    InvalidBackup,
    #[error("restore source failed verification")]
    InvalidRestore,
}

impl KernelError {
    /// Lists every variant so tests can verify exhaustive error partitions.
    #[cfg(any(test, feature = "test-support"))]
    pub const ALL: &'static [KernelError] = &[
        Self::Held,
        Self::EngineUnsupported,
        Self::Foreign,
        Self::Inconclusive,
        Self::Io,
        Self::Busy,
        Self::IdentityMismatch,
        Self::FenceLost,
        Self::Conflict,
        Self::CorruptCanonicalRow,
        Self::InvalidInput,
        Self::AdmissionPolicy,
        Self::PreviewAuthorityChanged,
        Self::FutureSnapshot,
        Self::NotFound,
        Self::InvalidCheckpoint,
        Self::NoRequiredConsumers,
        Self::ConsumerPending,
        Self::Fault,
        Self::Deadline,
        Self::UnsafeDestination,
        Self::InvalidBackup,
        Self::InvalidRestore,
    ];

    /// Returns true only when retrying the unchanged request is valid.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Busy | Self::Held)
    }
}

impl fmt::Debug for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Exclusive writer lease plus pooled SQLite connections for one kernel family.
///
/// One process owns the writer lease.
/// Mutations serialize through `writer`, while reads rotate across a fixed query-only pool.
/// A restore that cannot recover the original family poisons both paths until the store is reopened.
/// A restore rejected before displacement, or one whose original family is recovered, leaves both paths usable.
pub struct KernelStore {
    writer: Mutex<Connection>,
    pub(super) purge_intent_log: Mutex<File>,
    pub(super) readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
    // Distinct from mutex poisoning, which `PoisonError::into_inner` recovers: this
    // records that an unrecoverable restore left the family unusable.
    poisoned: AtomicBool,
    pub(super) cas_failed: AtomicBool,
    pub(super) artifact_cap: u64,
    /// The store root and the artifact tree's `objects` and `tmp` children,
    /// opened `NOFOLLOW` when the store opened and held for its lifetime. Every
    /// later directory open resolves below one of them rather than re-resolving
    /// a pathname a same-UID process could have swapped.
    pub(super) root_directory: File,
    pub(super) objects_directory: File,
    pub(super) tmp_directory: File,
    /// Shard directories below `objects`, each opened `NOFOLLOW` on first use
    /// and held for the store's lifetime. Reclamation never removes a shard, so
    /// a held descriptor never goes stale, and every publication, read, and
    /// unlink of a digest resolves through the same directory.
    pub(super) shard_directories: Mutex<std::collections::BTreeMap<String, std::sync::Arc<File>>>,
    lease_epoch: u64,
    /// Advances when an artifact's stored classification changes without a
    /// commit-log row, so a reader keyed on the tip alone can still tell that
    /// its egress facts are stale.
    pub(super) classification_generation: AtomicU64,
    pub(super) db_path: PathBuf,
    // Fields drop in declaration order, so `_lease` must stay last: it releases
    // the file lock only after every connection field above it has closed.
    _lease: HeldFileLease,
}

impl fmt::Debug for KernelStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelStore")
            .field("lease_epoch", &self.lease_epoch)
            .field("read_pool_size", &self.readers.len())
            .finish_non_exhaustive()
    }
}

#[must_use = "dropping the guard immediately closes the window before the change runs"]
pub(super) struct ClassificationChange<'a> {
    generation: &'a AtomicU64,
}

impl Drop for ClassificationChange<'_> {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl KernelStore {
    /// Opens, validates, or bootstraps the kernel store below `root`.
    ///
    /// Opening acquires the exclusive writer lease, resumes interrupted restore
    /// work, validates SQLite identity, activates WAL, stamps the writer fence,
    /// and runs artifact recovery. Failures use redacted [`KernelError`]
    /// classes. A structurally valid kernel family with a different build
    /// identity is refused with [`KernelError::IdentityMismatch`]; foreign or
    /// inconclusive content is left untouched.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, KernelError> {
        let identity =
            probe_sqlite_engine_identity_off_path().map_err(|_| KernelError::EngineUnsupported)?;
        Self::open_with_engine_identity_and_cap(root, &identity, super::cas::DEFAULT_ARTIFACT_CAP)
    }

    fn open_with_engine_identity_and_cap(
        root: impl AsRef<Path>,
        identity: &SqliteEngineIdentity,
        artifact_cap: u64,
    ) -> Result<Self, KernelError> {
        if !evaluate_sqlite_runtime_gate(identity).is_empty() {
            return Err(KernelError::EngineUnsupported);
        }
        Self::open_supported(root, artifact_cap, None)
    }

    /// Opens with `hook` run at each [`OpenPhase`], the points at which a
    /// pathname-resolved open could be pointed at another directory.
    #[cfg(feature = "test-support")]
    pub fn open_with_hook_for_test(
        root: impl AsRef<Path>,
        mut hook: impl FnMut(OpenPhase),
    ) -> Result<Self, KernelError> {
        let identity =
            probe_sqlite_engine_identity_off_path().map_err(|_| KernelError::EngineUnsupported)?;
        if !evaluate_sqlite_runtime_gate(&identity).is_empty() {
            return Err(KernelError::EngineUnsupported);
        }
        Self::open_supported(root, super::cas::DEFAULT_ARTIFACT_CAP, Some(&mut hook))
    }

    #[cfg(feature = "test-support")]
    pub fn open_with_engine_identity_for_test(
        root: impl AsRef<Path>,
        identity: &SqliteEngineIdentity,
    ) -> Result<Self, KernelError> {
        Self::open_with_engine_identity_and_cap(root, identity, super::cas::DEFAULT_ARTIFACT_CAP)
    }

    #[cfg(feature = "test-support")]
    pub fn open_with_artifact_cap_for_test(
        root: impl AsRef<Path>,
        artifact_cap: u64,
    ) -> Result<Self, KernelError> {
        let identity =
            probe_sqlite_engine_identity_off_path().map_err(|_| KernelError::EngineUnsupported)?;
        Self::open_with_engine_identity_and_cap(root, &identity, artifact_cap)
    }

    fn open_supported(
        root: impl AsRef<Path>,
        artifact_cap: u64,
        mut hook: Option<&mut dyn FnMut(OpenPhase)>,
    ) -> Result<Self, KernelError> {
        // The lease, the layout, and every SQLite connection below resolve `root` by
        // pathname. Holding the directory open from the moment its mode was set lets
        // the end of the open prove that all of them resolved the same directory.
        let (root, root_directory) = prepare_root(root.as_ref())?;
        let db_path = root.join("kernel.sqlite");
        let lease_store = FileLeaseStore::new(root.join("leases")).map_err(|_| KernelError::Io)?;
        let lease_key = LeaseKey::new("eidnara-kernel", "sqlite", "kernel");
        let mut lease = lease_store.acquire(&lease_key).map_err(map_lease_error)?;
        // The lease was taken by pathname. It fences the database of the directory
        // that pathname named at the time; if that is no longer the held root, the
        // lease covers nothing this open is about to touch.
        assert_root_unchanged(&root_directory, &root)?;
        let artifact_directories = super::cas::prepare_layout(&root_directory)?;

        let marker_name = restore_marker_path(&db_path)
            .file_name()
            .ok_or(KernelError::Io)?
            .to_os_string();
        let marker_present = match rustix::fs::statat(
            &root_directory,
            &marker_name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => true,
            Err(rustix::io::Errno::NOENT) => false,
            Err(_) => return Err(KernelError::Io),
        };
        if marker_present {
            let root = root_directory.try_clone().map_err(|_| KernelError::Io)?;
            super::backup::resume_restore(&db_path, root)?;
        } else {
            super::backup::reap_orphan_restore_recovery(&db_path, &root_directory)?;
        }

        // The database is opened by pathname below, so the root is checked once
        // more before the pathname is used for anything that could write.
        assert_root_unchanged(&root_directory, &root)?;
        let header = inspect_header(&db_path)?;
        if let Some(hook) = hook.as_mut() {
            hook(OpenPhase::BeforeDatabaseOpen);
        }
        let mut writer = match header {
            HeaderState::Pristine => bootstrap(
                &root_directory,
                &db_path,
                hook.as_mut()
                    .map(|hook| &mut **hook as &mut dyn FnMut(OpenPhase)),
            )?,
            HeaderState::Kernel => match classify_existing_family(&db_path)? {
                OpenIdentity::Exact => {
                    let conn = open_writer(&db_path).map_err(|_| KernelError::Inconclusive)?;
                    apply_preclassification_profile(&conn)
                        .map_err(|_| KernelError::Inconclusive)?;
                    conn
                }
                OpenIdentity::Mismatch => return Err(KernelError::IdentityMismatch),
            },
        };
        if let Some(hook) = hook.as_mut() {
            hook(OpenPhase::BeforeFirstWrite);
        }
        // SQLite resolved `db_path` by name. Before the first write through the
        // connection, the file that pathname reaches is compared with the
        // `kernel.sqlite` entry of the held root: a root renamed and replaced
        // since the lease was taken would have handed SQLite the replacement, and
        // stamping that with this store's epoch would fence a database the lease
        // does not cover and rewrite rows no lease of ours protects.
        super::backup::assert_same_file(
            &root_directory,
            std::ffi::OsStr::new("kernel.sqlite"),
            &db_path,
            KernelError::Io,
        )?;

        activate_wal(&writer)?;
        // The lease directory is a mutable child of the root, so its epochs alone
        // do not prove exclusivity: a fresh `leases` directory swapped in beside a
        // running writer issues epochs from one again. The durable fence is the
        // record that survives that. A lease whose epoch does not exceed the fence
        // the database already carries is not the lease that last fenced it, so it
        // is re-acquired above that fence; the stamp then only ever raises the
        // fence, and a database another writer has fenced higher since is refused.
        let durable = durable_fence_epoch(&writer)?;
        if lease.epoch() <= durable {
            drop(lease);
            lease = lease_store
                .acquire_above(&lease_key, durable)
                .map_err(map_lease_error)?;
        }
        let lease_epoch = lease.epoch();
        raise_writer_fence(&mut writer, lease_epoch)?;
        // A store written by the parent build retains a digest of pre-redaction
        // candidate input, so it is rewritten before the store is handed out.
        super::envelope::strip_legacy_candidate_verifiers(&mut writer, lease_epoch)?;
        // Two hardening passes are required because the family grows between
        // them. WAL activation creates `-wal` and `-shm` under the process umask,
        // so the first pass restricts them as early as possible; a read-only open
        // recreates `-shm` when it is missing, so the second pass restricts that
        // one too.
        harden_family(&db_path)?;
        let readers = open_read_pool(&db_path)?;
        harden_family(&db_path)?;
        let purge_intent_log =
            super::durable_fs::open_or_create_append_file(&root_directory, "purge-intent.jsonl")
                .map_err(|_| KernelError::Io)?;
        // A root renamed away and replaced after the lease was acquired would leave
        // the connections above inside the replacement while the held lease still
        // fences the original, so a second opener could take the replacement's
        // lease and write the same database under another epoch. Refusing the open
        // when the root no longer resolves to the held directory closes that window.
        assert_root_unchanged(&root_directory, &root)?;

        let store = Self {
            writer: Mutex::new(writer),
            purge_intent_log: Mutex::new(purge_intent_log),
            readers,
            next_reader: AtomicUsize::new(0),
            poisoned: AtomicBool::new(false),
            cas_failed: AtomicBool::new(false),
            artifact_cap,
            root_directory,
            objects_directory: artifact_directories.objects,
            tmp_directory: artifact_directories.tmp,
            shard_directories: Mutex::new(std::collections::BTreeMap::new()),
            lease_epoch,
            classification_generation: AtomicU64::new(0),
            db_path,
            _lease: lease,
        };
        // A purge unlink that cannot complete keeps its pending row for maintenance
        // to retry; refusing to open over it would take the whole store offline
        // for one object, so the count is not an open failure.
        store.recover_interrupted_work()?;
        Ok(store)
    }

    /// Finishes work an earlier process left behind in the database this store
    /// now serves: expired staging leases, abandoned ingestion reservations, and
    /// purges that committed but never unlinked their bytes. Runs when a store
    /// opens and again after a restore installs a different database, since the
    /// restored history carries its own interrupted work. Returns how many
    /// pending purge unlinks could not be completed.
    ///
    /// Reclaiming an expired lease keeps every row; deleting aged runs is left to an
    /// explicit call, so opening a store is not a destructive act.
    pub(super) fn recover_interrupted_work(&self) -> Result<usize, KernelError> {
        self.abandon_expired_staging_runs(crate::current_time_ms())?;
        self.run_artifact_recovery(crate::current_time_ms())
    }

    /// Returns the lease epoch stamped into writer-fence transactions.
    pub fn lease_epoch(&self) -> u64 {
        self.lease_epoch
    }

    /// A panic in a caller's closure drops the guard mid-unwind and poisons the
    /// mutex, so recovering the guard keeps one caught panic from disabling every
    /// later write. Dropping a rusqlite `Transaction` rolls it back, so the
    /// recovered connection has no in-flight statement.
    pub(super) fn lock_writer(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KernelError> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(KernelError::InvalidRestore);
        }
        let writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if self.poisoned.load(Ordering::Acquire) {
            return Err(KernelError::InvalidRestore);
        }
        Ok(writer)
    }

    /// `Mutex::lock` has no timeout, so a bounded caller polls instead of
    /// blocking behind an operation that may outlive its own budget.
    pub(crate) fn lock_writer_within(
        &self,
        limit: &AcquireLimit,
    ) -> Result<std::sync::MutexGuard<'_, Connection>, KernelError> {
        self.acquire_within(std::slice::from_ref(&self.writer), 0, limit)
    }

    /// Marks all later connection acquisition as an invalid restore.
    pub(super) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    /// Reader counterpart of [`Self::lock_writer_within`], over the whole pool:
    /// one long-running reader does not decide the outcome.
    pub(crate) fn lock_reader_within(
        &self,
        limit: &AcquireLimit,
    ) -> Result<std::sync::MutexGuard<'_, Connection>, KernelError> {
        let start = self.next_reader.fetch_add(1, Ordering::Relaxed);
        self.acquire_within(&self.readers, start, limit)
    }

    /// Opens a window in which stored artifact classification may change.
    ///
    /// The generation is odd while the returned guard lives and even again
    /// once it drops, whether or not the change succeeded, so a reader that
    /// observed the same even value on both sides of its snapshot knows the
    /// classification it read was not changing underneath it.
    pub(super) fn begin_classification_change(&self) -> ClassificationChange<'_> {
        self.classification_generation
            .fetch_add(1, Ordering::SeqCst);
        ClassificationChange {
            generation: &self.classification_generation,
        }
    }

    /// Polls `candidates` from `start` until one is free or `limit` says stop,
    /// so every bounded acquisition shares one poison-check order and one
    /// backoff.
    fn acquire_within<'pool>(
        &self,
        candidates: &'pool [Mutex<Connection>],
        start: usize,
        limit: &AcquireLimit,
    ) -> Result<std::sync::MutexGuard<'pool, Connection>, KernelError> {
        loop {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(KernelError::InvalidRestore);
            }
            for offset in 0..candidates.len() {
                let guard = match candidates[(start + offset) % candidates.len()].try_lock() {
                    Ok(guard) => guard,
                    Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                    Err(TryLockError::WouldBlock) => continue,
                };
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(KernelError::InvalidRestore);
                }
                return Ok(guard);
            }
            if limit.should_stop() {
                return Err(KernelError::Deadline);
            }
            std::thread::sleep(WRITER_ACQUIRE_POLL);
        }
    }

    pub(super) fn lock_reader(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KernelError> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(KernelError::InvalidRestore);
        }
        let index = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        let reader = self.readers[index]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.poisoned.load(Ordering::Acquire) {
            return Err(KernelError::InvalidRestore);
        }
        Ok(reader)
    }

    /// Holds every reader connection for `duration`, so a deadline-bounded read
    /// path can be observed returning at its own bound. `held` is passed once
    /// every reader is locked, so the caller waiting on it starts the bounded
    /// read against a fully occupied pool.
    #[cfg(feature = "test-support")]
    pub fn hold_readers_for_test(&self, held: &std::sync::Barrier, duration: std::time::Duration) {
        let guards = self
            .readers
            .iter()
            .map(|reader| reader.lock().unwrap_or_else(PoisonError::into_inner))
            .collect::<Vec<_>>();
        held.wait();
        std::thread::sleep(duration);
        drop(guards);
    }

    #[cfg(feature = "test-support")]
    pub fn invalidate_writer_fence_for_test(&self) -> Result<(), KernelError> {
        let writer = self.lock_writer()?;
        writer
            .execute(
                "UPDATE writer_fence SET writer_epoch=writer_epoch+1 WHERE id=0",
                [],
            )
            .map_err(|_| KernelError::Io)?;
        Ok(())
    }

    #[allow(
        dead_code,
        reason = "connection ownership is restricted to kernel mutation modules"
    )]
    pub(crate) fn with_writer<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, KernelError> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // `BEGIN IMMEDIATE` blocks a competing writer until this transaction ends.
        // The fence check and the mutation are therefore atomic.
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        let durable_epoch: i64 = tx
            .query_row(
                "SELECT writer_epoch FROM writer_fence WHERE id=0",
                [],
                |row| row.get(0),
            )
            .map_err(|_| KernelError::FenceLost)?;
        if u64::try_from(durable_epoch).ok() != Some(self.lease_epoch) {
            return Err(KernelError::FenceLost);
        }
        let value = operation(&tx).map_err(|_| KernelError::Io)?;
        tx.commit().map_err(|_| KernelError::Io)?;
        Ok(value)
    }

    #[allow(
        dead_code,
        reason = "connection ownership is restricted to kernel query modules"
    )]
    pub(crate) fn with_reader<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, KernelError> {
        let index = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        // Recovering a poisoned reader guard keeps its pool slot usable.
        // `Transaction::Drop` rolls back after a closure panic.
        let mut reader = self.readers[index]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // After its first read, the transaction keeps one snapshot for the
        // closure. Several reads therefore observe one consistent state.
        let tx = reader.transaction().map_err(|_| KernelError::Io)?;
        let value = operation(&tx).map_err(|_| KernelError::Io)?;
        tx.commit().map_err(|_| KernelError::Io)?;
        Ok(value)
    }
}

enum HeaderState {
    Pristine,
    Kernel,
}

/// `Path::exists` maps every error to `false`; only `NotFound` counts as absence.
fn entry_exists(path: &Path) -> Result<bool, KernelError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(KernelError::Io),
    }
}

/// A zero-length main file has never held a committed page.
///
/// A `-journal` does not prevent treating an empty main file as pristine.
fn classify_empty_family(path: &Path) -> Result<HeaderState, KernelError> {
    if entry_exists(&suffix_path(path, "-wal"))? || entry_exists(&suffix_path(path, "-shm"))? {
        return Err(KernelError::Inconclusive);
    }
    Ok(HeaderState::Pristine)
}

fn inspect_header(path: &Path) -> Result<HeaderState, KernelError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return classify_empty_family(path),
        Err(_) => return Err(KernelError::Io),
    };
    if !metadata.is_file() {
        return Err(KernelError::Inconclusive);
    }
    if metadata.len() == 0 {
        return classify_empty_family(path);
    }
    if metadata.len() < 100 {
        return Err(KernelError::Inconclusive);
    }
    let mut header = [0_u8; 100];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| KernelError::Inconclusive)?;
    if &header[..16] != SQLITE_HEADER {
        return Err(KernelError::Foreign);
    }
    let application_id =
        u32::from_be_bytes(header[68..72].try_into().map_err(|_| KernelError::Io)?);
    if application_id != KERNEL_APPLICATION_ID {
        return Err(KernelError::Foreign);
    }
    Ok(HeaderState::Kernel)
}

struct ExpectedIdentity {
    digest: String,
    inventory: Vec<(String, String)>,
}

static EXPECTED_IDENTITY: LazyLock<Option<ExpectedIdentity>> = LazyLock::new(|| {
    let mut conn = Connection::open_in_memory().ok()?;
    apply_kernel_schema(&mut conn, "00000000000000000000000000000000", 0).ok()?;
    Some(ExpectedIdentity {
        digest: kernel_schema_digest(&conn).ok()?,
        inventory: kernel_schema_object_inventory(&conn).ok()?,
    })
});

fn expected_identity() -> Result<&'static ExpectedIdentity, KernelError> {
    EXPECTED_IDENTITY.as_ref().ok_or(KernelError::Io)
}

/// Classification must leave the database and its `-wal` byte-identical.
///
/// The `Foreign` and `Inconclusive` outcomes promise untouched durable content.
/// Journal recovery and WAL checkpointing would both break that promise.
///
/// A read-only open may recreate a missing `-shm`; it contains no durable data.
/// SQLite rebuilds the `-shm` from the `-wal` on demand.
fn classify_existing_family(path: &Path) -> Result<OpenIdentity, KernelError> {
    let expected = expected_identity()?;
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| KernelError::Inconclusive)?;
    conn.pragma_update(None, "query_only", "ON")
        .map_err(|_| KernelError::Inconclusive)?;
    conn.pragma_update(None, "trusted_schema", "OFF")
        .map_err(|_| KernelError::Inconclusive)?;
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
        .map_err(|_| KernelError::Inconclusive)?;
    classify_open_kernel(&mut conn, expected)
}

enum OpenIdentity {
    Exact,
    Mismatch,
}

struct FormatMarker {
    epoch: i64,
    incarnation: String,
    schema_digest: String,
    created_at: i64,
    marker_digest: String,
}

/// Verifies page integrity, foreign keys, format marker, and schema identity.
///
/// Returns `IdentityMismatch` only for a structurally valid kernel family with
/// different identity. Failed integrity or marker checks are inconclusive.
pub(super) fn verify_exact_identity(conn: &mut Connection) -> Result<(), KernelError> {
    let integrity_check: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| KernelError::Inconclusive)?;
    if integrity_check != "ok" {
        return Err(KernelError::Inconclusive);
    }
    // `integrity_check` validates page structure, not references, so a family
    // written with `foreign_keys=OFF` can pass it while holding dangling rows.
    let foreign_key_violations: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(|_| KernelError::Inconclusive)?;
    if foreign_key_violations != 0 {
        return Err(KernelError::Inconclusive);
    }
    match classify_open_kernel(conn, expected_identity()?)? {
        OpenIdentity::Exact => Ok(()),
        OpenIdentity::Mismatch => Err(KernelError::IdentityMismatch),
    }
}

fn classify_open_kernel(
    conn: &mut Connection,
    expected: &ExpectedIdentity,
) -> Result<OpenIdentity, KernelError> {
    let tx = conn.transaction().map_err(|_| KernelError::Inconclusive)?;
    let quick_check: String = tx
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|_| KernelError::Inconclusive)?;
    if quick_check != "ok" {
        return Err(KernelError::Inconclusive);
    }
    let marker = read_valid_marker(&tx)?;
    let application_id: u32 = tx
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|_| KernelError::Inconclusive)?;
    let user_version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| KernelError::Inconclusive)?;
    let inventory = kernel_schema_object_inventory(&tx).map_err(|_| KernelError::Inconclusive)?;
    let digest = kernel_schema_digest(&tx).map_err(|_| KernelError::Inconclusive)?;
    let exact = application_id == KERNEL_APPLICATION_ID
        && user_version == KERNEL_FORMAT_EPOCH
        && marker.epoch == KERNEL_FORMAT_EPOCH
        && marker.schema_digest == expected.digest
        && digest == expected.digest
        && inventory == expected.inventory;
    tx.commit().map_err(|_| KernelError::Inconclusive)?;
    if exact {
        Ok(OpenIdentity::Exact)
    } else {
        Ok(OpenIdentity::Mismatch)
    }
}

fn read_valid_marker(conn: &Connection) -> Result<FormatMarker, KernelError> {
    let present: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type='table' AND name='kernel_format_marker'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| KernelError::Inconclusive)?;
    if present.is_none() {
        return Err(KernelError::Inconclusive);
    }
    // A lookalike table has no singleton constraint, so `LIMIT 2` detects multiple
    // rows.
    let mut statement = conn
        .prepare(
            "SELECT format_epoch,database_incarnation_id,schema_digest,created_at,marker_digest
             FROM kernel_format_marker
             WHERE length(database_incarnation_id)=32
               AND length(schema_digest)=64
               AND length(marker_digest)=64
             LIMIT 2",
        )
        .map_err(|_| KernelError::Inconclusive)?;
    let rows = statement
        .query_map([], |row| {
            Ok(FormatMarker {
                epoch: row.get(0)?,
                incarnation: row.get(1)?,
                schema_digest: row.get(2)?,
                created_at: row.get(3)?,
                marker_digest: row.get(4)?,
            })
        })
        .map_err(|_| KernelError::Inconclusive)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| KernelError::Inconclusive)?;
    let [marker] = rows.as_slice() else {
        return Err(KernelError::Inconclusive);
    };
    if !is_lower_hex(&marker.incarnation, 32)
        || !is_lower_hex(&marker.schema_digest, 64)
        || !is_lower_hex(&marker.marker_digest, 64)
        || marker.epoch < 1
        || marker.created_at < 0
    {
        return Err(KernelError::Inconclusive);
    }
    let expected_marker_digest = compute_marker_digest(
        marker.epoch,
        &marker.incarnation,
        &marker.schema_digest,
        marker.created_at,
    );
    if marker.marker_digest != expected_marker_digest {
        return Err(KernelError::Inconclusive);
    }
    Ok(FormatMarker {
        epoch: marker.epoch,
        incarnation: marker.incarnation.clone(),
        schema_digest: marker.schema_digest.clone(),
        created_at: marker.created_at,
        marker_digest: marker.marker_digest.clone(),
    })
}

pub(super) fn open_writer(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Creates the database for a pristine root and applies the schema.
///
/// The file is created below the held root descriptor, so it can only land in
/// the directory the lease covers, and the descriptor is held through the
/// bootstrap. SQLite is then opened without the create flag: it must find the
/// file the pathname reaches, and both the root's entry and that pathname are
/// compared with the held descriptor before any schema is written, so neither a
/// root swapped in between nor a file swapped under the same name can receive
/// a database, not even an empty one.
fn bootstrap(
    root_directory: &File,
    path: &Path,
    mut hook: Option<&mut dyn FnMut(OpenPhase)>,
) -> Result<Connection, KernelError> {
    let name = path.file_name().ok_or(KernelError::Io)?;
    let name_str = name.to_str().ok_or(KernelError::Io)?;
    let created = match super::durable_fs::create_new_file_rw(root_directory, name_str) {
        Ok(file) => file,
        // A pristine root may already hold an empty file from an interrupted
        // earlier bootstrap; opening the existing entry identifies it the same way.
        Err(super::durable_fs::StorageError::Other(source))
            if source.kind() == ErrorKind::AlreadyExists =>
        {
            super::durable_fs::open_regular_nofollow(root_directory, name_str)
                .map_err(|_| KernelError::Io)?
        }
        Err(_) => return Err(KernelError::Io),
    };
    if let Some(hook) = hook.as_mut() {
        hook(OpenPhase::AfterDatabaseCreated);
    }
    let bound = || {
        super::backup::assert_entry_is_descriptor(root_directory, name, &created)
            .map_err(|_| KernelError::Io)?;
        super::backup::assert_same_file(root_directory, name, path, KernelError::Io)
    };
    bound()?;
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| KernelError::Io)?;
    bound()?;
    apply_preclassification_profile(&conn).map_err(|_| KernelError::Io)?;
    let incarnation: String = conn
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(|_| KernelError::Io)?;
    apply_kernel_schema(&mut conn, &incarnation, current_time_ms()).map_err(|_| KernelError::Io)?;
    Ok(conn)
}

pub(super) fn apply_preclassification_profile(conn: &Connection) -> rusqlite::Result<()> {
    conn.set_prepared_statement_cache_capacity(PREPARED_STATEMENT_CACHE_CAPACITY);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
    // REPLACE deletes the conflicting row, which fires its BEFORE DELETE trigger
    // only when recursive_triggers is ON.
    conn.pragma_update(None, "recursive_triggers", "ON")?;
    Ok(())
}

/// `PRAGMA journal_mode` returns the mode now in effect rather than failing.
///
/// Readers depend on WAL for snapshot isolation, so the returned mode is checked.
pub(super) fn activate_wal(conn: &Connection) -> Result<(), KernelError> {
    let mode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .map_err(|_| KernelError::Io)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(KernelError::Io);
    }
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|_| KernelError::Io)
}

/// Atomically stamps `epoch` into the singleton writer-fence row.
///
/// Values outside SQLite's signed integer range or a missing singleton row
/// return `IdentityMismatch`.
/// The writer epoch the database carries; `0` for a database no writer has
/// fenced yet, so any acquired lease epoch exceeds it.
fn durable_fence_epoch(conn: &Connection) -> Result<u64, KernelError> {
    let durable: i64 = conn
        .query_row(
            "SELECT writer_epoch FROM writer_fence WHERE id=0",
            [],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::IdentityMismatch)?;
    Ok(u64::try_from(durable).unwrap_or(0))
}

/// Raises the durable fence to `epoch` for a database this open is taking over.
/// Refuses with `Held` when the database already carries an epoch at or above
/// it: another writer fenced it more recently, and its lease, not this one, is
/// the one the database honors.
fn raise_writer_fence(conn: &mut Connection, epoch: u64) -> Result<(), KernelError> {
    let epoch = i64::try_from(epoch).map_err(|_| KernelError::IdentityMismatch)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| KernelError::Io)?;
    let raised = tx
        .execute(
            "UPDATE writer_fence SET writer_epoch=?1 WHERE id=0 AND writer_epoch<?1",
            [epoch],
        )
        .map_err(|_| KernelError::Io)?;
    if raised != 1 {
        return Err(KernelError::Held);
    }
    tx.commit().map_err(|_| KernelError::Io)
}

/// Sets the durable fence to `epoch` regardless of what the database carried.
/// Only a restore does this: the installed family came from a backup whose
/// fence belongs to another history, and the lease this store holds is the one
/// that governs it from here on.
pub(super) fn stamp_writer_fence(conn: &mut Connection, epoch: u64) -> Result<(), KernelError> {
    let epoch = i64::try_from(epoch).map_err(|_| KernelError::IdentityMismatch)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| KernelError::Io)?;
    if tx
        .execute(
            "UPDATE writer_fence SET writer_epoch=?1 WHERE id=0",
            [epoch],
        )
        .map_err(|_| KernelError::Io)?
        != 1
    {
        return Err(KernelError::IdentityMismatch);
    }
    tx.commit().map_err(|_| KernelError::Io)
}

pub(super) fn open_reader(path: &Path) -> Result<Connection, KernelError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(super::map_sqlite)?;
    conn.pragma_update(None, "query_only", "ON")
        .map_err(super::map_sqlite)?;
    apply_preclassification_profile(&conn).map_err(super::map_sqlite)?;
    Ok(conn)
}

fn open_read_pool(path: &Path) -> Result<Vec<Mutex<Connection>>, KernelError> {
    (0..READ_POOL_SIZE)
        .map(|_| {
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| KernelError::Io)?;
            conn.pragma_update(None, "query_only", "ON")
                .map_err(|_| KernelError::Io)?;
            apply_preclassification_profile(&conn).map_err(|_| KernelError::Io)?;
            let query_only: i64 = conn
                .query_row("PRAGMA query_only", [], |row| row.get(0))
                .map_err(|_| KernelError::Io)?;
            if query_only != 1 {
                return Err(KernelError::Io);
            }
            Ok(Mutex::new(conn))
        })
        .collect()
}

pub(super) fn family_sidecars(path: &Path) -> [PathBuf; 3] {
    [
        suffix_path(path, "-wal"),
        suffix_path(path, "-shm"),
        suffix_path(path, "-journal"),
    ]
}

/// Restricts the main database and existing SQLite sidecars to private access.
pub(super) fn harden_family(path: &Path) -> Result<(), KernelError> {
    protect_file(path).map_err(|_| KernelError::Io)?;
    for sidecar in family_sidecars(path) {
        protect_file(&sidecar).map_err(|_| KernelError::Io)?;
    }
    Ok(())
}

pub(super) fn restore_marker_path(path: &Path) -> PathBuf {
    suffix_path(path, RESTORE_MARKER_SUFFIX)
}

/// `Path::display` replaces non-UTF-8 bytes, which would name a different file.
pub(super) fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Canonicalizing after creation gives every spelling of one directory one name;
/// `read_valid_restore_marker` compares the marker's `database_path` against
/// the live path by value. The returned descriptor is the directory whose mode
/// was set, so the caller can hold that directory rather than reopen the name.
fn prepare_root(root: &Path) -> Result<(PathBuf, File), KernelError> {
    // Only the ancestors are created here. `create_dir_all` would create the
    // root itself under the process umask, and the owner-only creation in
    // `prepare_private_dir` would then find it already present.
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent).map_err(|_| KernelError::Io)?;
    }
    let directory = prepare_private_dir(root)?;
    let canonical = fs::canonicalize(root).map_err(|_| KernelError::Io)?;
    Ok((canonical, directory))
}

/// A point during `KernelStore::open` at which a test hook runs. The open path
/// names it in every build; only the hook entry points are feature-gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPhase {
    /// After the root has been checked and the database pathname inspected,
    /// before it is opened or created.
    BeforeDatabaseOpen,
    /// During a pristine bootstrap, after the database file exists below the
    /// held root and before SQLite opens it.
    AfterDatabaseCreated,
    /// After the writer connection is open, before the first write through it.
    BeforeFirstWrite,
}

/// Opens a store root pathname with `DIRECTORY | NOFOLLOW`.
fn open_root_directory(root: &Path) -> Result<File, KernelError> {
    rustix::fs::open(
        root,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| KernelError::Io)
}

/// Fails with `Io` unless `root` still names the directory `held` is open on.
fn assert_root_unchanged(held: &File, root: &Path) -> Result<(), KernelError> {
    use std::os::unix::fs::MetadataExt;
    let held = held.metadata().map_err(|_| KernelError::Io)?;
    let current = fs::symlink_metadata(root).map_err(|_| KernelError::Io)?;
    if held.dev() != current.dev() || held.ino() != current.ino() {
        return Err(KernelError::Io);
    }
    Ok(())
}

/// Creates `path` owner-only if absent, then opens it `DIRECTORY | NOFOLLOW` and
/// checks and sets its mode through that one descriptor. A pathname the check
/// read and a pathname the mode change wrote could name different directories
/// after a replacement in between; the descriptor cannot.
fn prepare_private_dir(path: &Path) -> Result<File, KernelError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    // The umask would leave a readable window between `create_dir` and `chmod`.
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(KernelError::Io),
    }
    let directory = open_root_directory(path)?;
    let metadata = directory.metadata().map_err(|_| KernelError::Io)?;
    // Tightening the mode of a directory another user owns would hand that
    // user, not this process, exclusive control of the store's entries, so
    // ownership is checked before any mode change, as the artifact and backup
    // directories already require.
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(KernelError::Io);
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        rustix::fs::fchmod(&directory, rustix::fs::Mode::RWXU).map_err(|_| KernelError::Io)?;
    }
    Ok(directory)
}

fn map_lease_error(error: LeaseError) -> KernelError {
    match error {
        LeaseError::Held { .. } => KernelError::Held,
        LeaseError::Io(_) => KernelError::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_variant() {
        // The exhaustive match stops compiling when a variant is added, which
        // points at `ALL` as the list to extend.
        for error in KernelError::ALL {
            match error {
                KernelError::Held
                | KernelError::EngineUnsupported
                | KernelError::Foreign
                | KernelError::Inconclusive
                | KernelError::Io
                | KernelError::Busy
                | KernelError::IdentityMismatch
                | KernelError::FenceLost
                | KernelError::Conflict
                | KernelError::CorruptCanonicalRow
                | KernelError::InvalidInput
                | KernelError::AdmissionPolicy
                | KernelError::PreviewAuthorityChanged
                | KernelError::FutureSnapshot
                | KernelError::NotFound
                | KernelError::InvalidCheckpoint
                | KernelError::NoRequiredConsumers
                | KernelError::ConsumerPending
                | KernelError::Fault
                | KernelError::Deadline
                | KernelError::UnsafeDestination
                | KernelError::InvalidBackup
                | KernelError::InvalidRestore => {}
            }
        }
        let mut names: Vec<_> = KernelError::ALL.iter().map(|e| e.to_string()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), KernelError::ALL.len(), "duplicate in ALL");
    }

    #[test]
    fn owned_read_connections_are_query_only() {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        store
            .with_reader(|connection| {
                assert_eq!(
                    connection.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))?,
                    1
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn stale_writer_fence_blocks_the_operation() {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        store
            .with_writer(|connection| {
                connection.execute(
                    "UPDATE writer_fence SET writer_epoch=writer_epoch+1 WHERE id=0",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let mut called = false;
        let error = store
            .with_writer(|_| {
                called = true;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error, KernelError::FenceLost);
        assert!(!called);
    }

    #[test]
    fn failed_writer_operation_leaves_no_partial_write() {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        let error = store
            .with_writer(|tx| -> rusqlite::Result<()> {
                tx.execute(
                    "INSERT INTO commit_log(
                         transaction_id,writer_epoch,producer,operation_key,request_digest,
                         recorded_at,actor,cause
                     ) VALUES('t1',1,'fixture','t1','',1,'actor','cause')",
                    [],
                )?;
                Err(rusqlite::Error::InvalidQuery)
            })
            .unwrap_err();
        assert_eq!(error, KernelError::Io);
        store
            .with_reader(|tx| {
                assert_eq!(
                    tx.query_row("SELECT COUNT(*) FROM commit_log", [], |row| row
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn store_connections_run_delete_triggers_for_replace() {
        // The schema-level guard test drives a connection built by
        // `apply_kernel_connection_profile`, so it cannot observe the pragmas
        // `KernelStore` sets on its own writer.
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        store
            .with_writer(|tx| {
                assert_eq!(
                    tx.query_row("PRAGMA recursive_triggers", [], |row| row.get::<_, i64>(0))?,
                    1
                );
                tx.execute(
                    "INSERT INTO commit_log(
                         transaction_id,writer_epoch,producer,operation_key,request_digest,
                         recorded_at,actor,cause
                     ) VALUES('t1',1,'fixture','t1','',1,'actor','cause')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let commit_seq = store
            .with_reader(|tx| {
                tx.query_row("SELECT commit_seq FROM commit_log", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .unwrap();

        for verb in ["INSERT OR REPLACE", "REPLACE"] {
            let error = store
                .with_writer(|tx| {
                    tx.execute(
                        &format!(
                            "{verb} INTO commit_log(
                                 commit_seq,transaction_id,writer_epoch,producer,operation_key,
                                 request_digest,recorded_at,actor,cause
                             ) VALUES(?1,'hijack',1,'fixture','hijack','',1,'attacker','rewrite')"
                        ),
                        [commit_seq],
                    )?;
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(error, KernelError::Io, "{verb} must be refused");
        }

        store
            .with_reader(|tx| {
                assert_eq!(
                    tx.query_row("SELECT actor FROM commit_log", [], |row| row
                        .get::<_, String>(0))?,
                    "actor"
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_private_dir_is_prepared_through_the_descriptor_it_returns() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

        let held = prepare_private_dir(&root).unwrap();
        let by_descriptor = held.metadata().unwrap();
        let by_name = fs::symlink_metadata(&root).unwrap();
        assert_eq!(by_descriptor.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            (by_descriptor.dev(), by_descriptor.ino()),
            (by_name.dev(), by_name.ino()),
            "the descriptor is the directory the pathname named"
        );
    }

    #[test]
    fn a_symlinked_root_is_refused_and_its_target_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let root = dir.path().join("store");
        std::os::unix::fs::symlink(&target, &root).unwrap();

        assert_eq!(prepare_private_dir(&root).unwrap_err(), KernelError::Io);
        assert_eq!(
            fs::symlink_metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755,
            "a symlink at the root pathname must not have its target's mode changed"
        );
    }

    #[test]
    fn a_root_replaced_while_open_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        fs::create_dir(&root).unwrap();
        let held = open_root_directory(&root).unwrap();
        assert_root_unchanged(&held, &root).unwrap();

        // The same pathname now names a different directory.
        fs::rename(&root, dir.path().join("moved-away")).unwrap();
        fs::create_dir(&root).unwrap();
        assert_eq!(
            assert_root_unchanged(&held, &root).unwrap_err(),
            KernelError::Io
        );

        // A symlink at the pathname is not the held directory either.
        fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(dir.path().join("moved-away"), &root).unwrap();
        assert_eq!(
            assert_root_unchanged(&held, &root).unwrap_err(),
            KernelError::Io
        );
        assert_eq!(open_root_directory(&root).unwrap_err(), KernelError::Io);
    }
}

/// When a bounded caller stops waiting for a connection.
///
/// A deadline and a cooperative interrupt are separate mechanisms: a caller can
/// arm either, both, or only the interrupt, and an interrupt-only caller still
/// has to be able to stop. This carries them as plain values so the store's
/// acquisition path does not depend on the applicability engine's budget type.
#[derive(Debug, Clone, Default)]
pub(crate) struct AcquireLimit {
    deadline: Option<Instant>,
    interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl AcquireLimit {
    pub(crate) fn new(
        deadline: Option<Instant>,
        interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Self {
        Self {
            deadline,
            interrupt,
        }
    }

    pub(crate) fn until(deadline: Instant) -> Self {
        Self::new(Some(deadline), None)
    }

    /// Pooled connections outlive one scan; the guard clears the handler on
    /// every exit path. commentlint: allow(JUDGE)
    pub(crate) fn install_progress_handler(
        self,
        connection: &Connection,
        steps: i32,
    ) -> Result<ProgressInterrupt<'_>, KernelError> {
        connection
            .progress_handler(steps, Some(move || self.should_stop()))
            .map_err(|_| KernelError::Io)?;
        Ok(ProgressInterrupt { connection })
    }

    /// Raises the interrupt when the deadline passes, so one crossing stops
    /// every waiter sharing the flag rather than only the one that noticed.
    pub(crate) fn should_stop(&self) -> bool {
        if self
            .interrupt
            .as_ref()
            .is_some_and(|interrupt| interrupt.load(Ordering::Relaxed))
        {
            return true;
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            if let Some(interrupt) = &self.interrupt {
                interrupt.store(true, Ordering::Relaxed);
            }
            return true;
        }
        false
    }
}

/// Clears the progress handler `AcquireLimit::install_progress_handler` set
/// when dropped.
pub(crate) struct ProgressInterrupt<'c> {
    connection: &'c Connection,
}

impl Drop for ProgressInterrupt<'_> {
    fn drop(&mut self) {
        let _ = self.connection.progress_handler(0, None::<fn() -> bool>);
    }
}
