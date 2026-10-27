//! Evidence liveness at a snapshot: for a bounded batch of evidence ids,
//! whether each `evidence_meta` row existed and was not invalidated at the
//! snapshot's commit. The read is the fact P4's confirm checks before a
//! dispute cites evidence, offered without a write transaction or a hold.

use rusqlite::{OptionalExtension, TransactionBehavior};

use super::applicability::EvalBudget;
use super::commit_read::{CommitReadTarget, tip};
use super::{CachedSql, KernelError, KernelStore, map_sqlite};

/// The most evidence ids one [`KernelStore::evidence_liveness_at_within_budget`] call reads.
pub const MAX_EVIDENCE_LIVENESS_IDS: usize = 256;

/// One evidence id's standing at the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceLiveness {
    /// Created at or before the snapshot and not invalidated at or before it.
    Live,
    /// Invalidated at or before the snapshot.
    Retired,
    /// No row was created by the snapshot.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceLivenessError {
    #[error("evidence liveness request names more ids than the bound")]
    TooManyIds,
    #[error("evidence liveness target names another kernel incarnation")]
    IncarnationMismatch,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

impl KernelStore {
    /// The liveness of each of `evidence_ids` at `target.through_commit`, in
    /// request order, from one snapshot transaction with one indexed lookup
    /// per id. The incarnation is compared under the reader guard, so a
    /// restore between capturing the target and this read cannot pair one
    /// history's tip with another's rows.
    ///
    /// # Errors
    ///
    /// `TooManyIds` for more than [`MAX_EVIDENCE_LIVENESS_IDS`] ids before any
    /// row is read; `IncarnationMismatch` when the store is not the
    /// incarnation `target` was captured from; `Kernel(FutureSnapshot)` when
    /// the target is past the tip; `Kernel(Deadline)` from an exhausted
    /// budget; the SQLite classification for a read that fails.
    pub fn evidence_liveness_at_within_budget(
        &self,
        evidence_ids: &[&str],
        target: CommitReadTarget,
        budget: &EvalBudget,
    ) -> Result<Vec<EvidenceLiveness>, EvidenceLivenessError> {
        if evidence_ids.len() > MAX_EVIDENCE_LIVENESS_IDS {
            return Err(EvidenceLivenessError::TooManyIds);
        }
        let limit = budget.acquire_limit();
        limit.run(|| {
            let mut reader = self.reader_with_limit(&limit)?;
            if target.incarnation != self.incarnation() {
                return Err(EvidenceLivenessError::IncarnationMismatch);
            }
            let tx = reader.transaction(TransactionBehavior::Deferred)?;
            if target.through_commit > tip(&tx).map_err(map_sqlite)? {
                return Err(KernelError::FutureSnapshot.into());
            }
            let mut standing = Vec::with_capacity(evidence_ids.len());
            for evidence_id in evidence_ids {
                let row: Option<(i64, Option<i64>)> = tx
                    .query_row_cached(
                        "SELECT created_commit_seq, invalidated_commit_seq
                           FROM evidence_meta WHERE evidence_id=?1",
                        [evidence_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                standing.push(match row {
                    Some((created, _)) if created > target.through_commit => {
                        EvidenceLiveness::Unknown
                    }
                    Some((_, Some(invalidated))) if invalidated <= target.through_commit => {
                        EvidenceLiveness::Retired
                    }
                    Some(_) => EvidenceLiveness::Live,
                    None => EvidenceLiveness::Unknown,
                });
            }
            tx.commit()?;
            limit.check()?;
            Ok(standing)
        })
    }
}
