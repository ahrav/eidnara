//! The daemon's owner of the search projection lifecycle. One owner per data home holds the admission owner, the selection manager, and the running identity, and advances the durable lifecycle record one bounded slice at a time: a recorded rebuild or authorized recovery is built, selected, and completed across scheduled slices; a completed record is reopened and revalidated once and judged on its coverage after that; a disabled record admits nothing. Every slice first refreshes admission with what the daemon has actually opened: the selected family's own coverage once one is open, the unregistered observation before then, and no coverage at all for a selected family that refuses to be read. Readers pin the selected family through the owner and revalidate canonical authorization at use. Nothing here records intent on its own: a rebuild or recovery starts from an explicit request, and a restart resumes the record it finds without renewing its allowance. A Current family catches up in the same slices: the construction hold outlives completion, so each slice applies the commits since the family's checkpoint under that hold and acknowledges them. Holds die with the kernel's lease, so after a restart the family serves what it has until it trails the kernel past the freshness limit, at which point every hook is denied until a rebuild is requested. Embedding maintenance runs one supervisor at a time, rotated round-robin across the bound projects the daemon reports through a roster, each for a fixed tenure of slices; a slice that finds the tenure over or the roster changed hands the supervisor back to the loop, which joins it before the next slice starts the next one.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use host_runtime::local_embeddings::{LocalEmbeddingsComponent, LocalEmbeddingsStatus};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, CommitPageBounds, KernelStore, ProjectScope, SourceHoldAdmission,
    SourceHoldBinding, SourceHoldBounds, SourcePageBounds,
};
use retrieval::batch::{BatchBounds, VectorGeneration};
use retrieval::coverage::CoverageBounds;
use retrieval::dispatch::EpisodeGrant;
use retrieval::{PersistBounds, ProjectionIdentity};
use tokio_util::sync::CancellationToken;

use crate::coverage::ProjectionCoverage;
use crate::embedding_dispatch::{DispatchBounds, InputEnvelope, LaneIdentity};
use crate::embedding_supervisor::{DrainReport, Maintained, SliceBounds, Unresolved};
use crate::projection_admission::{
    AdmissionInputs, Closed, InputRefusal, ProjectionAdmission, Refresh, SelectedProjection,
};
use crate::projection_gates::{
    Denial, EntryPoint, Gate, InvalidationIdentity, ProjectionHook, RuntimeManifest,
};
use crate::projection_lifecycle::{
    ControlState, IntentRefusal, LifecycleIntent, LifecycleRequest, MAX_RECORD_BYTES,
    ProjectionLifecycle, Recorded, Transition,
};
#[cfg(feature = "test-support")]
use crate::search_catchup::EpisodeEvent;
use crate::search_catchup::{
    CatchUpConsumer, EpisodeBounds, EpisodeEnd, EpisodeReport, SearchCatchUp,
};
use crate::search_replacement::selection::disable::{DisableEvent, MaintenanceHandle};
use crate::search_replacement::selection::recovery::{RecoveryFailure, RecoveryProgress};
use crate::search_replacement::selection::retirement::DISPOSITION_ROW_BYTES;
use crate::search_replacement::selection::{SearchReader, SearchSelection};
use crate::search_replacement::{BuildError, ReplacementSpec, RetirementBounds};
use crate::search_seed::SeedBounds;

/// The construction contract every occurrence tuple and lineage identifier follows; a revision rebuilds the projection.
pub const IDENTITY_CONTRACT_VERSION: &str = "search-projection-identity-v3";
/// The source policy that selects classes, representations, and spans; a revision rebuilds the projection.
pub const PROJECTION_POLICY_VERSION: &str = "source-policy.v1";
/// How far before the record's deadline a slice bounded by it must end, so a later wall-clock read of that deadline cannot land before the bound.
const DEADLINE_MARGIN_MS: u64 = 1_000;
/// The largest occurrence tuple the identity contract admits: seven identity fields of at most 512 bytes each, with their names, the class, revision, representation, and span framing.
const MAX_TUPLE_BYTES: u64 = 4096;
/// How long the scheduler waits after a slice that did not advance the record, so a blocked record is retried without spinning.
pub const SLICE_IDLE: Duration = Duration::from_secs(5);
/// How long a repeated report of one kind waits before its changed detail is printed again.
const REPORT_REPEAT_INTERVAL: Duration = Duration::from_secs(60);
/// How often a budgeted wait for the manager retries while a slice holds it.
const LOCK_POLL: Duration = Duration::from_millis(1);
/// Owner slices one project's supervisor runs before the next bound project takes over.
pub const MAINTENANCE_TENURE_SLICES: u32 = 4;
/// The daemon's own projection serves local reads, so maintenance judges eligibility for local egress; a remote destination would retire every non-normal row as provider-sensitive.
const MAINTENANCE_DESTINATION: ArtifactDestination = ArtifactDestination::Local;

/// The bound projects maintenance rotates across, each under the project digest that orders the rotation, read at each slice so bindings take effect at the next one.
pub type ProjectRoster = Arc<dyn Fn() -> Vec<(String, ProjectScope)> + Send + Sync>;

/// Why a slice could not derive a projection identity or a bounded specification from the daemon's inputs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecRefusal {
    #[error("the local embeddings lane is not ready: {0}")]
    LaneNotReady(&'static str),
    #[error(transparent)]
    Inputs(#[from] InputRefusal),
    #[error("limit {0} is too small to bound a slice")]
    TooSmall(&'static str),
    #[error("limit {0} does not fit its bound")]
    LimitRange(&'static str),
    #[error("the kernel incarnation could not be read: {0}")]
    Kernel(String),
}

/// Why `prepare` produced no identity: the records refused, or a manager with a running supervisor must be joined before it can be replaced.
enum Unprepared {
    Refused(SpecRefusal),
    Rotate(MaintenanceHandle),
}

/// What one scheduled slice found or did.
#[derive(Debug)]
pub enum SliceOutcome {
    /// Admission stays closed because the daemon cannot derive an identity or bounds.
    Closed(SpecRefusal),
    /// No lifecycle record and no selected family.
    Unregistered,
    /// The recorded operation advanced: a replacement was selected, or the record reached Current.
    Advanced(RecoveryProgress),
    /// The selected family is Current, revalidated, and at the kernel tip.
    Current,
    /// The selected family is Current and this slice applied commits toward the tip; `ReachedTarget` means it got there.
    CaughtUp(EpisodeReport),
    /// The record is disabled, a disable is reconciling, or the owner is shut down; only explicit recovery reopens a disabled record.
    Disabled,
    /// A supervisor must be joined before the slice can go on: its tenure ended, its project left the roster, or the family it maintained is being replaced. The loop stops it and runs the next slice without idling.
    RotateMaintenance(MaintenanceHandle),
    /// The record could not be trusted.
    Unavailable(String),
    /// The recorded operation did not advance in this slice; the record stays as it was.
    Blocked(String),
}

/// The selection manager and what holds it.
#[derive(Default)]
enum Managed {
    #[default]
    None,
    Selection(Box<SearchSelection>),
    /// A disable took the manager to reconcile across an await; slices and pins see nothing until it returns.
    Disabling,
    /// The owner shut down; nothing takes or rebuilds the manager again. A manager whose supervisor did not drain within the grace stays here so its task, reader, and native work remain owned.
    ShutDown(Option<Box<SearchSelection>>),
}

/// One owner per data home.
pub struct SearchLifecycleOwner {
    home: PathBuf,
    kernel: Arc<KernelStore>,
    local_embeddings: LocalEmbeddingsComponent,
    admission: ProjectionAdmission,
    managed: Mutex<Managed>,
    roster: ProjectRoster,
    /// Woken when a disable hands the manager back, so a shutdown that found the manager disabling can retry.
    disabled: tokio::sync::Notify,
    /// Woken when a request is recorded, so the slice loop's idle wait ends and the record is observed with its time still ahead of it.
    requested: tokio::sync::Notify,
    /// The project digest whose supervisor ran last, how many slices it has had, and the bounds it started under; `None` before the first tenure.
    /// The tenant, its slice count, the bounds its supervisor runs under, and the `B_recovery_ms` those bounds were derived from.
    tenure: Mutex<Option<(String, u32, SliceBounds, u64)>>,
    #[cfg(feature = "test-support")]
    drain_grace_override: Mutex<Option<Duration>>,
    #[cfg(feature = "test-support")]
    slice_tap: Mutex<Option<SliceTap>>,
    /// The drain grace the last readable manifest approved, kept for a stop whose records are gone.
    /// The `physical_drain_ms` last read from records that named the identity they were read under, with that identity.
    last_grace: Mutex<Option<(ProjectionIdentity, Duration)>>,
    /// Makes the directory sync after the next authorized recovery's record rename fail once, on every selection this owner creates.
    #[cfg(feature = "test-support")]
    /// `(armed, _)`: while armed, the directory sync after the next record rename in a recovery write fails once, on whichever selection this owner created.
    recovery_sync_failure: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "test-support")]
type SliceTap = Arc<dyn Fn(&SliceEvent) + Send + Sync>;

/// A point a slice passes that a test may observe or hold.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy)]
pub enum SliceEvent {
    /// The slice is about to take the manager under this deadline, fixed before the wait.
    Waiting {
        deadline: Option<Instant>,
    },
    /// The records and identity are read and the manager synced; admission is not yet refreshed. Carries the slice budget's deadline.
    Prepared {
        deadline: Option<Instant>,
    },
    Episode(EpisodeEvent),
    /// A handed-back supervisor's drain begins under the grace given.
    Draining(Duration),
    /// The slice loop enters its idle wait, which a recorded request ends early.
    Idle,
    /// A request has taken its entry clock reading and is about to wait for the manager.
    RequestEntered,
}

/// Puts the manager a disable took back unless the owner shut down meanwhile, whether the disable finished or its future was dropped.
struct Restore<'a> {
    owner: &'a SearchLifecycleOwner,
    selection: Option<Box<SearchSelection>>,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let mut managed = self.owner.lock();
        if let (Managed::Disabling, Some(selection)) = (&*managed, self.selection.take()) {
            *managed = Managed::Selection(selection);
        }
        drop(managed);
        self.owner.disabled.notify_waiters();
    }
}

