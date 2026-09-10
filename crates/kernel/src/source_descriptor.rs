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

use super::envelope::{Envelope, Sensitivity};
use super::redaction::{identity, redact};
use super::slice::{ObservationPayload, ObservationSpec};
use super::source_identity::{
    EncodedOccurrence, Occurrence, OccurrenceRefusal, covers_whole, encode, payload_id, select,
    validate_span,
};
use super::{CachedSql, KernelError, map_sqlite};

/// The observation kind every descriptor row carries.
pub const SOURCE_DESCRIPTOR_KIND: &str = "source_descriptor";
/// Version of the JSON detail a descriptor row stores.
pub const SOURCE_DESCRIPTOR_DETAIL_VERSION: u32 = 1;
/// Descriptors one commit may publish, so a batch's verification and row
/// writes stay bounded under the single writer.
pub const MAX_DESCRIPTORS_PER_COMMIT: usize = 1024;

/// What a producer asks the kernel to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptorRequest<'a> {
    pub occurrence: Occurrence<'a>,
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
    /// selects from. One commit verifies each distinct buffer once.
    pub buffer: &'a str,
    pub sensitivity: Sensitivity,
    pub observed_at: i64,
}

/// The versioned detail stored on the descriptor observation. Every field is
/// an identifier, a digest, or a bounded native identity value; no payload
/// text is stored here, and a detail the redactor would rewrite is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptorDetail {
    pub descriptor_version: u32,
    pub class: String,
    /// Identity fields in tuple order.
    pub identity: Vec<(String, String)>,
    pub revision: String,
    pub representation: String,
    pub span: Option<(u64, u64)>,
    pub occurrence_id: String,
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

/// The registry `object_id` of a descriptor row: lineage plus revision, so
/// each revision is its own object and succession has a predecessor to name.
pub fn descriptor_object_id(lineage_id: &str, revision: &str) -> String {
    format!("srcdesc:{lineage_id}:{revision}")
}

