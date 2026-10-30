//! The verifier's bounded background pass: one task per data home classifies each bound
//! project's live decisions, verifies the verifiable ones outside the Kernel writer, and commits
//! each write set, under the user-only `verifier.enabled` flag.
//!
//! [`Verifier::pass`] reads the flag for every bound project before any Kernel read or repository
//! open; a project whose flag is off is passed over whole. For an enabled project it captures one
//! [`CommitReadTarget`] and pages the project scope's live decisions in `(created_commit_seq,
//! object_id)` order from a process-local cursor, excluding decisions the verifier authored. Each
//! decision ends in one of:
//!
//! - a skip, before any repository read or Kernel write: `skipped_stale` when its latest own
//!   admission is `Stale`, `skipped_attested` when the registry holds the attestation object id
//!   [`Identity::of`] derives for the decision and its anchored oid, live or retired;
//! - an unverifiable bucket `unverifiable_<reason>`, including `invalid_document_path` for
//!   [`ClassifyError::InvalidDocumentPath`];
//! - a verdict bucket (`match`, `mismatch`, `unavailable_<reason>`) and then, for `match` and
//!   `mismatch`, one [`attest`] call whose [`Outcome`] increments one effect counter after it
//!   returns.
//!
//! `considered` counts every unverifiable and verdict bucket increment, so the buckets sum to it at
//! every pass end. Per project per pass at most [`MAX_VERDICTS`] verdicts are computed and at most
//! [`MAX_VERDICTS`] × [`MAX_BLOB_BYTES`] blob bytes are read; skips and unverifiable decisions do
//! not count. The pass deadline is polled before each decision, before each repository identity
//! derivation, before each object read (through [`verify`]), and before each commit. Projects are
//! visited in bound-root order from a start that rotates by one each pass.
//!
//! A project's decisions resolve their documents only against the project's own bound roots, at
//! the first root in bound-root order whose repository identity the decision's document names;
//! publication derives that identity from the publishing project's root, so a document another
//! project's root publishes classifies `repository_unbound` here. Each root's repository identity
//! is derived when the project's turn comes and kept for the process lifetime once derived. A
//! derivation the pass deadline cuts short passes over the project for this pass before any
//! decision is read, so no decision is counted or attempted, and the next pass derives again. A
//! root whose repository is unreadable, or whose walk spends its own share of the pass, binds no
//! repository for this pass and is derived again on the next; the project's other roots proceed,
//! and the decisions under it classify `repository_unbound`. At each pass start the state of a
//! scope or root no longer bound is dropped.
//!
//! The cursor passes a decision only once it is resolved, and only while every decision before it
//! is resolved too; resolved decisions above the cursor are remembered, so no pass reads them
//! again. A decision is resolved by a skip, an unverifiable class, a decision outside the
//! verifier's input ([`Class::NotInput`]), a terminal `Unavailable` reason, a committed or replayed
//! write, a `conflict`, an invalid write set, or K = [`MAX_ATTEMPTS`] attempts in total, shared
//! across transient `Unavailable` reasons, deadlines, Kernel refusals, a changed decision, and
//! degraded witnesses. The K-th attempt counts `witness_degraded_skipped` when its witness
//! degraded and `unavailable_skipped` otherwise. A terminal reason is kept per `(decision, oid)` for the process
//! lifetime. All state is process-local: a restart rescans from the start, and the skips and the
//! write set's receipt replay make the rescan write nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::{
    ClaimFactsError, CommitReadIncarnation, CommitReadTarget, Disposition, KernelError, KernelStore,
};

use super::applicability::{self, Applicability};
use super::attest::{Identity, Outcome, attest};
use super::classify::{BoundRoot, Class, ClassifyError, VERIFIER_SOURCE_KIND, classify};
use super::{Deadline, Evaluation, MAX_BLOB_BYTES, Reason, Stage, Verdict, verify};
use crate::repository_identity::DerivationFailure;

