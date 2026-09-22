//! Fault episodes, cut coverage, lost-reply effects, and bounded liveness for
//! Suite C fault campaigns. The runner injects faults, reads barriers, and
//! drives episodes; this module decides what those observations prove.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint::StoreFamily;
use crate::manifest::{Cut, CutOutcome, CutReceipt};
use crate::statistics::LivenessBounds;
use context_core::canonical_json::protocol_digest;

pub const FAULT_REPORT_SCHEMA: &str = "eval-suite-c-fault-report/v1";
const FAULT_RESULT_DIGEST_PROTOCOL: &str = "eval-suite-c-fault-report-result/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEpisodeFault {
    LoseLocalCommitReply,
    LoseAcknowledgementReply,
    LoseAcknowledgementReplyAndCancel,
    AcknowledgeInsideLocalTransaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationFaultKind {
    LoseLocalCommitReply,
    LoseLocalCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactIngestFaultKind {
    Write,
    FileSync,
    ReservationCommit,
    Rename,
    AfterDirectorySync,
    TakeoverBeforeCleanupUnlink,
    AfterEvents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactDeletionFaultKind {
    IntentAppend,
    IntentStorageExhausted,
    BeforeCommit,
    AfterCommit,
    Unlink,
    UnlinkStorageExhausted,
}

/// One variant of a fault enum, hook, gate, lock holder, or kill that exists
/// at HEAD. Nothing else is a fault a campaign may claim to have run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FaultAction {
    SearchEpisode { fault: SearchEpisodeFault },
    EmbeddingPublication { fault: PublicationFaultKind },
    HeldPublication,
    ArtifactIngest { fault: ArtifactIngestFaultKind },
    ArtifactDeletion { fault: ArtifactDeletionFaultKind },
    ExternalLockHolder,
    ProcessKill { cut: String },
    CorruptQuiescentFile,
}

impl FaultAction {
    /// The heal each seam permits. A CAS storage failure that is not capacity
    /// exhaustion latches artifact ingestion closed until the store reopens,
    /// so every ingest fault and the EIO deletion faults heal by reopen.
    pub fn heal(&self) -> Heal {
        match self {
            Self::SearchEpisode { .. } | Self::EmbeddingPublication { .. } => Heal::Consumed,
            Self::ArtifactIngest { .. } => Heal::Reopen,
            Self::ArtifactDeletion { fault } => match fault {
                ArtifactDeletionFaultKind::IntentAppend | ArtifactDeletionFaultKind::Unlink => {
                    Heal::Reopen
                }
                ArtifactDeletionFaultKind::IntentStorageExhausted
                | ArtifactDeletionFaultKind::BeforeCommit
                | ArtifactDeletionFaultKind::AfterCommit
                | ArtifactDeletionFaultKind::UnlinkStorageExhausted => Heal::Consumed,
            },
            Self::HeldPublication | Self::ExternalLockHolder => Heal::Released,
            Self::ProcessKill { .. } | Self::CorruptQuiescentFile => Heal::Reopen,
        }
    }

    pub fn is_kill(&self) -> bool {
        matches!(self, Self::ProcessKill { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Heal {
    Consumed,
    Released,
    Reopen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultScope {
    pub store: StoreFamily,
    pub operation: String,
}

/// The only crash a `TestBinaryChild` kill proves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillLabel {
    pub crash_model: String,
    pub page_cache_intact: bool,
    pub killed_process: String,
}

pub const APPLICATION_CRASH: &str = "application_crash";
pub const TEST_BINARY_CHILD: &str = "test_binary_child";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultEpisode {
    pub id: String,
    pub trigger_step: u32,
    pub scope: FaultScope,
    pub action: FaultAction,
    pub heal: Heal,
    pub layer_contract: String,
    pub kill: Option<KillLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpisodeRefused {
    HealMismatch {
        id: String,
        declared: Heal,
        required: Heal,
    },
    CrashModelNotProved {
        id: String,
        crash_model: String,
    },
    KilledProcessNotProved {
        id: String,
        killed_process: String,
    },
    KillLabelMissing {
        id: String,
    },
    KillLabelOnNonKill {
        id: String,
    },
    EmptyLayerContract {
        id: String,
    },
    DuplicateEpisode {
        id: String,
    },
}

impl FaultEpisode {
    pub fn validate(&self) -> Result<(), EpisodeRefused> {
        let id = self.id.clone();
        let required = self.action.heal();
        if self.heal != required {
            return Err(EpisodeRefused::HealMismatch {
                id,
                declared: self.heal,
                required,
            });
        }
        if self.layer_contract.trim().is_empty() {
            return Err(EpisodeRefused::EmptyLayerContract { id });
        }
        match (&self.kill, self.action.is_kill()) {
            (None, true) => Err(EpisodeRefused::KillLabelMissing { id }),
            (Some(_), false) => Err(EpisodeRefused::KillLabelOnNonKill { id }),
            (None, false) => Ok(()),
            (Some(label), true) => {
                if label.crash_model != APPLICATION_CRASH || !label.page_cache_intact {
                    return Err(EpisodeRefused::CrashModelNotProved {
                        id,
                        crash_model: label.crash_model.clone(),
                    });
                }
                if label.killed_process != TEST_BINARY_CHILD {
                    return Err(EpisodeRefused::KilledProcessNotProved {
                        id,
                        killed_process: label.killed_process.clone(),
                    });
                }
                Ok(())
            }
        }
    }
}

pub fn validate_episodes(episodes: &[FaultEpisode]) -> Result<(), EpisodeRefused> {
    let mut seen = BTreeSet::new();
    for episode in episodes {
        episode.validate()?;
        if !seen.insert(episode.id.as_str()) {
            return Err(EpisodeRefused::DuplicateEpisode {
                id: episode.id.clone(),
            });
        }
    }
    Ok(())
}

/// The barrier line a killed child printed at its cut, read before the kill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarrierReceipt {
    pub episode: String,
    pub cut: String,
    pub pid: u32,
    pub line: String,
    pub signal: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarrierRefused {
    LineDoesNotNameCut { episode: String, line: String },
    ExitedWithStatus { episode: String },
}

impl BarrierReceipt {
    pub fn validate(&self) -> Result<(), BarrierRefused> {
        if !self.line.trim_end().ends_with(&self.cut) {
            return Err(BarrierRefused::LineDoesNotNameCut {
                episode: self.episode.clone(),
                line: self.line.clone(),
            });
        }
        if self.signal == 0 {
            return Err(BarrierRefused::ExitedWithStatus {
                episode: self.episode.clone(),
            });
        }
        Ok(())
    }
}

/// Every cut a campaign declares must be receipted at least once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutCoverage {
    pub declared: BTreeSet<String>,
    pub receipted: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageRefused {
    UndeclaredCut { cut: String },
    IncompleteCoverage { missing: BTreeSet<String> },
}

impl CutCoverage {
    pub fn declare(&mut self, cut: impl Into<String>) {
        self.declared.insert(cut.into());
    }

    pub fn receipt(&mut self, cut: &str) -> Result<(), CoverageRefused> {
        if !self.declared.contains(cut) {
            return Err(CoverageRefused::UndeclaredCut {
                cut: cut.to_string(),
            });
        }
        *self.receipted.entry(cut.to_string()).or_insert(0) += 1;
        Ok(())
    }

    pub fn verdict(&self) -> Result<(), CoverageRefused> {
        let missing: BTreeSet<String> = self
            .declared
            .iter()
            .filter(|cut| self.receipted.get(*cut).is_none_or(|n| *n == 0))
            .cloned()
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CoverageRefused::IncompleteCoverage { missing })
        }
    }
}

/// Oracle checkpoints resolve to runner receipts: a checkpoint the runner
/// receipted is `Reached`, every other declared one is `NotReached`.
pub fn cut_receipts(declared: &[Cut], receipted: &BTreeMap<Cut, u64>) -> Vec<CutReceipt> {
    declared
        .iter()
        .map(|cut| CutReceipt {
            cut: *cut,
            outcome: if receipted.get(cut).is_some_and(|n| *n > 0) {
                CutOutcome::Reached
            } else {
                CutOutcome::NotReached
            },
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    Applied,
    NotApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expected {
    Exactly { state: EffectState },
    OneOf { states: BTreeSet<EffectState> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutcome {
    Applied,
    NotApplied,
    Unknown,
}

/// One effect identity's counts and what the oracle may expect of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub attempted: u64,
    pub observed: u64,
    pub acknowledged: u64,
    pub reply_lost: bool,
    pub read_back: bool,
    pub expected: Expected,
    pub outcome: EffectOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLedger {
    pub effects: BTreeMap<String, Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectRefused {
    UnknownIdentity {
        identity: String,
    },
    BoundsViolated {
        identity: String,
        attempted: u64,
        observed: u64,
        acknowledged: u64,
    },
    PrematureSuccess {
        identity: String,
    },
    ExpectationCollapsedWithoutReadBack {
        identity: String,
    },
}

impl EffectLedger {
    pub fn attempt(&mut self, identity: &str) -> &mut Effect {
        let effect = self
            .effects
            .entry(identity.to_string())
            .or_insert_with(|| Effect {
                attempted: 0,
                observed: 0,
                acknowledged: 0,
                reply_lost: false,
                read_back: false,
                expected: Expected::Exactly {
                    state: EffectState::Applied,
                },
                outcome: EffectOutcome::Applied,
            });
        effect.attempted += 1;
        effect
    }

    fn effect(&mut self, identity: &str) -> Result<&mut Effect, EffectRefused> {
        self.effects
            .get_mut(identity)
            .ok_or_else(|| EffectRefused::UnknownIdentity {
                identity: identity.to_string(),
            })
    }

    pub fn observe(&mut self, identity: &str) -> Result<(), EffectRefused> {
        self.effect(identity)?.observed += 1;
        Ok(())
    }

    pub fn acknowledge(&mut self, identity: &str) -> Result<(), EffectRefused> {
        let effect = self.effect(identity)?;
        effect.observed += 1;
        effect.acknowledged += 1;
        Ok(())
    }

    /// The reply was lost: the oracle expects either state and records nothing
    /// stronger than `Unknown` until a read-back names one.
    pub fn lose_reply(&mut self, identity: &str) -> Result<(), EffectRefused> {
        let effect = self.effect(identity)?;
        effect.reply_lost = true;
        effect.read_back = false;
        effect.expected = Expected::OneOf {
            states: [EffectState::Applied, EffectState::NotApplied]
                .into_iter()
                .collect(),
        };
        effect.outcome = EffectOutcome::Unknown;
        Ok(())
    }

    /// A durable read-back by identity collapses the admissible set. A
    /// read-back that finds the effect applied is an observation of it, the
    /// only one a lost reply leaves.
    pub fn read_back(&mut self, identity: &str, state: EffectState) -> Result<(), EffectRefused> {
        let effect = self.effect(identity)?;
        effect.read_back = true;
        effect.expected = Expected::Exactly { state };
        effect.outcome = match state {
            EffectState::Applied => {
                effect.observed = effect.observed.max(1).min(effect.attempted);
                EffectOutcome::Applied
            }
            EffectState::NotApplied => EffectOutcome::NotApplied,
        };
        Ok(())
    }

    pub fn validate(&self) -> Result<(), EffectRefused> {
        for (identity, effect) in &self.effects {
            let identity = identity.clone();
            if !(effect.acknowledged <= effect.observed && effect.observed <= effect.attempted) {
                return Err(EffectRefused::BoundsViolated {
                    identity,
                    attempted: effect.attempted,
                    observed: effect.observed,
                    acknowledged: effect.acknowledged,
                });
            }
            if effect.reply_lost && !effect.read_back {
                if effect.outcome != EffectOutcome::Unknown {
                    return Err(EffectRefused::PrematureSuccess { identity });
                }
                match &effect.expected {
                    Expected::OneOf { states } if states.len() >= 2 => {}
                    _ => {
                        return Err(EffectRefused::ExpectationCollapsedWithoutReadBack {
                            identity,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    pub fn unknown(&self) -> BTreeSet<String> {
        self.effects
            .iter()
            .filter(|(_, e)| e.outcome == EffectOutcome::Unknown)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Refusals the production code makes on purpose; a campaign records them,
/// never counts them as safety failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedRefusal {
    R11DeletionBearingCatchUp,
    R24ReceiptQuotaExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedRefusal {
    pub episode: String,
    pub refusal: ExpectedRefusal,
    pub production_error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    CatchUpEpisodes,
    EmbeddingPasses,
    MaterializationEpisodes,
    ReviewerCoordinatorPasses,
}

impl Lane {
    pub const ALL: [Lane; 4] = [
        Lane::CatchUpEpisodes,
        Lane::EmbeddingPasses,
        Lane::MaterializationEpisodes,
        Lane::ReviewerCoordinatorPasses,
    ];

    pub fn bound(self, bounds: &LivenessBounds) -> u64 {
        match self {
            Lane::CatchUpEpisodes => bounds.catch_up_episodes,
            Lane::EmbeddingPasses => bounds.embedding_passes,
            Lane::MaterializationEpisodes => bounds.materialization_episodes,
            Lane::ReviewerCoordinatorPasses => bounds.reviewer_coordinator_passes,
        }
    }
}

/// Progress of one lane driven in its own unit up to its bound: the step at
/// which the predicate first held, the first step after that at which it did
/// not, whether it held at the bound, how much fresh work the window fed the
/// lane, and the block that stopped it if it never held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneProgress {
    pub bound: u64,
    pub steps: u64,
    pub met_at: Option<u64>,
    pub stalled_at: Option<u64>,
    pub holds_at_bound: bool,
    pub fresh_commits: u64,
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthyCore {
    pub families: BTreeSet<StoreFamily>,
    pub lanes: BTreeSet<Lane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivenessReport {
    pub core: HealthyCore,
    pub outside_core: BTreeSet<String>,
    pub armed_at_bound: BTreeSet<String>,
    pub lanes: BTreeMap<Lane, LaneProgress>,
    pub permanent_stalls: Vec<RecordedRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivenessRefused {
    LaneNotDriven {
        lane: Lane,
    },
    BoundMismatch {
        lane: Lane,
        declared: u64,
        profile: u64,
    },
    FaultHealed {
        episode: String,
    },
    ArmedInsideCore {
        episode: String,
    },
    LivenessUnmet {
        lane: Lane,
        progress_at_bound: u64,
        blocked: Option<String>,
    },
}

impl LivenessReport {
    /// Every in-core lane was driven to its profile bound, met its target at
    /// some step and again at the bound, and every outside-core fault was
    /// still armed when the bound was reached.
    pub fn verdict(&self, bounds: &LivenessBounds) -> Result<(), LivenessRefused> {
        for episode in &self.outside_core {
            if !self.armed_at_bound.contains(episode) {
                return Err(LivenessRefused::FaultHealed {
                    episode: episode.clone(),
                });
            }
        }
        for episode in &self.armed_at_bound {
            if !self.outside_core.contains(episode) {
                return Err(LivenessRefused::ArmedInsideCore {
                    episode: episode.clone(),
                });
            }
        }
        for lane in &self.core.lanes {
            let progress = self
                .lanes
                .get(lane)
                .ok_or(LivenessRefused::LaneNotDriven { lane: *lane })?;
            let profile = lane.bound(bounds);
            if progress.bound != profile {
                return Err(LivenessRefused::BoundMismatch {
                    lane: *lane,
                    declared: progress.bound,
                    profile,
                });
            }
            let met = progress
                .met_at
                .is_some_and(|k| k <= progress.bound && progress.holds_at_bound)
                && progress.stalled_at.is_none();
            if !met || progress.steps < progress.bound {
                return Err(LivenessRefused::LivenessUnmet {
                    lane: *lane,
                    progress_at_bound: progress.steps,
                    blocked: progress.blocked.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultReport {
    pub schema: String,
    pub eval_run_id: String,
    pub profile_digest: String,
    pub claim_boundary: crate::ClaimBoundary,
    pub episodes: Vec<FaultEpisode>,
    pub barriers: Vec<BarrierReceipt>,
    pub cuts: Vec<CutReceipt>,
    pub coverage: CutCoverage,
    pub effects: EffectLedger,
    pub expected_refusals: Vec<RecordedRefusal>,
    pub safety_checks_while_armed: u64,
    pub liveness: Option<LivenessReport>,
    pub markers: BTreeSet<String>,
    pub envelope: crate::Envelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultReportError {
    SchemaMismatch { found: String },
    Episode(EpisodeRefused),
    Barrier(BarrierRefused),
    Coverage(CoverageRefused),
    Effect(EffectRefused),
    Liveness(LivenessRefused),
    KillWithoutBarrier { episode: String },
    SafetyNeverChecked,
    Shape(String),
    Lossy,
}

impl FaultReport {
    pub fn validate(&self, bounds: &LivenessBounds) -> Result<(), FaultReportError> {
        if self.schema != FAULT_REPORT_SCHEMA {
            return Err(FaultReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        validate_episodes(&self.episodes).map_err(FaultReportError::Episode)?;
        for barrier in &self.barriers {
            barrier.validate().map_err(FaultReportError::Barrier)?;
        }
        for episode in self.episodes.iter().filter(|e| e.action.is_kill()) {
            if !self.barriers.iter().any(|b| b.episode == episode.id) {
                return Err(FaultReportError::KillWithoutBarrier {
                    episode: episode.id.clone(),
                });
            }
        }
        self.coverage
            .verdict()
            .map_err(FaultReportError::Coverage)?;
        self.effects.validate().map_err(FaultReportError::Effect)?;
        if self.safety_checks_while_armed == 0 {
            return Err(FaultReportError::SafetyNeverChecked);
        }
        if let Some(liveness) = &self.liveness {
            liveness
                .verdict(bounds)
                .map_err(FaultReportError::Liveness)?;
        }
        Ok(())
    }

    pub fn serialize(&self, bounds: &LivenessBounds) -> Result<Value, FaultReportError> {
        self.validate(bounds)?;
        serde_json::to_value(self).map_err(|e| FaultReportError::Shape(e.to_string()))
    }

    pub fn result_digest(report: &Value) -> Result<String, FaultReportError> {
        let mut value = report.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| FaultReportError::Shape("report is not an object".to_string()))?;
        if let Some(barriers) = object.get_mut("barriers").and_then(Value::as_array_mut) {
            for barrier in barriers.iter_mut().filter_map(Value::as_object_mut) {
                barrier.remove("pid");
            }
        }
        if let Some(envelope) = object.get_mut("envelope").and_then(Value::as_object_mut) {
            envelope.remove("peaks");
        }
        protocol_digest(FAULT_RESULT_DIGEST_PROTOCOL, &value)
            .map_err(|e| FaultReportError::Shape(e.to_string()))
    }
}

pub fn parse_fault_report(
    value: &Value,
    bounds: &LivenessBounds,
) -> Result<FaultReport, FaultReportError> {
    let report =
        FaultReport::deserialize(value).map_err(|e| FaultReportError::Shape(e.to_string()))?;
    report.validate(bounds)?;
    let again =
        serde_json::to_value(&report).map_err(|e| FaultReportError::Shape(e.to_string()))?;
    if again != *value {
        return Err(FaultReportError::Lossy);
    }
    Ok(report)
}

debug_display!(
    EpisodeRefused,
    BarrierRefused,
    CoverageRefused,
    EffectRefused,
    LivenessRefused,
    FaultReportError
);
