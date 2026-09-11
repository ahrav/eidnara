//! Reclaims embedding identities that nothing references any more: the vectors
//! and finished job rows of a retired generation, and of occurrences that have
//! stopped being live. Everything else is protected by construction: current
//! occurrences and their vectors, every unfinished job whatever its retry
//! disposition, and any row whose host job may still hold a native input or a
//! result lease. Payloads and tombstones are never candidates; an occurrence
//! row keeps its payload reference for replay, so one obsolete occurrence can
//! never take shared bytes with it.
//!
//! Selection and reclamation are separate observations. `candidates` reads a
//! bounded set under one snapshot; `reclaim` deletes each candidate only while
//! the same eligibility still holds inside the caller's write transaction, so a
//! row that became referenced in between survives. Deleting a row is not
//! secure erasure of its bytes.

use std::num::NonZeroUsize;

use rusqlite::params;
use storage::GuardedConn;

use crate::ProjectionError;

/// A job row is finished when no obligation remains on it; pending, admitted, and stopped rows are obligations whatever their retry disposition.
const FINISHED_JOB: &str = "j.state IN ('embedded','published','obsolete')";

/// The identity is unreferenced when its generation was retired with a receipt, or its occurrence stopped being live.
const UNREFERENCED: &str = "(EXISTS(SELECT 1 FROM vector_generations g JOIN retirement_receipts r ON r.generation_id=g.generation_id
                                    WHERE g.generation_id=j.generation_id AND g.state='retired')
                             OR EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=j.occurrence_id))";

/// One finished, unreferenced identity. It names rows, never payload text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub occurrence_id: String,
    pub generation_id: String,
    /// `embedded` and `published` rows consumed their result; an `obsolete` row was cut short and its host job may still run.
    pub state: String,
    /// The host job the row last named; the caller decides whether that host still holds it.
    pub host_job_id: Option<String>,
    pub has_vector: bool,
}

/// The finished, unreferenced identities, oldest job identifier first, at most `limit`.
pub fn candidates(
    conn: &GuardedConn<'_>,
    limit: NonZeroUsize,
) -> Result<Vec<Candidate>, ProjectionError> {
    let mut statement = conn.prepare(&format!(
        "SELECT j.occurrence_id,j.generation_id,j.state,j.host_job_id,
                EXISTS(SELECT 1 FROM occurrence_vectors v WHERE v.occurrence_id=j.occurrence_id AND v.generation_id=j.generation_id)
         FROM embedding_jobs j
         WHERE {FINISHED_JOB} AND {UNREFERENCED}
         ORDER BY j.job_id LIMIT ?1"
    ))?;
    let rows = statement.query_map([limit.get() as i64], |row| {
        Ok(Candidate {
            occurrence_id: row.get(0)?,
            generation_id: row.get(1)?,
            state: row.get(2)?,
            host_job_id: row.get(3)?,
            has_vector: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reclaimed {
    pub vectors: usize,
    pub jobs: usize,
    /// Candidates whose eligibility no longer held when the delete ran.
    pub survivors: usize,
}

/// Deletes each candidate's vector and job row, each only while the row is still finished and unreferenced. Payloads, occurrences, tombstones, generations, and receipts are never touched.
pub fn reclaim(
    conn: &GuardedConn<'_>,
    candidates: &[Candidate],
) -> Result<Reclaimed, ProjectionError> {
    let mut reclaimed = Reclaimed::default();
    let vectors = format!(
        "DELETE FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2
         AND EXISTS(SELECT 1 FROM embedding_jobs j WHERE j.occurrence_id=?1 AND j.generation_id=?2 AND {FINISHED_JOB} AND {UNREFERENCED})"
    );
    let jobs = format!(
        "DELETE FROM embedding_jobs AS j WHERE j.occurrence_id=?1 AND j.generation_id=?2 AND {FINISHED_JOB} AND {UNREFERENCED}"
    );
    let mut vectors = conn.prepare(&vectors)?;
    let mut jobs = conn.prepare(&jobs)?;
    for candidate in candidates {
        let key = params![candidate.occurrence_id, candidate.generation_id];
        reclaimed.vectors += vectors.execute(key)?;
        let removed = jobs.execute(key)?;
        reclaimed.jobs += removed;
        if removed == 0 {
            reclaimed.survivors += 1;
        }
    }
    Ok(reclaimed)
}

/// Which of the candidate's rows are still present, as `(job, vector)`, for reconciling a reclamation whose reply was lost.
pub fn presence(
    conn: &GuardedConn<'_>,
    candidate: &Candidate,
) -> Result<(bool, bool), ProjectionError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2),
                EXISTS(SELECT 1 FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2)",
        params![candidate.occurrence_id, candidate.generation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}