fn detail_for(
    request: &SourceDescriptorRequest<'_>,
    encoded: &EncodedOccurrence,
    payload_id: &str,
) -> SourceDescriptorDetail {
    SourceDescriptorDetail {
        descriptor_version: SOURCE_DESCRIPTOR_DETAIL_VERSION,
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

/// Evidence that was never rewritten stores an empty detection list.
const NO_DETECTIONS: &[u8] = b"[]";

impl Envelope<'_> {
    /// Publishes one descriptor; see [`Envelope::publish_source_descriptors`].
    pub fn publish_source_descriptor(
        &mut self,
        request: &SourceDescriptorRequest<'_>,
    ) -> Result<SourceDescriptorOutcome, SourceDescriptorError> {
        let mut outcomes = self.publish_source_descriptors(std::slice::from_ref(request))?;
        Ok(outcomes.pop().expect("one outcome per request"))
    }

    /// Publishes descriptors in request order inside this commit. For each:
    /// the occurrence is encoded and checked, then the span against the
    /// buffer (a span covering the whole buffer is normalized to the
    /// whole-block selection), then the artifact: the evidence row must be
    /// live, retained exactly, and carry `artifact_digest`, and the buffer
    /// must hash to it, which is verified once per distinct
    /// `(evidence, digest)` in the batch. The stored detail must survive the
    /// redactor unchanged, so no identity value can be content. The live
    /// predecessor of the lineage is found under this writer transaction and
    /// invalidated together with the new row; a revision that does not
    /// advance the lineage, a lineage owned by another domain, and a lineage
    /// id whose stored fields differ from the fresh ones are refused without
    /// touching either row. A refusal anywhere fails the whole commit.
    pub fn publish_source_descriptors(
        &mut self,
        requests: &[SourceDescriptorRequest<'_>],
    ) -> Result<Vec<SourceDescriptorOutcome>, SourceDescriptorError> {
        if requests.len() > MAX_DESCRIPTORS_PER_COMMIT {
            return Err(SourceDescriptorError::BatchTooLarge);
        }
        let mut verified: HashSet<(&str, &str)> = HashSet::new();
        let mut outcomes = Vec::with_capacity(requests.len());
        for request in requests {
            let mut encoded = encode(&request.occurrence)?;
            validate_span(encoded.span, request.buffer)?;
            if covers_whole(encoded.span, request.buffer) {
                encoded = encode(&Occurrence {
                    span: None,
                    ..request.occurrence.clone()
                })?;
            }
            if verified.insert((request.evidence_id, request.artifact_digest)) {
                self.check_evidence(request)?;
            }
            outcomes.push(self.publish_verified(request, encoded)?);
        }
        Ok(outcomes)
    }

    fn publish_verified(
        &mut self,
        request: &SourceDescriptorRequest<'_>,
        encoded: EncodedOccurrence,
    ) -> Result<SourceDescriptorOutcome, SourceDescriptorError> {
        // Each identity value is checked on its own with the non-aliasing
        // identity rule, then the whole detail once more: a value the
        // redactor would rewrite is content, and a rewritten detail would no
        // longer match the tuple it was hashed from.
        for (_, value) in request.occurrence.identity {
            if identity(value).is_err() {
                return Err(SourceDescriptorError::ContentRefused);
            }
        }
        let payload_id = payload_id(select(encoded.span, request.buffer));
        let detail = detail_for(request, &encoded, &payload_id);
        let detail_json = serde_json::to_string(&detail).map_err(|_| KernelError::InvalidInput)?;
        if !redact(&detail_json)?.detections.is_empty() {
            return Err(SourceDescriptorError::ContentRefused);
        }
        let object_id = descriptor_object_id(&encoded.lineage_id, request.occurrence.revision);
        let predecessor = self.live_lineage(request.domain_id, &detail)?;
        let spec = ObservationSpec {
            observation_id: object_id.clone(),
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
                self.insert_observation(spec)?;
            }
            Some(replaced) => {
                self.correct_observation(replaced, spec)
                    .map_err(|error| match error {
                        KernelError::Conflict => SourceDescriptorError::RevisionNotAdvanced,
                        other => other.into(),
                    })?;
            }
        }
        Ok(SourceDescriptorOutcome {
            object_id,
            occurrence_id: encoded.occurrence_id,
            lineage_id: encoded.lineage_id,
            payload_id,
            replaced_object_id: predecessor,
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
        if redactions != NO_DETECTIONS {
            return Err(SourceDescriptorError::EvidenceNotExact);
        }
        if payload_id(request.buffer.as_bytes()) != request.artifact_digest {
            return Err(SourceDescriptorError::BufferMismatch);
        }
        Ok(())
    }

    /// The live descriptor of this lineage, if any. More than one live row
    /// for one lineage is corruption; a row in another domain refuses the
    /// publication; and stored fields that differ from the fresh ones are a
    /// digest collision, refused rather than folded into the chain.
    fn live_lineage(
        &self,
        domain_id: &str,
        fresh: &SourceDescriptorDetail,
    ) -> Result<Option<String>, SourceDescriptorError> {
        let mut statement = self
            .tx
            .prepare_cached(
                "SELECT o.object_id,o.domain_id,b.observation_payload FROM object_registry o
                 JOIN observations b ON b.object_id=o.object_id
                 WHERE o.object_kind='observation' AND o.source_kind=?1 AND o.source_id=?2
                   AND b.observation_kind=?3 AND o.invalidated_commit_seq IS NULL
                   AND b.invalidated_commit_seq IS NULL
                 LIMIT 2",
            )
            .map_err(map_sqlite)?;
        let live: Vec<(String, String, Vec<u8>)> = statement
            .query_map(
                [
                    fresh.class.as_str(),
                    fresh.lineage_id.as_str(),
                    SOURCE_DESCRIPTOR_KIND,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<_>>()
            .map_err(map_sqlite)?;
        let [(object_id, domain, payload)] = live.as_slice() else {
            return match live.len() {
                0 => Ok(None),
                _ => Err(KernelError::CorruptCanonicalRow.into()),
            };
        };
        let stored: ObservationPayload =
            serde_json::from_slice(payload).map_err(|_| KernelError::CorruptCanonicalRow)?;
        let stored: SourceDescriptorDetail = stored
            .detail
            .as_deref()
            .and_then(|detail| serde_json::from_str(detail).ok())
            .ok_or(KernelError::CorruptCanonicalRow)?;
        if !same_lineage(&stored, fresh) {
            return Err(SourceDescriptorError::LineageCollision);
        }
        if domain != domain_id {
            return Err(SourceDescriptorError::DomainMismatch);
        }
        Ok(Some(object_id.clone()))
    }
}
