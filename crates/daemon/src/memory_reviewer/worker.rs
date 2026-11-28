//! The production MemoryReviewer worker: the one place Ready review jobs are claimed and run. Each pass reads the deployment owner's activation record anew; a closed gate runs nothing and sends nothing. With the gate open, the worker walks every route whose memories authority is MODULE, claims each Ready job under the authority's lease, begins or resumes its receipt, and runs the coordinator's investigation, which settles the run through its completed receipt. Both producers' jobs, History Summarizer subjects and Memory Classifier selections, enter here alike. Cancellation ends the pass at the coordinator's next check and the in-flight run is joined before the task returns, so shutdown observes physical completion before the stores are released.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use host_runtime::model_execution::subprocess::{CREDENTIAL_VALUE_CAP_BYTES, CREDENTIAL_VARIABLES};
use host_runtime::model_execution::supervisor::Supervisor;
use kernel::{ReviewBinding, ReviewOwner, ReviewStagedReference, SourceDependency};
use memory_store::memory_reviewer_jobs::{
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT, MemoryReviewerJob, MemoryReviewerJobState,
    ReviewTarget,
};
use memory_store::memory_reviewer_ledger::{MemoryReviewerBeginOutcome, MemoryReviewerReceipt};
use memory_store::{LeaseAcquireOutcome, LeaseClaim, MemoryStore};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::canonical_memory::MEMORY_DOMAIN_ID;
use crate::kernel_routes::{KernelOpenCoordinator, KernelState, ProjectBinding};
use crate::{MemoriesAuthority, RouteBindings, SessionBinding, memories_authority_for_route};

use super::activation::{self, Activation, Closed, LiveIdentity};
use super::broker::{MAX_ISSUED_INSPECTIONS, QuestionTemplate};
use super::classifier::{CLASSIFY_WAIT, ClassifierExchange, ClassifierRoute};
use super::coordinator::{
    Coordinator, InvestigationError, InvestigationPermits, JobContext, Rejection, StageOne,
};
use super::disclosure::{DisclosureApproval, ModelProfile};
use super::host_execution::{HOST_CREDENTIAL_ID, HostExecution};
use super::lifecycle::{ActivationState, MemoryReviewerStatus};
use super::model_request::{Credential, Provider, Providers, Sender};
use super::project_text::{InspectionBinding, ProjectText, ProtectedLocations};
use super::review_cycle::{self, Applied, ApplyScope, SelectionScope};
use super::settlement::Settled;
use super::settlement::TaskClaim;
use super::stage_one::{Decider, StageOneBook};
use crate::HistorySummarizerProducerFactory;
use crate::claim_sources::ClaimProgress;
use crate::config::{ReviewApply, Stage1Model};

/// The pause between two stage-1 attempts of one run after one ended without an answer.
#[cfg(not(test))]
const STAGE_ONE_RETRY_PAUSE: Duration = Duration::from_secs(2);
#[cfg(test)]
const STAGE_ONE_RETRY_PAUSE: Duration = Duration::from_millis(20);
/// Derived stage-1 defaults remembered at once; the map starts over past this.
const MAX_REMEMBERED_DEFAULTS: usize = 64;
/// This limit bounds the set of roots tracked for once-per-root `Grant::LeftToOwner` logging.
const MAX_REPORTED_OWNER_ROOTS: usize = 64;

/// Idle interval between passes when no job was run.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(30);
/// Runs per project per pass. Ready jobs the pass skips (no bound root owns the subject, another worker holds it, already settled) do not count, so a deadline-ordered prefix of them cannot starve the jobs behind it.
pub const JOBS_PER_PASS: usize = 8;
/// Output budget and sampling every MemoryReviewer request carries; the model id comes from the activation record.
pub const MAX_TOKENS: u32 = 4096;
/// Credential variables the host may hand the daemon: every name the startup envelope admits.
pub const MAX_CREDENTIALS: usize = CREDENTIAL_VARIABLES.len();
/// Bytes [`MemoryReviewerHost`] retains for the process lifetime: one value under [`CREDENTIAL_VALUE_CAP_BYTES`] per credential, its name, and its keyed identity. Declared under the daemon's retained-resident bytes.
pub const RETAINED_CREDENTIAL_BYTES: u64 =
    MAX_CREDENTIALS as u64 * (CREDENTIAL_VALUE_CAP_BYTES as u64 + 256);

pub struct MemoryReviewerHost {
    pub supervisor: Arc<Supervisor>,
    pub credentials: BTreeMap<String, Zeroizing<String>>,
    /// Credential identities keyed under the incarnation key; unset until the key exists.
    pub credential_identities: OnceLock<BTreeMap<String, String>>,
    pub worker_instance: String,
}