/// Why a shutdown did not release the manager.
#[derive(Debug, thiserror::Error)]
pub enum ShutdownUnresolved {
    #[error(transparent)]
    Drain(#[from] Unresolved),
    #[error("a disable was still reconciling after the grace period")]
    Disabling,
    #[error("a reader or request still held the manager after the grace period")]
    Held,
}

impl SearchLifecycleOwner {
    pub fn for_home(
        home: &Path,
        kernel: Arc<KernelStore>,
        local_embeddings: LocalEmbeddingsComponent,
    ) -> Self {
        Self {
            home: home.to_owned(),
            kernel,
            local_embeddings,
            admission: ProjectionAdmission::for_home(home),
            managed: Mutex::new(Managed::None),
            roster: Arc::new(Vec::new),
            tenure: Mutex::new(None),
            disabled: tokio::sync::Notify::new(),
            requested: tokio::sync::Notify::new(),
            #[cfg(feature = "test-support")]
            drain_grace_override: Mutex::new(None),
            #[cfg(feature = "test-support")]
            slice_tap: Mutex::new(None),
            last_grace: Mutex::new(None),
            #[cfg(feature = "test-support")]
            recovery_sync_failure: Arc::default(),
        }
    }

    /// Observes slice points and catch-up episode events on the slice thread, before the owner acts on them, so a test can hold a slice at a boundary.
    #[cfg(feature = "test-support")]
    pub fn tap_slice_events_for_test(&self, tap: impl Fn(&SliceEvent) + Send + Sync + 'static) {
        *self.slice_tap.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(tap));
    }

