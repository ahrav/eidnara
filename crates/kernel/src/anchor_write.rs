//! Typed anchor writes: one `anchors` row and its `object_registry` row per
//! [`Envelope::insert_anchor`] call.
//!
//! [`validate_anchor`] builds the exact row the write stores and runs the
//! applicability engine's own checks on it: the payload bound the engine
//! applies before evaluation, [`AnchorCondition::decode`], and a round trip
//! from the decoded condition back to the requested value. The write calls the
//! same function, so every committed anchor row is one the engine decodes.

use std::collections::BTreeMap;

use rusqlite::params;
use serde::Serialize;

use super::anchor::{
    AnchorCapture, AnchorCondition, AnchorDecodeError, AnchorKind, AnchorRowSpec, GitCondition,
    encode_anchor_captures,
};
use super::applicability::anchor_row_exceeds_bounds;
use super::envelope::{Envelope, ObjectRow, PendingChange};
use super::object_write::{insert_registry, map_write_error, record_registry_fields};
use super::redaction::{RedactedField, identity, payload_has_secret};
use super::slice::require_live;
use super::{CachedSql, KernelError, Sensitivity};

/// Change kind recorded for every anchor insert.
pub const ANCHOR_INSERT_KIND: &str = "anchor_insert";

/// The condition one anchor row stores, one variant per [`AnchorKind`].
///
/// Each variant holds exactly the values its kind's columns carry. Git
/// variants carry optional capture-time representations for the resolution
/// ladder; an empty list stores no payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorValue {
    Exact {
        value: String,
    },
    ReachableFrom {
        oid: String,
        captures: Vec<AnchorCapture>,
    },
    ReachableBetween {
        start_oid: String,
        end_oid: String,
        captures: Vec<AnchorCapture>,
    },
    DeploymentRevision {
        revision: String,
    },
    ConfigRevision {
        revision: String,
    },
    PlatformVersion {
        range: String,
    },
    /// Half-open `[start_ms, end_ms)` in UTC milliseconds.
    WallClockInterval {
        start_ms: i64,
        end_ms: i64,
    },
}

impl AnchorValue {
    pub fn kind(&self) -> AnchorKind {
        match self {
            Self::Exact { .. } => AnchorKind::Exact,
            Self::ReachableFrom { .. } => AnchorKind::ReachableFrom,
            Self::ReachableBetween { .. } => AnchorKind::ReachableBetween,
            Self::DeploymentRevision { .. } => AnchorKind::DeploymentRevision,
            Self::ConfigRevision { .. } => AnchorKind::ConfigRevision,
            Self::PlatformVersion { .. } => AnchorKind::PlatformVersion,
            Self::WallClockInterval { .. } => AnchorKind::WallClockInterval,
        }
    }

    fn row(&self, anchor_id: &str) -> AnchorRowSpec {
        let mut row = AnchorRowSpec {
            anchor_id: anchor_id.to_string(),
            anchor_kind: self.kind().as_str().to_string(),
            ..AnchorRowSpec::default()
        };
        let payload = |captures: &[AnchorCapture]| {
            (!captures.is_empty()).then(|| encode_anchor_captures(captures))
        };
        match self {
            Self::Exact { value } => row.exact_value = Some(value.clone()),
            Self::ReachableFrom { oid, captures } => {
                row.reachable_from_oid = Some(oid.clone());
                row.payload = payload(captures);
            }
            Self::ReachableBetween {
                start_oid,
                end_oid,
                captures,
            } => {
                row.reachable_between_start_oid = Some(start_oid.clone());
                row.reachable_between_end_oid = Some(end_oid.clone());
                row.payload = payload(captures);
            }
            Self::DeploymentRevision { revision } => {
                row.deployment_revision = Some(revision.clone());
            }
            Self::ConfigRevision { revision } => row.config_revision = Some(revision.clone()),
            Self::PlatformVersion { range } => row.platform_version_range = Some(range.clone()),
            Self::WallClockInterval { start_ms, end_ms } => {
                row.wall_clock_start = Some(*start_ms);
                row.wall_clock_end = Some(*end_ms);
            }
        }
        row
    }

    /// Whether a capture's `commit_oid` is outside this value's anchored oids.
    fn has_unanchored_capture(&self) -> bool {
        match self {
            Self::ReachableFrom { oid, captures } => {
                captures.iter().any(|capture| capture.commit_oid != *oid)
            }
            Self::ReachableBetween {
                start_oid,
                end_oid,
                captures,
            } => captures
                .iter()
                .any(|capture| capture.commit_oid != *start_oid && capture.commit_oid != *end_oid),
            Self::Exact { .. }
            | Self::DeploymentRevision { .. }
            | Self::ConfigRevision { .. }
            | Self::PlatformVersion { .. }
            | Self::WallClockInterval { .. } => false,
        }
    }

