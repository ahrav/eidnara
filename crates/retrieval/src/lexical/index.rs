//! `lexical` stores one row per live occurrence.
//! A tombstone deletes its lexical row in the invalidation transaction.

use std::num::NonZeroUsize;

use rusqlite::{OptionalExtension, params};
use storage::GuardedConn;

use super::{Analysis, LexicalBounds, analyze};
use crate::{ProjectionError, QueryRow};

pub const OCCURRENCE_ID_COLUMN: &str = "occurrence_id";

/// Candidate rowids per occurrence: one per sixty-four-bit word of its identifier.
pub const ROWID_WORDS: usize = 4;

/// The rowids the lexical row of a hex occurrence identifier may take: each of its four sixty-four-bit words with the sign bit cleared, in identifier order.
/// The first unheld word determines the rowid, so placement depends on the identifier and on which other live occurrences share a word with it.
/// `None` for anything but sixty-four hex digits.
#[must_use]
pub fn rowids(occurrence_id: &str) -> Option<[i64; ROWID_WORDS]> {
    if occurrence_id.len() != 16 * ROWID_WORDS
        || !occurrence_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let mut words = [0i64; ROWID_WORDS];
    for (word, digits) in words.iter_mut().zip(occurrence_id.as_bytes().chunks(16)) {
        let bits = u64::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
        *word = (bits & (i64::MAX as u64)) as i64;
    }
    Some(words)
}

/// The first of [`rowids`]: where the row sits when no other occurrence holds that rowid.
#[must_use]
pub fn rowid(occurrence_id: &str) -> Option<i64> {
    rowids(occurrence_id).map(|words| words[0])
}

fn keyed(occurrence_id: &str) -> Result<[i64; ROWID_WORDS], ProjectionError> {
    rowids(occurrence_id).ok_or(ProjectionError::CorruptRow)
}

const WORDS_IN: &str = "rowid IN (?1,?2,?3,?4)";

/// Every atom has at least one byte, so `max_payload_bytes` also bounds `max_atoms`.
/// NUL is replaced with a space before analysis because the analyzer refuses it.
fn analyzed(payload: &str, max_payload_bytes: NonZeroUsize) -> Result<Analysis, ProjectionError> {
    Ok(analyze(
        &payload.replace('\0', " "),
        LexicalBounds {
            max_input_bytes: max_payload_bytes,
            max_atoms: max_payload_bytes,
        },
    )?)
}

fn same_text(analysis: &Analysis, original: &str, parts: &str) -> bool {
    analysis.original_text() == original && analysis.parts_text() == parts
}

/// Stores the row at the first of its rowids no other occurrence holds, unless the occurrence already has a row storing the text `payload` analyzes to; returns whether a row was written.
///
/// # Errors
///
/// If no keyed rowid is free, returns [`ProjectionError::LexicalRowidCollision`] listing the holders; an analyzer refusal is [`ProjectionError::Lexical`]; an existing row storing other text is [`ProjectionError::CorruptRow`].
pub(crate) fn insert(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    payload: &str,
    max_payload_bytes: NonZeroUsize,
) -> Result<bool, ProjectionError> {
    let words = keyed(occurrence_id)?;
    let analysis = analyzed(payload, max_payload_bytes)?;
    let mut held = conn.prepare_cached(&format!(
        "SELECT rowid, occurrence_id, original, parts FROM lexical WHERE {WORDS_IN}"
    ))?;
    let holders: Vec<(i64, String, String, String)> = held
        .query_map(words, |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if let Some((_, _, original, parts)) = holders
        .iter()
        .find(|(_, holder, _, _)| holder == occurrence_id)
    {
        return if same_text(&analysis, original, parts) {
            Ok(false)
        } else {
            Err(ProjectionError::CorruptRow)
        };
    }
    let Some(rowid) = words
        .iter()
        .copied()
        .find(|word| holders.iter().all(|(held, ..)| held != word))
    else {
        return Err(ProjectionError::LexicalRowidCollision {
            occurrence_id: occurrence_id.to_string(),
            holders: words
                .iter()
                .filter_map(|word| {
                    holders
                        .iter()
                        .find(|(held, ..)| held == word)
                        .map(|(_, holder, ..)| holder.clone())
                })
                .collect(),
        });
    };
    conn.prepare_cached(
        "INSERT INTO lexical(rowid, original, parts, occurrence_id) VALUES (?1,?2,?3,?4)",
    )?
    .execute(params![
        rowid,
        analysis.original_text(),
        analysis.parts_text(),
        occurrence_id
    ])?;
    Ok(true)
}

pub(crate) fn delete(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
) -> Result<usize, ProjectionError> {
    let words = keyed(occurrence_id)?;
    Ok(conn
        .prepare_cached(&format!(
            "DELETE FROM lexical WHERE {WORDS_IN} AND occurrence_id=?5"
        ))?
        .execute(params![
            words[0],
            words[1],
            words[2],
            words[3],
            occurrence_id
        ])?)
}

pub(crate) fn present(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
) -> Result<bool, ProjectionError> {
    let words = keyed(occurrence_id)?;
    Ok(conn
        .prepare_cached(&format!(
            "SELECT EXISTS(SELECT 1 FROM lexical WHERE {WORDS_IN} AND occurrence_id=?5)"
        ))?
        .query_row(
            params![words[0], words[1], words[2], words[3], occurrence_id],
            |row| row.get(0),
        )?)
}

/// Whether the occurrence's row exists and stores the text `payload` analyzes to.
///
/// # Errors
///
/// A row storing other text is [`ProjectionError::CorruptRow`]; an analyzer refusal is [`ProjectionError::Lexical`].
pub(crate) fn stores(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    payload: &str,
    max_payload_bytes: NonZeroUsize,
) -> Result<bool, ProjectionError> {
    let words = keyed(occurrence_id)?;
    let stored: Option<(String, String)> = conn
        .prepare_cached(&format!(
            "SELECT original, parts FROM lexical WHERE {WORDS_IN} AND occurrence_id=?5"
        ))?
        .query_row(
            params![words[0], words[1], words[2], words[3], occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((original, parts)) = stored else {
        return Ok(false);
    };
    if same_text(&analyzed(payload, max_payload_bytes)?, &original, &parts) {
        Ok(true)
    } else {
        Err(ProjectionError::CorruptRow)
    }
}

/// Every live occurrence has exactly one lexical row, and every lexical row sits at one of its occurrence's rowids and names a live occurrence.
/// `PRAGMA integrity_check` verifies the inverted index itself; this check covers the join the engine cannot see.
///
/// # Errors
///
/// A missing, orphaned, duplicated, or misplaced row is [`ProjectionError::CorruptRow`].
pub fn verify_rows(conn: &impl QueryRow) -> Result<(), ProjectionError> {
    let (live, rows, distinct): (i64, i64, i64) = conn.query_row(
        "SELECT (SELECT count(*) FROM occurrences o
                 WHERE NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)),
                (SELECT count(*) FROM lexical),
                (SELECT count(DISTINCT occurrence_id) FROM lexical)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if live != rows || rows != distinct {
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
        if !rowids(&occurrence_id).is_some_and(|words| words.contains(&stored))
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
