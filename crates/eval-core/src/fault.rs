//! Fault episodes, cut coverage, lost-reply effects, and bounded liveness for
//! Suite C fault campaigns. The runner injects faults, reads barriers, and
//! drives episodes; this module decides what those observations prove.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint::StoreFamily;
use crate::manifest::{Cut, CutOutcome, CutReceipt, ResourceLimits};
use crate::statistics::LivenessBounds;
use context_core::canonical_json::{
    ContractError, canonical_json_encode, is_lower_hex, protocol_digest,
};

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

/// `claim_sources::EpisodeFault`: the claim materializer's acknowledgement seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationFaultKind {
    LoseAcknowledgementReply,
    SkipAcknowledgement,
    FailAcknowledgement,
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
/// `ExpectedRefusal` injects no fault: the runner drives production into a
/// refusal it makes on purpose (a deletion-bearing catch-up window, a receipt
/// charge at the quota) and records it as expected rather than as a failure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FaultAction {
    SearchEpisode { fault: SearchEpisodeFault },
    EmbeddingPublication { fault: PublicationFaultKind },
    HeldPublication,
    ClaimMaterialization { fault: MaterializationFaultKind },
    ArtifactIngest { fault: ArtifactIngestFaultKind },
    ArtifactDeletion { fault: ArtifactDeletionFaultKind },
    ExternalLockHolder,
    ProcessKill { cut: String },
    CorruptQuiescentFile,
    ExpectedRefusal { refusal: ExpectedRefusal },
}

impl FaultAction {
    /// `ReservationCommit` and `AfterEvents` abort a SQLite transaction and are
    /// consumed; the other ingest faults latch CAS ingestion closed until reopen.
    /// R11 clears only when a reopen rebuilds the projection; R24's receipt
    /// charges are retained for the store incarnation, so nothing heals it.
    pub fn heal(&self) -> Heal {
        match self {
            Self::SearchEpisode { .. }
            | Self::EmbeddingPublication { .. }
            | Self::ClaimMaterialization { .. } => Heal::Consumed,
            Self::ArtifactIngest { fault } => match fault {
                ArtifactIngestFaultKind::ReservationCommit
                | ArtifactIngestFaultKind::AfterEvents => Heal::Consumed,
                ArtifactIngestFaultKind::Write
                | ArtifactIngestFaultKind::FileSync
                | ArtifactIngestFaultKind::Rename
                | ArtifactIngestFaultKind::AfterDirectorySync
                | ArtifactIngestFaultKind::TakeoverBeforeCleanupUnlink => Heal::Reopen,
            },
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
            Self::ExpectedRefusal { refusal } => match refusal {
                ExpectedRefusal::R11DeletionBearingCatchUp => Heal::Reopen,
                ExpectedRefusal::R24ReceiptQuotaExhausted => Heal::Permanent,
            },
        }
    }

    pub fn is_kill(&self) -> bool {
        matches!(self, Self::ProcessKill { .. })
    }

    /// The action leaves an operation's outcome unknown to its caller: the
    /// work may have committed while the reply said otherwise. `LoseLocalCommit`
    /// rolls back and `SkipAcknowledgement` never acknowledges, which are known.
    pub fn loses_reply(&self) -> bool {
        match self {
            Self::SearchEpisode { fault } => {
                *fault != SearchEpisodeFault::AcknowledgeInsideLocalTransaction
            }
            Self::EmbeddingPublication { fault } => {
                *fault == PublicationFaultKind::LoseLocalCommitReply
            }
            Self::ClaimMaterialization { fault } => {
                *fault != MaterializationFaultKind::SkipAcknowledgement
            }
            Self::HeldPublication
            | Self::ArtifactIngest { .. }
            | Self::ArtifactDeletion { .. }
            | Self::ExternalLockHolder
            | Self::ProcessKill { .. }
            | Self::CorruptQuiescentFile
            | Self::ExpectedRefusal { .. } => false,
        }
    }

    /// The store the action's seam lives in: catch-up and publication write the
    /// search projection, the CAS and the materializer's outbox are the kernel,
    /// R11 is the projection's refusal and R24 the memory store's. A lock
    /// holder, a kill, and a corrupted file name their own store.
    pub fn family(&self) -> Option<StoreFamily> {
        match self {
            Self::SearchEpisode { .. }
            | Self::EmbeddingPublication { .. }
            | Self::HeldPublication => Some(StoreFamily::SearchProjection),
            Self::ClaimMaterialization { .. }
            | Self::ArtifactIngest { .. }
            | Self::ArtifactDeletion { .. } => Some(StoreFamily::Kernel),
            Self::ExpectedRefusal { refusal } => Some(match refusal {
                ExpectedRefusal::R11DeletionBearingCatchUp => StoreFamily::SearchProjection,
                ExpectedRefusal::R24ReceiptQuotaExhausted => StoreFamily::Memory,
            }),
            Self::ExternalLockHolder | Self::ProcessKill { .. } | Self::CorruptQuiescentFile => {
                None
            }
        }
    }

