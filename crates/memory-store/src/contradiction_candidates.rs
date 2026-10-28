//! Contradiction candidates: derived rows that name a decision a published
//! descriptor overlaps lexically. Each row is keyed by (project digest,
//! descriptor evidence id, decision object id) and holds ids, integers, and
//! times only. The store keeps the rows, bounds them per descriptor and per
//! project, keeps each project's row count and bytes in
//! `contradiction_candidate_totals`, and pages the rows; the daemon judges
//! their liveness. This module is the only writer of both tables: every write
//! here updates the totals in the transaction that changes the rows, which is
//! what keeps the cap check and the gauge exact.

use std::collections::BTreeMap;
use std::time::Instant;

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use context_core::canonical_json::is_lower_hex;
use context_core::redaction::{RedactionErrorKind, redact_durable_text};

use crate::{DurableWriteFamily, MemoryStore, MemoryStoreError, PreparedWrite, WriteDisposition};

/// The most rows one [`MemoryStore::candidates_for_decision`] page returns.
pub const MAX_CANDIDATE_PAGE: usize = 64;
/// The longest id a row stores.
const MAX_CANDIDATE_ID_BYTES: usize = 512;
/// Basis points: `score_bp = matched_terms * 10000 / decision_terms`.
const SCORE_SCALE: i64 = 10_000;

/// The table's columns in [`StoredCandidate`] field order.
macro_rules! columns {
    () => {
        "project_digest,evidence_id,decision_object_id,decision_id,matched_terms,\
     decision_terms,score_bp,publish_tip,kernel_incarnation_id,matcher_digest,created_at_ms"
    };
}

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
    /// Rows inserted or replaced, counting a new row the cap evicts in the same
    /// write, which is never inserted.
    pub written: usize,
    /// Rows of the descriptor at or below its tip that the new set no longer names.
    pub replaced_away: usize,
    /// Rows evicted to hold the project at its cap, held rows and new rows alike.
    pub dropped_at_cap: usize,
    /// The write was below the descriptor's held tip, so it updated held keys
    /// and discarded its other rows.
    pub late: bool,
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

/// The longest cursor [`format_cursor`] produces: a ten-digit score, a signed
/// twenty-character time, two dots, and the hex of a longest id.
const MAX_CURSOR_BYTES: usize = 10 + 1 + 20 + 1 + 2 * MAX_CANDIDATE_ID_BYTES;

