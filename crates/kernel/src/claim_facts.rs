//! A bounded, snapshot-bound read of every canonical fact a claim decision
//! carries: registry row, typed decision row, raw own and lineage admission
//! rows, the served interpretation per surface, the occurrence inventory the
//! kernel encodes for it, and its causal class.
//!
//! Stored values are copied, never re-evaluated. The own and lineage admission
//! rows are selected by the serving view's own SQL, the supporting approval is
//! the one the row names with the authority chain's state at the snapshot, and
//! served visibility comes from the serving query the read routes use. The
//! revision domains stay separate fields: `object.source_revision`, each
//! admission row's `policy_revision`, and the snapshot's `known_as_of`.
//! A required field that fails to decode is an error, not a default.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::LazyLock;

use rusqlite::{OptionalExtension, Transaction, params};

use super::admission::{
    Disposition, EventKind, Maturity, Outcome, ServedRow, SourceClass, Surface, SurfaceVisibility,
    TaintClass, VisibilityRow, served_classes, served_lineage_decision_sql,
    served_own_decision_sql, supporting_approval_valid_sql,
};
use super::claim_causality::{CausalClass, CausalRecord, causal_class_at, registry_row_at};
use super::envelope::{ObjectRow, Sensitivity};
use super::source_descriptor::{
    SOURCE_DESCRIPTOR_KIND, descriptor_object_id, reencoded_identity, stored_detail,
};
use super::source_hold::Descriptors;
use super::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use super::{CachedSql, KernelError, KernelStore, map_sqlite};

