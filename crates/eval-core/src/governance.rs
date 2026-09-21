//! History-policy arms: raw, pruned, and structured histories over one pair
//! set, described so the runner can select the production component and the
//! report can hold every arm to the same tasks, evidence, and control.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{ContractError, is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};

use crate::event::EventId;
use crate::pairs::PairSet;

const PAIR_SET_DIGEST_PROTOCOL: &str = "eval-pair-set-digest/v1";

/// The digest that pins a governance record to one compiled pair set, whole:
/// two sets can share every task and evidence ID and differ in everything
/// else.
pub fn pair_set_digest(set: &PairSet) -> Result<String, ContractError> {
    protocol_digest(
        PAIR_SET_DIGEST_PROTOCOL,
        &serde_json::to_value(set).expect("serializes"),
    )
}

/// A descriptor. The runner executes the production component it names;
/// nothing here models what that component does to a history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryPolicy {
    Raw,
    /// `message_cleanup` applied to the aged history.
    Pruned,
    /// HistorySummarizer output in place of the raw segments it covers.
    Structured,
}

impl HistoryPolicy {
    pub const ALL: [Self; 3] = [Self::Raw, Self::Pruned, Self::Structured];

    /// The workspace path and symbol of the production component a
    /// descriptor selects: the orchestrator production runs (the cleanup
    /// slice with its admission gate and budgets; the summarizer firing that
    /// validates and publishes), not a primitive it drives. A test holds both
    /// to the tree, so a renamed component breaks the descriptor instead of
    /// the campaign.
    pub fn production_component(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Raw => None,
            Self::Pruned => Some(("crates/daemon/src/message_cleanup.rs", "pub fn run_slice(")),
            Self::Structured => Some((
                "crates/daemon/src/history_summarizer.rs",
                "pub async fn run_history_summarizer_firing",
            )),
        }
    }
}

/// One arm's own record. `absent_evidence` is evidence the policy removed:
/// the task stays in the arm and records a loss, so every arm keeps the same
/// denominator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmRecord {
    pub policy_version: String,
    pub absent_evidence: BTreeSet<EventId>,
}

/// Arms over one pair set and one fresh control. The set is pinned whole by
/// digest; task, evidence, and control identity are stated once so a
/// mismatch is named, and an arm cannot disagree with another; only its
/// policy version and its losses are its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceArms {
    pub control_run_id: String,
    pub pair_set_digest: String,
    pub task_ids: BTreeSet<String>,
    pub evidence_ids: BTreeSet<EventId>,
    pub arms: BTreeMap<HistoryPolicy, ArmRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmError {
    NotCanonical(ContractError),
    MalformedControlRun,
    /// The arms describe another pair set, or tasks or evidence the pair set
    /// does not have.
    PairSetMismatch {
        field: &'static str,
    },
    /// Every policy has an arm; a report that dropped one is not a
    /// governance experiment.
    MissingArm {
        policy: HistoryPolicy,
    },
    /// The reference arm removed nothing, by definition.
    RawArmLostEvidence {
        id: EventId,
    },
    EmptyPolicyVersion {
        arm: HistoryPolicy,
    },
    AbsentEvidenceUnknown {
        arm: HistoryPolicy,
        id: EventId,
    },
}

debug_display!(ArmError);

impl GovernanceArms {
    /// Checks the arms against the pair set they govern.
    pub fn validate(&self, set: &PairSet) -> Result<(), ArmError> {
        // The control is a run: a 64-hex `eval-run-id`, not any text.
        if !is_lower_hex(&self.control_run_id, 64) {
            return Err(ArmError::MalformedControlRun);
        }
        if self.pair_set_digest != pair_set_digest(set).map_err(ArmError::NotCanonical)? {
            return Err(ArmError::PairSetMismatch {
                field: "pair_set_digest",
            });
        }
        let tasks: BTreeSet<String> = set.pairs.iter().map(|p| p.task.id.clone()).collect();
        if self.task_ids != tasks {
            return Err(ArmError::PairSetMismatch { field: "task_ids" });
        }
        let evidence: BTreeSet<EventId> = set
            .pairs
            .iter()
            .flat_map(|p| p.task.evidence.iter().cloned())
            .collect();
        if self.evidence_ids != evidence {
            return Err(ArmError::PairSetMismatch {
                field: "evidence_ids",
            });
        }
        for policy in HistoryPolicy::ALL {
            if !self.arms.contains_key(&policy) {
                return Err(ArmError::MissingArm { policy });
            }
        }
        if let Some(id) = self.arms[&HistoryPolicy::Raw].absent_evidence.first() {
            return Err(ArmError::RawArmLostEvidence { id: id.clone() });
        }
        for (policy, arm) in &self.arms {
            if arm.policy_version.is_empty() {
                return Err(ArmError::EmptyPolicyVersion { arm: *policy });
            }
            if let Some(id) = arm.absent_evidence.difference(&self.evidence_ids).next() {
                return Err(ArmError::AbsentEvidenceUnknown {
                    arm: *policy,
                    id: id.clone(),
                });
            }
        }
        Ok(())
    }
}
