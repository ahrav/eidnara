//! The production Curator worker: the one place Ready review jobs are claimed and run. Each pass reads the deployment owner's activation record anew; a closed gate runs nothing and sends nothing. With the gate open, the worker walks every route whose memories authority is MODULE, claims each Ready job under the authority's lease, begins or resumes its receipt, and runs the coordinator's investigation, which settles the run through its completed receipt. Both producers' jobs, History Summarizer subjects and Memory Classifier selections, enter here alike. Cancellation ends the pass at the coordinator's next check and the in-flight run is joined before the task returns, so shutdown observes physical completion before the stores are released.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_runtime::model_execution::supervisor::Supervisor;
use kernel::{ReviewBinding, ReviewOwner, SourceDependency};
use memory_store::curator_jobs::{CuratorJob, CuratorJobState, ReviewTarget};
use memory_store::curator_ledger::CuratorBeginOutcome;
use memory_store::{LeaseAcquireOutcome, MemoryStore};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::canonical_memory::MEMORY_DOMAIN_ID;
use crate::kernel_routes::{KernelOpenCoordinator, KernelState};
use crate::{MemoriesAuthority, RouteBindings, SessionBinding, memories_authority_for_route};

use super::activation::{self, Activation, Closed, LiveIdentity};
use super::broker::{MAX_ISSUED_INSPECTIONS, QuestionTemplate};
use super::coordinator::{Coordinator, InvestigationError, InvestigationPermits, JobContext};
use super::disclosure::ModelProfile;
use super::handoff::{PRODUCER as HISTORY_SUMMARIZER, review_binding};
use super::lifecycle::{ActivationState, CuratorStatus};
use super::model_request::{Credential, Endpoint, Sender};
use super::project_text::{InspectionBinding, ProjectText, ProtectedLocations};
use super::settlement::TaskClaim;

/// Idle interval between passes when no job was run.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(30);
/// Ready jobs claimed per project per pass.
pub const JOBS_PER_PASS: usize = 8;
/// Output budget and sampling every Curator request carries; the model id comes from the activation record.
pub const MAX_TOKENS: u32 = 4096;
const CREDENTIAL_DOMAIN: &[u8] = b"eidnara-curator-credential-v1";

/// What the host hands the daemon for Curator runs: the Model Execution supervisor its internal launches run under, and the startup credentials by name.
pub struct CuratorHost {
    pub supervisor: Arc<Supervisor>,
    pub credentials: BTreeMap<String, Zeroizing<String>>,
    pub worker_instance: String,
}

/// The stable identity of one credential, bound into approvals and attempt receipts without the secret.
pub fn credential_fingerprint(name: &str, secret: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(CREDENTIAL_DOMAIN);
    hash.update(b"\0");
    hash.update(name.as_bytes());
    hash.update(b"\0");
    hash.update(secret.as_bytes());
    format!("{:x}", hash.finalize())
}

/// One MODULE-authority project as a bound route presents it: the ledger project the jobs live under, the authority generation the lease is keyed by, and the Kernel scope the run reads and stages under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoute {
    pub project: String,
    pub authority_generation: u64,
    pub project_root: PathBuf,
    pub project_digest: String,
    pub scope_id: String,
}

pub struct Worker {
    pub host: Arc<CuratorHost>,
    /// The data home the activation record lives under; also the location a project inspection may never read.
    pub home: PathBuf,
    pub store: Arc<MemoryStore>,
    /// The Kernel while it is ready; `None` runs nothing this pass.
    pub kernel: Arc<dyn Fn() -> Option<Arc<kernel::KernelStore>> + Send + Sync>,
    /// The projects whose Ready jobs this worker runs.
    pub projects: Arc<dyn Fn() -> Vec<ProjectRoute> + Send + Sync>,
    pub status: Arc<CuratorStatus>,
    pub permits: Arc<InvestigationPermits>,
    pub endpoint: Endpoint,
}

/// The production project source: every bound route whose memories authority is MODULE, one entry per authority project. The newest root speaks for a project, the rule the scheduler applies to the same map, so the digest jobs are read and staged under is the one the scheduler selects under.
pub(crate) fn module_projects(
    store: &MemoryStore,
    bindings: &Mutex<RouteBindings>,
) -> Vec<ProjectRoute> {
    let mut roots: Vec<(u64, SessionBinding)> = bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .latest_per_root()
        .into_values()
        .map(|(seq, binding)| (seq, binding.clone()))
        .collect();
    roots.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    let mut projects: BTreeMap<String, ProjectRoute> = BTreeMap::new();
    for (_, binding) in roots {
        let root = binding.project_root.to_string_lossy().to_string();
        if let Ok(MemoriesAuthority::Module(authority)) = memories_authority_for_route(store, &root)
        {
            projects
                .entry(authority.project.clone())
                .or_insert(ProjectRoute {
                    project: authority.project,
                    authority_generation: authority.generation,
                    project_root: binding.project_root.clone(),
                    project_digest: binding.kernel_project.digest().to_string(),
                    scope_id: binding.kernel_project.scope_id(),
                });
        }
    }
    projects.into_values().collect()
}

