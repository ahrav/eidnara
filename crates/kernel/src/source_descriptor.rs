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
    cas::{ExactEvidence, MAX_TEXT_FIELD_BYTES, exact_evidence, is_artifact_digest},
    map_sqlite,
};

/// The observation kind every descriptor row carries.
pub const SOURCE_DESCRIPTOR_KIND: &str = "source_descriptor";
/// Version of the JSON detail a descriptor row stores.
pub const SOURCE_DESCRIPTOR_DETAIL_VERSION: u32 = 1;
/// Descriptors one commit may publish, so a batch's verification and row
/// writes stay bounded under the single writer.
pub const MAX_DESCRIPTORS_PER_COMMIT: usize = 1024;

pub(crate) const OCCURRENCE_ID_PREFIX: &str = "srcocc:";
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
    /// The highest revision the caller observed in this lineage, retired rows included; `None` when it observed no revision. A `repository_documents` publication commits only while the lineage's historical head still equals it, and its revision starts at 1 and exceeds that head. The producer keeps revisions contiguous by proposing the head plus one. Every other class passes `None`; a value is invalid input.
    pub expected_head: Option<i64>,
    /// How the tool call behind a `raw_tool_spans` occurrence settled ([`check_outcome`]). Only that class carries one; it is stored on the descriptor beside the identity, so equal bytes from calls that settled differently are two descriptors citing one artifact. The producer decides whether a tool descriptor carries a record: rows published before the record existed carry none.
    pub outcome: Option<&'a ToolOutcome>,
}

/// Version of the [`ToolOutcome`] record a tool descriptor stores. The record decodes strictly inside the descriptor detail, so a new record version is a new [`SOURCE_DESCRIPTOR_DETAIL_VERSION`].
pub const TOOL_OUTCOME_VERSION: u32 = 1;

/// How a tool call settled, as its harness reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Completed,
    Error,
}

/// The execution-outcome record of one tool occurrence: what the harness reported about the call, never inferred. `truncated`, `exit_code`, and `exception_ref` are present only when the harness reported them; an absent value is unknown, never a default. The stored JSON is at most [`MAX_TEXT_FIELD_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolOutcome {
    pub version: u32,
    pub status: ToolStatus,
    pub tool_name: String,
    /// The UTF-8 byte length of the occurrence's selected bytes.
    pub byte_length: u64,
    /// Whether the harness truncated the output it returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// The name the harness gave the exception that ended the assistant turn holding the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_ref: Option<String>,
}

impl ToolOutcome {
    /// The record's stored JSON bytes, the bytes a request digest binds.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a tool outcome serializes")
    }

    /// Checks the record against the occurrence it describes: the current version, a status that agrees with `representation` (`error` exactly for `tool_error`), a tool name and exception name that are well-formed identity values the redactor leaves unchanged, a byte length equal to `selected_len`, and stored JSON of at most [`MAX_TEXT_FIELD_BYTES`].
    fn validate(&self, representation: &str, selected_len: usize) -> bool {
        let name_ok = |name: &str| well_formed_value(name) && identity(name).is_ok();
        self.version == TOOL_OUTCOME_VERSION
            && (self.status == ToolStatus::Error) == (representation == "tool_error")
            && name_ok(&self.tool_name)
            && self.exception_ref.as_deref().is_none_or(name_ok)
            && u64::try_from(selected_len).is_ok_and(|len| len == self.byte_length)
            && self.encode().len() <= MAX_TEXT_FIELD_BYTES
    }
}

/// Judges the outcome record a descriptor of `class` and `representation` would carry over `selected_len` selected bytes: only a `raw_tool_spans` occurrence carries one, and it must describe that occurrence ([`ToolOutcome`]). The descriptor commit applies this check, and a producer applies it before retaining any byte.
///
/// # Errors
///
/// Returns [`SourceDescriptorError::OutcomeRefused`] for a record on another class or one that fails its checks.
pub fn check_outcome(
    outcome: Option<&ToolOutcome>,
    class: OccurrenceClass,
    representation: &str,
    selected_len: usize,
) -> Result<(), SourceDescriptorError> {
    match outcome {
        Some(outcome)
            if class != OccurrenceClass::RawToolSpans
                || !outcome.validate(representation, selected_len) =>
        {
            Err(SourceDescriptorError::OutcomeRefused)
        }
        _ => Ok(()),
    }
}