/// N: the verdicts one pass computes per project.
pub const MAX_VERDICTS: u64 = 8;
/// K: the passes a decision is attempted before a transient outcome resolves it.
pub const MAX_ATTEMPTS: u32 = 3;
/// The decisions one page read returns.
pub const PAGE_ROWS: NonZeroUsize = NonZeroUsize::new(32).unwrap();
/// The wall time one pass may spend.
pub const PASS_DEADLINE: Duration = Duration::from_secs(10);
/// The share of a pass one repository identity derivation may spend, as source capture allows.
const REPOSITORY_WALK_BUDGET: Duration = Duration::from_secs(2);
/// The interval after a pass that resolved nothing and failed nothing.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(30);
/// The interval after a pass that made progress or failed, and while the Kernel is not ready.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Every counter of the `metrics.verifier` block. All are process-local and content-free.
pub const COUNTERS: [&str; 40] = [
    "considered",
    "match",
    "mismatch",
    "unavailable_path_absent",
    "unavailable_not_a_blob",
    "unavailable_object_unreadable",
    "unavailable_over_budget",
    "unavailable_deadline",
    "unavailable_repository_unbound",
    "unavailable_repository_unopenable",
    "unverifiable_no_anchor",
    "unverifiable_anchor_not_git",
    "unverifiable_no_citation",
    "unverifiable_citation_not_document",
    "unverifiable_evidence_retired",
    "unverifiable_descriptor_retired",
    "unverifiable_digest_missing",
    "unverifiable_self_source",
    "unverifiable_invalid_document_path",
    "skipped_attested",
    "skipped_stale",
    "attestation_committed",
    "attestation_replayed",
    "stale_committed",
    "commit_refused_changed",
    "commit_refused_invalid_write_set",
    "commit_refused_busy",
    "commit_refused_deadline",
    "commit_refused_io",
    "commit_refused_other",
    "commit_conflict",
    "witness_degraded",
    "witness_degraded_skipped",
    "unavailable_skipped",
    "applicability_stale",
    "applicability_current",
    "applicability_unavailable",
    "bytes_read",
    "passes",
    "last_pass_ms",
];

/// The verifier's counters, starting at zero when the daemon starts.
pub struct Counters(Mutex<BTreeMap<&'static str, u64>>);

impl Default for Counters {
    fn default() -> Self {
        Self(Mutex::new(COUNTERS.iter().map(|name| (*name, 0)).collect()))
    }
}

impl Counters {
    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<&'static str, u64>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn add(&self, name: &str, amount: u64) {
        debug_assert!(COUNTERS.contains(&name), "unknown verifier counter {name}");
        if let Some(value) = self.map().get_mut(name) {
            *value = value.saturating_add(amount);
        }
    }

    /// The value of counter `name`; zero for a name outside [`COUNTERS`].
    pub fn get(&self, name: &str) -> u64 {
        self.map().get(name).copied().unwrap_or(0)
    }

    /// The `metrics.verifier` block: every counter of [`COUNTERS`].
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.map()
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).into()))
                .collect(),
        )
    }
}

/// Where the pass polls its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Before a project and before each decision.
    Decision,
    /// Before a root's repository identity derivation.
    Derive,
    /// Before the object read [`verify`] names.
    Read(Stage),
    /// Before a write set's commit.
    Commit,
}

/// The pass's time: its deadline polls and the budget its Kernel reads and writer waits run under.
pub trait PassClock {
    /// Whether the pass must stop at `poll`.
    fn passed(&self, poll: Poll) -> bool;
    fn budget(&self) -> &EvalBudget;
}

/// The production clock: the pass budget's deadline, raised early by the task's cancellation.
impl PassClock for EvalBudget {
    fn passed(&self, _poll: Poll) -> bool {
        self.is_exhausted()
    }

    fn budget(&self) -> &EvalBudget {
        self
    }
}

struct Reads<'a>(&'a dyn PassClock);

