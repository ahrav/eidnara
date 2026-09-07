//! Creates self-contained SQLite backups and restores them with crash recovery.
//!
//! Backup publication uses descriptor-relative filesystem operations. Restore
//! stages and verifies bytes before displacing the live database family.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use rustix::fs::{self as rfs, AtFlags, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::durable_fs::{
    self, PublishOutcome, create_new_file, create_secure_directory, durable_unlink, next_unique_id,
    open_secure_directory, publish_noreplace_locked, temp_name as durable_temp_name,
    write_and_sync,
};
use super::envelope::check_fence;
use crate::current_time_ms;

use super::open::{
    activate_wal, apply_preclassification_profile, family_sidecars, harden_family, open_reader,
    open_writer, restore_marker_path, stamp_writer_fence, suffix_path, verify_exact_identity,
};
use super::{KernelError, KernelStore, Sensitivity};

const BACKUP_PAGES_PER_STEP: i32 = 128;
// `Busy` and `Locked` mean the step made no progress, so yielding alone spins a
// core flat until the deadline when another connection holds the source lock.
const BACKUP_CONTENTION_BACKOFF: std::time::Duration = std::time::Duration::from_millis(1);
const DEFAULT_CAPTURE_PIN_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;
const BACKUP_PREFIX: &str = "kernel-backup-";
const RESTORE_INFIX: &str = ".restore-";
const RESTORE_MARKER_PROTOCOL: &str = "eidnara-kernel-restore-marker-v1";
/// Bound the marker read so invalid content cannot control allocation size.
const RESTORE_MARKER_MAX_BYTES: u64 = 64 * 1024;
#[cfg(target_os = "linux")]
const LOCAL_FILESYSTEMS: &[u64] = &[
    0x0000_ef53, // ext2, ext3, ext4
    0x5846_5342, // XFS
    0x9123_683e, // Btrfs
    0x0102_1994, // tmpfs
    0x8584_58f6, // ramfs
    0x794c_7630, // overlayfs
    0x2fc1_2fc1, // ZFS
    0x0000_f15f, // eCryptfs
    0x2405_1905, // UBIFS
    0xf2f5_2010, // F2FS
    0x0000_72b6, // JFFS2
    0x5265_4973, // ReiserFS
    0x0000_3434, // NILFS
];

/// Parameters for one bounded backup operation.
#[derive(Debug, Clone)]
pub struct BackupRequest {
    /// Private directory that receives the published backup.
    pub destination_directory: PathBuf,
    /// The backup checks `deadline` before the writer lock and around capture, copy, and verification.
    /// Destination validation runs before the first check, and publication with its directory sync runs after the last, so a backup can finish after `deadline`.
    pub deadline: Instant,
    /// Optional expiration time for evidence pins created by the capture.
    pub capture_pin_expires_at: Option<i64>,
}

/// Identity and retention metadata for a published backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupManifest {
    /// Highest commit sequence present in the artifact.
    pub captured_commit_seq: i64,
    /// Evidence active at `captured_commit_seq`. Rows invalidated earlier are in the
    /// artifact but are neither listed nor pinned.
    pub evidence_refs: Vec<String>,
    /// Highest sensitivity stored in the captured database.
    pub max_sensitivity: Sensitivity,
    /// Pin that protects referenced evidence until release or expiration.
    pub capture_pin_id: Option<String>,
    /// The requested directory joined with the published name. Publication goes
    /// through a validated descriptor, so this resolves to the artifact unless the
    /// destination directory is replaced concurrently.
    pub destination_path: PathBuf,
}

/// Restore interruption points used by crash-recovery proofs.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreFault {
    /// Fail after publishing the marker but before moving the live family.
    BeforeDisplace,
    /// Fail after moving the live family into recovery storage.
    AfterDisplace,
    /// Fail after displacement and force rollback recovery to fail.
    RecoveryFailure,
}

struct CaptureState {
    commit_seq: i64,
    evidence_refs: Vec<String>,
    max_sensitivity: Sensitivity,
    pin_id: Option<String>,
}

impl KernelStore {
    /// Captures and atomically publishes a verified database backup.
    pub fn backup(&self, request: BackupRequest) -> Result<BackupManifest, KernelError> {
        self.backup_inner(request, false, None, None)
    }

    #[cfg(feature = "test-support")]
    pub fn backup_with_fault_before_rename_for_test(
        &self,
        request: BackupRequest,
    ) -> Result<BackupManifest, KernelError> {
        self.backup_inner(request, true, None, None)
    }

    #[cfg(feature = "test-support")]
    pub fn backup_with_hook_for_test(
        &self,
        request: BackupRequest,
        mut hook: impl FnMut(),
    ) -> Result<BackupManifest, KernelError> {
        self.backup_inner(request, false, Some(&mut hook), None)
    }

    #[cfg(feature = "test-support")]
    pub fn backup_with_final_name_for_test(
        &self,
        request: BackupRequest,
        final_name: &str,
    ) -> Result<BackupManifest, KernelError> {
        self.backup_inner(request, false, None, Some(final_name))
    }