    /// The directory sync after the next authorized recovery's record rename fails once, leaving that record's durability unknown for a replay to reconcile.
    /// Arms one failure of the directory sync after the next record rename in a recovery write, leaving that record's durability unknown for a replay to reconcile.
    #[cfg(feature = "test-support")]
    pub fn fail_next_recovery_directory_sync_for_test(&self) {
        self.recovery_sync_failure
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// A new selection manager for `identity` under `bounds`, carrying this owner's test hooks.
    #[cfg(not(feature = "test-support"))]
    fn new_selection(
        &self,
        identity: ProjectionIdentity,
        bounds: CoverageBounds,
    ) -> SearchSelection {
        SearchSelection::new(&self.home, identity, bounds)
    }

    /// A new selection manager for `identity` under `bounds`, carrying this owner's test hooks: the barrier after a record rename moves an armed sync failure into the sync's own flag, once.
    #[cfg(feature = "test-support")]
    fn new_selection(
        &self,
        identity: ProjectionIdentity,
        bounds: CoverageBounds,
    ) -> SearchSelection {
        let armed = Arc::clone(&self.recovery_sync_failure);
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&fail);
        SearchSelection::new(&self.home, identity, bounds)
            .with_recovery_write_barrier_for_test(move |event| {
                if event == crate::projection_lifecycle::WriteBarrier::AfterRename
                    && armed.swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    fail.store(true, std::sync::atomic::Ordering::Release);
                }
            })
            .with_recovery_directory_sync_failure_for_test(flag)
    }

    #[cfg(feature = "test-support")]
    fn tap(&self, event: SliceEvent) {
        let tap = self
            .slice_tap
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(tap) = tap {
            tap(&event);
        }
    }

    /// Replaces the manifest's drain grace so a test can observe an unresolved drain without a manifest that no retirement could admit.
    #[cfg(feature = "test-support")]
    pub fn set_drain_grace_for_test(&self, grace: Duration) {
        *self
            .drain_grace_override
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(grace);
    }

    /// Supplies the bound projects maintenance rotates across; without one, no supervisor starts.
    pub fn with_roster(mut self, roster: ProjectRoster) -> Self {
        self.roster = roster;
        self
    }

    /// The lane the daemon's families are built under; a query embedded by it lives in the same vector space as the stored rows.
    pub fn embeddings(&self) -> &LocalEmbeddingsComponent {
        &self.local_embeddings
    }

    pub fn admission(&self) -> &ProjectionAdmission {
        &self.admission
    }

    fn lock(&self) -> MutexGuard<'_, Managed> {
        self.managed.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Takes the manager within `budget`; a slice holds it across its whole work, so a reader waits only as long as its own budget allows.
    fn lock_within(&self, budget: &EvalBudget) -> Result<MutexGuard<'_, Managed>, BuildError> {
        loop {
            match self.try_lock(budget)? {
                Some(guard) => return Ok(guard),
                None => std::thread::sleep(LOCK_POLL),
            }
        }
    }

    /// `lock_within` for an async caller: the wait yields to the runtime instead of sleeping its worker.
    async fn lock_within_async(
        &self,
        budget: &EvalBudget,
    ) -> Result<MutexGuard<'_, Managed>, BuildError> {
        loop {
            // The attempt's result is dropped before the wait, so no guard is alive across the await and the future stays `Send`.
            if let Some(guard) = self.try_lock(budget)? {
                return Ok(guard);
            }
            tokio::time::sleep(LOCK_POLL).await;
        }
    }

    /// `lock_within_async` that takes a free manager at once whatever `budget` says, so a grace of nothing still releases an uncontended manager; a held one is waited for within `budget`.
    async fn lock_free_or_within_async(
        &self,
        budget: &EvalBudget,
    ) -> Result<MutexGuard<'_, Managed>, BuildError> {
        // The free attempt's guard is returned or dropped before the wait, so none is alive across the await.
        if let Some(guard) = self.try_lock_free() {
            return Ok(guard);
        }
        self.lock_within_async(budget).await
    }

    /// The manager if no one holds it, whatever any budget says.
    fn try_lock_free(&self) -> Option<MutexGuard<'_, Managed>> {
        match self.managed.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// One attempt at the manager: the guard, `None` while another holder has it and `budget` still allows waiting, or `Expired`.
    fn try_lock(&self, budget: &EvalBudget) -> Result<Option<MutexGuard<'_, Managed>>, BuildError> {
        // An operation whose budget is already over gets no manager even when it is free, so it cannot act past its deadline.
        if budget.is_exhausted() {
            return Err(BuildError::Expired);
        }
        match self.managed.try_lock() {
            Ok(guard) => Ok(Some(guard)),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Ok(Some(poisoned.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) if budget.is_exhausted() => {
                Err(BuildError::Expired)
            }
            Err(std::sync::TryLockError::WouldBlock) => Ok(None),
        }
    }

    /// The identity the daemon runs with: the ready lane's model, tokenizer, dimension, and epoch; the manifest's limit protocol; and the kernel's incarnation. The manifest supplies only the protocol version, so its identity fields are still judged against this one.
    fn identity(
        &self,
        manifest: &RuntimeManifest,
        budget: &EvalBudget,
    ) -> Result<ProjectionIdentity, SpecRefusal> {
        let lane = match self.local_embeddings.status() {
            LocalEmbeddingsStatus::Ready(lane) => LaneIdentity::from(&lane),
            LocalEmbeddingsStatus::Starting => return Err(SpecRefusal::LaneNotReady("starting")),
            LocalEmbeddingsStatus::Disabled { .. } => {
                return Err(SpecRefusal::LaneNotReady("disabled"));
            }
            LocalEmbeddingsStatus::Failing { .. } => {
                return Err(SpecRefusal::LaneNotReady("failing"));
            }
        };
        let kernel_incarnation_id = self
            .kernel
            .database_incarnation_id_within_budget(budget)
            .map_err(|error| SpecRefusal::Kernel(error.to_string()))?;
        Ok(ProjectionIdentity {
            schema_version: retrieval::SCHEMA_VERSION,
            kernel_incarnation_id,
            projection_policy_version: PROJECTION_POLICY_VERSION.to_owned(),
            identity_contract_version: IDENTITY_CONTRACT_VERSION.to_owned(),
            limit_manifest_protocol_version: manifest.protocol_version.clone(),
            embedding_model: lane.embedding_model,
            tokenizer_fingerprint: lane.tokenizer_fingerprint,
            // The lexical analysis contract this build carries; the projection refuses any other.
            analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                .as_str()
                .to_owned(),
            vector_dimension: lane
                .vector_dimension
                .ok_or(SpecRefusal::LimitRange("vector_dimension"))?,
            generation_epoch: lane.generation_epoch,
        })
    }

    /// Reads the records, derives the identity and coverage bounds, and syncs the manager to both. A refusal leaves no manager for another identity or other bounds behind, except one whose supervisor still runs: that manager is kept and its supervisor handed back for joining, so replacing it never detaches running work.
    fn prepare(
        &self,
        managed: &mut Managed,
        budget: &EvalBudget,
        slice_started: Instant,
    ) -> Result<(AdmissionInputs, ProjectionIdentity, EvalBudget), Unprepared> {
        let prepared = AdmissionInputs::read(&self.home)
            .map_err(SpecRefusal::from)
            .and_then(|inputs| {
                let slice = Duration::from_millis(
                    nonzero_u64(
                        "supervisor_slice_ms",
                        limit(inputs.manifest(), "supervisor_slice_ms")?,
                    )?
                    .get(),
                );
                // The bound runs from the slice's start, so time spent waiting for the manager is not granted again here, and it only shortens the deadline fixed before the manager was taken.
                let budget = budget.bounded_by(slice_started + slice);
                let identity = self.identity(inputs.manifest(), &budget)?;
                let bounds = coverage_bounds(inputs.manifest())?;
                Ok((inputs, identity, bounds, budget))
            });
        let live = match &mut *managed {
            Managed::Selection(current) => current.maintenance(),
            _ => None,
        };
        let (inputs, identity, bounds, budget) = match prepared {
            Ok(prepared) => {
                self.remember_grace(&prepared.0, &prepared.1);
                prepared
            }
            Err(refusal) => {
                if let Some(handle) = live {
                    return Err(Unprepared::Rotate(handle));
                }
                *managed = Managed::None;
                return Err(Unprepared::Refused(refusal));
            }
        };
        if matches!(&*managed, Managed::Selection(current) if *current.identity() != identity || current.bounds() != bounds)
        {
            if let Some(handle) = live {
                return Err(Unprepared::Rotate(handle));
            }
            *managed = Managed::None;
        }
        if matches!(*managed, Managed::None) {
            *managed = Managed::Selection(Box::new(self.new_selection(identity.clone(), bounds)));
        }
        Ok((inputs, identity, budget))
    }

    /// Refreshes admission from the records and the daemon's current observation, then advances the lifecycle record one step. The slice ends within the manifest's `supervisor_slice_ms`, within `budget`, and, for an active record, within that record's own deadline.
    pub fn run_slice(&self, budget: &EvalBudget) -> SliceOutcome {
        // The manifest's slice bound runs from here, across the wait for the manager and the slice's work, and within the caller's budget; a cancelled slice returns rather than outliving its caller's cancellation. The bound is read before the manager is held, so a held manager cannot delay reading it.
        let slice_started = Instant::now();
        // A zero bound is not applied here: preparation refuses it, and that refusal needs the manager.
        let slice = AdmissionInputs::read(&self.home)
            .ok()
            .and_then(|inputs| limit(inputs.manifest(), "supervisor_slice_ms").ok())
            .filter(|slice_ms| *slice_ms > 0)
            .map_or(SLICE_IDLE, Duration::from_millis);
        let wait = budget.bounded_by(slice_started + slice);
        #[cfg(feature = "test-support")]
        self.tap(SliceEvent::Waiting {
            deadline: wait.deadline(),
        });
        let mut managed = match self.lock_within(&wait) {
            Ok(managed) => managed,
            Err(_) => return SliceOutcome::Blocked("the manager is held".to_owned()),
        };
        if matches!(*managed, Managed::Disabling | Managed::ShutDown(_)) {
            return SliceOutcome::Disabled;
        }
        let (inputs, identity, budget) = match self.prepare(&mut managed, &wait, slice_started) {
            Ok(prepared) => prepared,
            Err(Unprepared::Rotate(handle)) => {
                let _ = self.admission.refresh(None);
                return SliceOutcome::RotateMaintenance(handle);
            }
            Err(Unprepared::Refused(refusal)) => {
                let _ = self.admission.refresh(None);
                return SliceOutcome::Closed(refusal);
            }
        };
        #[cfg(feature = "test-support")]
        self.tap(SliceEvent::Prepared {
            deadline: budget.deadline(),
        });
        let Managed::Selection(selection) = &mut *managed else {
            return SliceOutcome::Disabled;
        };
        // `ProjectionLifecycle::open` syncs and locks the lifecycle directory; a probe that decides only whether to write must not.
        let control = ProjectionLifecycle::read_at(&self.home);
        let blocked = |closed: Closed| SliceOutcome::Blocked(closed.to_string());
        let (intent, completed) = match control {
            ControlState::Absent => {
                return match self.refresh(&inputs, Some(selection), &identity, &budget) {
                    Refresh::Installed(_) => SliceOutcome::Unregistered,
                    Refresh::Closed(closed) => blocked(closed),
                };
            }
            ControlState::Disabled(_) => {
                return match self.refresh(&inputs, Some(selection), &identity, &budget) {
                    Refresh::Installed(_) => SliceOutcome::Disabled,
                    Refresh::Closed(closed) => blocked(closed),
                };
            }
            ControlState::Unavailable(reason) => {
                let _ = self.admission.refresh(None);
                return SliceOutcome::Unavailable(reason);
            }
            ControlState::Intent(intent) => (intent, false),
            ControlState::Current(intent) => (intent, true),
        };
        // Work on an active record ends within the record's own deadline, which no slice renews, and starts only outside the margin before it; a completed record is revalidated under the slice's bound alone. A record whose deadline has passed is observed under nothing, but the records stay installed with no coverage so a cleanup keeps its evidence while every hook is denied.
        let deadline_at = (!completed).then(|| {
            let until = u64::try_from(intent.episodes.deadline.saturating_sub(crate::now_ms()))
                .unwrap_or(0);
            Instant::now() + Duration::from_millis(until)
        });
        let budget = match deadline_at {
            Some(at) if at <= Instant::now() => {
                let _ = self.admission.refresh_with(
                    inputs.clone(),
                    SelectedProjection {
                        identity: &identity,
                        coverage: None,
                    },
                );
                return SliceOutcome::Blocked("the record's deadline has passed".to_owned());
            }
            Some(at) => budget.bounded_by(at),
            None => budget,
        };
        let spec = match replacement_spec(
            inputs.manifest(),
            identity.clone(),
            intent.transition,
            intent.episodes.allowance,
            &intent.consumer.generation_id,
        ) {
            Ok(spec) => spec,
            Err(refusal) => {
                let _ = self.admission.refresh(None);
                return SliceOutcome::Closed(refusal);
            }
        };
        // An active record's own duration is charged against the transition's bound, as recovery charges it; a record a reloaded bound no longer fits is refused by every slice, so the records are installed with no coverage, as for an expired record: ordinary hooks are denied while a cleanup keeps its evidence.
        if !completed && let Err(denial) = duration_within_bound(inputs.manifest(), &intent) {
            let _ = self.admission.refresh_with(
                inputs.clone(),
                SelectedProjection {
                    identity: &identity,
                    coverage: None,
                },
            );
            return SliceOutcome::Blocked(denial.to_string());
        }
        // A supervisor whose bounds the manifest no longer yields is handed back before the gate renews on that manifest: renewal would cancel its grant and at once issue the next one, under which its loop would run a slice on the old bounds until the rotation reached it. The next slice, with no supervisor, installs the manifest and starts one under its bounds.
        if completed
            && selection.holds_operation(&intent)
            && let Some(live) = selection.maintenance()
            && self.tenure_outdated(inputs.manifest(), &spec)
        {
            return SliceOutcome::RotateMaintenance(live);
        }
        // Hooks are judged on this slice's observation before the record decides anything, so an expired or blocked record cannot leave the previous slice's evidence installed.
        if let Refresh::Closed(closed) = self.refresh(&inputs, Some(selection), &identity, &budget)
        {
            return blocked(closed);
        }
        if completed && selection.holds_operation(&intent) {
            // The family was reopened and revalidated when this owner first held it; a later slice judges it on its coverage and kernel without reopening it.
            return match selection.check_selected(&self.kernel, self.admission.gate(), &budget) {
                Ok(()) => {
                    self.settle_current(selection, &inputs, &spec, &intent, &identity, &budget)
                }
                Err(error) => SliceOutcome::Blocked(error.to_string()),
            };
        }
        // Measured again here: the refresh above may have waited, and the margin is from the deadline, not from the read.
        let budget = match deadline_at {
            Some(at) => match at.checked_sub(Duration::from_millis(DEADLINE_MARGIN_MS)) {
                Some(start_by) if start_by > Instant::now() => budget.bounded_by(start_by),
                _ => return SliceOutcome::Blocked("the record's deadline has passed".to_owned()),
            },
            None => budget,
        };
        if let Some(handle) = selection.maintenance() {
            return SliceOutcome::RotateMaintenance(handle);
        }
        let progress = selection.recover_slice(
            &self.kernel,
            self.admission.gate(),
            &spec,
            &budget,
            &mut |_| {},
        );
        match progress {
            Ok(progress) => {
                // The family the slice opened or selected is what later hooks are judged against.
                let _ = self.refresh(&inputs, Some(selection), &identity, &budget);
                if !(completed && progress == RecoveryProgress::Current) {
                    return SliceOutcome::Advanced(progress);
                }
                self.settle_current(selection, &inputs, &spec, &intent, &identity, &budget)
            }
            Err(RecoveryFailure::Build(mut failure)) => {
                // Cleanup runs under the same budget; a deferred cleanup keeps its owner's locks until the next slice retries.
                let cleanup = failure.cleanup(&budget).err();
                SliceOutcome::Blocked(with_followup(&failure.error, "cleanup", cleanup))
            }
            Err(RecoveryFailure::Selection(failure)) => SliceOutcome::Blocked(with_followup(
                &failure.error,
                "reconcile",
                failure.reconcile(selection, &budget).err(),
            )),
            Err(RecoveryFailure::Blocked(error)) => SliceOutcome::Blocked(error.to_string()),
        }
    }

    /// Reports a Current family: `Current` at the kernel tip, `CaughtUp` after one catch-up episode toward the tip with admission refreshed on the family the episode moved, or `RotateMaintenance` when the tenant's supervisor must be joined first. Maintenance is reconciled whether or not the episode advanced, so a family whose hold is dead still rotates; a maintenance start that fails after an episode ran leaves that episode's report in place.
    fn settle_current(
        &self,
        selection: &mut SearchSelection,
        inputs: &AdmissionInputs,
        spec: &ReplacementSpec,
        intent: &LifecycleIntent,
        identity: &ProjectionIdentity,
        budget: &EvalBudget,
    ) -> SliceOutcome {
        let report = match self.catch_up(
            selection,
            spec,
            &intent.consumer.consumer_id,
            identity,
            budget,
        ) {
            Ok(report) => report,
            Err(error) => return SliceOutcome::Blocked(error.to_string()),
        };
        if report.is_some() {
            let _ = self.refresh(inputs, Some(selection), identity, budget);
        }
        match self.maintain(selection, inputs.manifest(), spec, budget) {
            Ok(Some(handle)) => SliceOutcome::RotateMaintenance(handle),
            Ok(None) => report.map_or(SliceOutcome::Current, SliceOutcome::CaughtUp),
            // The episode's progress decides the loop's next step; maintenance is attempted again next slice.
            Err(error) => report.map_or_else(
                || SliceOutcome::Blocked(error.to_string()),
                SliceOutcome::CaughtUp,
            ),
        }
    }

    /// Applies the commits since the selected family's checkpoint under the hold that checkpoint names, one episode toward the tip, and acknowledges them. Returns `None` only when the selected checkpoint is at the kernel tip and the consumer has acknowledged that checkpoint; an unacknowledged prefix runs the episode, whose reconciliation acknowledges it. The hold binding is the running lease, so a family built in an earlier incarnation reports its dead hold as a blocked episode.
    fn catch_up(
        &self,
        selection: &SearchSelection,
        spec: &ReplacementSpec,
        consumer_id: &str,
        identity: &ProjectionIdentity,
        budget: &EvalBudget,
    ) -> Result<Option<EpisodeReport>, BuildError> {
        let checkpoint = selection.observe_selected(budget)?.checkpoint;
        let local = checkpoint.checkpoint_commit_seq;
        if local >= self.kernel.tip_within_budget(budget)?
            && self
                .kernel
                .outbox_consumer_checkpoint_within_budget(budget, consumer_id)?
                .is_some_and(|acknowledged| acknowledged >= local)
        {
            return Ok(None);
        }
        let reader = selection.pin(&self.kernel, self.admission.gate(), budget)?;
        let grants = self.admission.gate().admit_all(
            &[
                ProjectionHook::EmbeddingBootstrap,
                ProjectionHook::GitDurableRows,
            ],
            EntryPoint::Dispatch,
        )?;
        let expected = InvalidationIdentity::from(identity);
        for grant in &grants {
            self.admission
                .gate()
                .check_limits(grant, &expected, &spec.catchup_page_charges())?;
        }
        let consumer = CatchUpConsumer {
            binding: SourceHoldBinding {
                consumer_id: reader.consumer().consumer_id.clone(),
                lease_epoch: self.kernel.lease_epoch(),
                source_policy_version: identity.projection_policy_version.clone(),
            },
            hold_id: checkpoint.hold_id,
            kernel_incarnation_id: identity.kernel_incarnation_id.clone(),
            generation_id: Some(reader.consumer().generation_id.clone()),
        };
        // The caller's cancellation reaches the linked budget directly; the callback cancels only `episode`, leaving the caller's `budget` unmodified.
        let episode = budget.linked();
        let report = SearchCatchUp::new(&self.kernel, reader.projection())
            .with_budget(episode.clone())
            .run_episode(&consumer, &spec.episode, crate::now_ms(), &mut |event| {
                #[cfg(feature = "test-support")]
                self.tap(SliceEvent::Episode(event));
                #[cfg(not(feature = "test-support"))]
                let _ = event;
                if grants.iter().any(|grant| grant.invalidated.is_cancelled()) {
                    episode.cancel();
                }
            })?;
        Ok(Some(report))
    }

    /// Keeps one supervisor maintaining the selected family for the current tenant. A running tenant within its tenure and still on the roster is left alone; one past its tenure or off the roster, whose episode grant has expired, or whose bounds the manifest no longer yields is handed back for joining; with none running, the next roster project in sorted order after the last tenant starts through [`SearchSelection::start_maintenance`], which admits and charges it. No roster means no maintenance.
    fn maintain(
        &self,
        selection: &mut SearchSelection,
        manifest: &RuntimeManifest,
        spec: &ReplacementSpec,
        budget: &EvalBudget,
    ) -> Result<Option<MaintenanceHandle>, BuildError> {
        let roster: BTreeMap<String, ProjectScope> = (self.roster)().into_iter().collect();
        let mut tenure = self.tenure.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(live) = selection.maintenance() {
            let Some((tenant, slices, started, _)) = tenure.as_mut() else {
                return Ok(Some(live));
            };
            *slices += 1;
            // A lone project keeps its supervisor; rotating it would only pay a restart. An expired episode grant restarts its supervisor regardless of roster membership; bounds the manifest no longer yields hand it back before the gate renews, in `run_slice`.
            let over = *slices >= MAINTENANCE_TENURE_SLICES && roster.len() > 1;
            let expired = crate::now_ms() >= started.dispatch.grant.deadline;
            let bound = roster.get(tenant.as_str()) == Some(&*live.scope);
            return Ok((over || expired || !bound).then_some(live));
        }
        let last = tenure.as_ref().map(|(tenant, _, _, _)| tenant.as_str());
        let Some((next, scope)) = last
            .and_then(|last| {
                roster
                    .range::<str, _>((std::ops::Bound::Excluded(last), std::ops::Bound::Unbounded))
                    .next()
            })
            .or_else(|| roster.iter().next())
        else {
            return Ok(None);
        };
        let reader = selection.pin(&self.kernel, self.admission.gate(), budget)?;
        let bounds = maintenance_bounds(manifest, spec).map_err(|refusal| {
            BuildError::Invalid(match refusal {
                SpecRefusal::TooSmall(_) => "manifest limits cannot bound maintenance",
                _ => "maintenance bounds",
            })
        })?;
        selection.start_maintenance(
            Maintained {
                gate: Arc::clone(self.admission.gate()),
                kernel: Arc::clone(&self.kernel),
                projection: Arc::clone(reader.projection()),
                local_embeddings: Arc::new(self.local_embeddings.clone()),
                project: scope.clone(),
                destination: MAINTENANCE_DESTINATION,
            },
            bounds,
            Arc::new(crate::now_ms),
            tokio::sync::mpsc::unbounded_channel().0,
            budget,
        )?;
        let recovery_ms = limit(manifest, "B_recovery_ms")
            .map_err(|_| BuildError::Invalid("manifest limits cannot bound maintenance"))?;
        *tenure = Some((next.clone(), 0, bounds, recovery_ms));
        Ok(None)
    }

    /// Whether the manifest no longer yields the bounds the live supervisor started under. The grant's deadline is an instant derived from `B_recovery_ms` at the start, so that limit is compared on its own.
    fn tenure_outdated(&self, manifest: &RuntimeManifest, spec: &ReplacementSpec) -> bool {
        let tenure = self.tenure.lock().unwrap_or_else(|p| p.into_inner());
        tenure.as_ref().is_some_and(|(_, _, started, recovery_ms)| {
            !maintenance_bounds(manifest, spec).is_ok_and(|fresh| {
                without_grant_deadline(fresh) == without_grant_deadline(*started)
            }) || limit(manifest, "B_recovery_ms").ok() != Some(*recovery_ms)
        })
    }

    /// Keeps the records' `physical_drain_ms` for a stop whose records are gone or disagree by then. Records that do not both name `identity`, as during a partial reload, are not an approved grace and leave the last one in place.
    fn remember_grace(&self, inputs: &AdmissionInputs, identity: &ProjectionIdentity) {
        if !inputs.applies_to(identity) {
            return;
        }
        if let Ok(grace) = limit(inputs.manifest(), "physical_drain_ms") {
            *self.last_grace.lock().unwrap_or_else(|p| p.into_inner()) =
                Some((identity.clone(), Duration::from_millis(grace)));
        }
    }

    /// The drain grace maintenance stops are given: the records' `physical_drain_ms` when they still name the identity the last approved grace was read under, otherwise that last approved grace, or one slice when none was ever read.
    fn drain_grace(&self) -> Duration {
        #[cfg(feature = "test-support")]
        if let Some(grace) = *self
            .drain_grace_override
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            return grace;
        }
        let remembered = self
            .last_grace
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let (Ok(inputs), Some((identity, _))) = (AdmissionInputs::read(&self.home), &remembered)
        {
            self.remember_grace(&inputs, identity);
        }
        self.last_grace
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map_or(SLICE_IDLE, |(_, grace)| *grace)
    }

    /// Joins the supervisor a slice handed back. A resolved drain frees the slot for the next tenant; an unresolved one leaves the supervisor owned by its task and reported.
    pub async fn stop_maintenance(
        &self,
        handle: &MaintenanceHandle,
    ) -> Result<DrainReport, Unresolved> {
        let grace = self.drain_grace();
        #[cfg(feature = "test-support")]
        self.tap(SliceEvent::Draining(grace));
        handle.stop(grace).await
    }

    /// The supervisor still running on the selected family, if any.
    pub fn maintenance(&self) -> Option<MaintenanceHandle> {
        match &mut *self.lock() {
            Managed::Selection(selection) | Managed::ShutDown(Some(selection)) => {
                selection.maintenance()
            }
            _ => None,
        }
    }

    /// Refreshes admission from the records the slice read and the selected family's coverage, with no coverage when that family cannot be read, or from the unregistered observation at `tip` when no family is selected. Coverage is read outside gate admission so a denial does not block the next observation.
    fn refresh(
        &self,
        inputs: &AdmissionInputs,
        selection: Option<&SearchSelection>,
        identity: &ProjectionIdentity,
        budget: &EvalBudget,
    ) -> Refresh {
        // The tip is read at the observation, so a slice that just committed to the kernel is judged against the tip it moved.
        let tip = match self.kernel.tip_within_budget(budget) {
            Ok(tip) => tip,
            Err(_) => {
                self.admission.gate().close();
                return Refresh::Closed(Closed::NoProjection);
            }
        };
        // A selection kept under another identity, as after a reload the slice loop has not yet rotated on, is foreign evidence: the new identity has no registered family, so it is judged as unregistered rather than denied on the old family's report. No manager at all, before the first slice, is judged the same way.
        let coverage = match selection {
            Some(selection) if selection.has_selected() && *selection.identity() == *identity => {
                selection
                    .observe_selected(budget)
                    .ok()
                    .map(|report| ProjectionCoverage::unjudged(report, tip))
            }
            _ => Some(ProjectionCoverage::unregistered(identity, tip)),
        };
        self.admission.refresh_with(
            inputs.clone(),
            SelectedProjection {
                identity,
                coverage: coverage.as_ref(),
            },
        )
    }

    /// Records a rebuild request, or begins an authorized recovery, without running a slice. A request naming another kernel incarnation, one made while the admission records or the lane are unavailable, or one the installed manifest's limits cannot bound, is refused before anything is recorded.
    ///
    /// # Errors
    ///
    /// Returns the lifecycle's refusal or the selection's build error.
    pub fn request(
        &self,
        request: &LifecycleRequest,
        now: i64,
        budget: &EvalBudget,
    ) -> Result<Option<Recorded>, BuildError> {
        // A slice holds the manager across its whole work, so the wait for it is the request's own budget, like everything after it.
        let entered = Instant::now();
        #[cfg(feature = "test-support")]
        self.tap(SliceEvent::RequestEntered);
        let mut managed = self.lock_within(budget)?;
        // The records and lane are read now rather than trusted from the evidence an earlier slice installed; unavailable ones close the gate and refuse the request.
        let inputs = match AdmissionInputs::read(&self.home) {
            Ok(inputs) => inputs,
            Err(_) => {
                let _ = self.admission.refresh(None);
                return Err(IntentRefusal::Denied(Denial::NoManifest).into());
            }
        };
        let identity = match self.identity(inputs.manifest(), budget) {
            Ok(identity) => identity,
            Err(_) => {
                let _ = self.admission.refresh(None);
                return Err(IntentRefusal::Denied(Denial::EvidenceIdentity).into());
            }
        };
        // What a slice prepares under is checked first: a manifest with no slice bound or no coverage bounds would leave the recorded request to slices that all refuse it. That refusal is the manifest's, and closes the gate as a slice's would, since the earlier grants belong to a manifest that is gone.
        let slice_bound = limit(inputs.manifest(), "supervisor_slice_ms")
            .and_then(|slice_ms| nonzero_u64("supervisor_slice_ms", slice_ms));
        if slice_bound.is_err() || coverage_bounds(inputs.manifest()).is_err() {
            let _ = self.admission.refresh(None);
            return Err(BuildError::Invalid(
                "manifest limits cannot bound the request",
            ));
        }
        // A supervisor still running under the identity or bounds a reload replaced is not re-admitted under the new records: admission closes, the slice loop is woken to rotate it, and the request is refused until then.
        if let Managed::Selection(selection) = &mut *managed
            && selection.maintenance().is_some()
            && (*selection.identity() != identity
                || coverage_bounds(inputs.manifest()).ok() != Some(selection.bounds()))
        {
            let _ = self.admission.refresh(None);
            self.requested.notify_one();
            return Err(BuildError::Invalid(
                "maintenance runs under records a reload replaced; retry after the next slice rotates it",
            ));
        }
        // The records just read are installed before the request's own sizing is judged, so a refused request still leaves the gate on the current records rather than on the evidence the last slice installed or, before the first slice, on its initial closed state; a reload that changed them cancels the earlier grants here.
        let selection = match &*managed {
            Managed::Selection(selection) => Some(&**selection),
            _ => None,
        };
        if let Refresh::Closed(closed) = self.refresh(&inputs, selection, &identity, budget) {
            return Err(BuildError::Invalid(match closed {
                Closed::ShutDown => "the owner is shut down",
                _ => "admission is closed",
            }));
        }
        // Whether the manifest still bounds the operation already recorded, as its next slice would check. A replacement that fits the reduced limits may replace what no longer does; a request that does not fit either, or one refused for another reason, leaves the gate as that slice would: closed over an operation the manifest cannot bound, and on the records with no coverage over an active record past its duration bound, so a cleanup keeps its envelope.
        let recorded = ProjectionLifecycle::read_at(&self.home);
        let spec_fits = |current: &LifecycleIntent| {
            replacement_spec(
                inputs.manifest(),
                identity.clone(),
                current.transition,
                current.episodes.allowance,
                &current.consumer.generation_id,
            )
            .is_ok()
        };
        let current_fit = match &recorded {
            ControlState::Intent(current) if !spec_fits(current) => RecordFit::Unbounded,
            // An active record's duration is charged as its slices charge it; a completed record's construction is history.
            ControlState::Intent(current)
                if duration_within_bound(inputs.manifest(), current).is_err() =>
            {
                RecordFit::OverDuration
            }
            ControlState::Current(current) if !spec_fits(current) => RecordFit::Unbounded,
            // An unreadable record is what the next slice closes admission on.
            ControlState::Unavailable(_) => RecordFit::Unbounded,
            _ => RecordFit::Fits,
        };
        let leave_gate_as_a_slice_would = || match current_fit {
            RecordFit::Fits => {}
            RecordFit::OverDuration => {
                let _ = self.admission.refresh_with(
                    inputs.clone(),
                    SelectedProjection {
                        identity: &identity,
                        coverage: None,
                    },
                );
            }
            RecordFit::Unbounded => {
                let _ = self.admission.refresh(None);
            }
        };
        // Judged after the records are installed, so a request for another kernel still leaves the gate on the current records rather than on the evidence a reload replaced; over a recorded operation those records cannot bound, the gate the last slice closed stays closed.
        if self.kernel.database_incarnation_id_within_budget(budget)?
            != request.kernel_incarnation_id
        {
            leave_gate_as_a_slice_would();
            return Err(BuildError::Invalid(
                "the request names another kernel incarnation",
            ));
        }
        // A replay of the recorded intent reconciles that record's durability rather than recording anything new, so its time left is not judged here.
        let replay =
            matches!(&recorded, ControlState::Intent(existing) if existing.is_replay_of(request));
        // The replacement bounds depend on this request's allowance and generation, so their refusal is the request's alone.
        if replacement_spec(
            inputs.manifest(),
            identity.clone(),
            request.transition,
            request.allowance,
            &request.consumer.generation_id,
        )
        .is_err()
        {
            leave_gate_as_a_slice_would();
            return Err(BuildError::Invalid(
                "manifest limits cannot bound the request",
            ));
        }
        // The time left is judged, and the record stamped, at the clock as it stands after the waits above, not the caller's reading before them: a wait for a held manager can spend most of a short deadline.
        let now =
            now.saturating_add(i64::try_from(entered.elapsed().as_millis()).unwrap_or(i64::MAX));
        // A request with no time past the start margin would be recorded only for every slice to refuse it inside the margin until it expires.
        let duration = u64::try_from(request.deadline.saturating_sub(now)).unwrap_or(0);
        let bound = limit(inputs.manifest(), request.transition.duration_limit())
            .map_err(|_| BuildError::Invalid("manifest limits cannot bound the request"))?;
        if duration <= DEADLINE_MARGIN_MS && !replay {
            leave_gate_as_a_slice_would();
            return Err(BuildError::Invalid(
                "the request's deadline leaves no time past the start margin",
            ));
        }
        if duration > bound {
            leave_gate_as_a_slice_would();
            return Err(BuildError::Invalid(
                "the request's deadline lies past its transition's bound",
            ));
        }
        let recorded = match request.transition {
            Transition::Rebuilding => {
                let lifecycle = ProjectionLifecycle::open(&self.home)?;
                Some(lifecycle.record_at(
                    self.admission.gate(),
                    request,
                    now,
                    EntryPoint::Explicit,
                )?)
            }
            Transition::AuthorizedRecovery => {
                let Managed::Selection(selection) = &mut *managed else {
                    return Err(BuildError::Invalid(
                        "no slice has run under the current identity",
                    ));
                };
                selection.begin_authorized_recovery(
                    &self.kernel,
                    self.admission.gate(),
                    request,
                    budget,
                )?;
                None
            }
        };
        // The slice loop's idle wait ends now, so the record is not first seen after an idle period has spent its time.
        self.requested.notify_one();
        Ok(recorded)
    }

    /// Pins the selected family for one reader. Freshness is judged on the coverage the last slice observed, so a reader can trail the kernel by the commits that arrived since that slice on top of `catchup_lag_commits`; the next slice observes the family again.
    ///
    /// # Errors
    ///
    /// Returns the selection's error, including unavailability when no family is selected or no slice has run.
    pub fn pin(&self, budget: &EvalBudget) -> Result<SearchReader, BuildError> {
        let managed = self.lock_within(budget)?;
        let Managed::Selection(selection) = &*managed else {
            return Err(BuildError::Invalid("search unavailable; no slice has run"));
        };
        selection.pin(&self.kernel, self.admission.gate(), budget)
    }

    /// Closes admission, persists the disabled intent, then reconciles the disabled family within the manifest's cleanup envelope; a home with no lifecycle record has no family, so its persisted stop completes the disable. Admission is closed before any filesystem write. A dropped future leaves the persisted intent for a later disable to reconcile.
    ///
    /// # Errors
    ///
    /// Returns the selection's error; the persisted intent stays for a later disable or restart to reconcile.
    pub async fn disable(
        &self,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(DisableEvent),
    ) -> Result<(), BuildError> {
        let mut restore = Restore {
            owner: self,
            selection: {
                // A slice or reader holding the manager is waited for only within the disable's budget.
                let mut managed = self.lock_within_async(budget).await?;
                match std::mem::take(&mut *managed) {
                    Managed::Selection(selection) => {
                        *managed = Managed::Disabling;
                        Some(selection)
                    }
                    other => {
                        *managed = other;
                        return Err(BuildError::Invalid("search unavailable; no slice has run"));
                    }
                }
            },
        };
        let selection = restore.selection.as_mut().expect("taken above");
        selection.begin_disable(self.admission.gate(), observer)?;
        let ControlState::Disabled(disabled) = ProjectionLifecycle::open(&self.home)?.read() else {
            return Err(BuildError::Invalid("disable did not persist"));
        };
        // A stop with nothing to reconcile is complete; only cleanup needs the manifest's bounds, so the records are read after this.
        let Some(intent) = disabled.handoff.as_deref() else {
            return Ok(());
        };
        let inputs = AdmissionInputs::read(&self.home)
            .map_err(|_| BuildError::Invalid("admission records refused"))?;
        let spec = replacement_spec(
            inputs.manifest(),
            selection.identity().clone(),
            intent.transition,
            intent.episodes.allowance,
            &intent.consumer.generation_id,
        )
        .map_err(|_| BuildError::Invalid("manifest limits cannot bound the cleanup"))?;
        selection
            .reconcile_disabled(&self.kernel, self.admission.gate(), &spec, budget, observer)
            .await
            .map(|_| ())
    }

    /// Closes admission for good, joins the running supervisor within the drain grace, and releases the selection manager; the durable record and every kernel obligation stay for the next start. A disable still reconciling is waited for within that same grace, which the drain that follows shares, and otherwise reported, with the manager left to it. A supervisor that does not drain in time keeps its manager, so its task and native work stay owned, and the unresolved drain is returned. Idempotent, and a disable that returns afterwards restores nothing.
    pub async fn shutdown(&self) -> Result<(), ShutdownUnresolved> {
        self.admission.close();
        // One grace covers both the wait for a disable and the drain of the supervisor it hands back.
        let deadline = Instant::now() + self.drain_grace();
        let taken = loop {
            // Registered before the manager is inspected, so a disable that finishes in between still wakes the wait.
            let handed_back = self.disabled.notified();
            // A free manager is taken at once, whatever the grace; a held one is waited for only within the grace, and the wait yields the worker.
            let within_grace = EvalBudget::new(
                Some(deadline),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            );
            let taken = {
                let Ok(mut managed) = self.lock_free_or_within_async(&within_grace).await else {
                    return Err(ShutdownUnresolved::Held);
                };
                match std::mem::take(&mut *managed) {
                    Managed::Disabling => {
                        *managed = Managed::Disabling;
                        None
                    }
                    Managed::Selection(selection) | Managed::ShutDown(Some(selection)) => {
                        *managed = Managed::ShutDown(None);
                        Some(Some(selection))
                    }
                    _ => {
                        *managed = Managed::ShutDown(None);
                        Some(None)
                    }
                }
            };
            match taken {
                Some(taken) => break taken,
                None => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero()
                        || tokio::time::timeout(remaining, handed_back).await.is_err()
                    {
                        return Err(ShutdownUnresolved::Disabling);
                    }
                }
            }
        };
        let Some(mut selection) = taken else {
            return Ok(());
        };
        let Some(handle) = selection.maintenance() else {
            return Ok(());
        };
        let outcome = handle
            .stop(deadline.saturating_duration_since(Instant::now()))
            .await;
        if outcome.is_err() {
            *self.lock() = Managed::ShutDown(Some(selection));
        }
        outcome.map(|_| ()).map_err(ShutdownUnresolved::from)
    }
}

