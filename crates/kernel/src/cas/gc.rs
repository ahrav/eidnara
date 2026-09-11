//! Crash-resumable garbage collection for content-addressed artifact objects.
//!
//! Reclamation first records durable `Reclaiming` state under the writer lock,
//! unlinks the object while retaining that lock, then removes reclaim metadata.
//! Times passed to this module are Unix epoch milliseconds. Referenced artifacts
//! receive a 14-day grace period; unreferenced filesystem orphans receive one hour.

use std::collections::BTreeMap;
use std::fs::File;

use rusqlite::{TransactionBehavior, params};
use rustix::fs::{self as rfs, AtFlags};

use super::ingest::is_dot_entry;
use super::is_artifact_digest;
use crate::durable_fs::{StorageError, durable_unlink};
use crate::envelope::check_fence;
use crate::{KernelError, KernelStore};

const HOUR_MS: i64 = 60 * 60 * 1_000;
/// The result of probing one object file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectPresence {
    Present,
    /// The shard or the file does not exist, or the path is not a regular file.
    Absent,
    /// The probe failed for a reason other than absence.
    Unreadable,
}

pub(crate) const REFERENCED_GRACE_MS: i64 = 14 * 24 * HOUR_MS;
const ORPHAN_GRACE_MS: i64 = HOUR_MS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactGcResult {
    pub reclaimed_objects: usize,
    pub reclaimed_bytes: u64,
    pub failed_candidates: usize,
    /// Candidates kept only by the consumer horizon. Retention ends when every
    /// consumer checkpoint passes the citing descriptor.
    pub withheld_for_replay: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Withheld {
    /// A live reference, active pin, live reservation, or unexpired grace.
    Retained,
    /// A citing source descriptor is newer than the least advanced consumer checkpoint.
    UnacknowledgedReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reclaim {
    Removed(u64),
    Withheld(Withheld),
    /// Eligible, but the object file was already unlinked by another pass.
    AlreadyGone,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactGcFault {
    AfterReclaiming,
    /// Another opener raises the durable fence between the eligibility
    /// decision and the unlink.
    FenceRaisedBeforeUnlink,
    Unlink,
    AfterUnlink,
}

/// Injection flags carried through a reclaim pass so the private path keeps one
/// signature in every feature configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GcFaults {
    pub after_reclaiming: bool,
    pub fence_raised_before_unlink: bool,
    pub unlink: bool,
    pub after_unlink: bool,
}

#[cfg(feature = "test-support")]
impl From<ArtifactGcFault> for GcFaults {
    fn from(fault: ArtifactGcFault) -> Self {
        Self {
            after_reclaiming: fault == ArtifactGcFault::AfterReclaiming,
            fence_raised_before_unlink: fault == ArtifactGcFault::FenceRaisedBeforeUnlink,
            unlink: fault == ArtifactGcFault::Unlink,
            after_unlink: fault == ArtifactGcFault::AfterUnlink,
        }
    }
}

#[derive(Clone)]
struct Candidate {
    digest: String,
    modified_at: Option<i64>,
}

impl KernelStore {
    /// Normalizes reservations left behind by a writer that died mid-ingest, and
    /// re-arms the unlink of any purged digest whose bytes are still present.
    pub(crate) fn prepare_startup_cas_recovery(&self, now: i64) -> Result<(), KernelError> {
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&tx, self.lease_epoch())?;
        tx.execute(
            "DELETE FROM artifact_ingestion_reservations
             WHERE state='Live' AND EXISTS(
                 SELECT 1 FROM evidence_meta e
                 WHERE e.artifact_digest=artifact_ingestion_reservations.artifact_digest
                   AND e.invalidated_commit_seq IS NULL
             )",
            [],
        )
        .map_err(|_| KernelError::Io)?;
        tx.execute(
            "UPDATE artifact_ingestion_reservations
             SET state='Reclaiming',reclaim_started_at=heartbeat_at
             WHERE state='Live'
               AND NOT EXISTS(
                   SELECT 1 FROM evidence_meta e
                   WHERE e.artifact_digest=artifact_ingestion_reservations.artifact_digest
               )",
            [],
        )
        .map_err(|_| KernelError::Io)?;
        // Anything still live here has invalidated references only. The candidate
        // snapshot selects reclaim state or bytes on disk, so a reservation with
        // neither is unreachable; retiring it protects no fewer bytes because it
        // has none, and leaves reference history for `prepare_reclaim` to weigh.
        let unreachable = {
            let mut statement = tx
                .prepare(
                    "SELECT reservation_id,artifact_digest FROM artifact_ingestion_reservations
                     WHERE state='Live'",
                )
                .map_err(|_| KernelError::Io)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| KernelError::Io)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| KernelError::Io)?;
            rows.into_iter()
                .filter(|(_, digest)| {
                    is_artifact_digest(digest) && !self.artifact_object_is_present(digest)
                })
                .map(|(reservation_id, _)| reservation_id)
                .collect::<Vec<_>>()
        };
        for reservation_id in unreachable {
            tx.execute(
                "DELETE FROM artifact_ingestion_reservations WHERE reservation_id=?1",
                [&reservation_id],
            )
            .map_err(|_| KernelError::Io)?;
        }
        // A tombstone records a purge whose bytes must be gone, yet the tree this
        // process serves can still hold them: a restored database carries the purge
        // history of another store, and a failed ingest leaves its staging temp
        // behind. A digest with bytes in either place is re-armed as a pending
        // unlink so the purge-completion path removes the object and sweeps its
        // temps.
        let staged_digests = self
            .digests_with_staging_temps()
            .map_err(|_| KernelError::Io)?;
        let orphaned_tombstones = {
            let mut statement = tx
                .prepare(
                    "SELECT t.artifact_digest,t.artifact_reference FROM artifact_purge_tombstones t
                     WHERE NOT EXISTS(
                         SELECT 1 FROM artifact_pending_unlinks p
                         WHERE p.artifact_digest=t.artifact_digest
                     )",
                )
                .map_err(|_| KernelError::Io)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| KernelError::Io)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| KernelError::Io)?;
            rows.into_iter()
                .filter(|(digest, _)| {
                    is_artifact_digest(digest)
                        && (staged_digests.contains(digest)
                            || self.artifact_object_is_present(digest))
                })
                .collect::<Vec<_>>()
        };
        for (digest, reference) in orphaned_tombstones {
            tx.execute(
                "INSERT INTO artifact_pending_unlinks(artifact_digest,artifact_reference,created_at)
                 VALUES (?1,?2,?3)",
                params![digest, reference, now],
            )
            .map_err(|_| KernelError::Io)?;
        }
        tx.commit().map_err(|_| KernelError::Io)
    }

    /// Scans artifact objects and attempts every candidate eligible at `now`.
    ///
    /// `now` is Unix epoch milliseconds. Fence loss and injected faults stop the
    /// pass. Other candidate failures increment `failed_candidates` and collection
    /// continues. `reclaimed_bytes` saturates at [`u64::MAX`].
    pub(crate) fn run_artifact_gc(
        &self,
        now: i64,
        hook: Option<&mut dyn FnMut()>,
        faults: GcFaults,
    ) -> Result<ArtifactGcResult, KernelError> {
        let candidates = self.snapshot_gc_candidates()?;
        if let Some(hook) = hook {
            hook();
        }
        let mut result = ArtifactGcResult::default();
        for candidate in candidates {
            match self.reclaim_candidate(&candidate, now, faults) {
                Ok(Reclaim::Removed(bytes)) => {
                    result.reclaimed_objects += 1;
                    result.reclaimed_bytes = result.reclaimed_bytes.saturating_add(bytes);
                }
                Ok(Reclaim::Withheld(Withheld::UnacknowledgedReplay)) => {
                    result.withheld_for_replay += 1;
                }
                Ok(Reclaim::Withheld(Withheld::Retained) | Reclaim::AlreadyGone) => {}
                Err(error @ (KernelError::FenceLost | KernelError::Fault)) => return Err(error),
                Err(_) => result.failed_candidates += 1,
            }
        }
        Ok(result)
    }

    fn reclaim_candidate(
        &self,
        candidate: &Candidate,
        now: i64,
        faults: GcFaults,
    ) -> Result<Reclaim, KernelError> {
        let mut writer = self.lock_writer()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&tx, self.lease_epoch())?;
        let resuming_purge: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM artifact_pending_unlinks WHERE artifact_digest=?1)",
                [&candidate.digest],
                |row| row.get(0),
            )
            .map_err(|_| KernelError::Io)?;
        // Retained bytes cancel reclamation; absent bytes still require metadata cleanup.
        if let Some(withheld) = prepare_reclaim(&tx, candidate, now, self.lease_epoch())?
            && self.artifact_object_presence(&candidate.digest) != ObjectPresence::Absent
        {
            delete_reclaiming_reservations(&tx, &candidate.digest)?;
            tx.commit().map_err(|_| KernelError::Io)?;
            return Ok(Reclaim::Withheld(withheld));
        }
        tx.commit().map_err(|_| KernelError::Io)?;

        if faults.after_reclaiming {
            return Err(KernelError::Fault);
        }
        if faults.fence_raised_before_unlink {
            writer
                .execute(
                    "UPDATE writer_fence SET writer_epoch=writer_epoch+1 WHERE id=0",
                    [],
                )
                .map_err(|_| KernelError::Io)?;
        }
        if faults.unlink {
            return Err(self.latch_gc_failure());
        }
        // The unlink runs inside a fenced write transaction. The writer guard alone
        // is process-local: another opener whose lease has raised the durable fence
        // could restore a backup with a live reference for this digest between the
        // eligibility decision and the unlink. Holding the database write lock
        // from a fence check that passed until the reclaim rows are removed leaves
        // no such window, and a fence raised since is seen before the bytes go.
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| KernelError::Io)?;
        check_fence(&tx, self.lease_epoch())?;
        let (removed, bytes) = self.unlink_artifact(&candidate.digest)?;
        if faults.after_unlink {
            return Err(KernelError::Fault);
        }
        if resuming_purge {
            self.sweep_digest_temps(&candidate.digest)
                .map_err(|error| self.map_gc_storage_error(error))?;
        }
        delete_reclaiming_reservations(&tx, &candidate.digest)?;
        tx.execute(
            "DELETE FROM artifact_pending_unlinks WHERE artifact_digest=?1",
            [&candidate.digest],
        )
        .map_err(|_| KernelError::Io)?;
        tx.execute(
            "DELETE FROM capture_pin_refs
             WHERE released_at IS NOT NULL AND evidence_id IN (
                 SELECT evidence_id FROM evidence_meta WHERE artifact_digest=?1
             )",
            [&candidate.digest],
        )
        .map_err(|_| KernelError::Io)?;
        tx.commit().map_err(|_| KernelError::Io)?;
        Ok(if removed {
            Reclaim::Removed(bytes)
        } else {
            Reclaim::AlreadyGone
        })
    }

    /// Resumes durable reclaim rows without scanning the object tree and returns
    /// how many pending purge unlinks it could not complete.
    ///
    /// `now` is Unix epoch milliseconds. The writer connection avoids taking a
    /// read-pool snapshot while opening a store. Invalid digests and ordinary
    /// failures on abandoned reservations are skipped: those bytes are garbage
    /// the next GC pass collects. A pending purge whose unlink fails keeps its
    /// row for the next attempt and is counted, so the caller can decide whether
    /// still-readable purged content is a failure. Fence loss stops recovery.
    pub(crate) fn run_artifact_recovery(&self, now: i64) -> Result<usize, KernelError> {
        // Promotion runs first: it moves reservations abandoned by a dead writer
        // into `Reclaiming`, which is the state the snapshot below collects.
        self.prepare_startup_cas_recovery(now)?;
        let candidates = {
            let writer = self.lock_writer()?;
            let mut statement = writer
                .prepare(
                    "SELECT artifact_digest,0 FROM artifact_ingestion_reservations
                       WHERE state='Reclaiming'
                       AND artifact_digest NOT IN (SELECT artifact_digest FROM artifact_pending_unlinks)
                     UNION SELECT artifact_digest,1 FROM artifact_pending_unlinks",
                )
                .map_err(|_| KernelError::Io)?;
            let candidates = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
                })
                .map_err(|_| KernelError::Io)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| KernelError::Io)?;
            drop(statement);
            candidates
        };
        let mut unfinished_purges = 0usize;
        for (digest, pending_purge) in candidates {
            if !is_artifact_digest(&digest) {
                continue;
            }
            let candidate = Candidate {
                digest,
                modified_at: None,
            };
            match self.reclaim_candidate(&candidate, now, GcFaults::default()) {
                Ok(_) => {}
                Err(error @ (KernelError::FenceLost | KernelError::Fault)) => return Err(error),
                Err(_) if pending_purge => unfinished_purges += 1,
                Err(_) => {}
            }
        }
        Ok(unfinished_purges)
    }

    fn snapshot_reclaim_state(&self) -> Result<Vec<Candidate>, KernelError> {
        let reader = self.lock_reader()?;
        let mut statement = reader
            .prepare(
                "SELECT artifact_digest FROM artifact_ingestion_reservations
                   WHERE state='Reclaiming'
                 UNION SELECT artifact_digest FROM artifact_pending_unlinks",
            )
            .map_err(|_| KernelError::Io)?;
        let digests = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| KernelError::Io)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| KernelError::Io)?;
        Ok(digests
            .into_iter()
            .filter(|digest| is_artifact_digest(digest))
            .map(|digest| Candidate {
                digest,
                modified_at: None,
            })
            .collect())
    }

    fn snapshot_gc_candidates(&self) -> Result<Vec<Candidate>, KernelError> {
        let reclaim_state = self.snapshot_reclaim_state()?;
        // Reclaiming needs bytes to unlink or durable reclaim state to retire.
        // `evidence_meta` outlives both, and `prepare_reclaim` rechecks liveness per
        // candidate, so a pass costs the object scan plus outstanding reclaim rows
        // rather than the whole reference history.
        let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
        for candidate in reclaim_state {
            candidates.insert(candidate.digest.clone(), candidate);
        }
        for object in self.scan_objects()? {
            candidates
                .entry(object.digest.clone())
                .and_modify(|candidate| candidate.modified_at = object.modified_at)
                .or_insert(object);
        }
        Ok(candidates.into_values().collect())
    }

    /// Returns `false` only when the object is positively absent. A read failure
    /// returns `true` so recovery does not delete a reservation whose shard is
    /// merely unreadable.
    fn artifact_object_is_present(&self, digest: &str) -> bool {
        self.artifact_object_presence(digest) != ObjectPresence::Absent
    }

    /// Whether the object file for `digest` is on disk. `digest` must satisfy
    /// `is_artifact_digest`; callers that need "positively present" and
    /// callers that need "not positively absent" read different arms.
    pub(crate) fn artifact_object_presence(&self, digest: &str) -> ObjectPresence {
        match self.shard_directory(digest, false) {
            Ok(Some(shard)) => {
                match rfs::statat(&*shard, &digest[2..], AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) if rfs::FileType::from_raw_mode(stat.st_mode).is_file() => {
                        ObjectPresence::Present
                    }
                    Ok(_) | Err(rustix::io::Errno::NOENT) => ObjectPresence::Absent,
                    Err(_) => ObjectPresence::Unreadable,
                }
            }
            Ok(None) => ObjectPresence::Absent,
            Err(_) => ObjectPresence::Unreadable,
        }
    }

    fn unlink_artifact(&self, digest: &str) -> Result<(bool, u64), KernelError> {
        let shard = match self.shard_directory(digest, false) {
            Ok(Some(shard)) => shard,
            Ok(None) => return Ok((false, 0)),
            Err(error) => return Err(self.map_gc_storage_error(error)),
        };
        let stat = match rfs::statat(&shard, &digest[2..], AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok((false, 0)),
            Err(_) => return Err(self.latch_gc_failure()),
        };
        if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
            return Err(self.latch_gc_failure());
        }
        let byte_length = u64::try_from(stat.st_size).map_err(|_| self.latch_gc_failure())?;
        durable_unlink(&shard, &digest[2..]).map_err(|error| self.map_gc_storage_error(error))?;
        Ok((true, byte_length))
    }

    fn map_gc_storage_error(&self, error: StorageError) -> KernelError {
        if matches!(error, StorageError::Other(_)) {
            self.latch_cas_failure();
        }
        KernelError::Io
    }

    fn latch_gc_failure(&self) -> KernelError {
        self.latch_cas_failure();
        KernelError::Io
    }
}