impl Deadline for Reads<'_> {
    fn passed(&self, stage: Stage) -> bool {
        self.0.passed(Poll::Read(stage))
    }
}

/// A bound project root: its path and the project digest a fallback repository identity takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    pub digest: String,
}

/// A bound project: its Kernel scope and its roots in bound-root order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub scope_id: String,
    pub roots: Vec<Root>,
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassReport {
    /// Whether some decision was resolved or written.
    pub progressed: bool,
    /// Whether a Kernel read or commit was refused, or a verdict was a transient `Unavailable`.
    pub failed: bool,
}

#[derive(Default)]
struct ProjectState {
    incarnation: Option<CommitReadIncarnation>,
    cursor: Option<(i64, String)>,
    resolved: HashSet<String>,
    attempts: HashMap<String, u32>,
    applicability: Option<(i64, String)>,
}

/// The pass's process-local state.
#[derive(Default)]
pub struct Verifier {
    projects: HashMap<String, ProjectState>,
    terminal: HashSet<(String, String)>,
    repository_ids: HashMap<PathBuf, Option<String>>,
    rotation: usize,
}

enum Step {
    Resolved,
    Unresolved,
    StopProject,
    StopPass,
}

impl Verifier {
    /// The cursor of project `scope_id`: the `(created_commit_seq, object_id)` of the last decision
    /// of its resolved prefix.
    pub fn cursor(&self, scope_id: &str) -> Option<(i64, String)> {
        self.projects.get(scope_id)?.cursor.clone()
    }

    /// One pass over `projects`, counting into `counters`.
    pub fn pass(
        &mut self,
        kernel: &KernelStore,
        projects: &[Project],
        enabled: &dyn Fn(&Path) -> bool,
        counters: &Counters,
        clock: &dyn PassClock,
    ) -> PassReport {
        let started = Instant::now();
        self.projects
            .retain(|scope_id, _| projects.iter().any(|project| project.scope_id == *scope_id));
        self.repository_ids.retain(|path, _| {
            projects
                .iter()
                .flat_map(|project| &project.roots)
                .any(|root| root.path == *path)
        });
        let enabled: Vec<&Project> = projects
            .iter()
            .filter(|project| {
                project
                    .roots
                    .first()
                    .is_some_and(|root| enabled(&root.path))
            })
            .collect();
        let mut report = PassReport::default();
        if enabled.is_empty() {
            return report;
        }
        let start = self.rotation % enabled.len();
        self.rotation = self.rotation.wrapping_add(1);
        for project in enabled.iter().cycle().skip(start).take(enabled.len()) {
            if clock.passed(Poll::Decision) {
                break;
            }
            let Some(roots) = self.roots(project, clock) else {
                report.failed = true;
                continue;
            };
            let bound: Vec<BoundRoot<'_>> = roots
                .iter()
                .map(|(root, repository_id)| BoundRoot {
                    root,
                    repository_id,
                })
                .collect();
            let state = self.projects.entry(project.scope_id.clone()).or_default();
            let mut run = ProjectRun {
                kernel,
                scope_id: &project.scope_id,
                roots: &bound,
                counters,
                clock,
                report: &mut report,
                terminal: &mut self.terminal,
                state,
                verdicts: 0,
                bytes_read: 0,
            };
            if matches!(run.run(), Step::StopPass) {
                break;
            }
        }
        counters.add("passes", 1);
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut map = counters.map();
        map.insert("last_pass_ms", elapsed);
        report
    }

