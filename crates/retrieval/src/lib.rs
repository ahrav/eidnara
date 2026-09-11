//! The search projection's pure layer: the baseline schema of `search.sqlite`
//! and the identities and mutations over it. Everything here runs inside a
//! transaction the caller owns; the daemon opens the store, holds the
//! connection, and runs the effects. Among product crates this crate depends
//! only on the kernel, whose CC4 occurrence encoder and CC5 payload identity
//! it reuses rather than restating.
//!
//! Persistence is exact or refused. An occurrence is stored beside its tuple
//! bytes and a payload beside its byte length; a digest is never taken as
//! equality. When an identifier already exists, the stored tuple or bytes are
//! compared with the incoming ones, equal values replay as a no-op, and unequal
//! values are refused without a suffix, a rename, or a replacement. Payloads
//! are never logged; refusals name identities and sizes, not content.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use kernel::Sensitivity;
use kernel::source_identity::{
    EncodedOccurrence, Occurrence, OccurrenceRefusal, covers_whole, encode, payload_id, select,
    validate_span,
};
use rusqlite::{CachedStatement, OptionalExtension, params};
use storage::GuardedConn;

/// The complete schema, applied once to a pristine file by
/// `storage::open_sqlite`. The bytes of this text are part of the store
/// identity checked on every open.
pub const BASELINE: &str = include_str!("../baseline.sql");

/// The schema version the baseline text implements. A projection whose stored
/// identity names another version is rebuilt.
pub const SCHEMA_VERSION: u32 = 1;

/// The identity every row in one projection was built under. Any component
/// that differs at open makes the projection incompatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionIdentity {
    pub schema_version: u32,
    pub kernel_incarnation_id: String,
    pub projection_policy_version: String,
    pub identity_contract_version: String,
    pub limit_manifest_protocol_version: String,
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub generation_epoch: u64,
}

/// One canonical descriptor to persist: the occurrence, the whole buffer its
/// span selects from, and the canonical row it came from. `Debug` reports the
/// buffer's byte length, never its text.
#[derive(Clone)]
pub struct OccurrenceRecord<'a> {
    pub occurrence: Occurrence<'a>,
    pub buffer: &'a str,
    pub domain_id: &'a str,
    pub sensitivity: Sensitivity,
    pub source_object_id: &'a str,
    pub source_evidence_id: &'a str,
    pub source_artifact_digest: &'a str,
    pub created_commit_seq: i64,
}

impl std::fmt::Debug for OccurrenceRecord<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OccurrenceRecord")
            .field("occurrence", &self.occurrence)
            .field("buffer_bytes", &self.buffer.len())
            .field("domain_id", &self.domain_id)
            .field("sensitivity", &self.sensitivity)
            .field("source_object_id", &self.source_object_id)
            .field("source_evidence_id", &self.source_evidence_id)
            .field("source_artifact_digest", &self.source_artifact_digest)
            .field("created_commit_seq", &self.created_commit_seq)
            .finish()
    }
}

/// Bounds checked before any row is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistBounds {
    /// Records one call may persist.
    pub max_records: NonZeroUsize,
    /// Selected bytes one payload may carry.
    pub max_payload_bytes: NonZeroUsize,
    /// Encoded tuple bytes one occurrence may carry.
    pub max_tuple_bytes: NonZeroUsize,
}

/// What persisting one record did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedOccurrence {
    pub occurrence_id: String,
    pub lineage_id: String,
    pub payload_id: String,
    /// `false` when an equal occurrence was already stored and this call
    /// changed nothing for it.
    pub inserted: bool,
    /// `false` when equal bytes were already stored under the payload identity.
    pub payload_inserted: bool,
}