/// The Kernel store while the coordinator reports it ready.
pub(crate) fn ready_kernel(kernel: &KernelOpenCoordinator) -> Option<Arc<kernel::KernelStore>> {
    (kernel.state() == KernelState::Ready)
        .then(|| kernel.kernel_store().ok())
        .flatten()
}

impl Worker {
    fn live_identity(&self, kernel: &kernel::KernelStore) -> Option<LiveIdentity> {
        let budget = kernel::applicability::EvalBudget::new(
            Some(std::time::Instant::now() + Duration::from_secs(5)),
            Arc::default(),
        );
        let kernel_incarnation = kernel.database_incarnation_id_within_budget(&budget).ok()?;
        let memstore_incarnation = self.store.curator_store_incarnation().ok()?;
        let probe = Sender::new(self.endpoint.clone(), Credential::new("probe".into()).ok()?);
        Some(LiveIdentity {
            kernel_baseline_digest: kernel::kernel_baseline_digest().ok()?.to_string(),
            memstore_baseline_digest: memory_store::baseline_digest(),
            kernel_incarnation,
            memstore_incarnation,
            provider: probe.provider_identity(),
            credentials: self
                .host
                .credentials
                .iter()
                .map(|(name, secret)| (name.clone(), credential_fingerprint(name, secret)))
                .collect(),
        })
    }

    /// The gate as of now, with the closed state published for the operator surface.
    fn gate(&self, kernel: &kernel::KernelStore) -> Result<Activation, Closed> {
        let live = self
            .live_identity(kernel)
            .ok_or_else(|| Closed::Unreadable("store identity".to_string()))?;
        let gate = activation::read_gate(&self.home, &live);
        self.status.set_activation(match &gate {
            Ok(_) => ActivationState::Open,
            Err(closed) => ActivationState::from(closed),
        });
        gate
    }

    /// One pass: nothing runs unless the gate is open; then every Ready job of every MODULE project is claimed and investigated. Returns how many runs settled.
    pub async fn pass(&self, cancel: &CancellationToken) -> usize {
        let Some(kernel) = (self.kernel)() else {
            self.status.set_activation(ActivationState::Closed("store"));
            return 0;
        };
        let activation = match self.gate(&kernel) {
            Ok(activation) => activation,
            Err(_) => return 0,
        };
        let Some(secret) = self.host.credentials.get(&activation.credential) else {
            self.status
                .set_activation(ActivationState::Closed("unknown_credential"));
            return 0;
        };
        let Ok(credential) = Credential::new(secret.to_string()) else {
            self.status
                .set_activation(ActivationState::Closed("unknown_credential"));
            return 0;
        };
        let coordinator = Coordinator {
            store: Arc::clone(&kernel),
            ledger: Arc::clone(&self.store),
            supervisor: Arc::clone(&self.host.supervisor),
            sender: Arc::new(Sender::new(self.endpoint.clone(), credential)),
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
        };
        let mut settled = 0;
        for route in (self.projects)() {
            if cancel.is_cancelled() {
                break;
            }
            let ready = match self.store.ready_curator_jobs(&route.project, JOBS_PER_PASS) {
                Ok(ready) => ready,
                Err(error) => {
                    eprintln!(
                        "daemon: curator ready jobs for a project could not be listed: {error}"
                    );
                    continue;
                }
            };
            for job in ready {
                if cancel.is_cancelled() {
                    break;
                }
                // The owner can withdraw the record between jobs; a gate that closed or changed since the pass began admits no further run under the pass's approval.
                if self.gate(&kernel).as_ref() != Ok(&activation) {
                    return settled;
                }
                match self.run_job(&coordinator, &route, &job, cancel).await {
                    Ok(true) => settled += 1,
                    Ok(false) => {}
                    Err(InvestigationError::Capacity) => break,
                    Err(InvestigationError::Unavailable) => {
                        self.status
                            .set_activation(ActivationState::Closed("unavailable"));
                        return settled;
                    }
                    Err(InvestigationError::Cancelled) => return settled,
                    Err(error) => {
                        eprintln!("daemon: curator run ended without settlement: {error}");
                    }
                }
            }
        }
        settled
    }