/// The position `cursor` names; only a cursor [`format_cursor`] produces is accepted.
/// The length check runs first, so the split and the parses examine at most
/// `MAX_CURSOR_BYTES` bytes.
fn decode_cursor(cursor: &str) -> Result<(u32, i64, String), MemoryStoreError> {
    let refused = || invalid("the page cursor is not one this store issued");
    if cursor.len() > MAX_CURSOR_BYTES {
        return Err(refused());
    }
    let mut parts = cursor.splitn(3, '.');
    let (Some(score), Some(created), Some(evidence)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(refused());
    };
    let score: u32 = score.parse().map_err(|_| refused())?;
    let created: i64 = created.parse().map_err(|_| refused())?;
    // The table's rows hold `score_bp <= 10000` and `created_at_ms >= 0`.
    if i64::from(score) > SCORE_SCALE || created < 0 {
        return Err(refused());
    }
    if evidence.is_empty()
        || evidence.len() % 2 != 0
        || evidence.len() > 2 * MAX_CANDIDATE_ID_BYTES
        || !is_lower_hex(evidence, evidence.len())
    {
        return Err(refused());
    }
    let bytes = (0..evidence.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&evidence[at..at + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| refused())?;
    let evidence = String::from_utf8(bytes).map_err(|_| refused())?;
    // A stored id carries no control character, so a cursor naming one was never issued.
    if evidence.chars().any(char::is_control) || format_cursor(score, created, &evidence) != cursor
    {
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

/// A row the descriptor holds when its write begins.
struct HeldRow {
    decision_object_id: String,
    bytes: i64,
    publish_tip: i64,
    same_incarnation: bool,
}

/// The bytes the gauge counts for a stored row: its six text columns plus eight
/// per integer column. [`row_bytes`] computes the same sum for a row not yet
/// stored; `the_totals_follow_replaced_ids_late_writes_deletes_and_truncation`
/// keeps the two equal.
macro_rules! row_bytes {
    () => {
        "octet_length(project_digest) + octet_length(evidence_id)
     + octet_length(decision_object_id) + octet_length(decision_id)
     + octet_length(kernel_incarnation_id) + octet_length(matcher_digest) + 40"
    };
}

/// Replaces a held key's values at the write's tip and incarnation.
const UPDATE_HELD: &str = "UPDATE contradiction_candidates SET decision_id=?4, matched_terms=?5,
        decision_terms=?6, score_bp=?7, publish_tip=?8, kernel_incarnation_id=?9,
        matcher_digest=?10, created_at_ms=?11
  WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3";

/// Replaces a held key's values and keeps its higher tip. It binds the same
/// eleven values as [`UPDATE_HELD`]; `?8` and `?9` go unused.
const UPDATE_LATE: &str = "UPDATE contradiction_candidates SET decision_id=?4, matched_terms=?5,
        decision_terms=?6, score_bp=?7, matcher_digest=?10, created_at_ms=?11
  WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3";

const INSERT_ROW: &str = concat!(
    "INSERT INTO contradiction_candidates (",
    columns!(),
    ") VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"
);

const DELETE_KEY: &str = "DELETE FROM contradiction_candidates
      WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3";

const DELETE_ROWID: &str = "DELETE FROM contradiction_candidates WHERE rowid=?1";

/// A held row the cap may evict, in the eviction order's terms.
struct Lowest {
    rowid: i64,
    bytes: i64,
    score_bp: i64,
    created_at_ms: i64,
    evidence_id: String,
    decision_object_id: String,
}

/// A row's place in the eviction order: score, then creation time, then key.
type EvictionKey<'a> = (i64, i64, &'a str, &'a str);

/// Where an eviction candidate comes from, by index: a held row in the lowest
/// rows, or a new row of the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evictee {
    Held(usize),
    Fresh(usize),
}

/// The `excess` candidates the cap evicts from the project's lowest held rows
/// and the write's new rows together, in eviction order: lowest score, then
/// oldest, then smallest key. Keys are unique within a project, so the order
/// is total.
fn choose_evictions(
    lowest: &[Lowest],
    fresh: &[&CandidateScore<'_>],
    write: &DescriptorCandidates<'_>,
    excess: usize,
) -> Vec<Evictee> {
    let mut order: Vec<(EvictionKey<'_>, Evictee)> = lowest
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let key = (
                row.score_bp,
                row.created_at_ms,
                row.evidence_id.as_str(),
                row.decision_object_id.as_str(),
            );
            (key, Evictee::Held(index))
        })
        .chain(fresh.iter().enumerate().map(|(index, row)| {
            let key = (
                score_bp(row.matched_terms, row.decision_terms),
                write.created_at_ms,
                write.evidence_id,
                row.decision_object_id,
            );
            (key, Evictee::Fresh(index))
        }))
        .collect();
    order.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    order
        .into_iter()
        .take(excess)
        .map(|(_, evictee)| evictee)
        .collect()
}

/// The eleven columns of `row` in table order, bound as `?1` through `?11`.
fn row_values<'a>(
    write: &DescriptorCandidates<'a>,
    row: &CandidateScore<'a>,
) -> impl rusqlite::Params + 'a {
    (
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
    )
}

/// The bytes the gauge counts for `row` once stored, as `row_bytes!` sums them.
fn row_bytes(write: &DescriptorCandidates<'_>, row: &CandidateScore<'_>) -> i64 {
    let text = write.project_digest.len()
        + write.evidence_id.len()
        + row.decision_object_id.len()
        + row.decision_id.len()
        + write.kernel_incarnation_id.len()
        + write.matcher_digest.len();
    i64::try_from(text).unwrap_or(i64::MAX) + 40
}

