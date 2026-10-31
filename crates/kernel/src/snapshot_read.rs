//! [`KernelStore::read_at`]: several reads at one commit sequence in one reader transaction.
//!
//! A [`SnapshotRead`] answers each read as its single-call form answers it, with the same row
//! checks, inside one deferred transaction that checked the tip and the store's incarnation once
//! under its reader guard. Every read therefore sees one database state, and a restore cannot land
//! between two of them, because a restore holds every reader guard.

use rusqlite::{Transaction, TransactionBehavior};

use super::applicability::EvalBudget;
use super::commit_read::{CommitReadTarget, tip};
use super::{ClaimFactsError, KernelError, KernelStore, map_sqlite};

/// One reader transaction at a commit sequence, lent to the closure of [`KernelStore::read_at`].
pub struct SnapshotRead<'t> {
    pub(crate) tx: &'t Transaction<'t>,
    pub(crate) requested: i64,
}

impl KernelStore {
    /// Runs `read` in one reader transaction at `target.through_commit`, refused with
    /// `IncarnationMismatch` under the reader guard when the store is not the incarnation `target`
    /// was captured from. Reader acquisition and every statement stop at `budget`'s deadline or
    /// interrupt, and a budget exhausted by the end of `read` refuses its result or error. `read`
    /// holds one pooled reader throughout and a restore waits for that reader, so the closure
    /// performs all store reads through its `SnapshotRead` and keeps its other work brief.
    ///
    /// # Errors
    ///
    /// `Kernel(Deadline)` from an exhausted budget, `Kernel(InvalidInput)` for a negative
    /// sequence, `Kernel(FutureSnapshot)` past the tip, `IncarnationMismatch`, each before `read`
    /// runs, then `Kernel(Deadline)` for a budget exhausted during `read`, then `read`'s own error,
    /// and the store's read errors (`Busy`, `Io`).
    pub fn read_at<T, E>(
        &self,
        target: CommitReadTarget,
        budget: &EvalBudget,
        read: impl FnOnce(&SnapshotRead<'_>) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<KernelError> + From<ClaimFactsError>,
    {
        self.read_as_of(target.through_commit, budget, |snapshot| {
            if target.incarnation != self.incarnation() {
                return Err(ClaimFactsError::IncarnationMismatch.into());
            }
            read(snapshot)
        })
    }

    pub(crate) fn read_as_of<T, E: From<KernelError>>(
        &self,
        requested: i64,
        budget: &EvalBudget,
        read: impl FnOnce(&SnapshotRead<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let limit = budget.acquire_limit();
        limit.check()?;
        if requested < 0 {
            return Err(KernelError::InvalidInput.into());
        }
        let mut reader = self.reader_with_limit(&limit)?;
        let tx = reader.transaction(TransactionBehavior::Deferred)?;
        if requested > tip(&tx).map_err(map_sqlite)? {
            return Err(KernelError::FutureSnapshot.into());
        }
        let value = read(&SnapshotRead { tx: &tx, requested });
        // A budget that ran out during `read` refuses its result and its refusal alike.
        limit.check()?;
        let value = value?;
        tx.commit()?;
        limit.check()?;
        Ok(value)
    }
}
