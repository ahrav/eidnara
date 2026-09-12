//! Completes one durable embedding job with its vector in one statement group, so a job is `embedded` only where its matching vector exists.
//!
//! The vector is `vector_dimension` little-endian f32 values and must be finite; the unit-norm tolerance belongs to the inference owner and is judged before the vector reaches this module.
//! Completion compares the stored occurrence and the registered generation with the identity the vector was produced for.
//! A disagreement never completes the job: a tombstoned or replaced input makes the job obsolete, and a vector that disagrees with one already stored is a conflict the caller must not resolve automatically.

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::batch::{VectorGeneration, registered_generation};
use crate::{ProjectionError, StoredOccurrence, read_occurrence};
use kernel::CurrentInputDescriptor;

/// One vector for one occurrence, with the identity it was produced under.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorCompletion<'a> {
    pub input: &'a CurrentInputDescriptor,
    pub generation: &'a VectorGeneration,
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
/// Only a `pending` or `admitted` job accepts a vector or changes to `obsolete`; every job reports a tombstoned or replaced occurrence before replaying its stored vector, and a job in any other state is closed to completion.
/// A caller holding a current-input guard treats an obsolete outcome as projection corruption and rolls the transaction back, because the guard has already ruled out ordinary staleness.
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
    let occurrence_id = &completion.input.detail.occurrence_id;
    let Some(stored_occurrence) = read_occurrence(conn, occurrence_id)? else {
        return Err(ProjectionError::UnknownOccurrence {
            occurrence_id: occurrence_id.clone(),
        });
    };
    if !occurrence_matches(&stored_occurrence, completion.input) {
        return Err(ProjectionError::CorruptRow);
    }
    let job_state: Option<String> = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation.generation_id],
            |row| row.get(0),
        )
        .optional()?;
    let open = match job_state.as_deref() {
        Some("pending" | "admitted") => true,
        Some("embedded" | "published") => false,
        _ => {
            return Err(ProjectionError::NoPendingWork {
                occurrence_id: occurrence_id.clone(),
            });
        }
    };
    let obsolete = if stored_occurrence.tombstone.is_some() {
        Some(ObsoleteReason::Tombstoned)
    } else if stored_occurrence.payload_id != completion.input.detail.payload_id {
        Some(ObsoleteReason::PayloadChanged)
    } else {
        None
    };
    if let Some(reason) = obsolete {
        if open {
            conn.execute(
                "UPDATE embedding_jobs SET state='obsolete',updated_at=?3
                 WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
                params![occurrence_id, generation.generation_id, now],
            )?;
        }
        return Ok(CompletionOutcome::Obsolete(reason));
    }
    let encoded = encode(completion.vector);
    let stored: Option<Vec<u8>> = conn
        .query_row(
            "SELECT vector FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation.generation_id],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        Some(bytes) if bytes == encoded => {
            // The same vector is already durable; the job row follows it if a crash left it behind.
            conn.execute(
                "UPDATE embedding_jobs SET state='embedded',updated_at=?3
                 WHERE occurrence_id=?1 AND generation_id=?2 AND state IN ('pending','admitted')",
                params![occurrence_id, generation.generation_id, now],
            )?;
            return Ok(CompletionOutcome::Replayed);
        }
        Some(_) => {
            return Err(ProjectionError::VectorConflict {
                occurrence_id: occurrence_id.clone(),
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
            occurrence_id,
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
        params![occurrence_id, generation.generation_id, now],
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
    input: &CurrentInputDescriptor,
    generation_id: &str,
    now: i64,
) -> Result<Obsoletion, ProjectionError> {
    let occurrence_id = &input.detail.occurrence_id;
    let job_state: Option<String> = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            params![occurrence_id, generation_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_state) = job_state else {
        return Ok(Obsoletion::NoJob);
    };
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
    if stored_object_id != input.object_id
        || stored_artifact_digest != input.detail.artifact_digest
        || stored_payload_id != input.detail.payload_id
    {
        return Err(ProjectionError::IdentityMismatch);
    }
    let Some(stored_occurrence) = read_occurrence(conn, occurrence_id)? else {
        return Err(ProjectionError::CorruptRow);
    };
    if !occurrence_matches(&stored_occurrence, input) {
        return Err(ProjectionError::CorruptRow);
    }
    if matches!(job_state.as_str(), "embedded" | "published") {
        let has_vector: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM occurrence_vectors
             WHERE occurrence_id=?1 AND generation_id=?2)",
            params![occurrence_id, generation_id],
            |row| row.get(0),
        )?;
        if !has_vector {
            return Err(ProjectionError::CorruptRow);
        }
        return Ok(Obsoletion::AlreadyTerminal);
    }
    if job_state == "obsolete" || job_state == "failed" {
        return Ok(Obsoletion::AlreadyTerminal);
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

fn occurrence_matches(stored: &StoredOccurrence, input: &CurrentInputDescriptor) -> bool {
    let detail = &input.detail;
    stored.occurrence_id == detail.occurrence_id
        && stored.tuple == detail.occurrence_tuple
        && stored.lineage_id == detail.lineage_id
        && stored.class == detail.class
        && stored.revision == input.source_revision
        && stored.representation == detail.representation
        && stored.span == detail.span
        && stored.domain_id == input.domain_id
        && stored.sensitivity == input.sensitivity
        && stored.source_object_id == input.object_id
        && stored.source_evidence_id == detail.evidence_id
        && stored.source_artifact_digest == detail.artifact_digest
        && stored.created_commit_seq == input.created_commit_seq
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
