//! Contradiction candidates: derived rows that name a decision a published
//! descriptor overlaps lexically. Each row is keyed by (project digest,
//! descriptor evidence id, decision object id) and holds ids, integers, and
//! times only. The store keeps the rows, bounds them per descriptor and per
//! project, and pages them; the daemon judges their liveness.

use rusqlite::params;

use context_core::canonical_json::is_lower_hex;
use context_core::redaction::{RedactionErrorKind, redact_durable_text};

use crate::{DurableWriteFamily, MemoryStore, MemoryStoreError, PreparedWrite, WriteDisposition};

/// The most rows one [`MemoryStore::candidates_for_decision`] page returns.
pub const MAX_CANDIDATE_PAGE: usize = 64;
/// The longest id a row stores.
const MAX_CANDIDATE_ID_BYTES: usize = 512;
/// Basis points: `score_bp = matched_terms * 10000 / decision_terms`.
const SCORE_SCALE: i64 = 10_000;

const COLUMNS: &str = "project_digest,evidence_id,decision_object_id,decision_id,matched_terms,\
     decision_terms,score_bp,publish_tip,kernel_incarnation_id,matcher_digest,created_at_ms";

/// One scored decision a descriptor's write carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateScore<'a> {
    pub decision_object_id: &'a str,
    pub decision_id: &'a str,
    pub matched_terms: u32,
    pub decision_terms: u32,
}

/// One descriptor's rows, all written in one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorCandidates<'a> {
    pub project_digest: &'a str,
    pub evidence_id: &'a str,
    /// The Kernel tip the scores were judged at.
    pub publish_tip: i64,
    pub kernel_incarnation_id: &'a str,
    pub matcher_digest: &'a str,
    pub created_at_ms: i64,
    pub rows: &'a [CandidateScore<'a>],
}

/// The bounds a descriptor's write enforces; the daemon owns their values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCaps {
    /// More rows than this for one descriptor refuses the write whole.
    pub per_descriptor: usize,
    /// The project's row count after the write; lower-scored rows are evicted past it.
    pub per_project: usize,
}

/// What one descriptor's write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateWrite {
    /// Rows inserted or replaced.
    pub written: usize,
    /// Rows of the descriptor at or below its tip that the new set no longer names.
    pub replaced_away: usize,
    /// Rows evicted to hold the project at its cap.
    pub dropped_at_cap: usize,
}

/// A stored row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCandidate {
    pub project_digest: String,
    pub evidence_id: String,
    pub decision_object_id: String,
    pub decision_id: String,
    pub matched_terms: u32,
    pub decision_terms: u32,
    pub score_bp: u32,
    pub publish_tip: i64,
    pub kernel_incarnation_id: String,
    pub matcher_digest: String,
    pub created_at_ms: i64,
}

/// One page of a decision's rows, in reader order, with the cursor that
/// continues it; `next` is `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePage {
    pub items: Vec<StoredCandidate>,
    pub next: Option<String>,
}

/// A row's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateKey<'a> {
    pub project_digest: &'a str,
    pub evidence_id: &'a str,
    pub decision_object_id: &'a str,
}

fn invalid(message: &str) -> MemoryStoreError {
    MemoryStoreError::Serde(format!("contradiction candidate: {message}"))
}

/// A Kernel-issued id the row stores verbatim: non-empty, bounded, free of
/// control characters and of secret-scanner detections. The detection check
/// runs before the write, so a refused id leaves no scan record.
fn check_id(field: &str, value: &str) -> Result<(), MemoryStoreError> {
    if value.is_empty()
        || value.len() > MAX_CANDIDATE_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(&format!("{field} is not a storable id")));
    }
    if !redact_durable_text(value).detections.is_empty() {
        return Err(MemoryStoreError::Redaction(
            RedactionErrorKind::SecretDetected,
        ));
    }
    Ok(())
}

fn check_hex(field: &str, value: &str, len: usize) -> Result<(), MemoryStoreError> {
    if is_lower_hex(value, len) {
        Ok(())
    } else {
        Err(invalid(&format!("{field} is not {len} lowercase hex")))
    }
}

fn score_bp(matched: u32, decision: u32) -> i64 {
    i64::from(matched) * SCORE_SCALE / i64::from(decision)
}

fn candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCandidate> {
    Ok(StoredCandidate {
        project_digest: row.get(0)?,
        evidence_id: row.get(1)?,
        decision_object_id: row.get(2)?,
        decision_id: row.get(3)?,
        matched_terms: row.get(4)?,
        decision_terms: row.get(5)?,
        score_bp: row.get(6)?,
        publish_tip: row.get(7)?,
        kernel_incarnation_id: row.get(8)?,
        matcher_digest: row.get(9)?,
        created_at_ms: row.get(10)?,
    })
}