    fn backup_inner(
        &self,
        request: BackupRequest,
        fault_before_rename: bool,
        mut hook: Option<&mut dyn FnMut()>,
        final_name_override: Option<&str>,
    ) -> Result<BackupManifest, KernelError> {
        let destination = secure_destination(&request.destination_directory)?;
        if Instant::now() >= request.deadline {
            return Err(KernelError::Deadline);
        }
        let mut writer =
            self.lock_writer_within(&super::open::AcquireLimit::until(request.deadline))?;
        let capture = capture_state(
            &mut writer,
            self.lease_epoch(),
            request.capture_pin_expires_at,
            request.deadline,
        )?;
        let unique = next_unique_id();
        let final_name = final_name_override
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{BACKUP_PREFIX}{}-{unique}.sqlite", capture.commit_seq));
        let temp_name = durable_temp_name(&format!("{BACKUP_PREFIX}{}", capture.commit_seq));
        let final_path = request.destination_directory.join(&final_name);

        let mut published = false;
        let result = (|| {
            // The descriptor is held for the whole capture: every identity check
            // below compares a pathname entry against it, and the published entry
            // is compared against it last.
            let staged = create_new_file(&destination, &temp_name).map_err(|_| KernelError::Io)?;
            if let Some(callback) = hook.as_mut() {
                callback();
            }
            let sqlite_temp_path = request.destination_directory.join(&temp_name);
            let mut target = Connection::open_with_flags(
                &sqlite_temp_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| KernelError::InvalidBackup)?;
            // SQLite resolves a pathname, so the file it opened is compared with the file created through the verified descriptor before a page is copied; a destination swapped in between would otherwise receive the whole copy. commentlint: allow(JUDGE)
            assert_same_file(
                &destination,
                std::ffi::OsStr::new(&temp_name),
                &sqlite_temp_path,
                KernelError::InvalidBackup,
            )?;
            {
                let backup =
                    Backup::new(&writer, &mut target).map_err(|_| KernelError::InvalidBackup)?;
                loop {
                    if Instant::now() >= request.deadline {
                        return Err(KernelError::Deadline);
                    }
                    match backup
                        .step(BACKUP_PAGES_PER_STEP)
                        .map_err(|_| KernelError::InvalidBackup)?
                    {
                        StepResult::Done => break,
                        StepResult::More => {}
                        StepResult::Busy | StepResult::Locked => {
                            std::thread::sleep(BACKUP_CONTENTION_BACKOFF);
                        }
                        _ => return Err(KernelError::InvalidBackup),
                    }
                }
            }
            seal_artifact_journal(&target)?;
            drop(target);
            if Instant::now() >= request.deadline {
                return Err(KernelError::Deadline);
            }
            verify_database(
                &sqlite_temp_path,
                Some(capture.commit_seq),
                KernelError::InvalidBackup,
                Some(request.deadline),
            )?;
            if Instant::now() >= request.deadline {
                return Err(KernelError::Deadline);
            }
            cleanup_backup_sidecars(&destination, &temp_name)?;
            // SQLite uses a pathname, while cleanup uses the verified directory
            // descriptor; comparing identities rejects destination swaps.
            assert_same_file(
                &destination,
                std::ffi::OsStr::new(&temp_name),
                &sqlite_temp_path,
                KernelError::InvalidBackup,
            )?;
            sync_child(&destination, &temp_name)?;
            if Instant::now() >= request.deadline {
                return Err(KernelError::Deadline);
            }
            if fault_before_rename {
                return Err(KernelError::Fault);
            }
            // Set `published` before the directory sync so cleanup removes an
            // artifact published before a directory-sync failure.
            match publish_noreplace_locked(&destination, &temp_name, &final_name)
                .map_err(|_| KernelError::Io)?
            {
                PublishOutcome::AlreadyExists => return Err(KernelError::Io),
                PublishOutcome::Published => {}
                // `cleanup_backup_family` retries removal of both the final
                // artifact and the retained temp link on the error path.
                PublishOutcome::PublishedTempRetained => {
                    published = true;
                    return Err(KernelError::Io);
                }
            }
            published = true;
            // A temporary entry swapped between the last identity check and the
            // publish would have been published instead of the verified file; the
            // error arm then removes whatever was published.
            assert_entry_is_descriptor(&destination, std::ffi::OsStr::new(&final_name), &staged)
                .map_err(|_| KernelError::InvalidBackup)?;
            durable_fs::sync_directory(&destination).map_err(|_| KernelError::Io)?;
            Ok(BackupManifest {
                captured_commit_seq: capture.commit_seq,
                evidence_refs: capture.evidence_refs.clone(),
                max_sensitivity: capture.max_sensitivity,
                capture_pin_id: capture.pin_id.clone(),
                destination_path: final_path.clone(),
            })
        })();

        if result.is_err() {
            let mut cleaned = cleanup_backup_family(&destination, &temp_name);
            if published {
                cleaned &= cleanup_backup_family(&destination, &final_name);
            }
            let _ = destination.sync_all();
            // Keep the pin until cleanup succeeds: retention could reap
            // evidence still referenced by a lingering artifact. The pin's
            // own expiry reclaims it when cleanup never succeeds.
            if cleaned && let Some(pin_id) = capture.pin_id.as_deref() {
                rollback_capture_pin(&mut writer, self.lease_epoch(), pin_id);
            }
        }
        result
    }