/// The operator-facing text of a closed gate. An identity mismatch carries the live value of the term that differs; the credential fingerprint has no entry in [`activation::identity_terms`], so a credential mismatch reports no value.
fn closed_reason(closed: &Closed, live: Option<&LiveIdentity>) -> String {
    match (closed, live) {
        (Closed::IdentityMismatch(field), Some(live)) => activation::identity_terms(live)
            .into_iter()
            .find(|(term, _)| term == field)
            .map_or_else(
                || closed.to_string(),
                |(_, value)| format!("{closed}; the live value is {value}"),
            ),
        _ => closed.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoute {
    pub project: String,
    pub authority_generation: u64,
    /// Newest binding first.
    pub roots: Vec<RootScope>,
    /// The review switch of the project, when the user tier sets one: the user tier's models and mode as they read at this listing, with the harness and credential fingerprints of the project's newest listed root.
    pub review: Option<ReviewSwitch>,
    /// Whether the owner's activation record runs this project: its memories authority is one the owner recorded and a route of it is bound. The authority the review switch gives a root covers reviews of its stored memories alone, and a root listed after its routes closed is listed for those reviews.
    pub recorded: bool,
}

/// One project's `memory_reviewer` settings as the user tier reads at the worker's pass, with the route facts a host-routed request connects under, which the newest listed root's binding froze.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSwitch {
    /// The `provider/model` every proposal request of the project's reviews names.
    pub model_ref: String,
    /// The chat model that decides stage 1 of each review.
    pub stage1_model: Stage1Model,
    pub apply: ReviewApply,
    /// The `provider/model` of the typed classifier that decides stage 1 in the chat model's place, when the user tier names one.
    pub classifier: Option<String>,
    /// What the switch reviews: each session at its end, or each stored memory on later evidence.
    pub review: crate::config::ReviewScope,
    /// The user tier's memory budget, as `f64` bits: a session review reads the block a new session receives under it.
    pub memory_budget_bits: u64,
    /// Whether capture's consent holds (`memory_enabled`, `auto_promote`, and `memory_auto_capture`): a session review records a user's sentence as a row of its own only then.
    pub record_sentences: bool,
    pub harness: String,
    pub credential_fingerprints: BTreeMap<String, String>,
}

/// What the worker holds for projects under the review switch: the way to the host's model execution, whether another owner publishes claim descriptors, and the claim consumer's position when this worker publishes them.
#[derive(Default)]
pub struct SwitchedReview {
    /// The factory host-routed requests connect through. Without one, a switched project is listed and left unreviewed.
    pub producers: Option<Arc<dyn HistorySummarizerProducerFactory>>,
    /// Whether the search lifecycle owner runs the claim consumer; this worker runs it otherwise.
    pub claims_published: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    claims: Mutex<ClaimProgress>,
    /// One pass runs at a time, whether the loop or a caller of [`Worker::drain`] started it.
    serial: tokio::sync::Mutex<()>,
    /// Where typed-classifier requests wait for the harness; the daemon's `memory.review.classify` handlers hold the same exchange.
    pub classifiers: Arc<ClassifierExchange>,
    /// Each review's stage-1 record until its proposal is applied, and each project's last stage-1 failure.
    stage_one: Arc<StageOneBook>,
    derived_defaults: Mutex<DerivedDefaults>,
    /// What selection remembers between passes: the writers of the sources it looked up and the memories that wait for a pending proposal.
    selection: Arc<Mutex<review_cycle::SelectionMemory>>,
    /// The Kernel tip at which each project's change detection last found nothing left to read.
    detection: super::change_detection::DetectionMemory,
}

impl SwitchedReview {
    /// Sets the bounds of the selection's walk, so a test covers a store larger than one selection examines.
    #[cfg(test)]
    pub(crate) fn walk_within(&self, walk: review_cycle::Walk) {
        self.selection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .walk_within(walk);
    }

    /// Reviews switched projects through `producers`. `claims_published` reports whether another owner runs the claim consumer, and `classifiers` is the exchange the host's `memory.review.classify` handlers answer.
    pub fn through(
        producers: Arc<dyn HistorySummarizerProducerFactory>,
        claims_published: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        classifiers: Arc<ClassifierExchange>,
    ) -> Self {
        Self {
            producers: Some(producers),
            claims_published,
            classifiers,
            ..Self::default()
        }
    }
}

/// How each derived stage-1 default has fared in this process. A default derived from the review model's name is a guess at the host's catalog: an answer proves it, and a failure before any answer is read as the host refusing it, after which the project's stage 1 goes to the review model.
#[derive(Default)]
struct DerivedDefaults {
    /// `true` once the reference answered a request, `false` once it failed before answering any.
    proven: BTreeMap<String, bool>,
}

impl DerivedDefaults {
    /// The chat model that answers `switch`'s stage 1: its stage-1 model, or the review model once a derived default was refused.
    fn model<'a>(&self, switch: &'a ReviewSwitch) -> &'a str {
        let refused = switch.stage1_model.derived
            && self.proven.get(&switch.stage1_model.reference) == Some(&false);
        if refused {
            &switch.model_ref
        } else {
            &switch.stage1_model.reference
        }
    }

    /// Whether `switch`'s stage-1 model is a derived default that no request has yet answered or failed.
    fn unproven(&self, switch: &ReviewSwitch) -> bool {
        switch.stage1_model.derived && !self.proven.contains_key(&switch.stage1_model.reference)
    }

    /// Records how `switch`'s derived default fared in the job that just ran, and returns whether this call is the one that refused it. A named stage-1 model is the user's choice and is never replaced.
    fn note(&mut self, switch: &ReviewSwitch, answered: bool, failed: bool) -> bool {
        if !switch.stage1_model.derived || !(answered || failed) {
            return false;
        }
        let reference = &switch.stage1_model.reference;
        if self.proven.len() >= MAX_REMEMBERED_DEFAULTS && !self.proven.contains_key(reference) {
            self.proven.clear();
        }
        match self.proven.get(reference) {
            Some(_) if !answered => false,
            Some(true) => false,
            _ => {
                self.proven.insert(reference.clone(), answered);
                !answered
            }
        }
    }
}

/// One bound root and the Kernel scope derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootScope {
    pub project_root: PathBuf,
    pub project_digest: String,
    pub scope_id: String,
    /// Whether a route of the root is bound. A root listed after its routes closed is listed for the review switch, which reviews its stored memories; a fact set is run under a bound root.
    pub bound: bool,
}

pub struct Worker {
    pub host: Arc<MemoryReviewerHost>,
    /// The data home the activation record lives under; also the location a project inspection may never read.
    pub home: PathBuf,
    pub store: Arc<MemoryStore>,
    /// The Kernel while it is ready; `None` runs nothing this pass.
    pub kernel: Arc<dyn Fn() -> Option<Arc<kernel::KernelStore>> + Send + Sync>,
    /// The projects whose Ready jobs this worker runs.
    pub projects: Arc<dyn Fn() -> Vec<ProjectRoute> + Send + Sync>,
    pub status: Arc<MemoryReviewerStatus>,
    pub permits: Arc<InvestigationPermits>,
    pub providers: Providers,
    pub rejection: RejectionLatch,
    pub switched: SwitchedReview,
}

/// The rejection that latched the gate closed for the rest of the process, once one has. Only the worker's own pass sets it; a new worker, as after a restart, starts unlatched.
#[derive(Debug, Default)]
pub struct RejectionLatch(OnceLock<Rejection>);

impl RejectionLatch {
    pub fn get(&self) -> Option<&Rejection> {
        self.0.get()
    }
}