/// The rows the descriptor holds, whatever their incarnation.
fn held_rows(
    tx: &GuardedConn<'_>,
    write: &DescriptorCandidates<'_>,
) -> rusqlite::Result<Vec<HeldRow>> {
    tx.prepare_cached(concat!(
        "SELECT decision_object_id, ",
        row_bytes!(),
        ", publish_tip, kernel_incarnation_id=?3
           FROM contradiction_candidates WHERE project_digest=?1 AND evidence_id=?2"
    ))?
    .query_map(
        params![
            write.project_digest,
            write.evidence_id,
            write.kernel_incarnation_id
        ],
        |row| {
            Ok(HeldRow {
                decision_object_id: row.get(0)?,
                bytes: row.get(1)?,
                publish_tip: row.get(2)?,
                same_incarnation: row.get(3)?,
            })
        },
    )?
    .collect()
}

/// The project's `count` lowest rows in eviction order. The scan walks
/// `idx_contradiction_candidates_eviction` and stops after `count` rows; the
/// count stays out of the SQL because SQLite re-plans a cached statement each
/// time a bound LIMIT changes.
fn lowest_rows(
    tx: &GuardedConn<'_>,
    project_digest: &str,
    count: usize,
) -> rusqlite::Result<Vec<Lowest>> {
    tx.prepare_cached(concat!(
        "SELECT rowid, ",
        row_bytes!(),
        ", score_bp, created_at_ms, evidence_id, decision_object_id
           FROM contradiction_candidates WHERE project_digest=?1
          ORDER BY score_bp ASC, created_at_ms ASC, evidence_id ASC, decision_object_id ASC"
    ))?
    .query_map([project_digest], |row| {
        Ok(Lowest {
            rowid: row.get(0)?,
            bytes: row.get(1)?,
            score_bp: row.get(2)?,
            created_at_ms: row.get(3)?,
            evidence_id: row.get(4)?,
            decision_object_id: row.get(5)?,
        })
    })?
    .take(count)
    .collect()
}

/// The project's row count and bytes; a project without rows has no totals row.
fn project_totals(conn: &GuardedConn<'_>, project_digest: &str) -> rusqlite::Result<(i64, i64)> {
    conn.prepare_cached(
        "SELECT row_count, row_bytes FROM contradiction_candidate_totals WHERE project_digest=?1",
    )?
    .query_row([project_digest], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
    .map(Option::unwrap_or_default)
}

/// Stores the project's row count and bytes, deleting its totals row at zero
/// rows. A count or byte sum that cannot describe a set of rows means the totals
/// drifted from the rows, and the write is refused rather than stored.
fn set_project_totals(
    conn: &GuardedConn<'_>,
    project_digest: &str,
    rows: i64,
    bytes: i64,
) -> rusqlite::Result<()> {
    if rows < 0 || bytes < 0 || (rows == 0) != (bytes == 0) {
        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(invalid(
            "the project's totals disagree with its rows",
        ))));
    }
    if rows == 0 {
        conn.prepare_cached("DELETE FROM contradiction_candidate_totals WHERE project_digest=?1")?
            .execute([project_digest])?;
    } else {
        conn.prepare_cached(
            "INSERT INTO contradiction_candidate_totals (project_digest, row_count, row_bytes)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (project_digest) DO UPDATE SET
                 row_count=excluded.row_count, row_bytes=excluded.row_bytes",
        )?
        .execute(params![project_digest, rows, bytes])?;
    }
    Ok(())
}

