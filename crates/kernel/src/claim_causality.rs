//! Causality records bind one live claim decision to its causal evidence.
//! Readers re-derive the causal class from that evidence at any snapshot.
//!
//! `DirectObservation` names exact-retained acquisition evidence by id and digest.
//! `DerivedReinjection` names parent objects at their registered revisions.
//! The write path checks each named fact against the store inside the commit.
//! Request text, roles, and producer strings grant nothing.
//! Generic observation writers reject `CLAIM_CAUSALITY_KIND` as kind or source kind.
//! Generic observation writers reject the `claimcause:` and `claimcauseobj:` ids.
//! Retirement stays open through `retire_observation`: withdrawing lineage
//! yields `Unknown`, which grants nothing.
//! A reader returns `Unknown` when a named fact is missing or malformed.
//! `Unknown` carries no authority.
//! Outbox replay of a record needs no artifact bytes; the class is re-derived
//! from `evidence_meta` rows, so artifact reclamation does not hold for it.

use std::collections::{BTreeSet, HashSet};
use std::num::NonZeroU64;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::envelope::{Envelope, ObjectRow};
use super::redaction::identity;
use super::slice::{ObservationDependencySpec, ObservationPayload, ObservationSpec};
use super::{
    CachedSql, KernelError, KernelStore,
    cas::{ExactEvidence, exact_evidence, is_artifact_digest},
    map_sqlite,
};

pub const CLAIM_CAUSALITY_KIND: &str = "claim_causality";
pub const CLAIM_CAUSALITY_DETAIL_VERSION: u32 = 1;
pub const DERIVED_FROM_DEPENDENCY_KIND: &str = "derived_from";
pub const MAX_DERIVATION_PARENTS: usize = 16;

