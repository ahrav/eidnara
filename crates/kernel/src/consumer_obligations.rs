//! Bounded, fenced inventory for retiring a derived consumer without its database.

use std::num::{NonZeroU64, NonZeroUsize};

use rusqlite::{TransactionBehavior, params};

use crate::{CommitReadTarget, KernelError, KernelStore, applicability::EvalBudget, map_sqlite};

/// A retained canonical fact whose derived residue must be reconciled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerObligation {
    pub kind: String,
    pub identity: String,
    pub artifact_digest: String,
    pub commit_seq: i64,
    pub invalidated_commit_seq: Option<i64>,
}

const INVENTORY: &str = "
 SELECT 'source' AS kind,o.object_id AS identity,e.artifact_digest,
 o.created_commit_seq AS commit_seq,
 CASE WHEN e.invalidated_commit_seq<=?2 AND (o.invalidated_commit_seq IS NULL
       OR e.invalidated_commit_seq<o.invalidated_commit_seq) THEN e.invalidated_commit_seq
      WHEN o.invalidated_commit_seq<=?2 THEN o.invalidated_commit_seq END AS invalidated_commit_seq
 FROM object_registry o
 LEFT JOIN observations b ON b.object_id=o.object_id AND b.observation_kind='source_descriptor'
 LEFT JOIN evidence_meta e ON e.evidence_id=b.evidence_id
 WHERE o.object_id GLOB 'srcdesc:*' AND o.created_commit_seq<=?2
 UNION ALL
 SELECT 'barrier',bc.barrier_id,b.artifact_digest,bc.required_checkpoint_commit_seq,
 bc.required_checkpoint_commit_seq
 FROM deletion_backfill_barrier_consumers bc
 LEFT JOIN deletion_backfill_barriers b USING(barrier_id)
 WHERE bc.consumer_id=?1 AND bc.required_checkpoint_commit_seq<=?2";

impl KernelStore {
    /// Returns the global canonical source census through the fixed target, not a per-consumer source set.
    /// Barriers are consumer-specific and include satisfied memberships through the target.
    /// Whole-family removal applies to every source in the canonical census; missing joined authority indicates corruption.
    /// Row and encoded-field charges are checked before materialization.
    pub fn consumer_obligations_within_budget(
        &self,
        budget: &EvalBudget,
        consumer: &str,
        target: CommitReadTarget,
        max_rows: NonZeroUsize,
        max_bytes: NonZeroU64,
    ) -> Result<Vec<ConsumerObligation>, KernelError> {
        let limit = budget.acquire_limit();
        limit.run(|| {
            if consumer.is_empty() || target.through_commit < 0 {
                return Err(KernelError::InvalidInput);
            }
            let mut reader = self.reader_with_limit(&limit)?;
            if self.incarnation() != target.incarnation {
                return Err(KernelError::InvalidInput);
            }
            let tx = reader.transaction(TransactionBehavior::Deferred)?;
            crate::envelope::check_fence(&tx, self.lease_epoch())?;
            if target.through_commit > crate::commit_read::tip(&tx).map_err(map_sqlite)? {
                return Err(KernelError::InvalidCheckpoint);
            }
            let (rows, bytes, missing): (i64, i64, bool) = tx.query_row(
                &format!("SELECT count(*),coalesce(sum(length(CAST(kind AS BLOB))+length(CAST(identity AS BLOB))+length(artifact_digest)+16),0),count(*)!=count(artifact_digest) FROM ({INVENTORY})"),
                params![consumer, target.through_commit],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(map_sqlite)?;
            if missing {
                return Err(KernelError::CorruptCanonicalRow);
            }
            if rows as u64 > max_rows.get() as u64 || bytes as u64 > max_bytes.get() {
                return Err(KernelError::InvalidInput);
            }
            let mut statement = tx.prepare(&format!("{INVENTORY} ORDER BY kind,identity")).map_err(map_sqlite)?;
            let obligations = statement.query_map(params![consumer, target.through_commit], |row| {
                Ok(ConsumerObligation {
                    kind: row.get(0)?, identity: row.get(1)?, artifact_digest: row.get(2)?,
                    commit_seq: row.get(3)?, invalidated_commit_seq: row.get(4)?,
                })
            }).map_err(map_sqlite)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?;
            limit.check()?;
            Ok(obligations)
        })
    }
}
