//! A source descriptor is the canonical record that says "this occurrence
//! exists at this revision and its exact bytes are this artifact". It is an
//! observation whose lineage is the occurrence's class, native identity,
//! representation, and span, whose revision is the source revision, and whose
//! evidence is the exact-retained artifact. Publishing a newer revision
//! invalidates the predecessor in the same commit, so readers never see two
//! live revisions of one lineage or a gap between them.

use std::collections::HashSet;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::KernelStore;
use super::envelope::{Envelope, Sensitivity};
use super::redaction::{identity, redact};
use super::slice::{ObservationPayload, ObservationSpec};
use super::source_hold::{Descriptors, descriptor_rows_sql};
use super::source_identity::{
    self, EncodedOccurrence, Occurrence, OccurrenceClass, OccurrenceRefusal, encode,
    encode_preserving_span, payload_id, select, well_formed_value,
};
use super::{
    CachedSql, KernelError,
    cas::{is_artifact_digest, is_exact_retention},
    map_sqlite,
};

/// The observation kind every descriptor row carries.
pub const SOURCE_DESCRIPTOR_KIND: &str = "source_descriptor";
/// Version of the JSON detail a descriptor row stores.
pub const SOURCE_DESCRIPTOR_DETAIL_VERSION: u32 = 1;
/// Descriptors one commit may publish, so a batch's verification and row
/// writes stay bounded under the single writer.
pub const MAX_DESCRIPTORS_PER_COMMIT: usize = 1024;

const OCCURRENCE_ID_PREFIX: &str = "srcocc:";
const DESCRIPTOR_OBJECT_ID_PREFIX: &str = "srcdesc:";

/// Git policy versions identify the permitted refs and traversal boundary without changing occurrence identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceDescriptorPolicy {
    Native,
    Git { version: String },
}

impl SourceDescriptorPolicy {
    pub(crate) fn validate_for(&self, class: OccurrenceClass) -> Result<(), SourceDescriptorError> {
        match (self, class) {
            (Self::Git { version }, OccurrenceClass::GitCommits)
                if well_formed_value(version) && identity(version).is_ok() =>
            {
                Ok(())
            }
            (Self::Native, class) if class != OccurrenceClass::GitCommits => Ok(()),
            _ => Err(SourceDescriptorError::SourcePolicyRefused),
        }
    }
}

/// What a producer asks the kernel to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptorRequest<'a> {
    pub occurrence: Occurrence<'a>,
    pub source_policy: SourceDescriptorPolicy,
    /// Domain the descriptor row belongs to. A lineage lives in one domain;
    /// a later revision from another domain is refused.
    pub domain_id: &'a str,
    /// Project scope the row serves under; `None` publishes an unscoped row
    /// that no project route serves.
    pub scope_id: Option<&'a str>,
    /// The exact-retained artifact carrying the whole buffer the span selects from.
    pub evidence_id: &'a str,
    /// The digest of the whole buffer; must match the artifact the evidence cites.
    pub artifact_digest: &'a str,
    /// The whole buffer, so the span can be checked against the bytes it
    /// selects from. Every request's buffer must hash to `artifact_digest`.
    pub buffer: &'a str,
    pub sensitivity: Sensitivity,
    pub observed_at: i64,
}

/// Descriptor rows store versioned metadata and encoded identity tuples, not payload text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptorDetail {
    pub descriptor_version: u32,
    pub source_policy: SourceDescriptorPolicy,
    pub class: String,
    /// Identity fields in tuple order.
    pub identity: Vec<(String, String)>,
    pub revision: String,
    pub representation: String,
    pub span: Option<(u64, u64)>,
    pub occurrence_id: String,
    /// Encoded identity bytes support collision checks without trusting the digest.
    pub occurrence_tuple: Vec<u8>,
    pub lineage_id: String,
    pub payload_id: String,
    pub artifact_digest: String,
    pub evidence_id: String,
}