    /// Claims one Ready job under the authority's lease, begins or takes over its receipt, and investigates it. `Ok(false)` means another worker holds it, this worker's slot still holds another job's claim, or it already settled. A run that ends without settling leaves its receipt in progress; the next pass takes the receipt over under a fresh claim at the next generation, so a lease that lapsed under an abandoned run never runs again on its stale fence.
    async fn run_job(
        &self,
        coordinator: &Coordinator,
        route: &ProjectRoute,
        job: &CuratorJob,
        cancel: &CancellationToken,
    ) -> Result<bool, InvestigationError> {
        let CuratorJobState::Ready(input) = &job.state else {
            return Ok(false);
        };
        let project = route.project.as_str();
        let now = crate::now_ms();
        let store_error =
            |error: memory_store::MemoryStoreError| InvestigationError::Store(error.to_string());
        let acquisition_id = format!("curator:{}:{now}", job.causal_identity);
        let claim = match self
            .store
            .acquire_curator_task(
                project,
                &acquisition_id,
                &self.host.worker_instance,
                0,
                i64::try_from(route.authority_generation).unwrap_or(i64::MAX),
                &job.causal_identity,
                now,
            )
            .map_err(store_error)?
        {
            // The slot's live claim is rebound to the caller whatever job it names; a claim naming another job is that job's fence, not this one's.
            LeaseAcquireOutcome::Claim { claim, task, .. } if task == job.causal_identity => claim,
            _ => return Ok(false),
        };
        let kernel_incarnation = coordinator
            .approval
            .as_ref()
            .map(|approval| approval.kernel_incarnation.clone())
            .ok_or(InvestigationError::Unavailable)?;
        let receipt = match self
            .store
            .begin_curator_receipt(
                project,
                &job.causal_identity,
                &kernel_incarnation,
                &claim.claim_id,
                now,
            )
            .map_err(|error| InvestigationError::Store(error.to_string()))?
        {
            CuratorBeginOutcome::Begun(receipt) => receipt,
            CuratorBeginOutcome::InProgress(receipt) if receipt.claim_id == claim.claim_id => {
                receipt
            }
            CuratorBeginOutcome::InProgress(receipt) => self
                .store
                .take_over_curator_receipt(
                    project,
                    &job.causal_identity,
                    receipt.generation,
                    &claim.claim_id,
                    now,
                )
                .map_err(|error| InvestigationError::Store(error.to_string()))?,
            CuratorBeginOutcome::Complete(_) => return Ok(false),
        };
        let review_binding = job_binding(&route.project_digest, job);
        let mut project_text =
            ProtectedLocations::new([self.home.clone()])
                .ok()
                .and_then(|protected| {
                    ProjectText::open(
                        &route.project_root,
                        &protected,
                        InspectionBinding {
                            domain_id: MEMORY_DOMAIN_ID.to_string(),
                            scope_id: Some(route.scope_id.clone()),
                            retain_until: receipt.execution_cutoff_ms,
                        },
                    )
                    .ok()
                });
        coordinator
            .investigate(
                JobContext {
                    job,
                    input,
                    receipt: &receipt,
                    claim: &TaskClaim {
                        claim_id: claim.claim_id.clone(),
                        worker_instance: self.host.worker_instance.clone(),
                        slot: 0,
                    },
                    binding: &review_binding,
                    question: QuestionTemplate::ExtractedFacts,
                    project_root: project_text.as_mut(),
                },
                cancel,
            )
            .await?;
        Ok(true)
    }
}

/// The binding a job's subject is read and its proposals are staged under. A History Summarizer subject cites the session's chunk at the firing that produced it; a Memory Classifier target cites the canonical memory at its revision.
pub fn job_binding(project_digest: &str, job: &CuratorJob) -> ReviewBinding {
    match &job.target {
        ReviewTarget::StagedSubject { .. } if job.producer.producer == HISTORY_SUMMARIZER => {
            let (session_id, firing_seq) = job
                .producer
                .firing_id
                .rsplit_once('#')
                .and_then(|(session, seq)| Some((session, seq.parse::<u64>().ok()?)))
                .unwrap_or((job.producer.firing_id.as_str(), 0));
            review_binding(
                project_digest,
                MEMORY_DOMAIN_ID,
                session_id,
                firing_seq,
                &job.causal_identity,
            )
        }
        ReviewTarget::StagedSubject { candidate_id, .. } => ReviewBinding {
            project_digest: project_digest.to_string(),
            domain_id: MEMORY_DOMAIN_ID.to_string(),
            owner: ReviewOwner::Job {
                job_id: job.causal_identity.clone(),
            },
            subject_source: SourceDependency {
                source_kind: "staged_subject".to_string(),
                source_id: candidate_id.clone(),
                source_revision: 0,
            },
            reference_sources: Vec::new(),
        },
        ReviewTarget::Memory {
            object_id,
            source_revision,
        } => ReviewBinding {
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
        },
    }
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