pub(crate) fn stored_outcome_ok(
    detail: &SourceDescriptorDetail,
    class: OccurrenceClass,
    byte_length: u64,
) -> bool {
    let selected = match detail.span {
        None => byte_length,
        Some((start, end)) => end.saturating_sub(start),
    };
    usize::try_from(selected).is_ok_and(|selected| {
        check_outcome(
            detail.outcome.as_ref(),
            class,
            &detail.representation,
            selected,
        )
        .is_ok()
    })
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
    /// The execution-outcome record of a `raw_tool_spans` descriptor that carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ToolOutcome>,
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
    /// The lineage's historical head differs from the head the caller expected. The caller reads the head again ([`KernelStore::source_lineage_head`]) before proposing another revision.
    #[error("descriptor lineage head differs from the expected head")]
    HeadMismatch,
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
    /// The execution-outcome record is malformed, describes other bytes, exceeds [`MAX_TEXT_FIELD_BYTES`], or rides a class other than `raw_tool_spans`.
    #[error("descriptor outcome record is refused")]
    OutcomeRefused,
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
        outcome: request.outcome.cloned(),
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

impl SourceDescriptorDetail {
    /// Whether the stored identity re-encodes to itself and names `object_id`. Publication derived every field from one encoding, so `false` is corruption of the stored row, and every Kernel read refuses such a row; a reader outside the Kernel applies the same test before trusting a span or tuple.
    pub fn is_consistent_with(&self, object_id: &str) -> bool {
        reencoded_identity(self).is_some()
            && descriptor_object_id(&self.lineage_id, &self.revision) == object_id
    }
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
        self.check_reserved_ownership(DESCRIPTOR_OBJECT_ID_PREFIX, &self.descriptor_objects)
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
        check_outcome(
            request.outcome,
            encoded.class,
            request.occurrence.representation,
            select(encoded.span, request.buffer).len(),
        )?;
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
        let guarded = encoded.class == OccurrenceClass::RepositoryDocuments;
        if !guarded && request.expected_head.is_some() {
            return Err(KernelError::InvalidInput.into());
        }
        let predecessor = self.live_lineage(request.domain_id, &detail)?;
        if let Some((_, live_revision)) = &predecessor
            && encoded.revision <= *live_revision
        {
            return Err(SourceDescriptorError::RevisionNotAdvanced);
        }
        // A repository document's revision counts the observations of its path from 1, so it must exceed every revision the lineage ever held, retired rows included, and the caller's view of that head must still hold.
        if guarded {
            let head = historical_head(self.tx, &encoded.lineage_id)?;
            if encoded.revision < 1 || head.is_some_and(|head| encoded.revision <= head) {
                return Err(SourceDescriptorError::RevisionNotAdvanced);
            }
            if head != request.expected_head {
                return Err(SourceDescriptorError::HeadMismatch);
            }
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
        match exact_evidence(self.tx, request.evidence_id, request.artifact_digest, None)? {
            ExactEvidence::Missing => Err(SourceDescriptorError::EvidenceMissing),
            ExactEvidence::DigestMismatch => Err(SourceDescriptorError::ArtifactMismatch),
            ExactEvidence::NotExact => Err(SourceDescriptorError::EvidenceNotExact),
            ExactEvidence::Bound => Ok(()),
        }
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
        let lineage_low = descriptor_object_id(&fresh.lineage_id, "");
        let lineage_high = super::envelope::prefix_upper_bound(&lineage_low);
        let mut statement = self
            .tx
            .prepare_cached(
                "SELECT o.object_id,o.domain_id,o.source_revision,b.observation_payload
                 FROM object_registry o
                 JOIN observations b ON b.object_id=o.object_id
                 WHERE o.object_kind='observation' AND o.source_id=?1 AND o.object_id>=?2 AND o.object_id<?4
                   AND b.observation_kind=?3 AND o.invalidated_commit_seq IS NULL
                   AND b.invalidated_commit_seq IS NULL
                 LIMIT 2",
            )
            .map_err(map_sqlite)?;
        let live: Vec<(String, String, i64, Vec<u8>)> = statement
            .query_map(
                [
                    fresh.lineage_id.as_str(),
                    lineage_low.as_str(),
                    SOURCE_DESCRIPTOR_KIND,
                    lineage_high.as_str(),
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
                     WHERE o.object_kind='observation' AND o.source_id=?1 AND o.object_id>=?2 AND o.object_id<?4
                       AND b.observation_kind=?3
                     ORDER BY o.source_revision DESC LIMIT 1",
                    [
                        fresh.lineage_id.as_str(),
                        lineage_low.as_str(),
                        SOURCE_DESCRIPTOR_KIND,
                        lineage_high.as_str(),
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

/// The highest revision any descriptor of `lineage_id` was published at, live or retired.
fn historical_head(
    tx: &rusqlite::Transaction<'_>,
    lineage_id: &str,
) -> Result<Option<i64>, KernelError> {
    let lineage_low = descriptor_object_id(lineage_id, "");
    let lineage_high = super::envelope::prefix_upper_bound(&lineage_low);
    tx.query_row_cached(
        "SELECT MAX(o.source_revision)
         FROM object_registry o
         JOIN observations b ON b.object_id=o.object_id
         WHERE o.object_kind='observation' AND o.source_id=?1 AND o.object_id>=?2 AND o.object_id<?4
           AND b.observation_kind=?3",
        [
            lineage_id,
            lineage_low.as_str(),
            SOURCE_DESCRIPTOR_KIND,
            lineage_high.as_str(),
        ],
        |row| row.get(0),
    )
    .map_err(map_sqlite)
}

impl KernelStore {
    /// The historical head of `lineage_id`: the highest revision any of its descriptors was published at, retired rows included, or `None` for a lineage with no descriptor. A `repository_documents` publisher proposes the head plus one and names this head as the expected one.
    ///
    /// # Errors
    ///
    /// Returns a store failure from the read.
    pub fn source_lineage_head(&self, lineage_id: &str) -> Result<Option<i64>, KernelError> {
        self.read_snapshot(0, |tx, _| historical_head(tx, lineage_id))
            .map(|(_, head)| head)
    }
}

/// One page of live descriptors of one class at a fixed sequence, in object id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDescriptorPage {
    pub rows: Vec<LiveDescriptor>,
    /// The object id to continue after, or `None` when this page ends the inventory. A narrowed page ([`Envelope::live_source_descriptors_with`]) can hold fewer rows than its bound, none included, while this names a later page.
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDescriptor {
    pub object_id: String,
    pub domain_id: String,
    pub sensitivity: Sensitivity,
    pub created_commit_seq: i64,
    pub detail: SourceDescriptorDetail,
}

struct LiveDescriptorRaw {
    object_id: String,
    domain_id: String,
    sensitivity: String,
    registry_sensitivity: String,
    created_commit_seq: i64,
    payload: Vec<u8>,
    revision: i64,
    source_id: String,
    invalidated_commit_seq: Option<i64>,
    observation_created: i64,
    observation_invalidated: Option<i64>,
    evidence_id: String,
    artifact_digest: String,
    byte_length: i64,
}

impl KernelStore {
    /// The descriptors of `class` live at `requested`, keyset-paged by object id from `after`. The page uses the export's liveness predicate, so a descriptor whose cited evidence was deleted is absent here as it is from every export snapshot. A row whose stored identity does not re-encode to itself, or whose lineage and revision do not name its own object id, is refused rather than handed to a caller that may retire it. The wait for a pooled reader stops at `budget`'s deadline or interrupt.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Deadline`] when no reader frees before the budget runs out, [`KernelError::InvalidInput`] for a negative sequence, [`KernelError::FutureSnapshot`] when `requested` exceeds the tip, and [`KernelError::CorruptCanonicalRow`] when a stored descriptor does not decode or re-encode to its stored identity.
    pub fn live_source_descriptors(
        &self,
        class: OccurrenceClass,
        requested: i64,
        after: Option<&str>,
        max_rows: std::num::NonZeroUsize,
        budget: &crate::applicability::EvalBudget,
    ) -> Result<LiveDescriptorPage, KernelError> {
        let mut reader = self.lock_reader_within(&budget.acquire_limit())?;
        let tx = reader
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        crate::slice::snapshot_tip(&tx, requested)?;
        let bound = max_rows.get();
        let mut raw = descriptor_rows(
            &tx,
            class,
            requested,
            "o.object_id>?3",
            after.unwrap_or(""),
            bound.saturating_add(1),
        )?;
        let next = (raw.len() > bound).then(|| raw[bound - 1].object_id.clone());
        raw.truncate(bound);
        let rows = raw
            .into_iter()
            .map(|raw| checked(raw, class))
            .collect::<Result<_, _>>()?;
        Ok(LiveDescriptorPage { rows, next })
    }
}

/// `raw` as a live descriptor of `class`. A row whose registry row and stored detail disagree, as `source_export::preflight` requires them to agree, or whose stored identity does not re-encode to itself is [`KernelError::CorruptCanonicalRow`].
fn checked(raw: LiveDescriptorRaw, class: OccurrenceClass) -> Result<LiveDescriptor, KernelError> {
    let detail = stored_detail(&raw.payload)?;
    let byte_length =
        u64::try_from(raw.byte_length).map_err(|_| KernelError::CorruptCanonicalRow)?;
    if raw.created_commit_seq != raw.observation_created
        || raw.invalidated_commit_seq != raw.observation_invalidated
        || raw.sensitivity != raw.registry_sensitivity
        || detail.lineage_id != raw.source_id
        || detail.evidence_id != raw.evidence_id
        || detail.artifact_digest != raw.artifact_digest
        || detail.class != class.code()
        || detail.revision != raw.revision.to_string()
        || descriptor_object_id(&detail.lineage_id, &detail.revision) != raw.object_id
        || reencoded_identity(&detail).is_none()
        || !stored_outcome_ok(&detail, class, byte_length)
    {
        return Err(KernelError::CorruptCanonicalRow);
    }
    Ok(LiveDescriptor {
        object_id: raw.object_id,
        domain_id: raw.domain_id,
        sensitivity: Sensitivity::from_stored(&raw.sensitivity),
        created_commit_seq: raw.created_commit_seq,
        detail,
    })
}

/// Up to `limit` stored descriptor rows of `class` live at `requested` whose object id satisfies `key` over `?3` bound to `value`, in object id order. `key` is `o.object_id>?3` for an inventory page or `o.object_id IN (SELECT value FROM json_each(?3))` for a narrowed page's candidates.
fn descriptor_rows(
    tx: &rusqlite::Transaction<'_>,
    class: OccurrenceClass,
    requested: i64,
    key: &str,
    value: &str,
    limit: usize,
) -> Result<Vec<LiveDescriptorRaw>, KernelError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    // The bound is written into the statement text, so each bound has its own cached statement.
    let sql = format!(
        "SELECT o.object_id,o.domain_id,b.sensitivity_class,o.sensitivity_class,o.created_commit_seq,b.observation_payload,o.source_revision,o.source_id,
                o.invalidated_commit_seq,b.created_commit_seq,b.invalidated_commit_seq,e.evidence_id,e.artifact_digest,e.byte_length
         {rows}
           AND o.source_kind=?1 AND o.object_kind='observation'
           AND {live}
           AND {key}
         ORDER BY o.object_id
         LIMIT {limit}",
        rows = descriptor_rows_sql("idx_objects_source_descriptor_page"),
        live = Descriptors::LiveAtEnd.predicate("?2", "0"),
    );
    let mut statement = tx.prepare_cached(&sql).map_err(map_sqlite)?;
    statement
        .query_map(rusqlite::params![class.code(), requested, value], |row| {
            Ok(LiveDescriptorRaw {
                object_id: row.get(0)?,
                domain_id: row.get(1)?,
                sensitivity: row.get(2)?,
                registry_sensitivity: row.get(3)?,
                created_commit_seq: row.get(4)?,
                payload: row.get(5)?,
                revision: row.get(6)?,
                source_id: row.get(7)?,
                invalidated_commit_seq: row.get(8)?,
                observation_created: row.get(9)?,
                observation_invalidated: row.get(10)?,
                evidence_id: row.get(11)?,
                artifact_digest: row.get(12)?,
                byte_length: row.get(13)?,
            })
        })
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)
}

impl Envelope<'_> {
    /// The descriptors of `class` live in this envelope's view whose identity field `field` holds one of `values`, keyset-paged by object id from `after`. The view is the committed rows and the rows this commit wrote, less the rows it retired, so a writer can read the matching descriptors and act on them in one transaction. Every returned row passes [`KernelStore::live_source_descriptors`]' liveness predicate and integrity checks, and its decoded identity holds one of `values` at the field's position in the class's field order. A page examines at most `max_rows` live descriptors of the class, matching or not, so one call's work is bounded and a caller can stop between pages. A page can hold fewer rows than `max_rows`, none included, while `next` names a later page, so a caller reads until `next` is `None`. An empty `values` examines no descriptor: its page is empty and ends the inventory.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] when `field` is not an identity field of `class`, [`KernelError::CorruptCanonicalRow`] when a stored descriptor does not decode or re-encode to its stored identity, and storage errors.
    pub fn live_source_descriptors_with(
        &self,
        class: OccurrenceClass,
        field: &str,
        values: &[&str],
        after: Option<&str>,
        max_rows: std::num::NonZeroUsize,
    ) -> Result<LiveDescriptorPage, KernelError> {
        let position = class
            .identity_fields()
            .iter()
            .position(|name| *name == field)
            .ok_or(KernelError::InvalidInput)?;
        if values.is_empty() {
            return Ok(LiveDescriptorPage {
                rows: Vec::new(),
                next: None,
            });
        }
        Narrowing::new(position, field, values).page(
            self.tx,
            class,
            self.commit_seq,
            after,
            max_rows,
        )
    }
}

/// An identity value set a page matches: an identity field's position and name, the values, and each value's `[field, value]` pair as a stored payload spells it.
///
/// The descriptor writer serializes the detail with `serde_json` and stores that text as a JSON string inside the payload, so a pair appears in the payload escaped twice. `serde_json` escapes each character on its own, so the twice-escaped pair is a substring of every payload whose identity holds the pair. [`Narrowing::may_hold`] therefore keeps every row that holds a value, and [`Narrowing::holds`] decides each kept row exactly from its decoded detail.
struct Narrowing<'v> {
    position: usize,
    field: &'v str,
    values: &'v [&'v str],
    /// Finds the bytes every pair starts with: the twice-escaped `["field",`.
    lead: memchr::memmem::Finder<'static>,
    /// Each value's twice-escaped `["field","value"]`.
    pairs: Vec<Vec<u8>>,
}

impl<'v> Narrowing<'v> {
    fn new(position: usize, field: &'v str, values: &'v [&'v str]) -> Self {
        // The bytes `text` takes inside a stored JSON string: its JSON string form without the quotes.
        let stored = |text: &str| {
            let quoted = serde_json::to_string(text).expect("a string serializes");
            quoted.as_bytes()[1..quoted.len() - 1].to_vec()
        };
        let encoded_field = serde_json::to_string(field).expect("a string serializes");
        let pairs = values
            .iter()
            .map(|value| {
                stored(
                    &serde_json::to_string(&(field, value)).expect("a pair of strings serializes"),
                )
            })
            .collect();
        Self {
            position,
            field,
            values,
            lead: memchr::memmem::Finder::new(&stored(&format!("[{encoded_field},"))).into_owned(),
            pairs,
        }
    }

    /// One page of the descriptors of `class` live at `requested` on `tx` whose identity holds one of the values. The candidate scan reads each registry row and its payload; the page then reads the candidates' full rows, with their evidence.
    fn page(
        &self,
        tx: &rusqlite::Transaction<'_>,
        class: OccurrenceClass,
        requested: i64,
        after: Option<&str>,
        max_rows: std::num::NonZeroUsize,
    ) -> Result<LiveDescriptorPage, KernelError> {
        let bound = max_rows.get();
        let (candidates, next) = self.candidates(tx, class, requested, after, bound)?;
        if candidates.is_empty() {
            return Ok(LiveDescriptorPage {
                rows: Vec::new(),
                next,
            });
        }
        let ids = serde_json::to_string(&candidates).expect("strings serialize");
        let mut rows = Vec::with_capacity(candidates.len());
        for raw in descriptor_rows(
            tx,
            class,
            requested,
            "o.object_id IN (SELECT value FROM json_each(?3))",
            &ids,
            bound,
        )? {
            let row = checked(raw, class)?;
            if self.holds(&row.detail) {
                rows.push(row);
            }
        }
        Ok(LiveDescriptorPage { rows, next })
    }

    /// `candidates` returns object ids whose payloads may hold a value from the next `examine` live descriptors after `after`, plus the last examined object id if another live descriptor follows.
    fn candidates(
        &self,
        tx: &rusqlite::Transaction<'_>,
        class: OccurrenceClass,
        requested: i64,
        after: Option<&str>,
        examine: usize,
    ) -> Result<(Vec<String>, Option<String>), KernelError> {
        let sql = format!(
            "SELECT o.object_id,b.observation_payload
             FROM object_registry o INDEXED BY idx_objects_source_descriptor_page
             CROSS JOIN observations b ON b.object_id=o.object_id
             WHERE o.object_id GLOB 'srcdesc:*'
               AND b.observation_kind='{SOURCE_DESCRIPTOR_KIND}'
               AND o.source_kind=?1 AND o.object_kind='observation'
               AND {live}
               AND o.object_id>?3
             ORDER BY o.object_id",
            live = Descriptors::LiveAtEnd.registry_predicate("?2", "0"),
        );
        let mut statement = tx.prepare_cached(&sql).map_err(map_sqlite)?;
        let mut stored = statement
            .query(rusqlite::params![
                class.code(),
                requested,
                after.unwrap_or("")
            ])
            .map_err(map_sqlite)?;
        let mut candidates = Vec::new();
        let mut examined = 0;
        let mut boundary = None;
        while let Some(row) = stored.next().map_err(map_sqlite)? {
            if boundary.is_some() {
                return Ok((candidates, boundary));
            }
            examined += 1;
            let payload = row
                .get_ref(1)
                .map_err(map_sqlite)?
                .as_blob()
                .map_err(|_| KernelError::CorruptCanonicalRow)?;
            let held = self.may_hold(payload);
            if held || examined == examine {
                let object_id = row.get::<_, String>(0).map_err(map_sqlite)?;
                if examined == examine {
                    boundary = Some(object_id.clone());
                }
                if held {
                    candidates.push(object_id);
                }
            }
        }
        Ok((candidates, None))
    }

    /// Whether `payload` may hold one of the values: it contains one of the pairs. True for every payload whose identity holds one.
    fn may_hold(&self, payload: &[u8]) -> bool {
        self.lead.find_iter(payload).any(|at| {
            self.pairs
                .iter()
                .any(|pair| payload[at..].starts_with(pair))
        })
    }

    /// Whether the decoded identity holds one of the values in the field's position.
    fn holds(&self, detail: &SourceDescriptorDetail) -> bool {
        detail
            .identity
            .get(self.position)
            .is_some_and(|(name, value)| {
                name == self.field && self.values.contains(&value.as_str())
            })
    }
}

#[cfg(test)]
mod narrowing_tests {
    use super::*;

    fn detail(identity: &[(&str, &str)]) -> SourceDescriptorDetail {
        SourceDescriptorDetail {
            descriptor_version: SOURCE_DESCRIPTOR_DETAIL_VERSION,
            source_policy: SourceDescriptorPolicy::Native,
            class: OccurrenceClass::RawToolSpans.code().to_owned(),
            identity: identity
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            revision: "1".to_owned(),
            representation: "tool_output".to_owned(),
            span: None,
            occurrence_id: String::new(),
            occurrence_tuple: Vec::new(),
            lineage_id: String::new(),
            payload_id: String::new(),
            artifact_digest: String::new(),
            evidence_id: String::new(),
            outcome: None,
        }
    }

    /// `may_hold` admits a payload that spells a wanted pair outside its identity, and `holds` decides each admitted row from the decoded identity, by the field's name and position.
    #[test]
    fn holds_decides_each_row_may_hold_admits() {
        let values = ["call-b"];
        let narrowing = Narrowing::new(4, "tool_call_id", &values);
        let pair = serde_json::to_string(&("tool_call_id", "call-b")).unwrap();
        let payload = serde_json::to_vec(&serde_json::json!({ "summary": pair })).unwrap();
        assert!(narrowing.may_hold(&payload));
        assert!(!narrowing.may_hold(br#"{"summary":"call-b"}"#));
        let head = [("project_id", "p"), ("harness", "pi"), ("session_id", "s")];
        let identity = |rest: [(&'static str, &'static str); 2]| {
            let mut fields = head.to_vec();
            fields.extend(rest);
            detail(&fields)
        };
        assert!(narrowing.holds(&identity([
            ("parent_message_id", "m"),
            ("tool_call_id", "call-b")
        ])));
        assert!(!narrowing.holds(&identity([
            ("parent_message_id", "m"),
            ("tool_call_id", "call-other")
        ])));
        assert!(
            !narrowing.holds(&identity([
                ("tool_call_id", "call-b"),
                ("parent_message_id", "m")
            ])),
            "the value in another position"
        );
        assert!(
            !narrowing.holds(&identity([
                ("tool_call_id", "m"),
                ("parent_message_id", "call-b")
            ])),
            "the value under another field"
        );
    }
}