/// What one publication did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptorOutcome {
    pub object_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub payload_id: String,
    /// The predecessor invalidated in the same commit, when the lineage had a
    /// live revision.
    pub replaced_object_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceDescriptorError {
    #[error("source occurrence refused: {}", .0.name())]
    Occurrence(OccurrenceRefusal),
    #[error("descriptor evidence is not a live artifact")]
    EvidenceMissing,
    #[error("descriptor artifact digest does not match the evidence")]
    ArtifactMismatch,
    #[error("descriptor evidence was not retained exactly")]
    EvidenceNotExact,
    #[error("descriptor buffer does not hash to the artifact digest")]
    BufferMismatch,
    #[error("descriptor detail holds text the redactor would rewrite")]
    ContentRefused,
    #[error("descriptor revision does not advance its lineage")]
    RevisionNotAdvanced,
    #[error("descriptor lineage is owned by another domain")]
    DomainMismatch,
    #[error("descriptor lineage id names a different stored tuple")]
    LineageCollision,
    #[error("descriptor occurrence id names a different stored tuple")]
    OccurrenceCollision,
    #[error("descriptor lineage is published twice in one commit")]
    DuplicateLineage,
    #[error("descriptor source policy is missing or invalid")]
    SourcePolicyRefused,
    #[error("descriptor batch exceeds the per-commit bound")]
    BatchTooLarge,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

impl From<OccurrenceRefusal> for SourceDescriptorError {
    fn from(refusal: OccurrenceRefusal) -> Self {
        Self::Occurrence(refusal)
    }
}

impl SourceDescriptorError {
    /// The error the envelope records for this refusal: a kernel failure keeps
    /// its own kind, and every descriptor refusal is invalid input to the commit.
    fn poison(&self) -> KernelError {
        match self {
            Self::Kernel(error) => *error,
            _ => KernelError::InvalidInput,
        }
    }
}

/// The registry `object_id` of a descriptor row: lineage plus revision, so
/// each revision is its own object and succession has a predecessor to name.
pub fn descriptor_object_id(lineage_id: &str, revision: &str) -> String {
    format!("{DESCRIPTOR_OBJECT_ID_PREFIX}{lineage_id}:{revision}")
}

pub(crate) fn uses_descriptor_namespace(spec: &ObservationSpec) -> bool {
    spec.observation_kind == SOURCE_DESCRIPTOR_KIND
        || spec.observation_id.starts_with(OCCURRENCE_ID_PREFIX)
        || spec.object_id.starts_with(DESCRIPTOR_OBJECT_ID_PREFIX)
}

fn detail_for(
    request: &SourceDescriptorRequest<'_>,
    encoded: &EncodedOccurrence,
    payload_id: &str,
) -> SourceDescriptorDetail {
    SourceDescriptorDetail {
        descriptor_version: SOURCE_DESCRIPTOR_DETAIL_VERSION,
        source_policy: request.source_policy.clone(),
        class: encoded.class.code().to_string(),
        identity: encoded
            .class
            .identity_fields()
            .iter()
            .map(|field| {
                let value = request
                    .occurrence
                    .identity
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| *value)
                    .expect("encode verified every identity field");
                (field.to_string(), value.to_string())
            })
            .collect(),
        revision: request.occurrence.revision.to_string(),
        representation: request.occurrence.representation.to_string(),
        span: encoded.span.map(|span| (span.start, span.end)),
        occurrence_id: encoded.occurrence_id.clone(),
        occurrence_tuple: encoded.tuple.clone(),
        lineage_id: encoded.lineage_id.clone(),
        payload_id: payload_id.to_string(),
        artifact_digest: request.artifact_digest.to_string(),
        evidence_id: request.evidence_id.to_string(),
    }
}

/// Two details describe one lineage when every field the lineage id hashes
/// over agrees; the digest alone never decides.
fn same_lineage(stored: &SourceDescriptorDetail, fresh: &SourceDescriptorDetail) -> bool {
    stored.class == fresh.class
        && stored.identity == fresh.identity
        && stored.representation == fresh.representation
        && stored.span == fresh.span
}

pub(crate) fn stored_detail(payload: &[u8]) -> Result<SourceDescriptorDetail, KernelError> {
    let stored: ObservationPayload =
        serde_json::from_slice(payload).map_err(|_| KernelError::CorruptCanonicalRow)?;
    let detail: SourceDescriptorDetail = stored
        .detail
        .as_deref()
        .and_then(|detail| serde_json::from_str(detail).ok())
        .ok_or(KernelError::CorruptCanonicalRow)?;
    if detail.descriptor_version != SOURCE_DESCRIPTOR_DETAIL_VERSION {
        return Err(KernelError::CorruptCanonicalRow);
    }
    Ok(detail)
}