const OBSERVATION_ID_PREFIX: &str = "claimcause:";
const OBJECT_ID_PREFIX: &str = "claimcauseobj:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalOperation {
    Insert,
    Correct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentReference {
    pub object_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum CausalEvidence {
    DirectObservation {
        acquisition_evidence_id: String,
        artifact_digest: String,
    },
    DerivedReinjection {
        parents: Vec<ParentReference>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimCausalityDetail {
    pub causality_version: u32,
    pub subject_object_id: String,
    pub subject_revision: i64,
    pub operation: CausalOperation,
    pub evidence: CausalEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCausalityRequest<'a> {
    pub subject_object_id: &'a str,
    pub subject_revision: i64,
    pub evidence: CausalEvidence,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCausalityOutcome {
    pub observation_id: String,
    pub object_id: String,
    pub operation: CausalOperation,
    pub replaced_object_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimCausalityError {
    #[error("causality subject is not a live decision")]
    SubjectNotDecision,
    #[error("causality subject revision does not match the registry")]
    SubjectRevisionMismatch,
    #[error("acquisition evidence is not a live artifact")]
    EvidenceMissing,
    #[error("acquisition artifact digest is malformed or does not match the evidence")]
    ArtifactMismatch,
    #[error("acquisition evidence was not retained exactly")]
    EvidenceNotExact,
    #[error("derivation names no parent")]
    NoParents,
    #[error("derivation names more parents than the bound")]
    TooManyParents,
    #[error("derivation names one parent twice")]
    DuplicateParent,
    #[error("derivation names its own subject as a parent")]
    ParentIsSubject,
    #[error("derivation parent is not a live registered object")]
    ParentMissing,
    #[error("derivation parent revision does not match the registry")]
    ParentRevisionMismatch,
    #[error("causality for this subject is already recorded in this commit")]
    DuplicateSubject,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

impl ClaimCausalityError {
    fn poison(&self) -> KernelError {
        match self {
            Self::Kernel(error) => *error,
            _ => KernelError::InvalidInput,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    NoRecord,
    Conflicting,
    Oversized,
    Malformed,
    UnsupportedVersion,
    SubjectMismatch,
    EvidenceUnavailable,
    ParentUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CausalClass {
    DirectObservation {
        acquisition_evidence_id: String,
        artifact_digest: String,
    },
    DerivedReinjection {
        parents: Vec<ParentReference>,
    },
    Unknown(UnknownReason),
}

impl CausalClass {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

/// `producer` is the commit log's producer for the record's commit.
/// `operation` is `None` when the detail did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalRecord {
    pub observation_id: String,
    pub object_id: String,
    pub created_commit_seq: i64,
    pub producer: String,
    pub operation: Option<CausalOperation>,
}

/// `record` is present whenever exactly one live record names the subject at
/// the snapshot, whatever class it yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalReading {
    pub known_as_of: i64,
    pub tip: i64,
    pub class: CausalClass,
    pub record: Option<CausalRecord>,
}

pub(crate) fn uses_causality_namespace(spec: &ObservationSpec) -> bool {
    spec.observation_kind == CLAIM_CAUSALITY_KIND
        || spec.source_kind == CLAIM_CAUSALITY_KIND
        || spec.observation_id.starts_with(OBSERVATION_ID_PREFIX)
        || spec.object_id.starts_with(OBJECT_ID_PREFIX)
}

fn record_object_id(subject: &str, commit_seq: i64) -> String {
    format!("{OBJECT_ID_PREFIX}{subject}:{commit_seq}")
}

impl Envelope<'_> {
    /// The subject must be a live decision at `subject_revision`.
    /// `DirectObservation` cites live exact-retained evidence at `artifact_digest`.
    /// `DerivedReinjection` names 1..=[`MAX_DERIVATION_PARENTS`] distinct live parents.
    /// The operation is derived from the subject's succession, not supplied.
    /// A live record for the same subject is replaced in this commit.
    pub fn record_claim_causality(
        &mut self,
        request: &ClaimCausalityRequest<'_>,
    ) -> Result<ClaimCausalityOutcome, ClaimCausalityError> {
        self.guarded_typed(ClaimCausalityError::poison, |envelope| {
            envelope.record_causality_inner(request)
        })
    }

    fn record_causality_inner(
        &mut self,
        request: &ClaimCausalityRequest<'_>,
    ) -> Result<ClaimCausalityOutcome, ClaimCausalityError> {
        let subject_id = identity(request.subject_object_id)?;
        let subject = self.live_subject(&subject_id)?;
        if subject.object.source_revision != request.subject_revision {
            return Err(ClaimCausalityError::SubjectRevisionMismatch);
        }
        let replaced_predecessor: bool = self
            .tx
            .query_row_cached(
                "SELECT EXISTS(SELECT 1 FROM object_registry WHERE superseded_by=?1)",
                [&subject_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        let operation = if replaced_predecessor {
            CausalOperation::Correct
        } else {
            CausalOperation::Insert
        };
        let (evidence_id, dependencies) = match &request.evidence {
            CausalEvidence::DirectObservation {
                acquisition_evidence_id,
                artifact_digest,
            } => {
                let evidence_id = identity(acquisition_evidence_id)?;
                self.check_acquisition(&evidence_id, artifact_digest)?;
                (Some(evidence_id), Vec::new())
            }
            CausalEvidence::DerivedReinjection { parents } => {
                (None, self.check_parents(&subject_id, parents)?)
            }
        };
        let detail = ClaimCausalityDetail {
            causality_version: CLAIM_CAUSALITY_DETAIL_VERSION,
            subject_object_id: subject_id.clone(),
            subject_revision: request.subject_revision,
            operation,
            evidence: request.evidence.clone(),
        };
        let detail_json = serde_json::to_string(&detail).map_err(|_| KernelError::InvalidInput)?;
        let object_id = record_object_id(&subject_id, self.commit_seq);
        let observation_id = format!("{OBSERVATION_ID_PREFIX}{subject_id}:{}", self.commit_seq);
        let predecessor = self.live_record(&subject_id)?;
        if predecessor.as_deref() == Some(object_id.as_str()) {
            return Err(ClaimCausalityError::DuplicateSubject);
        }
        let spec = ObservationSpec {
            observation_id: observation_id.clone(),
            object_id: object_id.clone(),
            domain_id: subject.object.domain_id.clone(),
            proposition_id: None,
            scope_id: subject.scope_id.clone(),
            anchor_id: None,
            evidence_id,
            observation_kind: CLAIM_CAUSALITY_KIND.to_string(),
            payload: ObservationPayload {
                summary: CLAIM_CAUSALITY_KIND.to_string(),
                classification: match request.evidence {
                    CausalEvidence::DirectObservation { .. } => "direct_observation",
                    CausalEvidence::DerivedReinjection { .. } => "derived_reinjection",
                }
                .to_string(),
                detail: Some(detail_json),
            },
            observed_at: request.observed_at,
            dependencies,
            source_kind: CLAIM_CAUSALITY_KIND.to_string(),
            source_id: subject_id,
            // The commit sequence orders every record for one subject, so a
            // replacement always advances the lineage the slice writer checks.
            source_revision: self.commit_seq,
            sensitivity: subject.object.sensitivity,
        };
        match &predecessor {
            None => {
                self.insert_observation_inner(spec)?;
            }
            Some(replaced) => {
                self.correct_observation_inner(replaced, spec)?;
            }
        }
        Ok(ClaimCausalityOutcome {
            observation_id,
            object_id,
            operation,
            replaced_object_id: predecessor,
        })
    }

    fn live_subject(&self, subject_id: &str) -> Result<SubjectRow, ClaimCausalityError> {
        self.tx
            .query_row_cached(
                "SELECT r.object_id,r.object_kind,r.domain_id,r.source_kind,r.source_id,
                        r.source_revision,r.created_commit_seq,r.invalidated_commit_seq,
                        r.superseded_by,r.sensitivity_class,d.scope_id
                 FROM object_registry r JOIN decisions d ON d.object_id=r.object_id
                 WHERE r.object_id=?1 AND r.object_kind='decision'
                   AND r.invalidated_commit_seq IS NULL AND d.invalidated_commit_seq IS NULL",
                [subject_id],
                |row| {
                    Ok(SubjectRow {
                        object: super::envelope::object_row_from(row)?,
                        scope_id: row.get(10)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)?
            .ok_or(ClaimCausalityError::SubjectNotDecision)
    }

    fn check_acquisition(
        &self,
        evidence_id: &str,
        artifact_digest: &str,
    ) -> Result<(), ClaimCausalityError> {
        if !is_artifact_digest(artifact_digest) {
            return Err(ClaimCausalityError::ArtifactMismatch);
        }
        match exact_evidence(self.tx, evidence_id, artifact_digest, None)? {
            ExactEvidence::Missing => Err(ClaimCausalityError::EvidenceMissing),
            ExactEvidence::DigestMismatch => Err(ClaimCausalityError::ArtifactMismatch),
            ExactEvidence::NotExact => Err(ClaimCausalityError::EvidenceNotExact),
            ExactEvidence::Bound => Ok(()),
        }
    }

    fn check_parents(
        &self,
        subject_id: &str,
        parents: &[ParentReference],
    ) -> Result<Vec<ObservationDependencySpec>, ClaimCausalityError> {
        if parents.is_empty() {
            return Err(ClaimCausalityError::NoParents);
        }
        if parents.len() > MAX_DERIVATION_PARENTS {
            return Err(ClaimCausalityError::TooManyParents);
        }
        let mut seen: HashSet<String> = HashSet::with_capacity(parents.len());
        let mut dependencies = Vec::with_capacity(parents.len());
        for parent in parents {
            let parent_id = identity(&parent.object_id)?;
            if parent_id == subject_id {
                return Err(ClaimCausalityError::ParentIsSubject);
            }
            if !seen.insert(parent_id.clone()) {
                return Err(ClaimCausalityError::DuplicateParent);
            }
            let revision: Option<i64> = self
                .tx
                .query_row_cached(
                    "SELECT source_revision FROM object_registry
                     WHERE object_id=?1 AND invalidated_commit_seq IS NULL",
                    [&parent_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            let revision = revision.ok_or(ClaimCausalityError::ParentMissing)?;
            if revision != parent.revision {
                return Err(ClaimCausalityError::ParentRevisionMismatch);
            }
            dependencies.push(ObservationDependencySpec {
                dependency_object_id: parent_id,
                dependency_kind: DERIVED_FROM_DEPENDENCY_KIND.to_string(),
                dependency_payload: Some(parent.revision.to_string()),
            });
        }
        Ok(dependencies)
    }

    fn live_record(&self, subject_id: &str) -> Result<Option<String>, ClaimCausalityError> {
        let mut statement = self
            .tx
            .prepare_cached(&record_rows_sql("r.invalidated_commit_seq IS NULL"))
            .map_err(map_sqlite)?;
        let live: Vec<String> = statement
            .query_map(
                params![
                    CLAIM_CAUSALITY_KIND,
                    subject_id,
                    format!("{OBJECT_ID_PREFIX}*"),
                    i64::MAX
                ],
                |row| row.get(1),
            )
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<_>>()
            .map_err(map_sqlite)?;
        match live.len() {
            0 => Ok(None),
            1 => Ok(live.into_iter().next()),
            _ => Err(KernelError::CorruptCanonicalRow.into()),
        }
    }
}

struct SubjectRow {
    object: ObjectRow,
    scope_id: Option<String>,
}

/// Writer and reader select records through this one statement so they agree
/// on what a record is: a `claimcauseobj:` registry row of the causality
/// source kind whose observation carries the causality kind.
/// `?1` kind, `?2` subject, `?3` object glob, `?4` snapshot bound.
fn record_rows_sql(liveness: &str) -> String {
    format!(
        "SELECT b.observation_id,r.object_id,r.created_commit_seq,c.producer,
                length(b.observation_payload),b.evidence_id
         FROM object_registry r
         JOIN observations b ON b.object_id=r.object_id
         JOIN commit_log c ON c.commit_seq=r.created_commit_seq
         WHERE r.object_kind='observation' AND r.source_kind=?1 AND r.source_id=?2
           AND r.object_id GLOB ?3 AND b.observation_kind=?1
           AND r.created_commit_seq<=?4 AND {liveness}
         ORDER BY r.created_commit_seq DESC LIMIT 2"
    )
}

struct StoredRecord {
    observation_id: String,
    object_id: String,
    created_commit_seq: i64,
    producer: String,
    payload_bytes: u64,
    evidence_id: Option<String>,
}

impl KernelStore {
    /// Reads the causal class of `object_id` at `requested` from one snapshot.
    /// This call has no deadline for reader acquisition or query execution.
    pub fn causal_class_as_of(
        &self,
        object_id: &str,
        requested: i64,
        max_payload_bytes: NonZeroU64,
    ) -> Result<CausalReading, KernelError> {
        let (tip, (class, record)) = self.read_snapshot(requested, |tx, _| {
            let subject =
                registry_row_at(tx, requested, object_id)?.ok_or(KernelError::NotFound)?;
            causal_class_at(tx, requested, &subject, max_payload_bytes)
        })?;
        Ok(CausalReading {
            known_as_of: requested,
            tip,
            class,
            record,
        })
    }
}

/// The registry row as it stood at `requested`: an invalidation or succession
/// committed after the snapshot is not yet a fact of it.
pub(crate) fn registry_row_at(
    tx: &Transaction<'_>,
    requested: i64,
    object_id: &str,
) -> Result<Option<ObjectRow>, KernelError> {
    let row = tx
        .query_row_cached(
            "SELECT object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                    created_commit_seq,invalidated_commit_seq,superseded_by,sensitivity_class
             FROM object_registry WHERE object_id=?1 AND created_commit_seq<=?2",
            params![object_id, requested],
            super::envelope::object_row_from,
        )
        .optional()
        .map_err(map_sqlite)?;
    Ok(row.map(|mut object| {
        if object
            .invalidated_commit_seq
            .is_some_and(|seq| seq > requested)
        {
            object.invalidated_commit_seq = None;
            object.superseded_by = None;
        }
        object
    }))
}

/// `max_payload_bytes` bounds the stored observation payload (summary,
/// classification, and detail) and is checked before the payload is read.
/// The parent count is checked after the detail decodes; the byte bound is
/// what caps that decode.
pub(crate) fn causal_class_at(
    tx: &Transaction<'_>,
    requested: i64,
    subject: &ObjectRow,
    max_payload_bytes: NonZeroU64,
) -> Result<(CausalClass, Option<CausalRecord>), KernelError> {
    let mut statement = tx
        .prepare_cached(&record_rows_sql(
            "(r.invalidated_commit_seq IS NULL OR ?4<r.invalidated_commit_seq)",
        ))
        .map_err(map_sqlite)?;
    let records: Vec<StoredRecord> = statement
        .query_map(
            params![
                CLAIM_CAUSALITY_KIND,
                subject.object_id,
                format!("{OBJECT_ID_PREFIX}*"),
                requested
            ],
            |row| {
                Ok(StoredRecord {
                    observation_id: row.get(0)?,
                    object_id: row.get(1)?,
                    created_commit_seq: row.get(2)?,
                    producer: row.get(3)?,
                    payload_bytes: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                    evidence_id: row.get(5)?,
                })
            },
        )
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    let record = match records.as_slice() {
        [] => return Ok((CausalClass::Unknown(UnknownReason::NoRecord), None)),
        [record] => record,
        _ => return Ok((CausalClass::Unknown(UnknownReason::Conflicting), None)),
    };
    let mut summary = CausalRecord {
        observation_id: record.observation_id.clone(),
        object_id: record.object_id.clone(),
        created_commit_seq: record.created_commit_seq,
        producer: record.producer.clone(),
        operation: None,
    };
    if record.payload_bytes > max_payload_bytes.get() {
        return Ok((
            CausalClass::Unknown(UnknownReason::Oversized),
            Some(summary),
        ));
    }
    let payload: Vec<u8> = tx
        .query_row_cached(
            "SELECT observation_payload FROM observations WHERE observation_id=?1",
            [&record.observation_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    let detail = match decode_detail(&payload) {
        Ok(detail) => detail,
        Err(reason) => return Ok((CausalClass::Unknown(reason), Some(summary))),
    };
    summary.operation = Some(detail.operation);
    if detail.subject_object_id != subject.object_id
        || detail.subject_revision != subject.source_revision
    {
        return Ok((
            CausalClass::Unknown(UnknownReason::SubjectMismatch),
            Some(summary),
        ));
    }
    // The detail is the one column without an immutability guard, so each
    // class is accepted only where the guarded columns agree with it: the
    // cited `evidence_id` for an acquisition, the `derived_from` rows for a
    // derivation.
    let class = match detail.evidence {
        CausalEvidence::DirectObservation {
            acquisition_evidence_id,
            artifact_digest,
        } => {
            if record.evidence_id.as_deref() != Some(acquisition_evidence_id.as_str()) {
                CausalClass::Unknown(UnknownReason::Malformed)
            } else if exact_evidence(
                tx,
                &acquisition_evidence_id,
                &artifact_digest,
                Some(requested),
            )? == ExactEvidence::Bound
            {
                CausalClass::DirectObservation {
                    acquisition_evidence_id,
                    artifact_digest,
                }
            } else {
                CausalClass::Unknown(UnknownReason::EvidenceUnavailable)
            }
        }
        CausalEvidence::DerivedReinjection { parents } => {
            if parents.is_empty()
                || parents.len() > MAX_DERIVATION_PARENTS
                || !dependencies_match(tx, &record.observation_id, &parents)?
            {
                CausalClass::Unknown(UnknownReason::Malformed)
            } else if parents_existed(tx, requested, &parents)? {
                CausalClass::DerivedReinjection { parents }
            } else {
                CausalClass::Unknown(UnknownReason::ParentUnavailable)
            }
        }
    };
    Ok((class, Some(summary)))
}

fn decode_detail(payload: &[u8]) -> Result<ClaimCausalityDetail, UnknownReason> {
    let stored: ObservationPayload =
        serde_json::from_slice(payload).map_err(|_| UnknownReason::Malformed)?;
    let detail = stored.detail.ok_or(UnknownReason::Malformed)?;
    // The version is read before the rest so a newer layout reports itself as
    // unsupported rather than as malformed.
    #[derive(Deserialize)]
    struct Version {
        causality_version: i64,
    }
    let version: Version = serde_json::from_str(&detail).map_err(|_| UnknownReason::Malformed)?;
    if version.causality_version != i64::from(CLAIM_CAUSALITY_DETAIL_VERSION) {
        return Err(UnknownReason::UnsupportedVersion);
    }
    serde_json::from_str(&detail).map_err(|_| UnknownReason::Malformed)
}

/// The `derived_from` rows written with the record must name exactly the
/// parents the detail names, revision included.
fn dependencies_match(
    tx: &Transaction<'_>,
    observation_id: &str,
    parents: &[ParentReference],
) -> Result<bool, KernelError> {
    let mut statement = tx
        .prepare_cached(
            "SELECT dependency_object_id,dependency_payload FROM observation_dependencies
             WHERE observation_id=?1 AND dependency_kind=?2",
        )
        .map_err(map_sqlite)?;
    let stored: BTreeSet<(String, Option<String>)> = statement
        .query_map(
            params![observation_id, DERIVED_FROM_DEPENDENCY_KIND],
            |row| {
                let payload: Option<Vec<u8>> = row.get(1)?;
                Ok((
                    row.get(0)?,
                    payload.and_then(|bytes| String::from_utf8(bytes).ok()),
                ))
            },
        )
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<_>>()
        .map_err(map_sqlite)?;
    let named: BTreeSet<(String, Option<String>)> = parents
        .iter()
        .map(|parent| (parent.object_id.clone(), Some(parent.revision.to_string())))
        .collect();
    Ok(stored == named)
}

// A parent invalidated after the derivation was observed does not erase the
// lineage; a parent that never existed, or held another revision, does.
// Acquisition evidence differs: it must stay live, because the class is a
// standing statement about retained bytes, not about a past event.
fn parents_existed(
    tx: &Transaction<'_>,
    requested: i64,
    parents: &[ParentReference],
) -> Result<bool, KernelError> {
    for parent in parents {
        let revision: Option<i64> = tx
            .query_row_cached(
                "SELECT source_revision FROM object_registry
                 WHERE object_id=?1 AND created_commit_seq<=?2",
                params![parent.object_id, requested],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if revision != Some(parent.revision) {
            return Ok(false);
        }
    }
    Ok(true)
}