    /// Releases an active backup capture pin and all evidence references it owns.
    pub fn release_capture_pin(
        &self,
        capture_pin_id: &str,
        released_at: i64,
    ) -> Result<(), KernelError> {
        if capture_pin_id.trim().is_empty() || released_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&tx, self.lease_epoch())?;
        let changed = tx
            .execute(
                "UPDATE capture_pins SET released_at=?1
                 WHERE capture_pin_id=?2 AND released_at IS NULL",
                params![released_at, capture_pin_id],
            )
            .map_err(|_| KernelError::Io)?;
        if changed != 1 {
            return Err(KernelError::NotFound);
        }
        tx.execute(
            "UPDATE capture_pin_refs SET released_at=?1
             WHERE capture_pin_id=?2 AND released_at IS NULL",
            params![released_at, capture_pin_id],
        )
        .map_err(|_| KernelError::Io)?;
        tx.commit().map_err(|_| KernelError::Io)
    }

    /// Expires due capture pins and prunes references past the reclaim grace period.
    pub fn run_capture_pin_maintenance(&self, now_ms: i64) -> Result<(), KernelError> {
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&tx, self.lease_epoch())?;
        tx.execute(
            "UPDATE capture_pins SET released_at=?1
             WHERE released_at IS NULL AND expires_at IS NOT NULL AND expires_at<=?1",
            [now_ms],
        )
        .map_err(|_| KernelError::Io)?;
        tx.execute(
            "UPDATE capture_pin_refs
             SET released_at=(SELECT released_at FROM capture_pins
                              WHERE capture_pin_id=capture_pin_refs.capture_pin_id)
             WHERE released_at IS NULL AND capture_pin_id IN (
                 SELECT capture_pin_id FROM capture_pins WHERE released_at IS NOT NULL
             )",
            [],
        )
        .map_err(|_| KernelError::Io)?;
        // Past the reclaim grace a released reference no longer affects eligibility,
        // and reclamation only prunes references for artifacts it removes, so pins on
        // still-live artifacts accumulate without this.
        let horizon = now_ms.saturating_sub(crate::cas::gc::REFERENCED_GRACE_MS);
        tx.execute(
            "DELETE FROM capture_pin_refs WHERE released_at IS NOT NULL AND released_at<=?1",
            [horizon],
        )
        .map_err(|_| KernelError::Io)?;
        tx.commit().map_err(|_| KernelError::Io)
    }

    /// Verifies and installs a backup, returning its captured commit sequence.
    ///
    /// A backup whose live evidence references an artifact this store has purged, does not hold, or holds only as bytes that fail verification is refused as `InvalidRestore` before the live family is displaced: installing it would reverse an irreversible purge or publish references every read would then fail against. commentlint: allow(JUDGE)
    /// The checks run under the writer guard that purges and purge unlinks also hold, so neither can change the answer between the check and the displacement.
    /// After installing the backup, `restore` runs interrupted-work recovery before returning, so it unlinks the bytes of any purge the backup recorded as committed and pending unlink. commentlint: allow(JUDGE)
    /// Recovery errors, including a purge unlink that could not complete, are reported as `Io` after the backup is installed; the pending unlink stays recorded for maintenance to retry.
    pub fn restore(&self, backup_path: impl AsRef<Path>) -> Result<i64, KernelError> {
        self.restore_inner(backup_path.as_ref(), None, None)
    }

    #[cfg(feature = "test-support")]
    pub fn restore_with_hook_for_test(
        &self,
        backup_path: impl AsRef<Path>,
        mut hook: impl FnMut(),
    ) -> Result<i64, KernelError> {
        self.restore_inner(backup_path.as_ref(), None, Some(&mut hook))
    }

    // Leaves the on-disk state a process killed between publishing the marker and
    // displacing the family would leave: marker present, recovery directory empty,
    // live family untouched.
    #[cfg(feature = "test-support")]
    pub fn abandon_restore_marker_for_test(&self) -> Result<PathBuf, KernelError> {
        let root = self
            .root_directory
            .try_clone()
            .map_err(|_| KernelError::Io)?;
        let recovery = RecoveryDir::create(&self.db_path, root)?;
        publish_restore_marker(&self.db_path, &recovery)?;
        Ok(recovery.path)
    }

    #[cfg(feature = "test-support")]
    pub fn restore_with_fault_for_test(
        &self,
        backup_path: impl AsRef<Path>,
        fault: RestoreFault,
    ) -> Result<i64, KernelError> {
        self.restore_inner(backup_path.as_ref(), Some(fault), None)
    }

    fn restore_inner(
        &self,
        backup_path: &Path,
        #[cfg(feature = "test-support")] fault: Option<RestoreFault>,
        #[cfg(not(feature = "test-support"))] fault: Option<std::convert::Infallible>,
        hook: Option<&mut dyn FnMut()>,
    ) -> Result<i64, KernelError> {
        let source_seq = self.install_backup(backup_path, fault, hook)?;
        // The installed history carries its own interrupted work, such as a purge that committed without unlinking its bytes; the connection guards are released here, so recovery can take them. commentlint: allow(JUDGE)
        // A purge the restored history owes whose bytes are still readable is a failure of this restore, not a deferred chore, because the caller was promised those bytes are gone before the call returns. commentlint: allow(JUDGE)
        if self.recover_interrupted_work()? > 0 {
            return Err(KernelError::Io);
        }
        Ok(source_seq)
    }

    fn install_backup(
        &self,
        backup_path: &Path,
        #[cfg(feature = "test-support")] fault: Option<RestoreFault>,
        #[cfg(not(feature = "test-support"))] fault: Option<std::convert::Infallible>,
        mut hook: Option<&mut dyn FnMut()>,
    ) -> Result<i64, KernelError> {
        #[cfg(feature = "test-support")]
        let fault_before_displace = fault == Some(RestoreFault::BeforeDisplace);
        #[cfg(feature = "test-support")]
        let fault_after_displace = matches!(
            fault,
            Some(RestoreFault::AfterDisplace) | Some(RestoreFault::RecoveryFailure)
        );
        #[cfg(feature = "test-support")]
        let force_recovery_failure = fault == Some(RestoreFault::RecoveryFailure);
        #[cfg(not(feature = "test-support"))]
        let (fault_before_displace, fault_after_displace, force_recovery_failure) = {
            let _ = fault;
            (false, false, false)
        };
        let mut source = open_private_regular_nofollow(backup_path)?;
        let root = self
            .root_directory
            .try_clone()
            .map_err(|_| KernelError::Io)?;
        let temp_path = restore_temp_path(&self.db_path);
        let temp_name = temp_path.file_name().ok_or(KernelError::Io)?;
        let temp_name_str = temp_name.to_str().ok_or(KernelError::Io)?;
        let main_name = self.db_path.file_name().ok_or(KernelError::Io)?;
        // The staged copy is created and written below the held root descriptor, so
        // a root pathname pointing elsewhere while the copy runs cannot receive it.
        let mut staged_file = copy_to_private_temp(&mut source, &root, temp_name_str)?;
        // Verifying the staged copy rather than the source makes the verified bytes
        // the installed bytes, so neither a replaced pathname nor an in-place
        // rewrite of the source can change what is installed. It also keeps the
        // verification outside the writer lock. The header check reads the held
        // descriptor. The SQLite verifier resolves a pathname, so the entry is
        // compared with that descriptor afterwards: a staged entry swapped before
        // verification fails here, and the descriptor is what the installation
        // below is checked against.
        let staged = assert_self_contained(&mut staged_file)
            .and_then(|()| verify_database(&temp_path, None, KernelError::InvalidRestore, None))
            .and_then(|seq| {
                let required = live_artifacts(&temp_path)?;
                assert_entry_is_descriptor(&root, temp_name, &staged_file)?;
                Ok((seq, required))
            });
        let (source_seq, required_artifacts) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                let _ = rfs::unlinkat(&root, temp_name, AtFlags::empty());
                return Err(error);
            }
        };

        // Every return between staging and the guarded section below would otherwise
        // leave a full database copy behind, so the staged file is owned until the
        // section that already cleans it up takes over.
        let mut staged = StagedRestore(Some((&root, temp_name)));
        let mut writer = self.lock_writer()?;
        // Matching `lock_reader`, a poisoned guard is recovered rather than failing
        // the restore: the connection behind it is replaced immediately below.
        let mut readers = self
            .readers
            .iter()
            .map(|reader| {
                reader
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
            .collect::<Vec<_>>();
        let fence_tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&fence_tx, self.lease_epoch())?;
        let live_seq = fence_tx
            .query_row(
                "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| KernelError::Io)?;
        assert_none_purged(&fence_tx, &required_artifacts)?;
        fence_tx.commit().map_err(|_| KernelError::Io)?;
        self.assert_artifacts_verified(&required_artifacts)?;
        let mut temporary = (0..=readers.len())
            .map(|_| Connection::open_in_memory().map_err(|_| KernelError::Io))
            .collect::<Result<Vec<_>, _>>()?;
        let recovery = RecoveryDir::create(
            &self.db_path,
            root.try_clone().map_err(|_| KernelError::Io)?,
        )?;
        if let Err(error) = publish_restore_marker(&self.db_path, &recovery) {
            let _ = rfs::unlinkat(&recovery.root, &recovery.name, AtFlags::REMOVEDIR);
            return Err(error);
        }
        // A restored database can change an artifact's classification while keeping the displaced commit-log tip, so a verdict cached on `(tip, generation)` before this point must not survive it. commentlint: allow(JUDGE)
        // `_change` is declared after the connection guards so it drops first, restoring an even generation before readers can acquire a swapped connection.
        let _change = self.begin_classification_change();
        let temporary_writer = temporary.remove(0);
        let old_writer = std::mem::replace(&mut *writer, temporary_writer);
        let old_readers = readers
            .iter_mut()
            .zip(temporary)
            .map(|(guard, replacement)| std::mem::replace(&mut **guard, replacement))
            .collect::<Vec<_>>();
        drop(old_readers);
        drop(old_writer);
        let mut displaced = false;
        staged.disarm();
        let restore_result = (|| {
            if fault_before_displace {
                return Err(KernelError::Fault);
            }
            displace_family(&self.db_path, &recovery)?;
            displaced = true;
            if let Some(callback) = hook.as_mut() {
                callback();
            }
            if fault_after_displace {
                return Err(KernelError::Fault);
            }
            rfs::renameat(&recovery.root, temp_name, &recovery.root, main_name)
                .map_err(|_| KernelError::Io)?;
            // Whatever the rename installed must be the verified staged file; a
            // staged entry swapped after verification is refused here, and the error
            // arm removes it and rolls the displaced family back.
            assert_entry_is_descriptor(&recovery.root, main_name, &staged_file)?;
            durable_fs::sync_directory(&recovery.root).map_err(|_| KernelError::Io)?;
            let opened =
                open_live_family(&self.db_path, self.lease_epoch(), source_seq, readers.len())?;
            // The connections resolved `db_path` by name, so the entry that pathname
            // reaches is compared with the installed file once more: a root swapped
            // in between would have opened a database the held root never received.
            assert_same_file(
                &recovery.root,
                main_name,
                &self.db_path,
                KernelError::InvalidRestore,
            )?;
            remove_restore_marker(&self.db_path, &recovery.root)?;
            cleanup_recovery_dir(&recovery);
            Ok(opened)
        })();

        match restore_result {
            Ok((new_writer, new_readers)) => {
                *writer = new_writer;
                for (guard, connection) in readers.iter_mut().zip(new_readers) {
                    **guard = connection;
                }
                Ok(source_seq)
            }
            Err(error) => {
                let _ = rfs::unlinkat(&recovery.root, temp_name, AtFlags::empty());
                if displaced {
                    let _ = remove_family(&self.db_path, &recovery.root);
                }
                let recovered = if force_recovery_failure {
                    Err(KernelError::InvalidRestore)
                } else {
                    match restore_displaced_family(&self.db_path, &recovery) {
                        Ok(()) => match open_live_family(
                            &self.db_path,
                            self.lease_epoch(),
                            live_seq,
                            readers.len(),
                        )
                        .and_then(|opened| {
                            assert_same_file(
                                &recovery.root,
                                main_name,
                                &self.db_path,
                                KernelError::InvalidRestore,
                            )?;
                            Ok(opened)
                        }) {
                            Ok(opened) => Ok(opened),
                            Err(error) => {
                                let _ = displace_family(&self.db_path, &recovery);
                                Err(error)
                            }
                        },
                        Err(error) => {
                            let _ = displace_family(&self.db_path, &recovery);
                            Err(error)
                        }
                    }
                };
                match recovered {
                    Ok((original_writer, original_readers)) => {
                        *writer = original_writer;
                        for (guard, connection) in readers.iter_mut().zip(original_readers) {
                            **guard = connection;
                        }
                        if remove_restore_marker(&self.db_path, &recovery.root).is_err() {
                            self.poison();
                            return Err(KernelError::InvalidRestore);
                        }
                        cleanup_recovery_dir(&recovery);
                        Err(error)
                    }
                    Err(_) => {
                        self.poison();
                        Err(KernelError::InvalidRestore)
                    }
                }
            }
        }
    }
}

