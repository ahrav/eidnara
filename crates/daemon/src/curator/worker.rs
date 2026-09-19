//! The production Curator worker: the one place Ready review jobs are claimed and run. Each pass reads the deployment owner's activation record anew; a closed gate runs nothing and sends nothing. With the gate open, the worker walks every route whose memories authority is MODULE, claims each Ready job under the authority's lease, begins or resumes its receipt, and runs the coordinator's investigation, which settles the run through its completed receipt. Both producers' jobs, History Summarizer subjects and Memory Classifier selections, enter here alike. Cancellation ends the pass at the coordinator's next check and the in-flight run is joined before the task returns, so shutdown observes physical completion before the stores are released.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use host_runtime::model_execution::subprocess::{CREDENTIAL_VALUE_CAP_BYTES, CREDENTIAL_VARIABLES};
use host_runtime::model_execution::supervisor::Supervisor;
use kernel::{
    ReviewBinding, ReviewOwner, ReviewReadError, ReviewReadRefusal, ReviewStagedReference,
    SourceDependency,
};
use memory_store::curator_jobs::{
    CuratorJob, CuratorJobState, MAX_PENDING_CURATOR_JOBS_PER_PROJECT, ReviewTarget,
};
use memory_store::curator_ledger::{CuratorBeginOutcome, CuratorReceipt};
use memory_store::{LeaseAcquireOutcome, LeaseClaim, MemoryStore};
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
/// Runs per project per pass. Ready jobs the pass skips (no bound root owns the subject, another worker holds it, already settled) do not count, so a deadline-ordered prefix of them cannot starve the jobs behind it.
pub const JOBS_PER_PASS: usize = 8;
/// Output budget and sampling every Curator request carries; the model id comes from the activation record.
pub const MAX_TOKENS: u32 = 4096;
/// Credential variables the host may hand the daemon: every name the startup envelope admits.
pub const MAX_CREDENTIALS: usize = CREDENTIAL_VARIABLES.len();
/// Bytes [`CuratorHost`] retains for the process lifetime: one value under [`CREDENTIAL_VALUE_CAP_BYTES`] per credential, its name, and its keyed identity. Declared under the daemon's retained-resident bytes.
pub const RETAINED_CREDENTIAL_BYTES: u64 =
    MAX_CREDENTIALS as u64 * (CREDENTIAL_VALUE_CAP_BYTES as u64 + 256);