fn delete_reclaiming_reservations(
    tx: &rusqlite::Transaction<'_>,
    digest: &str,
) -> Result<(), KernelError> {
    tx.execute(
        "DELETE FROM artifact_ingestion_reservations
         WHERE artifact_digest=?1 AND state='Reclaiming'",
        [digest],
    )
    .map_err(|_| KernelError::Io)?;
    Ok(())
}

/// Only reservations from the current `lease_epoch` block reclamation; earlier
/// epochs do not extend the reference grace period.
fn prepare_reclaim(
    tx: &rusqlite::Transaction<'_>,
    candidate: &Candidate,
    now: i64,
    lease_epoch: u64,
) -> Result<Option<Withheld>, KernelError> {
    let pending_purge: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM artifact_pending_unlinks WHERE artifact_digest=?1)",
            [&candidate.digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    let reclaiming: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM artifact_ingestion_reservations
                           WHERE artifact_digest=?1 AND state='Reclaiming')",
            [&candidate.digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    if pending_purge {
        return Ok(None);
    }

    let live_reference: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence_meta
                           WHERE artifact_digest=?1 AND invalidated_commit_seq IS NULL)",
            [&candidate.digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    if live_reference {
        return Ok(Some(Withheld::Retained));
    }
    let active_pin: bool = tx
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM capture_pin_refs r
                 JOIN capture_pins p USING(capture_pin_id)
                 JOIN evidence_meta e USING(evidence_id)
                 WHERE e.artifact_digest=?1
                   AND COALESCE(r.released_at,p.released_at) IS NULL
             )",
            [&candidate.digest],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    if active_pin {
        return Ok(Some(Withheld::Retained));
    }
    // Reclaiming carries the grace decision across a crash; recovery has no mtime.
    if reclaiming {
        return Ok(has_unacknowledged_replay(tx, &candidate.digest)?
            .then_some(Withheld::UnacknowledgedReplay));
    }

    let writer_epoch = i64::try_from(lease_epoch).map_err(|_| KernelError::InvalidInput)?;
    let live_reservation_expires_at: Option<i64> = tx
        .query_row(
            "SELECT MAX(lease_expires_at) FROM artifact_ingestion_reservations
             WHERE artifact_digest=?1 AND state='Live' AND writer_epoch=?2",
            params![candidate.digest, writer_epoch],
            |row| row.get(0),
        )
        .map_err(|_| KernelError::Io)?;
    if live_reservation_expires_at.is_some_and(|expires_at| now < expires_at) {
        return Ok(Some(Withheld::Retained));
    }

    let (invalidated_at, retain_until, pin_released_at): (Option<i64>, Option<i64>, Option<i64>) =
        tx.query_row(
            "SELECT MAX(c.recorded_at),MAX(e.retain_until),
                    MAX(COALESCE(r.released_at,p.released_at))
             FROM evidence_meta e
             LEFT JOIN commit_log c ON c.commit_seq=e.invalidated_commit_seq
             LEFT JOIN capture_pin_refs r ON r.evidence_id=e.evidence_id
             LEFT JOIN capture_pins p ON p.capture_pin_id=r.capture_pin_id
             WHERE e.artifact_digest=?1",
            [&candidate.digest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| KernelError::Io)?;
    if invalidated_at.is_some() || pin_released_at.is_some() {
        let mut deadline = retain_until;
        for timestamp in [invalidated_at, pin_released_at].into_iter().flatten() {
            let Some(grace_deadline) = timestamp.checked_add(REFERENCED_GRACE_MS) else {
                return Ok(Some(Withheld::Retained));
            };
            deadline = Some(deadline.map_or(grace_deadline, |value| value.max(grace_deadline)));
        }
        if deadline.is_some_and(|deadline| now < deadline) {
            return Ok(Some(Withheld::Retained));
        }
    } else if live_reservation_expires_at.is_none() {
        let Some(modified_at) = candidate.modified_at else {
            return Ok(Some(Withheld::Retained));
        };
        if !elapsed(now, modified_at, ORPHAN_GRACE_MS) {
            return Ok(Some(Withheld::Retained));
        }
    }
    if has_unacknowledged_replay(tx, &candidate.digest)? {
        return Ok(Some(Withheld::UnacknowledgedReplay));
    }

    let changed = tx
        .execute(
            "UPDATE artifact_ingestion_reservations
             SET state='Reclaiming',reclaim_started_at=?1
             WHERE artifact_digest=?2 AND state='Live'",
            params![now, candidate.digest],
        )
        .map_err(|_| KernelError::Io)?;
    if changed == 0 {
        tx.execute(
            "INSERT INTO artifact_ingestion_reservations(
                 reservation_id,artifact_digest,artifact_reference,state,writer_epoch,
                 created_at,heartbeat_at,lease_expires_at,reclaim_started_at
             ) VALUES (?1,?2,?3,'Reclaiming',?4,?5,?5,?5,?5)",
            params![
                format!("gc-{}", candidate.digest),
                candidate.digest,
                format!(
                    "objects/{}/{}",
                    &candidate.digest[..2],
                    &candidate.digest[2..]
                ),
                i64::try_from(lease_epoch).map_err(|_| KernelError::InvalidInput)?,
                now,
            ],
        )
        .map_err(|_| KernelError::Io)?;
    }
    Ok(None)
}