    /// `project`'s roots with a repository identity, in bound-root order, or `None` when the pass
    /// deadline cut a derivation short. A root whose derivation failed otherwise is left out and
    /// derived again next pass.
    fn roots(
        &mut self,
        project: &Project,
        clock: &dyn PassClock,
    ) -> Option<Vec<(PathBuf, String)>> {
        let mut bound = Vec::new();
        for root in &project.roots {
            let id = match self.repository_ids.get(&root.path) {
                Some(id) => id.clone(),
                None if clock.passed(Poll::Derive) => return None,
                None => match repository_id(root, clock.budget()) {
                    Ok(id) => {
                        self.repository_ids.insert(root.path.clone(), id.clone());
                        id
                    }
                    Err(DerivationFailure::Budget) if clock.budget().is_exhausted() => return None,
                    Err(DerivationFailure::Budget | DerivationFailure::Unreadable) => None,
                },
            };
            bound.extend(id.map(|id| (root.path.clone(), id)));
        }
        Some(bound)
    }
}

/// The repository identity `root`'s documents are published under: the root commit identity when
/// the root is its repository's top level, the project digest when the repository has no derivable
/// identity, and `Some(None)` for a root nested inside a repository, whose document paths are
/// relative to the root and do not name the repository tree's paths.
fn repository_id(root: &Root, budget: &EvalBudget) -> Result<Option<String>, DerivationFailure> {
    use crate::repository_identity::{MAX_ROOT_WALK_COMMITS, RepositoryIdentity, derive};
    let walk = budget
        .linked()
        .bounded_by(Instant::now() + REPOSITORY_WALK_BUDGET);
    Ok(match derive(&root.path, &walk, MAX_ROOT_WALK_COMMITS)? {
        RepositoryIdentity::Git(id) => std::fs::symlink_metadata(root.path.join(".git"))
            .is_ok()
            .then_some(id),
        RepositoryIdentity::Fallback => Some(root.digest.clone()),
    })
}

struct ProjectRun<'a, 'r> {
    kernel: &'a KernelStore,
    scope_id: &'a str,
    roots: &'a [BoundRoot<'r>],
    counters: &'a Counters,
    clock: &'a dyn PassClock,
    report: &'a mut PassReport,
    terminal: &'a mut HashSet<(String, String)>,
    state: &'a mut ProjectState,
    verdicts: u64,
    bytes_read: u64,
}

impl ProjectRun<'_, '_> {
    fn refused(&mut self, deadline: bool) -> Step {
        self.report.failed = true;
        if deadline || self.clock.passed(Poll::Decision) {
            Step::StopPass
        } else {
            Step::StopProject
        }
    }

    fn run(&mut self) -> Step {
        let budget = self.clock.budget();
        let target = match self.kernel.capture_commit_read_target_within_budget(budget) {
            Ok(target) => target,
            Err(error) => return self.refused(error == KernelError::Deadline),
        };
        // A restore gives the store another history, where the cursor's sequences name nothing.
        if self.state.incarnation != Some(target.incarnation) {
            *self.state = ProjectState {
                incarnation: Some(target.incarnation),
                ..ProjectState::default()
            };
        }
        match self.decisions(target) {
            Step::StopPass => Step::StopPass,
            _ => self.applicability(target),
        }
    }

    fn decisions(&mut self, target: CommitReadTarget) -> Step {
        let budget = self.clock.budget();
        let mut after = self.state.cursor.clone();
        let mut prefix = true;
        loop {
            let page = match self.kernel.scope_decisions_after_within_budget(
                self.scope_id,
                VERIFIER_SOURCE_KIND,
                target.through_commit,
                after.as_ref().map(|(seq, id)| (*seq, id.as_str())),
                PAGE_ROWS,
                budget,
            ) {
                Ok(page) if page.is_empty() => return Step::Resolved,
                Ok(page) => page,
                Err(error) => return self.refused(error == KernelError::Deadline),
            };
            let ids: Vec<String> = page.iter().map(|(_, id)| id.clone()).collect();
            let standings = match self
                .kernel
                .claim_standing_at_within_budget(&ids, target, PAGE_ROWS, budget)
            {
                Ok(standings) => standings,
                Err(error) => {
                    return self.refused(error == ClaimFactsError::Kernel(KernelError::Deadline));
                }
            };
            for ((seq, object_id), standing) in page.into_iter().zip(standings) {
                after = Some((seq, object_id.clone()));
                let known = self.state.resolved.contains(&object_id);
                let step = if known {
                    Step::Resolved
                } else if self.clock.passed(Poll::Decision) {
                    return Step::StopPass;
                } else if standing
                    .as_ref()
                    .and_then(|standing| standing.own_admission.as_ref())
                    .is_some_and(|admission| admission.disposition == Disposition::Stale)
                {
                    self.counters.add("skipped_stale", 1);
                    Step::Resolved
                } else {
                    self.decision(&object_id, target)
                };
                match step {
                    Step::Resolved => {
                        self.report.progressed |= !known;
                        if prefix {
                            self.state.resolved.remove(&object_id);
                            self.state.cursor = Some((seq, object_id));
                        } else {
                            self.state.resolved.insert(object_id);
                        }
                    }
                    Step::Unresolved => prefix = false,
                    stop => return stop,
                }
                if self.verdicts >= MAX_VERDICTS || self.bytes_read >= MAX_VERDICTS * MAX_BLOB_BYTES
                {
                    return Step::StopProject;
                }
            }
        }
    }

