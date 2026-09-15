use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::{OccurrenceClass, Span, derived_lineage_id, identity_digest};
use rusqlite::params;
use storage::GuardedConn;

use crate::batch::{ProjectionCheckpoint, read_checkpoint};
use crate::exact::association::{
    CANONICAL_OBJECT_NAMESPACE, EXTRACTION_VERSION, derived_target, sha_namespace,
};
use crate::exact::selector::{Family, HexPrefix};
use crate::{ProjectionError, Tombstone, decode_span, decode_tombstone};

/// Variants correspond one to one with [`kernel::source_identity::OBJECT_FORMATS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    pub const ALL: [ObjectFormat; 2] = [Self::Sha1, Self::Sha256];

    pub fn code(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    /// Returns the number of hex digits in a complete object id accepted by the kernel.
    pub fn hex_len(self) -> usize {
        kernel::source_identity::OBJECT_FORMATS
            .iter()
            .find(|(code, _)| *code == self.code())
            .map(|(_, len)| *len)
            .expect("every ObjectFormat code is a kernel object format")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShaQueryRefusal {
    #[error("a {digits}-digit prefix exceeds the {} digits of {}", format.hex_len(), format.code())]
    PrefixTooLong { digits: usize, format: ObjectFormat },
}

/// A SHA lookup is only meaningful inside one repository's object format.
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
}

/// Only the families with a declared source mapping are queryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactQuery<'a> {
    CanonicalObject(&'a [u8]),
    Sha(ShaPrefixQuery<'a>),
}

impl ExactQuery<'_> {
    fn range(&self) -> KeyRange {
        match self {
            Self::CanonicalObject(key) => KeyRange::bounded(
                Family::Id,
                CANONICAL_OBJECT_NAMESPACE.to_string(),
                key.to_vec(),
                0x00,
            ),
            Self::Sha(query) => KeyRange::bounded(
                Family::Sha,
                query.namespace(),
                query.prefix.as_str().as_bytes().to_vec(),
                // Keys are lowercase hex, so `g` bounds every extension.
                if query.is_full_oid() { 0x00 } else { b'g' },
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyRange {
    family: Family,
    namespace: String,
    lo: Vec<u8>,
    hi: Vec<u8>,
}

impl KeyRange {
    /// `[lo, lo ++ sentinel)`: a NUL sentinel selects exactly `lo`.
    fn bounded(family: Family, namespace: String, lo: Vec<u8>, sentinel: u8) -> Self {
        let mut hi = lo.clone();
        hi.push(sentinel);
        Self {
            family,
            namespace,
            lo,
            hi,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    kernel_incarnation_id: String,
    range: KeyRange,
    checkpoint: ProjectionCheckpoint,
    last_key: Vec<u8>,
    last_occurrence_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationRow {
    pub key: Vec<u8>,
    pub target_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub class: OccurrenceClass,
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
    /// `None` means every matching row has been read.
    pub next: Option<Cursor>,
    pub checkpoint: ProjectionCheckpoint,
    /// Keys first seen in this page; summing pages counts each key once.
    pub distinct_keys: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct LookupContext<'a> {
    pub kernel_incarnation_id: &'a str,
    pub page_rows: NonZeroUsize,
    pub budget: &'a EvalBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LookupRefusal {
    #[error("the request budget is exhausted")]
    BudgetExhausted,
    /// No batch has committed, so there is no applied prefix to enumerate.
    #[error("the projection has no applied prefix")]
    NoCheckpoint,
    #[error("the cursor was issued at checkpoint {cursor} but the projection is at {current}")]
    StaleCursor { cursor: i64, current: i64 },
    #[error("the cursor belongs to another query or projection")]
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

const PAGE_COLUMNS: &str =
    "a.key,a.target_id,a.occurrence_id,a.extraction_version,a.created_commit_seq,
     o.lineage_id,o.class,o.revision,o.representation,o.span_start,o.span_end,o.payload_id,
     o.source_object_id,o.source_evidence_id,o.source_artifact_digest,o.created_commit_seq,
     t.invalidated_commit_seq,t.reason,o.tuple";

const PAGE_FROM: &str = "FROM exact_associations a
     JOIN occurrences o ON o.occurrence_id=a.occurrence_id
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=a.occurrence_id
     WHERE a.family=?1 AND a.namespace=?2 AND a.key<?4";

fn page_sql(has_cursor: bool) -> String {
    let lower = if has_cursor {
        "(a.key,a.occurrence_id)>(?3,?6)"
    } else {
        "a.key>=?3"
    };
    format!("SELECT {PAGE_COLUMNS} {PAGE_FROM} AND {lower} ORDER BY a.key,a.occurrence_id LIMIT ?5")
}

/// # Errors
///
/// Refuses an exhausted budget, a projection without a checkpoint or under
/// another kernel incarnation, a cursor from another query or checkpoint, and
/// rows derived under another extraction version.
pub fn page(
    conn: &GuardedConn<'_>,
    context: &LookupContext<'_>,
    query: &ExactQuery<'_>,
    cursor: Option<&Cursor>,
) -> Result<Page, LookupRefusal> {
    context
        .budget
        .check()
        .map_err(|_| LookupRefusal::BudgetExhausted)?;
    let checkpoint =
        read_checkpoint(conn, context.kernel_incarnation_id)?.ok_or(LookupRefusal::NoCheckpoint)?;
    let range = query.range();
    if let Some(cursor) = cursor {
        if cursor.range != range || cursor.kernel_incarnation_id != context.kernel_incarnation_id {
            return Err(LookupRefusal::ForeignCursor);
        }
        if cursor.checkpoint != checkpoint {
            return Err(LookupRefusal::StaleCursor {
                cursor: cursor.checkpoint.checkpoint_commit_seq,
                current: checkpoint.checkpoint_commit_seq,
            });
        }
    }
    let limit = i64::try_from(context.page_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare_cached(&page_sql(cursor.is_some()))?;
    let family = range.family.keyword();
    let mut rows = match cursor {
        Some(cursor) => statement.query(params![
            family,
            range.namespace,
            cursor.last_key,
            range.hi,
            limit,
            cursor.last_occurrence_id
        ])?,
        None => statement.query(params![family, range.namespace, range.lo, range.hi, limit])?,
    };
    let mut page = Page {
        rows: Vec::new(),
        next: None,
        checkpoint: checkpoint.clone(),
        distinct_keys: 0,
    };
    let mut previous_key = cursor.map(|cursor| cursor.last_key.clone());
    while let Some(row) = rows.next()? {
        if page.rows.len() == context.page_rows.get() {
            let last = page.rows.last().expect("page_rows is nonzero");
            page.next = Some(Cursor {
                kernel_incarnation_id: context.kernel_incarnation_id.to_string(),
                range,
                checkpoint,
                last_key: last.key.clone(),
                last_occurrence_id: last.occurrence_id.clone(),
            });
            break;
        }
        let version: u32 = row.get(3)?;
        if version != EXTRACTION_VERSION {
            return Err(LookupRefusal::ExtractionMismatch {
                stored: version,
                expected: EXTRACTION_VERSION,
            });
        }
        let decoded = AssociationRow::decode(row, range.family)?;
        if previous_key.as_ref() != Some(&decoded.key) {
            page.distinct_keys += 1;
        }
        previous_key = Some(decoded.key.clone());
        page.rows.push(decoded);
    }
    Ok(page)
}

impl AssociationRow {
    fn decode(row: &rusqlite::Row<'_>, family: Family) -> Result<Self, LookupRefusal> {
        let corrupt = || ProjectionError::CorruptRow;
        let span = decode_span(row.get(9)?, row.get(10)?)?;
        let occurrence_created_commit_seq: i64 = row.get(15)?;
        let tombstone = decode_tombstone(
            row.get(16)?,
            row.get::<_, Option<String>>(17)?.as_deref(),
            occurrence_created_commit_seq,
        )?;
        let class: String = row.get(6)?;
        let occurrence_id: String = row.get(2)?;
        let lineage_id: String = row.get(5)?;
        let revision: i64 = row.get(7)?;
        let representation: String = row.get(8)?;
        let tuple: Vec<u8> = row.get(18)?;
        let lineage = derived_lineage_id(
            &tuple,
            &class,
            revision,
            &representation,
            span.map(|(start, end)| Span { start, end }),
        );
        if identity_digest(&tuple) != occurrence_id || lineage.as_deref() != Some(&*lineage_id) {
            return Err(corrupt().into());
        }
        let key: Vec<u8> = row.get(0)?;
        let target_id: String = row.get(1)?;
        if derived_target(family, &key, &lineage_id) != Some(target_id.as_bytes()) {
            return Err(corrupt().into());
        }
        Ok(Self {
            key,
            target_id,
            occurrence_id,
            created_commit_seq: row.get(4)?,
            lineage_id,
            class: OccurrenceClass::from_code(&class).ok_or_else(corrupt)?,
            revision,
            representation,
            span,
            payload_id: row.get(11)?,
            source_object_id: row.get(12)?,
            source_evidence_id: row.get(13)?,
            source_artifact_digest: row.get(14)?,
            tombstone,
        })
    }
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
             WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<{count})
             INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                 extraction_version,created_commit_seq)
             SELECT 'sha','sha1:repo',CAST(printf('ab%038x',n) AS BLOB),printf('occ-%08d',n),
                 printf('obj-%08d',n),1,1 FROM ids;"
        ))
        .unwrap();
        conn
    }

    fn plan(conn: &Connection, sql: &str, params: &[rusqlite::types::Value]) -> Vec<String> {
        conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn first_and_cursor_pages_seek_the_primary_key_without_sorting_or_rescanning() {
        use rusqlite::types::Value;
        let mut first_steps = Vec::new();
        let mut resume_steps = Vec::new();
        for count in [1_024usize, 16_384] {
            let conn = seeded(count);
            let lo = Value::Blob(b"ab".to_vec());
            let hi = Value::Blob(b"abg".to_vec());
            let first = [
                Value::Text("sha".into()),
                Value::Text("sha1:repo".into()),
                lo.clone(),
                hi.clone(),
                Value::Integer(9),
            ];
            let first_plan = plan(&conn, &page_sql(false), &first);
            assert!(
                first_plan.iter().any(|d| d.contains(
                    "SEARCH a USING INDEX sqlite_autoindex_exact_associations_1 (family=? AND namespace=? AND key>? AND key<?)"
                )),
                "{first_plan:?}"
            );
            let last_key = Value::Blob(format!("ab{:038x}", count - 1).into_bytes());
            let resume = [
                Value::Text("sha".into()),
                Value::Text("sha1:repo".into()),
                last_key,
                hi,
                Value::Integer(9),
                Value::Text(format!("occ-{:08}", count - 1)),
            ];
            let resume_plan = plan(&conn, &page_sql(true), &resume);
            assert!(
                resume_plan.iter().any(|d| d.contains(
                    "SEARCH a USING INDEX sqlite_autoindex_exact_associations_1 (family=? AND namespace=? AND (key,occurrence_id)>(?,?) AND key<?)"
                )),
                "{resume_plan:?}"
            );
            for plan in [&first_plan, &resume_plan] {
                assert!(!plan.iter().any(|d| d.contains("TEMP B-TREE")), "{plan:?}");
            }
            let mut statement = conn.prepare(&page_sql(false)).unwrap();
            let keys = statement
                .query_map(rusqlite::params_from_iter(first.iter()), |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(keys.len(), 9);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            first_steps.push(statement.get_status(StatementStatus::VmStep));

            let mut statement = conn.prepare(&page_sql(true)).unwrap();
            let keys = statement
                .query_map(rusqlite::params_from_iter(resume.iter()), |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(keys, [format!("ab{:038x}", count).into_bytes()]);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            resume_steps.push(statement.get_status(StatementStatus::VmStep));
        }
        assert!(
            first_steps[1] <= first_steps[0] * 2,
            "first page must not grow with the collision count: {first_steps:?}"
        );
        assert!(
            resume_steps[1] <= resume_steps[0] * 2,
            "a cursor page must not rescan the range: {resume_steps:?}"
        );
    }

    #[test]
    fn a_prefix_query_binds_to_the_repository_format_and_refuses_overlong_prefixes() {
        let short = HexPrefix::parse("abc").unwrap();
        let full = HexPrefix::parse(&"a".repeat(40)).unwrap();
        let long = HexPrefix::parse(&"a".repeat(41)).unwrap();
        let query = ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &short).unwrap();
        assert!(!query.is_full_oid());
        assert_eq!(query.namespace(), "sha1:repo");
        assert_eq!(ExactQuery::Sha(query).range().hi, b"abcg");
        let query = ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &full).unwrap();
        assert!(query.is_full_oid());
        assert_eq!(ExactQuery::Sha(query).range().hi.last(), Some(&0));
        assert_eq!(
            ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &long).unwrap_err(),
            ShaQueryRefusal::PrefixTooLong {
                digits: 41,
                format: ObjectFormat::Sha1
            }
        );
        assert!(ShaPrefixQuery::bind("repo", ObjectFormat::Sha256, &long).is_ok());
        assert_ne!(
            ExactQuery::Sha(ShaPrefixQuery::bind("repo", ObjectFormat::Sha1, &short).unwrap())
                .range(),
            ExactQuery::Sha(ShaPrefixQuery::bind("repo", ObjectFormat::Sha256, &short).unwrap())
                .range(),
            "the algorithm is part of the query identity"
        );
        assert_eq!(ExactQuery::CanonicalObject(b"obj").range().hi, b"obj\0");
    }

    #[test]
    fn object_formats_are_exactly_the_kernel_table_so_every_written_sha_key_is_queryable() {
        use kernel::source_identity::{OBJECT_FORMATS, Occurrence, encode_preserving_span};

        let mut codes: Vec<&str> = ObjectFormat::ALL.iter().map(|f| f.code()).collect();
        codes.sort_unstable();
        let mut kernel_codes: Vec<&str> = OBJECT_FORMATS.iter().map(|(code, _)| *code).collect();
        kernel_codes.sort_unstable();
        assert_eq!(
            codes, kernel_codes,
            "a format on one side only is unreachable"
        );
        for format in ObjectFormat::ALL {
            let oid = "a".repeat(format.hex_len());
            let identity = [
                ("repository_id", "repo"),
                ("object_format", format.code()),
                ("oid", oid.as_str()),
            ];
            let commit = |oid: &str| {
                encode_preserving_span(&Occurrence {
                    class: "git_commits",
                    identity: &[identity[0], identity[1], ("oid", oid)],
                    revision: "1",
                    representation: "commit_message",
                    span: None,
                })
                .is_ok()
            };
            assert!(
                commit(&oid),
                "{format:?}: the kernel accepts a full-width oid"
            );
            assert!(
                !commit(&oid[1..]),
                "{format:?}: hex_len is the kernel's width"
            );
            let prefix = HexPrefix::parse(&oid).unwrap();
            let query = ShaPrefixQuery::bind("repo", format, &prefix).unwrap();
            assert!(query.is_full_oid());
            assert_eq!(query.namespace(), sha_namespace(format.code(), "repo"));
        }
    }
}
