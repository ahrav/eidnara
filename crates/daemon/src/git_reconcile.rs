//! One reconciliation episode over one repository's retained commit sources: the retained inventory is read at a fixed kernel sequence, every commit reachable from the permitted refs is traversed under a total work bound, and only then are the retained commits the traversal never reached retired. A ref that cannot be read or is not a full name, a shallow repository, a traversal that exceeds its bound, a ref that moved since the traversal began, a cancelled budget, an unreadable or oversized object, a retained row of another object format, or a retained row whose identity cannot be read ends the episode without retiring anything: absence is derived only from a complete, certified inventory. The repository is opened as the publisher opens it, with replacement refs ignored, so a `refs/replace` entry cannot rewrite the graph the walk certifies. An empty permitted set is a verified empty selection that retires every retained commit of the repository.
//!
//! Retirement runs page by page through receipt-keyed kernel commits, each preceded by another read of the ref tips, so the uncertified window is one ref read wide. The invalidations reach the projection as tombstones through the shared export, and the evidence stays retained under its holds. A retired descriptor's object id is never reused, so a wrongful retirement cannot be undone by publishing the commit again; every refusal above exists to keep retirement from ever being wrongful.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};

use kernel::applicability::EvalBudget;
use kernel::source_identity::{OccurrenceClass, identity_digest};
use kernel::{
    CommitIntent, Envelope, KernelError, KernelStore, LiveDescriptor, SourceDescriptorPolicy,
};
use tokio_util::sync::CancellationToken;

use crate::git_sources::{self, GitRefusal, RepositoryBinding};
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
    /// Live `git_commits` descriptors the inventory may read across all repositories; the kernel keys descriptors by lineage, not repository.
    pub max_scanned: NonZeroUsize,
    /// Retained descriptors of the repository the episode may hold in total.
    pub max_retained: NonZeroUsize,
    /// Commits the traversal may visit in total.
    pub max_commits: NonZeroUsize,
    /// Decoded bytes one commit object may occupy, applied as the object store's allocation limit.
    pub max_object_bytes: NonZeroU64,
}

/// Why an episode retired nothing, or stopped after the pages it had already retired. No variant implies that any unvisited source is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileBlocked {
    /// The budget was cancelled, its deadline passed, or the grant was invalidated before the phase named ran.
    Cancelled(ReconcilePhase),
    /// The gate denied the sweep or its lease before the inventory was read; nothing was retired.
    Denied(Denial),
    Repository(GitRefusal),
    /// A permitted ref is not a full ref name, does not exist, or does not resolve to a commit.
    RefUnresolved(String),
    /// The traversal reached the commit bound before it reached every commit.
    WorkExceeded {
        max: usize,
    },
    /// The inventory read `max_scanned` live `git_commits` descriptors without reaching the end of the class.
    ScannedExceeded {
        max: usize,
    },
    /// The retained inventory exceeds the episode's bound.
    RetainedExceeded {
        max: usize,
    },
    /// A retained row names an object format other than the repository's, so no traversal of this repository can judge it.
    ObjectFormatMismatch(String),
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
    /// The kernel sequence the retained inventory was read at; `0` when the episode was cancelled before reading it.
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
    probes: Vec<(Probe, Box<dyn FnMut() + 'a>)>,
}