fn with_followup(error: &BuildError, step: &str, followup: Option<BuildError>) -> String {
    match followup {
        Some(followup) => format!("{error}; {step}: {followup}"),
        None => error.to_string(),
    }
}

/// Charges an active record's own duration, its deadline less the clock it was recorded at, against its transition's bound in `manifest`, as recovery charges it at every admission.
/// How the manifest bounds the operation already recorded, as judged at a request.
#[derive(Clone, Copy)]
enum RecordFit {
    Fits,
    /// An active record's duration exceeds its transition's bound; the records still bound a cleanup.
    OverDuration,
    /// The manifest cannot bound the recorded operation, or the record is unreadable.
    Unbounded,
}

fn duration_within_bound(
    manifest: &RuntimeManifest,
    intent: &LifecycleIntent,
) -> Result<(), Denial> {
    let limit_name = intent.transition.duration_limit();
    let observed = u64::try_from(intent.episodes.deadline.saturating_sub(intent.recorded_at))
        .unwrap_or(u64::MAX);
    let max = limit(manifest, limit_name)
        .map_err(|_| Denial::Failed(Gate::Resource, format!("limit {limit_name} is absent")))?;
    if observed <= max {
        Ok(())
    } else {
        Err(Denial::LimitExceeded {
            limit: limit_name.to_owned(),
            observed,
            max,
        })
    }
}

