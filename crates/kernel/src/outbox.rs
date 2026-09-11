//! Durable outbox publication, consumer checkpoints, and retention barriers.
//!
//! Writer transactions serialize checkpoint and pruning changes. Consumer
//! checkpoints advance monotonically and bound pruning by commit sequence.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::envelope::{Envelope, ObjectRow, PendingChange, Sensitivity};
use super::redaction::{RedactedField, identity, redact};
use super::retention::begin_fenced_write;
use super::source_hold::release_consumer_holds_in_tx;
use super::{CachedSql, KernelError, KernelStore, current_time_ms, map_sqlite};

/// Result of pruning rows through the minimum consumer checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxPruneResult {
    /// Rows at or below this `commit_seq` were deleted; the bound is inclusive.
    pub horizon: i64,
    pub deleted: usize,
}

/// One unpublished outbox row, as a publisher reads it.
///
/// `payload` is the stored change-event document: fields that redaction
/// rewrote hold their placeholders, and `sensitivity` is the class the row was
/// recorded under, so a publisher can gate what it forwards on the stored
/// class without parsing the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    pub outbox_position: i64,
    pub commit_seq: i64,
    /// Position of this change within its commit, from zero.
    pub ordinal: i64,
    pub object_id: String,
    pub object_kind: String,
    pub source_kind: String,
    pub source_id: String,
    pub source_revision: i64,
    pub sensitivity: Sensitivity,
    pub payload: Vec<u8>,
    pub created_at: i64,
    /// True when this row is the last of its commit, so `outbox_position` is a
    /// position [`KernelStore::mark_outbox_published_through`] accepts.
    pub commit_boundary: bool,
}

/// Auditable operator authorization to remove a consumer checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerAbandonment {
    /// Operator identity, redacted before durable storage.
    pub operator_id: String,
    /// Human-readable reason, redacted before durable storage.
    pub reason: String,
    /// Nonnegative abandonment timestamp in caller-defined durable time units.
    /// Source-hold cleanup uses Unix-epoch milliseconds independently of the caller's audit time.
    pub abandoned_at: i64,
    /// Optional barrier that must already record this consumer.
    pub barrier_id: Option<String>,
}

