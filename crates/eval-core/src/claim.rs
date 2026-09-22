//! The claim class a report may carry. A generated world says nothing about
//! real repositories; `transfer` needs a real-history anchor set that meets a
//! frozen, approved criterion, and the pilot alone never derives it.

use std::collections::BTreeSet;

use context_core::canonical_json::is_lower_hex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimClass {
    GeneratedPhase1,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldProvenance {
    Generated,
    RealHistory,
}

/// `Pilot` populates the pilot and calibrates the generator; only a
/// `Transfer` set can support a transfer claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorRole {
    Pilot,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorVerdict {
    Valid,
    Residue,
    CutoffInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorTask {
    pub id: String,
    /// The project family the task comes from (`cargo`, `tokio`, `django`).
    pub family: String,
    pub verdict: AnchorVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorSet {
    pub role: AnchorRole,
    pub tasks: Vec<AnchorTask>,
}

/// The frozen rule a transfer claim must meet, approved before any outcome
/// and digested with the analysis family. Its clauses are the only thing
/// that can turn an anchor set into transfer evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferCriterion {
    pub approved_by: String,
    pub approved_at_run_id: String,
    pub min_valid_tasks: u32,
    pub required_families: BTreeSet<String>,
}

impl TransferCriterion {
    /// A criterion nobody approved, or one every anchor set would meet, is
    /// not a criterion; both faults are named when both hold. The approving
    /// run is an `eval-run-id` (64 lowercase hex), not any text; a blank
    /// family is no family, so requiring one is no floor.
    pub fn unmet(&self) -> Vec<UnmetClause> {
        let mut unmet = Vec::new();
        if self.approved_by.trim().is_empty() || !is_lower_hex(&self.approved_at_run_id, 64) {
            unmet.push(UnmetClause::CriterionNotApproved);
        }
        if self.min_valid_tasks == 0
            || self.required_families.is_empty()
            || self.required_families.contains("")
        {
            unmet.push(UnmetClause::CriterionHasNoFloor);
        }
        unmet
    }

    pub fn validate(&self) -> Result<(), UnmetClause> {
        self.unmet().into_iter().next().map_or(Ok(()), Err)
    }
}

/// One reason a report is not `transfer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "clause", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnmetClause {
    GeneratedWorld,
    NoAnchorSet,
    AnchorSetIsPilot,
    AnchorTaskNotValid,
    EmptyAnchorTaskId,
    /// A task from no named family proves no family and is not a task.
    EmptyAnchorTaskFamily,
    /// One ID listed twice is one task, whatever its verdicts.
    DuplicateAnchorTask {
        id: String,
    },
    NoTransferCriterion,
    CriterionNotApproved,
    CriterionHasNoFloor,
    TooFewValidTasks {
        required: u32,
        valid: u32,
    },
    FamilyMissing {
        family: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDerivation {
    pub class: ClaimClass,
    /// Every clause that fails, in declaration order; empty exactly when the
    /// class is `transfer`.
    pub unmet: Vec<UnmetClause>,
    /// Anchor tasks with a non-`valid` verdict, excluded from any claim.
    pub skipped: Vec<String>,
}

/// Derives the class from what is present, never from what a report says.
/// `transfer` needs a real-history world, a `transfer`-role anchor set with
/// every task valid, and an approved criterion whose every clause holds;
/// anything less is `generated_phase1` with each failing clause named.
pub fn derive_claim_class(
    provenance: WorldProvenance,
    anchor_set: Option<&AnchorSet>,
    criterion: Option<&TransferCriterion>,
) -> ClaimDerivation {
    let mut unmet = Vec::new();
    if provenance == WorldProvenance::Generated {
        unmet.push(UnmetClause::GeneratedWorld);
    }
    let (valid, skipped): (Vec<&AnchorTask>, Vec<&AnchorTask>) = anchor_set
        .into_iter()
        .flat_map(|set| set.tasks.iter())
        .partition(|task| task.verdict == AnchorVerdict::Valid);
    // The floor counts tasks, not list entries: a repeated, blank, or
    // family-less ID is one task or none, so a padded list cannot meet it.
    let valid: Vec<&AnchorTask> = {
        let mut seen = BTreeSet::new();
        valid
            .into_iter()
            .filter(|task| {
                !task.id.is_empty() && !task.family.is_empty() && seen.insert(task.id.as_str())
            })
            .collect()
    };
    match anchor_set {
        None => unmet.push(UnmetClause::NoAnchorSet),
        Some(set) => {
            if set.role == AnchorRole::Pilot {
                unmet.push(UnmetClause::AnchorSetIsPilot);
            }
            if !skipped.is_empty() {
                unmet.push(UnmetClause::AnchorTaskNotValid);
            }
            if set.tasks.iter().any(|task| task.id.is_empty()) {
                unmet.push(UnmetClause::EmptyAnchorTaskId);
            }
            if set.tasks.iter().any(|task| task.family.is_empty()) {
                unmet.push(UnmetClause::EmptyAnchorTaskFamily);
            }
            let mut ids = BTreeSet::new();
            let mut duplicates = BTreeSet::new();
            for task in &set.tasks {
                if !ids.insert(task.id.as_str()) {
                    duplicates.insert(task.id.as_str());
                }
            }
            unmet.extend(
                duplicates
                    .into_iter()
                    .map(|id| UnmetClause::DuplicateAnchorTask { id: id.to_string() }),
            );
        }
    }
    match criterion {
        None => unmet.push(UnmetClause::NoTransferCriterion),
        Some(criterion) => {
            unmet.extend(criterion.unmet());
            let count = u32::try_from(valid.len()).unwrap_or(u32::MAX);
            if count < criterion.min_valid_tasks {
                unmet.push(UnmetClause::TooFewValidTasks {
                    required: criterion.min_valid_tasks,
                    valid: count,
                });
            }
            let families: BTreeSet<&str> = valid.iter().map(|t| t.family.as_str()).collect();
            for family in &criterion.required_families {
                if !families.contains(family.as_str()) {
                    unmet.push(UnmetClause::FamilyMissing {
                        family: family.clone(),
                    });
                }
            }
        }
    }
    ClaimDerivation {
        class: if unmet.is_empty() {
            ClaimClass::Transfer
        } else {
            ClaimClass::GeneratedPhase1
        },
        unmet,
        skipped: skipped.into_iter().map(|t| t.id.clone()).collect(),
    }
}
