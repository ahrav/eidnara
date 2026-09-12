//! One reconciliation episode over one repository's retained commit sources: the retained inventory is read at a fixed kernel sequence, every commit reachable from the permitted refs is traversed under a total work bound, and only then are the retained commits the traversal never reached retired. A ref that cannot be read, a traversal that exceeds its bound, a ref that moved since the traversal began, a cancelled budget, an unreadable object, or a retained row whose identity cannot be read ends the episode without retiring anything: absence is derived only from a complete, certified inventory. An empty permitted set is a verified empty selection that retires every retained commit of the repository.
//!
//! Retirement runs page by page through receipt-keyed kernel commits, each preceded by another read of the ref tips, so the uncertified window is one ref read wide. The invalidations reach the projection as tombstones through the shared export, and the evidence stays retained under its holds.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::{OccurrenceClass, identity_digest};
use kernel::{CommitIntent, KernelError, KernelStore, LiveDescriptor, SourceDescriptorPolicy};
use tokio_util::sync::CancellationToken;

use crate::git_sources::{GitRefusal, RepositoryBinding};
use crate::projection_gates::{Denial, EntryPoint, HookGate, ProjectionHook};

const PRODUCER: &str = "eidnara-daemon/git-reconcile";

/// The refs a repository's retained commits must stay reachable from. An empty list is an explicit verified-empty selection, not an unknown one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileScope {
    pub binding: RepositoryBinding,
    /// Full ref names, such as `refs/heads/main`.
    pub permitted_refs: Vec<String>,
}

/// Total work one episode may do, judged before anything is retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryBounds {
    /// Retained descriptors one inventory page reads and one retirement commit invalidates.
    pub page_rows: NonZeroUsize,
    /// Retained descriptors of the repository the episode may hold in total.
    pub max_retained: NonZeroUsize,
    /// Commits the traversal may visit in total.
    pub max_commits: NonZeroUsize,
}