impl Envelope<'_> {
    /// `consumer_id` is the primary key of `outbox_consumers`, so it is stored verbatim through `identity`, which rejects an id carrying a secret rather than collapsing distinct ids onto one row.
    ///
    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `recorded_at` is negative, `consumer_id` is empty, or redaction rewrites `consumer_id`.
    /// - Returns [`KernelError::Conflict`] when the consumer is already registered.
    pub fn register_outbox_consumer(
        &mut self,
        consumer_id: &str,
        recorded_at: i64,
    ) -> Result<i64, KernelError> {
        self.guarded(|envelope| envelope.register_outbox_consumer_inner(consumer_id, recorded_at))
    }

    fn register_outbox_consumer_inner(
        &mut self,
        consumer_id: &str,
        recorded_at: i64,
    ) -> Result<i64, KernelError> {
        if recorded_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let consumer_id = consumer_identity(consumer_id)?;
        let oldest_commit = self
            .tx
            .query_row_cached("SELECT MIN(commit_seq) FROM outbox", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(map_sqlite)?;
        let checkpoint = match oldest_commit {
            Some(commit_seq) => commit_seq.saturating_sub(1),
            None => self.pre_operation_tip()?,
        };
        self.tx
            .execute_cached(
                "INSERT INTO outbox_consumers(consumer_id,checkpoint_commit_seq,updated_at)
                 VALUES (?1,?2,?3)",
                params![consumer_id, checkpoint, recorded_at],
            )
            .map_err(map_sqlite)?;
        let audit = serde_json::json!({
            "consumer_id": consumer_id.clone(),
            "checkpoint_commit_seq": checkpoint,
            "recorded_at": recorded_at,
        });
        self.push_control_change(
            consumer_id.clone(),
            "outbox_consumer",
            "consumer_register",
            audit,
            Vec::new(),
        );
        Ok(checkpoint)
    }

    /// Source-hold cleanup uses Unix-epoch milliseconds independently of the caller's audit time.
    ///
    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `recorded_at` is negative or `consumer_id` is empty.
    /// - Returns [`KernelError::NotFound`] when the consumer is not registered.
    /// - Returns [`KernelError::ConsumerPending`] when the consumer checkpoint is below the pre-operation tip.
    pub fn deregister_outbox_consumer(
        &mut self,
        consumer_id: &str,
        recorded_at: i64,
    ) -> Result<(), KernelError> {
        self.guarded(|envelope| envelope.deregister_outbox_consumer_inner(consumer_id, recorded_at))
    }

    fn deregister_outbox_consumer_inner(
        &mut self,
        consumer_id: &str,
        recorded_at: i64,
    ) -> Result<(), KernelError> {
        let (consumer_id, checkpoint) = self.consumer_checkpoint(consumer_id, recorded_at)?;
        if checkpoint < self.pre_operation_tip()? {
            return Err(KernelError::ConsumerPending);
        }
        // A missing `outbox_consumers` row counts as checkpoint -1.
        complete_satisfied_barriers(self.tx, recorded_at)?;
        // A consumer that leaves takes its pins with it; otherwise its bytes stay
        // pinned until expiry with no registered owner left to release them.
        release_consumer_holds_in_tx(self.tx, &consumer_id, None, current_time_ms())?;
        self.tx
            .execute_cached(
                "DELETE FROM outbox_consumers WHERE consumer_id=?1",
                [consumer_id.as_str()],
            )
            .map_err(map_sqlite)?;
        let audit = serde_json::json!({
            "consumer_id": consumer_id.clone(),
            "checkpoint_commit_seq": checkpoint,
            "recorded_at": recorded_at,
        });
        self.push_control_change(
            consumer_id.clone(),
            "outbox_consumer",
            "consumer_deregister",
            audit,
            Vec::new(),
        );
        Ok(())
    }

    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `abandoned_at` is negative, or when `consumer_id`, `operator_id`, or `reason` is empty.
    /// - Returns [`KernelError::NotFound`] when the consumer is not registered.
    pub fn abandon_outbox_consumer(
        &mut self,
        consumer_id: &str,
        abandonment: ConsumerAbandonment,
    ) -> Result<(), KernelError> {
        self.guarded(|envelope| envelope.abandon_outbox_consumer_inner(consumer_id, &abandonment))
    }

    fn abandon_outbox_consumer_inner(
        &mut self,
        consumer_id: &str,
        abandonment: &ConsumerAbandonment,
    ) -> Result<(), KernelError> {
        if abandonment.operator_id.trim().is_empty() || abandonment.reason.trim().is_empty() {
            return Err(KernelError::InvalidInput);
        }
        let (consumer_id, checkpoint) =
            self.consumer_checkpoint(consumer_id, abandonment.abandoned_at)?;
        let operator_id = redact(&abandonment.operator_id)?;
        let reason = redact(&abandonment.reason)?;
        // A secret-bearing barrier id is rejected because a redacted id would not
        // match its barrier row.
        let barrier_id = abandonment
            .barrier_id
            .as_deref()
            .map(barrier_identity)
            .transpose()?;
        if let Some(barrier_id) = &barrier_id {
            let recorded: bool = self
                .tx
                .query_row_cached(
                    "SELECT EXISTS(
                         SELECT 1 FROM deletion_backfill_barrier_consumers
                         WHERE barrier_id=?1 AND consumer_id=?2
                     )",
                    params![barrier_id, consumer_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            if !recorded {
                return Err(KernelError::NotFound);
            }
        }
        let abandonment_id = format!(
            "{}-{}",
            self.commit_seq,
            crate::durable_fs::next_unique_id()
        );
        // Deleting `outbox_consumers` removes the consumer checkpoint. Record one
        // abandonment per blocked barrier so each can still satisfy its
        // `required_checkpoint_commit_seq`.
        let recorded = self
            .tx
            .execute_cached(
                "INSERT INTO consumer_abandonments(
                     abandonment_id,consumer_id,barrier_id,operator_id,
                     last_checkpoint_commit_seq,reason,abandoned_at,commit_seq
                 )
                 SELECT ?1 || '-' || CAST(bc.rowid AS TEXT),?2,bc.barrier_id,?3,?4,?5,?6,?7
                 FROM deletion_backfill_barrier_consumers bc
                 JOIN deletion_backfill_barriers b USING(barrier_id)
                 WHERE bc.consumer_id=?2
                   AND (b.completed_at IS NULL OR b.barrier_id=?8)",
                params![
                    abandonment_id,
                    consumer_id,
                    operator_id.text,
                    checkpoint,
                    reason.text,
                    abandonment.abandoned_at,
                    self.commit_seq,
                    barrier_id.as_deref(),
                ],
            )
            .map_err(map_sqlite)?;
        if recorded == 0 {
            self.tx
                .execute_cached(
                    "INSERT INTO consumer_abandonments(
                         abandonment_id,consumer_id,barrier_id,operator_id,
                         last_checkpoint_commit_seq,reason,abandoned_at,commit_seq
                     ) VALUES (?1,?2,NULL,?3,?4,?5,?6,?7)",
                    params![
                        abandonment_id,
                        consumer_id,
                        operator_id.text,
                        checkpoint,
                        reason.text,
                        abandonment.abandoned_at,
                        self.commit_seq,
                    ],
                )
                .map_err(map_sqlite)?;
        }
        release_consumer_holds_in_tx(self.tx, &consumer_id, None, current_time_ms())?;
        self.tx
            .execute_cached(
                "DELETE FROM outbox_consumers WHERE consumer_id=?1",
                [consumer_id.as_str()],
            )
            .map_err(map_sqlite)?;
        complete_satisfied_barriers(self.tx, abandonment.abandoned_at)?;
        let audit = serde_json::json!({
            "consumer_id": consumer_id.clone(),
            "checkpoint_commit_seq": checkpoint,
            "operator_id": operator_id.text.clone(),
            "reason": reason.text.clone(),
            "abandoned_at": abandonment.abandoned_at,
            "barrier_id": barrier_id.clone(),
        });
        self.push_control_change(
            consumer_id.clone(),
            "outbox_consumer",
            "consumer_abandon",
            audit,
            vec![
                ("operator_id".to_string(), operator_id),
                ("reason".to_string(), reason),
            ],
        );
        Ok(())
    }

    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `abandoned_at` is negative, or when `barrier_id`, `operator_id`, or `reason` is empty.
    /// - Returns [`KernelError::Conflict`] when the barrier still has recorded consumers.
    /// - Returns [`KernelError::NotFound`] when no incomplete barrier has the id.
    pub fn abandon_deletion_barrier(
        &mut self,
        barrier_id: &str,
        operator_id: &str,
        reason: &str,
        abandoned_at: i64,
    ) -> Result<(), KernelError> {
        self.guarded(|envelope| {
            envelope.abandon_deletion_barrier_inner(barrier_id, operator_id, reason, abandoned_at)
        })
    }

    fn abandon_deletion_barrier_inner(
        &mut self,
        barrier_id: &str,
        operator_id: &str,
        reason: &str,
        abandoned_at: i64,
    ) -> Result<(), KernelError> {
        if operator_id.trim().is_empty() || reason.trim().is_empty() || abandoned_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let barrier_id = barrier_identity(barrier_id)?;
        let operator_id = redact(operator_id)?;
        let reason = redact(reason)?;
        let consumer_count: i64 = self
            .tx
            .query_row_cached(
                "SELECT COUNT(*) FROM deletion_backfill_barrier_consumers WHERE barrier_id=?1",
                [&barrier_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        if consumer_count != 0 {
            return Err(KernelError::Conflict);
        }
        if self
            .tx
            .execute_cached(
                "UPDATE deletion_backfill_barriers SET completed_at=?1
                 WHERE barrier_id=?2 AND completed_at IS NULL",
                params![abandoned_at, barrier_id],
            )
            .map_err(map_sqlite)?
            != 1
        {
            return Err(KernelError::NotFound);
        }
        let audit = serde_json::json!({
            "barrier_id": barrier_id.clone(),
            "operator_id": operator_id.text.clone(),
            "reason": reason.text.clone(),
            "abandoned_at": abandoned_at,
        });
        self.push_control_change(
            barrier_id,
            "deletion_backfill_barrier",
            "deletion_barrier_abandon",
            audit,
            vec![
                ("operator_id".to_string(), operator_id),
                ("reason".to_string(), reason),
            ],
        );
        Ok(())
    }

    fn consumer_checkpoint(
        &self,
        consumer_id: &str,
        recorded_at: i64,
    ) -> Result<(String, i64), KernelError> {
        if recorded_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let consumer_id = consumer_identity(consumer_id)?;
        let checkpoint = self
            .tx
            .query_row_cached(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [consumer_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_sqlite)?
            .ok_or(KernelError::NotFound)?;
        Ok((consumer_id, checkpoint))
    }

    fn pre_operation_tip(&self) -> Result<i64, KernelError> {
        self.tx
            .query_row_cached(
                "SELECT COALESCE(MAX(commit_seq),0) FROM commit_log WHERE commit_seq<?1",
                [self.commit_seq],
                |row| row.get(0),
            )
            .map_err(map_sqlite)
    }

    fn push_control_change(
        &mut self,
        object_id: String,
        object_kind: &'static str,
        kind: &'static str,
        audit: serde_json::Value,
        redactions: Vec<(String, RedactedField)>,
    ) {
        self.changes.push(PendingChange {
            object: ObjectRow {
                source_id: object_id.clone(),
                object_id,
                object_kind: object_kind.to_string(),
                domain_id: "kernel-control".to_string(),
                source_kind: "kernel-control".to_string(),
                source_revision: 0,
                created_commit_seq: self.commit_seq,
                invalidated_commit_seq: None,
                superseded_by: None,
                sensitivity: Sensitivity::Normal,
            },
            kind,
            replaced_object_id: None,
            redactions,
            audit: Some(audit),
        });
    }
}

impl KernelStore {
    /// Unpublished outbox rows in position order, at most `limit` of them.
    ///
    /// Rows are read in one snapshot, so `commit_boundary` is answered against
    /// the same rows the caller sees. A batch cut short by `limit` can end
    /// mid-commit; the publisher checkpoints at the last row whose
    /// `commit_boundary` is set, or reads again with a larger limit when the
    /// batch holds none.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] when `limit` is zero.
    pub fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxEntry>, KernelError> {
        if limit == 0 {
            return Err(KernelError::InvalidInput);
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        let mut statement = tx
            .prepare_cached(
                "SELECT o.outbox_position,o.commit_seq,o.ordinal,o.object_id,o.object_kind,
                        o.source_kind,o.source_id,o.source_revision,o.sensitivity_class,
                        o.payload,o.created_at,
                        NOT EXISTS(
                            SELECT 1 FROM outbox later
                            WHERE later.commit_seq=o.commit_seq
                              AND later.outbox_position>o.outbox_position
                        )
                 FROM outbox o
                 WHERE o.published_at IS NULL
                 ORDER BY o.outbox_position
                 LIMIT ?1",
            )
            .map_err(map_sqlite)?;
        let entries = statement
            .query_map([limit], outbox_entry)
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_sqlite)?;
        Ok(entries)
    }

    /// The watermark lives in `outbox_publication` rather than being derived from surviving `outbox` rows, so a publisher whose acknowledgement round trip was lost can repeat the same position after those rows are pruned.
    ///
    /// A position at or below the stored watermark counts as already published and skips the commit-boundary check.
    ///
    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `outbox_position` is below 1 or `published_at` is negative.
    /// - Returns [`KernelError::InvalidCheckpoint`] when the position is above the watermark and is not the last row of its commit.
    pub fn mark_outbox_published_through(
        &self,
        outbox_position: i64,
        published_at: i64,
    ) -> Result<(), KernelError> {
        if outbox_position < 1 || published_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let mut writer = self.lock_writer()?;
        let tx = begin_fenced_write(&mut writer, self.lease_epoch())?;
        let watermark = published_watermark(&tx)?;
        if outbox_position > watermark && !is_outbox_commit_boundary(&tx, outbox_position)? {
            return Err(KernelError::InvalidCheckpoint);
        }
        tx.execute_cached(
            "UPDATE outbox SET published_at=?1
             WHERE outbox_position<=?2 AND published_at IS NULL",
            params![published_at, outbox_position],
        )
        .map_err(map_sqlite)?;
        tx.execute_cached(
            "INSERT INTO outbox_publication(id,published_through_position,published_at)
             VALUES (0,?1,?2)
             ON CONFLICT(id) DO UPDATE SET
                 published_through_position=MAX(published_through_position,?1),
                 published_at=MAX(published_at,?2)",
            params![outbox_position, published_at],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)
    }

    /// Advances a registered consumer checkpoint monotonically.
    ///
    /// Repeating current checkpoint is idempotent. Barrier completion is checked
    /// in same writer transaction.
    ///
    /// # Errors
    ///
    /// - Returns [`KernelError::InvalidInput`] when `consumer_id` is empty or a timestamp is negative.
    /// - Returns [`KernelError::NotFound`] when consumer is not registered.
    /// - Returns [`KernelError::InvalidCheckpoint`] when checkpoint moves backwards or names no committed sequence.
    pub fn acknowledge_outbox(
        &self,
        consumer_id: &str,
        checkpoint_commit_seq: i64,
        updated_at: i64,
    ) -> Result<(), KernelError> {
        if checkpoint_commit_seq < 0 || updated_at < 0 {
            return Err(KernelError::InvalidInput);
        }
        let consumer_id = consumer_identity(consumer_id)?;
        let mut writer = self.lock_writer()?;
        let tx = begin_fenced_write(&mut writer, self.lease_epoch())?;
        acknowledge_outbox_in_tx(&tx, &consumer_id, checkpoint_commit_seq, updated_at)?;
        tx.commit().map_err(map_sqlite)
    }

    /// Registered consumers bound the horizon; publication does not. A publisher that needs its own rows retained registers as a consumer, because `published_at` records progress without holding rows back.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::NoRequiredConsumers`] when no consumer is registered, since an empty consumer set names no safe horizon.
    pub fn prune_outbox(&self) -> Result<OutboxPruneResult, KernelError> {
        let mut writer = self.lock_writer()?;
        let tx = begin_fenced_write(&mut writer, self.lease_epoch())?;
        let horizon = tx
            .query_row_cached(
                "SELECT MIN(checkpoint_commit_seq) FROM outbox_consumers",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(map_sqlite)?
            .ok_or(KernelError::NoRequiredConsumers)?;
        tx.execute_cached(
            "DELETE FROM durable_text_redactions
             WHERE owner_kind='outbox' AND owner_id IN (
                 SELECT CAST(outbox_position AS TEXT) FROM outbox WHERE commit_seq<=?1
             )",
            [horizon],
        )
        .map_err(map_sqlite)?;
        let deleted = tx
            .execute_cached("DELETE FROM outbox WHERE commit_seq<=?1", [horizon])
            .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(OutboxPruneResult { horizon, deleted })
    }
}

/// Maps the twelve-column outbox projection every reader selects: the eleven stored columns in
/// schema order, then the caller's commit-boundary expression.
pub(super) fn outbox_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxEntry> {
    let sensitivity: String = row.get(8)?;
    Ok(OutboxEntry {
        outbox_position: row.get(0)?,
        commit_seq: row.get(1)?,
        ordinal: row.get(2)?,
        object_id: row.get(3)?,
        object_kind: row.get(4)?,
        source_kind: row.get(5)?,
        source_id: row.get(6)?,
        source_revision: row.get(7)?,
        sensitivity: Sensitivity::from_stored(&sensitivity),
        payload: row.get(9)?,
        created_at: row.get(10)?,
        commit_boundary: row.get(11)?,
    })
}

/// Moves `consumer_id`'s checkpoint to `checkpoint_commit_seq` inside the
/// caller's fenced write and completes any deletion barrier that satisfies.
/// The checkpoint must name an existing commit at or after the current one.
pub(crate) fn acknowledge_outbox_in_tx(
    tx: &Transaction<'_>,
    consumer_id: &str,
    checkpoint_commit_seq: i64,
    updated_at: i64,
) -> Result<(), KernelError> {
    let current = tx
        .query_row_cached(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
            [consumer_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or(KernelError::NotFound)?;
    let is_existing_commit = checkpoint_commit_seq == current
        || tx
            .query_row_cached(
                "SELECT EXISTS(SELECT 1 FROM commit_log WHERE commit_seq=?1)",
                [checkpoint_commit_seq],
                |row| row.get::<_, bool>(0),
            )
            .map_err(map_sqlite)?;
    if checkpoint_commit_seq < current || !is_existing_commit {
        return Err(KernelError::InvalidCheckpoint);
    }
    tx.execute_cached(
        "UPDATE outbox_consumers
         SET checkpoint_commit_seq=?1,updated_at=MAX(updated_at,?2)
         WHERE consumer_id=?3",
        params![checkpoint_commit_seq, updated_at, consumer_id],
    )
    .map_err(map_sqlite)?;
    complete_satisfied_barriers(tx, updated_at)
}

fn consumer_identity(consumer_id: &str) -> Result<String, KernelError> {
    if consumer_id.trim().is_empty() {
        return Err(KernelError::InvalidInput);
    }
    identity(consumer_id)
}

fn barrier_identity(barrier_id: &str) -> Result<String, KernelError> {
    if barrier_id.trim().is_empty() {
        return Err(KernelError::InvalidInput);
    }
    identity(barrier_id)
}

/// A consumer whose acknowledgement is already persisted stays satisfied even if
/// its checkpoint later regresses or its row is deleted.
fn complete_satisfied_barriers(tx: &Transaction<'_>, completed_at: i64) -> Result<(), KernelError> {
    tx.execute_cached(
        "UPDATE deletion_backfill_barrier_consumers AS bc SET acknowledged_at=?1
         WHERE bc.acknowledged_at IS NULL
           AND EXISTS(
               SELECT 1 FROM outbox_consumers c
               WHERE c.consumer_id=bc.consumer_id
                 AND c.checkpoint_commit_seq>=bc.required_checkpoint_commit_seq
           )",
        params![completed_at],
    )
    .map_err(map_sqlite)?;
    tx.execute_cached(
        "UPDATE deletion_backfill_barriers AS b SET completed_at=?1
         WHERE b.completed_at IS NULL
           AND EXISTS(
               SELECT 1 FROM deletion_backfill_barrier_consumers bc
               WHERE bc.barrier_id=b.barrier_id
           )
           AND NOT EXISTS(
               SELECT 1 FROM deletion_backfill_barrier_consumers bc
               LEFT JOIN outbox_consumers c USING(consumer_id)
               WHERE bc.barrier_id=b.barrier_id
                 AND bc.acknowledged_at IS NULL
                 AND COALESCE(c.checkpoint_commit_seq,-1)<bc.required_checkpoint_commit_seq
                 AND NOT EXISTS(
                     SELECT 1 FROM consumer_abandonments a
                     WHERE a.barrier_id=bc.barrier_id AND a.consumer_id=bc.consumer_id
                       AND a.commit_seq>=bc.required_checkpoint_commit_seq
                 )
           )",
        params![completed_at],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Highest outbox position published to consumers; `0` before the first publication.
pub(super) fn published_watermark(tx: &Transaction<'_>) -> Result<i64, KernelError> {
    tx.query_row_cached(
        "SELECT published_through_position FROM outbox_publication WHERE id=0",
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|value| value.unwrap_or(0))
    .map_err(map_sqlite)
}

fn is_outbox_commit_boundary(
    tx: &Transaction<'_>,
    outbox_position: i64,
) -> Result<bool, KernelError> {
    tx.query_row_cached(
        "SELECT NOT EXISTS(
             SELECT 1 FROM outbox later
             WHERE later.commit_seq=chosen.commit_seq
               AND later.outbox_position>chosen.outbox_position
         ) FROM outbox chosen WHERE chosen.outbox_position=?1",
        [outbox_position],
        |row| row.get::<_, bool>(0),
    )
    .optional()
    .map(|value| value.unwrap_or(false))
    .map_err(map_sqlite)
}
