//! The production Curator worker against a real Kernel, Memory Store, and scripted TLS peer: a closed activation gate runs nothing and sends nothing; an open gate claims a History Summarizer job under the authority's lease and runs it through the coordinator, where the default-Sensitive staged subject reaches explicit policy-blocked abstention with zero provider connections and no canonical write; a settled job is not run again.

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use daemon::curator::activation::{ACTIVATION_DIR, IDENTITY_RECORD, IDENTITY_SCHEMA, LiveIdentity};
use daemon::curator::coordinator::InvestigationPermits;
use daemon::curator::handoff::review_binding;
use daemon::curator::lifecycle::{ActivationState, CuratorStatus};
use daemon::curator::model_request::Endpoint;
use daemon::curator::steps::STEP_VERSION;
use daemon::curator::worker::{CuratorHost, ProjectRoute, RootScope, Worker};
use host_runtime::model_execution::backend::LlmExecutionBackend;
use host_runtime::model_execution::supervisor::Supervisor;
use kernel::KernelStore;
use memory_store::MemoryStore;
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, CuratorJobOutcome, CuratorJobState, ProducerBinding,
    ReserveOutcome, ReviewTarget,
};
use memory_store::curator_ledger::{AbstainReason, CuratorReceiptTerminal};
use support::tls_peer::Peer;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const PROJECT: &str = "git:proj";
const PROJECT_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CREDENTIAL: &str = "ANTHROPIC_API_KEY";
const SECRET: &str = "sk-test-credential";
/// The keyed identity the host's selection file would record for `CREDENTIAL`.
const CREDENTIAL_IDENTITY: &str = "hmac-of-credential-under-the-incarnation-key";

/// Internal launches need a backend only for public runs; the Curator's sender speaks to the peer itself.
struct NoPublicModel;