/// Why an episode retired nothing, or stopped after the pages it had already retired. No variant implies that any unvisited source is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileBlocked {
    /// The budget was cancelled, its deadline passed, or the grant was invalidated before the phase named ran.
    Cancelled(ReconcilePhase),
    /// The gate denied the sweep or its lease before the inventory was read; nothing was retired.
    Denied(Denial),
    Repository(GitRefusal),
    /// A permitted ref does not exist or does not resolve to a commit.
    RefUnresolved(String),
    /// The traversal reached the commit bound before it reached every commit.
    WorkExceeded {
        max: usize,
    },
    /// The retained inventory exceeds the episode's bound.
    RetainedExceeded {
        max: usize,
    },
    /// A permitted ref moved after the traversal began, so the traversal describes a selection that no longer exists.
    RefsChanged,
    /// A retirement page's commit failed; the pages before it stand.
    Retire(KernelError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcilePhase {
    Inventory,
    Traversal,
    Retirement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileEnd {
    /// Every retained commit was either reachable or retired.
    Complete,
    Blocked(ReconcileBlocked),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// The kernel sequence the retained inventory was read at.
    pub snapshot: i64,
    /// Retained commits of the repository at `snapshot`, under any git policy version.
    pub retained: usize,
    /// Commits the traversal visited; `None` when the traversal did not complete.
    pub reachable: Option<usize>,
    /// Retained commits the traversal reached and left alone.
    pub preserved: usize,
    /// Descriptor objects this episode's retirement commits invalidated.
    pub retired: usize,
    pub end: ReconcileEnd,
}

pub struct GitReconciler<'a> {
    kernel: &'a KernelStore,
    #[cfg(feature = "test-support")]
    after_traversal: Option<Box<dyn FnMut() + 'a>>,
}

enum Stop {
    Blocked(ReconcileBlocked),
    Failed(KernelError),
}

impl From<ReconcileBlocked> for Stop {
    fn from(blocked: ReconcileBlocked) -> Self {
        Stop::Blocked(blocked)
    }
}

impl From<KernelError> for Stop {
    fn from(error: KernelError) -> Self {
        Stop::Failed(error)
    }
}

impl From<GitRefusal> for Stop {
    fn from(refusal: GitRefusal) -> Self {
        Stop::Blocked(ReconcileBlocked::Repository(refusal))
    }
}

/// The retained descriptors of one repository at one snapshot with the commit id each names.
struct Retained {
    object_id: String,
    oid: String,
}

/// The commit ids reachable from the permitted refs and the tips they were walked from.
struct Traversal {
    tips: Vec<gix::ObjectId>,
    reachable: BTreeSet<String>,
}

impl<'a> GitReconciler<'a> {
    pub fn new(kernel: &'a KernelStore) -> Self {
        Self {
            kernel,
            #[cfg(feature = "test-support")]
            after_traversal: None,
        }
    }

    /// Runs `hook` once the traversal has completed and before the ref tips are certified, so a test can move a ref or cancel the budget inside the window the episode must detect.
    #[cfg(feature = "test-support")]
    pub fn with_after_traversal_for_test(mut self, hook: impl FnMut() + 'a) -> Self {
        self.after_traversal = Some(Box::new(hook));
        self
    }

    /// Runs one episode: inventory at the kernel tip, traversal from the permitted refs, then retirement of the unreached commits.
    ///
    /// # Errors
    ///
    /// Returns the kernel's error when a read fails without a durable fact to reconcile against, or when a retained row's identity cannot be read. Every refusal ends the episode in the report.
    pub fn run_episode(
        &mut self,
        gate: &HookGate,
        scope: &ReconcileScope,
        bounds: InventoryBounds,
        budget: &EvalBudget,
    ) -> Result<ReconcileReport, KernelError> {
        let snapshot = self.kernel.tip()?;
        let mut report = ReconcileReport {
            snapshot,
            retained: 0,
            reachable: None,
            preserved: 0,
            retired: 0,
            end: ReconcileEnd::Complete,
        };
        let admissions = match gate.admit_all(
            &[ProjectionHook::GitSweeps, ProjectionHook::GitLeases],
            EntryPoint::Dispatch,
        ) {
            Ok(admissions) => admissions,
            Err(denial) => {
                report.end = ReconcileEnd::Blocked(ReconcileBlocked::Denied(denial));
                return Ok(report);
            }
        };
        match self.drive(
            scope,
            bounds,
            budget,
            &admissions[0].invalidated,
            &mut report,
        ) {
            Ok(()) => Ok(report),
            Err(Stop::Blocked(blocked)) => {
                report.end = ReconcileEnd::Blocked(blocked);
                Ok(report)
            }
            Err(Stop::Failed(error)) => Err(error),
        }
    }

    fn drive(
        &mut self,
        scope: &ReconcileScope,
        bounds: InventoryBounds,
        budget: &EvalBudget,
        invalidated: &CancellationToken,
        report: &mut ReconcileReport,
    ) -> Result<(), Stop> {
        // Every grant of one request carries the same token, so the first one speaks for the sweep and its lease; an invalidated grant ends the phase as a cancelled budget does.
        let check = |phase| {
            if budget.check().is_err() || invalidated.is_cancelled() {
                return Err(Stop::Blocked(ReconcileBlocked::Cancelled(phase)));
            }
            Ok(())
        };
        check(ReconcilePhase::Inventory)?;
        let retained = self.inventory(scope, bounds, report.snapshot)?;
        report.retained = retained.len();
        check(ReconcilePhase::Traversal)?;
        let repo = gix::open_opts(&scope.binding.path, gix::open::Options::isolated())
            .map_err(|_| GitRefusal::Open)?;
        let traversal = traverse(&repo, scope, bounds, budget)?;
        #[cfg(feature = "test-support")]
        if let Some(hook) = self.after_traversal.as_mut() {
            hook();
        }
        report.reachable = Some(traversal.reachable.len());
        let (preserved, excluded): (Vec<_>, Vec<_>) = retained
            .iter()
            .partition(|retained| traversal.reachable.contains(&retained.oid));
        report.preserved = preserved.len();
        let ids: Vec<String> = excluded
            .iter()
            .map(|retained| retained.object_id.clone())
            .collect();
        for page in ids.chunks(bounds.page_rows.get()) {
            check(ReconcilePhase::Retirement)?;
            // A ref that moved since the walk began makes the walk describe another selection; the pages already retired were certified by the reads before them.
            if ref_tips(&repo, &scope.permitted_refs)? != traversal.tips {
                return Err(ReconcileBlocked::RefsChanged.into());
            }
            self.retire(scope, report.snapshot, page)?;
            report.retired += page.len();
        }
        Ok(())
    }

    /// Every live `git_commits` descriptor of the repository at `snapshot`, under any git policy version.
    fn inventory(
        &self,
        scope: &ReconcileScope,
        bounds: InventoryBounds,
        snapshot: i64,
    ) -> Result<Vec<Retained>, Stop> {
        let mut retained = Vec::new();
        let mut after = None;
        loop {
            let page = self.kernel.live_source_descriptors(
                OccurrenceClass::GitCommits,
                snapshot,
                after.as_deref(),
                bounds.page_rows,
            )?;
            for descriptor in page.rows {
                if !names_repository(&descriptor, &scope.binding.repository_id) {
                    continue;
                }
                if retained.len() == bounds.max_retained.get() {
                    return Err(ReconcileBlocked::RetainedExceeded {
                        max: bounds.max_retained.get(),
                    }
                    .into());
                }
                let oid = identity_field(&descriptor, "oid")
                    .ok_or(KernelError::CorruptCanonicalRow)?
                    .to_owned();
                retained.push(Retained {
                    object_id: descriptor.object_id,
                    oid,
                });
            }
            match page.next {
                Some(next) => after = Some(next),
                None => return Ok(retained),
            }
        }
    }

    /// Retires the page's descriptors that are still live in one receipt-keyed commit.
    fn retire(&self, scope: &ReconcileScope, snapshot: i64, ids: &[String]) -> Result<(), Stop> {
        let digest = identity_digest(ids.join("\u{1f}").as_bytes());
        self.kernel
            .commit(
                CommitIntent {
                    producer: PRODUCER.to_owned(),
                    operation_key: format!(
                        "git-retire:{}:{snapshot}:{digest}",
                        scope.binding.repository_id
                    ),
                    request_digest: digest,
                    actor: scope.binding.repository_id.clone(),
                    cause: "git inventory reconciliation".to_owned(),
                },
                |envelope| {
                    for id in ids {
                        if envelope
                            .object_state(id)?
                            .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
                        {
                            envelope.retire_observation(id)?;
                        }
                    }
                    Ok(String::new())
                },
            )
            .map_err(|error| Stop::Blocked(ReconcileBlocked::Retire(error)))?;
        Ok(())
    }
}

fn identity_field<'d>(descriptor: &'d LiveDescriptor, name: &str) -> Option<&'d str> {
    descriptor
        .detail
        .identity
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