    /// The cut a kill is declared at; `None` for every other action.
    pub fn kill_cut(&self) -> Option<&str> {
        match self {
            Self::ProcessKill { cut } => Some(cut),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Heal {
    Consumed,
    Released,
    Reopen,
    /// Nothing in the run heals it; the refusal stands for the store incarnation.
    Permanent,
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
/// The one signal the runner sends; nothing else is the kill the label describes.
pub const SIGKILL: i32 = 9;

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
    ScopeMismatch {
        id: String,
        declared: StoreFamily,
        required: StoreFamily,
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
    EmptyId,
    EmptyOperation {
        id: String,
    },
    DuplicateEpisode {
        id: String,
    },
}

impl FaultEpisode {
    pub fn validate(&self) -> Result<(), EpisodeRefused> {
        if crate::blank(&self.id) {
            return Err(EpisodeRefused::EmptyId);
        }
        let id = self.id.clone();
        if crate::blank(&self.scope.operation) {
            return Err(EpisodeRefused::EmptyOperation { id });
        }
        let required = self.action.heal();
        if self.heal != required {
            return Err(EpisodeRefused::HealMismatch {
                id,
                declared: self.heal,
                required,
            });
        }
        if let Some(required) = self.action.family()
            && self.scope.store != required
        {
            return Err(EpisodeRefused::ScopeMismatch {
                id,
                declared: self.scope.store,
                required,
            });
        }
        if crate::blank(&self.layer_contract) {
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
    LineDoesNotNameCut {
        episode: String,
        line: String,
    },
    ExitedWithStatus {
        episode: String,
    },
    NotSigkill {
        episode: String,
        signal: i32,
    },
    /// No spawned child has pid 0.
    NoPid {
        episode: String,
    },
}

impl BarrierReceipt {
    /// Barrier lines are `<prefix> <cut>`, so the cut must be the last token;
    /// a suffix match would let `unacknowledged` name `acknowledged`.
    pub fn validate(&self) -> Result<(), BarrierRefused> {
        if self.line.split_whitespace().next_back() != Some(self.cut.as_str()) {
            return Err(BarrierRefused::LineDoesNotNameCut {
                episode: self.episode.clone(),
                line: self.line.clone(),
            });
        }
        if self.pid == 0 {
            return Err(BarrierRefused::NoPid {
                episode: self.episode.clone(),
            });
        }
        if self.signal == 0 {
            return Err(BarrierRefused::ExitedWithStatus {
                episode: self.episode.clone(),
            });
        }
        if self.signal != SIGKILL {
            return Err(BarrierRefused::NotSigkill {
                episode: self.episode.clone(),
                signal: self.signal,
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

    /// A receipt for a cut nobody declared is refused whichever way it arrived.
    pub fn verdict(&self) -> Result<(), CoverageRefused> {
        if let Some(cut) = self
            .receipted
            .keys()
            .find(|cut| !self.declared.contains(*cut))
        {
            return Err(CoverageRefused::UndeclaredCut { cut: cut.clone() });
        }
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
    /// The episode whose fault lost this effect's reply; `None` when no reply
    /// was lost.
    pub lost_by: Option<String>,
    pub read_back: bool,
    pub expected: Expected,
    pub outcome: EffectOutcome,
}

impl Effect {
    pub fn reply_lost(&self) -> bool {
        self.lost_by.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLedger {
    pub effects: BTreeMap<String, Effect>,
}

impl Effect {
    fn admits(&self, state: EffectState) -> bool {
        if state == EffectState::NotApplied && self.observed > 0 {
            return false;
        }
        match &self.expected {
            Expected::Exactly { state: expected } => *expected == state,
            Expected::OneOf { states } => states.contains(&state),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectRefused {
    UnknownIdentity {
        identity: String,
    },
    /// An entry `attempt` never created: nothing was tried under this identity.
    NeverAttempted {
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
    ReadBackNotAdmissible {
        identity: String,
        state: EffectState,
    },
    /// The outcome or expectation is not the one the recorded events derive.
    OutcomeNotDerived {
        identity: String,
    },
    /// A lost reply was observed applied yet never read back; the observation
    /// is the read-back the ledger must record.
    ObservedWithoutReadBack {
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
                lost_by: None,
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
    /// The reply was lost to `episode`'s fault: the oracle expects either
    /// state and records nothing stronger than `Unknown` until a read-back
    /// names one.
    pub fn lose_reply(&mut self, identity: &str, episode: &str) -> Result<(), EffectRefused> {
        let effect = self.effect(identity)?;
        effect.lost_by = Some(episode.to_string());
        effect.read_back = false;
        effect.expected = Expected::OneOf {
            states: [EffectState::Applied, EffectState::NotApplied]
                .into_iter()
                .collect(),
        };
        effect.outcome = EffectOutcome::Unknown;
        Ok(())
    }

    /// Refuses states outside the admissible set, leaving the entry unchanged.
    /// An applied read-back is one observation; it never lowers a count the
    /// bounds check must still see.
    pub fn read_back(&mut self, identity: &str, state: EffectState) -> Result<(), EffectRefused> {
        let effect = self.effect(identity)?;
        if !effect.admits(state) {
            return Err(EffectRefused::ReadBackNotAdmissible {
                identity: identity.to_string(),
                state,
            });
        }
        effect.read_back = true;
        effect.expected = Expected::Exactly { state };
        effect.outcome = match state {
            EffectState::Applied => {
                effect.observed = effect.observed.max(1);
                EffectOutcome::Applied
            }
            EffectState::NotApplied => EffectOutcome::NotApplied,
        };
        Ok(())
    }

    pub fn validate(&self) -> Result<(), EffectRefused> {
        for (identity, effect) in &self.effects {
            let identity = identity.clone();
            if effect.attempted == 0 {
                return Err(EffectRefused::NeverAttempted { identity });
            }
            if !(effect.acknowledged <= effect.observed && effect.observed <= effect.attempted) {
                return Err(EffectRefused::BoundsViolated {
                    identity,
                    attempted: effect.attempted,
                    observed: effect.observed,
                    acknowledged: effect.acknowledged,
                });
            }
            if effect.observed > 0 && effect.outcome == EffectOutcome::NotApplied {
                return Err(EffectRefused::ReadBackNotAdmissible {
                    identity,
                    state: EffectState::NotApplied,
                });
            }
            if effect.reply_lost() && !effect.read_back {
                if effect.observed > 0 {
                    return Err(EffectRefused::ObservedWithoutReadBack { identity });
                }
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
                continue;
            }
            // Without a lost reply only `Applied` was ever admissible; after a
            // read-back the outcome is exactly the state it named.
            let derived = match &effect.expected {
                Expected::Exactly {
                    state: EffectState::Applied,
                } => effect.outcome == EffectOutcome::Applied,
                Expected::Exactly {
                    state: EffectState::NotApplied,
                } => effect.reply_lost() && effect.outcome == EffectOutcome::NotApplied,
                Expected::OneOf { .. } => false,
            };
            if !derived {
                return Err(EffectRefused::OutcomeNotDerived { identity });
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

impl ExpectedRefusal {
    /// The production variant whose text a record must carry as evidence.
    pub fn production_variant(self) -> &'static str {
        match self {
            Self::R11DeletionBearingCatchUp => "DeletionUnpropagated",
            Self::R24ReceiptQuotaExhausted => "MetadataQuota",
        }
    }
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
    NoOutsideCoreFault,
    EmptyHealthyCore,
}

impl LivenessReport {
    /// Every in-core lane was fed fresh work and driven to its profile bound,
    /// and its predicate held from `met_at` through that bound with no stall.
    /// At least one outside-core fault is declared, and each stayed armed.
    pub fn verdict(&self, bounds: &LivenessBounds) -> Result<(), LivenessRefused> {
        if self.core.families.is_empty() || self.core.lanes.is_empty() {
            return Err(LivenessRefused::EmptyHealthyCore);
        }
        if self.outside_core.is_empty() {
            return Err(LivenessRefused::NoOutsideCoreFault);
        }
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
                && progress.stalled_at.is_none()
                && progress.blocked.is_none()
                && progress.fresh_commits > 0;
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
    SchemaMismatch {
        found: String,
    },
    ClaimBoundaryMismatch,
    MalformedDigest {
        field: &'static str,
    },
    EnvelopeExceeded(crate::EnvelopeExceeded),
    /// The report's envelope bounds are not the approved profile's limits.
    EnvelopeDisagreesWithProfile,
    NoEpisode,
    UnregisteredMarker {
        marker: String,
    },
    Episode(EpisodeRefused),
    Barrier(BarrierRefused),
    Coverage(CoverageRefused),
    Effect(EffectRefused),
    Liveness(LivenessRefused),
    KillWithoutBarrier {
        episode: String,
        cut: String,
    },
    /// A barrier no kill episode declares at that cut.
    BarrierWithoutKill {
        episode: String,
        cut: String,
    },
    /// One kill, one child, one barrier.
    DuplicateBarrier {
        episode: String,
        cut: String,
    },
    /// One cut receipted twice; two outcomes for one checkpoint is no outcome.
    DuplicateCut {
        cut: Cut,
    },
    UnknownEpisode {
        episode: String,
    },
    /// An outside-core fault scoped to a family the healthy core names.
    CoreFamilyFaulted {
        episode: String,
        store: StoreFamily,
    },
    /// A one-shot fault is consumed or never fired; neither is armed at the bound.
    ConsumedFaultArmed {
        episode: String,
    },
    /// A recorded refusal whose production error does not name its variant.
    RefusalNotEvidenced {
        episode: String,
        refusal: ExpectedRefusal,
    },
    /// A recorded refusal whose episode is not an `expected_refusal` of that refusal.
    RefusalNotDeclared {
        episode: String,
    },
    /// An `expected_refusal` episode with no recorded refusal of its own.
    RefusalNotRecorded {
        episode: String,
    },
    /// Fewer lost replies in the ledger than episodes that lose one.
    LostReplyUnrecorded {
        episode: String,
    },
    /// An effect names an episode that loses no reply as the one that lost its.
    LostByNonLosingEpisode {
        identity: String,
        episode: String,
    },
    SafetyNeverChecked,
    Shape(String),
    NotCanonical(ContractError),
    Lossy,
}

impl FaultReport {
    /// `bounds` and `limits` are the approved profile's; the report may not
    /// supply its own.
    pub fn validate(
        &self,
        bounds: &LivenessBounds,
        limits: &ResourceLimits,
    ) -> Result<(), FaultReportError> {
        if self.schema != FAULT_REPORT_SCHEMA {
            return Err(FaultReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.claim_boundary != crate::ClaimBoundary::pinned() {
            return Err(FaultReportError::ClaimBoundaryMismatch);
        }
        for (field, digest) in [
            ("eval_run_id", &self.eval_run_id),
            ("profile_digest", &self.profile_digest),
        ] {
            if !is_lower_hex(digest, 64) {
                return Err(FaultReportError::MalformedDigest { field });
            }
        }
        if self.envelope.bounds != *limits {
            return Err(FaultReportError::EnvelopeDisagreesWithProfile);
        }
        self.envelope
            .check()
            .map_err(FaultReportError::EnvelopeExceeded)?;
        if self.episodes.is_empty() {
            return Err(FaultReportError::NoEpisode);
        }
        if let Some(marker) = self
            .markers
            .iter()
            .find(|marker| !crate::MARKERS.iter().any(|m| m.name == marker.as_str()))
        {
            return Err(FaultReportError::UnregisteredMarker {
                marker: marker.clone(),
            });
        }
        validate_episodes(&self.episodes).map_err(FaultReportError::Episode)?;
        for barrier in &self.barriers {
            barrier.validate().map_err(FaultReportError::Barrier)?;
        }
        for episode in &self.episodes {
            let Some(cut) = episode.action.kill_cut() else {
                continue;
            };
            if !self.coverage.declared.contains(cut) {
                return Err(FaultReportError::Coverage(CoverageRefused::UndeclaredCut {
                    cut: cut.to_string(),
                }));
            }
            if !self
                .barriers
                .iter()
                .any(|b| b.episode == episode.id && b.cut == cut)
            {
                return Err(FaultReportError::KillWithoutBarrier {
                    episode: episode.id.clone(),
                    cut: cut.to_string(),
                });
            }
        }
        let mut seen_barriers = BTreeSet::new();
        for barrier in &self.barriers {
            if !seen_barriers.insert((&barrier.episode, &barrier.cut)) {
                return Err(FaultReportError::DuplicateBarrier {
                    episode: barrier.episode.clone(),
                    cut: barrier.cut.clone(),
                });
            }
            if !self
                .episodes
                .iter()
                .any(|e| e.id == barrier.episode && e.action.kill_cut() == Some(&barrier.cut))
            {
                return Err(FaultReportError::BarrierWithoutKill {
                    episode: barrier.episode.clone(),
                    cut: barrier.cut.clone(),
                });
            }
        }
        let mut cuts = BTreeSet::new();
        if let Some(receipt) = self.cuts.iter().find(|r| !cuts.insert(r.cut)) {
            return Err(FaultReportError::DuplicateCut { cut: receipt.cut });
        }
        self.coverage
            .verdict()
            .map_err(FaultReportError::Coverage)?;
        self.effects.validate().map_err(FaultReportError::Effect)?;
        for (identity, effect) in &self.effects.effects {
            let Some(episode) = &effect.lost_by else {
                continue;
            };
            match self.episodes.iter().find(|e| &e.id == episode) {
                None => {
                    return Err(FaultReportError::UnknownEpisode {
                        episode: episode.clone(),
                    });
                }
                Some(e) if !e.action.loses_reply() => {
                    return Err(FaultReportError::LostByNonLosingEpisode {
                        identity: identity.clone(),
                        episode: episode.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        if let Some(episode) = self.episodes.iter().find(|e| {
            e.action.loses_reply()
                && !self
                    .effects
                    .effects
                    .values()
                    .any(|effect| effect.lost_by.as_deref() == Some(e.id.as_str()))
        }) {
            return Err(FaultReportError::LostReplyUnrecorded {
                episode: episode.id.clone(),
            });
        }
        let stalls = self.liveness.iter().flat_map(|l| &l.permanent_stalls);
        for recorded in self.expected_refusals.iter().chain(stalls) {
            if !self.episodes.iter().any(|e| e.id == recorded.episode) {
                return Err(FaultReportError::UnknownEpisode {
                    episode: recorded.episode.clone(),
                });
            }
            if !recorded
                .production_error
                .contains(recorded.refusal.production_variant())
            {
                return Err(FaultReportError::RefusalNotEvidenced {
                    episode: recorded.episode.clone(),
                    refusal: recorded.refusal,
                });
            }
        }
        // A refusal or stall recorded against any other episode would
        // attribute it to a fault that never ran.
        let stalls = self.liveness.iter().flat_map(|l| &l.permanent_stalls);
        for recorded in self.expected_refusals.iter().chain(stalls) {
            let declared = FaultAction::ExpectedRefusal {
                refusal: recorded.refusal,
            };
            if !self
                .episodes
                .iter()
                .any(|e| e.id == recorded.episode && e.action == declared)
            {
                return Err(FaultReportError::RefusalNotDeclared {
                    episode: recorded.episode.clone(),
                });
            }
        }
        // An expected-refusal episode with no recorded production error
        // claims a refusal the run never observed.
        for episode in &self.episodes {
            let FaultAction::ExpectedRefusal { refusal } = episode.action else {
                continue;
            };
            if !self
                .expected_refusals
                .iter()
                .any(|r| r.episode == episode.id && r.refusal == refusal)
            {
                return Err(FaultReportError::RefusalNotRecorded {
                    episode: episode.id.clone(),
                });
            }
        }
        if self.safety_checks_while_armed == 0 {
            return Err(FaultReportError::SafetyNeverChecked);
        }
        if let Some(liveness) = &self.liveness {
            for id in &liveness.outside_core {
                let Some(episode) = self.episodes.iter().find(|e| &e.id == id) else {
                    return Err(FaultReportError::UnknownEpisode {
                        episode: id.clone(),
                    });
                };
                if liveness.core.families.contains(&episode.scope.store) {
                    return Err(FaultReportError::CoreFamilyFaulted {
                        episode: id.clone(),
                        store: episode.scope.store,
                    });
                }
                if episode.action.heal() == Heal::Consumed {
                    return Err(FaultReportError::ConsumedFaultArmed {
                        episode: id.clone(),
                    });
                }
            }
            liveness
                .verdict(bounds)
                .map_err(FaultReportError::Liveness)?;
        }
        Ok(())
    }

    /// Digestible on both runtimes: no integer may leave the canonical safe
    /// range, or `result_digest` would refuse the value `validate` accepted.
    pub fn serialize(
        &self,
        bounds: &LivenessBounds,
        limits: &ResourceLimits,
    ) -> Result<Value, FaultReportError> {
        self.validate(bounds, limits)?;
        let value =
            serde_json::to_value(self).map_err(|e| FaultReportError::Shape(e.to_string()))?;
        canonical_json_encode(&value).map_err(FaultReportError::NotCanonical)?;
        Ok(value)
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
    limits: &ResourceLimits,
) -> Result<FaultReport, FaultReportError> {
    let report =
        FaultReport::deserialize(value).map_err(|e| FaultReportError::Shape(e.to_string()))?;
    report.validate(bounds, limits)?;
    canonical_json_encode(value).map_err(FaultReportError::NotCanonical)?;
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