/// The worker's roots with MODULE authority, grouped by authority project, newest binding first within each project's roots: every bound root, and each root under the review switch whose routes have closed.
pub(crate) fn module_projects(
    store: &MemoryStore,
    bindings: &Mutex<RouteBindings>,
    user_tier: Option<&crate::config::DaemonConfig>,
) -> Vec<ProjectRoute> {
    let (mut roots, bound): (Vec<(u64, SessionBinding)>, BTreeSet<PathBuf>) = {
        let bindings = bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            bindings
                .review_roots()
                .into_values()
                .map(|(seq, binding)| (seq, binding.clone()))
                .collect(),
            bindings
                .latest_per_root()
                .into_keys()
                .map(Path::to_path_buf)
                .collect(),
        )
    };
    roots.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    let mut projects: BTreeMap<String, ProjectRoute> = BTreeMap::new();
    for (_, binding) in roots {
        let root = binding.project_root.to_string_lossy().to_string();
        // The six `memory_reviewer` keys are the user tier's alone. The tier as it reads now decides them, so a switch turned off stops the reviews of a root whose routes have closed; without it the binding's own load stands.
        let config = user_tier.unwrap_or(&binding.config);
        // A root whose own tier file the load refused is reviewed by no one, as it is folded by no one.
        let admitted = binding.config.admission == crate::config::ConfigAdmission::Admitted;
        let review = config
            .memory_reviewer_model()
            .filter(|_| admitted)
            .zip(config.memory_reviewer_stage1_model())
            .map(|(model, stage1_model)| ReviewSwitch {
                model_ref: model.to_string(),
                stage1_model,
                apply: config.memory_reviewer.apply,
                classifier: config.memory_reviewer_classifier().map(str::to_string),
                review: config.memory_reviewer.review,
                memory_budget_bits: config.memory_budget_tokens.to_bits(),
                // Capture's consent as capture reads it for the root: the user tier's own switches, and the root's merged load, whose project tier may turn memory or promotion off.
                record_sentences: config.memory_enabled
                    && config.auto_promote
                    && config.memory_auto_capture
                    && binding.config.memory_enabled
                    && binding.config.auto_promote,
                harness: binding.harness.clone(),
                credential_fingerprints: binding.credential_fingerprints.clone(),
            });
        // The switch is the user's consent to review this root's memories, so the root takes the authority every review row is keyed and fenced by.
        if review.is_some() {
            match review_cycle::grant_authority(store, &root) {
                Ok(review_cycle::Grant::Held) => {}
                Ok(review_cycle::Grant::LeftToOwner) => {
                    static REPORTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
                    let mut reported = REPORTED
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if reported.len() < MAX_REPORTED_OWNER_ROOTS && reported.insert(root.clone()) {
                        eprintln!(
                            "daemon: memory_reviewer leaves a switched root to the authority identity its route is bound to; its reviews start when that identity holds MODULE memories authority"
                        );
                    }
                }
                Err(error) => eprintln!(
                    "daemon: memory_reviewer could not take memories authority for a switched root: {error}"
                ),
            }
        }
        if let Ok(MemoriesAuthority::Module(authority)) = memories_authority_for_route(store, &root)
        {
            let owner_recorded =
                authority.context_store_uuid != review_cycle::SWITCH_AUTHORITY_STORE;
            let project = projects
                .entry(authority.project.clone())
                .or_insert(ProjectRoute {
                    project: authority.project,
                    authority_generation: authority.generation,
                    roots: Vec::new(),
                    review,
                    recorded: false,
                });
            // Any bound root of the project keeps it under the owner's record.
            project.recorded |= owner_recorded && bound.contains(&binding.project_root);
            project.roots.push(RootScope {
                project_root: binding.project_root.clone(),
                project_digest: binding.kernel_project.digest().to_string(),
                scope_id: binding.kernel_project.scope_id(),
                bound: bound.contains(&binding.project_root),
            });
        }
    }
    projects.into_values().collect()
}

/// Whether `job` reviews a stored memory, which is the review the switch selects.
fn reviews_memory(job: &MemoryReviewerJob) -> bool {
    job.question_template == QuestionTemplate::MemoryStatement.id()
}

/// The Kernel store while the coordinator reports it ready.
pub(crate) fn ready_kernel(kernel: &KernelOpenCoordinator) -> Option<Arc<kernel::KernelStore>> {
    (kernel.state() == KernelState::Ready)
        .then(|| kernel.kernel_store().ok())
        .flatten()
}

impl Worker {
    fn live_identity(
        &self,
        kernel: &kernel::KernelStore,
        provider: &Provider,
    ) -> Option<LiveIdentity> {
        let budget = kernel::applicability::EvalBudget::new(
            Some(std::time::Instant::now() + Duration::from_secs(5)),
            Arc::default(),
        );
        let kernel_incarnation = kernel.database_incarnation_id_within_budget(&budget).ok()?;
        let memstore_incarnation = self.store.memory_reviewer_store_incarnation().ok()?;
        Some(LiveIdentity {
            kernel_baseline_digest: kernel::kernel_baseline_digest().ok()?.to_string(),
            memstore_baseline_digest: memory_store::baseline_digest(),
            kernel_incarnation,
            memstore_incarnation,
            provider: provider.identity(),
            // Only the credential the provider's protocol carries can vouch for it; a record naming another provider's secret reads as an unknown credential.
            credentials: self
                .host
                .credential_identities
                .get()?
                .iter()
                .filter(|(name, _)| {
                    *name == provider.credential_name() && self.host.credentials.contains_key(*name)
                })
                .map(|(name, identity)| (name.clone(), identity.clone()))
                .collect(),
        })
    }

    /// The provider a record names: Anthropic for schema 1, and Bedrock invoking the record's model id in the startup region for schema 2.
    fn provider_for(&self, record: &activation::RuntimeIdentityRecord) -> Result<Provider, Closed> {
        match &record.model_id {
            None => Ok(self.providers.anthropic().clone()),
            Some(model_id) => self
                .providers
                .bedrock(model_id)
                .map_err(|_| Closed::Undialable("bedrock region or model id")),
        }
    }

    fn gate(&self, kernel: &kernel::KernelStore) -> Result<(Activation, Provider), Closed> {
        let mut live = None;
        let gate = activation::read_record(&self.home).and_then(|record| {
            let provider = self.provider_for(&record)?;
            let identity = self
                .live_identity(kernel, &provider)
                .ok_or_else(|| Closed::Unreadable("store identity".to_string()))?;
            let activation = activation::evaluate(&record, &identity);
            live = Some(identity);
            Ok((activation?, provider))
        });
        match &gate {
            Ok(_) => self.status.set_activation(ActivationState::Open),
            Err(closed) => {
                let reason = closed_reason(closed, live.as_ref());
                if self
                    .status
                    .set_closed(ActivationState::from(closed), reason.clone())
                {
                    eprintln!("daemon: memory_reviewer activation gate closed: {reason}");
                }
            }
        }
        gate
    }

    /// `gate` on the blocking pool: it reads the record from disk and queries both stores.
    async fn gate_off_runtime(
        self: &Arc<Self>,
        kernel: &Arc<kernel::KernelStore>,
    ) -> Result<(Activation, Provider), Closed> {
        let worker = Arc::clone(self);
        let kernel = Arc::clone(kernel);
        tokio::task::spawn_blocking(move || worker.gate(&kernel))
            .await
            .unwrap_or_else(|_| Err(Closed::Unreadable("gate evaluation".to_string())))
    }

    /// Evaluates the owner's record to its conclusion: the gate, then the credential the record's provider sends with. Every evaluation ends in one state, so the status reads the same after a pass's first evaluation and after any later one.
    async fn recorded_gate(
        self: &Arc<Self>,
        kernel: &Arc<kernel::KernelStore>,
    ) -> Option<(Activation, Provider, Credential)> {
        let (activation, provider) = self.gate_off_runtime(kernel).await.ok()?;
        let credential = provider.credential(activation.approval.credential_id.clone(), |name| {
            self.host
                .credentials
                .get(name)
                .map(|value| value.to_string())
        });
        match credential {
            Ok(credential) => Some((activation, provider, credential)),
            Err(_) => {
                self.status
                    .set_activation(ActivationState::Closed("unknown_credential"));
                None
            }
        }
    }