/// A descriptor belongs to the episode when it names the repository under any git source policy version; the permitted-ref selection does not depend on the version the bytes were published under.
fn names_repository(descriptor: &LiveDescriptor, repository_id: &str) -> bool {
    identity_field(descriptor, "repository_id") == Some(repository_id)
        && matches!(
            descriptor.detail.source_policy,
            SourceDescriptorPolicy::Git { .. }
        )
}

/// The commit ids reachable from the permitted refs, with the tips they were walked from.
fn traverse(
    repo: &gix::Repository,
    scope: &ReconcileScope,
    bounds: InventoryBounds,
    budget: &EvalBudget,
) -> Result<Traversal, Stop> {
    let tips = ref_tips(repo, &scope.permitted_refs)?;
    let mut reachable = BTreeSet::new();
    if tips.is_empty() {
        return Ok(Traversal { tips, reachable });
    }
    let walk = repo
        .rev_walk(tips.iter().copied())
        .all()
        .map_err(|_| GitRefusal::Unreadable(tips[0].to_string()))?;
    for info in walk {
        budget
            .check()
            .map_err(|_| ReconcileBlocked::Cancelled(ReconcilePhase::Traversal))?;
        // The walk reports the commit it could not read only through its error; the parent it was reached from is not recoverable here, so the refusal names the walk.
        let info = info.map_err(|_| GitRefusal::Unreadable("traversal".to_owned()))?;
        if reachable.len() == bounds.max_commits.get() {
            return Err(ReconcileBlocked::WorkExceeded {
                max: bounds.max_commits.get(),
            }
            .into());
        }
        reachable.insert(info.id.to_string());
    }
    Ok(Traversal { tips, reachable })
}

fn ref_tips(repo: &gix::Repository, refs: &[String]) -> Result<Vec<gix::ObjectId>, Stop> {
    refs.iter()
        .map(|name| {
            let mut reference = repo
                .try_find_reference(name.as_str())
                .ok()
                .flatten()
                .ok_or_else(|| ReconcileBlocked::RefUnresolved(name.clone()))?;
            let id = reference
                .peel_to_id()
                .map_err(|_| ReconcileBlocked::RefUnresolved(name.clone()))?
                .detach();
            match repo.try_find_header(id) {
                Ok(Some(header)) if header.kind() == gix::object::Kind::Commit => Ok(id),
                _ => Err(ReconcileBlocked::RefUnresolved(name.clone()).into()),
            }
        })
        .collect()
}