fn has_unacknowledged_replay(
    tx: &rusqlite::Transaction<'_>,
    digest: &str,
) -> Result<bool, KernelError> {
    // An absent consumer set gives no replay horizon, so descriptor evidence is kept.
    tx.query_row(
        &format!(
            "SELECT EXISTS(
                 SELECT 1 FROM evidence_meta e
                 JOIN observations b ON b.evidence_id=e.evidence_id
                 WHERE e.artifact_digest=?1
                   AND b.observation_kind='{}'
                   AND b.created_commit_seq>COALESCE(
                       (SELECT MIN(checkpoint_commit_seq) FROM outbox_consumers),-1)
             )",
            crate::source_descriptor::SOURCE_DESCRIPTOR_KIND
        ),
        [digest],
        |row| row.get(0),
    )
    .map_err(|_| KernelError::Io)
}

fn elapsed(now: i64, since: i64, duration: i64) -> bool {
    now.checked_sub(since).is_some_and(|age| age >= duration)
}

impl KernelStore {
    /// Enumerates stored objects through the `objects` descriptor and each
    /// shard's held descriptor, so a same-UID swap or rename of a path component
    /// cannot inject candidates: a shard the store already holds is scanned under
    /// the name it was created with, and a name that reaches a held shard's inode
    /// is skipped. Entries that vanish or fail to stat mid-scan are skipped:
    /// reclamation can unlink concurrently, and `prepare_reclaim` rechecks every
    /// candidate it acts on.
    fn scan_objects(&self) -> Result<Vec<Candidate>, KernelError> {
        let mut found = Vec::new();
        for shard in rfs::Dir::read_from(&self.objects_directory).map_err(|_| KernelError::Io)? {
            let Ok(shard) = shard else { continue };
            let shard_name = shard.file_name();
            if is_dot_entry(shard_name) {
                continue;
            }
            let Some(prefix) = shard_name.to_str().ok().map(str::to_owned) else {
                continue;
            };
            if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let Ok(Some(shard)) = self.shard_directory(&prefix, false) else {
                continue;
            };
            scan_shard(&shard, &prefix, &mut found);
        }
        Ok(found)
    }
}