fn secure_destination(path: &Path) -> Result<File, KernelError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| KernelError::UnsafeDestination)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o077 != 0
        || !owner_is_current(metadata.uid())
    {
        return Err(KernelError::UnsafeDestination);
    }
    let directory = rfs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| KernelError::UnsafeDestination)?;
    let opened = directory
        .metadata()
        .map_err(|_| KernelError::UnsafeDestination)?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(KernelError::UnsafeDestination);
    }
    classify_destination_filesystem(&directory)?;
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn classify_destination_filesystem(directory: &File) -> Result<(), KernelError> {
    let filesystem = rfs::fstatfs(directory).map_err(|_| KernelError::UnsafeDestination)?;
    // `FsWord` is `c_long`; masking to 32 bits prevents sign extension from changing filesystem magic values above `i32::MAX` on 32-bit targets.
    if filesystem_is_unsafe(filesystem.f_type as u64 & 0xffff_ffff) {
        return Err(KernelError::UnsafeDestination);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn classify_destination_filesystem(directory: &File) -> Result<(), KernelError> {
    let filesystem = rfs::fstatfs(directory).map_err(|_| KernelError::UnsafeDestination)?;
    let end = filesystem
        .f_fstypename
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(filesystem.f_fstypename.len());
    let name = filesystem.f_fstypename[..end]
        .iter()
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    let name = std::str::from_utf8(&name).map_err(|_| KernelError::UnsafeDestination)?;
    if filesystem_name_is_unsafe(name) {
        return Err(KernelError::UnsafeDestination);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn classify_destination_filesystem(_directory: &File) -> Result<(), KernelError> {
    Err(KernelError::UnsafeDestination)
}

#[cfg(target_os = "linux")]
fn filesystem_is_unsafe(fs_type: u64) -> bool {
    !LOCAL_FILESYSTEMS.contains(&fs_type)
}

#[cfg(target_os = "macos")]
fn filesystem_name_is_unsafe(name: &str) -> bool {
    !matches!(name, "apfs" | "hfs" | "tmpfs")
}

/// Applies the Linux destination-filesystem allowlist in tests.
#[cfg(all(target_os = "linux", feature = "test-support"))]
pub fn filesystem_is_unsafe_for_test(fs_type: u64) -> bool {
    filesystem_is_unsafe(fs_type)
}

/// Applies the macOS destination-filesystem allowlist in tests.
#[cfg(all(target_os = "macos", feature = "test-support"))]
pub fn filesystem_name_is_unsafe_for_test(name: &str) -> bool {
    filesystem_name_is_unsafe(name)
}

fn owner_is_current(uid: u32) -> bool {
    uid == rustix::process::geteuid().as_raw()
}

/// Reports whether `uid` matches the effective process owner.
#[cfg(feature = "test-support")]
pub fn owner_is_current_for_test(uid: u32) -> bool {
    owner_is_current(uid)
}

// Reaching the file through the verified directory descriptor means a destination
// swapped after `assert_same_file` cannot redirect this fsync.
fn sync_child(directory: &File, name: &str) -> Result<(), KernelError> {
    let file = rfs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| KernelError::Io)?;
    File::from(file).sync_all().map_err(|_| KernelError::Io)
}

/// Fails with `error` unless `pathname` resolves to the entry `name` inside
/// `directory`. SQLite opens by pathname, so this is how a connection is tied
/// back to a descriptor-anchored entry.
fn assert_same_file(
    directory: &File,
    name: &std::ffi::OsStr,
    pathname: &Path,
    error: KernelError,
) -> Result<(), KernelError> {
    let anchored = rfs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| error)?;
    let resolved = fs::symlink_metadata(pathname).map_err(|_| error)?;
    if anchored.st_dev != resolved.dev() || anchored.st_ino != resolved.ino() {
        return Err(error);
    }
    Ok(())
}

/// Fails with `InvalidRestore` unless the entry `name` inside `directory` is the
/// object `file` is open on.
fn assert_entry_is_descriptor(
    directory: &File,
    name: &std::ffi::OsStr,
    file: &File,
) -> Result<(), KernelError> {
    let entry = rfs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| KernelError::InvalidRestore)?;
    let held = file.metadata().map_err(|_| KernelError::InvalidRestore)?;
    if entry.st_dev != held.dev() || entry.st_ino != held.ino() {
        return Err(KernelError::InvalidRestore);
    }
    Ok(())
}

// Reference collection, the sensitivity scan and the per-evidence inserts all
// scale with stored rows, so a progress handler bounds them rather than leaving
// the writer held past the deadline.
fn capture_state(
    writer: &mut Connection,
    lease_epoch: u64,
    expires_at: Option<i64>,
    deadline: Instant,
) -> Result<CaptureState, KernelError> {
    if Instant::now() >= deadline {
        return Err(KernelError::Deadline);
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = Arc::clone(&interrupted);
        writer
            .progress_handler(
                1_000,
                Some(move || {
                    let expired = Instant::now() >= deadline;
                    if expired {
                        interrupted.store(true, Ordering::Release);
                    }
                    expired
                }),
            )
            .map_err(|_| KernelError::Io)?;
    }
    let captured = capture_state_inner(writer, lease_epoch, expires_at);
    writer
        .progress_handler(0, None::<fn() -> bool>)
        .map_err(|_| KernelError::Io)?;
    match captured {
        Err(KernelError::Io) if interrupted.load(Ordering::Acquire) => Err(KernelError::Deadline),
        other => other,
    }
}

fn capture_state_inner(
    writer: &mut Connection,
    lease_epoch: u64,
    expires_at: Option<i64>,
) -> Result<CaptureState, KernelError> {
    let tx = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| KernelError::Io)?;
    check_fence(&tx, lease_epoch)?;
    let commit_seq = tx
        .query_row(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| KernelError::Io)?;
    let evidence_refs = tx
        .prepare(
            "SELECT evidence_id FROM evidence_meta
             WHERE created_commit_seq<=?1
               AND (invalidated_commit_seq IS NULL OR invalidated_commit_seq>?1)
             ORDER BY evidence_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([commit_seq], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| KernelError::Io)?;
    let max_sensitivity = max_stored_sensitivity(&tx)?;
    let created_at = current_time_ms();
    let expires_at =
        expires_at.unwrap_or_else(|| created_at.saturating_add(DEFAULT_CAPTURE_PIN_LIFETIME_MS));
    if expires_at < created_at {
        return Err(KernelError::InvalidInput);
    }
    let pin_id = if evidence_refs.is_empty() {
        None
    } else {
        let pin_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|_| KernelError::Io)?;
        let epoch = i64::try_from(lease_epoch).map_err(|_| KernelError::InvalidInput)?;
        tx.execute(
            "INSERT INTO capture_pins(
                 capture_pin_id,pin_kind,owner_id,commit_seq,lease_epoch,writer_epoch,
                 created_at,expires_at
             ) VALUES (?1,'backup',?2,?3,?4,?4,?5,?6)",
            params![
                pin_id,
                format!("backup:{commit_seq}"),
                commit_seq,
                epoch,
                created_at,
                expires_at,
            ],
        )
        .map_err(|_| KernelError::Io)?;
        for evidence_id in &evidence_refs {
            tx.execute(
                "INSERT INTO capture_pin_refs(capture_pin_id,evidence_id,expires_at)
                 VALUES (?1,?2,?3)",
                params![pin_id, evidence_id, expires_at],
            )
            .map_err(|_| KernelError::Io)?;
        }
        Some(pin_id)
    };
    tx.commit().map_err(|_| KernelError::Io)?;
    Ok(CaptureState {
        commit_seq,
        evidence_refs,
        max_sensitivity,
        pin_id,
    })
}