/// One stored occurrence read back with its bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredOccurrence {
    pub occurrence_id: String,
    pub tuple: Vec<u8>,
    pub lineage_id: String,
    pub class: String,
    pub revision: i64,
    pub representation: String,
    pub span: Option<(u64, u64)>,
    pub payload_id: String,
    pub bytes: Vec<u8>,
    pub domain_id: String,
    pub sensitivity: Sensitivity,
    pub source_object_id: String,
    pub source_evidence_id: String,
    pub source_artifact_digest: String,
    pub created_commit_seq: i64,
    pub tombstone: Option<Tombstone>,
}

impl std::fmt::Debug for StoredOccurrence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredOccurrence")
            .field("occurrence_id", &self.occurrence_id)
            .field("lineage_id", &self.lineage_id)
            .field("class", &self.class)
            .field("revision", &self.revision)
            .field("representation", &self.representation)
            .field("span", &self.span)
            .field("payload_id", &self.payload_id)
            .field("byte_length", &self.bytes.len())
            .field("domain_id", &self.domain_id)
            .field("sensitivity", &self.sensitivity)
            .field("source_object_id", &self.source_object_id)
            .field("created_commit_seq", &self.created_commit_seq)
            .field("tombstone", &self.tombstone)
            .finish()
    }
}

/// Why an occurrence stopped being live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneReason {
    Superseded,
    Retired,
    EvidenceInvalidated,
    Purged,
}

impl TombstoneReason {
    /// The whole vocabulary, in the order the schema's `CHECK` spells it.
    pub const ALL: [Self; 4] = [
        Self::Superseded,
        Self::Retired,
        Self::EvidenceInvalidated,
        Self::Purged,
    ];

    /// The spelling stored in `occurrence_tombstones.reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Superseded => "superseded",
            Self::Retired => "retired",
            Self::EvidenceInvalidated => "evidence_invalidated",
            Self::Purged => "purged",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tombstone {
    pub invalidated_commit_seq: i64,
    pub reason: TombstoneReason,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    #[error("occurrence refused: {}", .0.name())]
    Occurrence(OccurrenceRefusal),
    #[error("record {index} exceeds the {bound} bound with {size} bytes")]
    OverBound {
        index: usize,
        bound: &'static str,
        size: usize,
    },
    #[error("{count} records exceed the per-call bound")]
    TooManyRecords { count: usize },
    #[error("record {index} names a commit sequence that is not positive")]
    NonPositiveSequence { index: usize },
    #[error(
        "the tombstone for occurrence {occurrence_id} names a commit sequence that is not positive"
    )]
    NonPositiveTombstoneSequence { occurrence_id: String },
    #[error(
        "occurrence {occurrence_id} is stored with a different tuple under the same identifier"
    )]
    OccurrenceCollision { occurrence_id: String },
    #[error("payload {payload_id} is stored with different bytes under the same identifier")]
    PayloadCollision { payload_id: String },
    #[error(
        "occurrence {occurrence_id} already has a tombstone with a different sequence or reason"
    )]
    TombstoneCollision { occurrence_id: String },
    #[error("the projection identity is already installed and differs")]
    IdentityMismatch,
    #[error("occurrence {occurrence_id} is not stored")]
    UnknownOccurrence { occurrence_id: String },
    #[error("a stored row is not in the shape the schema promises")]
    CorruptRow,
    #[error("sqlite: {0}")]
    Sqlite(String),
}

impl From<rusqlite::Error> for ProjectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error.to_string())
    }
}

impl From<OccurrenceRefusal> for ProjectionError {
    fn from(refusal: OccurrenceRefusal) -> Self {
        Self::Occurrence(refusal)
    }
}

fn parse_sensitivity(value: &str) -> Option<Sensitivity> {
    Sensitivity::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == value)
}