fn limit(manifest: &RuntimeManifest, name: &'static str) -> Result<u64, SpecRefusal> {
    manifest
        .limits
        .get(name)
        .copied()
        .ok_or(SpecRefusal::TooSmall(name))
}

fn nonzero_usize(name: &'static str, value: u64) -> Result<NonZeroUsize, SpecRefusal> {
    usize::try_from(value)
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or(SpecRefusal::TooSmall(name))
}

fn nonzero_u64(name: &'static str, value: u64) -> Result<NonZeroU64, SpecRefusal> {
    NonZeroU64::new(value).ok_or(SpecRefusal::TooSmall(name))
}

/// The bounds with the grant deadline cleared, so two derivations of the same manifest compare equal.
fn without_grant_deadline(mut bounds: SliceBounds) -> SliceBounds {
    bounds.dispatch.grant.deadline = 0;
    bounds
}

/// Supervisor slice bounds within the limits `start_maintenance` charges: `embedding_recovery_attempts`, `pending_count`, `supervisor_slice_ms`, and `local_transaction_rows`. The episode deadline is the recovery bound from now, and a result is awaited for one lease before the pass moves on.
fn maintenance_bounds(
    manifest: &RuntimeManifest,
    spec: &ReplacementSpec,
) -> Result<SliceBounds, SpecRefusal> {
    let slice_ms = limit(manifest, "supervisor_slice_ms")?;
    let lease_ms = limit(manifest, "lease_duration_ms")?;
    let retry_after =
        i64::try_from(lease_ms).map_err(|_| SpecRefusal::LimitRange("lease_duration_ms"))?;
    // A recovery bound of nothing would start every supervisor on an expired grant.
    let recovery_ms =
        i64::try_from(nonzero_u64("B_recovery_ms", limit(manifest, "B_recovery_ms")?)?.get())
            .map_err(|_| SpecRefusal::LimitRange("B_recovery_ms"))?;
    Ok(SliceBounds {
        dispatch: DispatchBounds {
            max_jobs: spec.episode.batch.max_pending,
            grant: EpisodeGrant {
                allowance: u32::try_from(limit(manifest, "embedding_recovery_attempts")?)
                    .ok()
                    .and_then(NonZeroU32::new)
                    .ok_or(SpecRefusal::TooSmall("embedding_recovery_attempts"))?,
                deadline: crate::now_ms().saturating_add(recovery_ms),
            },
            retry_after,
            result_wait: Duration::from_millis(lease_ms.min(slice_ms / 2)),
            input: InputEnvelope {
                bytes: nonzero_u64(
                    "embedding_input_bytes",
                    limit(manifest, "embedding_input_bytes")?,
                )?
                .get(),
                tokens: nonzero_u64(
                    "embedding_input_tokens",
                    limit(manifest, "embedding_input_tokens")?,
                )?
                .get(),
            },
        },
        sweep_candidates: nonzero_usize(
            "local_transaction_rows",
            limit(manifest, "local_transaction_rows")? / 10,
        )?,
        slice: Duration::from_millis(slice_ms),
        idle: SLICE_IDLE,
    })
}