/// The page cursor: `{score_bp}.{created_at_ms}.{evidence id as hex}`, the
/// position of the last row a page returned in reader order.
fn format_cursor(score_bp: u32, created_at_ms: i64, evidence_id: &str) -> String {
    let evidence: String = evidence_id
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{score_bp}.{created_at_ms}.{evidence}")
}

/// The position `cursor` names; only a cursor [`format_cursor`] produces is accepted.
fn decode_cursor(cursor: &str) -> Result<(u32, i64, String), MemoryStoreError> {
    let refused = || invalid("the page cursor is not one this store issued");
    let mut parts = cursor.splitn(3, '.');
    let (Some(score), Some(created), Some(evidence)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(refused());
    };
    let score: u32 = score.parse().map_err(|_| refused())?;
    let created: i64 = created.parse().map_err(|_| refused())?;
    if evidence.is_empty() || evidence.len() % 2 != 0 || !is_lower_hex(evidence, evidence.len()) {
        return Err(refused());
    }
    let bytes = (0..evidence.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&evidence[at..at + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| refused())?;
    let evidence = String::from_utf8(bytes).map_err(|_| refused())?;
    if format_cursor(score, created, &evidence) != cursor {
        return Err(refused());
    }
    Ok((score, created, evidence))
}

fn check_write(
    write: &DescriptorCandidates<'_>,
    caps: CandidateCaps,
) -> Result<(), MemoryStoreError> {
    check_hex("project_digest", write.project_digest, 64)?;
    check_id("evidence_id", write.evidence_id)?;
    check_hex("kernel_incarnation_id", write.kernel_incarnation_id, 32)?;
    check_hex("matcher_digest", write.matcher_digest, 64)?;
    if write.publish_tip < 0 || write.created_at_ms < 0 {
        return Err(invalid("publish_tip and created_at_ms are non-negative"));
    }
    if write.rows.len() > caps.per_descriptor {
        return Err(invalid("a descriptor carries more rows than its cap"));
    }
    for (index, row) in write.rows.iter().enumerate() {
        check_id("decision_object_id", row.decision_object_id)?;
        check_id("decision_id", row.decision_id)?;
        if row.matched_terms == 0 || row.matched_terms > row.decision_terms {
            return Err(invalid("matched_terms is between 1 and decision_terms"));
        }
        if write.rows[..index]
            .iter()
            .any(|earlier| earlier.decision_object_id == row.decision_object_id)
        {
            return Err(invalid("a descriptor names each decision once"));
        }
    }
    Ok(())
}

impl MemoryStore {
    /// Writes one descriptor's rows in one transaction. When the write's tip
    /// is at or above every tip the descriptor holds under the same Kernel
    /// incarnation, each key is inserted or its values replaced and the
    /// descriptor's other rows are deleted. A write below that tip is a late
    /// write: it replaces the values of keys the descriptor already holds,
    /// keeps their higher tip, and inserts and deletes nothing, so the
    /// descriptor never holds more than its cap. While the project then holds
    /// more than `caps.per_project` rows, the lowest `score_bp`, then oldest,
    /// then smallest key is evicted.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Redaction`] for an id a secret detector matches;
    /// [`MemoryStoreError::Serde`] for a malformed id, digest, or count, a
    /// repeated decision, or more rows than `caps.per_descriptor`. Either
    /// refusal writes nothing. Storage errors otherwise.
    pub fn replace_candidates_for_descriptor(
        &self,
        write: &DescriptorCandidates<'_>,
        caps: CandidateCaps,
    ) -> Result<CandidateWrite, MemoryStoreError> {
        check_write(write, caps)?;
        let per_project = i64::try_from(caps.per_project).map_err(|_| invalid("cap too large"))?;
        let kept = serde_json::to_string(
            &write
                .rows
                .iter()
                .map(|row| row.decision_object_id)
                .collect::<Vec<_>>(),
        )
        .map_err(|error| invalid(&error.to_string()))?;
        PreparedWrite::new(DurableWriteFamily::ContradictionCandidates).execute(&self.inner, |tx| {
            let tx = tx.tx();
            let held_tip: Option<i64> = tx.query_row(
                "SELECT MAX(publish_tip) FROM contradiction_candidates
                  WHERE project_digest=?1 AND evidence_id=?2 AND kernel_incarnation_id=?3",
                params![write.project_digest, write.evidence_id, write.kernel_incarnation_id],
                |row| row.get(0),
            )?;
            let late = held_tip.is_some_and(|held| held > write.publish_tip);
            let mut written = 0;
            for row in write.rows {
                let values = params![
                    write.project_digest,
                    write.evidence_id,
                    row.decision_object_id,
                    row.decision_id,
                    row.matched_terms,
                    row.decision_terms,
                    score_bp(row.matched_terms, row.decision_terms),
                    write.publish_tip,
                    write.kernel_incarnation_id,
                    write.matcher_digest,
                    write.created_at_ms,
                ];
                written += if late {
                    tx.execute(
                        "UPDATE contradiction_candidates SET decision_id=?4, matched_terms=?5,
                                decision_terms=?6, score_bp=?7, matcher_digest=?10,
                                created_at_ms=?11
                          WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3
                            AND kernel_incarnation_id=?9 AND ?8 IS NOT NULL",
                        values,
                    )?
                } else {
                    tx.execute(
                        "INSERT INTO contradiction_candidates (
                             project_digest,evidence_id,decision_object_id,decision_id,
                             matched_terms,decision_terms,score_bp,publish_tip,
                             kernel_incarnation_id,matcher_digest,created_at_ms
                         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                         ON CONFLICT (project_digest,evidence_id,decision_object_id) DO UPDATE SET
                             decision_id=excluded.decision_id,
                             matched_terms=excluded.matched_terms,
                             decision_terms=excluded.decision_terms,
                             score_bp=excluded.score_bp,
                             publish_tip=CASE WHEN kernel_incarnation_id=excluded.kernel_incarnation_id
                                 THEN max(publish_tip,excluded.publish_tip)
                                 ELSE excluded.publish_tip END,
                             kernel_incarnation_id=excluded.kernel_incarnation_id,
                             matcher_digest=excluded.matcher_digest,
                             created_at_ms=excluded.created_at_ms",
                        values,
                    )?
                };
            }
            let replaced_away = if late {
                0
            } else {
                tx.execute(
                    "DELETE FROM contradiction_candidates
                      WHERE project_digest=?1 AND evidence_id=?2
                        AND decision_object_id NOT IN (SELECT value FROM json_each(?3))",
                    params![write.project_digest, write.evidence_id, kept],
                )?
            };
            let rows: i64 = tx.query_row(
                "SELECT COUNT(*) FROM contradiction_candidates WHERE project_digest=?1",
                [write.project_digest],
                |row| row.get(0),
            )?;
            let dropped_at_cap = if rows > per_project {
                tx.execute(
                    "DELETE FROM contradiction_candidates WHERE rowid IN (
                         SELECT rowid FROM contradiction_candidates WHERE project_digest=?1
                          ORDER BY score_bp ASC, created_at_ms ASC, evidence_id ASC,
                                   decision_object_id ASC
                          LIMIT ?2)",
                    params![write.project_digest, rows - per_project],
                )?
            } else {
                0
            };
            Ok(WriteDisposition::Applied(CandidateWrite {
                written,
                replaced_away,
                dropped_at_cap,
            }))
        })
    }

    /// One page of the rows naming `decision_object_id`, ordered `score_bp`
    /// descending, `created_at_ms` descending, `evidence_id` ascending, after the
    /// cursor `after`; `limit` is clamped to `1..=`[`MAX_CANDIDATE_PAGE`].
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Serde`] for a malformed digest or id, or a cursor
    /// this store did not issue. Storage errors otherwise.
    pub fn candidates_for_decision(
        &self,
        project_digest: &str,
        decision_object_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<CandidatePage, MemoryStoreError> {
        check_hex("project_digest", project_digest, 64)?;
        check_id("decision_object_id", decision_object_id)?;
        let after = after.map(decode_cursor).transpose()?;
        let limit = limit.clamp(1, MAX_CANDIDATE_PAGE);
        let fetch = i64::try_from(limit + 1).map_err(|_| invalid("limit"))?;
        let (score, created, evidence) = after.map_or(
            (i64::MAX, i64::MAX, String::new()),
            |(score, created, evidence)| (i64::from(score), created, evidence),
        );
        let mut items = self
            .inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(&format!(
                    "SELECT {COLUMNS} FROM contradiction_candidates
                      WHERE project_digest=?1 AND decision_object_id=?2
                        AND score_bp<=?3
                        AND (score_bp<?3 OR (score_bp=?3 AND created_at_ms<?4)
                             OR (score_bp=?3 AND created_at_ms=?4 AND evidence_id>?5))
                      ORDER BY score_bp DESC, created_at_ms DESC, evidence_id ASC
                      LIMIT ?6"
                ))?;
                statement
                    .query_map(
                        params![
                            project_digest,
                            decision_object_id,
                            score,
                            created,
                            evidence,
                            fetch
                        ],
                        candidate_from_row,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(MemoryStoreError::from)?;
        let next = if items.len() > limit {
            items.truncate(limit);
            items
                .last()
                .map(|row| format_cursor(row.score_bp, row.created_at_ms, &row.evidence_id))
        } else {
            None
        };
        Ok(CandidatePage { items, next })
    }

    /// At most `page` rows of every project with a rowid above `after_rowid`,
    /// in rowid order, each with its rowid, for a sweep's watermark. A new row
    /// can take a rowid at or below a watermark after the highest row is
    /// deleted, so a sweep wraps to 0 after a short page.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn candidate_sweep_page(
        &self,
        after_rowid: i64,
        page: usize,
    ) -> Result<Vec<(i64, StoredCandidate)>, MemoryStoreError> {
        let page = i64::try_from(page).map_err(|_| invalid("page"))?;
        self.inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(&format!(
                    "SELECT {COLUMNS},rowid FROM contradiction_candidates
                      WHERE rowid>?1 ORDER BY rowid LIMIT ?2"
                ))?;
                statement
                    .query_map(params![after_rowid, page], |row| {
                        Ok((row.get(11)?, candidate_from_row(row)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(MemoryStoreError::from)
    }

    /// Deletes the rows `keys` name in one transaction and answers how many existed.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn delete_candidates(&self, keys: &[CandidateKey<'_>]) -> Result<usize, MemoryStoreError> {
        if keys.is_empty() {
            return Ok(0);
        }
        PreparedWrite::new(DurableWriteFamily::ContradictionCandidates).execute(&self.inner, |tx| {
            let tx = tx.tx();
            let mut deleted = 0;
            for key in keys {
                deleted += tx.execute(
                    "DELETE FROM contradiction_candidates
                      WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3",
                    params![key.project_digest, key.evidence_id, key.decision_object_id],
                )?;
            }
            Ok(WriteDisposition::Applied(deleted))
        })
    }

    /// Deletes each of `rows` whose stored row still holds the values the
    /// caller read: a row rewritten since then keeps its newer values and
    /// stays. Answers how many rows were deleted.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn delete_candidates_as_read(
        &self,
        rows: &[&StoredCandidate],
    ) -> Result<usize, MemoryStoreError> {
        if rows.is_empty() {
            return Ok(0);
        }
        PreparedWrite::new(DurableWriteFamily::ContradictionCandidates).execute(&self.inner, |tx| {
            let tx = tx.tx();
            let mut deleted = 0;
            for row in rows {
                deleted += tx.execute(
                    "DELETE FROM contradiction_candidates
                      WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3
                        AND created_at_ms=?4 AND publish_tip=?5 AND kernel_incarnation_id=?6
                        AND matcher_digest=?7",
                    params![
                        row.project_digest,
                        row.evidence_id,
                        row.decision_object_id,
                        row.created_at_ms,
                        row.publish_tip,
                        row.kernel_incarnation_id,
                        row.matcher_digest,
                    ],
                )?;
            }
            Ok(WriteDisposition::Applied(deleted))
        })
    }

    /// The row count and retained bytes of one project, or of every project
    /// when `project_digest` is `None`. A row's bytes are the lengths of its six
    /// text columns plus eight per integer column.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Serde`] for a malformed digest; storage errors otherwise.
    pub fn candidate_rows_and_bytes(
        &self,
        project_digest: Option<&str>,
    ) -> Result<(u64, u64), MemoryStoreError> {
        if let Some(project_digest) = project_digest {
            check_hex("project_digest", project_digest, 64)?;
        }
        let sum = "SELECT COUNT(*), COALESCE(SUM(
                 length(CAST(project_digest AS BLOB)) + length(CAST(evidence_id AS BLOB))
                 + length(CAST(decision_object_id AS BLOB)) + length(CAST(decision_id AS BLOB))
                 + length(CAST(kernel_incarnation_id AS BLOB))
                 + length(CAST(matcher_digest AS BLOB)) + 40), 0)
               FROM contradiction_candidates";
        let (rows, bytes): (i64, i64) = self
            .inner
            .with_conn(|conn| match project_digest {
                Some(project_digest) => conn.query_row(
                    &format!("{sum} WHERE project_digest=?1"),
                    [project_digest],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ),
                None => conn.query_row(sum, [], |row| Ok((row.get(0)?, row.get(1)?))),
            })
            .map_err(MemoryStoreError::from)?;
        Ok((rows.unsigned_abs(), bytes.unsigned_abs()))
    }

    /// Empties the table, the first step of a rebuild a test replays.
    #[cfg(any(test, feature = "test-support"))]
    pub fn truncate_contradiction_candidates_for_test(&self) -> Result<(), MemoryStoreError> {
        PreparedWrite::new(DurableWriteFamily::ContradictionCandidates).execute(&self.inner, |tx| {
            tx.tx()
                .execute("DELETE FROM contradiction_candidates", [])?;
            Ok(WriteDisposition::Applied(()))
        })
    }
}
