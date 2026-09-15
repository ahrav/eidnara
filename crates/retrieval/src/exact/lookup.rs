use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use rusqlite::params;
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use crate::batch::read_checkpoint;
use crate::exact::association::{EXTRACTION_VERSION, sha_namespace};
use crate::exact::selector::{Family, HexPrefix};
use crate::{ProjectionError, Tombstone, TombstoneReason};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupBounds {
    pub page_rows: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    pub fn code(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            _ => None,
        }
    }

    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyQuery<'a> {
    pub family: Family,
    pub namespace: &'a str,
    pub key: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShaQueryRefusal {
    #[error("a {digits}-digit prefix exceeds the {} digits of {}", format.hex_len(), format.code())]
    PrefixTooLong { digits: usize, format: ObjectFormat },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaPrefixQuery<'a> {
    repository_id: &'a str,
    object_format: ObjectFormat,
    prefix: &'a HexPrefix,
}

impl<'a> ShaPrefixQuery<'a> {
    pub fn bind(
        repository_id: &'a str,
        object_format: ObjectFormat,
        prefix: &'a HexPrefix,
    ) -> Result<Self, ShaQueryRefusal> {
        if prefix.digits() > object_format.hex_len() {
            return Err(ShaQueryRefusal::PrefixTooLong {
                digits: prefix.digits(),
                format: object_format,
            });
        }
        Ok(Self {
            repository_id,
            object_format,
            prefix,
        })
    }

    pub fn is_full_oid(&self) -> bool {
        self.prefix.digits() == self.object_format.hex_len()
    }

    pub fn namespace(&self) -> String {
        sha_namespace(self.object_format.code(), self.repository_id)
    }

    fn range(&self) -> KeyRange {
        let lo = self.prefix.as_str().as_bytes().to_vec();
        let mut hi = lo.clone();
        // Keys are lowercase hex, so `g` bounds every extension of the prefix.
        hi.push(if self.is_full_oid() { 0x00 } else { b'g' });
        KeyRange {
            family: Family::Sha,
            namespace: self.namespace(),
            lo,
            hi,
        }
    }
}

struct KeyRange {
    family: Family,
    namespace: String,
    lo: Vec<u8>,
    hi: Vec<u8>,
}

impl KeyRange {
    fn equality(query: &KeyQuery<'_>) -> Self {
        let lo = query.key.to_vec();
        let mut hi = lo.clone();
        // A trailing NUL excludes every longer key while keeping the range shape.
        hi.push(0x00);
        Self {
            family: query.family,
            namespace: query.namespace.to_string(),
            lo,
            hi,
        }
    }

    fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        for part in [
            self.family.keyword().as_bytes(),
            self.namespace.as_bytes(),
            &self.lo,
            &self.hi,
        ] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part);
        }
        format!("{:x}", hasher.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    query: String,
    checkpoint_commit_seq: i64,
    last_key: Vec<u8>,
    last_occurrence_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationRow {
    pub key: Vec<u8>,
    pub target_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub class: String,
    pub revision: i64,
    pub representation: String,
    pub span: Option<(u64, u64)>,
    pub payload_id: String,
    pub source_object_id: String,
    pub source_evidence_id: String,
    pub source_artifact_digest: String,
    pub created_commit_seq: i64,
    pub tombstone: Option<Tombstone>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub rows: Vec<AssociationRow>,
    pub next: Option<Cursor>,
    /// `false` means matching rows remain unread.
    pub exhausted: bool,
    pub checkpoint_commit_seq: i64,
    /// Keys first seen in this page; summing pages counts each key once.
    pub distinct_keys: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LookupRefusal {
    #[error("the request budget is exhausted")]
    Interrupted,
    /// No batch has committed, so there is no applied prefix to enumerate.
    #[error("the projection has no applied prefix")]
    NoCheckpoint,
    #[error("the cursor was issued at checkpoint {cursor} but the projection is at {current}")]
    StaleCursor { cursor: i64, current: i64 },
    #[error("the cursor belongs to another query")]
    ForeignCursor,
    #[error("a stored association carries extraction version {stored}, not {expected}")]
    ExtractionMismatch { stored: u32, expected: u32 },
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

impl From<rusqlite::Error> for LookupRefusal {
    fn from(error: rusqlite::Error) -> Self {
        Self::Projection(error.into())
    }
}

/// Reads at most `bounds.page_rows` associations for one key in occurrence order.
///
/// # Errors
///
/// Refuses an exhausted budget, a projection without a checkpoint or under
/// another kernel incarnation, a cursor from another query or checkpoint, and
/// rows derived under another extraction version.
pub fn key_page(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    query: &KeyQuery<'_>,
    cursor: Option<&Cursor>,
    bounds: LookupBounds,
    budget: &EvalBudget,
) -> Result<Page, LookupRefusal> {
    range_page(
        conn,
        kernel_incarnation_id,
        &KeyRange::equality(query),
        cursor,
        bounds,
        budget,
    )
}

/// Reads at most `bounds.page_rows` associations whose key extends the prefix,
/// in key then occurrence order, within one repository and object format.
/// Tombstoned occurrences are included so colliding object ids stay counted.
///
/// # Errors
///
/// As [`key_page`].
pub fn sha_prefix_page(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    query: &ShaPrefixQuery<'_>,
    cursor: Option<&Cursor>,
    bounds: LookupBounds,
    budget: &EvalBudget,
) -> Result<Page, LookupRefusal> {
    range_page(
        conn,
        kernel_incarnation_id,
        &query.range(),
        cursor,
        bounds,
        budget,
    )
}

const PAGE_COLUMNS: &str =
    "a.key,a.target_id,a.occurrence_id,a.extraction_version,a.created_commit_seq,
     o.lineage_id,o.class,o.revision,o.representation,o.span_start,o.span_end,o.payload_id,
     o.source_object_id,o.source_evidence_id,o.source_artifact_digest,
     t.invalidated_commit_seq,t.reason";

const PAGE_FROM: &str = "FROM exact_associations a
     JOIN occurrences o ON o.occurrence_id=a.occurrence_id
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=a.occurrence_id
     WHERE a.family=?1 AND a.namespace=?2 AND a.key>=?3 AND a.key<?4";

fn page_sql(has_cursor: bool) -> String {
    let after = if has_cursor {
        " AND (a.key,a.occurrence_id)>(?6,?7)"
    } else {
        ""
    };
    format!("SELECT {PAGE_COLUMNS} {PAGE_FROM}{after} ORDER BY a.key,a.occurrence_id LIMIT ?5")
}

#[cfg(feature = "test-support")]
pub fn page_sql_for_test(has_cursor: bool) -> String {
    page_sql(has_cursor)
}

fn range_page(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    range: &KeyRange,
    cursor: Option<&Cursor>,
    bounds: LookupBounds,
    budget: &EvalBudget,
) -> Result<Page, LookupRefusal> {
    budget.check().map_err(|_| LookupRefusal::Interrupted)?;
    let checkpoint =
        read_checkpoint(conn, kernel_incarnation_id)?.ok_or(LookupRefusal::NoCheckpoint)?;
    let digest = range.digest();
    if let Some(cursor) = cursor {
        if cursor.query != digest {
            return Err(LookupRefusal::ForeignCursor);
        }
        if cursor.checkpoint_commit_seq != checkpoint.checkpoint_commit_seq {
            return Err(LookupRefusal::StaleCursor {
                cursor: cursor.checkpoint_commit_seq,
                current: checkpoint.checkpoint_commit_seq,
            });
        }
    }
    let limit = i64::try_from(bounds.page_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(&page_sql(cursor.is_some()))?;
    let family = range.family.keyword();
    let mut rows = match cursor {
        Some(cursor) => statement.query(params![
            family,
            range.namespace,
            range.lo,
            range.hi,
            limit,
            cursor.last_key,
            cursor.last_occurrence_id
        ])?,
        None => statement.query(params![family, range.namespace, range.lo, range.hi, limit])?,
    };
    let mut page = Page {
        rows: Vec::new(),
        next: None,
        exhausted: true,
        checkpoint_commit_seq: checkpoint.checkpoint_commit_seq,
        distinct_keys: 0,
    };
    let mut previous_key = cursor.map(|cursor| cursor.last_key.clone());
    while let Some(row) = rows.next()? {
        if page.rows.len() == bounds.page_rows.get() {
            page.exhausted = false;
            break;
        }
        let version: u32 = row.get(3)?;
        if version != EXTRACTION_VERSION {
            return Err(LookupRefusal::ExtractionMismatch {
                stored: version,
                expected: EXTRACTION_VERSION,
            });
        }
        let key: Vec<u8> = row.get(0)?;
        if previous_key.as_ref() != Some(&key) {
            page.distinct_keys += 1;
        }
        previous_key = Some(key.clone());
        let span = match (
            row.get::<_, Option<i64>>(9)?,
            row.get::<_, Option<i64>>(10)?,
        ) {
            (Some(start), Some(end)) => Some((
                u64::try_from(start).map_err(|_| ProjectionError::CorruptRow)?,
                u64::try_from(end).map_err(|_| ProjectionError::CorruptRow)?,
            )),
            (None, None) => None,
            _ => return Err(ProjectionError::CorruptRow.into()),
        };
        let tombstone = match (
            row.get::<_, Option<i64>>(15)?,
            row.get::<_, Option<String>>(16)?,
        ) {
            (Some(invalidated_commit_seq), Some(reason)) => Some(Tombstone {
                invalidated_commit_seq,
                reason: TombstoneReason::ALL
                    .into_iter()
                    .find(|candidate| candidate.as_str() == reason)
                    .ok_or(ProjectionError::CorruptRow)?,
            }),
            (None, None) => None,
            _ => return Err(ProjectionError::CorruptRow.into()),
        };
        page.rows.push(AssociationRow {
            key,
            target_id: row.get(1)?,
            occurrence_id: row.get(2)?,
            created_commit_seq: row.get(4)?,
            lineage_id: row.get(5)?,
            class: row.get(6)?,
            revision: row.get(7)?,
            representation: row.get(8)?,
            span,
            payload_id: row.get(11)?,
            source_object_id: row.get(12)?,
            source_evidence_id: row.get(13)?,
            source_artifact_digest: row.get(14)?,
            tombstone,
        });
    }
    if !page.exhausted
        && let Some(last) = page.rows.last()
    {
        page.next = Some(Cursor {
            query: digest,
            checkpoint_commit_seq: checkpoint.checkpoint_commit_seq,
            last_key: last.key.clone(),
            last_occurrence_id: last.occurrence_id.clone(),
        });
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, StatementStatus};

    fn seeded(count: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO payloads(payload_id,bytes,byte_length,created_at) VALUES ('p',x'00',1,0);
             WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<{count})
             INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                 payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                 source_artifact_digest,created_commit_seq,persisted_at)
             SELECT printf('occ-%08d',n),x'00','l','canonical_claims',1,'decision_summary','p','d',
                 'normal','s','e','digest',1,0 FROM ids;
             WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<{count})
             INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                 extraction_version,created_commit_seq)
             SELECT 'id','canonical_object',CAST(printf('obj-%08d',n) AS BLOB),printf('occ-%08d',n),
                 printf('obj-%08d',n),1,1 FROM ids;
             INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                 extraction_version,created_commit_seq)
             VALUES ('sha','sha1:repo',CAST('abc123' AS BLOB),'occ-00000001','t',1,1),
                    ('sha','sha1:repo',CAST('abc999' AS BLOB),'occ-00000002','t',1,1),
                    ('sha','sha1:repo',CAST('abd000' AS BLOB),'occ-00000003','t',1,1);"
        ))
        .unwrap();
        conn
    }

    #[test]
    fn equality_and_range_pages_search_the_primary_key_without_sorting() {
        let mut measured = Vec::new();
        for count in [1_024usize, 16_384] {
            let conn = seeded(count);
            for has_cursor in [false, true] {
                let sql = page_sql(has_cursor);
                let plan: Vec<String> = conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(
                        rusqlite::params_from_iter(
                            [
                                rusqlite::types::Value::Text("sha".into()),
                                rusqlite::types::Value::Text("sha1:repo".into()),
                                rusqlite::types::Value::Blob(b"abc".to_vec()),
                                rusqlite::types::Value::Blob(b"abcg".to_vec()),
                                rusqlite::types::Value::Integer(3),
                                rusqlite::types::Value::Blob(b"abc123".to_vec()),
                                rusqlite::types::Value::Text("occ-00000001".into()),
                            ]
                            .into_iter()
                            .take(if has_cursor { 7 } else { 5 }),
                        ),
                        |row| row.get::<_, String>(3),
                    )
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert!(
                    plan.iter()
                        .any(|detail| detail.contains(
                            "SEARCH a USING INDEX sqlite_autoindex_exact_associations_1 (family=? AND namespace=? AND key>? AND key<?)"
                        )),
                    "{has_cursor}: {plan:?}"
                );
                assert!(
                    !plan.iter().any(|detail| detail.contains("TEMP B-TREE")),
                    "{has_cursor}: {plan:?}"
                );
            }
            let mut statement = conn.prepare(&page_sql(false)).unwrap();
            let keys = statement
                .query_map(
                    params!["sha", "sha1:repo", b"abc".to_vec(), b"abcg".to_vec(), 3],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(keys, [b"abc123".to_vec(), b"abc999".to_vec()]);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            measured.push((count, statement.get_status(StatementStatus::VmStep)));
        }
        assert!(
            measured.iter().all(|(_, steps)| *steps < 512),
            "{measured:?}"
        );
        assert!(measured[1].1 <= measured[0].1 * 2, "{measured:?}");
    }

    #[test]
    fn a_prefix_query_binds_to_the_repository_format_and_refuses_overlong_prefixes() {
        let short = HexPrefix::parse("abc").unwrap();
        let full = HexPrefix::parse(&"a".repeat(40)).unwrap();
        let long = HexPrefix::parse(&"a".repeat(41)).unwrap();
        let query = ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &short).unwrap();
        assert!(!query.is_full_oid());
        assert_eq!(query.namespace(), "sha1:repo");
        assert_eq!(query.range().hi, b"abcg");
        let query = ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &full).unwrap();
        assert!(query.is_full_oid());
        assert_eq!(query.range().hi.last(), Some(&0));
        assert_eq!(
            ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &long).unwrap_err(),
            ShaQueryRefusal::PrefixTooLong {
                digits: 41,
                format: ObjectFormat::Sha1
            }
        );
        assert!(ShaPrefixQuery::bind("repo", ObjectFormat::Sha256, &long).is_ok());
        assert_ne!(
            ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &short)
                .unwrap()
                .range()
                .digest(),
            ShaPrefixQuery::bind("repo", ObjectFormat::Sha256, &short)
                .unwrap()
                .range()
                .digest(),
            "the algorithm is part of the query identity"
        );
    }
}
