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
use kernel::source_identity::{Occurrence, OccurrenceClass, Span};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use crate::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, Tombstone, TombstoneReason,
    persist_occurrences, tombstone_occurrence,
};

/// The batch's source prefix does not depend on vector generation.
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

/// Contains every source row and invalidation through `identity.through_commit_seq`.
/// The local writer cannot detect omitted source pages or control events.
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
    /// The batch ends strictly below the stored checkpoint, so none of its statements ran; `batch_status` is the way to interrogate such a window.
    pub older_prefix: bool,
}

/// A covering checkpoint alone cannot distinguish batches that request different vector generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStatus {
    /// The checkpoint covers the batch, rows and invalidations match, and required generation jobs exist.
    Applied,
    /// The checkpoint does not cover the batch, or a required effect is missing.
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
///
/// The caller must collect every page from the window's start through its terminal page before applying the batch.
/// Source pages are keyset-ordered, not commit-ordered; a partial page cannot justify a complete checkpoint.
/// Artifact deletion and other control events come from canonical commit history, not descriptor rows.
/// The caller must add their invalidations before applying the batch.
/// This mapper cannot verify either completeness obligation from a row slice.
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
        if !fields.iter().copied().eq(row
            .detail
            .identity
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())))
        {
            return Err(ProjectionError::MalformedBatch);
        }
        let occurrence = Occurrence {
            class: &row.detail.class,
            identity: fields,
            revision: &row.detail.revision,
            representation: &row.detail.representation,
            span: row.detail.span.map(|(start, end)| Span { start, end }),
        };
        if kernel::source_identity::encode_preserving_span(&occurrence)?.occurrence_id
            != row.detail.occurrence_id
        {
            return Err(ProjectionError::MalformedBatch);
        }
        if let Some(text) = row.text.as_deref() {
            if kernel::source_identity::payload_id(text.as_bytes()) != row.detail.payload_id {
                return Err(ProjectionError::MalformedBatch);
            }
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

/// Reconciles an unknown commit outcome from the checkpoint and the batch's durable effects in the caller's read transaction.
/// A covering checkpoint alone does not prove that work for the requested generation exists.
/// Jobs in any state count as applied; tombstoned records require no job.
/// Status does not identify which transaction produced the durable effects.
/// A missing or different installed kernel incarnation is an identity error.
/// Conflicting occurrence identity or payload bytes return collision errors.
/// A missing or different tombstone returns `NotApplied`; replay can still refuse a conflicting tombstone.
/// Recovery must supply the original records, including their payload variants and spans.
pub fn batch_status(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
) -> Result<BatchStatus, ProjectionError> {
    let identity = &batch.identity;
    let Some(checkpoint) = read_checkpoint(conn, &identity.kernel_incarnation_id)? else {
        return Ok(BatchStatus::NotApplied);
    };
    if checkpoint.hold_id != identity.hold_id
        || checkpoint.snapshot_commit_seq != identity.snapshot_commit_seq
        || checkpoint.checkpoint_commit_seq < identity.through_commit_seq
    {
        return Ok(BatchStatus::NotApplied);
    }
    for record in &batch.records {
        let (encoded, selected) = crate::encode_record(record)?;
        let (occurrence_id, payload_id) = crate::canonical_digests(&encoded, selected);
        let Some(stored) = crate::read_occurrence(conn, &occurrence_id)? else {
            return Ok(BatchStatus::NotApplied);
        };
        if stored.tuple != encoded.tuple || stored.payload_id != payload_id {
            return Err(ProjectionError::OccurrenceCollision { occurrence_id });
        }
        if stored.bytes != selected {
            return Err(ProjectionError::PayloadCollision {
                payload_id: stored.payload_id,
            });
        }
        if let Some(generation) = batch.generation_id
            && dense_eligible(record.occurrence.class)
            && stored.tombstone.is_none()
            && !has_job(conn, &occurrence_id, generation)?
        {
            return Ok(BatchStatus::NotApplied);
        }
    }
    for invalidation in &batch.invalidations {
        let applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM occurrence_tombstones
             WHERE occurrence_id=?1 AND invalidated_commit_seq=?2 AND reason=?3)
             AND NOT EXISTS(SELECT 1 FROM embedding_jobs
             WHERE occurrence_id=?1 AND state IN ('pending','admitted'))",
            params![
                invalidation.occurrence_id,
                invalidation.tombstone.invalidated_commit_seq,
                invalidation.tombstone.reason.as_str(),
            ],
            |row| row.get(0),
        )?;
        if !applied {
            return Ok(BatchStatus::NotApplied);
        }
    }
    Ok(BatchStatus::Applied)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionCheckpoint {
    pub snapshot_commit_seq: i64,
    pub checkpoint_commit_seq: i64,
    pub hold_id: String,
}