impl MemoryStore {
    /// Writes one descriptor's rows in one transaction. When the write's tip
    /// is at or above every tip the descriptor holds under the same Kernel
    /// incarnation, each named key ends holding the write's values and the
    /// descriptor's other rows are deleted. A write below that tip is a late
    /// write: it replaces the values of keys the descriptor already holds,
    /// keeps their higher tip, and inserts and deletes nothing, so the
    /// descriptor keeps the row count its held write established, within the
    /// `caps.per_descriptor` that write enforced. While the project then holds
    /// more than `caps.per_project` rows, the lowest `score_bp`, then oldest,
    /// then smallest key is evicted. A new row the cap would evict at once is
    /// never inserted and is counted in both `written` and `dropped_at_cap`, so
    /// the rows that remain and the counts equal inserting it and evicting it.
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
        self.replace_candidates_for_descriptor_bounded(write, caps, None)
    }

    /// [`Self::replace_candidates_for_descriptor`] whose wait for the store's connection and
    /// write lock ends at `deadline` when one is given.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Store`] carrying [`storage::StoreError::Deadline`] when the connection
    /// or the write lock is still held at `deadline`; nothing was written. Otherwise as
    /// [`Self::replace_candidates_for_descriptor`].
    pub fn replace_candidates_for_descriptor_bounded(
        &self,
        write: &DescriptorCandidates<'_>,
        caps: CandidateCaps,
        deadline: Option<Instant>,
    ) -> Result<CandidateWrite, MemoryStoreError> {
        check_write(write, caps)?;
        let per_project = i64::try_from(caps.per_project).map_err(|_| invalid("cap too large"))?;
        let family = PreparedWrite::new(DurableWriteFamily::ContradictionCandidates);
        family.execute_bounded(&self.inner, deadline, |tx| {
            let tx = tx.tx();
            let held = held_rows(tx, write)?;
            let late = held
                .iter()
                .filter(|row| row.same_incarnation)
                .map(|row| row.publish_tip)
                .max()
                .is_some_and(|held| held > write.publish_tip);
            let totals = project_totals(tx, write.project_digest)?;
            let (mut rows, mut bytes) = totals;
            let mut written = 0;
            let mut fresh = Vec::new();
            let mut update = tx.prepare_cached(if late { UPDATE_LATE } else { UPDATE_HELD })?;
            for row in write.rows {
                let Some(prior) = held
                    .iter()
                    .find(|held| held.decision_object_id == row.decision_object_id)
                else {
                    if !late {
                        fresh.push(row);
                    }
                    continue;
                };
                written += update.execute(row_values(write, row))?;
                bytes += row_bytes(write, row) - prior.bytes;
            }
            drop(update);
            let mut replaced_away = 0;
            if !late {
                let mut delete = tx.prepare_cached(DELETE_KEY)?;
                for prior in &held {
                    if write
                        .rows
                        .iter()
                        .all(|row| row.decision_object_id != prior.decision_object_id)
                        && delete.execute(params![
                            write.project_digest,
                            write.evidence_id,
                            prior.decision_object_id
                        ])? == 1
                    {
                        replaced_away += 1;
                        rows -= 1;
                        bytes -= prior.bytes;
                    }
                }
            }
            let fresh_count = i64::try_from(fresh.len()).unwrap_or(i64::MAX);
            let excess = usize::try_from(rows + fresh_count - per_project).unwrap_or(0);
            let mut doomed = vec![false; fresh.len()];
            let mut evicted = Vec::new();
            let mut dropped_at_cap = 0;
            if excess > 0 {
                let lowest = lowest_rows(tx, write.project_digest, excess)?;
                for evictee in choose_evictions(&lowest, &fresh, write, excess) {
                    match evictee {
                        Evictee::Fresh(index) => {
                            doomed[index] = true;
                            dropped_at_cap += 1;
                        }
                        Evictee::Held(index) => {
                            evicted.push((lowest[index].rowid, lowest[index].bytes))
                        }
                    }
                }
            }
            // New rows go in before the evicted rows go out, so a new row never
            // takes the rowid of a row this write evicts.
            let mut insert = tx.prepare_cached(INSERT_ROW)?;
            for (row, doomed) in fresh.iter().zip(doomed) {
                written += 1;
                if !doomed {
                    insert.execute(row_values(write, row))?;
                    rows += 1;
                    bytes += row_bytes(write, row);
                }
            }
            let mut delete = tx.prepare_cached(DELETE_ROWID)?;
            for (rowid, freed) in evicted {
                if delete.execute([rowid])? == 1 {
                    dropped_at_cap += 1;
                    rows -= 1;
                    bytes -= freed;
                }
            }
            if (rows, bytes) != totals {
                set_project_totals(tx, write.project_digest, rows, bytes)?;
            }
            Ok(WriteDisposition::Applied(CandidateWrite {
                written,
                replaced_away,
                dropped_at_cap,
                late,
            }))
        })
    }

    /// One page of the rows naming `decision_object_id`, ordered `score_bp`
    /// descending, `created_at_ms` descending, `evidence_id` ascending, after the
    /// cursor `after`; `limit` is clamped to `1..=`[`MAX_CANDIDATE_PAGE`].
    /// `decision_object_id` is a Kernel object id of 1 to `MAX_CANDIDATE_ID_BYTES` bytes.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Serde`] for a malformed digest or id, or for a cursor
    /// outside the canonical form a page's `next` carries. Storage errors
    /// otherwise; the page query runs before the cursor is judged, so a storage
    /// error takes precedence over a malformed cursor.
    pub fn candidates_for_decision(
        &self,
        project_digest: &str,
        decision_object_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<CandidatePage, MemoryStoreError> {
        self.candidates_for_decision_inner(project_digest, decision_object_id, after, limit, None)
    }

    /// [`Self::module_authority_for_route`] whose connection wait ends at `deadline`.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Store`] with [`storage::StoreError::Deadline`] when the
    /// connection is still held at `deadline`; otherwise as
    /// [`Self::module_authority_for_route`].
    pub fn module_authority_for_route_within(
        &self,
        route_project_root: &str,
        domain: &str,
        deadline: Instant,
    ) -> Result<Option<crate::AuthorityRow>, MemoryStoreError> {
        self.module_authority_for_route_then(route_project_root, domain, Some(deadline), |_, _| {
            Ok(())
        })
        .map(|found| found.map(|(row, ())| row))
    }

    /// [`Self::candidates_for_decision`] whose connection wait ends at `deadline`.
    ///
    /// # Errors
    ///
    /// [`MemoryStoreError::Store`] with [`storage::StoreError::Deadline`] when the
    /// connection is still held at `deadline`; otherwise as
    /// [`Self::candidates_for_decision`].
    pub fn candidates_for_decision_within(
        &self,
        project_digest: &str,
        decision_object_id: &str,
        after: Option<&str>,
        limit: usize,
        deadline: Instant,
    ) -> Result<CandidatePage, MemoryStoreError> {
        self.candidates_for_decision_inner(
            project_digest,
            decision_object_id,
            after,
            limit,
            Some(deadline),
        )
    }

    /// [`Self::candidates_for_decision_within`] read in the connection that
    /// reads the MODULE authority row serving `route_project_root` for `domain`
    /// ([`Self::module_authority_for_route`]). `None` when the route has no
    /// MODULE or DRAINING binding or its project has no MODULE row; the page is
    /// read only under a found row.
    ///
    /// # Errors
    ///
    /// As [`Self::candidates_for_decision_within`], and the authority domain
    /// errors of [`Self::module_authority_for_route`].
    #[allow(clippy::too_many_arguments)]
    pub fn candidates_for_decision_under_route_within(
        &self,
        route_project_root: &str,
        domain: &str,
        project_digest: &str,
        decision_object_id: &str,
        after: Option<&str>,
        limit: usize,
        deadline: Instant,
    ) -> Result<Option<(crate::AuthorityRow, CandidatePage)>, MemoryStoreError> {
        let query = PageQuery::new(project_digest, decision_object_id, after, limit)?;
        let found = self.module_authority_for_route_then(
            route_project_root,
            domain,
            Some(deadline),
            |conn, _| query.read(conn),
        )?;
        let Some((row, items)) = found else {
            query.cursor()?;
            return Ok(None);
        };
        Ok(Some((row, query.page(items)?)))
    }

    fn candidates_for_decision_inner(
        &self,
        project_digest: &str,
        decision_object_id: &str,
        after: Option<&str>,
        limit: usize,
        deadline: Option<Instant>,
    ) -> Result<CandidatePage, MemoryStoreError> {
        let query = PageQuery::new(project_digest, decision_object_id, after, limit)?;
        let read = |conn: &GuardedConn<'_>| query.read(conn);
        let items = match deadline {
            Some(deadline) => self.inner.with_conn_within(deadline, read),
            None => self.inner.with_conn(read),
        }
        .map_err(MemoryStoreError::from)?;
        query.page(items)
    }

    /// At most `page` rows of every project with a rowid above `after_rowid`,
    /// in rowid order, each with its rowid, for a sweep's watermark. A rewrite
    /// of a held key keeps its rowid. A new row can take a rowid at or below a
    /// watermark after the highest row is deleted, so a sweep wraps to 0 after a
    /// short page.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn candidate_sweep_page(
        &self,
        after_rowid: i64,
        page: usize,
    ) -> Result<Vec<(i64, StoredCandidate)>, MemoryStoreError> {
        self.inner
            .with_conn(|conn| {
                let mut statement = conn.prepare_cached(concat!(
                    "SELECT ",
                    columns!(),
                    ",rowid FROM contradiction_candidates
                      WHERE rowid>?1 ORDER BY rowid"
                ))?;
                statement
                    .query_map([after_rowid], |row| {
                        Ok((row.get(11)?, candidate_from_row(row)?))
                    })?
                    .take(page)
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(MemoryStoreError::from)
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
            let mut freed: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
            let mut find = tx.prepare_cached(concat!(
                "SELECT rowid, ",
                row_bytes!(),
                " FROM contradiction_candidates
                  WHERE project_digest=?1 AND evidence_id=?2 AND decision_object_id=?3
                    AND created_at_ms=?4 AND publish_tip=?5 AND kernel_incarnation_id=?6
                    AND matcher_digest=?7 AND decision_id=?8 AND matched_terms=?9
                    AND decision_terms=?10 AND score_bp=?11"
            ))?;
            let mut delete = tx.prepare_cached(DELETE_ROWID)?;
            for row in rows {
                let found = find
                    .query_row(
                        params![
                            row.project_digest,
                            row.evidence_id,
                            row.decision_object_id,
                            row.created_at_ms,
                            row.publish_tip,
                            row.kernel_incarnation_id,
                            row.matcher_digest,
                            row.decision_id,
                            row.matched_terms,
                            row.decision_terms,
                            row.score_bp,
                        ],
                        |found| Ok((found.get::<_, i64>(0)?, found.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                if let Some((rowid, bytes)) = found
                    && delete.execute([rowid])? == 1
                {
                    let (count, total) = freed.entry(row.project_digest.as_str()).or_default();
                    *count += 1;
                    *total += bytes;
                }
            }
            let mut deleted = 0;
            for (project, (count, bytes)) in freed {
                let (held_rows, held_bytes) = project_totals(tx, project)?;
                set_project_totals(tx, project, held_rows - count, held_bytes - bytes)?;
                deleted += usize::try_from(count).unwrap_or(usize::MAX);
            }
            Ok(WriteDisposition::Applied(deleted))
        })
    }

    /// The row count and retained bytes of one project, or of every project
    /// when `project_digest` is `None`. A row's bytes are the lengths of its six
    /// text columns plus eight per integer column. Every write keeps the
    /// project's sums in `contradiction_candidate_totals`, so this reads one row
    /// per project.
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
        let (rows, bytes) = self
            .inner
            .with_conn(|conn| match project_digest {
                Some(project_digest) => project_totals(conn, project_digest),
                None => conn
                    .prepare_cached(
                        "SELECT COALESCE(SUM(row_count), 0), COALESCE(SUM(row_bytes), 0)
                           FROM contradiction_candidate_totals",
                    )?
                    .query_row([], |row| Ok((row.get(0)?, row.get(1)?))),
            })
            .map_err(MemoryStoreError::from)?;
        Ok((rows.unsigned_abs(), bytes.unsigned_abs()))
    }

    /// Empties the table, the first step of a rebuild a test replays.
    #[cfg(any(test, feature = "test-support"))]
    pub fn truncate_contradiction_candidates_for_test(&self) -> Result<(), MemoryStoreError> {
        PreparedWrite::new(DurableWriteFamily::ContradictionCandidates).execute(&self.inner, |tx| {
            let tx = tx.tx();
            tx.execute("DELETE FROM contradiction_candidates", [])?;
            tx.execute("DELETE FROM contradiction_candidate_totals", [])?;
            Ok(WriteDisposition::Applied(()))
        })
    }
}

/// One keyset page query: its checked inputs, its decoded cursor, and the
/// rows it reads from a connection.
struct PageQuery<'a> {
    project_digest: &'a str,
    decision_object_id: &'a str,
    /// The decoded cursor; a malformed spelling is kept so the first page's storage errors take precedence over cursor validation errors.
    after: Result<Option<(u32, i64, String)>, MemoryStoreError>,
    limit: usize,
}

