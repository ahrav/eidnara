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
    use std::time::Instant;

    use super::*;
    use crate::instance::LOCK_RETRY_DELAY;

    #[test]
    fn one_attempt_shared_lock_returns_at_once_under_a_mutator() {
        let root = tempfile::tempdir().expect("temp data root");
        assert!(
            LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
                .expect("no root")
                .is_none()
        );
        let mutator =
            LifecycleTransactionLock::acquire_exclusive(Some(root.path())).expect("mutator");
        let started = Instant::now();
        assert!(
            LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
                .expect("held root")
                .is_none()
        );
        assert!(
            started.elapsed() < LOCK_RETRY_DELAY,
            "one attempt never sleeps"
        );
        drop(mutator);
        let shared = LifecycleTransactionLock::try_acquire_shared(Some(root.path()))
            .expect("free root")
            .expect("a free lock is taken shared");
        assert!(!shared.is_exclusive());
    }
}