    /// Whether `decoded` is the condition this value requested.
    fn matches(&self, decoded: &AnchorCondition) -> bool {
        match (self, decoded) {
            (Self::Exact { value }, AnchorCondition::Exact { value: stored }) => value == stored,
            (
                Self::ReachableFrom { oid, captures },
                AnchorCondition::Git(GitCondition::ReachableFrom {
                    oid: stored,
                    captures: stored_captures,
                }),
            ) => oid == stored && same_captures(captures, stored_captures),
            (
                Self::ReachableBetween {
                    start_oid,
                    end_oid,
                    captures,
                },
                AnchorCondition::Git(GitCondition::ReachableBetween {
                    start_oid: stored_start,
                    end_oid: stored_end,
                    captures: stored_captures,
                }),
            ) => {
                start_oid == stored_start
                    && end_oid == stored_end
                    && same_captures(captures, stored_captures)
            }
            (
                Self::DeploymentRevision { revision },
                AnchorCondition::DeploymentRevision { revision: stored },
            )
            | (
                Self::ConfigRevision { revision },
                AnchorCondition::ConfigRevision { revision: stored },
            ) => revision == stored,
            (Self::PlatformVersion { range }, AnchorCondition::PlatformVersion { req }) => {
                semver::VersionReq::parse(range).is_ok_and(|parsed| &parsed == req)
            }
            (
                Self::WallClockInterval { start_ms, end_ms },
                AnchorCondition::WallClockInterval {
                    start_ms: stored_start,
                    end_ms: stored_end,
                },
            ) => start_ms == stored_start && end_ms == stored_end,
            _ => false,
        }
    }
}

/// Decode refuses two captures of one commit, so equal lengths and every
/// requested capture present mean the two sets are equal.
fn same_captures(requested: &[AnchorCapture], stored: &BTreeMap<String, AnchorCapture>) -> bool {
    requested.len() == stored.len()
        && requested
            .iter()
            .all(|capture| stored.get(&capture.commit_oid) == Some(capture))
}

/// Complete input for inserting one anchor and its object identity. The
/// registry fields match [`crate::DecisionSpec`]'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorSpec {
    pub anchor_id: String,
    pub object_id: String,
    pub domain_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub source_revision: i64,
    pub sensitivity: Sensitivity,
    pub value: AnchorValue,
}

/// Stable identifiers returned after an anchor write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnchorWriteOutcome {
    pub anchor_id: String,
    pub object_id: String,
}

impl AnchorWriteOutcome {
    /// Serializes the string-only outcome; serialization cannot fail for these fields.
    pub fn result_json(&self) -> String {
        serde_json::to_string(self).expect("string-only outcome is serializable")
    }
}

/// Why [`validate_anchor`] or [`validate_anchor_row`] refused an anchor.
/// [`Envelope::insert_anchor`] reports every cause as
/// [`KernelError::InvalidInput`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum AnchorValidationError {
    /// An identity field is blank or holds a detected secret, or
    /// `source_revision` is negative.
    #[error("anchor identity field is blank, negative, or secret-bearing")]
    InvalidIdentity,
    /// The row's text columns and payload exceed the engine's object payload
    /// bound, so the engine would evaluate it uncertain.
    #[error("anchor row exceeds the object payload bound")]
    ExceedsBounds,
    #[error(transparent)]
    Decode(#[from] AnchorDecodeError),
    /// The row decodes to a condition other than the requested one. The
    /// typed builder and the decoder agree for every value, so this check
    /// catches a later change to either one.
    #[error("anchor row decodes to a different condition")]
    RoundTrip,
    /// A capture names a commit other than the anchor's own oids. The
    /// resolution ladder reads captures by anchored oid only.
    #[error("anchor capture names a commit the anchor does not name")]
    UnanchoredCapture,
    /// A value column or the capture payload holds a detected secret, or the
    /// scanner could not read it, as for a value above its input bound.
    #[error("anchor value holds a detected secret or could not be scanned")]
    SecretValue,
}

/// Checks one stored-form anchor row the way the applicability engine reads
/// it: within the object payload bound, then decodable.
pub fn validate_anchor_row(row: &AnchorRowSpec) -> Result<AnchorCondition, AnchorValidationError> {
    if anchor_row_exceeds_bounds(row) {
        return Err(AnchorValidationError::ExceedsBounds);
    }
    Ok(AnchorCondition::decode(row)?)
}