impl<'a> PageQuery<'a> {
    fn new(
        project_digest: &'a str,
        decision_object_id: &'a str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Self, MemoryStoreError> {
        check_hex("project_digest", project_digest, 64)?;
        if decision_object_id.is_empty() || decision_object_id.len() > MAX_CANDIDATE_ID_BYTES {
            return Err(invalid("decision_object_id is not a Kernel object id"));
        }
        Ok(Self {
            project_digest,
            decision_object_id,
            after: after.map(decode_cursor).transpose(),
            limit: limit.clamp(1, MAX_CANDIDATE_PAGE),
        })
    }

    /// The cursor's validation error, after the page ran.
    fn cursor(self) -> Result<(), MemoryStoreError> {
        self.after.map(|_| ())
    }

    /// Up to `limit + 1` rows from the cursor in reader order: score descending, creation time descending, evidence id ascending.
    fn read(&self, conn: &GuardedConn<'_>) -> rusqlite::Result<Vec<StoredCandidate>> {
        macro_rules! page_range {
            ($range:literal) => {
                concat!(
                    "SELECT ",
                    columns!(),
                    " FROM contradiction_candidates
                      WHERE project_digest=?1 AND decision_object_id=?2 AND ",
                    $range
                )
            };
        }
        const SAME_TIME: &str = page_range!(
            "score_bp=?3 AND created_at_ms=?4 AND evidence_id>?5 ORDER BY evidence_id ASC"
        );
        const SAME_SCORE: &str = page_range!(
            "score_bp=?3 AND created_at_ms<?4 ORDER BY created_at_ms DESC, evidence_id ASC"
        );
        const LOWER_SCORE: &str =
            page_range!("score_bp<?3 ORDER BY score_bp DESC, created_at_ms DESC, evidence_id ASC");
        let want = self.limit + 1;
        let mut items = Vec::with_capacity(want);
        let mut extend = |sql: &str, values: &[&dyn rusqlite::ToSql]| {
            let room = want.saturating_sub(items.len());
            if room == 0 {
                return Ok(());
            }
            for row in conn
                .prepare_cached(sql)?
                .query_map(values, candidate_from_row)?
                .take(room)
            {
                items.push(row?);
            }
            Ok::<_, rusqlite::Error>(())
        };
        let (project, decision) = (&self.project_digest, &self.decision_object_id);
        let score = match &self.after {
            Ok(Some((score, created, evidence))) => {
                let score = i64::from(*score);
                extend(
                    SAME_TIME,
                    params![project, decision, score, created, evidence],
                )?;
                extend(SAME_SCORE, params![project, decision, score, created])?;
                score
            }
            _ => i64::MAX,
        };
        extend(LOWER_SCORE, params![project, decision, score])?;
        Ok(items)
    }

    /// The page the read rows make: at most `limit` rows and the cursor after the last one when a further row was read.
    fn page(self, mut items: Vec<StoredCandidate>) -> Result<CandidatePage, MemoryStoreError> {
        let limit = self.limit;
        self.cursor()?;
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
}