    /// Closes the gate as `unavailable` for the rest of the process after a provider rejected an attempt. The first rejection logs once, naming the status or the reported model and never any content.
    fn latch(&self, rejection: Rejection) {
        if self.rejection.0.set(rejection.clone()).is_ok() {
            eprintln!(
                "daemon: memory_reviewer provider rejected an attempt ({rejection}); the gate stays closed until the daemon restarts"
            );
        }
        self.status.set_switched_review(false);
        self.stamp_latched();
    }

    /// Records the latched gate as `unavailable` with the rejection as its reason.
    fn stamp_latched(&self) {
        if let Some(rejection) = self.rejection.get() {
            self.status.set_closed(
                ActivationState::Closed("unavailable"),
                format!("provider rejected an attempt ({rejection})"),
            );
        }
    }

    /// One pass over every project the worker runs for: first the projects the owner's activation record admits, then the projects under the review switch. Both read one listing of the projects, so one pass sees one configuration. The record is evaluated on every pass, so the status always reads a current state. Returns how many runs settled. Passes of one worker run one at a time.
    pub async fn pass(self: &Arc<Self>, cancel: &CancellationToken) -> usize {
        self.pass_at(cancel, super::change_detection::Moment::Idle)
            .await
    }

    /// [`Self::pass`], with change detection told whether the caller marks the end of a session: a drain does, so the newest session's messages are read at once; the worker's own loop reads a session once it has been quiet (`change_detection::QUIET`) or a newer session has begun.
    async fn pass_at(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        moment: super::change_detection::Moment,
    ) -> usize {
        let _serial = self.switched.serial.lock().await;
        let projects = Arc::clone(&self.projects);
        let Ok(routes) = tokio::task::spawn_blocking(move || projects()).await else {
            eprintln!("daemon: memory_reviewer project listing ended without a result");
            return 0;
        };
        let recorded = self.recorded_pass(&routes, cancel).await;
        recorded + self.switched_pass(routes, cancel, moment).await
    }

    /// Runs passes until one settles nothing and returns the runs settled. A pass also settles nothing while a project's reviews wait: for 60 seconds after a failed stage-1 request, and while its review holds are at their bound.
    pub async fn drain(self: &Arc<Self>, cancel: &CancellationToken) -> usize {
        let mut settled = 0;
        loop {
            let pass = self
                .pass_at(cancel, super::change_detection::Moment::SessionEnd)
                .await;
            settled += pass;
            if pass == 0 || cancel.is_cancelled() {
                return settled;
            }
        }
    }