/// Each `OccurrenceClass` receives one live and one tombstone share of `local_transaction_rows`, so `CoverageBounds::max_rows` fits the limit; a live share also stays within `export_page_rows`.
fn coverage_bounds(manifest: &RuntimeManifest) -> Result<CoverageBounds, SpecRefusal> {
    let shares = 2 * OccurrenceClass::ALL.len() as u64;
    let per_class = nonzero_usize(
        "local_transaction_rows",
        limit(manifest, "local_transaction_rows")? / shares,
    )?;
    Ok(CoverageBounds {
        max_live_per_class: nonzero_usize(
            "export_page_rows",
            (per_class.get() as u64).min(limit(manifest, "export_page_rows")?),
        )?,
        max_tombstoned_per_class: per_class,
    })
}

/// The replacement specification the manifest's named limits bound. `ReplacementSpec::admit`, the retirement admission, and the cleanup admission charge aggregates of these bounds against the same limits, so each bound is sized so that its aggregate fits: a page and a batch each take half of a shared byte limit, rows split between the batch and the page, one record's bytes are reserved on top of a window and a retirement, the seed and the capture share the disk limit across the episode allowance, and checkpoint attempts divide the retry limit by that allowance. A limit too small to fit any positive bound refuses the specification instead of recording work that every slice would then deny.
fn replacement_spec(
    manifest: &RuntimeManifest,
    identity: ProjectionIdentity,
    transition: Transition,
    allowance: u32,
    generation_id: &str,
) -> Result<ReplacementSpec, SpecRefusal> {
    let allowance = u64::from(allowance.max(1));
    // A slice of no duration could run nothing recorded under these bounds.
    nonzero_u64(
        "supervisor_slice_ms",
        limit(manifest, "supervisor_slice_ms")?,
    )?;
    let local_bytes = limit(manifest, "local_transaction_bytes")?;
    let quarter_local = local_bytes / 4;
    let half_rows = nonzero_usize(
        "local_transaction_rows",
        limit(manifest, "local_transaction_rows")? / 2,
    )?;
    let page_rows = nonzero_usize(
        "export_page_rows",
        limit(manifest, "export_page_rows")?.min(half_rows.get() as u64),
    )?;
    let row_bytes = nonzero_u64(
        "export_row_standalone_encoded_bytes",
        limit(manifest, "export_row_standalone_encoded_bytes")?,
    )?;
    let page_encoded = nonzero_u64(
        "export_page_encoded_bytes",
        limit(manifest, "export_page_encoded_bytes")?.min(row_bytes.get()),
    )?;
    let half_decoded = limit(manifest, "export_live_decoded_bytes")? / 2;
    let source_bytes = nonzero_usize(
        "catchup_batch_source_bytes",
        limit(manifest, "catchup_batch_source_bytes")?
            .min(half_decoded)
            .min(quarter_local),
    )?;
    // A backfill pass obsoletes every terminal candidate it selected in one transaction, one row each, so the pending bound also fits the rows one transaction may mutate.
    let pending_width = ReplacementSpec::pending_row_width(generation_id);
    let max_pending = nonzero_usize(
        "pending_count",
        limit(manifest, "pending_count")?
            .min(limit(manifest, "pending_bytes")? / pending_width)
            .min(quarter_local / pending_width)
            .min(half_rows.get() as u64),
    )?;
    let tuple_bytes = nonzero_usize(
        "export_row_standalone_encoded_bytes",
        row_bytes.get().min(MAX_TUPLE_BYTES),
    )?;
    let batch_rows = nonzero_usize(
        "local_transaction_bytes",
        (half_rows.get() as u64).min(quarter_local / tuple_bytes.get() as u64),
    )?;
    // The commit page's payload and the window's exported source are both charged against `catchup_batch_encoded_bytes`, so each takes half.
    let two_per_row = nonzero_usize(
        "local_transaction_rows",
        (batch_rows.get() as u64).saturating_mul(2),
    )?;
    let commit_bytes = nonzero_u64(
        "catchup_batch_encoded_bytes",
        limit(manifest, "catchup_batch_encoded_bytes")? / 2,
    )?;
    // A window's encoded source and a retirement's censused bytes are each charged with one record's bytes on top.
    let with_record = (local_bytes / 2)
        .checked_sub(MAX_RECORD_BYTES)
        .ok_or(SpecRefusal::LimitRange("local_transaction_bytes"))?;
    let window_encoded = nonzero_u64(
        "local_transaction_bytes",
        commit_bytes.get().min(with_record),
    )?;
    let obligation_bytes = nonzero_u64("local_transaction_bytes", with_record.min(quarter_local))?;
    let disk = limit(manifest, "capture_disk_bytes")?;
    let source_disk = nonzero_u64("capture_disk_bytes", disk / 4)?;
    let seed_bytes = (disk / 2 / (allowance + 2))
        .checked_sub(MAX_RECORD_BYTES)
        .filter(|bytes| *bytes > 0)
        .ok_or(SpecRefusal::LimitRange("capture_disk_bytes"))?;
    // Checkpoint attempts spend the retry allowance and their waits spend half the episode bound, both across the episode allowance: one attempt's wait is the lease duration capped so one attempt per episode fits, and the attempts are as many as both limits afford. A limit that cannot afford one attempt per episode refuses the specification.
    let duration_limit = transition.duration_limit();
    let wait_bound = limit(manifest, duration_limit)? / 2 / allowance;
    let attempt_wait_ms = limit(manifest, "lease_duration_ms")?.min(wait_bound);
    if attempt_wait_ms == 0 {
        return Err(SpecRefusal::LimitRange(duration_limit));
    }
    let checkpoint_attempts = u32::try_from(
        (limit(manifest, "retry_attempts")? / allowance).min(wait_bound / attempt_wait_ms),
    )
    .ok()
    .and_then(NonZeroU32::new)
    .ok_or(SpecRefusal::LimitRange("retry_attempts"))?;
    // The kernel refuses a hold lifetime past its own maximum at every capture.
    let hold_expiry = nonzero_u64(
        "capture_hold_expiry_ms",
        limit(manifest, "capture_hold_expiry_ms")?,
    )?;
    if hold_expiry.get() > kernel::MAX_SOURCE_HOLD_LIFETIME_MS {
        return Err(SpecRefusal::LimitRange("capture_hold_expiry_ms"));
    }
    let admission = SourceHoldAdmission {
        max_references: nonzero_usize(
            "capture_reference_count",
            limit(manifest, "capture_reference_count")?,
        )?,
        max_encoded_bytes: source_disk,
    };
    let generation = VectorGeneration {
        generation_id: generation_id.to_owned(),
        embedding_model: identity.embedding_model.clone(),
        tokenizer_fingerprint: identity.tokenizer_fingerprint.clone(),
        vector_dimension: identity.vector_dimension,
        generation_epoch: identity.generation_epoch,
    };
    Ok(ReplacementSpec {
        capture: SourceHoldBounds {
            admission,
            max_descriptor_rows: half_rows,
            expiry_ms: hold_expiry,
        },
        episode: EpisodeBounds {
            commits: CommitPageBounds {
                max_commits: nonzero_usize(
                    "catchup_batch_commits",
                    limit(manifest, "catchup_batch_commits")?,
                )?,
                // A window never holds more rows than one batch admits, so it can always be applied.
                max_rows: batch_rows,
                max_payload_bytes: commit_bytes,
            },
            hold_admission: admission,
            source_page: SourcePageBounds {
                max_rows: page_rows,
                max_encoded_bytes: page_encoded,
                max_decoded_bytes: nonzero_u64("export_live_decoded_bytes", half_decoded)?,
                max_row_bytes: nonzero_u64(
                    "export_row_standalone_encoded_bytes",
                    row_bytes.get().min(page_encoded.get()),
                )?,
            },
            // Every page that continues a window admits at least one row, so a window within its row allowance spans at most `batch_rows` pages whether its pages end on rows or on bytes.
            max_source_pages: batch_rows,
            max_source_encoded_bytes: window_encoded,
            batch: BatchBounds {
                persist: PersistBounds {
                    max_records: batch_rows,
                    max_payload_bytes: nonzero_usize("local_transaction_bytes", local_bytes)?,
                    max_tuple_bytes: tuple_bytes,
                },
                max_source_bytes: source_bytes,
                // A row created and invalidated inside one window is two mutations.
                max_local_mutations: two_per_row,
                max_pending,
            },
        },
        seed: SeedBounds {
            checkpoint_attempts,
            attempt_wait: Duration::from_millis(attempt_wait_ms),
            max_bytes: seed_bytes,
        },
        // A retirement charges one disposition row per obligation plus the censused bytes and one record.
        retirement: RetirementBounds {
            max_obligations: nonzero_usize(
                "local_transaction_rows",
                (half_rows.get() as u64).min(quarter_local / DISPOSITION_ROW_BYTES),
            )?,
            max_obligation_bytes: obligation_bytes,
        },
        identity,
        generation,
    })
}