pub struct CuratorHost {
    pub supervisor: Arc<Supervisor>,
    pub credentials: BTreeMap<String, Zeroizing<String>>,
    /// Credential identities keyed under the incarnation key; unset until the key exists.
    pub credential_identities: OnceLock<BTreeMap<String, String>>,
    pub worker_instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoute {
    pub project: String,
    pub authority_generation: u64,
    /// Newest binding first.
    pub roots: Vec<RootScope>,
}

/// One bound root and the Kernel scope derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootScope {
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

/// MODULE-authority bound routes grouped by authority project, newest binding first within each project's roots.
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
                    roots: Vec::new(),
                })
                .roots
                .push(RootScope {
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
        let probe = Sender::new(
            self.endpoint.clone(),
            Credential::new("probe".into(), "probe".into()).ok()?,
        );
        Some(LiveIdentity {
            kernel_baseline_digest: kernel::kernel_baseline_digest().ok()?.to_string(),
            memstore_baseline_digest: memory_store::baseline_digest(),
            kernel_incarnation,
            memstore_incarnation,
            provider: probe.provider_identity(),
            // Only the credential the sender's protocol carries can vouch for this provider; a record naming another provider's secret reads as an unknown credential.
            credentials: self
                .host
                .credential_identities
                .get()?
                .iter()
                .filter(|(name, _)| {
                    *name == super::model_request::CREDENTIAL_NAME
                        && self.host.credentials.contains_key(*name)
                })
                .map(|(name, identity)| (name.clone(), identity.clone()))
                .collect(),
        })
    }

    /// The gate as of now, with the closed state published for the operator surface.
    fn gate(&self, kernel: &kernel::KernelStore) -> Result<Activation, Closed> {
        let gate = self
            .live_identity(kernel)
            .ok_or_else(|| Closed::Unreadable("store identity".to_string()))
            .and_then(|live| activation::read_gate(&self.home, &live));
        self.status.set_activation(match &gate {
            Ok(_) => ActivationState::Open,
            Err(closed) => ActivationState::from(closed),
        });
        gate
    }

    /// `gate` on the blocking pool: it reads the record from disk and queries both stores.
    async fn gate_off_runtime(
        self: &Arc<Self>,
        kernel: &Arc<kernel::KernelStore>,
    ) -> Result<Activation, Closed> {
        let worker = Arc::clone(self);
        let kernel = Arc::clone(kernel);
        tokio::task::spawn_blocking(move || worker.gate(&kernel))
            .await
            .unwrap_or_else(|_| Err(Closed::Unreadable("gate evaluation".to_string())))
    }

    /// One pass: nothing runs unless the gate is open; then every Ready job of every MODULE project is claimed and investigated. Returns how many runs settled. Store and filesystem work runs on the blocking pool; only the investigation itself is awaited on the runtime.
    pub async fn pass(self: &Arc<Self>, cancel: &CancellationToken) -> usize {
        let Some(kernel) = (self.kernel)() else {
            self.status.set_activation(ActivationState::Closed("store"));
            return 0;
        };
        let activation = match self.gate_off_runtime(&kernel).await {
            Ok(activation) => activation,
            Err(_) => return 0,
        };
        let Some(secret) = self.host.credentials.get(&activation.credential) else {
            self.status
                .set_activation(ActivationState::Closed("unknown_credential"));
            return 0;
        };
        let Ok(credential) = Credential::new(
            activation.approval.credential_id.clone(),
            secret.to_string(),
        ) else {
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
            let ready = {
                let store = Arc::clone(&self.store);
                let project = route.project.clone();
                tokio::task::spawn_blocking(move || {
                    store
                        .ready_curator_jobs(
                            &project,
                            MAX_PENDING_CURATOR_JOBS_PER_PROJECT,
                            crate::now_ms(),
                        )
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            };
            let ready = match ready {
                Ok(ready) => ready,
                Err(error) => {
                    eprintln!(
                        "daemon: curator ready jobs for a project could not be listed: {error}"
                    );
                    continue;
                }
            };
            let route = Arc::new(route);
            let mut runs = 0;
            for job in ready {
                if cancel.is_cancelled() || runs == JOBS_PER_PASS {
                    break;
                }
                // The owner can withdraw the record between jobs; a gate that closed or changed since the pass began admits no further run under the pass's approval.
                if self.gate_off_runtime(&kernel).await.as_ref() != Ok(&activation) {
                    return settled;
                }
                let outcome = self
                    .run_job(&coordinator, &kernel, &route, Arc::new(job), cancel)
                    .await;
                if !matches!(outcome, Ok(false)) {
                    runs += 1;
                }
                match outcome {
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

    /// Claims one Ready job under the authority's lease, begins or takes over its receipt, and investigates it. `Ok(false)` means another worker holds it, this worker's slot still holds another job's claim, it already settled, or no bound root holds its staged subject. A run that ends without settling leaves its receipt in progress; the next pass takes the receipt over under a fresh claim at the next generation, so a lease that lapsed under an abandoned run never runs again on its stale fence.
    async fn run_job(
        self: &Arc<Self>,
        coordinator: &Coordinator,
        kernel: &Arc<kernel::KernelStore>,
        route: &Arc<ProjectRoute>,
        job: Arc<CuratorJob>,
        cancel: &CancellationToken,
    ) -> Result<bool, InvestigationError> {
        let CuratorJobState::Ready(input) = &job.state else {
            return Ok(false);
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
            return Ok(false);
        };
        coordinator
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
                    question: QuestionTemplate::ExtractedFacts,
                    project_root: prepared.project_text.as_mut(),
                },
                cancel,
            )
            .await?;
        Ok(true)
    }

    /// The synchronous prefix of a run: the root the job is read under, the lease claim, the receipt, and the project inspection. `Ok(None)` is a job this pass leaves alone.
    fn prepare_job(
        &self,
        kernel: &kernel::KernelStore,
        route: &ProjectRoute,
        job: &CuratorJob,
        kernel_incarnation: &str,
    ) -> Result<Option<PreparedJob>, InvestigationError> {
        let project = route.project.as_str();
        let now = crate::now_ms();
        // Resolved before the claim: a job left unclaimed here stays Ready for its queue deadline instead of leaving a receipt in progress.
        let Some((root, binding)) = job_root(kernel, route, job, now) else {
            eprintln!(
                "daemon: curator job {}/{} was staged under a root no bound route names; it is left for its queue deadline",
                project, job.causal_identity
            );
            return Ok(None);
        };
        let store_error =
            |error: &dyn std::fmt::Display| InvestigationError::Store(error.to_string());
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
            .map_err(|error| store_error(&error))?
        {
            // The slot's live claim is rebound to the caller whatever job it names; a claim naming another job is that job's fence, not this one's.
            LeaseAcquireOutcome::Claim { claim, task, .. } if task == job.causal_identity => claim,
            _ => return Ok(None),
        };
        let receipt = match self
            .store
            .begin_curator_receipt(
                project,
                &job.causal_identity,
                kernel_incarnation,
                &claim.claim_id,
                now,
            )
            .map_err(|error| store_error(&error))?
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
                .map_err(|error| store_error(&error))?,
            CuratorBeginOutcome::Complete(_) => return Ok(None),
        };
        // The inspection serves the run the coordinator binds its hold to: the same Kernel digest, incarnations, job, and generation.
        let hold = kernel::CuratorHoldBinding {
            project_digest: binding.project_digest.clone(),
            kernel_incarnation: receipt.kernel_incarnation_id.clone(),
            memstore_incarnation: self
                .store
                .curator_store_incarnation()
                .map_err(|error| store_error(&error))?,
            subject: job.causal_identity.clone(),
            generation: receipt.generation,
        };
        let project_text =
            ProtectedLocations::new([self.home.clone()])
                .ok()
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
    receipt: CuratorReceipt,
    project_text: Option<ProjectText>,
}

/// Only `HISTORY_SUMMARIZER` has a defined staged-subject binding; other producers return `None`.
pub fn job_binding(project_digest: &str, job: &CuratorJob) -> Option<ReviewBinding> {
    match &job.target {
        ReviewTarget::StagedSubject { .. } if job.producer.producer == HISTORY_SUMMARIZER => {
            let (session_id, firing_seq) = job
                .producer
                .firing_id
                .rsplit_once('#')
                .and_then(|(session, seq)| Some((session, seq.parse::<u64>().ok()?)))
                .unwrap_or((job.producer.firing_id.as_str(), 0));
            Some(review_binding(
                project_digest,
                MEMORY_DOMAIN_ID,
                session_id,
                firing_seq,
                &job.causal_identity,
            ))
        }
        ReviewTarget::StagedSubject { .. } => None,
        ReviewTarget::Memory {
            object_id,
            source_revision,
        } => Some(ReviewBinding {
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
        }),
    }
}

/// The root a job is read under, with its binding. A `ScopeMismatch` means the root does not own the staged subject, so the next root is tried; any other outcome is that root's to settle. Memory targets use the newest root. `None` when no root owns the staged subject.
fn job_root<'a>(
    kernel: &kernel::KernelStore,
    route: &'a ProjectRoute,
    job: &CuratorJob,
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
            return Some((root, job_binding(&root.project_digest, job)?));
        }
    };
    for root in &route.roots {
        let binding = job_binding(&root.project_digest, job)?;
        match kernel.read_review_input(&reference, &binding, now) {
            Err(ReviewReadError::Refused(ReviewReadRefusal::ScopeMismatch)) => continue,
            Ok(_) | Err(_) => return Some((root, binding)),
        }
    }
    None
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
    use memory_store::curator_jobs::ProducerBinding;

    use super::*;

    fn staged_job(producer: &str) -> CuratorJob {
        CuratorJob {
            project: "git:proj".to_string(),
            causal_identity: "job-1".to_string(),
            producer: ProducerBinding {
                producer: producer.to_string(),
                firing_id: "ses#3".to_string(),
                ordinal: 1,
            },
            target: ReviewTarget::StagedSubject {
                kernel_incarnation: "k".repeat(64),
                candidate_id: "cand-1".to_string(),
                payload_digest: "0".repeat(64),
            },
            input_fingerprint: "1".repeat(64),
            question_template: "extracted_facts".to_string(),
            state: CuratorJobState::Reserved,
            queue_deadline_ms: 10,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    /// A staged subject is read under the binding its stager wrote, and only the History Summarizer's is known; another producer's job yields no binding rather than one the Kernel would refuse.
    #[test]
    fn only_a_history_summarizer_staged_subject_has_a_binding() {
        let digest = "a".repeat(64);
        let history = job_binding(&digest, &staged_job(HISTORY_SUMMARIZER)).unwrap();
        assert_eq!(
            history,
            review_binding(&digest, MEMORY_DOMAIN_ID, "ses", 3, "job-1")
        );
        assert_eq!(job_binding(&digest, &staged_job("other_producer")), None);
    }
}
