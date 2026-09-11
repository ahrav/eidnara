//! Completes one durable embedding job with its vector in one statement group, so a job is `embedded` only where its matching vector exists.
//!
//! The vector is `vector_dimension` little-endian f32 values and must be finite; the unit-norm tolerance belongs to the inference owner and is judged before the vector reaches this module.
//! Completion compares the stored occurrence and the registered generation with the identity the vector was produced for.
//! A disagreement never completes the job: a tombstoned or replaced input makes the job obsolete, and a vector that disagrees with one already stored is a conflict the caller must not resolve automatically.

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::ProjectionError;
use crate::batch::{VectorGeneration, registered_generation};

/// One vector for one occurrence, with the identity it was produced under.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorCompletion<'a> {
    pub occurrence_id: &'a str,
    pub generation: &'a VectorGeneration,
    /// The canonical object whose descriptor selected the occurrence.
    pub source_object_id: &'a str,
    /// The canonical artifact whose bytes produced the payload.
    pub source_artifact_digest: &'a str,
    /// The payload identity the embedded bytes were selected from.
    pub payload_id: &'a str,
    pub vector: &'a [f32],
    pub input_bytes: u64,
    pub input_tokens: u32,
}

/// Why a stored job cannot be completed by this vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsoleteReason {
    /// The occurrence has a tombstone.
    Tombstoned,
    /// The occurrence's payload is not the one the vector was produced from.
    PayloadChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// The vector and the completion are now durable together.
    Embedded,
    /// The same vector was already durable; nothing changed.
    Replayed,
    /// The input is no longer current; the job is `obsolete` and no vector was written.
    Obsolete(ObsoleteReason),
}

/// The durable state of one job, for reconciling an unknown commit outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionStatus {
    pub job_state: Option<String>,
    /// The stored vector bytes, as [`encode`] lays them out.
    pub vector: Option<Vec<u8>>,
}

impl CompletionStatus {
    /// A durable vector is coverage on its own; job bookkeeping that has not caught up does not withdraw it.
    pub fn has_durable_vector(&self, vector: &[f32]) -> bool {
        self.vector.as_deref() == Some(encode(vector).as_slice())
    }
}

/// One mutation of a new completion, reported as it lands inside the caller's transaction; a replay or an obsoletion reports none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionPhase {
    /// The vector row is inserted; the job row is not yet `embedded`.
    VectorInserted,
    /// The job row is `embedded`.
    JobEmbedded,
}

/// Completes the job for `completion.occurrence_id` under `completion.generation` inside the caller's transaction, reporting each mutation to `observer` as it lands while the transaction is still open.
/// Only a `pending` or `admitted` job accepts a vector or becomes obsolete; a job already `embedded` or `published` is judged by its stored vector alone (a replay or a conflict), and a job in any other state is closed to completion.
///
/// # Errors
///
/// In the order checked: [`ProjectionError::InvalidVector`] when the vector is not `vector_dimension` finite values, [`ProjectionError::UnknownGeneration`], [`ProjectionError::IdentityMismatch`], or [`ProjectionError::RetiredGeneration`] when the generation is missing, registered under another identity, or retired, [`ProjectionError::UnknownOccurrence`] when the occurrence is not stored, [`ProjectionError::NoPendingWork`] when no job is open for the pair, [`ProjectionError::VectorConflict`] when a different vector is already durable for the pair, and [`ProjectionError::CorruptRow`] when a completed job has no durable vector.
pub fn complete_embedding_observed(
    conn: &GuardedConn<'_>,
    completion: &VectorCompletion<'_>,
    now: i64,
    observer: &mut dyn FnMut(CompletionPhase),
) -> Result<CompletionOutcome, ProjectionError> {
    let generation = completion.generation;
    if completion.vector.len() != generation.vector_dimension as usize {
        return Err(ProjectionError::InvalidVector {
            reason: "dimension",
        });
    }
    if completion.vector.iter().any(|value| !value.is_finite()) {
        return Err(ProjectionError::InvalidVector {
            reason: "nonfinite",
        });
    }
    check_generation(conn, generation)?;
    let Some((payload_id, tombstoned, source_object_id, source_artifact_digest)) = conn
        .query_row(
            "SELECT o.payload_id,
                    EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id),
                    o.source_object_id,o.source_artifact_digest
             FROM occurrences o WHERE o.occurrence_id=?1",
            [completion.occurrence_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Err(ProjectionError::UnknownOccurrence {
            occurrence_id: completion.occurrence_id.to_owned(),
        });
    };
    if source_object_id != completion.source_object_id
        || source_artifact_digest != completion.source_artifact_digest
    {
        return Err(ProjectionError::CorruptRow);
    }
    let job_state: Option<String> = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            params![completion.occurrence_id, generation.generation_id],
            |row| row.get(0),
        )
        .optional()?;
    let open = match job_state.as_deref() {
        Some("pending" | "admitted") => true,
        Some("embedded" | "published") => false,
        _ => {
            return Err(ProjectionError::NoPendingWork {
                occurrence_id: completion.occurrence_id.to_owned(),
            });
        }
    };
    let obsolete = if tombstoned {
        Some(ObsoleteReason::Tombstoned)
    } else if payload_id != completion.payload_id {
        Some(ObsoleteReason::PayloadChanged)
    } else {
        None
    };
    // A completed job is closed to obsoletion as it is to completion; the stored vector alone judges a redelivery to it.
    if let Some(reason) = obsolete
        && open
    {
        conn.execute(
            "UPDATE embedding_jobs SET state='obsolete',updated_at=?3
             WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
            params![completion.occurrence_id, generation.generation_id, now],
        )?;
        return Ok(CompletionOutcome::Obsolete(reason));
    }
    let encoded = encode(completion.vector);
    let stored: Option<Vec<u8>> = conn
        .query_row(
            "SELECT vector FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2",
            params![completion.occurrence_id, generation.generation_id],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        Some(bytes) if bytes == encoded => {
            // The same vector is already durable; the job row follows it if a crash left it behind.
            conn.execute(
                "UPDATE embedding_jobs SET state='embedded',updated_at=?3
                 WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
                params![completion.occurrence_id, generation.generation_id, now],
            )?;
            return Ok(CompletionOutcome::Replayed);
        }
        Some(_) => {
            return Err(ProjectionError::VectorConflict {
                occurrence_id: completion.occurrence_id.to_owned(),
            });
        }
        // A completed job requires a vector.
        None if !open => {
            return Err(ProjectionError::CorruptRow);
        }
        None => {}
    }
    conn.execute(
        "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            completion.occurrence_id,
            generation.generation_id,
            encoded,
            generation.vector_dimension,
            i64::try_from(completion.input_bytes).map_err(|_| ProjectionError::InvalidVector {
                reason: "input_bytes",
            })?,
            completion.input_tokens,
            now,
        ],
    )?;
    observer(CompletionPhase::VectorInserted);
    conn.execute(
        "UPDATE embedding_jobs SET state='embedded',updated_at=?3
         WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
        params![completion.occurrence_id, generation.generation_id, now],
    )?;
    observer(CompletionPhase::JobEmbedded);
    Ok(CompletionOutcome::Embedded)
}

