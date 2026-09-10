//! One complete canonical prefix applied to the projection in one transaction:
//! the rows created in the window, the invalidation of rows that stopped being
//! live, the pending embedding work the new rows need, and the checkpoint that
//! says the projection has applied everything through the window's end. The
//! batch is admitted from its sizes before any row is written, and a batch
//! that does not fit is refused whole; a canonical commit is never split.
//!
//! The batch carries its mutation identity: the fixed sequence S it was
//! exported at, the hold it was exported under, and the commit it runs
//! through. Replaying a batch already applied changes nothing, an older prefix
//! never moves the checkpoint back or revives a tombstone, and a batch under a
//! different hold or snapshot than the projection was built with is refused.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use kernel::SourceRow;
use kernel::source_identity::{Occurrence, OccurrenceClass, Span, encode};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use crate::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, Tombstone, TombstoneReason,
    persist_occurrences, tombstone_occurrence,
};

/// Where the batch comes from and how far it reaches. Two batches with the
/// same identity apply the same rows; an application is replayed by identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationIdentity {
    /// The kernel incarnation the projection identity was installed under.
    pub kernel_incarnation_id: String,
    /// The hold the rows were exported under.
    pub hold_id: String,
    /// The fixed sequence the projection was built at.
    pub snapshot_commit_seq: i64,
    /// The last canonical commit the batch applies.
    pub through_commit_seq: i64,
}

/// One invalidation fact from the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalidation {
    pub occurrence_id: String,
    pub tombstone: Tombstone,
}

/// A complete prefix to apply. Rows and invalidations are the window's, in
/// export order.
#[derive(Clone)]
pub struct ProjectionBatch<'a> {
    pub identity: MutationIdentity,
    pub records: Vec<OccurrenceRecord<'a>>,
    pub invalidations: Vec<Invalidation>,
    /// The vector generation new dense-eligible rows are queued for; `None`
    /// records rows without any dense work.
    pub generation_id: Option<&'a str>,
}

impl std::fmt::Debug for ProjectionBatch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionBatch")
            .field("identity", &self.identity)
            .field("records", &self.records.len())
            .field("invalidations", &self.invalidations.len())
            .field("generation_id", &self.generation_id)
            .finish()
    }
}

/// Bounds a whole batch is admitted against before any row is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchBounds {
    pub persist: PersistBounds,
    /// Bytes of source text across the batch's records.
    pub max_source_bytes: NonZeroUsize,
    /// Rows plus invalidations the batch may write.
    pub max_local_mutations: NonZeroUsize,
    /// Pending jobs the projection may hold in total after the batch.
    pub max_pending: NonZeroUsize,
}

/// What applying a batch did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BatchOutcome {
    pub rows_inserted: usize,
    pub rows_replayed: usize,
    pub tombstones_recorded: usize,
    pub pending_created: usize,
    pub pending_obsoleted: usize,
    /// The checkpoint after the batch; unchanged when the batch was an
    /// already-applied prefix.
    pub checkpoint_commit_seq: i64,
}

/// Whether a batch's mutation identity is durably applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStatus {
    /// The checkpoint under this hold and snapshot reaches the batch's end.
    Applied,
    /// The checkpoint stands before the batch's end, or nothing is applied yet.
    NotApplied,
}

/// The dense-eligible classes: raw tool spans stay lexical-only.
fn dense_eligible(class: &str) -> bool {
    OccurrenceClass::from_code(class).is_some_and(|class| class != OccurrenceClass::RawToolSpans)
}