    /// One more attempt at a decision that ended without a terminal outcome; the K-th resolves it
    /// under `skipped`.
    fn retry(&mut self, object_id: &str, skipped: &str) -> Step {
        self.report.failed = true;
        let attempts = self.state.attempts.entry(object_id.to_owned()).or_insert(0);
        *attempts += 1;
        if *attempts < MAX_ATTEMPTS {
            return Step::Unresolved;
        }
        self.state.attempts.remove(object_id);
        self.counters.add(skipped, 1);
        Step::Resolved
    }

    /// [`Self::retry`] under `unavailable_skipped` for an attempt that may have ended at the
    /// deadline: an attempt that leaves the decision unresolved stops the pass when `deadline`
    /// holds, and the K-th resolves it.
    fn transient(&mut self, object_id: &str, deadline: bool) -> Step {
        match self.retry(object_id, "unavailable_skipped") {
            Step::Unresolved if deadline => Step::StopPass,
            step => step,
        }
    }

    fn decision(&mut self, object_id: &str, target: CommitReadTarget) -> Step {
        let budget = self.clock.budget();
        let classification = match classify(self.kernel, object_id, target, self.roots, budget) {
            Ok(classification) => classification,
            Err(ClassifyError::InvalidDocumentPath) => {
                self.counters.add("considered", 1);
                self.counters.add("unverifiable_invalid_document_path", 1);
                return Step::Resolved;
            }
            Err(ClassifyError::Facts(ClaimFactsError::Kernel(KernelError::Deadline))) => {
                return self.transient(object_id, true);
            }
            // The next pass captures the new incarnation and resets the project's state.
            Err(ClassifyError::Facts(ClaimFactsError::IncarnationMismatch)) => {
                return self.refused(false);
            }
            Err(ClassifyError::Facts(_)) => return self.transient(object_id, false),
        };
        let input = match &classification.class {
            Class::NotInput => return Step::Resolved,
            Class::Unverifiable(reason) => {
                self.counters.add("considered", 1);
                self.counters
                    .add(&format!("unverifiable_{}", reason.as_str()), 1);
                return Step::Resolved;
            }
            Class::Verifiable(input) => input,
        };
        let oid = input.claim.oid().to_owned();
        let attestation = Identity::of(object_id, &oid).object_id;
        match self
            .kernel
            .object_states_within_budget(budget, std::slice::from_ref(&attestation))
        {
            Ok((_, states)) if states.first().is_some_and(Option::is_some) => {
                self.counters.add("skipped_attested", 1);
                return Step::Resolved;
            }
            Ok(_) => {}
            Err(error) => return self.refused(error == KernelError::Deadline),
        }
        if self.terminal.contains(&(object_id.to_owned(), oid.clone())) {
            return Step::Resolved;
        }
        let evaluation = match &input.root {
            Ok(root) => verify(root, &input.claim, &Reads(self.clock)),
            Err(reason) => Evaluation {
                verdict: Verdict::Unavailable(*reason),
                bytes_read: 0,
                blob_oid: None,
            },
        };
        self.verdicts += 1;
        self.bytes_read = self.bytes_read.saturating_add(evaluation.bytes_read);
        self.counters.add("considered", 1);
        self.counters.add("bytes_read", evaluation.bytes_read);
        let bucket = match evaluation.verdict {
            Verdict::Match => "match".to_owned(),
            Verdict::Mismatch => "mismatch".to_owned(),
            Verdict::Unavailable(reason) => format!("unavailable_{}", reason.as_str()),
        };
        self.counters.add(&bucket, 1);
        match evaluation.verdict {
            Verdict::Unavailable(reason) if reason.is_terminal() => {
                self.terminal.insert((object_id.to_owned(), oid));
                return Step::Resolved;
            }
            Verdict::Unavailable(reason) => {
                let deadline = matches!(reason, Reason::Deadline(_));
                return self.transient(object_id, deadline);
            }
            Verdict::Match | Verdict::Mismatch => {}
        }
        if self.clock.passed(Poll::Commit) {
            return self.transient(object_id, true);
        }
        let outcome = attest(self.kernel, &classification, &evaluation, budget);
        self.outcome(object_id, evaluation.verdict, outcome)
    }