pub const MAX_CLAIM_OBJECT_ID_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimFactBounds {
    pub max_claims: NonZeroUsize,
    pub max_causal_payload_bytes: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimFactsError {
    #[error("claim facts request names more claims than the bound")]
    TooManyClaims,
    #[error("claim facts request names one claim twice")]
    DuplicateClaim,
    #[error("claim facts request names an object that is not a decision")]
    NotADecision,
    #[error("claim facts row holds a required field this build cannot interpret")]
    MalformedRequiredField,
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportingApproval {
    pub object_id: String,
    pub valid_at_snapshot: bool,
}

/// One stored admission row, copied as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionFacts {
    pub admission_decision_id: String,
    pub commit_seq: i64,
    pub event_kind: EventKind,
    pub historical_maturity: Maturity,
    pub effective_maturity: Maturity,
    pub disposition: Disposition,
    pub visibility: VisibilityRow,
    pub outcome: Outcome,
    pub source_class: SourceClass,
    pub taint_class: TaintClass,
    pub policy_revision: i64,
    pub sensitivity: Sensitivity,
    pub evidence_id: Option<String>,
    pub supporting_approval: Option<SupportingApproval>,
    pub trigger_object_id: Option<String>,
    pub elevated_support: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDecisionFacts {
    pub decision_id: String,
    pub decision_kind: String,
    pub proposition_id: Option<String>,
    pub scope_id: Option<String>,
    pub anchor_id: Option<String>,
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedFacts {
    pub sensitivity: Sensitivity,
    pub auto_inject: SurfaceVisibility,
    pub auto_search: SurfaceVisibility,
    pub explicit_search: SurfaceVisibility,
}

/// Why the serving view has no row for a claim, or the row it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServedStanding {
    Served(ServedFacts),
    /// The registry row was invalidated at or before the snapshot.
    NotLiveAtSnapshot,
    /// No own admission row has been committed for the object.
    NeverAdmitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOccurrence {
    pub class: OccurrenceClass,
    pub representation: &'static str,
    pub descriptor_object_id: String,
    pub occurrence_id: String,
    pub lineage_id: String,
    pub payload_id: String,
    pub artifact_digest: String,
    pub evidence_id: String,
    pub descriptor_commit_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepresentationExclusion {
    /// No descriptor for the representation is live at the snapshot with live
    /// evidence, the same rule the export applies.
    NoDescriptor,
    MalformedIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedRepresentation {
    pub class: OccurrenceClass,
    pub representation: &'static str,
    pub reason: RepresentationExclusion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFacts {
    pub object: ObjectRow,
    pub decision: ClaimDecisionFacts,
    pub own_admission: Option<AdmissionFacts>,
    pub lineage_admission: Option<AdmissionFacts>,
    pub served: ServedStanding,
    pub occurrences: Vec<ClaimOccurrence>,
    pub excluded_representations: Vec<ExcludedRepresentation>,
    pub causality: CausalClass,
    pub causal_record: Option<CausalRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFactsSnapshot {
    pub known_as_of: i64,
    pub tip: i64,
    pub claims: Vec<ClaimFacts>,
    /// Requested ids with no registry row created by the snapshot.
    pub missing: Vec<String>,
}

impl KernelStore {
    /// Reads every claim in `object_ids` at `requested` from one snapshot.
    /// `claims` follows the request order; `missing` keeps ids with no registry
    /// row by the snapshot.
    ///
    /// # Errors
    ///
    /// `TooManyClaims` and `DuplicateClaim` before any row is read;
    /// `NotADecision` when a named object is registered as another kind;
    /// `MalformedRequiredField` when an admission or decision column holds a
    /// value this build cannot interpret. Kernel errors pass through:
    /// `InvalidInput` for a negative sequence, `FutureSnapshot` past the tip,
    /// `CorruptCanonicalRow` when a decision or descriptor row disagrees with
    /// its registry row, and `Busy`, `Deadline`, or `Io` from the reader.
    pub fn claim_facts_as_of(
        &self,
        object_ids: &[String],
        requested: i64,
        bounds: ClaimFactBounds,
    ) -> Result<ClaimFactsSnapshot, ClaimFactsError> {
        if object_ids.len() > bounds.max_claims.get() {
            return Err(ClaimFactsError::TooManyClaims);
        }
        if object_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > MAX_CLAIM_OBJECT_ID_BYTES)
        {
            return Err(KernelError::InvalidInput.into());
        }
        let mut distinct = HashSet::with_capacity(object_ids.len());
        if !object_ids.iter().all(|id| distinct.insert(id.as_str())) {
            return Err(ClaimFactsError::DuplicateClaim);
        }
        let (tip, loaded) = self.read_snapshot(requested, |tx, _| {
            let mut claims = Vec::with_capacity(object_ids.len());
            let mut missing = Vec::new();
            let mut served = load_served(tx, requested, object_ids)?;
            for object_id in object_ids {
                match registry_row_at(tx, requested, object_id)? {
                    None => missing.push(object_id.clone()),
                    Some(object) => {
                        // A `load_claim` error aborts the request before later `object_ids` are read.
                        match load_claim(tx, requested, object, served.remove(object_id), bounds) {
                            Ok(claim) => claims.push(claim),
                            Err(error) => return Ok(Err(error)),
                        }
                    }
                }
            }
            Ok(Ok((claims, missing)))
        })?;
        let (claims, missing) = loaded?;
        Ok(ClaimFactsSnapshot {
            known_as_of: requested,
            tip,
            claims,
            missing,
        })
    }
}

fn load_served(
    tx: &Transaction<'_>,
    requested: i64,
    object_ids: &[String],
) -> Result<HashMap<String, ServedRow>, KernelError> {
    if object_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = serde_json::to_string(object_ids).map_err(|_| KernelError::InvalidInput)?;
    Ok(served_classes(tx, requested, Some(&ids), None)?
        .into_iter()
        .map(|row| (row.object.object_id.clone(), row))
        .collect())
}

fn load_claim(
    tx: &Transaction<'_>,
    requested: i64,
    object: ObjectRow,
    served: Option<ServedRow>,
    bounds: ClaimFactBounds,
) -> Result<ClaimFacts, ClaimFactsError> {
    if object.object_kind != "decision" {
        return Err(ClaimFactsError::NotADecision);
    }
    let decision = load_decision(tx, &object)?;
    let own_admission = load_admission(tx, requested, &object, AdmissionScope::Own)?;
    let lineage_admission = load_admission(tx, requested, &object, AdmissionScope::Lineage)?;
    let served = match served {
        Some(row) => ServedStanding::Served(ServedFacts {
            sensitivity: row.object.sensitivity,
            auto_inject: row.visibility(Surface::AutoInject),
            auto_search: row.visibility(Surface::AutoSearch),
            explicit_search: row.visibility(Surface::ExplicitSearch),
        }),
        None if object.invalidated_commit_seq.is_some() => ServedStanding::NotLiveAtSnapshot,
        None => ServedStanding::NeverAdmitted,
    };
    let (occurrences, excluded_representations) = load_occurrences(tx, requested, &object)?;
    let (causality, causal_record) =
        causal_class_at(tx, requested, &object, bounds.max_causal_payload_bytes)?;
    Ok(ClaimFacts {
        object,
        decision,
        own_admission,
        lineage_admission,
        served,
        occurrences,
        excluded_representations,
        causality,
        causal_record,
    })
}

fn sensitivity_field(value: &str) -> Result<Sensitivity, ClaimFactsError> {
    Sensitivity::ALL
        .iter()
        .copied()
        .find(|class| class.as_str() == value)
        .ok_or(ClaimFactsError::MalformedRequiredField)
}

/// The decision row is written from the same spec as its registry row, so a
/// disagreement on creation commit, invalidation, or class is corruption, not a fact.
/// `ObjectRow` decodes an unrecognized class as `Secret`; compare raw stored
/// values.
fn load_decision(
    tx: &Transaction<'_>,
    object: &ObjectRow,
) -> Result<ClaimDecisionFacts, ClaimFactsError> {
    struct Raw {
        decision: ClaimDecisionFacts,
        created: i64,
        invalidated: Option<i64>,
        registry_invalidated: Option<i64>,
        sensitivity: String,
        registry_sensitivity: String,
    }
    let row = tx
        .query_row_cached(
            "SELECT d.decision_id,d.decision_kind,d.proposition_id,d.scope_id,d.anchor_id,
                    d.evidence_id,d.created_commit_seq,d.invalidated_commit_seq,
                    o.invalidated_commit_seq,d.sensitivity_class,o.sensitivity_class
             FROM decisions d
             JOIN object_registry o ON o.object_id=d.object_id
             WHERE d.object_id=?1",
            [&object.object_id],
            |row| {
                Ok(Raw {
                    decision: ClaimDecisionFacts {
                        decision_id: row.get(0)?,
                        decision_kind: row.get(1)?,
                        proposition_id: row.get(2)?,
                        scope_id: row.get(3)?,
                        anchor_id: row.get(4)?,
                        evidence_id: row.get(5)?,
                    },
                    created: row.get(6)?,
                    invalidated: row.get(7)?,
                    registry_invalidated: row.get(8)?,
                    sensitivity: row.get(9)?,
                    registry_sensitivity: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)?;
    let raw = row.ok_or(KernelError::CorruptCanonicalRow)?;
    if raw.created != object.created_commit_seq
        || raw.invalidated != raw.registry_invalidated
        || raw.sensitivity != raw.registry_sensitivity
    {
        return Err(KernelError::CorruptCanonicalRow.into());
    }
    sensitivity_field(&raw.registry_sensitivity)?;
    Ok(raw.decision)
}

#[derive(Clone, Copy)]
enum AdmissionScope {
    Own,
    Lineage,
}

struct RawAdmission {
    admission_decision_id: String,
    commit_seq: i64,
    event_kind: String,
    maturity: String,
    effective_maturity: String,
    disposition: String,
    visibility: String,
    outcome: String,
    source_class: String,
    taint_class: String,
    policy_revision: i64,
    sensitivity: String,
    evidence_id: Option<String>,
    approval_object_id: Option<String>,
    approval_valid: bool,
    trigger_object_id: Option<String>,
    elevated_support: bool,
}

/// One statement per scope, built once: the row the serving view selects for
/// `o` at `:governing_as_of`, with the supporting approval's validity there.
fn admission_sql(scope: AdmissionScope) -> String {
    let selection = match scope {
        AdmissionScope::Own => served_own_decision_sql("a"),
        AdmissionScope::Lineage => served_lineage_decision_sql("a"),
    };
    let approval_valid = supporting_approval_valid_sql("d.approval_object_id");
    format!(
        "SELECT d.admission_decision_id,d.commit_seq,d.event_kind,d.maturity,
                d.effective_maturity,d.disposition,d.visibility,d.outcome,d.source_class,
                d.taint_class,d.policy_revision,d.sensitivity_class,d.evidence_id,
                d.approval_object_id,{approval_valid},d.trigger_object_id,d.elevated_support
         FROM object_registry o
         JOIN admission_decisions d ON d.admission_decision_id={selection}
         WHERE o.object_id=:object_id"
    )
}

fn load_admission(
    tx: &Transaction<'_>,
    requested: i64,
    object: &ObjectRow,
    scope: AdmissionScope,
) -> Result<Option<AdmissionFacts>, ClaimFactsError> {
    static OWN_SQL: LazyLock<String> = LazyLock::new(|| admission_sql(AdmissionScope::Own));
    static LINEAGE_SQL: LazyLock<String> = LazyLock::new(|| admission_sql(AdmissionScope::Lineage));
    let sql = match scope {
        AdmissionScope::Own => OWN_SQL.as_str(),
        AdmissionScope::Lineage => LINEAGE_SQL.as_str(),
    };
    let mut statement = tx.prepare_cached(sql).map_err(map_sqlite)?;
    let raw = statement
        .query_row(
            rusqlite::named_params! {
                ":object_id": object.object_id,
                ":governing_as_of": requested,
            },
            |row| {
                Ok(RawAdmission {
                    admission_decision_id: row.get(0)?,
                    commit_seq: row.get(1)?,
                    event_kind: row.get(2)?,
                    maturity: row.get(3)?,
                    effective_maturity: row.get(4)?,
                    disposition: row.get(5)?,
                    visibility: row.get(6)?,
                    outcome: row.get(7)?,
                    source_class: row.get(8)?,
                    taint_class: row.get(9)?,
                    policy_revision: row.get(10)?,
                    sensitivity: row.get(11)?,
                    evidence_id: row.get(12)?,
                    approval_object_id: row.get(13)?,
                    approval_valid: row.get(14)?,
                    trigger_object_id: row.get(15)?,
                    elevated_support: row.get(16)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)?;
    raw.map(interpret_admission).transpose()
}

fn interpret_admission(raw: RawAdmission) -> Result<AdmissionFacts, ClaimFactsError> {
    fn field<T: for<'a> TryFrom<&'a str>>(value: &str) -> Result<T, ClaimFactsError> {
        T::try_from(value).map_err(|_| ClaimFactsError::MalformedRequiredField)
    }
    Ok(AdmissionFacts {
        admission_decision_id: raw.admission_decision_id,
        commit_seq: raw.commit_seq,
        event_kind: field(&raw.event_kind)?,
        historical_maturity: field(&raw.maturity)?,
        effective_maturity: field(&raw.effective_maturity)?,
        disposition: field(&raw.disposition)?,
        visibility: field(&raw.visibility)?,
        outcome: field(&raw.outcome)?,
        source_class: field(&raw.source_class)?,
        taint_class: field(&raw.taint_class)?,
        policy_revision: raw.policy_revision,
        sensitivity: sensitivity_field(&raw.sensitivity)?,
        evidence_id: raw.evidence_id,
        supporting_approval: raw.approval_object_id.map(|object_id| SupportingApproval {
            object_id,
            valid_at_snapshot: raw.approval_valid,
        }),
        trigger_object_id: raw.trigger_object_id,
        elevated_support: raw.elevated_support,
    })
}

/// The identity a claim decision publishes under each class it can carry.
fn claim_identity(class: OccurrenceClass, object_id: &str) -> Option<[(&'static str, &str); 1]> {
    match class {
        OccurrenceClass::CanonicalClaims => Some([("object_id", object_id)]),
        OccurrenceClass::PromotedMemory => Some([("decision_object_id", object_id)]),
        _ => None,
    }
}

fn load_occurrences(
    tx: &Transaction<'_>,
    requested: i64,
    object: &ObjectRow,
) -> Result<(Vec<ClaimOccurrence>, Vec<ExcludedRepresentation>), KernelError> {
    let revision = object.source_revision.to_string();
    let mut occurrences = Vec::new();
    let mut excluded = Vec::new();
    for class in [
        OccurrenceClass::CanonicalClaims,
        OccurrenceClass::PromotedMemory,
    ] {
        let identity = claim_identity(class, &object.object_id)
            .expect("both claim classes have a one-field identity");
        for representation in class.representations() {
            let encoded = encode_preserving_span(&Occurrence {
                class: class.code(),
                identity: &identity,
                revision: &revision,
                representation,
                span: None,
            });
            let Ok(encoded) = encoded else {
                excluded.push(ExcludedRepresentation {
                    class,
                    representation,
                    reason: RepresentationExclusion::MalformedIdentity,
                });
                continue;
            };
            let descriptor_object_id = descriptor_object_id(&encoded.lineage_id, &revision);
            let descriptor = load_descriptor(tx, requested, &descriptor_object_id)?;
            match descriptor {
                None => excluded.push(ExcludedRepresentation {
                    class,
                    representation,
                    reason: RepresentationExclusion::NoDescriptor,
                }),
                Some(raw) => {
                    let detail = stored_detail(&raw.payload)?;
                    let reencoded =
                        reencoded_identity(&detail).ok_or(KernelError::CorruptCanonicalRow)?;
                    if reencoded.occurrence_id != encoded.occurrence_id
                        || raw.source_kind != class.code()
                        || raw.source_revision != object.source_revision
                        || raw.sensitivity != raw.registry_sensitivity
                        || raw.created_commit_seq != raw.observation_created
                        || raw.invalidated_commit_seq != raw.observation_invalidated
                        || detail.lineage_id != raw.source_id
                        || detail.evidence_id != raw.evidence_id
                        || detail.artifact_digest != raw.artifact_digest
                        || detail.payload_id != raw.artifact_digest
                    {
                        return Err(KernelError::CorruptCanonicalRow);
                    }
                    occurrences.push(ClaimOccurrence {
                        class,
                        representation,
                        descriptor_object_id,
                        occurrence_id: detail.occurrence_id,
                        lineage_id: detail.lineage_id,
                        payload_id: detail.payload_id,
                        artifact_digest: detail.artifact_digest,
                        evidence_id: detail.evidence_id,
                        descriptor_commit_seq: raw.created_commit_seq,
                    });
                }
            }
        }
    }
    Ok((occurrences, excluded))
}

struct RawDescriptor {
    payload: Vec<u8>,
    source_kind: String,
    source_revision: i64,
    sensitivity: String,
    registry_sensitivity: String,
    created_commit_seq: i64,
    invalidated_commit_seq: Option<i64>,
    observation_created: i64,
    observation_invalidated: Option<i64>,
    source_id: String,
    evidence_id: String,
    artifact_digest: String,
}

/// Returns the descriptor row visible at `requested` under `Descriptors::LiveAtEnd`.
fn load_descriptor(
    tx: &Transaction<'_>,
    requested: i64,
    descriptor_object_id: &str,
) -> Result<Option<RawDescriptor>, KernelError> {
    static SQL: LazyLock<String> = LazyLock::new(|| {
        format!(
            "SELECT b.observation_payload,o.source_kind,o.source_revision,b.sensitivity_class,
                    o.sensitivity_class,o.created_commit_seq,o.invalidated_commit_seq,
                    b.created_commit_seq,b.invalidated_commit_seq,o.source_id,
                    b.evidence_id,e.artifact_digest
             FROM object_registry o
             JOIN observations b ON b.object_id=o.object_id
             JOIN evidence_meta e ON e.evidence_id=b.evidence_id
             WHERE o.object_id=?1 AND b.observation_kind=?2 AND {live}",
            live = Descriptors::LiveAtEnd.predicate("?3", "0"),
        )
    });
    let mut statement = tx.prepare_cached(&SQL).map_err(map_sqlite)?;
    statement
        .query_row(
            params![descriptor_object_id, SOURCE_DESCRIPTOR_KIND, requested],
            |row| {
                Ok(RawDescriptor {
                    payload: row.get(0)?,
                    source_kind: row.get(1)?,
                    source_revision: row.get(2)?,
                    sensitivity: row.get(3)?,
                    registry_sensitivity: row.get(4)?,
                    created_commit_seq: row.get(5)?,
                    invalidated_commit_seq: row.get(6)?,
                    observation_created: row.get(7)?,
                    observation_invalidated: row.get(8)?,
                    source_id: row.get(9)?,
                    evidence_id: row.get(10)?,
                    artifact_digest: row.get(11)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)
}
