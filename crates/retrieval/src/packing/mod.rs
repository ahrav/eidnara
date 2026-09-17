//! Selected occurrences are read by occurrence identity from their own rows.
//! Byte-identical payloads remain distinct occurrences.

mod grouping;
mod required;
mod scan;

use std::num::NonZeroUsize;

use kernel::source_identity::{OccurrenceClass, Span, payload_id};
use kernel::{MAX_PAYLOAD_BYTES, Sensitivity};
use rusqlite::params;
use sha2::{Digest, Sha256};
use storage::GuardedConn;

use crate::eligibility::OccurrenceCandidate;
use crate::fusion::{IdentityRefusal, OccurrenceId, ParentGroupKey};
use crate::{ProjectionError, Tombstone, decode_span, decode_tombstone, parse_sensitivity};

pub use grouping::{Group, GroupIdentity, MergedRange, Partition, Selected, Ungrouped, group};
pub use required::{
    AdmittedRequired, RequiredBound, RequiredBounds, RequiredContextFailure, RequiredFact,
    RequiredRequest, RequiredReservation, TokenCount, admit_required, reserve_required,
};
pub use scan::{
    BoundExceeded, OptionalBound, OptionalBounds, Scan, admit_fused_candidates, admit_optional_set,
    skip_and_continue,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupingKey {
    class: OccurrenceClass,
    parent: ParentGroupKey,
    representation: String,
}

impl GroupingKey {
    pub fn class(&self) -> OccurrenceClass {
        self.class
    }

    pub fn parent(&self) -> &ParentGroupKey {
        &self.parent
    }

    pub fn representation(&self) -> &str {
        &self.representation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Grouping {
    Grouped(GroupingKey),
    NonGrouping(OccurrenceClass),
}

impl Grouping {
    /// Raw tool spans are the one class cut into ranges of a parent buffer;
    /// every other class persists whole objects, so no two of its occurrences
    /// can merge.
    #[must_use]
    pub fn applies_to(class: OccurrenceClass) -> bool {
        class == OccurrenceClass::RawToolSpans
    }

    /// [`ParentGroupKey::derive`] verifies that the tuple bytes match `class`,
    /// `revision`, `representation`, and `span`, so it runs for every class,
    /// not only the grouping one. A non-grouping class that skipped the
    /// derivation would accept a column altered independently of the tuple,
    /// relabelling a span row out of grouping or changing its revision.
    pub fn derive(
        tuple: &[u8],
        class: OccurrenceClass,
        revision: i64,
        representation: &str,
        span: Option<Span>,
    ) -> Result<Self, IdentityRefusal> {
        let parent = ParentGroupKey::derive(tuple, class, revision, representation, span)?;
        if !Self::applies_to(class) {
            return Ok(Self::NonGrouping(class));
        }
        Ok(Self::Grouped(GroupingKey {
            class,
            parent,
            representation: representation.to_owned(),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PayloadRef {
    pub payload_id: String,
    pub byte_length: u64,
}

impl PayloadRef {
    /// # Errors
    ///
    /// [`ProjectionError::CorruptRow`] when the length or the digest of `bytes`
    /// disagrees with this reference.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), ProjectionError> {
        if bytes.len() as u64 != self.byte_length || payload_id(bytes) != self.payload_id {
            return Err(ProjectionError::CorruptRow);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub domain_id: String,
    pub source_object_id: String,
    pub source_evidence_id: String,
    pub source_artifact_digest: String,
    pub created_commit_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedOccurrence {
    pub occurrence: OccurrenceId,
    pub class: OccurrenceClass,
    pub revision: i64,
    pub representation: String,
    pub span: Option<Span>,
    pub payload: PayloadRef,
    pub sensitivity: Sensitivity,
    pub provenance: Provenance,
    pub tombstone: Option<Tombstone>,
    pub grouping: Grouping,
}

impl SelectedOccurrence {
    #[must_use]
    pub fn is_stale_for(&self, selected_revision: i64) -> bool {
        self.tombstone.is_some() || self.revision != selected_revision
    }

    /// The verdict for this candidate lives only in the report
    /// [`crate::eligibility::judge_occurrences`] returns.
    pub fn eligibility_candidate(&self) -> OccurrenceCandidate {
        OccurrenceCandidate::new(
            self.occurrence.to_string(),
            self.class,
            self.provenance.source_object_id.clone(),
            self.revision,
            self.provenance.source_artifact_digest.clone(),
        )
    }
}

// `length(p.bytes)` is answered from the record header. `p.byte_length` is
// stored after the blob, so reading it walks every overflow page of the blob.
const SELECTED_SQL: &str =
    "SELECT o.tuple,o.class,o.revision,o.representation,o.span_start,o.span_end,
            o.payload_id,length(p.bytes),o.domain_id,o.sensitivity,o.source_object_id,
            o.source_evidence_id,o.source_artifact_digest,o.created_commit_seq,
            t.invalidated_commit_seq,t.reason
     FROM occurrences o
     LEFT JOIN payloads p ON p.payload_id=o.payload_id
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     WHERE o.occurrence_id=?1";

/// Reads each selected occurrence from its own row, in the order given,
/// without loading payload bytes. A repeated identity yields a repeated entry.
///
/// # Errors
///
/// [`ProjectionError::TooManyRecords`] when more than `max` identities are
/// given, [`ProjectionError::UnknownOccurrence`] for an identity with no row,
/// [`ProjectionError::CorruptRow`] for a row outside the schema's shape, whose
/// tuple digest does not match its identifier, whose class, revision,
/// representation, or span column disagrees with its tuple, or whose
/// eligibility metadata the kernel would refuse, and the SQLite error
/// otherwise.
pub fn read_selected(
    conn: &GuardedConn<'_>,
    selected: &[OccurrenceId],
    max: NonZeroUsize,
) -> Result<Vec<SelectedOccurrence>, ProjectionError> {
    if selected.len() > max.get() {
        return Err(ProjectionError::TooManyRecords {
            count: selected.len(),
        });
    }
    let mut statement = conn.prepare_cached(SELECTED_SQL)?;
    selected
        .iter()
        .map(|occurrence| {
            let occurrence_id = occurrence.to_string();
            let mut rows = statement.query(params![occurrence_id])?;
            let row = rows
                .next()?
                .ok_or(ProjectionError::UnknownOccurrence { occurrence_id })?;
            decode(row, *occurrence)
        })
        .collect()
}

fn decode(
    row: &rusqlite::Row<'_>,
    occurrence: OccurrenceId,
) -> Result<SelectedOccurrence, ProjectionError> {
    let corrupt = || ProjectionError::CorruptRow;
    let class = OccurrenceClass::from_code(&row.get::<_, String>(1)?).ok_or_else(corrupt)?;
    let representation: String = row.get(3)?;
    if !class.representations().contains(&representation.as_str()) {
        return Err(corrupt());
    }
    let revision: i64 = row.get(2)?;
    let span = decode_span(row.get(4)?, row.get(5)?)?.map(|(start, end)| Span { start, end });
    let byte_length = row
        .get::<_, Option<i64>>(7)?
        .and_then(|length| u64::try_from(length).ok())
        .ok_or_else(corrupt)?;
    let created_commit_seq: i64 = row.get(13)?;
    let tombstone = decode_tombstone(
        row.get(14)?,
        row.get::<_, Option<String>>(15)?.as_deref(),
        created_commit_seq,
    )?;
    let tuple = row.get_ref(0)?.as_blob().map_err(|_| corrupt())?;
    if Sha256::digest(tuple).as_slice() != occurrence.as_bytes() {
        return Err(corrupt());
    }
    let grouping =
        Grouping::derive(tuple, class, revision, &representation, span).map_err(|_| corrupt())?;
    let selected = SelectedOccurrence {
        occurrence,
        class,
        revision,
        representation,
        span,
        payload: PayloadRef {
            payload_id: row.get(6)?,
            byte_length,
        },
        sensitivity: parse_sensitivity(&row.get::<_, String>(9)?).ok_or_else(corrupt)?,
        provenance: Provenance {
            domain_id: row.get(8)?,
            source_object_id: row.get(10)?,
            source_evidence_id: row.get(11)?,
            source_artifact_digest: row.get(12)?,
            created_commit_seq,
        },
        tombstone,
        grouping,
    };
    // Metadata the kernel would refuse is this row's fault, not an untyped
    // `InvalidInput` over the whole batch it is judged in.
    selected
        .eligibility_candidate()
        .candidate
        .validate()
        .map_err(|_| corrupt())?;
    Ok(selected)
}

/// Returns bytes for `payload` without verifying its digest.
/// The caller must call [`PayloadRef::verify`] after releasing `conn` to avoid
/// hashing while the connection is held.
/// Rows with a mismatched byte length yield NULL, so SQLite does not
/// materialize their blobs.
///
/// # Errors
///
/// [`ProjectionError::CorruptRow`] when no row carries the identifier, the
/// reference exceeds [`MAX_PAYLOAD_BYTES`], or the row fails the length
/// predicate; the SQLite error otherwise.
pub fn fetch_payload(
    conn: &GuardedConn<'_>,
    payload: &PayloadRef,
) -> Result<Vec<u8>, ProjectionError> {
    let Ok(byte_length) = i64::try_from(payload.byte_length) else {
        return Err(ProjectionError::CorruptRow);
    };
    if payload.byte_length > MAX_PAYLOAD_BYTES as u64 {
        return Err(ProjectionError::CorruptRow);
    }
    let bytes: Option<Vec<u8>> = conn
        .prepare_cached(
            "SELECT CASE WHEN byte_length=length(bytes) AND byte_length=?2 THEN bytes END
             FROM payloads WHERE payload_id=?1",
        )?
        .query_row(params![payload.payload_id, byte_length], |row| row.get(0))
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => ProjectionError::CorruptRow,
            other => other.into(),
        })?;
    bytes.ok_or(ProjectionError::CorruptRow)
}