    /// Counts `outcome` under its effect counter, after the receipt or refusal returned.
    fn outcome(&mut self, object_id: &str, verdict: Verdict, outcome: Outcome) -> Step {
        let counter = match outcome {
            Outcome::Committed(_) => "attestation_committed",
            Outcome::StaleCommitted(_) => "stale_committed",
            // A replayed `MarkStale` receipt, or a decision found `Stale` in the envelope, left the
            // decision `Stale`; neither is this pass's effect.
            Outcome::Replayed(_) if verdict == Verdict::Match => "attestation_replayed",
            Outcome::Replayed(_) | Outcome::Skipped(_) => return Step::Resolved,
            Outcome::Conflict => "commit_conflict",
            Outcome::InvalidWriteSet => "commit_refused_invalid_write_set",
            Outcome::WitnessDegraded => {
                self.counters.add("witness_degraded", 1);
                return self.retry(object_id, "witness_degraded_skipped");
            }
            Outcome::Changed => {
                self.counters.add("commit_refused_changed", 1);
                return self.transient(object_id, false);
            }
            // An `Io` refusal can follow a durable commit; the next attempt finds the attestation
            // in the registry or the decision `Stale` and skips it.
            Outcome::Refused(error) => {
                self.counters.add(refused_counter(error), 1);
                return self.transient(object_id, error == KernelError::Deadline);
            }
        };
        self.counters.add(counter, 1);
        Step::Resolved
    }
}

fn refused_counter(error: KernelError) -> &'static str {
    match error {
        KernelError::Busy => "commit_refused_busy",
        KernelError::Deadline => "commit_refused_deadline",
        KernelError::Io => "commit_refused_io",
        _ => "commit_refused_other",
    }
}

