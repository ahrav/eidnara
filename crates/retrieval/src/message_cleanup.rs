//! Reclaims message-index state that is obsolete under the projection's own facts: a `messages` occurrence that is tombstoned, whose tombstone lies at or below both the acknowledged kernel prefix the caller names and the projection's own checkpoint, and on which no embedding job row remains. Its vectors, its occurrence row, its tombstone, and a payload row no other occurrence references are removed together. Everything else is protected by construction: live occurrences, tombstones above the acknowledged prefix (a window still being reconciled compares its invalidations against them), and any occurrence with a job row of any state, which may still name a host input, a result lease, or a retry obligation.
//!
//! Selection and reclamation are separate observations: `reclaim` deletes a candidate only while the eligibility it was selected under, and the same tombstone, still hold inside the caller's write transaction. Both derive the cap through the projection's identity, so a projection from another kernel incarnation is refused rather than compared. Deleting a row is not secure erasure of its bytes.

use std::num::NonZeroUsize;

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use crate::ProjectionError;
use crate::batch::read_checkpoint;

const CLASS: &str = "messages";

/// A candidate is eligible while its tombstone sits at or below the cap and no job row names it.
const ELIGIBLE: &str = "o.class='messages'
    AND t.invalidated_commit_seq<=?1
    AND NOT EXISTS(SELECT 1 FROM embedding_jobs j WHERE j.occurrence_id=o.occurrence_id)";

/// The projection's checkpoint caps the caller's prefix. An absent checkpoint yields `i64::MIN`, so no tombstone is eligible; `read_checkpoint` rejects a projection from another kernel incarnation, whose commit sequences are incomparable.
fn eligibility_cap(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    acknowledged_through: i64,
) -> Result<i64, ProjectionError> {
    Ok(
        read_checkpoint(conn, kernel_incarnation_id)?.map_or(i64::MIN, |checkpoint| {
            checkpoint.checkpoint_commit_seq.min(acknowledged_through)
        }),
    )
}

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

/// The tombstone key range drives the scan and each occurrence is probed by key: `+o.class` keeps the planner off `idx_occurrences_source`, which would visit every live message and sort, so a page costs what the page holds rather than what the table holds.
fn candidates_sql() -> String {
    format!(
        "SELECT o.occurrence_id,t.invalidated_commit_seq,({ELIGIBLE})
         FROM occurrence_tombstones t
         JOIN occurrences o ON o.occurrence_id=t.occurrence_id
         WHERE t.occurrence_id>?3 AND +o.class=?2
         ORDER BY t.occurrence_id
         LIMIT ?4"
    )
}

/// Reads at most `max` tombstoned `messages` occurrences after `after`, returning those eligible under `acknowledged_through`.
///
/// # Errors
///
/// Returns [`ProjectionError::IdentityMismatch`] if the projection belongs to another kernel incarnation; propagates SQLite errors.
pub fn candidates(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    acknowledged_through: i64,
    after: Option<&str>,
    max: NonZeroUsize,
) -> Result<CandidatePage, ProjectionError> {
    let cap = eligibility_cap(conn, kernel_incarnation_id, acknowledged_through)?;
    let limit = i64::try_from(max.get()).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(&candidates_sql())?;
    let rows = statement
        .query_map(params![cap, CLASS, after.unwrap_or(""), limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
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
/// Returns [`ProjectionError::IdentityMismatch`] if the projection belongs to another kernel incarnation; propagates SQLite errors. The caller's transaction decides whether anything committed.
pub fn reclaim(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    candidates: &[Candidate],
    acknowledged_through: i64,
) -> Result<Reclaimed, ProjectionError> {
    let cap = eligibility_cap(conn, kernel_incarnation_id, acknowledged_through)?;
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
                    cap,
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

/// # Errors
///
/// Fails only when the row lookup itself fails; an absent row is `Ok(false)`.
pub fn present(conn: &GuardedConn<'_>, candidate: &Candidate) -> Result<bool, ProjectionError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM occurrences WHERE occurrence_id=?1)",
        [&candidate.occurrence_id],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, StatementStatus, params};

    use super::candidates_sql;

    /// One page costs what the page holds, not what the table holds: the scan walks the tombstone key range and probes each occurrence by key, so no sort runs and the steps for a page stay flat while the live rows behind it grow.
    #[test]
    fn a_page_of_candidates_walks_the_tombstone_key_range_without_sorting() {
        let mut measured = Vec::new();
        for live in [1_024_i64, 16_384] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(crate::BASELINE).unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            conn.execute_batch(
                "INSERT INTO payloads VALUES ('p',x'61',1,0);
                 INSERT INTO projection_checkpoint(singleton,snapshot_commit_seq,checkpoint_commit_seq,hold_id,updated_at)
                 VALUES (1,1,100,'hold',0);",
            )
            .unwrap();
            conn.execute(
                "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<?1)
                 INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                     payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                     source_artifact_digest,created_commit_seq,persisted_at)
                 SELECT printf('%05d',n),x'00','lineage','messages',1,'text','p','domain','normal',
                     'source','evidence','digest',1,0 FROM ids",
                [live],
            )
            .unwrap();
            // Eight tombstones spread through the id space, each below the checkpoint.
            conn.execute(
                "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                 SELECT occurrence_id,2,'retired',0 FROM occurrences
                 WHERE CAST(occurrence_id AS INTEGER)%(?1/8)=0",
                [live],
            )
            .unwrap();
            let sql = candidates_sql();
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params![100, "messages", "", 4], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plan.first()
                    .is_some_and(|detail| detail.contains("occurrence_tombstones")),
                "the tombstone table does not drive the scan: {plan:?}"
            );
            assert!(
                plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "query plan sorts: {plan:?}"
            );
            let mut statement = conn.prepare(&sql).unwrap();
            let ids = statement
                .query_map(params![100, "messages", "", 4], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids.len(), 4);
            measured.push((
                live,
                statement.get_status(StatementStatus::Sort),
                statement.get_status(StatementStatus::VmStep),
            ));
        }
        let [(_, small_sorts, small_steps), (_, large_sorts, large_steps)] = measured[..] else {
            unreachable!()
        };
        assert_eq!((small_sorts, large_sorts), (0, 0), "{measured:?}");
        assert!(
            large_steps <= small_steps * 2,
            "a page's cost grew with the live rows behind it: {measured:?}"
        );
    }
}