/// The job identity for one occurrence in one generation, so the same work
/// can never be queued twice.
fn job_id(occurrence_id: &str, generation_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(occurrence_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(generation_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Turns exported kernel rows into a batch: rows with text become records
/// over their selected text, rows invalidated inside the window become
/// invalidations, and a row that is both is both. `identities` holds each
/// row's identity fields in borrowed form, one entry per row.
pub fn batch_from_rows<'a>(
    rows: &'a [SourceRow],
    identities: &'a [Vec<(&'a str, &'a str)>],
    identity: MutationIdentity,
    generation_id: Option<&'a str>,
) -> Result<ProjectionBatch<'a>, ProjectionError> {
    if rows.len() != identities.len() {
        return Err(ProjectionError::MalformedBatch);
    }
    let mut records = Vec::new();
    let mut invalidations = Vec::new();
    for (row, fields) in rows.iter().zip(identities) {
        let occurrence = Occurrence {
            class: &row.detail.class,
            identity: fields,
            revision: &row.detail.revision,
            representation: &row.detail.representation,
            span: row.detail.span.map(|(start, end)| Span { start, end }),
        };
        if let Some(text) = row.text.as_deref() {
            records.push(OccurrenceRecord {
                occurrence,
                payload: Payload::Selected(text),
                domain_id: &row.domain_id,
                sensitivity: row.sensitivity,
                source_object_id: &row.object_id,
                source_evidence_id: &row.detail.evidence_id,
                source_artifact_digest: &row.detail.artifact_digest,
                created_commit_seq: row.created_commit_seq,
            });
        }
        if let Some(at) = row.invalidated_commit_seq
            && at > identity.snapshot_commit_seq
            && at <= identity.through_commit_seq
        {
            let reason = if row.superseded_by.is_some() {
                TombstoneReason::Superseded
            } else {
                TombstoneReason::Retired
            };
            invalidations.push(Invalidation {
                occurrence_id: row.detail.occurrence_id.clone(),
                tombstone: Tombstone {
                    invalidated_commit_seq: at,
                    reason,
                },
            });
        }
    }
    Ok(ProjectionBatch {
        identity,
        records,
        invalidations,
        generation_id,
    })
}

/// The identity fields of exported rows in the borrowed shape
/// [`batch_from_rows`] takes.
pub fn row_identities(rows: &[SourceRow]) -> Vec<Vec<(&str, &str)>> {
    rows.iter()
        .map(|row| {
            row.detail
                .identity
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect()
        })
        .collect()
}

/// Whether the batch's end is already inside the durable checkpoint under the
/// batch's hold and snapshot. Reconciles an unknown commit outcome from the
/// durable contents rather than assuming a rollback.
pub fn batch_status(
    conn: &GuardedConn<'_>,
    identity: &MutationIdentity,
) -> Result<BatchStatus, ProjectionError> {
    let stored = read_checkpoint(conn)?;
    Ok(match stored {
        Some(checkpoint)
            if checkpoint.hold_id == identity.hold_id
                && checkpoint.snapshot_commit_seq == identity.snapshot_commit_seq
                && checkpoint.checkpoint_commit_seq >= identity.through_commit_seq =>
        {
            BatchStatus::Applied
        }
        _ => BatchStatus::NotApplied,
    })
}

struct Checkpoint {
    snapshot_commit_seq: i64,
    checkpoint_commit_seq: i64,
    hold_id: String,
}

fn read_checkpoint(conn: &GuardedConn<'_>) -> Result<Option<Checkpoint>, ProjectionError> {
    conn.query_row(
        "SELECT snapshot_commit_seq,checkpoint_commit_seq,hold_id FROM projection_checkpoint WHERE singleton=1",
        [],
        |row| {
            Ok(Checkpoint {
                snapshot_commit_seq: row.get(0)?,
                checkpoint_commit_seq: row.get(1)?,
                hold_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        },
    )
    .optional()
    .map_err(ProjectionError::from)
}

/// Applies one batch inside the caller's transaction. Admission from sizes
/// comes first, then the identity checks, then rows, tombstones, pending work,
/// and the checkpoint, in that order; any refusal leaves the caller to roll
/// back with nothing of the batch durable.
pub fn apply_batch(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
    bounds: BatchBounds,
    now: i64,
) -> Result<BatchOutcome, ProjectionError> {
    apply_batch_inner(conn, batch, bounds, now, Fault::default())
}

/// A phase after which [`apply_batch_with_fault_for_test`] fails.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchFault {
    AfterAdmission,
    AfterRows,
    AfterTombstones,
    AfterPending,
    AfterCheckpoint,
}

/// [`apply_batch`] failing right after `fault`'s phase, so a test can prove
/// the transaction leaves nothing of the batch behind.
#[cfg(feature = "test-support")]
pub fn apply_batch_with_fault_for_test(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
    bounds: BatchBounds,
    now: i64,
    fault: BatchFault,
) -> Result<BatchOutcome, ProjectionError> {
    apply_batch_inner(conn, batch, bounds, now, Some(fault))
}

#[cfg(feature = "test-support")]
type Fault = Option<BatchFault>;

/// A production build has no fault to inject; the phases still run in the
/// same order.
#[cfg(not(feature = "test-support"))]
#[derive(Clone, Copy, Default)]
struct Fault;

/// Fails when the injected fault names this phase; a production build has no
/// fault to name.
#[cfg(feature = "test-support")]
fn trip(fault: Fault, phase: BatchFault) -> Result<(), ProjectionError> {
    if fault == Some(phase) {
        return Err(ProjectionError::Sqlite(format!(
            "injected fault after {phase:?}"
        )));
    }
    Ok(())
}

#[cfg(feature = "test-support")]
macro_rules! phase {
    ($fault:expr, $phase:ident) => {
        trip($fault, BatchFault::$phase)?
    };
}
#[cfg(not(feature = "test-support"))]
macro_rules! phase {
    ($fault:expr, $phase:ident) => {
        let _: Fault = $fault;
    };
}

/// Charges judged before any row is written.
struct Admission {
    /// Occurrence identifiers of the records, in record order.
    occurrence_ids: Vec<String>,
    /// Whether the projection already reaches the batch's end.
    already_applied: bool,
    /// The checkpoint after the batch.
    checkpoint_commit_seq: i64,
}

fn admit(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
    bounds: BatchBounds,
) -> Result<Admission, ProjectionError> {
    let identity = &batch.identity;
    if identity.through_commit_seq < identity.snapshot_commit_seq
        || identity.snapshot_commit_seq < 0
        || identity.hold_id.is_empty()
    {
        return Err(ProjectionError::MutationConflict);
    }
    let mutations = batch.records.len() + batch.invalidations.len();
    if mutations > bounds.max_local_mutations.get() {
        return Err(ProjectionError::BatchOverBound {
            bound: "local_mutations",
            size: mutations,
        });
    }
    let source_bytes: usize = batch
        .records
        .iter()
        .map(|record| record.payload.len())
        .sum();
    if source_bytes > bounds.max_source_bytes.get() {
        return Err(ProjectionError::BatchOverBound {
            bound: "source_bytes",
            size: source_bytes,
        });
    }
    // The identities decide which pending work is new; encoding refuses a
    // malformed record here, before anything is written.
    let occurrence_ids = batch
        .records
        .iter()
        .map(|record| encode(&record.occurrence).map(|encoded| encoded.occurrence_id))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(generation) = batch.generation_id {
        let outstanding: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embedding_jobs WHERE state IN ('pending','admitted')",
            [],
            |row| row.get(0),
        )?;
        let outstanding = usize::try_from(outstanding).map_err(|_| ProjectionError::CorruptRow)?;
        let mut new_jobs = 0;
        for (record, occurrence_id) in batch.records.iter().zip(&occurrence_ids) {
            if !dense_eligible(record.occurrence.class) {
                continue;
            }
            let queued: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2)",
                params![occurrence_id, generation],
                |row| row.get(0),
            )?;
            if !queued {
                new_jobs += 1;
            }
        }
        let total = outstanding.saturating_add(new_jobs);
        if total > bounds.max_pending.get() {
            return Err(ProjectionError::BatchOverBound {
                bound: "pending",
                size: total,
            });
        }
        let known: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM vector_generations WHERE generation_id=?1 AND state<>'retired')",
            [generation],
            |row| row.get(0),
        )?;
        if !known {
            return Err(ProjectionError::UnknownGeneration {
                generation_id: generation.to_string(),
            });
        }
    }
    // The projection this batch belongs to: same incarnation, same hold and
    // snapshot as every earlier batch, never a checkpoint moving backward.
    let installed = crate::read_identity(conn)?.ok_or(ProjectionError::IdentityMismatch)?;
    if installed.kernel_incarnation_id != identity.kernel_incarnation_id {
        return Err(ProjectionError::IdentityMismatch);
    }
    let stored = read_checkpoint(conn)?;
    if let Some(stored) = &stored
        && (stored.hold_id != identity.hold_id
            || stored.snapshot_commit_seq != identity.snapshot_commit_seq)
    {
        return Err(ProjectionError::MutationConflict);
    }
    let checkpoint_commit_seq = stored
        .as_ref()
        .map_or(identity.through_commit_seq, |stored| {
            stored
                .checkpoint_commit_seq
                .max(identity.through_commit_seq)
        });
    Ok(Admission {
        occurrence_ids,
        already_applied: checkpoint_commit_seq > identity.through_commit_seq
            || stored
                .as_ref()
                .is_some_and(|stored| stored.checkpoint_commit_seq == identity.through_commit_seq),
        checkpoint_commit_seq,
    })
}

