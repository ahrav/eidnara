use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CHECKPOINT_DIGEST_PROTOCOL: &str = "eval-checkpoint/v1";
pub const LIVE_DIGEST_PROTOCOL: &str = "eval-guard-live/v1";
pub const GUARD_DIGEST_PROTOCOL: &str = "eval-prefix-guard/v1";
pub const AGING_REPORT_SCHEMA: &str = "eval-suite-c-aging-report/v1";
pub const AGING_RESULT_DIGEST_PROTOCOL: &str = "eval-suite-c-aging-report-result/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreFamily {
    Kernel,
    Memory,
    SearchProjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkCounter {
    OutboxUnpublished,
    CatchUpLag,
    EmbeddingOpen,
    CaptureJobsPending,
    ReviewerJobsOpen,
}

impl StoreFamily {
    pub const ALL: [Self; 3] = [Self::Kernel, Self::Memory, Self::SearchProjection];

    pub fn counters(self) -> &'static [WorkCounter] {
        match self {
            Self::Kernel => &[WorkCounter::OutboxUnpublished],
            Self::Memory => &[
                WorkCounter::CaptureJobsPending,
                WorkCounter::ReviewerJobsOpen,
            ],
            Self::SearchProjection => &[WorkCounter::CatchUpLag, WorkCounter::EmbeddingOpen],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalCheckpoint {
    pub busy: i64,
    pub wal_frames: i64,
    pub checkpointed_frames: i64,
}

impl WalCheckpoint {
    /// SQLite reports `-1` frames for a database outside WAL mode.
    pub fn is_truncated(&self) -> bool {
        self.busy == 0 && self.wal_frames >= 0 && self.wal_frames == self.checkpointed_frames
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreQuiescence {
    pub pending: BTreeMap<WorkCounter, u64>,
    pub wal: WalCheckpoint,
    pub wal_sidecar_bytes: u64,
    pub handles_closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiescenceReceipt {
    pub step: u32,
    pub stores: BTreeMap<StoreFamily, StoreQuiescence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointRefused {
    MissingStoreEvidence {
        family: StoreFamily,
    },
    MissingCounter {
        family: StoreFamily,
        counter: WorkCounter,
    },
    /// A counter outside the family's declared set.
    UndeclaredCounter {
        family: StoreFamily,
        counter: WorkCounter,
    },
    PendingWork {
        family: StoreFamily,
        counter: WorkCounter,
        observed: u64,
    },
    WalNotTruncated {
        family: StoreFamily,
        wal: WalCheckpoint,
    },
    WalSidecarPresent {
        family: StoreFamily,
        bytes: u64,
    },
    HandleOpen {
        family: StoreFamily,
    },
    MalformedIncarnation(String),
    NoFiles,
    /// An empty path or a digest that is not 64 lowercase hex digits.
    MalformedFile {
        path: String,
    },
    Shape(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub receipt: QuiescenceReceipt,
    pub incarnation_id: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreIntegrity {
    pub integrity_check: String,
    pub foreign_key_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reopened {
    pub incarnation_id: String,
    pub stores: BTreeMap<StoreFamily, StoreIntegrity>,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreRefused {
    ForeignIncarnation {
        expected: String,
        found: String,
    },
    MissingStore {
        family: StoreFamily,
    },
    IntegrityCheck {
        family: StoreFamily,
        reported: String,
    },
    ForeignKeyViolations {
        family: StoreFamily,
        count: u64,
    },
    FileMissing {
        path: String,
    },
    FileDiffers {
        path: String,
    },
}

impl QuiescenceReceipt {
    pub fn check(&self) -> Result<(), CheckpointRefused> {
        for family in StoreFamily::ALL {
            let store = self
                .stores
                .get(&family)
                .ok_or(CheckpointRefused::MissingStoreEvidence { family })?;
            for counter in family.counters() {
                match store.pending.get(counter) {
                    None => {
                        return Err(CheckpointRefused::MissingCounter {
                            family,
                            counter: *counter,
                        });
                    }
                    Some(0) => {}
                    Some(observed) => {
                        return Err(CheckpointRefused::PendingWork {
                            family,
                            counter: *counter,
                            observed: *observed,
                        });
                    }
                }
            }
            if let Some(counter) = store
                .pending
                .keys()
                .find(|counter| !family.counters().contains(counter))
            {
                return Err(CheckpointRefused::UndeclaredCounter {
                    family,
                    counter: *counter,
                });
            }
            if !store.wal.is_truncated() {
                return Err(CheckpointRefused::WalNotTruncated {
                    family,
                    wal: store.wal,
                });
            }
            if store.wal_sidecar_bytes > 0 {
                return Err(CheckpointRefused::WalSidecarPresent {
                    family,
                    bytes: store.wal_sidecar_bytes,
                });
            }
            if !store.handles_closed {
                return Err(CheckpointRefused::HandleOpen { family });
            }
        }
        Ok(())
    }
}

impl Checkpoint {
    pub fn admit(
        receipt: &QuiescenceReceipt,
        incarnation_id: &str,
    ) -> Result<(), CheckpointRefused> {
        receipt.check()?;
        if !is_lower_hex(incarnation_id, 32) {
            return Err(CheckpointRefused::MalformedIncarnation(
                incarnation_id.to_string(),
            ));
        }
        Ok(())
    }

    pub fn new(
        receipt: QuiescenceReceipt,
        incarnation_id: String,
        files: BTreeMap<String, String>,
    ) -> Result<Self, CheckpointRefused> {
        Self::admit(&receipt, &incarnation_id)?;
        if files.is_empty() {
            return Err(CheckpointRefused::NoFiles);
        }
        if let Some((path, _)) = files
            .iter()
            .find(|(path, digest)| path.is_empty() || !is_lower_hex(digest, 64))
        {
            return Err(CheckpointRefused::MalformedFile { path: path.clone() });
        }
        Ok(Self {
            receipt,
            incarnation_id,
            files,
        })
    }

    pub fn digest(&self) -> Result<String, CheckpointRefused> {
        let value =
            serde_json::to_value(self).map_err(|e| CheckpointRefused::Shape(e.to_string()))?;
        protocol_digest(CHECKPOINT_DIGEST_PROTOCOL, &value)
            .map_err(|e| CheckpointRefused::Shape(e.to_string()))
    }

    pub fn accept(&self, reopened: &Reopened) -> Result<(), RestoreRefused> {
        if reopened.incarnation_id != self.incarnation_id {
            return Err(RestoreRefused::ForeignIncarnation {
                expected: self.incarnation_id.clone(),
                found: reopened.incarnation_id.clone(),
            });
        }
        for family in StoreFamily::ALL {
            let store = reopened
                .stores
                .get(&family)
                .ok_or(RestoreRefused::MissingStore { family })?;
            if store.integrity_check != "ok" {
                return Err(RestoreRefused::IntegrityCheck {
                    family,
                    reported: store.integrity_check.clone(),
                });
            }
            if store.foreign_key_violations > 0 {
                return Err(RestoreRefused::ForeignKeyViolations {
                    family,
                    count: store.foreign_key_violations,
                });
            }
        }
        for (path, digest) in &self.files {
            match reopened.files.get(path) {
                None => return Err(RestoreRefused::FileMissing { path: path.clone() }),
                Some(found) if found != digest => {
                    return Err(RestoreRefused::FileDiffers { path: path.clone() });
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneReason {
    Superseded,
    Retired,
    EvidenceInvalidated,
    Purged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Building,
    Verified,
    Selected,
    Retired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRows {
    pub occurrences: BTreeMap<String, String>,
    pub lexical: BTreeSet<String>,
    pub pending_embedding: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Death {
    pub invalidated_commit_seq: i64,
    pub reason: TombstoneReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalRows {
    pub occurrences: BTreeMap<String, i64>,
    pub tombstones: BTreeMap<String, Death>,
    pub generation_state: GenerationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRows {
    pub snapshot_commit_seq: i64,
    pub live: LiveRows,
    pub historical: HistoricalRows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Divergence {
    TombstonedBeforeSnapshot {
        occurrence_id: String,
        death: Death,
    },
    GenerationState {
        earlier: GenerationState,
        later: GenerationState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unenumerated {
    SnapshotOrder { earlier: i64, later: i64 },
    OccurrenceOnlyInEarlier { occurrence_id: String },
    OccurrenceOnlyInLater { occurrence_id: String },
    TombstoneOnlyInEarlier { occurrence_id: String },
    TombstoneOnlyInLater { occurrence_id: String },
    TombstoneDiffers { occurrence_id: String },
    CreatedDiffers { occurrence_id: String },
    OrphanTombstone { occurrence_id: String },
    Shape(String),
}

pub fn live_digest(live: &LiveRows) -> Result<String, Unenumerated> {
    let value = serde_json::to_value(live).map_err(|e| Unenumerated::Shape(e.to_string()))?;
    protocol_digest(LIVE_DIGEST_PROTOCOL, &value).map_err(|e| Unenumerated::Shape(e.to_string()))
}

pub fn historical_diff(
    earlier: &ProjectionRows,
    later: &ProjectionRows,
) -> Result<Vec<Divergence>, Unenumerated> {
    if later.snapshot_commit_seq < earlier.snapshot_commit_seq {
        return Err(Unenumerated::SnapshotOrder {
            earlier: earlier.snapshot_commit_seq,
            later: later.snapshot_commit_seq,
        });
    }
    let (a, b) = (&earlier.historical, &later.historical);
    for rows in [a, b] {
        if let Some(id) = rows
            .tombstones
            .keys()
            .find(|id| !rows.occurrences.contains_key(*id))
        {
            return Err(Unenumerated::OrphanTombstone {
                occurrence_id: id.clone(),
            });
        }
    }
    let mut divergences = Vec::new();
    for (occurrence_id, created) in &a.occurrences {
        let occurrence_id = occurrence_id.clone();
        match b.occurrences.get(&occurrence_id) {
            Some(other) if other != created => {
                return Err(Unenumerated::CreatedDiffers { occurrence_id });
            }
            Some(_) => match (
                a.tombstones.get(&occurrence_id),
                b.tombstones.get(&occurrence_id),
            ) {
                (None, None) => {}
                (Some(x), Some(y)) if x == y => {}
                (Some(_), Some(_)) => {
                    return Err(Unenumerated::TombstoneDiffers { occurrence_id });
                }
                (Some(_), None) => {
                    return Err(Unenumerated::TombstoneOnlyInEarlier { occurrence_id });
                }
                (None, Some(_)) => {
                    return Err(Unenumerated::TombstoneOnlyInLater { occurrence_id });
                }
            },
            None => {
                let death = a.tombstones.get(&occurrence_id).copied().filter(|death| {
                    earlier.snapshot_commit_seq < death.invalidated_commit_seq
                        && death.invalidated_commit_seq <= later.snapshot_commit_seq
                });
                let Some(death) = death else {
                    return Err(Unenumerated::OccurrenceOnlyInEarlier { occurrence_id });
                };
                divergences.push(Divergence::TombstonedBeforeSnapshot {
                    occurrence_id,
                    death,
                });
            }
        }
    }
    if let Some(id) = b
        .occurrences
        .keys()
        .find(|id| !a.occurrences.contains_key(*id))
    {
        return Err(Unenumerated::OccurrenceOnlyInLater {
            occurrence_id: id.clone(),
        });
    }
    if let Some(id) = b
        .tombstones
        .keys()
        .find(|id| !a.tombstones.contains_key(*id))
    {
        return Err(Unenumerated::TombstoneOnlyInLater {
            occurrence_id: id.clone(),
        });
    }
    if a.generation_state != b.generation_state {
        divergences.push(Divergence::GenerationState {
            earlier: a.generation_state,
            later: b.generation_state,
        });
    }
    Ok(divergences)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub source_revision: i64,
    pub created_commit_seq: i64,
    pub invalidated_commit_seq: Option<i64>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub start_message: i64,
    pub end_message: i64,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSnapshot {
    pub commit_seq: i64,
    pub kernel: BTreeMap<String, Descriptor>,
    pub projection_live: LiveRows,
    pub memory: BTreeMap<i64, Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixRefused {
    HistorySlipped { family: StoreFamily },
    CommitSeqDiffers { full: i64, resumed: i64 },
    CommitSeqNotMonotonic { at_checkpoint: i64, at_end: i64 },
    HistoryRewritten { object_id: String },
    Shape(String),
}

impl StateSnapshot {
    pub fn guard_digest(&self) -> Result<String, PrefixRefused> {
        let value = serde_json::to_value(self).map_err(|e| PrefixRefused::Shape(e.to_string()))?;
        protocol_digest(GUARD_DIGEST_PROTOCOL, &value)
            .map_err(|e| PrefixRefused::Shape(e.to_string()))
    }

    pub fn compare(full: &Self, resumed: &Self) -> Result<(), PrefixRefused> {
        if full.commit_seq != resumed.commit_seq {
            return Err(PrefixRefused::CommitSeqDiffers {
                full: full.commit_seq,
                resumed: resumed.commit_seq,
            });
        }
        let slipped = [
            (StoreFamily::Kernel, full.kernel != resumed.kernel),
            (StoreFamily::Memory, full.memory != resumed.memory),
            (
                StoreFamily::SearchProjection,
                full.projection_live != resumed.projection_live,
            ),
        ];
        match slipped.into_iter().find(|(_, differs)| *differs) {
            Some((family, _)) => Err(PrefixRefused::HistorySlipped { family }),
            None => Ok(()),
        }
    }

    pub fn advanced(reopened: &Self, resumed: &Self) -> Result<(), PrefixRefused> {
        if resumed.commit_seq <= reopened.commit_seq {
            return Err(PrefixRefused::CommitSeqNotMonotonic {
                at_checkpoint: reopened.commit_seq,
                at_end: resumed.commit_seq,
            });
        }
        for (object_id, descriptor) in &resumed.kernel {
            // The kernel's append-only triggers refuse every other rewrite of a
            // known row; a backdated creation or death is what they cannot see.
            let backdated = match reopened.kernel.get(object_id) {
                None => descriptor.created_commit_seq <= reopened.commit_seq,
                Some(before) => {
                    before.invalidated_commit_seq.is_none()
                        && descriptor
                            .invalidated_commit_seq
                            .is_some_and(|at| at <= reopened.commit_seq)
                }
            };
            if backdated {
                return Err(PrefixRefused::HistoryRewritten {
                    object_id: object_id.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowDeaths {
    pub supersessions: u64,
    pub retirements: u64,
}

impl WindowDeaths {
    pub fn count(descriptors: &BTreeMap<String, Descriptor>, snapshot: i64, through: i64) -> Self {
        let mut deaths = Self::default();
        for descriptor in descriptors.values() {
            let Some(at) = descriptor.invalidated_commit_seq else {
                continue;
            };
            if descriptor.created_commit_seq <= snapshot && snapshot < at && at <= through {
                if descriptor.superseded_by.is_some() {
                    deaths.supersessions += 1;
                } else {
                    deaths.retirements += 1;
                }
            }
        }
        deaths
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionKind {
    CatchUp,
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionConstruction {
    pub kind: ConstructionKind,
    pub snapshot_commit_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardComparison {
    pub earlier: ProjectionConstruction,
    pub later: ProjectionConstruction,
    pub live_digests_equal: bool,
    pub divergences: Vec<Divergence>,
}

impl GuardComparison {
    pub fn of(
        earlier: (&ProjectionRows, ConstructionKind),
        later: (&ProjectionRows, ConstructionKind),
    ) -> Result<Self, Unenumerated> {
        let construction =
            |(rows, kind): (&ProjectionRows, ConstructionKind)| ProjectionConstruction {
                kind,
                snapshot_commit_seq: rows.snapshot_commit_seq,
            };
        Ok(Self {
            earlier: construction(earlier),
            later: construction(later),
            live_digests_equal: live_digest(&earlier.0.live)? == live_digest(&later.0.live)?,
            divergences: historical_diff(earlier.0, later.0)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgingReportError {
    SchemaMismatch {
        found: String,
    },
    ClaimBoundaryMismatch,
    MalformedDigest {
        field: &'static str,
    },
    CheckpointStepMismatch {
        checkpoint_step: u32,
        receipt_step: u32,
    },
    CommitSeqNotMonotonic {
        at_checkpoint: i64,
        at_end: i64,
    },
    /// The guard passed without the situation it exists for.
    WindowDeathsIncomplete {
        supersessions: u64,
        retirements: u64,
    },
    Receipt(CheckpointRefused),
    Shape(String),
    Lossy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgingReport {
    pub schema: String,
    pub eval_run_id: String,
    pub profile_digest: String,
    pub claim_boundary: crate::ClaimBoundary,
    pub steps: u32,
    pub checkpoint_step: u32,
    pub checkpoint_digest: String,
    pub receipt: QuiescenceReceipt,
    pub full_guard_digest: String,
    pub resumed_guard_digest: String,
    pub commit_seq_at_checkpoint: i64,
    pub commit_seq_at_end: i64,
    pub against_resumed: GuardComparison,
    pub against_bulk: GuardComparison,
    pub window_deaths: WindowDeaths,
    pub markers: BTreeSet<String>,
    pub envelope: crate::Envelope,
}

impl AgingReport {
    pub fn validate(&self) -> Result<(), AgingReportError> {
        if self.schema != AGING_REPORT_SCHEMA {
            return Err(AgingReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.claim_boundary != crate::ClaimBoundary::pinned() {
            return Err(AgingReportError::ClaimBoundaryMismatch);
        }
        for (field, digest) in [
            ("eval_run_id", &self.eval_run_id),
            ("profile_digest", &self.profile_digest),
            ("checkpoint_digest", &self.checkpoint_digest),
            ("full_guard_digest", &self.full_guard_digest),
            ("resumed_guard_digest", &self.resumed_guard_digest),
        ] {
            if !is_lower_hex(digest, 64) {
                return Err(AgingReportError::MalformedDigest { field });
            }
        }
        if self.receipt.step != self.checkpoint_step {
            return Err(AgingReportError::CheckpointStepMismatch {
                checkpoint_step: self.checkpoint_step,
                receipt_step: self.receipt.step,
            });
        }
        if self.commit_seq_at_end <= self.commit_seq_at_checkpoint {
            return Err(AgingReportError::CommitSeqNotMonotonic {
                at_checkpoint: self.commit_seq_at_checkpoint,
                at_end: self.commit_seq_at_end,
            });
        }
        if self.window_deaths.supersessions == 0 || self.window_deaths.retirements == 0 {
            return Err(AgingReportError::WindowDeathsIncomplete {
                supersessions: self.window_deaths.supersessions,
                retirements: self.window_deaths.retirements,
            });
        }
        self.receipt.check().map_err(AgingReportError::Receipt)
    }

    pub fn serialize(&self) -> Result<Value, AgingReportError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|e| AgingReportError::Shape(e.to_string()))
    }

    pub fn result_digest(report: &Value) -> Result<String, AgingReportError> {
        let mut value = report.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| AgingReportError::Shape("report is not an object".to_string()))?;
        object.remove("receipt");
        object.remove("checkpoint_digest");
        if let Some(envelope) = object.get_mut("envelope").and_then(Value::as_object_mut) {
            envelope.remove("peaks");
        }
        protocol_digest(AGING_RESULT_DIGEST_PROTOCOL, &value)
            .map_err(|e| AgingReportError::Shape(e.to_string()))
    }
}

pub fn parse_aging_report(value: &Value) -> Result<AgingReport, AgingReportError> {
    let report =
        AgingReport::deserialize(value).map_err(|e| AgingReportError::Shape(e.to_string()))?;
    report.validate()?;
    let again =
        serde_json::to_value(&report).map_err(|e| AgingReportError::Shape(e.to_string()))?;
    if again != *value {
        return Err(AgingReportError::Lossy);
    }
    Ok(report)
}

debug_display!(
    CheckpointRefused,
    RestoreRefused,
    Unenumerated,
    PrefixRefused,
    AgingReportError
);