impl ProjectRun<'_, '_> {
    fn applicability(&mut self, target: CommitReadTarget) -> Step {
        let budget = self.clock.budget();
        let mut evaluated = 0;
        loop {
            let after = self.state.applicability.clone();
            let page = match self.kernel.scope_decisions_of_source_after_within_budget(
                self.scope_id,
                VERIFIER_SOURCE_KIND,
                target.through_commit,
                after.as_ref().map(|(seq, id)| (*seq, id.as_str())),
                PAGE_ROWS,
                budget,
            ) {
                Ok(page) if page.is_empty() => {
                    self.state.applicability = None;
                    return Step::Resolved;
                }
                Ok(page) => page,
                Err(error) => return self.refused(error == KernelError::Deadline),
            };
            let ids: Vec<String> = page.iter().map(|(_, id)| id.clone()).collect();
            let standings = match self
                .kernel
                .claim_standing_at_within_budget(&ids, target, PAGE_ROWS, budget)
            {
                Ok(standings) => standings,
                Err(error) => {
                    return self.refused(error == ClaimFactsError::Kernel(KernelError::Deadline));
                }
            };
            for ((seq, object_id), standing) in page.into_iter().zip(standings) {
                let active = standing
                    .as_ref()
                    .and_then(|standing| standing.own_admission.as_ref())
                    .is_some_and(|admission| admission.disposition == Disposition::Active);
                if active {
                    if self.clock.passed(Poll::Decision) {
                        return Step::StopPass;
                    }
                    if evaluated == MAX_VERDICTS {
                        return Step::StopProject;
                    }
                    evaluated += 1;
                    match self.applicable(&object_id, target) {
                        Step::Resolved => {}
                        stop => return stop,
                    }
                }
                self.state.applicability = Some((seq, object_id));
            }
        }
    }

    fn applicable(&mut self, object_id: &str, target: CommitReadTarget) -> Step {
        let budget = self.clock.budget();
        let republished = match applicability::evaluate(self.kernel, object_id, target, budget) {
            Ok(Applicability::Current) => {
                self.counters.add("applicability_current", 1);
                return Step::Resolved;
            }
            Ok(Applicability::Unavailable) => {
                self.counters.add("applicability_unavailable", 1);
                return Step::Resolved;
            }
            Ok(Applicability::Republished(republished)) => republished,
            Err(error) => return self.refused(error == KernelError::Deadline),
        };
        if self.clock.passed(Poll::Commit) {
            return Step::StopPass;
        }
        let counter =
            match applicability::mark_republished(self.kernel, target, &republished, budget) {
                Outcome::StaleCommitted(_) | Outcome::Committed(_) => {
                    self.report.progressed = true;
                    "applicability_stale"
                }
                Outcome::Replayed(_) | Outcome::Skipped(_) => return Step::Resolved,
                Outcome::Changed => "commit_refused_changed",
                Outcome::Conflict => "commit_conflict",
                Outcome::InvalidWriteSet | Outcome::WitnessDegraded => {
                    "commit_refused_invalid_write_set"
                }
                Outcome::Refused(error) => {
                    self.counters.add(refused_counter(error), 1);
                    self.report.failed = true;
                    if error == KernelError::Deadline {
                        return Step::StopPass;
                    }
                    return Step::Resolved;
                }
            };
        self.report.failed |= counter != "applicability_stale";
        self.counters.add(counter, 1);
        Step::Resolved
    }
}

#[cfg(test)]
mod tests {
    use super::super::classify::Unverifiable;
    use super::super::{EntryClass, ObjectClass};
    use super::*;

    #[test]
    fn every_reason_and_unverifiable_class_names_a_counter() {
        let reasons = [
            Reason::PathAbsent,
            Reason::NotABlob(EntryClass::Tree),
            Reason::ObjectUnreadable(ObjectClass::Blob),
            Reason::OverBudget,
            Reason::Deadline(Stage::Blob),
            Reason::RepositoryUnbound,
            Reason::RepositoryUnopenable,
        ];
        let classes = [
            Unverifiable::NoAnchor,
            Unverifiable::AnchorNotGit,
            Unverifiable::NoCitation,
            Unverifiable::CitationNotDocument,
            Unverifiable::EvidenceRetired,
            Unverifiable::DescriptorRetired,
            Unverifiable::DigestMissing,
            Unverifiable::SelfSource,
        ];
        let names = reasons
            .iter()
            .map(|reason| format!("unavailable_{}", reason.as_str()))
            .chain(
                classes
                    .iter()
                    .map(|class| format!("unverifiable_{}", class.as_str())),
            )
            .chain(["unverifiable_invalid_document_path".to_owned()]);
        for name in names {
            assert!(COUNTERS.contains(&name.as_str()), "{name}");
        }
    }
}
