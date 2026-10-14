//! Artifact usage: the `st_size` sum of regular files below `objects`.
//!
//! The store walks the tree when it opens, before startup recovery, and then
//! keeps the count current under the writer lock: every publication adds the
//! bytes it placed and every unlink subtracts the bytes it removed, so
//! admission and the health sampler read the count without walking. The walk
//! remains the reconciliation. Recovery, including the startup recovery the
//! first walk precedes, and maintenance re-walk the tree and compare; a count
//! that disagrees with the walk latches the fail-closed CAS state, so the next
//! admission is refused.

use std::fs::File;

use rustix::fs::{self as rfs, AtFlags};

use super::ingest::regular_file_bytes;
use crate::durable_fs::{StorageError, durable_unlink};
use crate::{KernelError, KernelStore};

impl KernelStore {
    /// The counted usage, walking the tree only when no count is held: before
    /// the first walk succeeded or after an unlink whose outcome is unknown.
    /// The caller holds the writer lock, so no publication or unlink races the
    /// walk it may take.
    pub(super) fn counted_artifact_usage(&self) -> Result<u64, StorageError> {
        if let Some(bytes) = *self.counted_usage() {
            return Ok(bytes);
        }
        let walked = self.walk_artifact_usage()?;
        *self.counted_usage() = Some(walked);
        Ok(walked)
    }

    /// Adds the bytes a publication placed below `objects`. The caller holds
    /// the writer lock.
    pub(super) fn count_published_artifact(&self, byte_length: u64) {
        if let Some(bytes) = self.counted_usage().as_mut() {
            *bytes = bytes.saturating_add(byte_length);
        }
    }

    /// Durably unlinks the object `name` in `shard` and subtracts the bytes the
    /// walk counted for it. The caller holds the writer lock. An unlink that
    /// fails leaves the count unknown, so the next reader walks; a removal
    /// larger than the count is drift and latches the fail-closed state.
    pub(super) fn unlink_counted_object(
        &self,
        shard: &File,
        name: &str,
    ) -> Result<(), StorageError> {
        let removed = match rfs::statat(shard, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if rfs::FileType::from_raw_mode(stat.st_mode).is_file() => {
                Some(u64::try_from(stat.st_size).unwrap_or(0))
            }
            Ok(_) | Err(rustix::io::Errno::NOENT) => Some(0),
            Err(_) => None,
        };
        let unlinked = durable_unlink(shard, name);
        let mut usage = self.counted_usage();
        match (unlinked.is_ok(), removed, *usage) {
            (true, Some(removed), Some(bytes)) => match bytes.checked_sub(removed) {
                Some(remaining) => *usage = Some(remaining),
                None => {
                    drop(usage);
                    self.latch_cas_failure();
                }
            },
            (_, _, None) => {}
            _ => *usage = None,
        }
        unlinked
    }

    /// Re-walks the tree under the writer lock and compares the walk with the
    /// count. With no count held the walk becomes the count. A disagreement is
    /// drift: the count stays as it was and the fail-closed state latches. A
    /// walk that fails for a reason other than exhaustion latches it too; an
    /// exhausted walk leaves the count as it was for the next reader.
    pub(crate) fn reconcile_artifact_usage(&self) {
        let Ok(_writer) = self.lock_writer() else {
            return;
        };
        let walked = match self.walk_artifact_usage() {
            Ok(walked) => walked,
            Err(StorageError::Exhausted(_)) => return,
            Err(StorageError::Other(_)) => return self.latch_cas_failure(),
        };
        let mut usage = self.counted_usage();
        match *usage {
            None => *usage = Some(walked),
            Some(bytes) if bytes == walked => {}
            Some(_) => {
                drop(usage);
                self.latch_cas_failure();
            }
        }
    }

    /// The usage the health sampler reports: the count when one is held, and
    /// otherwise a lock-free walk that stops once `cancelled` returns true.
    pub(crate) fn reported_artifact_usage(
        &self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<u64>, KernelError> {
        if let Some(bytes) = *self.counted_usage() {
            return Ok(Some(bytes));
        }
        regular_file_bytes(&self.objects_directory, cancelled).map_err(|_| KernelError::Io)
    }

    /// Walks of the object tree this store took to count its usage since it
    /// opened.
    #[cfg(feature = "test-support")]
    pub fn artifact_census_walks_for_test(&self) -> usize {
        self.artifact_census_walks
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Drops the held count, as an unlink with an unknown outcome does, so the
    /// next reader walks the tree.
    #[cfg(feature = "test-support")]
    pub fn forget_artifact_usage_for_test(&self) {
        let _writer = self.lock_writer();
        *self.counted_usage() = None;
    }

    fn walk_artifact_usage(&self) -> Result<u64, StorageError> {
        #[cfg(feature = "test-support")]
        self.artifact_census_walks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(regular_file_bytes(&self.objects_directory, &|| false)?
            .expect("an uncancellable walk completes"))
    }

    fn counted_usage(&self) -> std::sync::MutexGuard<'_, Option<u64>> {
        self.artifact_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