/// Appends every regular file below `shard` whose name completes a digest under `prefix`.
fn scan_shard(shard: &File, prefix: &str, found: &mut Vec<Candidate>) {
    let Ok(entries) = rfs::Dir::read_from(shard) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if is_dot_entry(name) {
            continue;
        }
        let Ok(stat) = rfs::statat(shard, name, AtFlags::SYMLINK_NOFOLLOW) else {
            continue;
        };
        let Some(suffix) = name.to_str().ok() else {
            continue;
        };
        let digest = format!("{prefix}{suffix}");
        if rfs::FileType::from_raw_mode(stat.st_mode).is_file() && is_artifact_digest(&digest) {
            found.push(Candidate {
                digest,
                modified_at: stat_modified_ms(&stat),
            });
        }
    }
}

/// Milliseconds since the Unix epoch of the file's last modification, or `None` when the timestamp does not fit.
fn stat_modified_ms(stat: &rfs::Stat) -> Option<i64> {
    // The field widths differ across Linux targets, so both widen into `i128`.
    let seconds = i128::from(stat.st_mtime);
    let nanos = i128::from(stat.st_mtime_nsec);
    i64::try_from(seconds * 1_000 + nanos / 1_000_000).ok()
}

/// Returns total bytes occupied by regular files under the artifact object
/// tree, or `None` once `cancelled` returns true.
pub(crate) fn object_usage(
    store: &KernelStore,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<u64>, KernelError> {
    super::ingest::regular_file_bytes(&store.objects_directory, cancelled)
        .map_err(|_| KernelError::Io)
}