/// A point inside an episode where a test may act on the repository or the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// After each inventory page is read and before the next is requested.
    AfterInventoryPage,
    /// After the traversal has completed and before the ref tips are certified.
    AfterTraversal,
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
            probes: Vec::new(),
        }
    }

    /// Runs `hook` at `probe`, so a test can move a ref or cancel the budget inside the window the episode must detect.
    #[cfg(feature = "test-support")]
    pub fn with_probe_for_test(mut self, probe: Probe, hook: impl FnMut() + 'a) -> Self {
        self.probes.push((probe, Box::new(hook)));
        self
    }

    #[cfg(feature = "test-support")]
    fn probe(&mut self, probe: Probe) {
        for (at, hook) in &mut self.probes {
            if *at == probe {
                hook();
            }
        }
    }

    #[cfg(not(feature = "test-support"))]
    fn probe(&mut self, _: Probe) {}

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
        let mut report = ReconcileReport {
            snapshot: 0,
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
        report.snapshot = self
            .kernel
            .tip_within_budget(budget)
            .map_err(|error| cancelled_or_failed(error, ReconcilePhase::Inventory))?;
        let repo = git_sources::open(&scope.binding.path, bounds.max_object_bytes)?;
        let format = git_sources::object_format(repo.object_hash())?;
        // The walk stops at a shallow boundary while the store may still hold the ancestors beyond it, so the traversal cannot certify those ancestors' descriptors as absent.
        if repo.is_shallow() {
            return Err(GitRefusal::Shallow.into());
        }
        let retained = self.inventory(scope, bounds, budget, format, report.snapshot)?;
        report.retained = retained.len();
        check(ReconcilePhase::Traversal)?;
        let traversal = traverse(&repo, scope, bounds, budget)?;
        self.probe(Probe::AfterTraversal);
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
            report.retired += self.retire(scope, budget, report.snapshot, page)?;
        }
        Ok(())
    }

    /// Returns the live `git_commits` descriptors of the repository at `snapshot`. Each descriptor's object format must match `format`, because the repository walk cannot classify an id of another format.
    fn inventory(
        &mut self,
        scope: &ReconcileScope,
        bounds: InventoryBounds,
        budget: &EvalBudget,
        format: &str,
        snapshot: i64,
    ) -> Result<Vec<Retained>, Stop> {
        let mut retained = Vec::new();
        let mut scanned = 0usize;
        let mut after = None;
        let exceeded = || ReconcileBlocked::ScannedExceeded {
            max: bounds.max_scanned.get(),
        };
        loop {
            budget
                .check()
                .map_err(|_| ReconcileBlocked::Cancelled(ReconcilePhase::Inventory))?;
            // A page asks for no more rows than the scan bound still allows, so the bound limits what the kernel reads and decodes, not only what is counted afterwards. The loop only continues while `scanned` is below the bound, so the allowance is at least one.
            let allowance = NonZeroUsize::new(bounds.max_scanned.get() - scanned)
                .ok_or_else(exceeded)?
                .min(bounds.page_rows);
            let page = self
                .kernel
                .live_source_descriptors(
                    OccurrenceClass::GitCommits,
                    snapshot,
                    after.as_deref(),
                    allowance,
                    budget,
                )
                .map_err(|error| cancelled_or_failed(error, ReconcilePhase::Inventory))?;
            self.probe(Probe::AfterInventoryPage);
            for descriptor in page.rows {
                scanned += 1;
                if !names_repository(&descriptor, &scope.binding.repository_id) {
                    continue;
                }
                if retained.len() == bounds.max_retained.get() {
                    return Err(ReconcileBlocked::RetainedExceeded {
                        max: bounds.max_retained.get(),
                    }
                    .into());
                }
                let object_format = identity_field(&descriptor, "object_format")
                    .ok_or(KernelError::CorruptCanonicalRow)?;
                if object_format != format {
                    return Err(
                        ReconcileBlocked::ObjectFormatMismatch(object_format.to_owned()).into(),
                    );
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
                // The lookahead says another row exists; a bound already spent refuses here rather than reading it.
                Some(_) if scanned == bounds.max_scanned.get() => return Err(exceeded().into()),
                Some(next) => after = Some(next),
                None => return Ok(retained),
            }
        }
    }

    /// One receipt-keyed commit that invalidates the page's live descriptors; returns how many it invalidated, which excludes descriptors another writer invalidated first and every replayed receipt.
    ///
    /// The wait for the kernel writer stops at the budget's deadline or interrupt, and the budget is checked again once the writer is held, so a page whose budget ran out behind another writer is cancelled rather than committed.
    fn retire(
        &self,
        scope: &ReconcileScope,
        budget: &EvalBudget,
        snapshot: i64,
        ids: &[String],
    ) -> Result<usize, Stop> {
        let digest = identity_digest(ids.join("\u{1f}").as_bytes());
        let mut retired = 0usize;
        let intent = CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: format!(
                "git-retire:{}:{snapshot}:{digest}",
                scope.binding.repository_id
            ),
            request_digest: digest,
            actor: scope.binding.repository_id.clone(),
            cause: "git inventory reconciliation".to_owned(),
        };
        let operation = |envelope: &mut Envelope<'_>| {
            if budget.is_exhausted() {
                return Err(KernelError::Deadline);
            }
            for id in ids {
                if envelope
                    .object_state(id)?
                    .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
                {
                    envelope.retire_observation(id)?;
                    retired += 1;
                }
            }
            Ok(String::new())
        };
        let receipt = self
            .kernel
            .commit_within_budget(budget, intent, operation)
            .map_err(|error| match error {
                KernelError::Deadline => ReconcileBlocked::Cancelled(ReconcilePhase::Retirement),
                error => ReconcileBlocked::Retire(error),
            })?;
        Ok(if receipt.replayed { 0 } else { retired })
    }
}

/// A kernel read that could not take a pooled connection within the budget ends the episode as a cancellation of `phase`; any other kernel error is the episode's failure.
fn cancelled_or_failed(error: KernelError, phase: ReconcilePhase) -> Stop {
    match error {
        KernelError::Deadline => Stop::Blocked(ReconcileBlocked::Cancelled(phase)),
        error => Stop::Failed(error),
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
    let hash = repo.object_hash();
    // The commit graph is a cache of parent edges that nothing below verifies, so the walk reads every commit from the object store.
    let walk = repo
        .rev_walk(tips.iter().copied())
        .use_commit_graph(false)
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
        // The store does not rehash what it returns, so bytes stored under a commit's id that hash to another commit would give the walk that other commit's parents. As in `read_selection`, the id must identify the bytes before they count.
        let oid = info.id.to_string();
        let object = info
            .id()
            .object()
            .map_err(|_| GitRefusal::Unreadable(oid.clone()))?;
        let actual = gix::objs::compute_hash(hash, object.kind, &object.data)
            .map_err(|_| GitRefusal::Unreadable(oid.clone()))?;
        if actual != info.id {
            return Err(GitRefusal::HashMismatch(oid).into());
        }
        reachable.insert(oid);
    }
    Ok(Traversal { tips, reachable })
}

/// The commit each permitted ref points at. A ref is looked up only by its full name, so a tag or another namespace's ref of the same short name cannot stand in for it.
fn ref_tips(repo: &gix::Repository, refs: &[String]) -> Result<Vec<gix::ObjectId>, Stop> {
    refs.iter()
        .map(|name| {
            let unresolved = || ReconcileBlocked::RefUnresolved(name.clone());
            let full = gix::refs::FullName::try_from(name.as_str()).map_err(|_| unresolved())?;
            let mut reference = repo
                .try_find_reference(full.as_ref())
                .ok()
                .flatten()
                .filter(|reference| reference.name() == full.as_ref())
                .ok_or_else(unresolved)?;
            let id = reference.peel_to_id().map_err(|_| unresolved())?.detach();
            match repo.try_find_header(id) {
                Ok(Some(header)) if header.kind() == gix::object::Kind::Commit => Ok(id),
                _ => Err(unresolved().into()),
            }
        })
        .collect()
}