fn rollback_capture_pin(writer: &mut Connection, lease_epoch: u64, pin_id: &str) {
    let Ok(tx) = writer.transaction_with_behavior(TransactionBehavior::Immediate) else {
        return;
    };
    if check_fence(&tx, lease_epoch).is_err() {
        return;
    }
    let released_at = current_time_ms();
    if tx
        .execute(
            "UPDATE capture_pin_refs SET released_at=?1 WHERE capture_pin_id=?2",
            params![released_at, pin_id],
        )
        .and_then(|_| {
            tx.execute(
                "UPDATE capture_pins SET released_at=?1 WHERE capture_pin_id=?2",
                params![released_at, pin_id],
            )
        })
        .and_then(|_| {
            tx.execute(
                "DELETE FROM capture_pin_refs WHERE capture_pin_id=?1",
                [pin_id],
            )
        })
        .and_then(|_| tx.execute("DELETE FROM capture_pins WHERE capture_pin_id=?1", [pin_id]))
        .is_ok()
    {
        let _ = tx.commit();
    }
}

fn cleanup_backup_family(directory: &File, name: &str) -> bool {
    let mut cleaned = true;
    for candidate in [
        name.to_string(),
        format!("{name}-journal"),
        format!("{name}-wal"),
        format!("{name}-shm"),
    ] {
        cleaned &= durable_unlink(directory, &candidate).is_ok();
    }
    cleaned
}

fn sensitivity_bearing_tables(tx: &rusqlite::Transaction<'_>) -> Result<Vec<String>, KernelError> {
    let mut statement = tx
        .prepare(
            "SELECT m.name FROM sqlite_schema m, pragma_table_info(m.name) p
             WHERE m.type='table' AND p.name='sensitivity_class'
             ORDER BY m.name",
        )
        .map_err(|_| KernelError::Io)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|_| KernelError::Io)?;
    // Interpolating a name into SQL is safe only for a plain identifier.
    if !names
        .iter()
        .all(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(KernelError::Io);
    }
    Ok(names)
}

fn max_stored_sensitivity(tx: &rusqlite::Transaction<'_>) -> Result<Sensitivity, KernelError> {
    let names = sensitivity_bearing_tables(tx)?;
    if names.is_empty() {
        return Ok(Sensitivity::Normal);
    }
    // Classify values outside the known vocabulary as `Secret`, matching `Sensitivity::from_stored`.
    let selects = names
        .iter()
        .map(|name| {
            format!("SELECT 1 FROM {name} WHERE sensitivity_class NOT IN ('normal','sensitive')")
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let has_secret: bool = tx
        .query_row(&format!("SELECT EXISTS({selects})"), [], |row| row.get(0))
        .map_err(|_| KernelError::Io)?;
    if has_secret {
        return Ok(Sensitivity::Secret);
    }
    let selects = names
        .iter()
        .map(|name| format!("SELECT 1 FROM {name} WHERE sensitivity_class='sensitive'"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let has_sensitive: bool = tx
        .query_row(&format!("SELECT EXISTS({selects})"), [], |row| row.get(0))
        .map_err(|_| KernelError::Io)?;
    Ok(if has_sensitive {
        Sensitivity::Sensitive
    } else {
        Sensitivity::Normal
    })
}

/// Lists tables whose rows contribute to backup sensitivity classification.
#[cfg(feature = "test-support")]
pub fn sensitivity_bearing_tables_for_test(conn: &mut Connection) -> Vec<String> {
    let tx = conn.transaction().expect("transaction");
    sensitivity_bearing_tables(&tx).expect("schema scan")
}

fn cleanup_backup_sidecars(directory: &File, name: &str) -> Result<(), KernelError> {
    for candidate in [
        format!("{name}-journal"),
        format!("{name}-wal"),
        format!("{name}-shm"),
    ] {
        durable_unlink(directory, &candidate).map_err(|_| KernelError::Io)?;
    }
    Ok(())
}

// SQLite refuses to open a read-only artifact whose header declares WAL but
// lacks a `-wal` sidecar, which is the state the backup copy leaves behind.
// `backup` seals its artifacts into rollback-journal mode, so a source still
// declaring WAL is a bare copy of a live main file whose committed pages may sit
// in a `-wal` that was never copied. SQLite would open it and silently read the
// older checkpointed state.
fn assert_self_contained(file: &mut File) -> Result<(), KernelError> {
    let mut header = [0u8; 20];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut header))
        .map_err(|_| KernelError::InvalidRestore)?;
    if header[18] != 1 || header[19] != 1 {
        return Err(KernelError::InvalidRestore);
    }
    Ok(())
}

fn seal_artifact_journal(target: &Connection) -> Result<(), KernelError> {
    let mode: String = target
        .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
        .map_err(|_| KernelError::InvalidBackup)?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(KernelError::InvalidBackup);
    }
    Ok(())
}

fn verify_database(
    path: &Path,
    expected_seq: Option<i64>,
    invalid_error: KernelError,
    deadline: Option<Instant>,
) -> Result<i64, KernelError> {
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| invalid_error)?;
    apply_preclassification_profile(&connection).map_err(|_| invalid_error)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    if let Some(deadline) = deadline {
        let interrupted = Arc::clone(&interrupted);
        connection
            .progress_handler(
                1_000,
                Some(move || {
                    let expired = Instant::now() >= deadline;
                    if expired {
                        interrupted.store(true, Ordering::Release);
                    }
                    expired
                }),
            )
            .map_err(|_| invalid_error)?;
    }
    let verification_error = || {
        if interrupted.load(Ordering::Acquire) {
            KernelError::Deadline
        } else {
            invalid_error
        }
    };
    verify_exact_identity(&mut connection).map_err(|_| verification_error())?;
    let commit_seq = connection
        .query_row(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| verification_error())?;
    if expected_seq.is_some_and(|expected| expected != commit_seq) {
        return Err(invalid_error);
    }
    Ok(commit_seq)
}

