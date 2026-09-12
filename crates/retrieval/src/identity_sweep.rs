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

/// A generation `g` releases its identities only once it is retired and a receipt records the retirement.
const RETIRED_GENERATION: &str = "g.state='retired' AND EXISTS(SELECT 1 FROM retirement_receipts r WHERE r.generation_id=g.generation_id)";

fn unreferenced() -> String {
    format!(
        "(EXISTS(SELECT 1 FROM vector_generations g WHERE g.generation_id=j.generation_id AND {RETIRED_GENERATION})
          OR EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=j.occurrence_id))"
    )
}

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
    pub job_id: String,
}

/// An empty candidate list can still carry a continuation cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidatePage {
    pub candidates: Vec<Candidate>,
    /// Job rows visited, whether or not they are eligible for reclamation.
    pub inspected: usize,
    /// The last inspected job ID, even when no candidates were found; `None` only for an empty page.
    pub last_job_id: Option<String>,
}

fn candidates_statement(has_after: bool) -> String {
    let range = if has_after { "WHERE j.job_id>?2" } else { "" };
    let unreferenced = unreferenced();
    format!(
        "SELECT j.occurrence_id,j.generation_id,j.state,j.host_job_id,
                EXISTS(SELECT 1 FROM occurrence_vectors v WHERE v.occurrence_id=j.occurrence_id AND v.generation_id=j.generation_id),
                j.job_id,({FINISHED_JOB} AND {unreferenced})
         FROM embedding_jobs j
         {range}
         ORDER BY j.job_id LIMIT ?1"
    )
}

/// The exact SQL `candidates` runs, so a test can measure its plan and cost against the baseline schema.
#[cfg(feature = "test-support")]
pub fn candidates_sql(has_after: bool) -> String {
    candidates_statement(has_after)
}

/// Inspects at most `limit` job rows after `after` and returns finished, unreferenced identities in job ID order.
///
/// Eligibility filters the page, so zero candidates does not mean exhaustion.
/// A full page's `last_job_id` is the continuation cursor; fewer than `limit` inspected rows means this snapshot reached the end.
pub fn candidates(
    conn: &GuardedConn<'_>,
    limit: NonZeroUsize,
    after: Option<&str>,
) -> Result<CandidatePage, ProjectionError> {
    let mut statement = conn.prepare(&candidates_statement(after.is_some()))?;
    // A limit above `i64::MAX` clamps instead of wrapping: SQLite reads a negative limit as no limit at all.
    let limit = i64::try_from(limit.get()).unwrap_or(i64::MAX);
    let mut rows = match after {
        Some(after) => statement.query(params![limit, after])?,
        None => statement.query([limit])?,
    };
    let mut page = CandidatePage::default();
    while let Some(row) = rows.next()? {
        let job_id: String = row.get(5)?;
        if row.get::<_, bool>(6)? {
            page.candidates.push(Candidate {
                occurrence_id: row.get(0)?,
                generation_id: row.get(1)?,
                state: row.get(2)?,
                host_job_id: row.get(3)?,
                has_vector: row.get(4)?,
                job_id: job_id.clone(),
            });
        }
        page.inspected += 1;
        page.last_job_id = Some(job_id);
    }
    Ok(page)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reclaimed {
    pub vectors: usize,
    pub jobs: usize,
    /// Candidates whose eligibility no longer held when the delete ran.
    pub survivors: usize,
}

/// Recovery authorizations prevent replay only while their parent job exists, so reclaim removes them in the same transaction as an eligible job.
pub fn reclaim(
    conn: &GuardedConn<'_>,
    candidates: &[Candidate],
) -> Result<Reclaimed, ProjectionError> {
    let mut reclaimed = Reclaimed::default();
    let unreferenced = unreferenced();
    let vectors = format!(
        "DELETE FROM occurrence_vectors WHERE occurrence_id=?1 AND generation_id=?2
         AND EXISTS(SELECT 1 FROM embedding_jobs j WHERE j.occurrence_id=?1 AND j.generation_id=?2 AND {FINISHED_JOB} AND {unreferenced})"
    );
    let authorizations = format!(
        "DELETE FROM embedding_recovery_authorizations WHERE job_id IN
         (SELECT j.job_id FROM embedding_jobs j WHERE j.occurrence_id=?1 AND j.generation_id=?2 AND {FINISHED_JOB} AND {unreferenced})"
    );
    let jobs = format!(
        "DELETE FROM embedding_jobs AS j WHERE j.occurrence_id=?1 AND j.generation_id=?2 AND {FINISHED_JOB} AND {unreferenced}"
    );
    let mut vectors = conn.prepare(&vectors)?;
    let mut authorizations = conn.prepare(&authorizations)?;
    let mut jobs = conn.prepare(&jobs)?;
    for candidate in candidates {
        let key = params![candidate.occurrence_id, candidate.generation_id];
        reclaimed.vectors += vectors.execute(key)?;
        authorizations.execute(key)?;
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
