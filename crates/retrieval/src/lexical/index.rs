//! `lexical` stores one row per live occurrence.
//! A tombstone deletes its lexical row in the invalidation transaction.

use std::num::NonZeroUsize;

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use super::{LexicalBounds, analyze};
use crate::{ProjectionError, QueryRow};

pub const OCCURRENCE_ID_COLUMN: &str = "occurrence_id";

/// The rowid of the lexical row for a lowercase hex occurrence identifier: its leading sixty-four bits with the sign bit cleared.
/// The value depends on the identifier alone, so a rebuild and incremental application place a row identically.
/// `None` for an identifier that does not start with sixteen hex digits.
#[must_use]
pub fn rowid(occurrence_id: &str) -> Option<i64> {
    let leading = occurrence_id.get(..16)?;
    if !leading.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let bits = u64::from_str_radix(leading, 16).ok()?;
    Some((bits & (i64::MAX as u64)) as i64)
}

fn keyed(occurrence_id: &str) -> Result<i64, ProjectionError> {
    rowid(occurrence_id).ok_or(ProjectionError::CorruptRow)
}

/// Stores the row unless it is already present; returns whether a row was written.
/// Every atom has at least one byte, so `max_payload_bytes` also bounds `max_atoms`.
/// NUL is replaced with a space before analysis because the analyzer refuses it.
///
/// # Errors
///
/// A rowid held by another occurrence is [`ProjectionError::LexicalRowidCollision`]; an analyzer refusal is [`ProjectionError::Lexical`].
pub(crate) fn insert(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    payload: &str,
    max_payload_bytes: NonZeroUsize,
) -> Result<bool, ProjectionError> {
    let rowid = keyed(occurrence_id)?;
    let holder: Option<String> = conn
        .query_row(
            "SELECT occurrence_id FROM lexical WHERE rowid=?1",
            [rowid],
            |row| row.get(0),
        )
        .optional()?;
    match holder {
        Some(holder) if holder == occurrence_id => return Ok(false),
        Some(holder) => {
            return Err(ProjectionError::LexicalRowidCollision {
                occurrence_id: occurrence_id.to_string(),
                holder,
            });
        }
        None => {}
    }
    let text = payload.replace('\0', " ");
    let analysis = analyze(
        &text,
        LexicalBounds {
            max_input_bytes: max_payload_bytes,
            max_atoms: max_payload_bytes,
        },
    )?;
    conn.execute(
        "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1,?2,?3,?4)",
        params![
            rowid,
            analysis.original_text(),
            analysis.parts_text(),
            occurrence_id
        ],
    )?;
    Ok(true)
}

pub(crate) fn delete(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
) -> Result<usize, ProjectionError> {
    Ok(conn.execute(
        "DELETE FROM lexical WHERE rowid=?1 AND occurrence_id=?2",
        params![keyed(occurrence_id)?, occurrence_id],
    )?)
}

pub(crate) fn present(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
) -> Result<bool, ProjectionError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM lexical WHERE rowid=?1 AND occurrence_id=?2)",
        params![keyed(occurrence_id)?, occurrence_id],
        |row| row.get(0),
    )?)
}

/// Every live occurrence has exactly one lexical row, and every lexical row sits at its derived rowid and names a live occurrence.
/// `PRAGMA integrity_check` verifies the inverted index itself; this check covers the join the engine cannot see.
///
/// # Errors
///
/// A missing, orphaned, or misplaced row is [`ProjectionError::CorruptRow`].
pub fn verify_rows(conn: &GuardedConn<'_>) -> Result<(), ProjectionError> {
    let (live, rows): (i64, i64) = conn.query_row(
        "SELECT (SELECT count(*) FROM occurrences o
                 WHERE NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)),
                (SELECT count(*) FROM lexical)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if live != rows {
        return Err(ProjectionError::CorruptRow);
    }
    let mut rows = conn.prepare("SELECT rowid, occurrence_id FROM lexical")?;
    let mut live = conn.prepare(
        "SELECT EXISTS(SELECT 1 FROM occurrences o WHERE o.occurrence_id=?1
                AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id))",
    )?;
    for row in rows.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (stored, occurrence_id) = row?;
        if rowid(&occurrence_id) != Some(stored)
            || !live.query_row([&occurrence_id], |row| row.get::<_, bool>(0))?
        {
            return Err(ProjectionError::CorruptRow);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineIdentity {
    pub sqlite_version: String,
    pub sqlite_source_id: String,
}

/// A literal query against `lexical` confirms that FTS5 is linked and that the table's tokenizer loads; a build without either cannot prepare it.
///
/// # Errors
///
/// A missing FTS5 module or tokenizer is [`ProjectionError::Unsupported`]; any other engine error is [`ProjectionError::Sqlite`].
pub fn probe_engine(conn: &impl QueryRow) -> Result<EngineIdentity, ProjectionError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM lexical WHERE lexical MATCH '\"probe\"')",
        [],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("no such module") || message.contains("tokenizer") {
            ProjectionError::Unsupported {
                reason: "the linked SQLite has no FTS5 or cannot load the lexical tokenizer",
            }
        } else {
            ProjectionError::from(error)
        }
    })?;
    let (sqlite_version, sqlite_source_id) =
        conn.query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    Ok(EngineIdentity {
        sqlite_version,
        sqlite_source_id,
    })
}