/// An artifact a backup's live evidence references, as that evidence records it.
struct RequiredArtifact {
    digest: String,
    byte_length: i64,
}

/// Every artifact the database's live evidence references. Evidence a purge or
/// deletion invalidated is excluded, so a backup that carries its own purge
/// history is not held to bytes that history removed. Two live rows recording
/// different lengths for one digest are returned as two entries, so the length
/// check below refuses the pair.
fn live_artifacts(path: &Path) -> Result<Vec<RequiredArtifact>, KernelError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| KernelError::InvalidRestore)?;
    apply_preclassification_profile(&connection).map_err(|_| KernelError::InvalidRestore)?;
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT artifact_digest,byte_length FROM evidence_meta
             WHERE invalidated_commit_seq IS NULL ORDER BY artifact_digest",
        )
        .map_err(|_| KernelError::InvalidRestore)?;
    let artifacts = statement
        .query_map([], |row| {
            Ok(RequiredArtifact {
                digest: row.get(0)?,
                byte_length: row.get(1)?,
            })
        })
        .map_err(|_| KernelError::InvalidRestore)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| KernelError::InvalidRestore)?;
    Ok(artifacts)
}

/// Refuses as `InvalidRestore` when the live store has purged any required
/// artifact. A purge is irreversible, so a backup that would republish live
/// evidence for a purged digest cannot be installed, whether or not the bytes
/// still await their unlink.
fn assert_none_purged(
    tx: &rusqlite::Transaction<'_>,
    required: &[RequiredArtifact],
) -> Result<(), KernelError> {
    let mut statement = tx
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM artifact_purge_tombstones WHERE artifact_digest=?1)",
        )
        .map_err(|_| KernelError::Io)?;
    for artifact in required {
        let purged: bool = statement
            .query_row([&artifact.digest], |row| row.get(0))
            .map_err(|_| KernelError::Io)?;
        if purged {
            return Err(KernelError::InvalidRestore);
        }
    }
    Ok(())
}

impl KernelStore {
    /// Refuses as `InvalidRestore` unless every required artifact is a regular
    /// file in this store's object tree whose bytes hash to its digest and
    /// match the length its evidence recorded. Reading the bytes is what
    /// `read_artifact` will do against the installed references, so the same
    /// verification decides here whether those reads can succeed.
    fn assert_artifacts_verified(&self, required: &[RequiredArtifact]) -> Result<(), KernelError> {
        for artifact in required {
            if !super::cas::is_artifact_digest(&artifact.digest) {
                return Err(KernelError::InvalidRestore);
            }
            let bytes = self
                .read_verified_object(&artifact.digest)
                .map_err(|_| KernelError::InvalidRestore)?;
            if i64::try_from(bytes.len()).ok() != Some(artifact.byte_length) {
                return Err(KernelError::InvalidRestore);
            }
        }
        Ok(())
    }
}

/// Verifies backup identity and commit sequence before `deadline`.
#[cfg(feature = "test-support")]
pub fn verify_backup_with_deadline_for_test(
    path: &Path,
    expected_seq: i64,
    deadline: Instant,
) -> Result<i64, KernelError> {
    verify_database(
        path,
        Some(expected_seq),
        KernelError::InvalidBackup,
        Some(deadline),
    )
}

// `serde_json` cannot round-trip a non-UTF-8 `PathBuf`, and a lossy path would
// fail the byte-exact comparison in `resume_restore`. The raw `OsStr` bytes do
// round-trip.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreMarker {
    protocol: String,
    database_path: Vec<u8>,
    recovery_directory: Vec<u8>,
    marker_digest: String,
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
}

fn restore_marker_digest(marker: &RestoreMarker) -> String {
    let mut hasher = Sha256::new();
    hasher.update(RESTORE_MARKER_PROTOCOL.as_bytes());
    hasher.update(b"\ndatabase_path=");
    hasher.update(&marker.database_path);
    hasher.update(b"\nrecovery_directory=");
    hasher.update(&marker.recovery_directory);
    format!("{:x}", hasher.finalize())
}