    /// Stage 1 of a switched project's reviews: a small chat model decides through the host's model execution, and the typed classifier the project names decides in its place while a harness answers classifier work. `None` when the stage-1 model reference names no host route.
    fn stage_one(
        &self,
        producers: &Arc<dyn HistorySummarizerProducerFactory>,
        switch: &ReviewSwitch,
        root: &RootScope,
        (kernel_incarnation, memstore_incarnation): (&str, &str),
    ) -> Option<StageOne> {
        let (model, unproven_default) = {
            let defaults = self
                .switched
                .derived_defaults
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                defaults.model(switch).to_string(),
                defaults.unproven(switch),
            )
        };
        let host = HostExecution::new(
            Arc::clone(producers),
            root.project_root.clone(),
            switch.harness.clone(),
            switch.credential_fingerprints.clone(),
            model.clone(),
        )
        .ok()?;
        let sender = Arc::new(Sender::through_host(Arc::new(host)));
        Some(StageOne {
            approval: DisclosureApproval {
                provider: sender.provider_identity(),
                model,
                credential_id: HOST_CREDENTIAL_ID.to_string(),
                kernel_incarnation: kernel_incarnation.to_string(),
                memstore_incarnation: memstore_incarnation.to_string(),
            },
            sender,
            classifier: switch
                .classifier
                .as_ref()
                .map(|classifier| ClassifierRoute {
                    exchange: Arc::clone(&self.switched.classifiers),
                    classifier: classifier.clone(),
                    project: root.project_digest.clone(),
                    wait: CLASSIFY_WAIT,
                }),
            book: Arc::clone(&self.switched.stage_one),
            project: root.project_digest.clone(),
            retry_pause: STAGE_ONE_RETRY_PAUSE,
            unproven_default,
        })
    }

    /// Whether a route's project is reviewed under the switch: the user tier sets one for it and the worker can reach the host's model execution.
    fn is_switched(&self, route: &ProjectRoute) -> bool {
        route.review.is_some() && self.switched.producers.is_some()
    }

    /// Brings the claim consumer up to the Kernel's tip on a host where this worker runs it, on the blocking pool, and returns whether it reached the tip. A caught-up consumer costs one checkpoint read and one tip read. A host whose search lifecycle owner runs the consumer returns false at once.
    async fn catch_up_claims(
        self: &Arc<Self>,
        kernel: &Arc<kernel::KernelStore>,
        cancel: &CancellationToken,
    ) -> bool {
        if self
            .switched
            .claims_published
            .as_ref()
            .is_some_and(|published| published())
        {
            return false;
        }
        let worker = Arc::clone(self);
        let kernel = Arc::clone(kernel);
        let cancel = cancel.clone();
        let caught_up = tokio::task::spawn_blocking(move || {
            let mut claims = worker
                .switched
                .claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            review_cycle::publish_claims(&kernel, &mut claims, crate::now_ms(), &|| {
                cancel.is_cancelled()
            })
        })
        .await;
        caught_up.unwrap_or_else(|error| {
            eprintln!("daemon: memory_reviewer claim catch-up task ended: {error}");
            false
        })
    }

    /// The pass over switched projects: claim descriptors are brought up to the Kernel's tip, each project's most suspicious unreviewed memories are enqueued, its Ready jobs run through the host's model execution, and each published proposal is treated as the project's apply mode says.
    async fn switched_pass(
        self: &Arc<Self>,
        routes: Vec<ProjectRoute>,
        cancel: &CancellationToken,
        moment: super::change_detection::Moment,
    ) -> usize {
        let Some(producers) = self.switched.producers.clone() else {
            return 0;
        };
        let routes: Vec<ProjectRoute> = routes
            .into_iter()
            .filter(|route| self.is_switched(route))
            .collect();
        // An answer from another model closed review for the rest of the process, as it does on the direct route.
        let latched = self.rejection.get().is_some();
        // The status block reads `open` while a switched project is under review. The owner's record keeps its own state, which is what admits fact sets.
        self.status
            .set_switched_review(!routes.is_empty() && !latched);
        if latched {
            self.stamp_latched();
            return 0;
        }
        if routes.is_empty() {
            return 0;
        }
        let Some(kernel) = (self.kernel)() else {
            return 0;
        };
        // A catch-up that reached the tip is repeated before each review. One that stopped short, as a blocked slice does, has said so once and is left to the next pass, and so is the consumer a search lifecycle owner runs.
        let mut repeat_catch_up = self.catch_up_claims(&kernel, cancel).await;
        let prepared = {
            let worker = Arc::clone(self);
            let kernel = Arc::clone(&kernel);
            tokio::task::spawn_blocking(move || {
                let budget = kernel::applicability::EvalBudget::new(
                    Some(std::time::Instant::now() + Duration::from_secs(5)),
                    Arc::default(),
                );
                Some((
                    kernel.database_incarnation_id_within_budget(&budget).ok()?,
                    worker.store.memory_reviewer_store_incarnation().ok()?,
                ))
            })
            .await
            .ok()
            .flatten()
        };
        let Some((kernel_incarnation, memstore_incarnation)) = prepared else {
            return 0;
        };
        let mut settled = 0;
        for route in routes {
            if cancel.is_cancelled() {
                break;
            }
            let (Some(switch), Some(root)) = (route.review.clone(), route.roots.first().cloned())
            else {
                continue;
            };
            let Ok(host) = HostExecution::new(
                Arc::clone(&producers),
                root.project_root.clone(),
                switch.harness.clone(),
                switch.credential_fingerprints.clone(),
                switch.model_ref.clone(),
            ) else {
                eprintln!(
                    "daemon: memory_reviewer model reference for a switched project names no host route; its reviews wait for a corrected memory_reviewer.model"
                );
                continue;
            };
            // Under `review: session` the review model reads each session that ended beside the delivered statements, and stored memories are not reviewed one by one. `apply: propose` writes nothing a reader has not accepted, so it keeps the per-memory review, whose proposals wait for a reader.
            if switch.review == crate::config::ReviewScope::Session
                && switch.apply != ReviewApply::Propose
            {
                // The root's path is resolved again here; a path that now names another project than the one its route bound is left unread.
                let project = ProjectBinding::new(&root.project_root);
                if project.digest() != root.project_digest {
                    eprintln!(
                        "daemon: memory_reviewer leaves a switched root whose path no longer names the project its route bound"
                    );
                    continue;
                }
                let detected = super::change_detection::pass(
                    &kernel,
                    &self.store,
                    &project,
                    &host,
                    &self.switched.detection,
                    super::change_detection::PassSettings {
                        moment,
                        memory_budget_tokens: f64::from_bits(switch.memory_budget_bits),
                        writes: super::change_detection::Writes {
                            record_sentences: switch.record_sentences,
                        },
                    },
                    cancel,
                )
                .await;
                if let Some(rejection) = detected.rejection {
                    self.latch(rejection);
                    self.stamp_latched();
                    return settled;
                }
                settled += usize::from(detected.covered);
                continue;
            }
            // A decider that failed a moment ago is given time: the project's jobs stay Ready for a later pass.
            let stage_one_failing = || self.switched.stage_one.failing(&root.project_digest);
            if stage_one_failing() {
                continue;
            }
            let sender = Arc::new(Sender::through_host(Arc::new(host)));
            let Some(stage_one) = self.stage_one(
                &producers,
                &switch,
                &root,
                (&kernel_incarnation, &memstore_incarnation),
            ) else {
                eprintln!(
                    "daemon: memory_reviewer stage-1 model reference for a switched project names no host route; its reviews wait for a corrected memory_reviewer.stage1_model"
                );
                continue;
            };
            let mut coordinator = Coordinator {
                store: Arc::clone(&kernel),
                ledger: Arc::clone(&self.store),
                supervisor: Arc::clone(&self.host.supervisor),
                approval: Some(DisclosureApproval {
                    provider: sender.provider_identity(),
                    model: switch.model_ref.clone(),
                    credential_id: HOST_CREDENTIAL_ID.to_string(),
                    kernel_incarnation: kernel_incarnation.clone(),
                    memstore_incarnation: memstore_incarnation.clone(),
                }),
                sender,
                profile: ModelProfile {
                    model: switch.model_ref.clone(),
                    max_tokens: MAX_TOKENS,
                    temperature: None,
                },
                credential_id: HOST_CREDENTIAL_ID.to_string(),
                permits: Arc::clone(&self.permits),
                now_ms: Arc::new(crate::now_ms),
                inspection_limit: MAX_ISSUED_INSPECTIONS,
                stage_one: Some(stage_one),
            };
            let route = Arc::new(route);
            let ready = {
                let store = Arc::clone(&self.store);
                let kernel = Arc::clone(&kernel);
                let route = Arc::clone(&route);
                let root = root.clone();
                let selection = Arc::clone(&self.switched.selection);
                tokio::task::spawn_blocking(move || {
                    let now = crate::now_ms();
                    let list = |store: &MemoryStore| {
                        store
                            .ready_memory_reviewer_jobs(
                                &route.project,
                                MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT,
                                now,
                            )
                            .map_err(|error| error.to_string())
                    };
                    let mut selection = selection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Each run ends in a proposal that takes a review hold, so the pass selects and runs as many reviews as the hold bounds still admit.
                    let holds_left = review_cycle::holds_left(
                        &kernel,
                        &mut selection,
                        &root.project_digest,
                        now,
                    )
                    .map_err(|error| error.to_string())?;
                    if holds_left == 0 {
                        return Ok((Vec::new(), 0));
                    }
                    let pending = list(&store)?;
                    let scope = kernel::ProjectScope::new(&root.project_digest)
                        .map_err(|error| error.to_string())?;
                    let selected = review_cycle::select_and_enqueue(
                        &kernel,
                        &store,
                        &mut selection,
                        &SelectionScope {
                            project: &scope,
                            project_digest: &root.project_digest,
                            ledger_project: &route.project,
                        },
                        // A fact-set job of the project waits for the owner's record and its pass, so the switch's allowance counts the memory reviews alone. The store bounds the project's pending jobs of every kind at reservation.
                        review_cycle::room(
                            pending.iter().filter(|job| reviews_memory(job)).count(),
                            JOBS_PER_PASS,
                            holds_left,
                        ),
                        now,
                    )
                    .map_err(|error| error.to_string())?;
                    if selected.total() == 0 {
                        return Ok((pending, holds_left));
                    }
                    eprintln!(
                        "daemon: memory_reviewer selected {} of {} memories for review: {:?}; {} wait for a pending proposal",
                        selected.total(),
                        selected.examined,
                        review_cycle::describe(&selected),
                        selected.waiting
                    );
                    Ok((list(&store)?, holds_left))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            };
            let (mut ready, holds_left) = match ready {
                Ok(ready) => ready,
                Err(error) => {
                    eprintln!(
                        "daemon: memory_reviewer selection for a switched project failed: {error}"
                    );
                    continue;
                }
            };
            // The switch reviews stored memories. A fact-set job of the project belongs to the owner's record and its pass.
            ready.retain(reviews_memory);
            // A selection numbers its jobs in its own order, the automatically delivered memories first and the newest evidence first among them; the jobs of one selection share a queue deadline, so its order decides which run first.
            ready.sort_by_key(|job| (job.queue_deadline_ms, job.producer.ordinal));
            let mut runs = 0;
            for job in ready {
                if cancel.is_cancelled()
                    || runs == JOBS_PER_PASS.min(holds_left)
                    || stage_one_failing()
                {
                    break;
                }
                // While the last catch-up reached the tip, the claim consumer reads the commits of the reviews before this one, so its checkpoint trails the Kernel's tip by one review at most however long the pass runs.
                if repeat_catch_up {
                    repeat_catch_up = self.catch_up_claims(&kernel, cancel).await;
                }
                // A pass over many reviews outlasts one evaluation's lifetime, so each run restates that a switched project is under review and evaluates the owner's record again, which is what admits fact sets meanwhile.
                self.status.set_switched_review(true);
                let _ = self.recorded_gate(&kernel).await;
                let job = Arc::new(job);
                let outcome = self
                    .run_job(&coordinator, &kernel, &route, Arc::clone(&job), cancel)
                    .await;
                if !matches!(outcome, Ok(None)) {
                    runs += 1;
                }
                // The run's stage-1 record is taken here whether or not the run published a proposal, so the book holds none of a job that ended.
                let stage_one = self.switched.stage_one.take_record(&job.causal_identity);
                let answered = stage_one.as_ref().is_some_and(|record| {
                    record.decider == Decider::ChatModel(switch.stage1_model.reference.clone())
                });
                // The run reports a default that failed before any answer; the worker then sends the project's stage 1 to the review model, for the reviews left in this pass too.
                let failed = self
                    .switched
                    .stage_one
                    .take_default_refused(&root.project_digest);
                let refused = self
                    .switched
                    .derived_defaults
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .note(&switch, answered, failed);
                if refused {
                    eprintln!(
                        "daemon: memory_reviewer stage-1 default {} failed its first request; stage 1 uses the review model until the daemon restarts. Set memory_reviewer.stage1_model to a small model the host serves.",
                        switch.stage1_model.reference
                    );
                }
                // A derived default is proven by its first answer and replaced at its first refusal; the next run is built with what this one showed.
                if switch.stage1_model.derived {
                    coordinator.stage_one = self.stage_one(
                        &producers,
                        &switch,
                        &root,
                        (&kernel_incarnation, &memstore_incarnation),
                    );
                }
                match outcome {
                    Ok(Some(outcome)) => {
                        settled += 1;
                        if matches!(outcome, Settled::Published(_)) {
                            let applied = {
                                let kernel = Arc::clone(&kernel);
                                let store = Arc::clone(&self.store);
                                let route = Arc::clone(&route);
                                let root = root.clone();
                                let (kernel_incarnation, memstore_incarnation) =
                                    (kernel_incarnation.clone(), memstore_incarnation.clone());
                                let mode = switch.apply;
                                let walk = self
                                    .switched
                                    .selection
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .walk();
                                // A change acts on the later references of a review stage 1 flagged. A run with no stage-1 record, as one adopted after a restart, has none.
                                let later = stage_one
                                    .filter(|record| {
                                        record.decision == super::stage_one::Stage1::Flagged
                                    })
                                    .map(|record| record.later);
                                let job = Arc::clone(&job);
                                tokio::task::spawn_blocking(move || {
                                    review_cycle::apply_proposal(
                                        &kernel,
                                        &store,
                                        &ApplyScope {
                                            ledger_project: &route.project,
                                            project_root: &root.project_root,
                                            project_digest: &root.project_digest,
                                            kernel_incarnation: &kernel_incarnation,
                                            memstore_incarnation: &memstore_incarnation,
                                            walk,
                                        },
                                        &job.causal_identity,
                                        mode,
                                        later.as_deref(),
                                        crate::now_ms(),
                                    )
                                })
                                .await
                                .unwrap_or_else(|error| Applied::Refused(error.to_string()))
                            };
                            // A change proposal a reader has yet to settle holds its memory: new evidence waits for it. A proposal the write refused keeps its review hold too, and takes the memory's place only when no earlier proposal holds it.
                            if applied.holds_memory()
                                && let ReviewTarget::Memory { object_id, .. } = &job.target
                            {
                                self.switched
                                    .selection
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .note_pending(
                                        &route.project,
                                        object_id,
                                        &job.causal_identity,
                                        matches!(applied, Applied::Refused(_)),
                                    );
                            }
                            match &applied {
                                Applied::Left => eprintln!(
                                    "daemon: memory_reviewer left a change proposal for a reader: job={}",
                                    &job.causal_identity[..16.min(job.causal_identity.len())]
                                ),
                                // A proposal that keeps its memory is accepted in every mode and changes nothing worth a line.
                                Applied::Accepted(settlement)
                                    if settlement.effect == kernel::SettlementEffect::Unchanged => {
                                }
                                Applied::Refused(reason) => eprintln!(
                                    "daemon: memory_reviewer left a published proposal unapplied: job={} {reason}",
                                    &job.causal_identity[..16.min(job.causal_identity.len())]
                                ),
                                applied => eprintln!(
                                    "daemon: memory_reviewer applied a proposal: job={} {}",
                                    &job.causal_identity[..16.min(job.causal_identity.len())],
                                    applied.kind()
                                ),
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(InvestigationError::Capacity) => break,
                    Err(InvestigationError::Rejected(rejection)) => {
                        self.latch(rejection);
                        return settled;
                    }
                    Err(InvestigationError::Cancelled) => return settled,
                    Err(error) => {
                        eprintln!("daemon: memory_reviewer run ended without settlement: {error}");
                    }
                }
            }
        }
        settled
    }

    /// The pass over projects the owner's activation record admits: nothing runs unless the gate is open; then every Ready job of every project whose authority the owner recorded is claimed and investigated, except a review of a stored memory, which the switched pass runs. Returns how many runs settled. Store and filesystem work runs on the blocking pool; only the investigation itself is awaited on the runtime. A latched rejection closes the gate before the record is read, and every pass stamps it again.
    async fn recorded_pass(
        self: &Arc<Self>,
        routes: &[ProjectRoute],
        cancel: &CancellationToken,
    ) -> usize {
        if self.rejection.get().is_some() {
            self.stamp_latched();
            return 0;
        }
        let Some(kernel) = (self.kernel)() else {
            self.status.set_activation(ActivationState::Closed("store"));
            return 0;
        };
        let Some((activation, provider, credential)) = self.recorded_gate(&kernel).await else {
            return 0;
        };
        let coordinator = Coordinator {
            store: Arc::clone(&kernel),
            ledger: Arc::clone(&self.store),
            supervisor: Arc::clone(&self.host.supervisor),
            sender: Arc::new(Sender::new(provider, credential)),
            approval: Some(activation.approval.clone()),
            profile: ModelProfile {
                model: activation.approval.model.clone(),
                max_tokens: MAX_TOKENS,
                temperature: None,
            },
            credential_id: activation.approval.credential_id.clone(),
            permits: Arc::clone(&self.permits),
            now_ms: Arc::new(crate::now_ms),
            inspection_limit: MAX_ISSUED_INSPECTIONS,
            stage_one: None,
        };
        let mut settled = 0;
        for route in routes {
            if cancel.is_cancelled() {
                break;
            }
            // The record admits the bound projects whose authority the owner recorded. A project the switch gave its authority, and a root listed after its routes closed, are reviewed in the switched pass alone.
            if !route.recorded {
                continue;
            }
            let ready = {
                let store = Arc::clone(&self.store);
                let project = route.project.clone();
                tokio::task::spawn_blocking(move || {
                    store
                        .ready_memory_reviewer_jobs(
                            &project,
                            MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT,
                            crate::now_ms(),
                        )
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            };
            let mut ready = match ready {
                Ok(ready) => ready,
                Err(error) => {
                    eprintln!(
                        "daemon: memory_reviewer ready jobs for a project could not be listed: {error}"
                    );
                    continue;
                }
            };
            // A review of a stored memory is the switch's: it runs in the switched pass, through the host's model execution.
            ready.retain(|job| !reviews_memory(job));
            let route = Arc::new(route.clone());
            let mut runs = 0;
            for job in ready {
                if cancel.is_cancelled() || runs == JOBS_PER_PASS {
                    break;
                }
                // The owner can withdraw the record between jobs; a gate that closed or changed since the pass began admits no further run under the pass's approval.
                if self
                    .gate_off_runtime(&kernel)
                    .await
                    .map(|(current, _)| current)
                    .as_ref()
                    != Ok(&activation)
                {
                    return settled;
                }
                let outcome = self
                    .run_job(&coordinator, &kernel, &route, Arc::new(job), cancel)
                    .await;
                if !matches!(outcome, Ok(None)) {
                    runs += 1;
                }
                match outcome {
                    Ok(Some(_)) => settled += 1,
                    Ok(None) => {}
                    Err(InvestigationError::Capacity) => break,
                    Err(InvestigationError::Unavailable) => {
                        self.status
                            .set_activation(ActivationState::Closed("unavailable"));
                        return settled;
                    }
                    Err(InvestigationError::Rejected(rejection)) => {
                        self.latch(rejection);
                        return settled;
                    }
                    Err(InvestigationError::Cancelled) => return settled,
                    Err(error) => {
                        eprintln!("daemon: memory_reviewer run ended without settlement: {error}");
                    }
                }
            }
        }
        settled
    }

    /// Claims one Ready job under the authority's lease, begins or takes over its receipt, and investigates it, returning how the run settled. `Ok(None)` means another worker holds it, this worker's slot still holds another job's claim, it already settled, or no bound root holds its staged subject. A run that ends without settling leaves its receipt in progress; the next pass takes the receipt over under a fresh claim at the next generation, so a lease that lapsed under an abandoned run never runs again on its stale fence.
    async fn run_job(
        self: &Arc<Self>,
        coordinator: &Coordinator,
        kernel: &Arc<kernel::KernelStore>,
        route: &Arc<ProjectRoute>,
        job: Arc<MemoryReviewerJob>,
        cancel: &CancellationToken,
    ) -> Result<Option<Settled>, InvestigationError> {
        let MemoryReviewerJobState::Ready(input) = &job.state else {
            return Ok(None);
        };
        let kernel_incarnation = coordinator
            .approval
            .as_ref()
            .map(|approval| approval.kernel_incarnation.clone())
            .ok_or(InvestigationError::Unavailable)?;
        let prepared = {
            let worker = Arc::clone(self);
            let kernel = Arc::clone(kernel);
            let route = Arc::clone(route);
            let job = Arc::clone(&job);
            tokio::task::spawn_blocking(move || {
                worker.prepare_job(&kernel, &route, &job, &kernel_incarnation)
            })
            .await
            .map_err(|error| InvestigationError::Store(error.to_string()))??
        };
        let Some(mut prepared) = prepared else {
            return Ok(None);
        };
        let settled = coordinator
            .investigate(
                JobContext {
                    job: &job,
                    input,
                    receipt: &prepared.receipt,
                    claim: &TaskClaim {
                        claim_id: prepared.claim.claim_id.clone(),
                        worker_instance: self.host.worker_instance.clone(),
                        slot: 0,
                    },
                    binding: &prepared.binding,
                    // The job names the template it was reserved under; a name this build does not hold is read as the fact-set template.
                    question: QuestionTemplate::parse(&job.question_template)
                        .unwrap_or(QuestionTemplate::ExtractedFacts),
                    project_root: prepared.project_text.as_mut(),
                },
                cancel,
            )
            .await?;
        Ok(Some(settled))
    }

    /// The synchronous prefix of a run: the root the job is read under, the lease claim, the receipt, and the project inspection. `Ok(None)` is a job this pass leaves alone.
    fn prepare_job(
        &self,
        kernel: &kernel::KernelStore,
        route: &ProjectRoute,
        job: &MemoryReviewerJob,
        kernel_incarnation: &str,
    ) -> Result<Option<PreparedJob>, InvestigationError> {
        let project = route.project.as_str();
        let now = crate::now_ms();
        // Resolved before the claim: a job left unclaimed here stays Ready for its queue deadline instead of leaving a receipt in progress.
        let Some((root, binding)) = job_root(kernel, route, job, now) else {
            eprintln!(
                "daemon: memory_reviewer job {}/{} was staged under a root no bound route names; it is left for its queue deadline",
                project, job.causal_identity
            );
            return Ok(None);
        };
        let store_error =
            |error: &dyn std::fmt::Display| InvestigationError::Store(error.to_string());
        // The pass snapshotted routes once; the root's live mapping is read again here, before the claim, so a root rebound to another project since is not read or staged under the stale one. The lease fences the project's authority, not this mapping.
        let live = memories_authority_for_route(&self.store, &root.project_root.to_string_lossy())
            .map_err(|error| store_error(&error))?;
        if !matches!(&live, MemoriesAuthority::Module(authority) if authority.project == route.project && authority.generation == route.authority_generation)
        {
            eprintln!(
                "daemon: memory_reviewer job {}/{} is staged under a root that no longer resolves to its project at the pass's authority generation; it is left for the next pass",
                project, job.causal_identity
            );
            return Ok(None);
        }
        let acquisition_id = format!("memory_reviewer:{}:{now}", job.causal_identity);
        let claim = match self
            .store
            .acquire_memory_reviewer_task(
                project,
                &acquisition_id,
                &self.host.worker_instance,
                0,
                i64::try_from(route.authority_generation).unwrap_or(i64::MAX),
                &job.causal_identity,
                now,
            )
            .map_err(|error| store_error(&error))?
        {
            // The slot's live claim is rebound to the caller whatever job it names; a claim naming another job is that job's fence, not this one's.
            LeaseAcquireOutcome::Claim { claim, task, .. } if task == job.causal_identity => claim,
            _ => return Ok(None),
        };
        let receipt = match self
            .store
            .begin_memory_reviewer_receipt(
                project,
                &job.causal_identity,
                kernel_incarnation,
                &claim.claim_id,
                now,
            )
            .map_err(|error| store_error(&error))?
        {
            MemoryReviewerBeginOutcome::Begun(receipt) => receipt,
            MemoryReviewerBeginOutcome::InProgress(receipt)
                if receipt.claim_id == claim.claim_id =>
            {
                receipt
            }
            MemoryReviewerBeginOutcome::InProgress(receipt) => self
                .store
                .take_over_memory_reviewer_receipt(
                    project,
                    &job.causal_identity,
                    receipt.generation,
                    &claim.claim_id,
                    now,
                )
                .map_err(|error| store_error(&error))?,
            MemoryReviewerBeginOutcome::Complete(_) => return Ok(None),
        };
        // The inspection serves the run the coordinator binds its hold to: the same Kernel digest, incarnations, job, and generation.
        let hold = kernel::MemoryReviewerHoldBinding {
            project_digest: binding.project_digest.clone(),
            kernel_incarnation: receipt.kernel_incarnation_id.clone(),
            memstore_incarnation: self
                .store
                .memory_reviewer_store_incarnation()
                .map_err(|error| store_error(&error))?,
            subject: job.causal_identity.clone(),
            generation: receipt.generation,
        };
        // A fact-set run may inspect the project's working tree. A memory review reads stored memory and conversation text alone, so it opens no inspection.
        let project_text = (!reviews_memory(job))
            .then(|| ProtectedLocations::new([self.home.clone()]).ok())
            .flatten()
            .and_then(|protected| {
                ProjectText::open(
                    &root.project_root,
                    &protected,
                    InspectionBinding {
                        hold,
                        domain_id: MEMORY_DOMAIN_ID.to_string(),
                        scope_id: Some(root.scope_id.clone()),
                        retain_until: receipt.execution_cutoff_ms,
                    },
                )
                .ok()
            });
        Ok(Some(PreparedJob {
            binding,
            claim,
            receipt,
            project_text,
        }))
    }
}

struct PreparedJob {
    binding: ReviewBinding,
    claim: LeaseClaim,
    receipt: MemoryReviewerReceipt,
    project_text: Option<ProjectText>,
}

/// The binding a memory-target job is read under; a staged subject carries its own in the Kernel row.
fn memory_binding(project_digest: &str, job: &MemoryReviewerJob) -> Option<ReviewBinding> {
    let ReviewTarget::Memory {
        object_id,
        source_revision,
    } = &job.target
    else {
        return None;
    };
    Some(ReviewBinding {
        project_digest: project_digest.to_string(),
        domain_id: MEMORY_DOMAIN_ID.to_string(),
        owner: ReviewOwner::Job {
            job_id: job.causal_identity.clone(),
        },
        subject_source: SourceDependency {
            source_kind: "canonical_memory".to_string(),
            source_id: object_id.clone(),
            source_revision: *source_revision,
        },
        reference_sources: Vec::new(),
    })
}

/// The root a job is read under, with its binding. A staged subject's binding is the one its stager wrote, read back from the Kernel row: the job row carries neither the session nor the scope it was staged under, so nothing is reconstructed. The root is the bound one whose digest the binding names, and the binding must belong to this job. Memory targets use the newest root. `None` when no bound root owns the subject.
fn job_root<'a>(
    kernel: &kernel::KernelStore,
    route: &'a ProjectRoute,
    job: &MemoryReviewerJob,
    now: i64,
) -> Option<(&'a RootScope, ReviewBinding)> {
    let reference = match &job.target {
        ReviewTarget::StagedSubject {
            kernel_incarnation,
            candidate_id,
            payload_digest,
        } => ReviewStagedReference {
            database_incarnation_id: kernel_incarnation.clone(),
            candidate_id: candidate_id.clone(),
            payload_digest: payload_digest.clone(),
        },
        ReviewTarget::Memory { .. } => {
            let root = route.roots.first()?;
            return Some((root, memory_binding(&root.project_digest, job)?));
        }
    };
    let binding = kernel.staged_review_binding(&reference, now).ok()?;
    if binding.domain_id != MEMORY_DOMAIN_ID
        || !matches!(&binding.owner, ReviewOwner::Job { job_id } if *job_id == job.causal_identity)
    {
        return None;
    }
    let root = route
        .roots
        .iter()
        .find(|root| root.bound && root.project_digest == binding.project_digest)?;
    Some((root, binding))
}

/// Runs passes until cancelled: a pass that settled a run is followed at once, an idle one after [`IDLE_INTERVAL`].
pub async fn run(worker: Arc<Worker>, cancel: CancellationToken) {
    loop {
        let settled = worker.pass(&cancel).await;
        if cancel.is_cancelled() {
            return;
        }
        if settled > 0 {
            continue;
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(IDLE_INTERVAL) => {}
        }
    }
}

impl Worker {
    pub(crate) fn home_of(store_path: &str) -> Option<PathBuf> {
        crate::sqlite_store_data_home(store_path).map(|home| Path::new(home).to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch(stage1: &str, derived: bool) -> ReviewSwitch {
        ReviewSwitch {
            model_ref: "prov/claude-opus-9".to_string(),
            stage1_model: Stage1Model {
                reference: stage1.to_string(),
                derived,
            },
            apply: ReviewApply::Contest,
            classifier: None,
            review: crate::config::ReviewScope::Memory,
            memory_budget_bits: crate::config::DEFAULT_MEMORY_BUDGET_TOKENS.to_bits(),
            record_sentences: true,
            harness: "pi".to_string(),
            credential_fingerprints: BTreeMap::new(),
        }
    }

    /// A derived default that fails before it has answered is replaced by the review model for the process; one that has answered keeps its place through later failures, and a model the user named is never replaced.
    #[test]
    fn a_derived_default_is_dropped_only_on_a_failure_before_any_answer() {
        let derived = switch("prov/claude-haiku-9", true);
        let mut defaults = DerivedDefaults::default();
        assert_eq!(defaults.model(&derived), "prov/claude-haiku-9");
        assert!(
            !defaults.note(&derived, false, false),
            "a job without a stage-1 request"
        );
        assert!(
            defaults.note(&derived, false, true),
            "the first failure refuses it"
        );
        assert_eq!(defaults.model(&derived), "prov/claude-opus-9");
        assert!(!defaults.note(&derived, false, true), "reported once");
        assert_eq!(defaults.model(&derived), "prov/claude-opus-9");

        let mut defaults = DerivedDefaults::default();
        assert!(!defaults.note(&derived, true, false));
        assert!(
            !defaults.note(&derived, false, true),
            "an outage after an answer"
        );
        assert_eq!(defaults.model(&derived), "prov/claude-haiku-9");

        let named = switch("prov/small", false);
        let mut defaults = DerivedDefaults::default();
        assert!(!defaults.note(&named, false, true));
        assert_eq!(defaults.model(&named), "prov/small");
    }
}