/// Reads the durable local prefix, or `None` before any batch has committed.
///
/// # Errors
///
/// Returns [`ProjectionError::IdentityMismatch`] when no identity is installed
/// or the installed identity names another kernel incarnation.
pub fn read_checkpoint(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
) -> Result<Option<ProjectionCheckpoint>, ProjectionError> {
    let installed = crate::read_identity(conn)?.ok_or(ProjectionError::IdentityMismatch)?;
    if installed.kernel_incarnation_id != kernel_incarnation_id {
        return Err(ProjectionError::IdentityMismatch);
    }
    conn.query_row(
        "SELECT snapshot_commit_seq,checkpoint_commit_seq,hold_id FROM projection_checkpoint WHERE singleton=1",
        [],
        |row| {
            Ok(ProjectionCheckpoint {
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
/// The caller must establish the source completeness required by [`ProjectionBatch`] before calling.
/// Admission checks local bounds and identities, not canonical history coverage.
pub fn apply_batch(
    conn: &GuardedConn<'_>,
    batch: &ProjectionBatch<'_>,
    bounds: BatchBounds,
    now: i64,
) -> Result<BatchOutcome, ProjectionError> {
    apply_batch_inner(conn, batch, bounds, now, NO_FAULT)
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
#[cfg(feature = "test-support")]
const NO_FAULT: Fault = None;

/// A production build has no fault to inject; the phases still run in the
/// same order.
#[cfg(not(feature = "test-support"))]
#[derive(Clone, Copy)]
struct Fault;
#[cfg(not(feature = "test-support"))]
const NO_FAULT: Fault = Fault;

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
struct Admission<'a> {
    occurrence_ids: Vec<String>,
    tombstoned: HashSet<&'a str>,
    /// Whether the stored checkpoint is exactly the batch's end, so the batch re-validates its rows without moving the checkpoint.
    already_applied: bool,
    /// Whether a later window already moved the checkpoint past the batch's end. Such a batch is an older prefix: its rows may since have been tombstoned by later windows or reclaimed by cleanup, so running its statements again could only contradict state that supersedes it.
    older_prefix: bool,
    /// The checkpoint after the batch.
    checkpoint_commit_seq: i64,
}

/// Admission and job insertion use this predicate to apply identical
/// eligibility rules.
fn queues_work(
    conn: &GuardedConn<'_>,
    class: &str,
    occurrence_id: &str,
    tombstoned: &HashSet<&str>,
) -> Result<bool, ProjectionError> {
    Ok(dense_eligible(class)
        && !tombstoned.contains(occurrence_id)
        && !has_tombstone(conn, occurrence_id)?)
}

fn admit<'a>(
    conn: &GuardedConn<'_>,
    batch: &'a ProjectionBatch<'_>,
    bounds: BatchBounds,
) -> Result<Admission<'a>, ProjectionError> {
    let identity = &batch.identity;
    if identity.through_commit_seq < identity.snapshot_commit_seq
        || identity.snapshot_commit_seq < 0
        || identity.hold_id.is_empty()
        || batch
            .records
            .iter()
            .any(|record| record.created_commit_seq > identity.through_commit_seq)
        || batch.invalidations.iter().any(|invalidation| {
            let at = invalidation.tombstone.invalidated_commit_seq;
            at <= identity.snapshot_commit_seq || at > identity.through_commit_seq
        })
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
        .map(|record| crate::encode_record(record).map(|(encoded, _)| encoded.occurrence_id))
        .collect::<Result<Vec<_>, _>>()?;
    let tombstoned: HashSet<&str> = batch
        .invalidations
        .iter()
        .map(|invalidation| invalidation.occurrence_id.as_str())
        .collect();
    // A checkpoint for kernel_incarnation_id retains hold_id and snapshot_commit_seq; checkpoint_commit_seq never decreases.
    let stored = read_checkpoint(conn, &identity.kernel_incarnation_id)?;
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
    let admission = Admission {
        occurrence_ids,
        tombstoned,
        already_applied: stored
            .as_ref()
            .is_some_and(|stored| stored.checkpoint_commit_seq == identity.through_commit_seq),
        older_prefix: checkpoint_commit_seq > identity.through_commit_seq,
        checkpoint_commit_seq,
    };
    // Older prefixes skip queue and generation validation because cleanup can remove their rows, making a replay appear new.
    if admission.older_prefix {
        return Ok(admission);
    }
    if let Some(generation) = batch.generation_id {
        let outstanding: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embedding_jobs WHERE state IN ('pending','admitted')",
            [],
            |row| row.get(0),
        )?;
        let outstanding = usize::try_from(outstanding).map_err(|_| ProjectionError::CorruptRow)?;
        // The pending bound excludes queued work invalidated by this batch.
        let mut obsoleting = 0usize;
        for occurrence_id in &admission.tombstoned {
            let queued: i64 = conn.query_row(
                "SELECT COUNT(*) FROM embedding_jobs
                 WHERE occurrence_id=?1 AND state IN ('pending','admitted')",
                [occurrence_id],
                |row| row.get(0),
            )?;
            obsoleting += usize::try_from(queued).map_err(|_| ProjectionError::CorruptRow)?;
        }
        let mut new_jobs = HashSet::new();
        for (record, occurrence_id) in batch.records.iter().zip(&admission.occurrence_ids) {
            if !queues_work(
                conn,
                record.occurrence.class,
                occurrence_id,
                &admission.tombstoned,
            )? {
                continue;
            }
            if !has_job(conn, occurrence_id, generation)? {
                new_jobs.insert(occurrence_id);
            }
        }
        let total = outstanding
            .saturating_sub(obsoleting)
            .saturating_add(new_jobs.len());
        if total > bounds.max_pending.get() {
            return Err(ProjectionError::BatchOverBound {
                bound: "pending",
                size: total,
            });
        }
        let known: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM vector_generations
             WHERE generation_id=?1 AND (state<>'retired' OR ?2))",
            params![generation, new_jobs.is_empty()],
            |row| row.get(0),
        )?;
        if !known {
            return Err(ProjectionError::UnknownGeneration {
                generation_id: generation.to_string(),
            });
        }
    }
    Ok(admission)
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
    if admission.older_prefix {
        outcome.older_prefix = true;
        return Ok(outcome);
    }
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

    for invalidation in &batch.invalidations {
        if tombstone_occurrence(
            conn,
            &invalidation.occurrence_id,
            invalidation.tombstone,
            now,
        )? {
            outcome.tombstones_recorded += 1;
        }
    }
    phase!(fault, AfterTombstones);

    let tombstoned = &admission.tombstoned;
    if let Some(generation) = batch.generation_id {
        for (record, row) in batch.records.iter().zip(&persisted) {
            if !queues_work(
                conn,
                record.occurrence.class,
                &row.occurrence_id,
                tombstoned,
            )? {
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
    for occurrence_id in tombstoned {
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

fn has_job(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    generation_id: &str,
) -> Result<bool, ProjectionError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2)",
        params![occurrence_id, generation_id],
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
    match registered_generation(conn, generation)? {
        Some(RegisteredGeneration {
            identity_matches: true,
            ..
        }) => Ok(false),
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

/// The stored row for a generation id, judged against the identity a caller presents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegisteredGeneration {
    /// Every identity field of the stored row agrees with the presented generation.
    pub(crate) identity_matches: bool,
    /// The stored lifecycle state: `building`, `verified`, `selected`, or `retired`.
    pub(crate) state: String,
}

impl RegisteredGeneration {
    pub(crate) fn is_retired(&self) -> bool {
        self.state == "retired"
    }
}

/// Reads `generation.generation_id`'s row, if registered, and compares its identity with `generation`.
pub(crate) fn registered_generation(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
) -> Result<Option<RegisteredGeneration>, ProjectionError> {
    let stored: Option<(String, String, i64, i64, String)> = conn
        .query_row(
            "SELECT embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch,state
             FROM vector_generations WHERE generation_id=?1",
            [&generation.generation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    Ok(stored.map(
        |(model, fingerprint, dimension, epoch, state)| RegisteredGeneration {
            identity_matches: model == generation.embedding_model
                && fingerprint == generation.tokenizer_fingerprint
                && dimension == i64::from(generation.vector_dimension)
                && u64::try_from(epoch).ok() == Some(generation.generation_epoch),
            state,
        },
    ))
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