/// Installs the identity in an empty projection or checks an installed one.
pub fn install_identity(
    conn: &GuardedConn<'_>,
    identity: &ProjectionIdentity,
    installed_at: i64,
) -> Result<(), ProjectionError> {
    if identity.schema_version != SCHEMA_VERSION {
        return Err(ProjectionError::IdentityMismatch);
    }
    if let Some(stored) = read_identity(conn)? {
        return if stored == *identity {
            Ok(())
        } else {
            Err(ProjectionError::IdentityMismatch)
        };
    }
    conn.execute(
        "INSERT INTO projection_identity(
             singleton,schema_version,kernel_incarnation_id,projection_policy_version,
             identity_contract_version,limit_manifest_protocol_version,embedding_model,
             tokenizer_fingerprint,vector_dimension,generation_epoch,installed_at
         ) VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            identity.schema_version,
            identity.kernel_incarnation_id,
            identity.projection_policy_version,
            identity.identity_contract_version,
            identity.limit_manifest_protocol_version,
            identity.embedding_model,
            identity.tokenizer_fingerprint,
            identity.vector_dimension,
            i64::try_from(identity.generation_epoch).map_err(|_| ProjectionError::CorruptRow)?,
            installed_at,
        ],
    )?;
    Ok(())
}

pub fn read_identity(
    conn: &GuardedConn<'_>,
) -> Result<Option<ProjectionIdentity>, ProjectionError> {
    let row: Option<(ProjectionIdentity, i64)> = conn
        .query_row(
            "SELECT schema_version,kernel_incarnation_id,projection_policy_version,
                    identity_contract_version,limit_manifest_protocol_version,embedding_model,
                    tokenizer_fingerprint,vector_dimension,generation_epoch
             FROM projection_identity WHERE singleton=1",
            [],
            |row| {
                Ok((
                    ProjectionIdentity {
                        schema_version: row.get(0)?,
                        kernel_incarnation_id: row.get(1)?,
                        projection_policy_version: row.get(2)?,
                        identity_contract_version: row.get(3)?,
                        limit_manifest_protocol_version: row.get(4)?,
                        embedding_model: row.get(5)?,
                        tokenizer_fingerprint: row.get(6)?,
                        vector_dimension: row.get(7)?,
                        generation_epoch: 0,
                    },
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    row.map(|(identity, epoch)| {
        Ok(ProjectionIdentity {
            generation_epoch: u64::try_from(epoch).map_err(|_| ProjectionError::CorruptRow)?,
            ..identity
        })
    })
    .transpose()
}

/// The digests one record persists under. Production computes both from the
/// bytes; a test may inject them to force a collision.
type Digests<'a> = dyn Fn(&EncodedOccurrence, &[u8]) -> (String, String) + 'a;

fn canonical_digests(encoded: &EncodedOccurrence, selected: &[u8]) -> (String, String) {
    (encoded.occurrence_id.clone(), payload_id(selected))
}

/// Persists `records` in input order. Every collision check runs before the
/// first `INSERT`, so a refused batch writes no rows.
pub fn persist_occurrences(
    conn: &GuardedConn<'_>,
    records: &[OccurrenceRecord<'_>],
    bounds: PersistBounds,
    persisted_at: i64,
) -> Result<Vec<PersistedOccurrence>, ProjectionError> {
    persist_with_digests(conn, records, bounds, persisted_at, &canonical_digests)
}

/// [`persist_occurrences`] with `digests` naming the occurrence and payload
/// identifiers, so a test can make two unequal values meet under one digest.
#[cfg(feature = "test-support")]
pub fn persist_occurrences_with_digests_for_test(
    conn: &GuardedConn<'_>,
    records: &[OccurrenceRecord<'_>],
    bounds: PersistBounds,
    persisted_at: i64,
    digests: &Digests<'_>,
) -> Result<Vec<PersistedOccurrence>, ProjectionError> {
    persist_with_digests(conn, records, bounds, persisted_at, digests)
}

struct Prepared<'a> {
    record: &'a OccurrenceRecord<'a>,
    encoded: EncodedOccurrence,
    selected: &'a [u8],
    occurrence_id: String,
    payload_id: String,
    insert_occurrence: bool,
    insert_payload: bool,
}

/// A batch prepares these statements once rather than once per record.
struct Statements<'c> {
    occurrence_lookup: CachedStatement<'c>,
    payload_lookup: CachedStatement<'c>,
    payload_insert: CachedStatement<'c>,
    occurrence_insert: CachedStatement<'c>,
}

impl<'c> Statements<'c> {
    fn prepare(conn: &GuardedConn<'c>) -> Result<Self, ProjectionError> {
        Ok(Self {
            occurrence_lookup: conn
                .prepare_cached("SELECT tuple,payload_id FROM occurrences WHERE occurrence_id=?1")?,
            payload_lookup: conn.prepare_cached("SELECT bytes FROM payloads WHERE payload_id=?1")?,
            payload_insert: conn.prepare_cached(
                "INSERT INTO payloads(payload_id,bytes,byte_length,created_at) VALUES (?1,?2,?3,?4)",
            )?,
            occurrence_insert: conn.prepare_cached(
                "INSERT INTO occurrences(
                     occurrence_id,tuple,lineage_id,class,revision,representation,
                     span_start,span_end,payload_id,domain_id,sensitivity,source_object_id,
                     source_evidence_id,source_artifact_digest,created_commit_seq,persisted_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            )?,
        })
    }

    fn occurrence_stored(
        &mut self,
        occurrence_id: &str,
        tuple: &[u8],
        payload_id: &str,
    ) -> Result<bool, ProjectionError> {
        let stored: Option<(Vec<u8>, String)> = self
            .occurrence_lookup
            .query_row([occurrence_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        match stored {
            None => Ok(false),
            Some((stored_tuple, stored_payload))
                if stored_tuple == tuple && stored_payload == payload_id =>
            {
                Ok(true)
            }
            Some(_) => Err(ProjectionError::OccurrenceCollision {
                occurrence_id: occurrence_id.to_string(),
            }),
        }
    }

    fn payload_stored(
        &mut self,
        payload_id: &str,
        selected: &[u8],
    ) -> Result<bool, ProjectionError> {
        let stored: Option<Vec<u8>> = self
            .payload_lookup
            .query_row([payload_id], |row| row.get(0))
            .optional()?;
        match stored {
            None => Ok(false),
            Some(bytes) if bytes == selected => Ok(true),
            Some(_) => Err(ProjectionError::PayloadCollision {
                payload_id: payload_id.to_string(),
            }),
        }
    }

    fn insert_payload(
        &mut self,
        item: &Prepared<'_>,
        persisted_at: i64,
    ) -> Result<(), ProjectionError> {
        self.payload_insert.execute(params![
            item.payload_id,
            item.selected,
            i64::try_from(item.selected.len()).map_err(|_| ProjectionError::CorruptRow)?,
            persisted_at,
        ])?;
        Ok(())
    }

    fn insert_occurrence(
        &mut self,
        item: &Prepared<'_>,
        persisted_at: i64,
    ) -> Result<(), ProjectionError> {
        let record = item.record;
        let span = item
            .encoded
            .span
            .map(|span| {
                Ok::<_, ProjectionError>((
                    i64::try_from(span.start).map_err(|_| ProjectionError::CorruptRow)?,
                    i64::try_from(span.end).map_err(|_| ProjectionError::CorruptRow)?,
                ))
            })
            .transpose()?;
        self.occurrence_insert.execute(params![
            item.occurrence_id,
            item.encoded.tuple,
            item.encoded.lineage_id,
            item.encoded.class.code(),
            item.encoded.revision,
            record.occurrence.representation,
            span.map(|(start, _)| start),
            span.map(|(_, end)| end),
            item.payload_id,
            record.domain_id,
            record.sensitivity.as_str(),
            record.source_object_id,
            record.source_evidence_id,
            record.source_artifact_digest,
            record.created_commit_seq,
            persisted_at,
        ])?;
        Ok(())
    }
}

fn persist_with_digests<'c>(
    conn: &GuardedConn<'c>,
    records: &[OccurrenceRecord<'_>],
    bounds: PersistBounds,
    persisted_at: i64,
    digests: &Digests<'_>,
) -> Result<Vec<PersistedOccurrence>, ProjectionError> {
    if records.len() > bounds.max_records.get() {
        return Err(ProjectionError::TooManyRecords {
            count: records.len(),
        });
    }
    let mut statements = Statements::prepare(conn)?;
    let mut prepared: Vec<Prepared<'_>> = Vec::with_capacity(records.len());
    let mut batch_occurrences: HashMap<String, usize> = HashMap::new();
    let mut batch_payloads: HashMap<String, usize> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        if record.created_commit_seq <= 0 {
            return Err(ProjectionError::NonPositiveSequence { index });
        }
        let mut encoded = encode(&record.occurrence)?;
        validate_span(record.occurrence.span, record.buffer)?;
        if covers_whole(encoded.span, record.buffer) {
            encoded = encode(&Occurrence {
                span: None,
                ..record.occurrence
            })?;
        }
        if encoded.tuple.len() > bounds.max_tuple_bytes.get() {
            return Err(ProjectionError::OverBound {
                index,
                bound: "tuple_bytes",
                size: encoded.tuple.len(),
            });
        }
        let selected = select(record.occurrence.span, record.buffer);
        if selected.len() > bounds.max_payload_bytes.get() {
            return Err(ProjectionError::OverBound {
                index,
                bound: "payload_bytes",
                size: selected.len(),
            });
        }
        let (occurrence_id, payload_id) = digests(&encoded, selected);
        let insert_occurrence = match batch_occurrences.get(&occurrence_id) {
            Some(&earlier) => {
                let earlier = &prepared[earlier];
                if earlier.encoded.tuple != encoded.tuple || earlier.payload_id != payload_id {
                    return Err(ProjectionError::OccurrenceCollision { occurrence_id });
                }
                false
            }
            None => {
                let stored =
                    statements.occurrence_stored(&occurrence_id, &encoded.tuple, &payload_id)?;
                batch_occurrences.insert(occurrence_id.clone(), index);
                !stored
            }
        };
        let insert_payload = match batch_payloads.get(&payload_id) {
            Some(&earlier) => {
                if prepared[earlier].selected != selected {
                    return Err(ProjectionError::PayloadCollision { payload_id });
                }
                false
            }
            None => {
                let stored = statements.payload_stored(&payload_id, selected)?;
                batch_payloads.insert(payload_id.clone(), index);
                !stored
            }
        };
        prepared.push(Prepared {
            record,
            encoded,
            selected,
            occurrence_id,
            payload_id,
            insert_occurrence,
            insert_payload,
        });
    }
    // Insert payload rows before occurrence rows to satisfy the foreign key.
    for item in &prepared {
        if item.insert_payload {
            statements.insert_payload(item, persisted_at)?;
        }
        if item.insert_occurrence {
            statements.insert_occurrence(item, persisted_at)?;
        }
    }
    Ok(prepared
        .into_iter()
        .map(|item| PersistedOccurrence {
            occurrence_id: item.occurrence_id,
            lineage_id: item.encoded.lineage_id,
            payload_id: item.payload_id,
            inserted: item.insert_occurrence,
            payload_inserted: item.insert_payload,
        })
        .collect())
}

/// Records that an occurrence stopped being live. Recording the same
/// tombstone again is a no-op; a different one for the same occurrence is a
/// collision, because an invalidation fact never changes.
pub fn tombstone_occurrence(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
    tombstone: Tombstone,
    recorded_at: i64,
) -> Result<bool, ProjectionError> {
    if tombstone.invalidated_commit_seq <= 0 {
        return Err(ProjectionError::NonPositiveTombstoneSequence {
            occurrence_id: occurrence_id.to_string(),
        });
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM occurrences WHERE occurrence_id=?1)",
        [occurrence_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(ProjectionError::UnknownOccurrence {
            occurrence_id: occurrence_id.to_string(),
        });
    }
    let stored: Option<(i64, String)> = conn
        .query_row(
            "SELECT invalidated_commit_seq,reason FROM occurrence_tombstones WHERE occurrence_id=?1",
            [occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match stored {
        Some((at, reason))
            if at == tombstone.invalidated_commit_seq && reason == tombstone.reason.as_str() =>
        {
            Ok(false)
        }
        Some(_) => Err(ProjectionError::TombstoneCollision {
            occurrence_id: occurrence_id.to_string(),
        }),
        None => {
            conn.execute(
                "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
                 VALUES (?1,?2,?3,?4)",
                params![
                    occurrence_id,
                    tombstone.invalidated_commit_seq,
                    tombstone.reason.as_str(),
                    recorded_at,
                ],
            )?;
            Ok(true)
        }
    }
}

/// One stored occurrence with its payload bytes and tombstone, or `None`. A
/// row whose columns do not decode to the shape the schema promises is
/// `CorruptRow`.
pub fn read_occurrence(
    conn: &GuardedConn<'_>,
    occurrence_id: &str,
) -> Result<Option<StoredOccurrence>, ProjectionError> {
    // The closure can only fail with a rusqlite error, so corruption is
    // signalled through `InvalidQuery` and mapped back below.
    let corrupt = || rusqlite::Error::InvalidQuery;
    let stored = conn
        .query_row(
            "SELECT o.occurrence_id,o.tuple,o.lineage_id,o.class,o.revision,o.representation,
                    o.span_start,o.span_end,o.payload_id,p.bytes,o.domain_id,o.sensitivity,
                    o.source_object_id,o.source_evidence_id,o.source_artifact_digest,
                    o.created_commit_seq,t.invalidated_commit_seq,t.reason
             FROM occurrences o
             JOIN payloads p ON p.payload_id=o.payload_id
             LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
             WHERE o.occurrence_id=?1",
            [occurrence_id],
            |row| {
                let span = match (row.get::<_, Option<i64>>(6)?, row.get::<_, Option<i64>>(7)?) {
                    (None, None) => None,
                    (Some(start), Some(end)) => Some((
                        u64::try_from(start).map_err(|_| corrupt())?,
                        u64::try_from(end).map_err(|_| corrupt())?,
                    )),
                    _ => return Err(corrupt()),
                };
                let tombstone = match (
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ) {
                    (None, None) => None,
                    (Some(invalidated_commit_seq), Some(reason)) => Some(Tombstone {
                        invalidated_commit_seq,
                        reason: TombstoneReason::parse(&reason).ok_or_else(corrupt)?,
                    }),
                    _ => return Err(corrupt()),
                };
                Ok(StoredOccurrence {
                    occurrence_id: row.get(0)?,
                    tuple: row.get(1)?,
                    lineage_id: row.get(2)?,
                    class: row.get(3)?,
                    revision: row.get(4)?,
                    representation: row.get(5)?,
                    span,
                    payload_id: row.get(8)?,
                    bytes: row.get(9)?,
                    domain_id: row.get(10)?,
                    sensitivity: parse_sensitivity(&row.get::<_, String>(11)?)
                        .ok_or_else(corrupt)?,
                    source_object_id: row.get(12)?,
                    source_evidence_id: row.get(13)?,
                    source_artifact_digest: row.get(14)?,
                    created_commit_seq: row.get(15)?,
                    tombstone,
                })
            },
        )
        .optional()
        .map_err(|error| match error {
            rusqlite::Error::InvalidQuery => ProjectionError::CorruptRow,
            other => other.into(),
        })?;
    Ok(stored)
}
