//! Real-history anchor tasks: a corpus of commit and issue identifiers, the
//! cutoff audit that keeps future knowledge out of a task, the
//! current-tree-only insufficiency proof, and the no-repository control that
//! marks a memorized task and excludes it from one provider's transfer
//! evidence.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};

use crate::campaign::Terminal;
use crate::claim::{AnchorRole, AnchorSet, AnchorTask, AnchorVerdict, TransferCriterion};
use crate::task::{HiddenOutcome, HiddenResults};

pub const ANCHOR_CORPUS_SCHEMA: &str = "eval-anchor-corpus/v1";
pub const ANCHOR_CORPUS_DIGEST_PROTOCOL: &str = "eval-anchor-corpus-digest/v1";
/// The pilot: eight Cargo, eight Tokio, four Django tasks.
pub const PILOT_COMPOSITION: [(Family, u32); 3] =
    [(Family::Cargo, 8), (Family::Tokio, 8), (Family::Django, 4)];
/// Tasks the time study prepares before the pilot's cost is committed to.
pub const TIME_STUDY_TASKS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Cargo,
    Tokio,
    Django,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Tokio => "tokio",
            Self::Django => "django",
        }
    }
}

/// One anchor task as the corpus persists it: identifiers only. The issue
/// and pull-request text is fetched at run time and never written into a
/// corpus, report, or witness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorEntry {
    pub id: String,
    pub family: Family,
    /// The repository's clone URL, whose license governs fetched source and
    /// generated workspaces.
    pub repository: String,
    pub license: String,
    /// The commit the task's snapshot is built at.
    pub base_sha: String,
    /// The commit that fixed the issue; strictly after the cutoff.
    pub fix_sha: String,
    pub issue: u64,
    pub pull_request: Option<u64>,
    /// The cutoff: nothing committed or filed after it may reach the task.
    #[serde(with = "crate::decimal")]
    pub cutoff_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorError {
    SchemaMismatch {
        found: String,
    },
    EmptyField {
        id: String,
        field: &'static str,
    },
    NotASha {
        id: String,
        field: &'static str,
    },
    /// A field carries text that reads like a statement, issue, or diff.
    TextPersisted {
        id: String,
        field: &'static str,
    },
    DuplicateId {
        id: String,
    },
    /// The id names clone and snapshot directories; it must be one plain path
    /// component to remain under the runner root.
    NotAPathComponent {
        id: String,
    },
    NotPilotComposition {
        found: BTreeMap<Family, u32>,
    },
}

debug_display!(AnchorError);