/// Runs one slice after another until `cancel` fires. A slice runs on the blocking pool because it holds SQLite and filesystem work; cancellation cancels the slice's budget and waits for the slice to return, so no slice is left running detached. A slice that advanced the record or applied commits runs the next one without waiting; every other outcome idles first, and a request recorded during the idle wait ends it. A supervisor a slice hands back is drained here unless `cancel` fires first, in which case the drain is left to [`SearchLifecycleOwner::shutdown`] so one grace covers it. A panicking slice closes admission and ends the loop, since its state is no longer known.
pub async fn run_slices(owner: Arc<SearchLifecycleOwner>, cancel: CancellationToken) {
    let mut reporter = SliceReporter::default();
    loop {
        let budget = EvalBudget::new(None, Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let slice_owner = Arc::clone(&owner);
        let slice_budget = budget.clone();
        let mut slice = tokio::task::spawn_blocking(move || slice_owner.run_slice(&slice_budget));
        let outcome = tokio::select! {
            biased;
            outcome = &mut slice => outcome,
            () = cancel.cancelled() => {
                budget.cancel();
                slice.await
            }
        };
        let outcome = match outcome {
            Ok(SliceOutcome::RotateMaintenance(handle)) => {
                // Once cancelled, the drain is left to `shutdown`, whose grace then covers it alone.
                let stopped = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    stopped = owner.stop_maintenance(&handle) => stopped,
                };
                match stopped {
                    Ok(_) => continue,
                    Err(unresolved) => {
                        SliceOutcome::Blocked(format!("maintenance did not drain: {unresolved}"))
                    }
                }
            }
            Ok(outcome) => outcome,
            Err(join) => {
                eprintln!("daemon: search lifecycle slice ended abnormally: {join}");
                owner.admission().close();
                return;
            }
        };
        if let Some(report) = reporter.report(&outcome, Instant::now()) {
            eprintln!("daemon: search lifecycle {report}");
        }
        if cancel.is_cancelled() {
            return;
        }
        let advanced = match &outcome {
            SliceOutcome::Advanced(_) => true,
            SliceOutcome::CaughtUp(report) => report.batches_applied > 0,
            _ => false,
        };
        if advanced {
            continue;
        }
        #[cfg(feature = "test-support")]
        owner.tap(SliceEvent::Idle);
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(SLICE_IDLE) => {}
            () = owner.requested.notified() => {}
        }
    }
}

