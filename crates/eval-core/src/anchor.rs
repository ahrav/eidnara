//! Real-history anchor tasks: a corpus of commit and issue identifiers, the
//! cutoff audit that keeps future knowledge out of a task, the
//! current-tree-only insufficiency proof, and the no-repository control that
//! marks a memorized task and excludes it from one provider's transfer
//! evidence.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{is_lower_hex, protocol_digest};
use serde::{Deserialize, Serialize};

use crate::campaign::Terminal;
use crate::claim::{AnchorRole, AnchorSet, AnchorTask, AnchorVerdict};
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
    /// Issues and pull requests are numbered from one.
    ZeroNumber {
        id: String,
        field: &'static str,
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
    /// The id names clone and snapshot directories; it must be one plain path
    /// component to remain under the runner root.
    NotAPathComponent {
        id: String,
    },
    /// Two rows name one fix commit, or one issue, of one repository: one
    /// historical task under two ids.
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
            if !is_lower_hex(sha, 40) || sha.bytes().all(|b| b == b'0') {
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
        if self.fix_sha == self.base_sha {
            return Err(AnchorError::FixIsBase { id: id() });
        }
        for (field, number) in [
            ("issue", Some(self.issue)),
            ("pull_request", self.pull_request),
        ] {
            if number == Some(0) {
                return Err(AnchorError::ZeroNumber { id: id(), field });
            }
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

/// An `https://` clone URL of a host in lowercase DNS labels and a
/// repository path of non-empty, non-dot segments in unreserved URL
/// characters: no user, port, query, fragment, `./`, `..`, `//`, or trailing
/// slash, and one spelling per host, so the web path `repository_web_path`
/// derives is the URL itself and the repository's `/pull/` URLs are
/// recognizable from the row alone. `git@host:path`, `ssh://git@host:22/path`,
/// `https:///path`, `file:///path`, `https://host/path?x`,
/// `https://HOST/path`, `https://host./path`, `https://-host/path`,
/// `https://host/a/./b`, and `https://host/` are not accepted.
fn is_url(text: &str) -> bool {
    text.strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(host, path)| {
            host.len() <= 253
                && host.split('.').all(|label| {
                    let edges_alphanumeric = label
                        .chars()
                        .next()
                        .zip(label.chars().last())
                        .is_some_and(|(a, z)| {
                            a.is_ascii_alphanumeric() && z.is_ascii_alphanumeric()
                        });
                    edges_alphanumeric
                        && label.len() <= 63
                        && label
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                })
                && path.split('/').all(|segment| {
                    !segment.is_empty()
                        && segment != "."
                        && segment != ".."
                        && segment
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
                })
        })
}

/// An SPDX expression: identifiers of SPDX characters joined by `AND`,
/// `OR`, or `WITH`, so `MIT OR Apache-2.0` is one and `fixed by rebasing`
/// is not. Identifiers are judged by shape, not against the SPDX list.
fn is_spdx_expression(text: &str) -> bool {
    const OPERATORS: [&str; 3] = ["AND", "OR", "WITH"];
    let balanced = text
        .chars()
        .try_fold(0i32, |depth, c| match c {
            '(' => Some(depth + 1),
            ')' => (depth > 0).then(|| depth - 1),
            _ => Some(depth),
        })
        .is_some_and(|depth| depth == 0);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    balanced
        && tokens.len() % 2 == 1
        && tokens.iter().enumerate().all(|(i, token)| {
            if i % 2 == 1 {
                // `WITH` joins one simple license to one exception; neither
                // side is a group.
                OPERATORS.contains(token)
                    && (*token != "WITH"
                        || (!tokens[i - 1].ends_with(')')
                            && !tokens[i + 1].starts_with('(')
                            && !tokens[i + 1].trim_end_matches(')').ends_with('+')
                            && tokens.get(i + 2) != Some(&"WITH")))
            } else {
                let core = token.trim_start_matches('(').trim_end_matches(')');
                let id = core.strip_suffix('+').unwrap_or(core);
                !OPERATORS.contains(&core)
                    && id.chars().any(|c| c.is_ascii_alphanumeric())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-.".contains(c))
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
        let mut issues = BTreeMap::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(entry.id.as_str()) {
                return Err(AnchorError::DuplicateId {
                    id: entry.id.clone(),
                });
            }
            let repository = repository_web_path(&entry.repository);
            let by_fix = fixes.insert((repository.clone(), entry.fix_sha.as_str()), &entry.id);
            let by_issue = issues.insert((repository, entry.issue), &entry.id);
            if let Some(of) = by_fix.or(by_issue) {
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
    /// `AnchorEntry::digest` of the row that was prepared.
    pub entry_digest: String,
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
    /// The task was prepared as another version of its row.
    RowMismatch {
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
    corpus.digest().map_err(TimeStudyRefused::Corpus)?;
    if let Err(AnchorError::NotPilotComposition { found }) = corpus.is_pilot() {
        return Err(TimeStudyRefused::NotThePilot { found });
    }
    if measured.len() != TIME_STUDY_TASKS {
        return Err(TimeStudyRefused::WrongTaskCount {
            measured: measured.len(),
        });
    }
    for p in measured {
        let Some(entry) = corpus.entries.iter().find(|e| e.id == p.task) else {
            return Err(TimeStudyRefused::NotFromCorpus {
                task: p.task.clone(),
            });
        };
        if entry.digest().map_err(TimeStudyRefused::Corpus)? != p.entry_digest {
            return Err(TimeStudyRefused::RowMismatch {
                task: p.task.clone(),
            });
        }
    }
    let mut tasks = BTreeSet::new();
    if let Some(repeat) = measured.iter().find(|p| !tasks.insert(p.task.as_str())) {
        return Err(TimeStudyRefused::DuplicateTask {
            task: repeat.task.clone(),
        });
    }
    let total: u128 = measured.iter().map(|p| u128::from(p.prepare_ms)).sum();
    let pilot: u128 = PILOT_COMPOSITION.iter().map(|(_, n)| u128::from(*n)).sum();
    let projected = total * pilot / TIME_STUDY_TASKS as u128;
    let projected_ms = u64::try_from(projected).unwrap_or(u64::MAX);
    Ok(if projected <= u128::from(bound_ms) {
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
    /// When the repair first became public: the earliest of the pull
    /// request's creation and the commit times of the fix-side commits not
    /// reachable from the base. A merge committed after the cutoff can merge
    /// work that was public before it.
    #[serde(with = "crate::decimal")]
    pub repair_public_ms: i64,
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
    /// The digests of `fix_sha`'s tree and of its first parent's tree, read
    /// the same way; a fix that changes no file is not a fix.
    pub fix_tree_digest: String,
    pub fix_parent_tree_digest: String,
    /// Whether the base commit is an ancestor of the fix commit; a fix from
    /// an unrelated branch fixes nothing at this base.
    pub fix_descends_from_base: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum CutoffRefused {
    BaseAfterCutoff,
    FixNotAfterCutoff,
    /// The repair (its pull request or its commits) was public at or before
    /// the cutoff, whatever the fix commit's own time.
    RepairPublicBeforeCutoff,
    /// The repair is dated after the fix commit, which is one of the things
    /// it covers; the audit is not internally possible.
    RepairPublicAfterFix,
    /// The issue was filed after the cutoff, so its text is future knowledge.
    IssueAfterCutoff,
    /// The issue text was edited after the cutoff; the edit can describe the
    /// fix.
    IssueTextAfterCutoff,
    /// The issue text is dated before the issue was filed, so the audit is
    /// not evidence of anything.
    IssueTextBeforeIssue,
    /// The fix commit does not descend from the base commit.
    FixNotFromBase,
    /// The fix commit's tree is its parent's tree or the base tree: nothing
    /// was repaired.
    FixChangesNothing,
    SnapshotDigestMissing,
    /// A tree digest that is neither a git object id (forty hex) nor a
    /// protocol digest (sixty-four hex), is all zeroes, or is not in the
    /// same format as the snapshot digest names no comparable tree.
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
        let width = self.snapshot_digest.len();
        for digest in [
            &self.snapshot_digest,
            &self.base_tree_digest,
            &self.fix_tree_digest,
            &self.fix_parent_tree_digest,
        ] {
            if (width != 40 && width != 64)
                || !is_lower_hex(digest, width)
                || digest.bytes().all(|b| b == b'0')
            {
                return Err(CutoffRefused::MalformedDigest);
            }
        }
        if self.base_committed_ms > self.cutoff_ms {
            return Err(CutoffRefused::BaseAfterCutoff);
        }
        if self.fix_committed_ms <= self.cutoff_ms {
            return Err(CutoffRefused::FixNotAfterCutoff);
        }
        if self.repair_public_ms <= self.cutoff_ms {
            return Err(CutoffRefused::RepairPublicBeforeCutoff);
        }
        if self.repair_public_ms > self.fix_committed_ms {
            return Err(CutoffRefused::RepairPublicAfterFix);
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
        if !self.fix_descends_from_base {
            return Err(CutoffRefused::FixNotFromBase);
        }
        if self.fix_tree_digest == self.fix_parent_tree_digest
            || self.fix_tree_digest == self.base_tree_digest
        {
            return Err(CutoffRefused::FixChangesNothing);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum InsufficiencyRefused {
    /// Every hidden test passed at the cutoff: there is nothing to fix.
    TreeAlreadyPasses,
    /// No hidden test ran.
    NothingExecuted,
    /// Some hidden test does not pass on the fix tree, so the runner's
    /// environment is not shown to build and run the tests, and a failure or
    /// error on the base tree says nothing.
    ReferenceDoesNotPass,
    ProofForOtherTask,
    /// The proof ran over a row with this id that has since changed.
    RowMismatch,
    /// The proof ran over a tree other than the audited snapshot.
    TreeMismatch,
}

debug_display!(InsufficiencyRefused);

/// The current-tree-only run: the hidden tests over the snapshot with no
/// agent, next to the same tests over the snapshot with the fix applied. A
/// recorded pair of results is the proof; a corpus row without one is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsufficiencyProof {
    pub task: String,
    /// `AnchorEntry::digest` of the row whose snapshot the tests ran over.
    pub entry_digest: String,
    /// The digest of the tree the tests ran over; `anchor_set` requires it
    /// to be the audited snapshot.
    pub snapshot_digest: String,
    pub hidden: HiddenResults,
    /// The hidden tests over the fix tree. A base-tree failure or error
    /// counts only when every test passes here, because then the runner's
    /// environment is shown to build and run them.
    pub reference: HiddenResults,
}

impl InsufficiencyProof {
    /// A proof needs every hidden test to pass on the fix tree and at least
    /// one not to pass on the base tree. An `errored` base-tree test counts
    /// once the reference shows the runner can run it: a test that needs the
    /// fix to compile is insufficiency too.
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
    /// Whether the agent started; a run censored before it did shows nothing
    /// about the statement.
    pub started: bool,
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
    /// Whether the agent started.
    pub started: bool,
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

/// A terminal a started run reached; a censored run whose agent never
/// started (a budget spent before the first call) ran nothing.
fn ran(terminal: Terminal, started: bool) -> bool {
    started
        && matches!(
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
    if !ran(control.terminal, control.started) {
        return Err(ControlRefused::NotRun {
            terminal: control.terminal,
        });
    }
    if !ran(comparison.terminal, comparison.started) {
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
/// The fix commit counts as any whole word of hexadecimal digits, in either
/// case, that is at least `SHA_ABBREV` long and a prefix of it; the word is
/// taken whole between non-alphanumerics, so neither `a0123456` nor
/// `g0123456` names `0123456...`. A pull request counts as
/// `#<n>`, GitHub's `GH-<n>`, `PR <n>`, `pull request <n>`, or the
/// repository's `/pull/<n>` URL in any letter case, each as a whole number,
/// so `#20` is not found inside `#2016`.
pub fn future_answers(entry: &AnchorEntry, output: &str) -> Vec<String> {
    // Lowercased, with every run of whitespace one space, so `PR\t2016` and
    // `pull request\n2016` read as their single-spaced forms.
    let output = output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let mut found = Vec::new();
    if is_lower_hex(&entry.fix_sha, 40)
        && output
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| {
                word.len() >= SHA_ABBREV
                    && word.bytes().all(|b| b.is_ascii_hexdigit())
                    && entry.fix_sha.starts_with(word)
            })
    {
        found.push(format!("fix_sha:{}", entry.fix_sha));
    }
    if let Some(pr) = entry.pull_request.filter(|pr| {
        [
            format!("#{pr}"),
            format!("gh-{pr}"),
            format!("pr {pr}"),
            format!("pull request {pr}"),
            format!("pull-request {pr}"),
            format!("{}/pull/{pr}", repository_web_path(&entry.repository)),
        ]
        .iter()
        .any(|needle| names_whole_number(&output, needle))
    }) {
        found.push(format!("pull_request:{pr}"));
    }
    found
}

/// `needle` occurs in `output` as a whole number: not followed by a letter
/// or digit (so `#2016ff` is a colour, not `#2016`, while `/pull/2016/files`
/// still names 2016), and, when it begins with a host name rather than `#`,
/// not preceded by a name character, so `notexample.invalid/...` does not
/// name `example.invalid/...` (a `.` before it may, as in `www.`) while
/// `PR#2016` still names `#2016`.
fn names_whole_number(output: &str, needle: &str) -> bool {
    let bytes = output.as_bytes();
    let bounded_left = !needle.starts_with('#');
    output.match_indices(needle).any(|(at, _)| {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + needle.len());
        !(bounded_left && before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-'))
            && !after.is_some_and(u8::is_ascii_alphanumeric)
    })
}

/// The clone URL lowercased and without its scheme, trailing slash, or
/// `.git` suffix: the web path that prefixes pull-request URLs for the
/// repository and keys it. `is_url` admits no user or port for it to strip.
fn repository_web_path(repository: &str) -> String {
    let lower = repository.to_ascii_lowercase();
    let path = lower
        .split_once("://")
        .map_or(lower.as_str(), |(_, rest)| rest)
        .trim_end_matches('/');
    path.strip_suffix(".git").unwrap_or(path).to_string()
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
        let proof = proofs.get(&entry.id).map(|proof| {
            proof.validate_for(entry)?;
            let audited = audits.get(&entry.id).map(|a| a.snapshot_digest.as_str());
            if audited != Some(proof.snapshot_digest.as_str()) {
                return Err(InsufficiencyRefused::TreeMismatch);
            }
            Ok(())
        });
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
/// defaults, and a pilot corpus is never a transfer set. The transfer
/// criterion is the analysis family's, not a setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealHistorySettings {
    pub providers: Vec<ProviderProfile>,
    pub execution_image: String,
    pub preparation_bound_ms: Option<u64>,
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
}

debug_display!(SettingsRefused);

impl RealHistorySettings {
    /// Refuses before execution. The transfer criterion is not a setting:
    /// it lives on the frozen analysis family, the one place
    /// `AnalysisFamily::claim_class` reads it from.
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
        Ok(())
    }
}