impl AnchorEntry {
    pub fn validate(&self) -> Result<(), AnchorError> {
        let id = || self.id.clone();
        for (field, text) in [
            ("id", &self.id),
            ("repository", &self.repository),
            ("license", &self.license),
        ] {
            if text.trim().is_empty() {
                return Err(AnchorError::EmptyField { id: id(), field });
            }
            if text.contains('\n') || text.split_whitespace().count() > 3 {
                return Err(AnchorError::TextPersisted { id: id(), field });
            }
        }
        for (field, sha) in [("base_sha", &self.base_sha), ("fix_sha", &self.fix_sha)] {
            if !is_lower_hex(sha, 40) {
                return Err(AnchorError::NotASha { id: id(), field });
            }
        }
        let plain = self
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !plain || self.id == "." || self.id == ".." {
            return Err(AnchorError::NotAPathComponent { id: id() });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorCorpus {
    pub schema: String,
    pub entries: Vec<AnchorEntry>,
}

impl AnchorCorpus {
    pub fn validate(&self) -> Result<(), AnchorError> {
        if self.schema != ANCHOR_CORPUS_SCHEMA {
            return Err(AnchorError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(entry.id.as_str()) {
                return Err(AnchorError::DuplicateId {
                    id: entry.id.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn composition(&self) -> BTreeMap<Family, u32> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts.entry(entry.family).or_insert(0) += 1;
        }
        counts
    }

    /// The pilot is exactly the planned composition; more or fewer tasks in
    /// any family is not the pilot.
    pub fn is_pilot(&self) -> Result<(), AnchorError> {
        let found = self.composition();
        if found == PILOT_COMPOSITION.into_iter().collect() {
            Ok(())
        } else {
            Err(AnchorError::NotPilotComposition { found })
        }
    }

    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("corpus serializes");
        protocol_digest(ANCHOR_CORPUS_DIGEST_PROTOCOL, &value).expect("corpus is canonical")
    }
}

/// How long one task took to prepare, measured, never estimated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    pub task: String,
    #[serde(with = "crate::decimal")]
    pub prepare_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Affordability {
    Affordable {
        #[serde(with = "crate::decimal")]
        projected_ms: u64,
    },
    /// The projected pilot exceeds the bound; the run stops here for the
    /// maintainer instead of shrinking the pilot on its own.
    StopForApproval {
        #[serde(with = "crate::decimal")]
        projected_ms: u64,
        #[serde(with = "crate::decimal")]
        bound_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeStudyRefused {
    WrongTaskCount { measured: usize },
    NotFromCorpus { task: String },
}

debug_display!(TimeStudyRefused);

/// Projects the pilot's preparation cost from `TIME_STUDY_TASKS` measured
/// preparations of corpus tasks, scaled to the whole pilot.
pub fn time_study(
    corpus: &AnchorCorpus,
    measured: &[Preparation],
    bound_ms: u64,
) -> Result<Affordability, TimeStudyRefused> {
    if measured.len() != TIME_STUDY_TASKS {
        return Err(TimeStudyRefused::WrongTaskCount {
            measured: measured.len(),
        });
    }
    if let Some(stranger) = measured
        .iter()
        .find(|p| !corpus.entries.iter().any(|e| e.id == p.task))
    {
        return Err(TimeStudyRefused::NotFromCorpus {
            task: stranger.task.clone(),
        });
    }
    let total: u64 = measured.iter().map(|p| p.prepare_ms).sum();
    let pilot: u64 = PILOT_COMPOSITION.iter().map(|(_, n)| u64::from(*n)).sum();
    let projected_ms = total.saturating_mul(pilot) / TIME_STUDY_TASKS as u64;
    Ok(if projected_ms <= bound_ms {
        Affordability::Affordable { projected_ms }
    } else {
        Affordability::StopForApproval {
            projected_ms,
            bound_ms,
        }
    })
}

/// What the snapshot builder established about one task's cutoff, from the
/// repository's own commit times and the issue's creation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutoffAudit {
    pub task: String,
    #[serde(with = "crate::decimal")]
    pub cutoff_ms: i64,
    #[serde(with = "crate::decimal")]
    pub base_committed_ms: i64,
    #[serde(with = "crate::decimal")]
    pub fix_committed_ms: i64,
    #[serde(with = "crate::decimal")]
    pub issue_created_ms: i64,
    /// The digest of the tree the snapshot holds.
    pub snapshot_digest: String,
    /// Whether the snapshot holds any path the fix commit added.
    pub fix_paths_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum CutoffRefused {
    BaseAfterCutoff,
    FixNotAfterCutoff,
    /// The issue was filed after the cutoff, so its text is future knowledge.
    IssueAfterCutoff,
    FutureContentInSnapshot,
    SnapshotDigestMissing,
}

debug_display!(CutoffRefused);

impl CutoffAudit {
    pub fn validate(&self) -> Result<(), CutoffRefused> {
        if self.snapshot_digest.is_empty() {
            return Err(CutoffRefused::SnapshotDigestMissing);
        }
        if self.base_committed_ms > self.cutoff_ms {
            return Err(CutoffRefused::BaseAfterCutoff);
        }
        if self.fix_committed_ms <= self.cutoff_ms {
            return Err(CutoffRefused::FixNotAfterCutoff);
        }
        if self.issue_created_ms > self.cutoff_ms {
            return Err(CutoffRefused::IssueAfterCutoff);
        }
        if self.fix_paths_present {
            return Err(CutoffRefused::FutureContentInSnapshot);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum InsufficiencyRefused {
    /// The tree at the cutoff already passes: there is nothing to fix.
    TreeAlreadyPasses,
    NothingExecuted,
    /// Some hidden test does not pass on the fix tree.
    ReferenceDoesNotPass,
}

debug_display!(InsufficiencyRefused);

/// The current-tree-only run: the hidden tests over the snapshot with no
/// agent, next to the same tests over the snapshot with the fix applied. A
/// recorded pair of results is the proof; a corpus row without one is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsufficiencyProof {
    pub task: String,
    pub hidden: HiddenResults,
    /// The hidden tests over the fix tree. A base-tree failure or error
    /// counts only when every test passes here, because then the runner's
    /// environment is shown to build and run them.
    pub reference: HiddenResults,
}

impl InsufficiencyProof {
    pub fn validate(&self) -> Result<(), InsufficiencyRefused> {
        if self.hidden.is_empty() {
            return Err(InsufficiencyRefused::NothingExecuted);
        }
        let reference_passes = self
            .hidden
            .keys()
            .all(|name| self.reference.get(name) == Some(&HiddenOutcome::Passed));
        if !reference_passes {
            return Err(InsufficiencyRefused::ReferenceDoesNotPass);
        }
        if self.hidden.values().all(|o| *o == HiddenOutcome::Passed) {
            return Err(InsufficiencyRefused::TreeAlreadyPasses);
        }
        Ok(())
    }
}

/// One provider and model under one tokenizer accounting profile; controls
/// and comparisons are keyed by it and never mixed across it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub provider: String,
    pub model: String,
    pub tokenizer_profile: String,
}

impl ProviderProfile {
    pub fn key(&self) -> String {
        format!(
            "{}/{}@{}",
            self.provider, self.model, self.tokenizer_profile
        )
    }
}

/// The no-repository control: the task statement alone, the same execution
/// image, task identity, provider profile, and frozen analysis rules as the
/// eligible comparison, and no repository. Anything the agent reached that
/// only the repository or the future holds is contamination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoRepositoryControl {
    pub task: String,
    pub provider: ProviderProfile,
    pub execution_image: String,
    pub analysis_family_digest: String,
    pub terminal: Terminal,
    /// Repository paths or commands the control reached; a control has none.
    pub repository_access: Vec<String>,
    /// Post-cutoff identifiers (the fix commit, the pull request) the
    /// control's output named.
    pub future_answers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Contamination {
    /// The statement alone was enough: the task is memorized for this pair.
    Memorized,
    RepositoryAccess {
        evidence: Vec<String>,
    },
    FutureAnswer {
        evidence: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlVerdict {
    Eligible,
    Excluded { contamination: Contamination },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRefused {
    /// The control and its comparison differ in image, identity, or rules.
    NotComparable { field: &'static str },
}

debug_display!(ControlRefused);

/// Classifies one control against the comparison it must match.
pub fn classify_control(
    control: &NoRepositoryControl,
    comparison: &NoRepositoryControl,
) -> Result<ControlVerdict, ControlRefused> {
    for (field, same) in [
        ("task", control.task == comparison.task),
        ("provider", control.provider == comparison.provider),
        (
            "execution_image",
            control.execution_image == comparison.execution_image,
        ),
        (
            "analysis_family_digest",
            control.analysis_family_digest == comparison.analysis_family_digest,
        ),
    ] {
        if !same {
            return Err(ControlRefused::NotComparable { field });
        }
    }
    Ok(if !control.repository_access.is_empty() {
        ControlVerdict::Excluded {
            contamination: Contamination::RepositoryAccess {
                evidence: control.repository_access.clone(),
            },
        }
    } else if !control.future_answers.is_empty() {
        ControlVerdict::Excluded {
            contamination: Contamination::FutureAnswer {
                evidence: control.future_answers.clone(),
            },
        }
    } else if control.terminal == Terminal::Pass {
        ControlVerdict::Excluded {
            contamination: Contamination::Memorized,
        }
    } else {
        ControlVerdict::Eligible
    })
}

/// Names the fix commit and pull request a control's output must not know.
pub fn future_answers(entry: &AnchorEntry, output: &str) -> Vec<String> {
    let mut found = Vec::new();
    if output.contains(&entry.fix_sha[..12]) {
        found.push(format!("fix_sha:{}", entry.fix_sha));
    }
    if let Some(pr) = entry
        .pull_request
        .filter(|pr| output.contains(&format!("#{pr}")))
    {
        found.push(format!("pull_request:{pr}"));
    }
    found
}

/// One provider pair's anchor accounting: every task keeps its row and its
/// reason; only `eligible` tasks count toward that pair's transfer claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairAccounting {
    pub provider: ProviderProfile,
    pub eligible: BTreeSet<String>,
    pub excluded: BTreeMap<String, Contamination>,
    pub cutoff_invalid: BTreeMap<String, CutoffRefused>,
    pub insufficiency_missing: BTreeSet<String>,
}

/// Folds audits, insufficiency proofs, and controls for one provider pair
/// into the anchor set `derive_claim_class` judges: a task is `valid` only
/// with a passing cutoff audit, an insufficiency proof, and an eligible
/// control; a memorized or contaminated task is `residue`; a failed audit is
/// `cutoff_invalid`.
pub fn anchor_set(
    corpus: &AnchorCorpus,
    role: AnchorRole,
    audits: &BTreeMap<String, CutoffAudit>,
    proofs: &BTreeMap<String, InsufficiencyProof>,
    controls: &BTreeMap<String, ControlVerdict>,
    provider: &ProviderProfile,
) -> (AnchorSet, PairAccounting) {
    let mut accounting = PairAccounting {
        provider: provider.clone(),
        eligible: BTreeSet::new(),
        excluded: BTreeMap::new(),
        cutoff_invalid: BTreeMap::new(),
        insufficiency_missing: BTreeSet::new(),
    };
    let tasks = corpus
        .entries
        .iter()
        .map(|entry| {
            let audit = audits
                .get(&entry.id)
                .map(CutoffAudit::validate)
                .unwrap_or(Err(CutoffRefused::SnapshotDigestMissing));
            let proof = proofs.get(&entry.id).map(InsufficiencyProof::validate);
            let verdict = match (audit, proof, controls.get(&entry.id)) {
                (Err(refused), _, _) => {
                    accounting.cutoff_invalid.insert(entry.id.clone(), refused);
                    AnchorVerdict::CutoffInvalid
                }
                (Ok(()), Some(Ok(())), Some(ControlVerdict::Eligible)) => {
                    accounting.eligible.insert(entry.id.clone());
                    AnchorVerdict::Valid
                }
                (Ok(()), Some(Ok(())), Some(ControlVerdict::Excluded { contamination })) => {
                    accounting
                        .excluded
                        .insert(entry.id.clone(), contamination.clone());
                    AnchorVerdict::Residue
                }
                _ => {
                    accounting.insufficiency_missing.insert(entry.id.clone());
                    AnchorVerdict::Residue
                }
            };
            AnchorTask {
                id: entry.id.clone(),
                family: entry.family.label().to_string(),
                verdict,
            }
        })
        .collect();
    (AnchorSet { role, tasks }, accounting)
}

/// The settings a real-history campaign must hold before it executes; none
/// defaults, and a pilot corpus is never a transfer set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealHistorySettings {
    pub providers: Vec<ProviderProfile>,
    pub execution_image: String,
    pub preparation_bound_ms: Option<u64>,
    pub transfer_criterion: Option<TransferCriterion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRefused {
    NoProviders,
    NoExecutionImage,
    NoPreparationBound,
}

debug_display!(SettingsRefused);

impl RealHistorySettings {
    /// Refuses before execution; the transfer criterion may be absent, in
    /// which case every claim derives as `generated_phase1`.
    pub fn validate(&self) -> Result<(), SettingsRefused> {
        if self.providers.is_empty() {
            return Err(SettingsRefused::NoProviders);
        }
        if self.execution_image.trim().is_empty() {
            return Err(SettingsRefused::NoExecutionImage);
        }
        if self.preparation_bound_ms.is_none() {
            return Err(SettingsRefused::NoPreparationBound);
        }
        Ok(())
    }
}