impl LlmExecutionBackend for NoPublicModel {
    fn execute(
        &self,
        _request: host_runtime::model_execution::backend::BackendRequest,
        _events: host_runtime::model_execution::backend::EventSink,
        _cancel: CancellationToken,
    ) -> host_runtime::model_execution::backend::BackendFuture {
        Box::pin(async { unreachable!("no public run is started") })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

struct Rig {
    _dirs: Vec<tempfile::TempDir>,
    home: std::path::PathBuf,
    kernel: Arc<KernelStore>,
    store: Arc<MemoryStore>,
    kernel_incarnation: String,
    generation: u64,
    status: Arc<CuratorStatus>,
    peer: Peer,
}

impl Rig {
    async fn open() -> Self {
        let home_dir = tempfile::tempdir().unwrap();
        let kernel_dir = tempfile::tempdir().unwrap();
        let kernel = Arc::new(KernelStore::open(kernel_dir.path()).unwrap());
        // The memory domain every production Kernel commits before a job can exist; holds pin the commit snapshot.
        kernel
            .commit(
                kernel::CommitIntent {
                    producer: "curator-worker-test".to_string(),
                    operation_key: "domain".to_string(),
                    request_digest: "0".repeat(64),
                    actor: "test".to_string(),
                    cause: "fixture".to_string(),
                },
                |envelope| {
                    envelope.insert_domain(kernel::DomainSpec {
                        domain_id: "memory".to_string(),
                        object_id: "domain-memory".to_string(),
                        name: "memory".to_string(),
                        source_kind: "fixture".to_string(),
                        source_id: "memory".to_string(),
                        source_revision: 1,
                        sensitivity: kernel::Sensitivity::Normal,
                    })?;
                    Ok("domain".to_string())
                },
            )
            .unwrap();
        let kernel_incarnation = kernel
            .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::new(
                None,
                Arc::default(),
            ))
            .unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            MemoryStore::open(&MemoryStore::test_descriptor(
                store_dir.path(),
                "eidnara-curator-worker-test",
            ))
            .unwrap(),
        );
        let generation = {
            let preparing = store
                .authority_begin_prepare("ctx", PROJECT, "memories")
                .unwrap();
            store
                .authority_finish_prepare(
                    "ctx",
                    PROJECT,
                    "memories",
                    preparing.generation,
                    "hash",
                    "hash",
                    true,
                )
                .unwrap()
                .generation
        };
        Rig {
            home: home_dir.path().to_path_buf(),
            _dirs: vec![home_dir, kernel_dir, store_dir],
            kernel,
            store,
            kernel_incarnation,
            generation,
            status: Arc::default(),
            peer: Peer::start().await,
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint::for_test("localhost", self.peer.port, self.peer.roots.clone()).unwrap()
    }

    fn worker(&self) -> Arc<Worker> {
        self.worker_with_roots(vec![self.root("project", PROJECT_DIGEST)])
    }

    /// Creates the root directory so a project inspection can open it.
    fn root(&self, name: &str, digest: &str) -> RootScope {
        let project_root = self.home.join(name);
        std::fs::create_dir_all(&project_root).unwrap();
        RootScope {
            project_root,
            project_digest: digest.to_string(),
            scope_id: format!("project:{digest}"),
        }
    }

    fn worker_with_roots(&self, roots: Vec<RootScope>) -> Arc<Worker> {
        let host = self.host();
        host.credential_identities
            .set(BTreeMap::from([(
                CREDENTIAL.to_string(),
                CREDENTIAL_IDENTITY.to_string(),
            )]))
            .unwrap();
        self.worker_for(host, roots)
    }

    fn host(&self) -> Arc<CuratorHost> {
        self.host_with(&[(CREDENTIAL, SECRET)])
    }

    fn host_with(&self, credentials: &[(&str, &str)]) -> Arc<CuratorHost> {
        Arc::new(CuratorHost {
            supervisor: Arc::new(Supervisor::new(Arc::new(NoPublicModel))),
            credentials: credentials
                .iter()
                .map(|(name, secret)| (name.to_string(), Zeroizing::new(secret.to_string())))
                .collect(),
            credential_identities: OnceLock::new(),
            worker_instance: "worker-a".to_string(),
        })
    }

    /// Binds every root to the project in the store, as the daemon does when a session binds, so the worker's dispatch-time route check finds the mapping its snapshot was taken from.
    fn worker_for(&self, host: Arc<CuratorHost>, roots: Vec<RootScope>) -> Arc<Worker> {
        for root in &roots {
            self.bind_route(&root.project_root, PROJECT);
        }
        let kernel = Arc::clone(&self.kernel);
        Arc::new(Worker {
            host,
            home: self.home.clone(),
            store: Arc::clone(&self.store),
            kernel: Arc::new(move || Some(Arc::clone(&kernel))),
            projects: {
                let generation = self.generation;
                Arc::new(move || {
                    vec![ProjectRoute {
                        project: PROJECT.to_string(),
                        authority_generation: generation,
                        roots: roots.clone(),
                    }]
                })
            },
            status: Arc::clone(&self.status),
            permits: Arc::new(InvestigationPermits::default()),
            endpoint: self.endpoint(),
        })
    }

    fn bind_route(&self, root: &std::path::Path, project: &str) {
        self.store
            .bind_authority_route("ctx", project, root.to_str().unwrap())
            .unwrap();
    }

    /// The owner's activation record for exactly this deployment.
    fn write_activation(&self) {
        self.write_activation_naming(CREDENTIAL, CREDENTIAL_IDENTITY);
    }

    fn write_activation_naming(&self, credential: &str, credential_fingerprint: &str) {
        let provider = self.peer.sender().provider_identity();
        let record = serde_json::json!({
            "schema": IDENTITY_SCHEMA,
            "model": "claude-test",
            "prompt_template_version": LiveIdentity::prompt_template_version(),
            "step_schema_version": STEP_VERSION,
            "scanner_ruleset_version": LiveIdentity::scanner_ruleset_version(),
            "egress_policy_version": context_core::curator_policy_union::CURATOR_POLICY_UNION_VERSION,
            "kernel_baseline_digest": kernel::kernel_baseline_digest().unwrap(),
            "memstore_baseline_digest": memory_store::baseline_digest(),
            "kernel_incarnation": self.kernel_incarnation,
            "memstore_incarnation": self.store.curator_store_incarnation().unwrap(),
            "provider": provider,
            "credential": credential,
            "credential_fingerprint": credential_fingerprint,
            "provider_retention": {
                "attested_by": "deployment owner",
                "attested_on": "2026-09-18",
                "retention_terms": "test peer; nothing leaves the host",
                "finite_work_exposure_acknowledged": true
            }
        });
        let dir = self.home.join(ACTIVATION_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join(IDENTITY_RECORD);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// A History Summarizer job as the handoff leaves it: a sealed staged subject, a reservation under the producer's identity, and activation with reference-only input.
    fn ready_history_summarizer_job(&self, now: i64) -> String {
        self.ready_history_summarizer_job_under(now, PROJECT_DIGEST)
    }

    fn ready_history_summarizer_job_under(&self, now: i64, project_digest: &str) -> String {
        self.ready_history_summarizer_job_tagged(now, project_digest, "ses-3")
    }

    /// `tag` distinguishes the staged payload and its staging run, so several jobs can coexist.
    fn ready_history_summarizer_job_tagged(
        &self,
        now: i64,
        project_digest: &str,
        tag: &str,
    ) -> String {
        let producer = ProducerBinding {
            producer: "history_summarizer".to_string(),
            // As the handoff records it: the hashed handoff key and the firing sequence, with the chunk ordinal the subject was staged under.
            firing_id: format!("{}#3", "5".repeat(32)),
            ordinal: 2,
        };
        let payload = kernel::ReviewPayload::Subject(kernel::ReviewSubject {
            facts: vec![kernel::ExtractedFact {
                text: format!("bun builds the workspace {tag}"),
                spans: vec![kernel::SourceSpan {
                    alias: "s1".to_string(),
                    start: 0,
                    end: 4,
                }],
            }],
            origins: vec![kernel::SubjectOrigin {
                alias: "s1".to_string(),
                message_id: "m2".to_string(),
                ordinal: 2,
                block_ids: vec!["m2#0".to_string()],
                block_hashes: vec!["0".repeat(64)],
                ranges: vec![kernel::ByteRange { start: 0, end: 4 }],
            }],
        });
        let payload_digest = payload.digest().unwrap();
        let candidate_id = format!("hs-ses-{}", &payload_digest[..32]);
        let inputs = CausalInputs {
            target: ReviewTarget::StagedSubject {
                kernel_incarnation: self.kernel_incarnation.clone(),
                candidate_id: candidate_id.clone(),
                payload_digest: payload_digest.clone(),
            },
            question_template: "extracted_facts".to_string(),
            signals: Vec::new(),
            required_evidence: Vec::new(),
            policy_versions: daemon::curator::handoff::review_policy_versions(),
        };
        let ReserveOutcome::Reserved(job) = self
            .store
            .reserve_curator_job(PROJECT, &producer, &inputs, now)
            .unwrap()
        else {
            panic!("fresh inputs reserve")
        };
        self.kernel
            .stage_review_input(kernel::ReviewStagingSpec {
                extraction_run_id: format!("hs-run-{tag}"),
                candidate_id,
                producer: "history_summarizer".to_string(),
                // The handoff stages under the real session id and the chunk ordinal; neither appears in the job row.
                binding: review_binding(
                    project_digest,
                    "memory",
                    "session-1",
                    2,
                    &job.causal_identity,
                ),
                payload,
                recorded_at: now,
                queue_deadline_at: job.queue_deadline_ms,
            })
            .unwrap();
        self.kernel
            .finish_staging_run(
                &format!("hs-run-{tag}"),
                kernel::StagingTerminalState::Completed,
                now,
            )
            .unwrap();
        self.store
            .activate_curator_job(
                PROJECT,
                &job.causal_identity,
                &producer,
                &CuratorJobInput {
                    subject: job.target.clone(),
                    starting_references: Vec::new(),
                    question_template: "extracted_facts".to_string(),
                },
                now,
            )
            .unwrap();
        job.causal_identity
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_gate_runs_nothing_and_an_open_gate_runs_the_job_to_policy_blocked_abstention() {
    let rig = Rig::open().await;
    let now = now_ms();
    let identity = rig.ready_history_summarizer_job(now);
    let worker = rig.worker();
    let cancel = CancellationToken::new();

    // No record: the gate is closed, the Ready job is untouched, and nothing connects to the provider.
    assert_eq!(worker.pass(&cancel).await, 0);
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Closed("missing")
    );
    assert!(matches!(
        rig.store
            .lookup_curator_job(PROJECT, &identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Ready(_)
    ));
    assert!(
        rig.store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .is_none()
    );
    assert_eq!(rig.peer.connections.load(Ordering::SeqCst), 0);

    // The owner's record opens the gate: the job is claimed under the authority lease and run; the default-Sensitive staged subject never reaches the remote model and the run settles as a policy-blocked abstention.
    rig.write_activation();
    assert_eq!(worker.pass(&cancel).await, 1);
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Open
    );
    let receipt = rig
        .store
        .lookup_curator_receipt(PROJECT, &identity)
        .unwrap()
        .expect("the run began a receipt");
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Abstained));
    assert_eq!(
        receipt.abstained_reason,
        Some(AbstainReason::OwnerSensitive)
    );
    assert_eq!(
        rig.store
            .lookup_curator_job(PROJECT, &identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Terminal(CuratorJobOutcome::Abstained)
    );
    assert_eq!(
        rig.peer.connections.load(Ordering::SeqCst),
        0,
        "nothing was sent"
    );
    assert!(
        rig.store
            .list_curator_attempts(PROJECT, &identity)
            .unwrap()
            .is_empty()
    );
    // The Kernel tip did not move: no canonical write and no invented provenance.
    let facts = rig.store.curator_status_facts().unwrap();
    assert_eq!(facts.jobs_abstained, 1);
    assert_eq!(facts.jobs_ready, 0);
    assert_eq!(facts.receipts_complete, 1);
    assert_eq!(facts.attempts_attempted, 0);

    assert_eq!(
        worker.pass(&cancel).await,
        0,
        "a settled job is not run again"
    );
    assert_eq!(
        rig.store.curator_status_facts().unwrap().receipts_complete,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subject_staged_from_an_older_root_of_the_project_is_read_under_that_root() {
    // The staged row is keyed by the older root's digest. Reading it under the newest root would consume the job as a wrong-scope abstention.
    let rig = Rig::open().await;
    let now = now_ms();
    let identity = rig.ready_history_summarizer_job_under(now, PROJECT_DIGEST);
    let newest = rig.root("worktree", &"b".repeat(64));
    let older = rig.root("project", PROJECT_DIGEST);
    let worker = rig.worker_with_roots(vec![newest, older]);
    rig.write_activation();

    assert_eq!(worker.pass(&CancellationToken::new()).await, 1);
    let receipt = rig
        .store
        .lookup_curator_receipt(PROJECT, &identity)
        .unwrap()
        .expect("the run began a receipt");
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Abstained));
    assert_eq!(
        receipt.abstained_reason,
        Some(AbstainReason::OwnerSensitive),
        "the subject was read under the root that staged it and settled on its own sensitivity, not on a scope refusal"
    );
    assert_eq!(rig.peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_jobs_no_bound_root_owns_do_not_starve_the_jobs_behind_them() {
    // Unroutable jobs stay Ready for their queue deadline and sort first by deadline. A pass must still reach the routable job behind a full page of them.
    let rig = Rig::open().await;
    let now = now_ms();
    let unbound = "c".repeat(64);
    for index in 0..daemon::curator::worker::JOBS_PER_PASS {
        rig.ready_history_summarizer_job_tagged(now, &unbound, &format!("orphan-{index}"));
    }
    let identity = rig.ready_history_summarizer_job_tagged(now + 1, PROJECT_DIGEST, "routable");
    let worker = rig.worker();
    rig.write_activation();

    assert_eq!(worker.pass(&CancellationToken::new()).await, 1);
    assert_eq!(
        rig.store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .expect("the routable job ran")
            .terminal,
        Some(CuratorReceiptTerminal::Abstained)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_naming_another_providers_credential_closes_the_gate() {
    // The sender speaks Anthropic's protocol and writes the named credential into `x-api-key`. A record naming the OpenAI secret, fingerprint and all, must close the gate rather than send that secret to Anthropic.
    let rig = Rig::open().await;
    let now = now_ms();
    let identity = rig.ready_history_summarizer_job(now);
    let host = rig.host_with(&[(CREDENTIAL, SECRET), ("OPENAI_API_KEY", "sk-openai")]);
    host.credential_identities
        .set(BTreeMap::from([
            (CREDENTIAL.to_string(), CREDENTIAL_IDENTITY.to_string()),
            ("OPENAI_API_KEY".to_string(), "hmac-of-openai".to_string()),
        ]))
        .unwrap();
    let worker = rig.worker_for(host, vec![rig.root("project", PROJECT_DIGEST)]);
    rig.write_activation_naming("OPENAI_API_KEY", "hmac-of-openai");

    assert_eq!(worker.pass(&CancellationToken::new()).await, 0);
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Closed("unknown_credential")
    );
    assert!(
        rig.store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .is_none()
    );
    assert_eq!(rig.peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_moved_to_another_project_after_the_pass_began_is_not_dispatched() {
    // The pass snapshots root-to-project routes once. A root rebound to another project before its job is claimed must not be read or staged under the stale project.
    let rig = Rig::open().await;
    let now = now_ms();
    let identity = rig.ready_history_summarizer_job(now);
    let root = rig.root("project", PROJECT_DIGEST);
    let worker = rig.worker_with_roots(vec![root.clone()]);
    rig.write_activation();
    rig.bind_route(&root.project_root, "git:other");

    assert_eq!(worker.pass(&CancellationToken::new()).await, 0);
    assert!(matches!(
        rig.store
            .lookup_curator_job(PROJECT, &identity)
            .unwrap()
            .unwrap()
            .state,
        CuratorJobState::Ready(_)
    ));
    assert!(
        rig.store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .is_none()
    );
    assert_eq!(rig.peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_worker_loop_returns_before_the_stores_are_released() {
    // The daemon joins the worker under its task tracker before it releases the stores; the loop must return on cancellation from its idle wait, not after the next interval.
    let rig = Rig::open().await;
    let worker = rig.worker();
    let cancel = CancellationToken::new();
    let running = tokio::spawn(daemon::curator::worker::run(worker, cancel.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .expect("the loop returns on cancellation")
        .unwrap();
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Closed("missing")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gate_stays_closed_until_the_host_has_derived_the_credential_identities() {
    // The record names the credential by the keyed identity the host derives once the incarnation key exists; before that nothing can vouch for the named credential, so a matching record must not open the gate.
    let rig = Rig::open().await;
    let now = now_ms();
    let identity = rig.ready_history_summarizer_job(now);
    let host = rig.host();
    let worker = rig.worker_for(Arc::clone(&host), vec![rig.root("project", PROJECT_DIGEST)]);
    rig.write_activation();
    let cancel = CancellationToken::new();

    assert_eq!(worker.pass(&cancel).await, 0);
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Closed("unreadable")
    );
    assert!(
        rig.store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .is_none()
    );

    host.credential_identities
        .set(BTreeMap::from([(
            CREDENTIAL.to_string(),
            CREDENTIAL_IDENTITY.to_string(),
        )]))
        .unwrap();
    assert_eq!(worker.pass(&cancel).await, 1);
    assert_eq!(
        rig.status.reported().activation_state.0,
        ActivationState::Open
    );
}
