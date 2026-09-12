//! Reclaims message-index state that is obsolete under the projection's own facts: a `messages` occurrence that is tombstoned, whose tombstone lies at or below both the acknowledged kernel prefix the caller names and the projection's own checkpoint, and on which no embedding job row remains. Its vectors, its occurrence row, its tombstone, and a payload row no other occurrence references are removed together. Everything else is protected by construction: live occurrences, tombstones above the acknowledged prefix (a window still being reconciled compares its invalidations against them), and any occurrence with a job row of any state, which may still name a host input, a result lease, or a retry obligation.
//!
//! Selection and reclamation are separate observations: `candidates` reads a bounded page under one snapshot, and `reclaim` deletes each candidate only while the same eligibility, under the same tombstone, still holds inside the caller's write transaction. Deleting a row is not secure erasure of its bytes.

use std::num::NonZeroUsize;

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::ProjectionError;

const CLASS: &str = "messages";

/// A candidate is eligible while its tombstone sits at or below the acknowledged prefix, which the projection's own checkpoint caps whatever the caller claimed, and no job row names it.
const ELIGIBLE: &str = "o.class='messages'
    AND t.invalidated_commit_seq<=MIN(?1,(SELECT checkpoint_commit_seq FROM projection_checkpoint WHERE singleton=1))
    AND NOT EXISTS(SELECT 1 FROM embedding_jobs j WHERE j.occurrence_id=o.occurrence_id)";

/// One obsolete occurrence and the tombstone it was selected under. It names rows, never payload text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub occurrence_id: String,
    pub invalidated_commit_seq: i64,
}

/// One bounded page of candidates in occurrence id order; `last_occurrence_id` continues the scan even when the page is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidatePage {
    pub candidates: Vec<Candidate>,
    /// Tombstoned message occurrences visited, eligible or not.
    pub inspected: usize,
    pub last_occurrence_id: Option<String>,
}

/// Reads at most `max` tombstoned `messages` occurrences after `after`, returning those eligible under `acknowledged_through`.
///
/// # Errors
///
/// Returns the SQLite error.
pub fn candidates(
    conn: &GuardedConn<'_>,
    acknowledged_through: i64,
    after: Option<&str>,
    max: NonZeroUsize,
) -> Result<CandidatePage, ProjectionError> {
    let limit = i64::try_from(max.get()).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(&format!(
        "SELECT o.occurrence_id,t.invalidated_commit_seq,({ELIGIBLE})
         FROM occurrence_tombstones t
         JOIN occurrences o ON o.occurrence_id=t.occurrence_id
         WHERE o.class=?2 AND o.occurrence_id>?3
         ORDER BY o.occurrence_id
         LIMIT ?4"
    ))?;
    let rows = statement
        .query_map(
            params![acknowledged_through, CLASS, after.unwrap_or(""), limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut page = CandidatePage {
        inspected: rows.len(),
        last_occurrence_id: rows.last().map(|row| row.0.clone()),
        ..CandidatePage::default()
    };
    page.candidates = rows
        .into_iter()
        .filter(|row| row.2)
        .map(|(occurrence_id, invalidated_commit_seq, _)| Candidate {
            occurrence_id,
            invalidated_commit_seq,
        })
        .collect();
    Ok(page)
}

/// What one reclamation removed and left.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reclaimed {
    pub occurrences: usize,
    pub vectors: usize,
    /// Payload rows that lost their last referencing occurrence.
    pub payloads: usize,
    /// Candidates whose eligibility, or whose tombstone, no longer held inside the transaction.
    pub protected: usize,
}

/// Removes each candidate that is still eligible under `acknowledged_through`, and still carries the tombstone it was selected under, inside the caller's write transaction: vectors, the occurrence, its tombstone, and its payload when unreferenced.
///
/// # Errors
///
/// Returns the SQLite error; the caller's transaction decides whether anything committed.
pub fn reclaim(
    conn: &GuardedConn<'_>,
    candidates: &[Candidate],
    acknowledged_through: i64,
) -> Result<Reclaimed, ProjectionError> {
    let mut reclaimed = Reclaimed::default();
    let mut eligible = conn.prepare_cached(&format!(
        "SELECT o.payload_id FROM occurrences o
         JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE o.occurrence_id=?2 AND t.invalidated_commit_seq=?3 AND ({ELIGIBLE})"
    ))?;
    let mut vectors =
        conn.prepare_cached("DELETE FROM occurrence_vectors WHERE occurrence_id=?1")?;
    let mut tombstone =
        conn.prepare_cached("DELETE FROM occurrence_tombstones WHERE occurrence_id=?1")?;
    let mut occurrence = conn.prepare_cached("DELETE FROM occurrences WHERE occurrence_id=?1")?;
    let mut payload = conn.prepare_cached(
        "DELETE FROM payloads WHERE payload_id=?1
         AND NOT EXISTS(SELECT 1 FROM occurrences o WHERE o.payload_id=?1)",
    )?;
    for candidate in candidates {
        let payload_id: Option<String> = eligible
            .query_row(
                params![
                    acknowledged_through,
                    candidate.occurrence_id,
                    candidate.invalidated_commit_seq
                ],
                |row| row.get(0),
            )
            .optional()?;
        let Some(payload_id) = payload_id else {
            reclaimed.protected += 1;
            continue;
        };
        reclaimed.vectors += vectors.execute([&candidate.occurrence_id])?;
        tombstone.execute([&candidate.occurrence_id])?;
        reclaimed.occurrences += occurrence.execute([&candidate.occurrence_id])?;
        reclaimed.payloads += payload.execute([&payload_id])?;
    }
    Ok(reclaimed)
}