fn apply_batch_inner(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
    bounds: BatchBounds,
    now: i64,
    fault: Fault,
) -> Result<BatchOutcome, ProjectionError> {
    let admission = admit(conn, batch, bounds)?;
    phase!(fault, AfterAdmission);

    let mut outcome = BatchOutcome {
        checkpoint_commit_seq: admission.checkpoint_commit_seq,
        ..BatchOutcome::default()
    };
    let persisted = persist_occurrences(conn, &batch.records, bounds.persist, now)?;
    for (row, expected) in persisted.iter().zip(&admission.occurrence_ids) {
        if row.occurrence_id != *expected {
            return Err(ProjectionError::CorruptRow);
        }
        if row.inserted {
            outcome.rows_inserted += 1;
        } else {
            outcome.rows_replayed += 1;
        }
    }
    phase!(fault, AfterRows);

    let mut tombstoned: HashSet<&str> = HashSet::new();
    for invalidation in &batch.invalidations {
        if tombstone_occurrence(
            conn,
            &invalidation.occurrence_id,
            invalidation.tombstone,
            now,
        )? {
            outcome.tombstones_recorded += 1;
        }
        tombstoned.insert(invalidation.occurrence_id.as_str());
    }
    phase!(fault, AfterTombstones);

    if let Some(generation) = batch.generation_id {
        for (record, row) in batch.records.iter().zip(&persisted) {
            let live = !tombstoned.contains(row.occurrence_id.as_str())
                && !has_tombstone(conn, &row.occurrence_id)?;
            if !dense_eligible(record.occurrence.class) || !live {
                continue;
            }
            let job_id = job_id(&row.occurrence_id, generation);
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO embedding_jobs(
                     job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at
                 ) VALUES (?1,?2,?3,'pending',0,?4,?4)",
                params![job_id, row.occurrence_id, generation, now],
            )?;
            outcome.pending_created += inserted;
        }
    }
    // Work queued for an occurrence that stopped being live is obsolete,
    // whatever generation queued it.
    for occurrence_id in &tombstoned {
        outcome.pending_obsoleted += conn.execute(
            "UPDATE embedding_jobs SET state='obsolete',updated_at=?2
             WHERE occurrence_id=?1 AND state IN ('pending','admitted')",
            params![occurrence_id, now],
        )?;
    }
    phase!(fault, AfterPending);

    if !admission.already_applied {
        conn.execute(
            "INSERT INTO projection_checkpoint(singleton,snapshot_commit_seq,checkpoint_commit_seq,hold_id,updated_at)
             VALUES (1,?1,?2,?3,?4)
             ON CONFLICT(singleton) DO UPDATE SET checkpoint_commit_seq=excluded.checkpoint_commit_seq,
                 updated_at=excluded.updated_at",
            params![
                batch.identity.snapshot_commit_seq,
                batch.identity.through_commit_seq,
                batch.identity.hold_id,
                now
            ],
        )?;
    }
    phase!(fault, AfterCheckpoint);
    Ok(outcome)
}