fn valid_recovery_path(path: &Path, recovery_dir: &Path) -> bool {
    recovery_dir.parent() == path.parent()
        && recovery_dir.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with(&format!(
                "{}{RESTORE_INFIX}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })
}

/// Reads the restore marker and applies every check `resume_restore` requires
/// before it acts, without touching anything else. `Inconclusive` means the next
/// open would refuse this root.
fn read_valid_restore_marker(
    path: &Path,
    root: File,
) -> Result<(RestoreMarker, RecoveryDir), KernelError> {
    let marker_name = restore_marker_name(path).map_err(|_| KernelError::Inconclusive)?;
    // Validating a pathname and then reopening it leaves a window for a swap, so the
    // checks and the read share one descriptor, opened below the held root.
    // `NONBLOCK` keeps a FIFO from blocking the open before the type check runs.
    let marker_file = rfs::openat(
        &root,
        &marker_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| KernelError::Inconclusive)?;
    let metadata = marker_file
        .metadata()
        .map_err(|_| KernelError::Inconclusive)?;
    if !metadata.is_file() || metadata.len() > RESTORE_MARKER_MAX_BYTES {
        return Err(KernelError::Inconclusive);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::take(marker_file, RESTORE_MARKER_MAX_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| KernelError::Inconclusive)?;
    let marker: RestoreMarker =
        serde_json::from_slice(&bytes).map_err(|_| KernelError::Inconclusive)?;
    let recovery_directory = path_from_bytes(&marker.recovery_directory);
    if marker.protocol != RESTORE_MARKER_PROTOCOL
        || path_from_bytes(&marker.database_path) != path
        || marker.marker_digest != restore_marker_digest(&marker)
        || !valid_recovery_path(path, &recovery_directory)
    {
        return Err(KernelError::Inconclusive);
    }
    // `Path::is_dir` follows a symlink, so the directory is opened `NOFOLLOW` and every rollback step below runs relative to that descriptor; a `.restore-*` entry swapped for a link cannot redirect the rollback. commentlint: allow(JUDGE)
    let recovery = RecoveryDir::open(root, recovery_directory)?;
    Ok((marker, recovery))
}

/// Whether a present restore marker would let the next open resume. Only tests
/// need to ask without opening; the oracle uses it to treat a corrupted marker as
/// different state from a valid one.
#[cfg(feature = "test-support")]
pub fn restore_marker_is_valid_for_test(database_path: &Path) -> bool {
    let Some(parent) = database_path.parent() else {
        return false;
    };
    let Ok(root) = rfs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    read_valid_restore_marker(database_path, File::from(root)).is_ok()
}

// Rolls back rather than rolling forward. A surviving marker pairs with a
// `restore` that never returned to its caller, so the displaced family is the
// authoritative copy and a half-installed replacement is discarded.
pub(super) fn resume_restore(path: &Path, root: File) -> Result<(), KernelError> {
    let (_marker, recovery) = read_valid_restore_marker(path, root)?;
    remove_restore_scratch(path, &recovery.root)?;
    // Only remove the live family after a displaced main file exists; otherwise it
    // remains the sole copy.
    let main_name = path.file_name().ok_or(KernelError::Inconclusive)?;
    if regular_file_present(&recovery.dir, main_name) {
        remove_family(path, &recovery.root).map_err(|_| KernelError::Inconclusive)?;
    } else if !regular_file_present(&recovery.root, main_name) {
        // The main file is in neither place, so the recovery directory cannot be
        // trusted to hold the family. Bootstrapping here would discard it.
        return Err(KernelError::Inconclusive);
    }
    restore_displaced_family(path, &recovery).map_err(|_| KernelError::Inconclusive)?;
    remove_restore_marker(path, &recovery.root)?;
    cleanup_recovery_dir(&recovery);
    Ok(())
}

// A crash between removing the marker and cleaning up leaves the prior family,
// which may hold sensitive rows, under `.restore-*` with nothing to reclaim it.
// `allocate_recovery_dir` appends only decimal digits, so anything else sharing
// the prefix was created by someone else and is left alone.
fn generated_recovery_suffix(name: &std::ffi::OsStr, prefix: &str) -> bool {
    let name = name.to_string_lossy();
    match name.strip_prefix(prefix) {
        Some(suffix) => !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

pub(super) fn reap_orphan_restore_recovery(
    path: &Path,
    parent_dir: &File,
) -> Result<(), KernelError> {
    let Some(stem) = path.file_name() else {
        return Ok(());
    };
    let prefix = format!("{}{RESTORE_INFIX}", stem.to_string_lossy());
    // Enumeration and every removal are relative to the held root descriptor, so
    // a candidate renamed and replaced by a symlink after enumeration cannot
    // redirect an unlink outside this directory.
    let members = family_member_names(path).ok_or(KernelError::Inconclusive)?;
    let entries = rfs::Dir::read_from(parent_dir).map_err(|_| KernelError::Inconclusive)?;
    let mut reaped = false;
    for entry in entries {
        let entry = entry.map_err(|_| KernelError::Inconclusive)?;
        let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string();
        if !generated_recovery_suffix(&name, &prefix) {
            continue;
        }
        let Ok(candidate) = rfs::openat(
            parent_dir,
            &name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) else {
            continue;
        };
        let candidate = File::from(candidate);
        for member in &members {
            match rfs::unlinkat(&candidate, member.as_os_str(), AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(_) => return Err(KernelError::Inconclusive),
            }
        }
        drop(candidate);
        if rfs::unlinkat(parent_dir, &name, AtFlags::REMOVEDIR).is_err() {
            continue;
        }
        reaped = true;
    }
    if reaped {
        durable_fs::sync_directory(parent_dir).map_err(|_| KernelError::Io)?;
    }
    remove_restore_scratch(path, parent_dir)
}

// Enumeration, the type check, and the unlink all run relative to the held root
// descriptor, so a root pathname pointing at another store cannot have its
// scratch files removed by this store's recovery.
fn remove_restore_scratch(path: &Path, root: &File) -> Result<(), KernelError> {
    let Some(stem) = path.file_name() else {
        return Ok(());
    };
    let prefix = format!("{}.restore-", stem.to_string_lossy());
    let entries = rfs::Dir::read_from(root).map_err(|_| KernelError::Inconclusive)?;
    for entry in entries {
        let entry = entry.map_err(|_| KernelError::Inconclusive)?;
        let name = entry.file_name();
        let Some(middle) = name
            .to_str()
            .ok()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".tmp"))
        else {
            continue;
        };
        // `restore_temp_path` writes only decimal digits here, so anything else in
        // the store root belongs to someone else.
        if middle.is_empty() || !middle.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if !regular_file_present(root, std::ffi::OsStr::from_bytes(name.to_bytes())) {
            continue;
        }
        rfs::unlinkat(root, name, AtFlags::empty()).map_err(|_| KernelError::Inconclusive)?;
    }
    Ok(())
}

/// The marker's entry name inside the store root.
fn restore_marker_name(path: &Path) -> Result<String, KernelError> {
    restore_marker_path(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or(KernelError::Io)
}

// The marker is written and published relative to the held root descriptor, so
// a root pathname pointing elsewhere while a restore runs cannot receive it.
fn publish_restore_marker(path: &Path, recovery: &RecoveryDir) -> Result<(), KernelError> {
    let mut marker = RestoreMarker {
        protocol: RESTORE_MARKER_PROTOCOL.to_string(),
        database_path: path_bytes(path),
        recovery_directory: path_bytes(&recovery.path),
        marker_digest: String::new(),
    };
    marker.marker_digest = restore_marker_digest(&marker);
    let marker_name = restore_marker_name(path)?;
    let temp_name = format!("{marker_name}.{}.tmp", next_unique_id());
    let bytes = serde_json::to_vec(&marker).map_err(|_| KernelError::Io)?;
    let mut file = create_new_file(&recovery.root, &temp_name).map_err(|_| KernelError::Io)?;
    if write_and_sync(&mut file, &bytes).is_err() {
        drop(file);
        let _ = rfs::unlinkat(&recovery.root, &temp_name, AtFlags::empty());
        return Err(KernelError::Io);
    }
    drop(file);
    if rfs::renameat(&recovery.root, &temp_name, &recovery.root, &marker_name).is_err() {
        let _ = rfs::unlinkat(&recovery.root, &temp_name, AtFlags::empty());
        return Err(KernelError::Io);
    }
    durable_fs::sync_directory(&recovery.root).map_err(|_| KernelError::Io)
}

fn remove_restore_marker(path: &Path, root: &File) -> Result<(), KernelError> {
    let marker_name = restore_marker_name(path)?;
    match rfs::unlinkat(root, &marker_name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => {}
        Err(_) => return Err(KernelError::Io),
    }
    durable_fs::sync_directory(root).map_err(|_| KernelError::Io)
}

// Best effort: a member that cannot be removed leaves the directory for
// `reap_orphan_restore_recovery` on the next open.
fn cleanup_recovery_dir(recovery: &RecoveryDir) {
    for entry in rfs::Dir::read_from(&recovery.dir).into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let _ = rfs::unlinkat(&recovery.dir, name, AtFlags::empty());
    }
    if rfs::unlinkat(&recovery.root, &recovery.name, AtFlags::REMOVEDIR).is_ok() {
        let _ = durable_fs::sync_directory(&recovery.root);
    }
}

fn open_private_regular_nofollow(path: &Path) -> Result<File, KernelError> {
    // `NONBLOCK` avoids blocking on a FIFO before the later type check; it is inert for regular files.
    let fd = rfs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| KernelError::InvalidRestore)?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| KernelError::InvalidRestore)?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || !owner_is_current(metadata.uid()) {
        return Err(KernelError::InvalidRestore);
    }
    Ok(file)
}

fn open_live_family(
    path: &Path,
    lease_epoch: u64,
    expected_seq: i64,
    reader_count: usize,
) -> Result<(Connection, Vec<Connection>), KernelError> {
    let mut writer = open_writer(path).map_err(|_| KernelError::InvalidRestore)?;
    apply_preclassification_profile(&writer).map_err(|_| KernelError::InvalidRestore)?;
    verify_exact_identity(&mut writer).map_err(|_| KernelError::InvalidRestore)?;
    let actual_seq = writer
        .query_row(
            "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| KernelError::InvalidRestore)?;
    if actual_seq != expected_seq {
        return Err(KernelError::InvalidRestore);
    }
    activate_wal(&writer)?;
    stamp_writer_fence(&mut writer, lease_epoch)?;
    super::envelope::strip_legacy_candidate_verifiers(&mut writer)?;
    harden_family(path)?;
    let readers = (0..reader_count)
        .map(|_| open_reader(path))
        .collect::<Result<Vec<_>, _>>()?;
    harden_family(path)?;
    Ok((writer, readers))
}

/// The directory a restore displaces the live family into, held open together
/// with the store root so every move between them names a descriptor rather
/// than a pathname that a concurrent swap could redirect.
struct RecoveryDir {
    path: PathBuf,
    name: std::ffi::OsString,
    root: File,
    dir: File,
}

impl RecoveryDir {
    fn create(path: &Path, root: File) -> Result<Self, KernelError> {
        for _ in 0..10_000 {
            let candidate = suffix_path(path, &format!("{RESTORE_INFIX}{}", next_unique_id()));
            let name = candidate.file_name().ok_or(KernelError::Io)?.to_os_string();
            match create_secure_directory(&root, &name) {
                Ok(dir) => {
                    return Ok(Self {
                        path: candidate,
                        name,
                        root,
                        dir,
                    });
                }
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::EXIST.raw_os_error()) =>
                {
                    continue;
                }
                Err(_) => return Err(KernelError::Io),
            }
        }
        Err(KernelError::Io)
    }

    /// Opens an existing recovery directory named by a restore marker. The
    /// directory must be a real owner-only directory, not a symlink to one.
    fn open(root: File, recovery_path: PathBuf) -> Result<Self, KernelError> {
        let name = recovery_path
            .file_name()
            .ok_or(KernelError::Inconclusive)?
            .to_os_string();
        let dir = open_secure_directory(&root, name.to_str().ok_or(KernelError::Inconclusive)?)
            .map_err(|_| KernelError::Inconclusive)?;
        Ok(Self {
            path: recovery_path,
            name,
            root,
            dir,
        })
    }
}

/// The main database file name followed by its sidecar names.
fn family_member_names(path: &Path) -> Option<Vec<std::ffi::OsString>> {
    let mut members = vec![path.file_name()?.to_os_string()];
    for sidecar in family_sidecars(path) {
        members.push(sidecar.file_name()?.to_os_string());
    }
    Some(members)
}

/// Whether `name` is a regular file directly inside `directory`; a symlink or
/// a vanished entry reads as absent.
fn regular_file_present(directory: &File, name: &std::ffi::OsStr) -> bool {
    rfs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| rfs::FileType::from_raw_mode(stat.st_mode).is_file())
}