/// Builds the row [`Envelope::insert_anchor`] would store for `spec` and
/// returns it when every check the write applies passes.
pub fn validate_anchor(spec: &AnchorSpec) -> Result<AnchorRowSpec, AnchorValidationError> {
    let identities = [
        &spec.anchor_id,
        &spec.object_id,
        &spec.domain_id,
        &spec.source_kind,
        &spec.source_id,
    ];
    if spec.source_revision < 0
        || identities
            .iter()
            .any(|field| field.trim().is_empty() || identity(field).is_err())
    {
        return Err(AnchorValidationError::InvalidIdentity);
    }
    let row = spec.value.row(&spec.anchor_id);
    let decoded = validate_anchor_row(&row)?;
    if !spec.value.matches(&decoded) {
        return Err(AnchorValidationError::RoundTrip);
    }
    if spec.value.has_unanchored_capture() {
        return Err(AnchorValidationError::UnanchoredCapture);
    }
    let values = [
        &row.exact_value,
        &row.reachable_from_oid,
        &row.reachable_between_start_oid,
        &row.reachable_between_end_oid,
        &row.deployment_revision,
        &row.config_revision,
        &row.platform_version_range,
    ];
    if values
        .into_iter()
        .flatten()
        .any(|value| identity(value).is_err())
        || row
            .payload
            .as_deref()
            .is_some_and(|payload| payload_has_secret(payload).unwrap_or(true))
    {
        return Err(AnchorValidationError::SecretValue);
    }
    Ok(row)
}

impl Envelope<'_> {
    /// Inserts one anchor row and its `anchor` registry row in this envelope.
    ///
    /// The row is the one [`validate_anchor`] returns; any refusal there is
    /// `InvalidInput`. A missing live domain is `NotFound`, and a duplicate
    /// `anchor_id` or `object_id` is `Conflict`. A refusal poisons the
    /// envelope, so the commit stores no part of the anchor.
    pub fn insert_anchor(&mut self, spec: AnchorSpec) -> Result<AnchorWriteOutcome, KernelError> {
        self.guarded(|envelope| envelope.insert_anchor_inner(spec))
    }

    fn insert_anchor_inner(&mut self, spec: AnchorSpec) -> Result<AnchorWriteOutcome, KernelError> {
        let row = validate_anchor(&spec).map_err(|_| KernelError::InvalidInput)?;
        require_live(self.tx, "domains", "domain_id", &spec.domain_id)?;
        let object = ObjectRow {
            object_id: spec.object_id.clone(),
            object_kind: "anchor".to_string(),
            domain_id: spec.domain_id.clone(),
            source_kind: spec.source_kind.clone(),
            source_id: spec.source_id.clone(),
            source_revision: spec.source_revision,
            created_commit_seq: self.commit_seq,
            invalidated_commit_seq: None,
            superseded_by: None,
            sensitivity: spec.sensitivity,
        };
        insert_registry(self.tx, self.commit_seq, &object)?;
        // `validate_anchor` refused every identity field holding a detection.
        let field = |text: &str| RedactedField {
            text: text.to_string(),
            detections: Vec::new(),
        };
        record_registry_fields(
            self.tx,
            &spec.object_id,
            &field(&spec.domain_id),
            &field(&spec.object_id),
            &field(&spec.source_kind),
            &field(&spec.source_id),
            self.commit_seq,
        )?;
        self.tx
            .execute_cached(
                "INSERT INTO anchors(
                     anchor_id,object_id,anchor_kind,exact_value,reachable_from_oid,
                     reachable_between_start_oid,reachable_between_end_oid,deployment_revision,
                     config_revision,platform_version_range,wall_clock_start,wall_clock_end,
                     payload,created_commit_seq,sensitivity_class
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    row.anchor_id,
                    spec.object_id,
                    row.anchor_kind,
                    row.exact_value,
                    row.reachable_from_oid,
                    row.reachable_between_start_oid,
                    row.reachable_between_end_oid,
                    row.deployment_revision,
                    row.config_revision,
                    row.platform_version_range,
                    row.wall_clock_start,
                    row.wall_clock_end,
                    row.payload,
                    self.commit_seq,
                    spec.sensitivity.as_str(),
                ],
            )
            .map_err(map_write_error)?;
        self.changes.push(PendingChange {
            object,
            kind: ANCHOR_INSERT_KIND,
            replaced_object_id: None,
            redactions: Vec::new(),
            audit: None,
        });
        Ok(AnchorWriteOutcome {
            anchor_id: spec.anchor_id,
            object_id: spec.object_id,
        })
    }
}