fn has_tombstone(conn: &GuardedConn<'_>, occurrence_id: &str) -> Result<bool, ProjectionError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM occurrence_tombstones WHERE occurrence_id=?1)",
        [occurrence_id],
        |row| row.get(0),
    )?)
}

/// Records a vector generation the batches may queue work for. Registering
/// the same generation again with the same identity is a no-op; a different
/// identity under the same id is refused.
pub fn register_generation(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
    now: i64,
) -> Result<bool, ProjectionError> {
    let stored: Option<(String, String, i64, i64)> = conn
        .query_row(
            "SELECT embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch
             FROM vector_generations WHERE generation_id=?1",
            [&generation.generation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    match stored {
        Some((model, fingerprint, dimension, epoch))
            if model == generation.embedding_model
                && fingerprint == generation.tokenizer_fingerprint
                && dimension == i64::from(generation.vector_dimension)
                && u64::try_from(epoch).ok() == Some(generation.generation_epoch) =>
        {
            Ok(false)
        }
        Some(_) => Err(ProjectionError::IdentityMismatch),
        None => {
            conn.execute(
                "INSERT INTO vector_generations(
                     generation_id,embedding_model,tokenizer_fingerprint,vector_dimension,
                     generation_epoch,state,created_at,updated_at
                 ) VALUES (?1,?2,?3,?4,?5,'building',?6,?6)",
                params![
                    generation.generation_id,
                    generation.embedding_model,
                    generation.tokenizer_fingerprint,
                    generation.vector_dimension,
                    i64::try_from(generation.generation_epoch)
                        .map_err(|_| ProjectionError::CorruptRow)?,
                    now,
                ],
            )?;
            Ok(true)
        }
    }
}

/// The identity of one vector generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorGeneration {
    pub generation_id: String,
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub generation_epoch: u64,
}

/// Pending jobs in dispatch order, for the process-local job table to admit.
pub fn pending_jobs(conn: &GuardedConn<'_>) -> Result<Vec<(String, String)>, ProjectionError> {
    let mut statement = conn.prepare(
        "SELECT job_id,occurrence_id FROM embedding_jobs WHERE state='pending'
         ORDER BY next_attempt_at,job_id",
    )?;
    let jobs = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(jobs)
}
