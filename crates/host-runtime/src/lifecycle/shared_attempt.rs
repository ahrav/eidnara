use std::path::Path;

use rustix::fs::FlockOperation;

use super::{LifecycleTransactionLock, TRANSACTION_LOCK_NAME, open_coordination_lock_probe};
use crate::instance::{InstanceError, flock_nonblocking};

impl LifecycleTransactionLock {
    /// Callers can use this method to serve a fallback result when a mutator holds the lock.
    /// `Ok(None)` means no coordination root exists or a mutator holds the lock at the attempt.
    pub fn try_acquire_shared(
        data_dir_override: Option<&Path>,
    ) -> Result<Option<Self>, InstanceError> {
        let Some((file, path)) =
            open_coordination_lock_probe(data_dir_override, TRANSACTION_LOCK_NAME)?
        else {
            return Ok(None);
        };
        if flock_nonblocking(
            &file,
            &path,
            "flock_transaction",
            FlockOperation::NonBlockingLockShared,
        )? {
            Ok(Some(Self {
                _file: file,
                path,
                exclusive: false,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_attempt_reports_a_mutator_and_takes_a_free_lock() {
        let root = tempfile::tempdir().expect("temp data root");
        assert!(
            LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
                .expect("no root")
                .is_none()
        );
        let mutator =
            LifecycleTransactionLock::acquire_exclusive(Some(root.path())).expect("mutator");
        assert!(
            LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
                .expect("held root")
                .is_none()
        );
        drop(mutator);
        let shared = LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
            .expect("free root")
            .expect("a free lock is taken shared");
        assert!(!shared.is_exclusive());
    }
}