/// Reads the durable job state and vector bytes for the pair, for reconciling a commit whose reply was lost.
pub fn completion_status(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    generation_id: &str,
) -> Result<CompletionStatus, ProjectionError> {
    let job_state = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation_id],
            |row| row.get(0),
        )
        .optional()?;
    let vector = conn
        .query_row(
            "SELECT vector FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(CompletionStatus { job_state, vector })
}

/// What obsoleting a job found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Obsoletion {
    /// An open job is now `obsolete`.
    Marked,
    /// The job was already in a terminal state, which keeps its vector if it has one.
    AlreadyTerminal,
    /// No job exists for the pair.
    NoJob,
}

/// Marks the pair's open job obsolete because the kernel no longer holds its input.
pub fn obsolete_embedding(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    generation_id: &str,
    source_object_id: &str,
    source_artifact_digest: &str,
    payload_id: &str,
    now: i64,
) -> Result<Obsoletion, ProjectionError> {
    let job_exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2
         )",
        params![occurrence_id, generation_id],
        |row| row.get(0),
    )?;
    if !job_exists {
        return Ok(Obsoletion::NoJob);
    }
    let Some((stored_object_id, stored_artifact_digest, stored_payload_id)) = conn
        .query_row(
            "SELECT source_object_id,source_artifact_digest,payload_id
             FROM occurrences WHERE occurrence_id=?1",
            [occurrence_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Err(ProjectionError::CorruptRow);
    };
    if stored_object_id != source_object_id
        || stored_artifact_digest != source_artifact_digest
        || stored_payload_id != payload_id
    {
        return Err(ProjectionError::IdentityMismatch);
    }
    let changed = conn.execute(
        "UPDATE embedding_jobs SET state='obsolete',updated_at=?3
         WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
        params![occurrence_id, generation_id, now],
    )?;
    if changed == 1 {
        return Ok(Obsoletion::Marked);
    }
    Ok(Obsoletion::AlreadyTerminal)
}

/// A retired generation's files may already be reclaimed, so it accepts no completion; the same rule keeps `apply_batch` from queuing new work for it.
fn check_generation(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
) -> Result<(), ProjectionError> {
    let Some(registered) = registered_generation(conn, generation)? else {
        return Err(ProjectionError::UnknownGeneration {
            generation_id: generation.generation_id.clone(),
        });
    };
    if !registered.identity_matches {
        return Err(ProjectionError::IdentityMismatch);
    }
    if registered.is_retired() {
        return Err(ProjectionError::RetiredGeneration {
            generation_id: generation.generation_id.clone(),
        });
    }
    Ok(())
}

/// Little-endian f32 values, four bytes each, as the schema stores them.
pub fn encode(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
