//! Real-history anchor tasks: a corpus of commit and issue identifiers, the
//! cutoff audit that keeps future knowledge out of a task, the
//! current-tree-only insufficiency proof, and the no-repository control that
//! marks a memorized task and excludes it from one provider's transfer
//! evidence.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};

use crate::campaign::Terminal;
use crate::claim::{
    AnchorRole, AnchorSet, AnchorTask, AnchorVerdict, TransferCriterion, UnmetClause,
};
use crate::task::{HiddenOutcome, HiddenResults};

pub const ANCHOR_CORPUS_SCHEMA: &str = "eval-anchor-corpus/v1";
pub const ANCHOR_CORPUS_DIGEST_PROTOCOL: &str = "eval-anchor-corpus-digest/v1";
/// The digest evidence names to say which corpus row it was produced for.
pub const ANCHOR_ENTRY_DIGEST_PROTOCOL: &str = "eval-anchor-entry-digest/v1";
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
    /// The fix commit is the base commit, so nothing was fixed after the
    /// cutoff.
    FixIsBase {
        id: String,
    },
    /// A field holds text, not the identifier it names: an id or URL with
    /// whitespace, a URL without a scheme, or a license that is not an SPDX
    /// expression.
    TextPersisted {
        id: String,
        field: &'static str,
    },
    DuplicateId {
        id: String,
    },
    /// Two rows name one fix commit of one repository: one historical task
    /// under two ids.
    DuplicateTask {
        id: String,
        of: String,
    },
    NotPilotComposition {
        found: BTreeMap<Family, u32>,
    },
    /// The pilot corpus was offered as a transfer set; the pilot alone never
    /// transfers.
    PilotIsNotATransferSet,
    /// The corpus does not canonicalize, so it has no digest.
    NotCanonical {
        detail: String,
    },
}

debug_display!(AnchorError);

impl AnchorEntry {
    pub fn validate(&self) -> Result<(), AnchorError> {
        let id = || self.id.clone();
        for (field, text, is_identifier) in [
            ("id", &self.id, is_token as fn(&str) -> bool),
            ("repository", &self.repository, is_url),
            ("license", &self.license, is_spdx_expression),
        ] {
            if text.trim().is_empty() {
                return Err(AnchorError::EmptyField { id: id(), field });
            }
            if !is_identifier(text) {
                return Err(AnchorError::TextPersisted { id: id(), field });
            }
        }
        for (field, sha) in [("base_sha", &self.base_sha), ("fix_sha", &self.fix_sha)] {
            if !is_lower_hex(sha, 40) {
                return Err(AnchorError::NotASha { id: id(), field });
            }
        }
        if self.fix_sha == self.base_sha {
            return Err(AnchorError::FixIsBase { id: id() });
        }
        Ok(())
    }

    /// The identity evidence for this row carries; a row edited in any field
    /// has another digest, so evidence produced before the edit matches
    /// nothing.
    pub fn digest(&self) -> Result<String, AnchorError> {
        self.validate()?;
        let value = serde_json::to_value(self).expect("entry serializes");
        protocol_digest(ANCHOR_ENTRY_DIGEST_PROTOCOL, &value).map_err(|e| {
            AnchorError::NotCanonical {
                detail: e.to_string(),
            }
        })
    }
}

fn is_token(text: &str) -> bool {
    !text.contains(char::is_whitespace)
}

/// An `https://` clone URL of a lowercase host and a repository path, in
/// unreserved URL characters only: no user, port, query, or fragment, and
/// one spelling per host, so the web path `repository_web_path` derives is
/// the URL itself, the repository's `/pull/` URLs are recognizable from the
/// row alone, and one repository has one key. `git@host:path`,
/// `ssh://git@host:22/path`, `https:///path`, `file:///path`,
/// `https://host/path?x`, `https://HOST/path`, and `https://host/` are not
/// accepted.
fn is_url(text: &str) -> bool {
    text.strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(host, path)| {
            !host.is_empty()
                && !host.contains(|c: char| c.is_ascii_uppercase())
                && !path.is_empty()
                && [host, path]
                    .concat()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._~/".contains(c))
        })
}