/// The kind of a reported slice outcome; a report's detail may embed a moving value, so repeats are judged by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportKind {
    Blocked,
    Unavailable,
    Closed,
    CatchUpBlocked,
}

/// Decides which slice outcomes reach the log: the first report after an outcome that is not one, a change of kind, and a repeat of the same kind with different detail once per `REPORT_REPEAT_INTERVAL`. A reason that embeds a moving value, such as the kernel tip, would otherwise print on every slice.
#[derive(Default)]
struct SliceReporter {
    last: Option<(ReportKind, String, Instant)>,
}

impl SliceReporter {
    fn report(&mut self, outcome: &SliceOutcome, now: Instant) -> Option<String> {
        let (kind, detail) = match outcome {
            SliceOutcome::Blocked(reason) => {
                (ReportKind::Blocked, format!("did not advance: {reason}"))
            }
            SliceOutcome::Unavailable(reason) => (
                ReportKind::Unavailable,
                format!("did not advance: {reason}"),
            ),
            SliceOutcome::Closed(refusal) => {
                (ReportKind::Closed, format!("admission closed: {refusal}"))
            }
            SliceOutcome::CaughtUp(EpisodeReport {
                end: EpisodeEnd::Blocked(blocked),
                ..
            }) => (
                ReportKind::CatchUpBlocked,
                format!("catch-up blocked: {blocked:?}"),
            ),
            SliceOutcome::Unregistered
            | SliceOutcome::Advanced(_)
            | SliceOutcome::Current
            | SliceOutcome::CaughtUp(_)
            | SliceOutcome::RotateMaintenance(_)
            | SliceOutcome::Disabled => {
                self.last = None;
                return None;
            }
        };
        let print = match &self.last {
            Some((last_kind, last_detail, printed_at)) if *last_kind == kind => {
                *last_detail != detail && now.duration_since(*printed_at) >= REPORT_REPEAT_INTERVAL
            }
            _ => true,
        };
        if !print {
            return None;
        }
        self.last = Some((kind, detail.clone(), now));
        Some(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_gates::REQUIRED_LIMITS;

    const LIMIT: u64 = 1_000_000;

    fn blocked(reason: &str) -> SliceOutcome {
        SliceOutcome::Blocked(reason.to_owned())
    }

    fn test_identity() -> ProjectionIdentity {
        ProjectionIdentity {
            schema_version: retrieval::SCHEMA_VERSION,
            kernel_incarnation_id: "kernel".to_owned(),
            projection_policy_version: PROJECTION_POLICY_VERSION.to_owned(),
            identity_contract_version: IDENTITY_CONTRACT_VERSION.to_owned(),
            limit_manifest_protocol_version: "protocol".to_owned(),
            embedding_model: "model".to_owned(),
            tokenizer_fingerprint: "tokenizer".to_owned(),
            analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                .as_str()
                .to_owned(),
            vector_dimension: 4,
            generation_epoch: 1,
        }
    }

    fn manifest(identity: &ProjectionIdentity) -> RuntimeManifest {
        RuntimeManifest {
            protocol_version: identity.limit_manifest_protocol_version.clone(),
            identity: InvalidationIdentity::from(identity),
            limits: REQUIRED_LIMITS
                .into_iter()
                .map(|name| (name.to_owned(), LIMIT))
                .collect(),
            enabled: Default::default(),
            compressed_activation: false,
            packing: None,
        }
    }

    fn spec(
        manifest: &RuntimeManifest,
        identity: ProjectionIdentity,
    ) -> Result<ReplacementSpec, SpecRefusal> {
        replacement_spec(manifest, identity, Transition::Rebuilding, 3, "generation")
    }

    #[test]
    fn maintenance_limits_that_do_not_fit_the_clock_refuse_the_bounds() {
        for name in ["lease_duration_ms", "B_recovery_ms"] {
            let identity = test_identity();
            let mut manifest = manifest(&identity);
            manifest.limits.insert(name.to_owned(), u64::MAX);
            let spec = spec(&manifest, identity).unwrap();
            assert_eq!(
                maintenance_bounds(&manifest, &spec).err(),
                Some(SpecRefusal::LimitRange(name)),
                "{name}"
            );
        }
    }

    /// An authorized recovery's specification is bounded by `B_authorized_recovery_ms`, so its maintenance bounds are what refuse a zero `B_recovery_ms`.
    #[test]
    fn a_zero_recovery_bound_refuses_the_maintenance_bounds() {
        let identity = test_identity();
        let mut manifest = manifest(&identity);
        manifest.limits.insert("B_recovery_ms".to_owned(), 0);
        let spec = replacement_spec(
            &manifest,
            identity,
            Transition::AuthorizedRecovery,
            3,
            "generation",
        )
        .unwrap();
        assert_eq!(
            maintenance_bounds(&manifest, &spec).err(),
            Some(SpecRefusal::TooSmall("B_recovery_ms"))
        );
    }

    /// A backfill pass obsoletes every terminal candidate it selected in one transaction, one row each, so the pending bound fits the rows a transaction may mutate.
    #[test]
    fn the_pending_bound_fits_the_transaction_rows() {
        let identity = test_identity();
        let mut manifest = manifest(&identity);
        manifest
            .limits
            .insert("local_transaction_rows".to_owned(), 20);
        let spec = spec(&manifest, identity).unwrap();
        assert!(
            spec.episode.batch.max_pending.get() <= 10,
            "{}",
            spec.episode.batch.max_pending
        );
    }

    #[test]
    fn a_zero_supervisor_slice_refuses_the_specification() {
        let identity = test_identity();
        let mut manifest = manifest(&identity);
        manifest.limits.insert("supervisor_slice_ms".to_owned(), 0);
        assert_eq!(
            spec(&manifest, identity).err(),
            Some(SpecRefusal::TooSmall("supervisor_slice_ms"))
        );
    }

    #[test]
    fn a_hold_expiry_past_the_kernel_maximum_refuses_the_specification() {
        let identity = test_identity();
        let mut manifest = manifest(&identity);
        manifest.limits.insert(
            "capture_hold_expiry_ms".to_owned(),
            kernel::MAX_SOURCE_HOLD_LIFETIME_MS + 1,
        );
        assert_eq!(
            spec(&manifest, identity).err(),
            Some(SpecRefusal::LimitRange("capture_hold_expiry_ms"))
        );
    }

    #[test]
    fn the_catch_up_encoded_byte_charges_fit_their_limit_together() {
        let identity = test_identity();
        let spec = spec(&manifest(&identity), identity).unwrap();
        let charged: u64 = spec
            .catchup_page_charges()
            .iter()
            .filter(|(name, _)| *name == "catchup_batch_encoded_bytes")
            .map(|(_, bytes)| bytes)
            .sum();
        assert!(charged <= LIMIT, "{charged} > {LIMIT}");
    }

    #[test]
    fn a_report_of_one_kind_with_moving_detail_repeats_once_per_interval() {
        let mut reporter = SliceReporter::default();
        let start = Instant::now();
        assert!(
            reporter
                .report(&blocked("snapshot 10 exceeds 9"), start)
                .is_some()
        );
        assert!(
            reporter
                .report(&blocked("snapshot 10 exceeds 9"), start)
                .is_none()
        );
        assert!(
            reporter
                .report(&blocked("snapshot 11 exceeds 9"), start + SLICE_IDLE)
                .is_none(),
            "a moving value in the reason does not print every slice"
        );
        assert!(
            reporter
                .report(
                    &blocked("snapshot 12 exceeds 9"),
                    start + REPORT_REPEAT_INTERVAL
                )
                .is_some(),
            "the same kind prints again once the interval has passed"
        );
    }

    #[test]
    fn a_change_of_kind_or_an_intervening_advance_prints_at_once() {
        let mut reporter = SliceReporter::default();
        let start = Instant::now();
        assert!(reporter.report(&blocked("stuck"), start).is_some());
        assert!(
            reporter
                .report(&SliceOutcome::Unavailable("corrupt".to_owned()), start)
                .is_some()
        );
        assert!(reporter.report(&SliceOutcome::Current, start).is_none());
        assert!(
            reporter.report(&blocked("stuck"), start).is_some(),
            "a report after an outcome that is not one prints"
        );
    }
}