// Sidecars move before the main file, so the main file's presence in the
// recovery directory means the whole family is there.
fn displace_family(path: &Path, recovery: &RecoveryDir) -> Result<(), KernelError> {
    let members = family_member_names(path).ok_or(KernelError::Io)?;
    for name in members.iter().rev() {
        if regular_file_present(&recovery.root, name) {
            rfs::renameat(&recovery.root, name, &recovery.dir, name)
                .map_err(|_| KernelError::Io)?;
        }
    }
    durable_fs::sync_directory(&recovery.dir).map_err(|_| KernelError::Io)?;
    durable_fs::sync_directory(&recovery.root).map_err(|_| KernelError::Io)
}

// The main file moves first; its presence in the recovery directory means no
// member has been restored. Re-running after a crash then skips members already
// moved back instead of deleting them, keeping a partial rollback idempotent.
fn restore_displaced_family(path: &Path, recovery: &RecoveryDir) -> Result<(), KernelError> {
    let members = family_member_names(path).ok_or(KernelError::Io)?;
    for name in &members {
        if regular_file_present(&recovery.dir, name) {
            rfs::renameat(&recovery.dir, name, &recovery.root, name)
                .map_err(|_| KernelError::Io)?;
        }
    }
    durable_fs::sync_directory(&recovery.dir).map_err(|_| KernelError::Io)?;
    durable_fs::sync_directory(&recovery.root).map_err(|_| KernelError::Io)
}

fn remove_family(path: &Path, root: &File) -> Result<(), KernelError> {
    let members = family_member_names(path).ok_or(KernelError::Io)?;
    for name in members.iter().rev() {
        match rfs::unlinkat(root, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(KernelError::Io),
        }
    }
    durable_fs::sync_directory(root).map_err(|_| KernelError::Io)
}

/// Unlinks the staged copy, named relative to the held root, unless disarmed.
struct StagedRestore<'a>(Option<(&'a File, &'a std::ffi::OsStr)>);

impl StagedRestore<'_> {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for StagedRestore<'_> {
    fn drop(&mut self) {
        if let Some((root, name)) = self.0 {
            let _ = rfs::unlinkat(root, name, AtFlags::empty());
        }
    }
}

fn restore_temp_path(path: &Path) -> PathBuf {
    suffix_path(path, &format!(".restore-{}.tmp", next_unique_id()))
}

/// Copies `source` into a fresh owner-only file named `name` inside `root` and
/// returns the descriptor the bytes were written through, which identifies the
/// staged file independently of the pathname.
fn copy_to_private_temp(source: &mut File, root: &File, name: &str) -> Result<File, KernelError> {
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| KernelError::Io)?;
    let mut target = durable_fs::create_new_file_rw(root, name).map_err(|_| KernelError::Io)?;
    if std::io::copy(source, &mut target)
        .and_then(|_| target.sync_all())
        .is_err()
    {
        drop(target);
        let _ = rfs::unlinkat(root, name, AtFlags::empty());
        return Err(KernelError::Io);
    }
    Ok(target)
}