/// An SPDX expression: identifiers of SPDX characters joined by `AND`,
/// `OR`, or `WITH`, so `MIT OR Apache-2.0` is one and `fixed by rebasing`
/// is not. Identifiers are judged by shape, not against the SPDX list.
fn is_spdx_expression(text: &str) -> bool {
    const OPERATORS: [&str; 3] = ["AND", "OR", "WITH"];
    let tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.len() % 2 == 1
        && tokens.iter().enumerate().all(|(i, token)| {
            if i % 2 == 1 {
                OPERATORS.contains(token)
            } else {
                !OPERATORS.contains(token)
                    && token
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-.+()".contains(c))
            }
        })
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
        let mut fixes = BTreeMap::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(entry.id.as_str()) {
                return Err(AnchorError::DuplicateId {
                    id: entry.id.clone(),
                });
            }
            if let Some(of) = fixes.insert(
                (entry.repository.as_str(), entry.fix_sha.as_str()),
                &entry.id,
            ) {
                return Err(AnchorError::DuplicateTask {
                    id: entry.id.clone(),
                    of: of.clone(),
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

    pub fn digest(&self) -> Result<String, AnchorError> {
        self.validate()?;
        let value = serde_json::to_value(self).expect("corpus serializes");
        protocol_digest(ANCHOR_CORPUS_DIGEST_PROTOCOL, &value).map_err(|e| {
            AnchorError::NotCanonical {
                detail: e.to_string(),
            }
        })
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
    /// The corpus itself refuses; its measurements say nothing.
    Corpus(AnchorError),
    /// The study projects the pilot's cost from the pilot's own tasks.
    NotThePilot {
        found: BTreeMap<Family, u32>,
    },
    WrongTaskCount {
        measured: usize,
    },
    NotFromCorpus {
        task: String,
    },
    /// One task measured twice is one task, not two.
    DuplicateTask {
        task: String,
    },
}

debug_display!(TimeStudyRefused);

/// Projects the pilot's preparation cost from `TIME_STUDY_TASKS` measured
/// preparations of corpus tasks, scaled to the whole pilot.
pub fn time_study(
    corpus: &AnchorCorpus,
    measured: &[Preparation],
    bound_ms: u64,
) -> Result<Affordability, TimeStudyRefused> {
    corpus.validate().map_err(TimeStudyRefused::Corpus)?;
    if let Err(AnchorError::NotPilotComposition { found }) = corpus.is_pilot() {
        return Err(TimeStudyRefused::NotThePilot { found });
    }
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
    let mut tasks = BTreeSet::new();
    if let Some(repeat) = measured.iter().find(|p| !tasks.insert(p.task.as_str())) {
        return Err(TimeStudyRefused::DuplicateTask {
            task: repeat.task.clone(),
        });
    }
    let total: u128 = measured.iter().map(|p| u128::from(p.prepare_ms)).sum();
    let pilot: u128 = PILOT_COMPOSITION.iter().map(|(_, n)| u128::from(*n)).sum();
    let projected_ms = u64::try_from(total * pilot / TIME_STUDY_TASKS as u128).unwrap_or(u64::MAX);
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
    /// `AnchorEntry::digest` of the row the audit was produced for.
    pub entry_digest: String,
    #[serde(with = "crate::decimal")]
    pub cutoff_ms: i64,
    #[serde(with = "crate::decimal")]
    pub base_committed_ms: i64,
    #[serde(with = "crate::decimal")]
    pub fix_committed_ms: i64,
    #[serde(with = "crate::decimal")]
    pub issue_created_ms: i64,
    /// When the issue text the task is given was written: its last edit, or
    /// its creation when it was never edited. An edit after the cutoff can
    /// name the fix.
    #[serde(with = "crate::decimal")]
    pub issue_text_ms: i64,
    /// The digest of the tree the snapshot holds.
    pub snapshot_digest: String,
    /// The digest of `base_sha`'s tree, read from the repository the same
    /// way; the snapshot is that tree and nothing else.
    pub base_tree_digest: String,
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
    /// The issue text was edited after the cutoff; the edit can describe the
    /// fix.
    IssueTextAfterCutoff,
    /// The issue text is dated before the issue was filed, so the audit is
    /// not evidence of anything.
    IssueTextBeforeIssue,
    FutureContentInSnapshot,
    SnapshotDigestMissing,
    /// A tree digest that is neither a git object id (forty hex) nor a
    /// protocol digest (sixty-four hex) names no tree.
    MalformedDigest,
    /// The snapshot's tree is not the base commit's tree.
    SnapshotNotBaseTree,
    AuditForOtherTask,
    /// The audit was produced for a row with this id that has since changed
    /// in some field (commits, issue, repository, cutoff).
    RowMismatch,
    /// The audit judged a cutoff other than the corpus row's, so its
    /// timestamps say nothing about the row's cutoff.
    CutoffMismatch,
}

debug_display!(CutoffRefused);

impl CutoffAudit {
    /// Validates the audit as evidence for `entry`: it must name the entry's
    /// task and row and judge the entry's cutoff before its own timestamps
    /// count. A row with no digest matches no audit.
    pub fn validate_for(&self, entry: &AnchorEntry) -> Result<(), CutoffRefused> {
        if self.task != entry.id {
            return Err(CutoffRefused::AuditForOtherTask);
        }
        if entry.digest().ok().as_deref() != Some(self.entry_digest.as_str()) {
            return Err(CutoffRefused::RowMismatch);
        }
        if self.cutoff_ms != entry.cutoff_ms {
            return Err(CutoffRefused::CutoffMismatch);
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), CutoffRefused> {
        if self.snapshot_digest.is_empty() {
            return Err(CutoffRefused::SnapshotDigestMissing);
        }
        for digest in [&self.snapshot_digest, &self.base_tree_digest] {
            if !is_lower_hex(digest, 40) && !is_lower_hex(digest, 64) {
                return Err(CutoffRefused::MalformedDigest);
            }
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
        if self.issue_text_ms > self.cutoff_ms {
            return Err(CutoffRefused::IssueTextAfterCutoff);
        }
        if self.issue_text_ms < self.issue_created_ms {
            return Err(CutoffRefused::IssueTextBeforeIssue);
        }
        if self.snapshot_digest != self.base_tree_digest {
            return Err(CutoffRefused::SnapshotNotBaseTree);
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
    /// Every hidden test that reached a verdict passed at the cutoff.
    TreeAlreadyPasses,
    /// No hidden test reached a verdict: the run is empty or every test
    /// errored.
    NothingExecuted,
    ProofForOtherTask,
    /// The proof ran over a row with this id that has since changed.
    RowMismatch,
}

debug_display!(InsufficiencyRefused);

/// The current-tree-only run: the hidden tests over the snapshot with no
/// agent. A recorded result is the proof; a corpus row without one is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsufficiencyProof {
    pub task: String,
    /// `AnchorEntry::digest` of the row whose snapshot the tests ran over.
    pub entry_digest: String,
    pub hidden: HiddenResults,
}

impl InsufficiencyProof {
    /// A proof needs at least one hidden test that ran and failed; an
    /// `errored` test never reached a verdict and proves nothing.
    pub fn validate(&self) -> Result<(), InsufficiencyRefused> {
        if self.hidden.values().any(|o| *o == HiddenOutcome::Failed) {
            return Ok(());
        }
        Err(
            if self.hidden.values().any(|o| *o == HiddenOutcome::Passed) {
                InsufficiencyRefused::TreeAlreadyPasses
            } else {
                InsufficiencyRefused::NothingExecuted
            },
        )
    }

    pub fn validate_for(&self, entry: &AnchorEntry) -> Result<(), InsufficiencyRefused> {
        if self.task != entry.id {
            return Err(InsufficiencyRefused::ProofForOtherTask);
        }
        if entry.digest().ok().as_deref() != Some(self.entry_digest.as_str()) {
            return Err(InsufficiencyRefused::RowMismatch);
        }
        self.validate()
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
    /// `AnchorEntry::digest` of the row whose statement the control was given.
    pub entry_digest: String,
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

/// The repository-bearing run a control is judged against: the same task,
/// provider profile, execution image, and frozen analysis rules, with the
/// repository. A control is never its own comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryComparison {
    pub task: String,
    /// `AnchorEntry::digest` of the row the run was over.
    pub entry_digest: String,
    pub provider: ProviderProfile,
    pub execution_image: String,
    pub analysis_family_digest: String,
    pub terminal: Terminal,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifiedControl {
    pub task: String,
    pub entry_digest: String,
    pub provider: ProviderProfile,
    pub verdict: ControlVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRefused {
    /// The control and its comparison differ in image, identity, or rules.
    NotComparable { field: &'static str },
    /// The frozen analysis rules are not named by a digest, so nothing was
    /// compared under them.
    MalformedDigest { field: &'static str },
    /// The control did not run, so it cannot show the statement alone was
    /// insufficient.
    NotRun { terminal: Terminal },
    /// The repository-bearing comparison did not run, so there is nothing to
    /// compare the control with.
    ComparisonNotRun { terminal: Terminal },
}

debug_display!(ControlRefused);

/// A terminal a run reached, not one it never started.
fn ran(terminal: Terminal) -> bool {
    matches!(
        terminal,
        Terminal::Pass | Terminal::Fail | Terminal::Censored { .. }
    )
}

/// Classifies comparable controls whose control and comparison both ran to
/// `Pass`, `Fail`, or `Censored`.
pub fn classify_control(
    control: &NoRepositoryControl,
    comparison: &RepositoryComparison,
) -> Result<ClassifiedControl, ControlRefused> {
    if !is_lower_hex(&control.analysis_family_digest, 64) {
        return Err(ControlRefused::MalformedDigest {
            field: "analysis_family_digest",
        });
    }
    for (field, same) in [
        ("task", control.task == comparison.task),
        (
            "entry_digest",
            control.entry_digest == comparison.entry_digest,
        ),
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
    if !ran(control.terminal) {
        return Err(ControlRefused::NotRun {
            terminal: control.terminal,
        });
    }
    if !ran(comparison.terminal) {
        return Err(ControlRefused::ComparisonNotRun {
            terminal: comparison.terminal,
        });
    }
    let verdict = if !control.repository_access.is_empty() {
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
    };
    Ok(ClassifiedControl {
        task: control.task.clone(),
        entry_digest: control.entry_digest.clone(),
        provider: control.provider.clone(),
        verdict,
    })
}

/// The shortest abbreviation of a commit that names it: what `git log
/// --oneline` prints.
const SHA_ABBREV: usize = 7;

/// Names the fix commit and pull request a control's output must not know.
/// The fix commit counts as any run of hexadecimal digits, in either case,
/// that is at least `SHA_ABBREV` long and a prefix of it; the run is taken
/// whole, so `a0123456` does not name `0123456...`. A pull request counts as
/// `#<n>` or as the repository's `/pull/<n>` URL in any letter case, each as
/// a whole number, so `#20` is not found inside `#2016`.
pub fn future_answers(entry: &AnchorEntry, output: &str) -> Vec<String> {
    let output = output.to_ascii_lowercase();
    let mut found = Vec::new();
    if is_lower_hex(&entry.fix_sha, 40)
        && output
            .split(|c: char| !c.is_ascii_hexdigit())
            .any(|run| run.len() >= SHA_ABBREV && entry.fix_sha.starts_with(run))
    {
        found.push(format!("fix_sha:{}", entry.fix_sha));
    }
    if let Some(pr) = entry.pull_request.filter(|pr| {
        names_whole_number(&output, &format!("#{pr}"))
            || names_whole_number(
                &output,
                &format!(
                    "{}/pull/{pr}",
                    repository_web_path(&entry.repository).to_ascii_lowercase()
                ),
            )
    }) {
        found.push(format!("pull_request:{pr}"));
    }
    found
}

fn names_whole_number(output: &str, needle: &str) -> bool {
    output.match_indices(needle).any(|(at, _)| {
        !output
            .as_bytes()
            .get(at + needle.len())
            .is_some_and(u8::is_ascii_digit)
    })
}

/// The clone URL without its scheme, trailing slash, or `.git` suffix,
/// which prefixes pull-request URLs for the repository; `is_url` admits no
/// user or port for it to strip.
fn repository_web_path(repository: &str) -> &str {
    let path = repository
        .split_once("://")
        .map_or(repository, |(_, rest)| rest)
        .trim_end_matches('/');
    path.strip_suffix(".git").unwrap_or(path)
}

/// One provider pair's anchor accounting: every task keeps its row and its
/// reason; only `eligible` tasks count toward that pair's transfer claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairAccounting {
    pub provider: ProviderProfile,
    pub eligible: BTreeSet<String>,
    pub excluded: BTreeMap<String, Contamination>,
    pub cutoff_missing: BTreeSet<String>,
    pub cutoff_invalid: BTreeMap<String, CutoffRefused>,
    pub insufficiency_missing: BTreeSet<String>,
    pub insufficiency_refused: BTreeMap<String, InsufficiencyRefused>,
    pub control_missing: BTreeSet<String>,
}

/// Folds audits, insufficiency proofs, and controls for one provider pair
/// into the anchor set `derive_claim_class` judges. The corpus must validate,
/// and the pilot corpus is never a `Transfer` set. A task is `valid` only
/// when its audit and proof name it and pass, and its control was classified
/// for it under `provider` as eligible; a failed audit is `cutoff_invalid`
/// and every other task, a task with no audit included, is `residue`. Each
/// task lands in exactly one accounting set, at its first failing gate.
pub fn anchor_set(
    corpus: &AnchorCorpus,
    role: AnchorRole,
    audits: &BTreeMap<String, CutoffAudit>,
    proofs: &BTreeMap<String, InsufficiencyProof>,
    controls: &BTreeMap<String, ClassifiedControl>,
    provider: &ProviderProfile,
) -> Result<(AnchorSet, PairAccounting), AnchorError> {
    corpus.validate()?;
    if role == AnchorRole::Transfer && corpus.is_pilot().is_ok() {
        return Err(AnchorError::PilotIsNotATransferSet);
    }
    let mut accounting = PairAccounting {
        provider: provider.clone(),
        eligible: BTreeSet::new(),
        excluded: BTreeMap::new(),
        cutoff_missing: BTreeSet::new(),
        cutoff_invalid: BTreeMap::new(),
        insufficiency_missing: BTreeSet::new(),
        insufficiency_refused: BTreeMap::new(),
        control_missing: BTreeSet::new(),
    };
    let mut tasks = Vec::with_capacity(corpus.entries.len());
    for entry in &corpus.entries {
        let digest = entry.digest()?;
        let audit = audits.get(&entry.id).map(|audit| audit.validate_for(entry));
        let proof = proofs.get(&entry.id).map(|proof| proof.validate_for(entry));
        let control = controls
            .get(&entry.id)
            .filter(|c| c.task == entry.id && c.entry_digest == digest && c.provider == *provider)
            .map(|c| &c.verdict);
        let verdict = match (audit, proof, control) {
            (None, _, _) => {
                accounting.cutoff_missing.insert(entry.id.clone());
                AnchorVerdict::Residue
            }
            (Some(Err(refused)), _, _) => {
                accounting.cutoff_invalid.insert(entry.id.clone(), refused);
                AnchorVerdict::CutoffInvalid
            }
            (Some(Ok(())), None, _) => {
                accounting.insufficiency_missing.insert(entry.id.clone());
                AnchorVerdict::Residue
            }
            (Some(Ok(())), Some(Err(refused)), _) => {
                accounting
                    .insufficiency_refused
                    .insert(entry.id.clone(), refused);
                AnchorVerdict::Residue
            }
            (Some(Ok(())), Some(Ok(())), None) => {
                accounting.control_missing.insert(entry.id.clone());
                AnchorVerdict::Residue
            }
            (Some(Ok(())), Some(Ok(())), Some(ControlVerdict::Eligible)) => {
                accounting.eligible.insert(entry.id.clone());
                AnchorVerdict::Valid
            }
            (Some(Ok(())), Some(Ok(())), Some(ControlVerdict::Excluded { contamination })) => {
                accounting
                    .excluded
                    .insert(entry.id.clone(), contamination.clone());
                AnchorVerdict::Residue
            }
        };
        tasks.push(AnchorTask {
            id: entry.id.clone(),
            family: entry.family.label().to_string(),
            verdict,
        });
    }
    Ok((AnchorSet { role, tasks }, accounting))
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
    /// A provider profile with a blank field names no pair.
    EmptyProviderField {
        field: &'static str,
    },
    NoExecutionImage,
    NoPreparationBound,
    /// The criterion is one the claim would refuse.
    TransferCriterion(UnmetClause),
}

debug_display!(SettingsRefused);

impl RealHistorySettings {
    /// Refuses before execution; the transfer criterion may be absent, in
    /// which case every claim derives as `generated_phase1`.
    pub fn validate(&self) -> Result<(), SettingsRefused> {
        if self.providers.is_empty() {
            return Err(SettingsRefused::NoProviders);
        }
        for profile in &self.providers {
            for (field, text) in [
                ("provider", &profile.provider),
                ("model", &profile.model),
                ("tokenizer_profile", &profile.tokenizer_profile),
            ] {
                if text.trim().is_empty() {
                    return Err(SettingsRefused::EmptyProviderField { field });
                }
            }
        }
        if self.execution_image.trim().is_empty() {
            return Err(SettingsRefused::NoExecutionImage);
        }
        if self.preparation_bound_ms.is_none() {
            return Err(SettingsRefused::NoPreparationBound);
        }
        if let Some(criterion) = &self.transfer_criterion {
            criterion
                .validate()
                .map_err(SettingsRefused::TransferCriterion)?;
        }
        Ok(())
    }
}
