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

use super::ingest::{regular_file_bytes, regular_file_bytes_concurrently};
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

    /// Durably unlinks the object `name` in `shard`, subtracts the bytes the
    /// walk counted for it, and returns them. The caller holds the writer
    /// lock. An unlink that fails, or a size the stat cannot report, leaves the
    /// count unknown, so the next reader walks; a removal larger than the count
    /// is drift, which leaves the count unknown and latches the fail-closed
    /// state.
    pub(super) fn unlink_counted_object(
        &self,
        shard: &File,
        name: &str,
    ) -> Result<u64, StorageError> {
        let removed = match rfs::statat(shard, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if rfs::FileType::from_raw_mode(stat.st_mode).is_file() => {
                u64::try_from(stat.st_size).ok()
            }
            Ok(_) | Err(rustix::io::Errno::NOENT) => Some(0),
            Err(_) => None,
        };
        let unlinked = durable_unlink(shard, name);
        let mut usage = self.counted_usage();
        let Some(bytes) = *usage else {
            return unlinked.map(|()| removed.unwrap_or(0));
        };
        *usage = None;
        if let (Ok(()), Some(removed)) = (&unlinked, removed) {
            match bytes.checked_sub(removed) {
                Some(remaining) => *usage = Some(remaining),
                None => self.latch_cas_failure(),
            }
        }
        unlinked.map(|()| removed.unwrap_or(0))
    }

    /// Re-walks the tree under the writer lock and compares the walk with the
    /// count. With no count held the walk becomes the count. A disagreement is
    /// drift: the fail-closed state latches and the walk replaces the count,
    /// so admission stays refused while the health sampler reports the bytes
    /// on disk. A walk that fails for a reason other than exhaustion latches
    /// too; an exhausted walk leaves the count as it was for the next reader.
    pub(crate) fn reconcile_artifact_usage(&self) {
        let Ok(_writer) = self.lock_writer() else {
            return;
        };
        self.settle_census(self.walk_artifact_usage());
    }

    /// [`reconcile_artifact_usage`](Self::reconcile_artifact_usage) with the
    /// walk `census` took while the store was opening. Until `open` returns,
    /// the opening thread is the store's only user, so that walk counts the
    /// tree the writer lock would have seen. A census whose thread could not be
    /// started walks here instead.
    pub(crate) fn seed_artifact_usage(&self, mut census: ArtifactCensus) {
        let Ok(_writer) = self.lock_writer() else {
            return;
        };
        let walked = match census.0.take() {
            Some(walk) => {
                #[cfg(feature = "test-support")]
                self.artifact_census_walks
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                walk.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            }
            None => self.walk_artifact_usage(),
        };
        self.settle_census(walked);
    }

    fn settle_census(&self, walked: Result<u64, StorageError>) {
        let walked = match walked {
            Ok(walked) => walked,
            Err(StorageError::Exhausted(_)) => return,
            Err(StorageError::Other(_)) => return self.latch_cas_failure(),
        };
        let mut usage = self.counted_usage();
        if usage.is_some_and(|bytes| bytes != walked) {
            self.latch_cas_failure();
        }
        *usage = Some(walked);
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

    pub(crate) fn held_artifact_usage(&self) -> Option<u64> {
        *self.counted_usage()
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
        regular_file_bytes_concurrently(&self.objects_directory)
    }

    fn counted_usage(&self) -> std::sync::MutexGuard<'_, Option<u64>> {
        self.artifact_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The census `open` starts on a thread of its own once the layout and any
/// interrupted restore are settled, so the walk overlaps the database open.
/// Dropping it joins the walk.
pub(crate) struct ArtifactCensus(Option<std::thread::JoinHandle<Result<u64, StorageError>>>);

impl ArtifactCensus {
    pub(crate) fn start(objects: &File) -> Self {
        Self(objects.try_clone().ok().and_then(|objects| {
            std::thread::Builder::new()
                .spawn(move || regular_file_bytes_concurrently(&objects))
                .ok()
        }))
    }
}

impl Drop for ArtifactCensus {
    fn drop(&mut self) {
        if let Some(walk) = self.0.take() {
            let _ = walk.join();
        }
    }
}