/// Publication derived the stored tuple, ids, span, payload id, and policy from
/// one encoding of one request, so `None` here is corruption of the stored row.
pub(crate) fn reencoded_identity(detail: &SourceDescriptorDetail) -> Option<EncodedOccurrence> {
    let span = detail
        .span
        .map(|(start, end)| source_identity::Span { start, end });
    let identity: Vec<(&str, &str)> = detail
        .identity
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let encoded = encode_preserving_span(&Occurrence {
        class: &detail.class,
        identity: &identity,
        revision: &detail.revision,
        representation: &detail.representation,
        span,
    })
    .ok()?;
    (encoded.occurrence_id == detail.occurrence_id
        && encoded.lineage_id == detail.lineage_id
        && encoded.tuple == detail.occurrence_tuple
        && encoded.span == span
        && is_artifact_digest(&detail.payload_id)
        && detail.source_policy.validate_for(encoded.class).is_ok())
    .then_some(encoded)
}

impl Envelope<'_> {
    /// Registry inserts use several writers; commit validation admits reserved IDs only when this envelope's descriptor publisher owns them.
    pub(super) fn check_descriptor_ownership(&self) -> Result<(), KernelError> {
        let mut statement = self
            .tx
            .prepare_cached(
                "SELECT object_id,source_id FROM object_registry
                 WHERE created_commit_seq=?1 AND object_id GLOB ?2",
            )
            .map_err(map_sqlite)?;
        let rows = statement
            .query_map(
                rusqlite::params![self.commit_seq, format!("{DESCRIPTOR_OBJECT_ID_PREFIX}*")],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(map_sqlite)?;
        for row in rows {
            let (object_id, lineage_id) = row.map_err(map_sqlite)?;
            if self.descriptor_objects.get(&lineage_id) != Some(&object_id) {
                return Err(KernelError::InvalidInput);
            }
        }
        Ok(())
    }

    /// Publishes one descriptor; see [`Envelope::publish_source_descriptors`].
    pub fn publish_source_descriptor(
        &mut self,
        request: &SourceDescriptorRequest<'_>,
    ) -> Result<SourceDescriptorOutcome, SourceDescriptorError> {
        let mut outcomes = self.publish_source_descriptors(std::slice::from_ref(request))?;
        Ok(outcomes.pop().expect("one outcome per request"))
    }

    /// Publishes descriptors in request order inside this commit.
    /// Encoding validates identity and span before evidence checks; a whole-buffer span encodes as a whole-block selection.
    /// Each buffer must hash to its artifact digest, and cited evidence must be live and exactly retained.
    /// Evidence metadata is checked once per distinct `(evidence, digest)` pair in the batch.
    /// Identity values and stored detail must survive redaction unchanged so their identifiers remain valid.
    /// Each lineage may appear once per envelope, and all calls share [`MAX_DESCRIPTORS_PER_COMMIT`].
    /// Publication replaces a live predecessor atomically and refuses stale revisions, domain changes, and unequal tuples sharing a digest.
    /// A refusal poisons the envelope, so the commit fails even if the caller discards the error.
    pub fn publish_source_descriptors(
        &mut self,
        requests: &[SourceDescriptorRequest<'_>],
    ) -> Result<Vec<SourceDescriptorOutcome>, SourceDescriptorError> {
        self.guarded_typed(SourceDescriptorError::poison, |envelope| {
            envelope.publish_batch(requests)
        })
    }

    fn publish_batch(
        &mut self,
        requests: &[SourceDescriptorRequest<'_>],
    ) -> Result<Vec<SourceDescriptorOutcome>, SourceDescriptorError> {
        if requests.len() > MAX_DESCRIPTORS_PER_COMMIT - self.descriptor_objects.len() {
            return Err(SourceDescriptorError::BatchTooLarge);
        }
        let mut checked_evidence: HashSet<(&str, &str)> = HashSet::new();
        let mut verified = Vec::with_capacity(requests.len());
        for request in requests {
            let encoded = encode(&request.occurrence, request.buffer)?;
            if checked_evidence.insert((request.evidence_id, request.artifact_digest)) {
                self.check_evidence(request)?;
            }
            if payload_id(request.buffer.as_bytes()) != request.artifact_digest {
                return Err(SourceDescriptorError::BufferMismatch);
            }
            if self
                .descriptor_objects
                .insert(
                    encoded.lineage_id.clone(),
                    descriptor_object_id(&encoded.lineage_id, request.occurrence.revision),
                )
                .is_some()
            {
                return Err(SourceDescriptorError::DuplicateLineage);
            }
            verified.push(encoded);
        }
        requests
            .iter()
            .zip(verified)
            .map(|(request, encoded)| self.publish_verified(request, encoded))
            .collect()
    }

    fn publish_verified(
        &mut self,
        request: &SourceDescriptorRequest<'_>,
        encoded: EncodedOccurrence,
    ) -> Result<SourceDescriptorOutcome, SourceDescriptorError> {
        request.source_policy.validate_for(encoded.class)?;
        // Each identity value is checked on its own with the non-aliasing
        // identity rule, then the whole detail once more: a value the
        // redactor would rewrite is content, and a rewritten detail would no
        // longer match the tuple it was hashed from.
        for (_, value) in request.occurrence.identity {
            if identity(value).is_err() {
                return Err(SourceDescriptorError::ContentRefused);
            }
        }
        // The buffer already hashed to the artifact digest, so the whole
        // buffer's payload id is that digest; only a proper sub-span is hashed.
        let payload_id = match encoded.span {
            None => request.artifact_digest.to_string(),
            Some(span) => payload_id(select(Some(span), request.buffer)),
        };
        let detail = detail_for(request, &encoded, &payload_id);
        let detail_json = serde_json::to_string(&detail).map_err(|_| KernelError::InvalidInput)?;
        if !redact(&detail_json)?.detections.is_empty() {
            return Err(SourceDescriptorError::ContentRefused);
        }
        let observation_id = format!("{OCCURRENCE_ID_PREFIX}{}", encoded.occurrence_id);
        let existing: Option<Vec<u8>> = self
            .tx
            .query_row_cached(
                "SELECT observation_payload FROM observations WHERE observation_id=?1",
                [&observation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if let Some(payload) = existing
            && stored_detail(&payload)?.occurrence_tuple != encoded.tuple
        {
            return Err(SourceDescriptorError::OccurrenceCollision);
        }
        let object_id = descriptor_object_id(&encoded.lineage_id, request.occurrence.revision);
        let predecessor = self.live_lineage(request.domain_id, &detail)?;
        if let Some((_, live_revision)) = &predecessor
            && encoded.revision <= *live_revision
        {
            return Err(SourceDescriptorError::RevisionNotAdvanced);
        }
        let spec = ObservationSpec {
            observation_id,
            object_id: object_id.clone(),
            domain_id: request.domain_id.to_string(),
            proposition_id: None,
            scope_id: request.scope_id.map(str::to_string),
            anchor_id: None,
            evidence_id: Some(request.evidence_id.to_string()),
            observation_kind: SOURCE_DESCRIPTOR_KIND.to_string(),
            payload: ObservationPayload {
                summary: SOURCE_DESCRIPTOR_KIND.to_string(),
                classification: encoded.class.code().to_string(),
                detail: Some(detail_json),
            },
            observed_at: request.observed_at,
            dependencies: Vec::new(),
            source_kind: encoded.class.code().to_string(),
            source_id: encoded.lineage_id.clone(),
            source_revision: encoded.revision,
            sensitivity: request.sensitivity,
        };
        match &predecessor {
            None => {
                self.insert_observation_inner(spec)?;
            }
            Some((replaced, _)) => {
                self.correct_observation_inner(replaced, spec)?;
            }
        }
        Ok(SourceDescriptorOutcome {
            object_id,
            occurrence_id: encoded.occurrence_id,
            lineage_id: encoded.lineage_id,
            payload_id,
            replaced_object_id: predecessor.map(|(object_id, _)| object_id),
        })
    }

    fn check_evidence(
        &self,
        request: &SourceDescriptorRequest<'_>,
    ) -> Result<(), SourceDescriptorError> {
        let stored: Option<(String, Vec<u8>)> = self
            .tx
            .query_row_cached(
                "SELECT artifact_digest,redaction_metadata FROM evidence_meta
                 WHERE evidence_id=?1 AND invalidated_commit_seq IS NULL",
                [request.evidence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        let (digest, redactions) = stored.ok_or(SourceDescriptorError::EvidenceMissing)?;
        if digest != request.artifact_digest {
            return Err(SourceDescriptorError::ArtifactMismatch);
        }
        if !is_exact_retention(&redactions) {
            return Err(SourceDescriptorError::EvidenceNotExact);
        }
        Ok(())
    }

    /// The live descriptor of this lineage and its revision, if any. More
    /// than one live row for one lineage is corruption; a row in another
    /// domain refuses the publication; and stored fields that differ from the
    /// fresh ones are a digest collision, refused rather than folded into the
    /// chain.
    fn live_lineage(
        &self,
        domain_id: &str,
        fresh: &SourceDescriptorDetail,
    ) -> Result<Option<(String, i64)>, SourceDescriptorError> {
        let object_pattern = descriptor_object_id(&fresh.lineage_id, "*");
        let mut statement = self
            .tx
            .prepare_cached(
                "SELECT o.object_id,o.domain_id,o.source_revision,b.observation_payload
                 FROM object_registry o
                 JOIN observations b ON b.object_id=o.object_id
                 WHERE o.object_kind='observation' AND o.source_id=?1 AND o.object_id GLOB ?2
                   AND b.observation_kind=?3 AND o.invalidated_commit_seq IS NULL
                   AND b.invalidated_commit_seq IS NULL
                 LIMIT 2",
            )
            .map_err(map_sqlite)?;
        let live: Vec<(String, String, i64, Vec<u8>)> = statement
            .query_map(
                [
                    fresh.lineage_id.as_str(),
                    object_pattern.as_str(),
                    SOURCE_DESCRIPTOR_KIND,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<_>>()
            .map_err(map_sqlite)?;
        if live.len() > 1 {
            return Err(KernelError::CorruptCanonicalRow.into());
        }
        let predecessor = live
            .first()
            .map(|(id, _, revision, _)| (id.clone(), *revision));
        let stored = match live.into_iter().next() {
            Some(row) => Some(row),
            None => self
                .tx
                .query_row_cached(
                    "SELECT o.object_id,o.domain_id,o.source_revision,b.observation_payload
                     FROM object_registry o
                     JOIN observations b ON b.object_id=o.object_id
                     WHERE o.object_kind='observation' AND o.source_id=?1 AND o.object_id GLOB ?2
                       AND b.observation_kind=?3
                     ORDER BY o.source_revision DESC LIMIT 1",
                    [
                        fresh.lineage_id.as_str(),
                        object_pattern.as_str(),
                        SOURCE_DESCRIPTOR_KIND,
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(map_sqlite)?,
        };
        let Some((_, domain, _, payload)) = stored else {
            return Ok(None);
        };
        let stored = stored_detail(&payload)?;
        if !same_lineage(&stored, fresh) {
            return Err(SourceDescriptorError::LineageCollision);
        }
        if domain != domain_id {
            return Err(SourceDescriptorError::DomainMismatch);
        }
        Ok(predecessor)
    }
}

/// One page of live descriptors of one class at a fixed sequence, in object id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDescriptorPage {
    pub rows: Vec<LiveDescriptor>,
    /// The object id to continue after, or `None` when this page ends the inventory.
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDescriptor {
    pub object_id: String,
    pub domain_id: String,
    pub detail: SourceDescriptorDetail,
}

impl KernelStore {
    /// The descriptors of `class` live at `requested`, keyset-paged by object id from `after`. The page uses the export's liveness predicate, so a descriptor whose cited evidence was deleted is absent here as it is from every export snapshot. A row whose stored identity does not re-encode to itself is refused rather than handed to a caller that may retire it.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] for a negative sequence, [`KernelError::FutureSnapshot`] when `requested` exceeds the tip, and [`KernelError::CorruptCanonicalRow`] when a stored descriptor does not decode or re-encode to its stored identity.
    pub fn live_source_descriptors(
        &self,
        class: OccurrenceClass,
        requested: i64,
        after: Option<&str>,
        max_rows: std::num::NonZeroUsize,
    ) -> Result<LiveDescriptorPage, KernelError> {
        let mut reader = self.lock_reader()?;
        let tx = reader
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        crate::slice::snapshot_tip(&tx, requested)?;
        let limit = i64::try_from(max_rows.get()).unwrap_or(i64::MAX);
        let sql = format!(
            "SELECT o.object_id,o.domain_id,b.observation_payload
             {rows}
               AND o.source_kind=?1 AND o.object_kind='observation'
               AND {live}
               AND o.object_id>?3
             ORDER BY o.object_id
             LIMIT ?4",
            rows = descriptor_rows_sql("idx_objects_source_descriptor_page"),
            live = Descriptors::LiveAtEnd.predicate("?2", "0"),
        );
        let mut statement = tx.prepare_cached(&sql).map_err(map_sqlite)?;
        let raw: Vec<(String, String, Vec<u8>)> = statement
            .query_map(
                rusqlite::params![
                    class.code(),
                    requested,
                    after.unwrap_or(""),
                    limit.saturating_add(1)
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<_>>()
            .map_err(map_sqlite)?;
        let next = (raw.len() > max_rows.get()).then(|| raw[max_rows.get() - 1].0.clone());
        let rows = raw
            .into_iter()
            .take(max_rows.get())
            .map(|(object_id, domain_id, payload)| {
                let detail = stored_detail(&payload)?;
                if detail.class != class.code() || reencoded_identity(&detail).is_none() {
                    return Err(KernelError::CorruptCanonicalRow);
                }
                Ok(LiveDescriptor {
                    object_id,
                    domain_id,
                    detail,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(LiveDescriptorPage { rows, next })
    }
}
