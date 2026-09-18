//! The coordinator against real stores, a real supervisor, and a scripted local provider: every corpus case reaches its labeled outcome with evidence delivered, citations bound to disclosed evidence, and contradiction and limitation fields preserved; capacity, cancellation, exhaustion, unknown outcomes, invalidation, and provider failure end as the ledger says; and no case mutates canonical memory or cites what the model was never shown.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use daemon::curator::broker::{MAX_ISSUED_INSPECTIONS, QuestionTemplate};
use daemon::curator::coordinator::{
    Coordinator, InvestigationError, InvestigationPermits, JobContext, MAX_ACTIVE_PER_HOST,
};
use daemon::curator::disclosure::{DisclosureApproval, ModelProfile};
use daemon::curator::model_request::{ANTHROPIC_VERSION, MESSAGES_PATH};
use daemon::curator::project_text::{InspectionBinding, ProjectText, ProtectedLocations};
use daemon::curator::settlement::{ReadRefusal, Settled, TaskClaim, read_selected_proposal};
use daemon::git_sources::{GitReadBounds, RepositoryBinding, read_selection};
use daemon::harness_sources::SourcePublisher;
use host_runtime::model_execution::backend::LlmExecutionBackend;
use host_runtime::model_execution::supervisor::Supervisor;
use kernel::{
    CommitIntent, Dimension, DomainSpec, KernelStore, ProviderEgress, ReviewBinding, ReviewOwner,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceDependency, SourceDescriptorDetail,
};
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, EvidenceAvailability, ProducerBinding, ReserveOutcome,
    ReviewTarget,
};
use memory_store::curator_ledger::{
    AbstainReason, CuratorAttemptTerminal, CuratorBeginOutcome, CuratorReceipt,
    CuratorReceiptTerminal,
};
use memory_store::{LeaseAcquireOutcome, MemoryStore};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use support::curator_corpus::{CASES, Case, Expected, Relation, Source, judge};
use support::tls_peer::{Peer, json_response, text_response};

const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MODEL: &str = "claude-canonical-1";
const CREDENTIAL_ID: &str = "cred-1";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "curator-coordinator-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn kernel_incarnation(root: &std::path::Path) -> String {
    rusqlite::Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT database_incarnation_id FROM kernel_format_marker",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// The supervisor's public backend: never reached, since every Curator attempt is an internal launch.
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

/// Publishes one commit with `message` as remote-eligible evidence, or as `Sensitive` evidence a remote model never receives.
fn publish_commit(
    store: &KernelStore,
    root: &std::path::Path,
    message: &str,
    protected: bool,
    now: i64,
) -> (String, SourceDescriptorDetail) {
    gix::init(root).unwrap();
    let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
    let tree = repo
        .write_object(gix::objs::Tree::empty())
        .unwrap()
        .detach();
    let signature = gix::actor::Signature {
        name: "fixture".into(),
        email: "fixture@example.com".into(),
        time: gix::date::Time::new(1, 0),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: Default::default(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    let oid = repo.write_object(&commit).unwrap().detach().to_string();
    let selection = read_selection(
        &support::projection_gate::open_gate(),
        &RepositoryBinding {
            repository_id: format!("repo-{}", root.file_name().unwrap().to_str().unwrap()),
            path: root.to_path_buf(),
        },
        std::slice::from_ref(&oid),
        GitReadBounds {
            max_commits: std::num::NonZeroUsize::new(1).unwrap(),
            max_object_bytes: std::num::NonZeroU64::new(4096).unwrap(),
            max_total_object_bytes: std::num::NonZeroU64::new(4096).unwrap(),
        },
    )
    .unwrap();
    let published = SourcePublisher {
        kernel: store,
        domain_id: DOMAIN,
        scope_id: Some(SCOPE),
        egress: ProviderEgress::RemoteAllowed,
        sensitivity: if protected {
            Sensitivity::Sensitive
        } else {
            Sensitivity::Normal
        },
    }
    .publish(&selection.units[0], now)
    .unwrap();
    let tip = store.tip().unwrap();
    let row = store
        .observation_for_object_as_of(&published.object_id, tip)
        .unwrap()
        .unwrap();
    let detail: SourceDescriptorDetail =
        serde_json::from_str(row.payload.detail.as_deref().unwrap()).unwrap();
    (published.object_id, detail)
}

/// Real stores, the case's published sources, a claimed job with a begun receipt, and a coordinator over a real supervisor and a local TLS provider.
struct Fixture {
    kernel_dir: tempfile::TempDir,
    _dirs: Vec<tempfile::TempDir>,
    store: Arc<KernelStore>,
    ledger: Arc<MemoryStore>,
    now: i64,
    registration: i64,
    identity: String,
    claim: TaskClaim,
    receipt: CuratorReceipt,
    input: CuratorJobInput,
    sources: Vec<(String, SourceDescriptorDetail)>,
    supervisor: Arc<Supervisor>,
    permits: Arc<InvestigationPermits>,
    clock: Arc<AtomicI64>,
    inspection_limit: usize,
    /// A confined project directory the run may inspect, when the test gives it one.
    project_root: Option<tempfile::TempDir>,
}

impl Fixture {
    fn open(sources: &[Source]) -> Self {
        let kernel_dir = tempfile::tempdir().unwrap();
        let store = KernelStore::open(kernel_dir.path()).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: DOMAIN.to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT.to_string()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            })
            .unwrap();
        let now = now_ms();
        let mut dirs = Vec::new();
        let mut published = Vec::new();
        for source in sources {
            let dir = tempfile::tempdir().unwrap();
            published.push(publish_commit(
                &store,
                dir.path(),
                source.message,
                source.protected,
                now,
            ));
            dirs.push(dir);
        }
        let ledger_dir = tempfile::tempdir().unwrap();
        let ledger = MemoryStore::open(&MemoryStore::test_descriptor(
            ledger_dir.path(),
            "eidnara-curator-coordinator-test",
        ))
        .unwrap();
        dirs.push(ledger_dir);
        let registration = {
            let preparing = ledger
                .authority_begin_prepare("ctx", PROJECT, "memories")
                .unwrap();
            let row = ledger
                .authority_finish_prepare(
                    "ctx",
                    PROJECT,
                    "memories",
                    preparing.generation,
                    "hash",
                    "hash",
                    true,
                )
                .unwrap();
            i64::try_from(row.generation).unwrap()
        };
        let kernel_id = kernel_incarnation(kernel_dir.path());
        let target = ReviewTarget::Memory {
            object_id: published[0].0.clone(),
            source_revision: published[0].1.revision.parse().unwrap(),
        };
        let producer = ProducerBinding {
            producer: "memory-classifier".to_string(),
            firing_id: "firing-1".to_string(),
            ordinal: 0,
        };
        let ReserveOutcome::Reserved(job) = ledger
            .reserve_curator_job(
                PROJECT,
                &producer,
                &CausalInputs {
                    target: target.clone(),
                    question_template: "extracted_facts".to_string(),
                    signals: vec![],
                    required_evidence: published
                        .iter()
                        .map(|(_, detail)| EvidenceAvailability {
                            evidence_id: detail.evidence_id.clone(),
                            available: true,
                        })
                        .collect(),
                    policy_versions: BTreeMap::new(),
                },
                now,
            )
            .unwrap()
        else {
            panic!("fresh inputs reserve")
        };
        let input = CuratorJobInput {
            subject: target,
            starting_references: published[1..]
                .iter()
                .map(|(object_id, _)| object_id.clone())
                .collect(),
            question_template: "extracted_facts".to_string(),
        };
        ledger
            .activate_curator_job(PROJECT, &job.causal_identity, &producer, &input, now)
            .unwrap();
        let LeaseAcquireOutcome::Claim { claim, .. } = ledger
            .acquire_curator_task(
                PROJECT,
                "acq-1",
                "worker-a",
                0,
                registration,
                &job.causal_identity,
                now,
            )
            .unwrap()
        else {
            panic!("the ready job is claimed")
        };
        let CuratorBeginOutcome::Begun(receipt) = ledger
            .begin_curator_receipt(
                PROJECT,
                &job.causal_identity,
                &kernel_id,
                &claim.claim_id,
                now,
            )
            .unwrap()
        else {
            panic!("first claim begins")
        };
        Self {
            kernel_dir,
            _dirs: dirs,
            store: Arc::new(store),
            ledger: Arc::new(ledger),
            now,
            registration,
            identity: job.causal_identity,
            claim: TaskClaim {
                claim_id: claim.claim_id,
                worker_instance: "worker-a".to_string(),
                slot: 0,
            },
            receipt,
            input,
            sources: published,
            supervisor: Arc::new(Supervisor::new(Arc::new(NoPublicModel))),
            permits: Arc::new(InvestigationPermits::default()),
            clock: Arc::new(AtomicI64::new(now + 5)),
            inspection_limit: MAX_ISSUED_INSPECTIONS,
            project_root: None,
        }
    }

    /// A confined project directory with one file mentioning bun.
    fn with_project_root(mut self) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("README.md"),
            b"# Project\nbun builds the workspace\n",
        )
        .unwrap();
        self.project_root = Some(root);
        self
    }

    fn kernel_incarnation(&self) -> String {
        kernel_incarnation(self.kernel_dir.path())
    }

    fn registration(&self) -> i64 {
        self.registration
    }

    fn binding(&self) -> ReviewBinding {
        ReviewBinding {
            project_digest: PROJECT.to_string(),
            domain_id: DOMAIN.to_string(),
            owner: ReviewOwner::Job {
                job_id: self.identity.clone(),
            },
            subject_source: SourceDependency {
                source_kind: "git_commits".to_string(),
                source_id: self.sources[0].0.clone(),
                source_revision: 1,
            },
            reference_sources: vec![],
        }
    }

    fn approval(&self) -> DisclosureApproval {
        DisclosureApproval {
            provider: format!("localhost{MESSAGES_PATH}@{ANTHROPIC_VERSION}"),
            model: MODEL.to_string(),
            credential_id: CREDENTIAL_ID.to_string(),
            kernel_incarnation: self.kernel_incarnation(),
            memstore_incarnation: self.ledger.curator_store_incarnation().unwrap(),
        }
    }

    fn coordinator(&self, peer: &Peer, approval: Option<DisclosureApproval>) -> Coordinator {
        let clock = Arc::clone(&self.clock);
        Coordinator {
            store: Arc::clone(&self.store),
            ledger: Arc::clone(&self.ledger),
            supervisor: Arc::clone(&self.supervisor),
            sender: Arc::new(peer.sender_with_credential(CREDENTIAL_ID)),
            approval,
            profile: ModelProfile {
                model: MODEL.to_string(),
                max_tokens: 1024,
                temperature: None,
            },
            credential_id: CREDENTIAL_ID.to_string(),
            permits: Arc::clone(&self.permits),
            now_ms: Arc::new(move || clock.load(Ordering::SeqCst)),
            inspection_limit: self.inspection_limit,
        }
    }

    fn job(&self) -> memory_store::curator_jobs::CuratorJob {
        self.ledger
            .lookup_curator_job(PROJECT, &self.identity)
            .unwrap()
            .unwrap()
    }

    async fn run(
        &self,
        peer: &Peer,
        approval: Option<DisclosureApproval>,
        cancel: &CancellationToken,
    ) -> Result<Settled, InvestigationError> {
        let coordinator = self.coordinator(peer, approval);
        let job = self.job();
        let binding = self.binding();
        let mut project_text = self.project_root.as_ref().map(|root| {
            ProjectText::open(
                root.path(),
                &ProtectedLocations::new([self.kernel_dir.path().to_path_buf()]).unwrap(),
                InspectionBinding {
                    domain_id: DOMAIN.to_string(),
                    scope_id: Some(SCOPE.to_string()),
                    retain_until: self.now + 60 * 60 * 1_000,
                },
            )
            .unwrap()
        });
        coordinator
            .investigate(
                JobContext {
                    job: &job,
                    input: &self.input,
                    receipt: &self.receipt,
                    claim: &self.claim,
                    binding: &binding,
                    question: QuestionTemplate::ExtractedFacts,
                    project_root: project_text.as_mut(),
                },
                cancel,
            )
            .await
    }

    fn receipt(&self) -> CuratorReceipt {
        self.ledger
            .lookup_curator_receipt(PROJECT, &self.identity)
            .unwrap()
            .unwrap()
    }

    fn attempts(&self) -> Vec<memory_store::curator_ledger::CuratorAttempt> {
        self.ledger
            .list_curator_attempts(PROJECT, &self.identity)
            .unwrap()
    }

    fn evidence(&self, index: usize) -> String {
        self.sources[index].1.evidence_id.clone()
    }

    /// Alias `{alias:N}` for source `N`: the subject is issued first, then each linked reference in order.
    fn expand(&self, turn: &str) -> String {
        let mut text = turn.to_string();
        for index in 0..self.sources.len() {
            text = text.replace(&format!("{{alias:{index}}}"), &format!("ref-{}", index + 1));
        }
        text
    }
}

/// The wire bodies of every request, as the model saw them.
fn prompts(observed: &[support::tls_peer::Observed]) -> Vec<serde_json::Value> {
    observed
        .iter()
        .map(|observed| serde_json::from_slice(&observed.body).unwrap())
        .collect()
}

fn user_text(prompt: &serde_json::Value) -> String {
    prompt["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn run_case(case: &Case) {
    // The corpus label and the scripted citations must both follow from the source text, or the case proves nothing about delivery.
    for source in &case.sources[1..] {
        let read = judge(source);
        assert!(
            read == case.relation
                || (case.relation == Relation::SharedOrigin && read == Relation::Supports)
                || (case.relation == Relation::DecisiveOutsideContext
                    && matches!(read, Relation::Contradicts | Relation::IncompleteSearch)),
            "{}: label {:?} disagrees with the text's own reading {read:?}",
            case.name,
            case.relation
        );
    }
    if let Expected::Published {
        support,
        contradictions,
        ..
    } = case.expected
    {
        for index in support {
            assert!(
                matches!(
                    judge(&case.sources[*index]),
                    Relation::Supports | Relation::Supersedes
                ),
                "{}: a cited support must read as support",
                case.name
            );
        }
        for index in contradictions {
            assert_eq!(
                judge(&case.sources[*index]),
                Relation::Contradicts,
                "{}",
                case.name
            );
        }
    }
    let mut fixture = Fixture::open(case.sources);
    if case.relation == Relation::IncompleteSearch {
        fixture = fixture.with_project_root();
    }
    let tip_before = fixture.store.tip().unwrap();
    let mut peer = Peer::start().await;
    let script: Vec<Vec<u8>> = case
        .turns
        .iter()
        .map(|turn| text_response(&fixture.expand(turn.0)))
        .collect();
    let server = peer.serve_script(script);
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
    let observed = server.await.unwrap();
    assert_eq!(
        observed.len(),
        case.turns.len(),
        "{}: one physical request per scripted turn",
        case.name
    );
    let prompts = prompts(&observed);
    // The subject and the linked aliases are in every prompt; a source is on the wire only after the model asked for it, and a protected source never is.
    for prompt in &prompts {
        let text = user_text(prompt);
        assert!(
            text.contains(case.sources[0].message.trim_end()),
            "{}",
            case.name
        );
        for index in 1..case.sources.len() {
            assert!(
                text.contains(&format!("ref-{}", index + 1)),
                "{}",
                case.name
            );
        }
    }
    for (index, source) in case.sources.iter().enumerate().skip(1) {
        // A ranged read delivers a prefix; the first 30 bytes identify the source either way.
        let marker = &source.message[..source.message.len().min(30)];
        let first = user_text(&prompts[0]).contains(marker);
        assert!(
            !first,
            "{}: source {index} is not in the initial context",
            case.name
        );
        let later = prompts[1..]
            .iter()
            .any(|prompt| user_text(prompt).contains(marker));
        assert_eq!(
            later, !source.protected,
            "{}: source {index} reaches the model only when policy admits it",
            case.name
        );
    }
    let receipt = fixture.receipt();
    match case.expected {
        Expected::Published {
            support,
            contradictions,
            min_limitations,
        } => {
            let Settled::Published(reference) = settled else {
                panic!("{}: {settled:?}", case.name)
            };
            assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Complete));
            let selected = read_selected_proposal(
                &fixture.store,
                &fixture.ledger,
                PROJECT,
                &fixture.identity,
                &fixture.binding(),
                fixture.now + 6,
            )
            .unwrap();
            assert_eq!(selected.reference, reference);
            let proposal = &selected.proposal;
            let cited = |references: &[kernel::EvidenceReference]| -> Vec<String> {
                references
                    .iter()
                    .map(|reference| reference.evidence_id.clone())
                    .collect()
            };
            let expected = |indexes: &[usize]| -> Vec<String> {
                indexes
                    .iter()
                    .map(|index| fixture.evidence(*index))
                    .collect()
            };
            assert_eq!(cited(&proposal.support), expected(support), "{}", case.name);
            assert_eq!(
                cited(&proposal.contradictions),
                expected(contradictions),
                "{}",
                case.name
            );
            assert!(
                proposal.limitations.len() >= min_limitations,
                "{}",
                case.name
            );
            // Citation validity is the coordinator's; whether the cited source supports the claim is the corpus label, kept apart.
            let disclosed: Vec<String> = proposal
                .policy_dependencies
                .disclosed_inputs
                .iter()
                .map(|input| input.evidence_id.clone())
                .collect();
            for evidence in cited(&proposal.support)
                .into_iter()
                .chain(cited(&proposal.contradictions))
            {
                assert!(
                    disclosed.contains(&evidence),
                    "{}: every citation was disclosed",
                    case.name
                );
            }
            assert!(disclosed.contains(&fixture.evidence(0)), "{}", case.name);
            if case.relation == Relation::SharedOrigin {
                assert!(
                    prompts[1..]
                        .iter()
                        .any(|prompt| user_text(prompt).contains("shares its origin")
                            || user_text(prompt).contains(case.sources[2].message.trim_end())),
                    "{}: both forms were delivered",
                    case.name
                );
            }
            if case.relation == Relation::IncompleteSearch {
                assert!(
                    user_text(&prompts[1]).contains("related search: 0 hits"),
                    "{}: zero results are reported, not absence",
                    case.name
                );
                assert!(
                    user_text(&prompts[1]).contains("refused: policy_blocked"),
                    "{}: a confined root exists, and a remote run is still refused project text by policy",
                    case.name
                );
                assert!(
                    !user_text(&prompts[1]).contains("bun builds the workspace\n#"),
                    "{}: no project byte reached the model",
                    case.name
                );
            }
            if case.relation == Relation::Protected {
                assert!(
                    user_text(&prompts[1]).contains("refused ref-2: policy_blocked"),
                    "{}: the protected source is refused by code only",
                    case.name
                );
            }
        }
        Expected::Abstained(reason) => {
            assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Abstained));
            assert_eq!(
                receipt.abstained_reason.map(AbstainReason::as_str),
                Some(reason),
                "{}",
                case.name
            );
            assert!(matches!(settled, Settled::Abstained(_)), "{}", case.name);
            if case.relation == Relation::Injected {
                assert!(
                    user_text(&prompts[2]).contains("refused: unknown_alias"),
                    "{}: the fabricated citation was refused before any effect",
                    case.name
                );
            }
        }
    }
    // No canonical mutation: every source descriptor reads the same at the run's end as before it started.
    let tip_after = fixture.store.tip().unwrap();
    for (object_id, _) in &fixture.sources {
        let before = fixture
            .store
            .observation_for_object_as_of(object_id, tip_before)
            .unwrap()
            .unwrap();
        let after = fixture
            .store
            .observation_for_object_as_of(object_id, tip_after)
            .unwrap()
            .unwrap();
        assert_eq!(before, after, "{}", case.name);
    }
}

macro_rules! corpus_case {
    ($name:ident, $index:expr) => {
        #[tokio::test]
        async fn $name() {
            run_case(&CASES[$index]).await;
        }
    };
}

corpus_case!(corpus_supporting_evidence, 0);
corpus_case!(corpus_contradiction, 1);
corpus_case!(corpus_supersession, 2);
corpus_case!(corpus_shared_origin, 3);
corpus_case!(corpus_decisive_evidence_outside_initial_context, 4);
corpus_case!(corpus_protected_source, 5);
corpus_case!(corpus_incomplete_search, 6);
corpus_case!(corpus_injected_instructions, 7);

#[test]
fn the_corpus_covers_every_relation_once() {
    assert_eq!(CASES.len(), 8);
    let mut relations: Vec<_> = CASES.iter().map(|case| case.relation).collect();
    relations.dedup();
    assert_eq!(relations.len(), 8);
}

#[tokio::test]
async fn capacity_is_acquired_before_anything_is_spawned_and_never_queued() {
    let fixture = Fixture::open(CASES[0].sources);
    let peer = Peer::start().await;
    let permits = Arc::clone(&fixture.permits);
    let held = permits.try_acquire(PROJECT).unwrap();
    assert_eq!(
        fixture
            .run(&peer, Some(fixture.approval()), &CancellationToken::new())
            .await,
        Err(InvestigationError::Capacity),
        "a second investigation of the project is refused, not queued"
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
    drop(held);
    let mut held = Vec::new();
    for index in 0..MAX_ACTIVE_PER_HOST {
        held.push(permits.try_acquire(&format!("{:0>64}", index)).unwrap());
    }
    assert!(permits.try_acquire(PROJECT).is_none(), "the host is full");
    held.pop();
    assert!(permits.try_acquire(PROJECT).is_some());
}

#[tokio::test]
async fn a_missing_approval_sends_nothing_and_leaves_the_receipt_open() {
    let fixture = Fixture::open(CASES[0].sources);
    let peer = Peer::start().await;
    assert_eq!(
        fixture.run(&peer, None, &CancellationToken::new()).await,
        Err(InvestigationError::Unavailable)
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
    assert_eq!(fixture.receipt().terminal, None);
    assert_eq!(fixture.permits.active(), 0, "the permit is released");
}

#[tokio::test]
async fn exhausted_rounds_and_malformed_output_abstain_as_budget_exhausted() {
    let fixture = Fixture::open(CASES[0].sources);
    let mut peer = Peer::start().await;
    // Four rounds of output the step schema rejects: each is charged, refused before any effect, and reported back by code; the fifth request never happens.
    let server = peer.serve_script(vec![
        text_response("not json"),
        text_response(r#"{"v":2,"step":{"kind":"abstain","reason":"x"}}"#),
        text_response(r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-77"}]}}"#),
        text_response(r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-2","range":{"start":9,"end":3}}]}}"#),
    ]);
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Abstained(AbstainReason::BudgetExhausted));
    let observed = server.await.unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(
        peer.connections.load(Ordering::SeqCst),
        4,
        "four physical requests, never a fifth"
    );
    let prompts = prompts(&observed);
    assert!(user_text(&prompts[1]).contains("refused: undecodable"));
    assert!(user_text(&prompts[2]).contains("refused: undecodable"));
    assert!(user_text(&prompts[3]).contains("refused: unknown_alias"));
    assert_eq!(fixture.attempts().len(), 4);
    assert!(
        fixture
            .attempts()
            .iter()
            .all(|attempt| attempt.terminal.map(|(terminal, _)| terminal)
                == Some(CuratorAttemptTerminal::Complete))
    );
    assert_eq!(
        fixture.receipt().abstained_reason,
        Some(AbstainReason::BudgetExhausted)
    );
}

#[tokio::test]
async fn provider_failure_spends_a_round_and_the_cutoff_ends_the_run_without_a_request() {
    let fixture = Fixture::open(CASES[0].sources);
    let mut peer = Peer::start().await;
    let server = peer.serve_script(vec![
        json_response(
            "529 Overloaded",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
            "",
        ),
        text_response(r#"{"v":1,"step":{"kind":"abstain","reason":"enough"}}"#),
    ]);
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Abstained(AbstainReason::ModelDeclined));
    assert_eq!(server.await.unwrap().len(), 2);
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Failed)
    );
    // Past the execution cutoff nothing is admitted: no request, an exhausted abstention, and the original deadline untouched. The claim is renewed on the way so only the cutoff, not a lapsed lease, ends the run.
    let fixture = Fixture::open(CASES[0].sources);
    let registration = fixture.registration();
    for step in 1..4 {
        fixture
            .ledger
            .renew_curator_task(
                PROJECT,
                &fixture.claim.claim_id,
                "worker-a",
                0,
                registration,
                fixture.now + step * 30_000,
            )
            .unwrap();
    }
    fixture
        .clock
        .store(fixture.receipt.execution_cutoff_ms + 1, Ordering::SeqCst);
    let peer = Peer::start().await;
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Abstained(AbstainReason::BudgetExhausted));
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.receipt().run_deadline_ms,
        fixture.receipt.run_deadline_ms
    );
}

#[tokio::test]
async fn a_protected_subject_abstains_by_policy_before_any_request() {
    let fixture = Fixture::open(&[Source {
        message: "sensitive subject text\n",
        protected: true,
    }]);
    let peer = Peer::start().await;
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Abstained(AbstainReason::OwnerSensitive));
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
    assert_eq!(
        fixture.receipt().abstained_reason,
        Some(AbstainReason::OwnerSensitive)
    );
}

#[tokio::test]
async fn invalidation_after_disclosure_abstains_at_settlement() {
    let fixture = Fixture::open(CASES[0].sources);
    let mut peer = Peer::start().await;
    let store = Arc::clone(&fixture.store);
    let object = fixture.sources[1].0.clone();
    // The linked source is disclosed in round one and retired after the proposal request was read, before its response is written: the model concluded over evidence that no longer stands.
    let server = peer.serve_script_with(
        vec![
            text_response(&fixture.expand(CASES[0].turns[0].0)),
            text_response(&fixture.expand(CASES[0].turns[1].0)),
        ],
        move |index| {
            let store = Arc::clone(&store);
            let object = object.clone();
            Box::pin(async move {
                if index == 1 {
                    store
                        .commit(intent("retire-linked"), |envelope| {
                            envelope.retire_observation(&object)?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
            })
        },
    );
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        settled,
        Settled::Abstained(AbstainReason::ExpectationChanged)
    );
    assert_eq!(server.await.unwrap().len(), 2);
    assert_eq!(
        read_selected_proposal(
            &fixture.store,
            &fixture.ledger,
            PROJECT,
            &fixture.identity,
            &fixture.binding(),
            fixture.now + 6,
        ),
        Err(ReadRefusal::NotSelected)
    );
}

#[tokio::test]
async fn an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt() {
    // A peer that accepts the request and never answers: the attempt hits its deadline after the body was sent, so the attempt is failed, not unknown; cancellation while it waits joins the run and leaves the receipt open.
    let fixture = Fixture::open(CASES[0].sources);
    let mut peer = Peer::start().await;
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = peer.serve(
        Box::new(move |_, _| {
            Box::pin(async move {
                release_rx.await.ok();
            })
        }),
        |_| text_response(r#"{"v":1,"step":{"kind":"abstain","reason":"late"}}"#),
    );
    let cancel = CancellationToken::new();
    let run = fixture.run(&peer, Some(fixture.approval()), &cancel);
    let cancelling = async {
        while peer.connections.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
    };
    let (outcome, ()) = tokio::join!(run, cancelling);
    assert_eq!(outcome, Err(InvestigationError::Cancelled));
    release_tx.send(()).ok();
    let _ = server.await;
    assert_eq!(
        fixture.receipt().terminal,
        None,
        "the lifecycle owner settles a cancelled run"
    );
    assert_eq!(
        fixture.permits.active(),
        0,
        "the permit is released after the join"
    );
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Cancelled)
    );
    // An attempt whose terminal is genuinely unknown makes the whole run unknown at settlement.
    let fixture = Fixture::open(CASES[0].sources);
    fixture
        .ledger
        .dispatch_curator_attempt(
            PROJECT,
            &fixture.identity,
            1,
            &fixture.claim.claim_id,
            &fixture.kernel_incarnation(),
            &memory_store::curator_ledger::AttemptMarker {
                body_digest: "b".repeat(64),
                request_bytes: 100,
                provider: "localhost/v1/messages@2023-06-01".to_string(),
                model: MODEL.to_string(),
                credential_id: CREDENTIAL_ID.to_string(),
                policy_union_digest: "u".repeat(64),
            },
            (),
            || fixture.now + 3,
            |()| (),
        )
        .unwrap();
    let mut peer = Peer::start().await;
    let server = peer.serve_script(vec![text_response(
        r#"{"v":1,"step":{"kind":"abstain","reason":"x"}}"#,
    )]);
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Unknown);
    server.await.unwrap();
    assert_eq!(
        fixture.receipt().terminal,
        Some(CuratorReceiptTerminal::Unknown)
    );
}

#[tokio::test]
async fn a_staged_subject_is_policy_blocked_for_a_remote_model_and_abstains() {
    // AE1: a default-Sensitive staged candidate never reaches a remote model; the run abstains by policy before any request.
    let mut fixture = Fixture::open(CASES[0].sources);
    let reference = fixture
        .store
        .stage_review_input(kernel::ReviewStagingSpec {
            extraction_run_id: "run-1".to_string(),
            candidate_id: "subject-1".to_string(),
            producer: "history-summarizer".to_string(),
            binding: fixture.binding(),
            payload: kernel::ReviewPayload::Subject(kernel::ReviewSubject {
                facts: vec![kernel::ExtractedFact {
                    text: "bun builds the workspace".to_string(),
                    spans: vec![kernel::SourceSpan {
                        alias: "s1".to_string(),
                        start: 0,
                        end: 4,
                    }],
                }],
                origins: vec![kernel::SubjectOrigin {
                    alias: "s1".to_string(),
                    message_id: "m1".to_string(),
                    ordinal: 1,
                    block_ids: vec!["m1#0".to_string()],
                    block_hashes: vec!["0".repeat(64)],
                    ranges: vec![kernel::ByteRange { start: 0, end: 4 }],
                }],
            }),
            recorded_at: fixture.now,
            queue_deadline_at: fixture.now + 20 * 60 * 60 * 1_000,
        })
        .unwrap();
    fixture
        .store
        .finish_staging_run(
            "run-1",
            kernel::StagingTerminalState::Completed,
            fixture.now + 1,
        )
        .unwrap();
    fixture.input.subject = ReviewTarget::StagedSubject {
        kernel_incarnation: reference.database_incarnation_id,
        candidate_id: reference.candidate_id,
        payload_digest: reference.payload_digest,
    };
    let peer = Peer::start().await;
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(settled, Settled::Abstained(AbstainReason::OwnerSensitive));
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
}

#[tokio::test]
async fn inspections_are_charged_once_per_read_and_capped_per_job_and_per_batch() {
    // Two linked sources and a limit of three issued inspections: the subject is the first, the batch of two reads the second and third, and any further read is refused by the job cap, not the batch cap.
    let mut fixture = Fixture::open(CASES[4].sources);
    fixture.inspection_limit = 3;
    let mut peer = Peer::start().await;
    let server = peer.serve_script(vec![
        text_response(&fixture.expand(r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"{alias:1}"},{"op":"read_reference","alias":"{alias:2}"}]}}"#)),
        text_response(&fixture.expand(r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"{alias:1}","range":{"start":0,"end":4}}]}}"#)),
        text_response(&fixture.expand(r#"{"v":1,"step":{"kind":"propose","action":"retire","support":[],"contradictions":[{"alias":"{alias:2}"}],"limitations":[],"uncertainty":"low"}}"#)),
    ]);
    // The cap refusal truncates the evidence set, so the run ends as a partial-disclosure abstention however the model concludes.
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        settled,
        Settled::Abstained(AbstainReason::PartialDisclosure)
    );
    let first = prompts(&server.await.unwrap());
    let second = user_text(&first[1]);
    assert!(second.contains(case_marker(&CASES[4].sources[1])));
    assert!(second.contains(case_marker(&CASES[4].sources[2])));
    assert!(
        !second.contains("refused"),
        "two reads fit one batch: {second}"
    );
    let third = user_text(&first[2]);
    assert!(
        third.contains("refused ref-2: inspection_limit"),
        "the fourth read is the job cap's refusal, not a batch refusal: {third}"
    );
    // A batch of eight fits; the ninth operation never reaches the coordinator because the decoder refuses the batch.
    let mut fixture = Fixture::open(CASES[0].sources);
    fixture.inspection_limit = MAX_ISSUED_INSPECTIONS;
    let mut peer = Peer::start().await;
    let eight = std::iter::repeat_n(
        r#"{"op":"read_reference","alias":"ref-2","range":{"start":0,"end":4}}"#,
        8,
    )
    .collect::<Vec<_>>()
    .join(",");
    let server = peer.serve_script(vec![
        text_response(&format!(
            r#"{{"v":1,"step":{{"kind":"read_batch","operations":[{eight}]}}}}"#
        )),
        text_response(r#"{"v":1,"step":{"kind":"abstain","reason":"done"}}"#),
    ]);
    fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    let later = prompts(&server.await.unwrap());
    let second = user_text(&later[1]);
    assert!(
        !second.contains("refused"),
        "eight reads fill one batch without refusal: {second}"
    );
    assert_eq!(
        second.matches("ci: ").count(),
        8,
        "each read was delivered: {second}"
    );
}

fn case_marker(source: &Source) -> &str {
    &source.message[..source.message.len().min(30)]
}

#[tokio::test]
async fn a_citation_outside_the_disclosed_bytes_is_refused_before_binding() {
    let fixture = Fixture::open(CASES[2].sources);
    let mut peer = Peer::start().await;
    // Only the first 40 bytes were disclosed; a citation of bytes 0..60 names bytes the model never saw and settles as an undisclosed citation, not a proposal.
    let server = peer.serve_script(vec![
        text_response(&fixture.expand(CASES[2].turns[0].0)),
        text_response(&fixture.expand(r#"{"v":1,"step":{"kind":"propose","action":"revise","new_text":"revised","support":[{"alias":"{alias:1}","range":{"start":0,"end":60}}],"contradictions":[],"limitations":[],"uncertainty":"low"}}"#)),
    ]);
    let settled = fixture
        .run(&peer, Some(fixture.approval()), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        settled,
        Settled::Abstained(AbstainReason::ExpectationChanged)
    );
    server.await.unwrap();
    assert_eq!(
        fixture.receipt().terminal,
        Some(CuratorReceiptTerminal::Abstained)
    );
}

/// The production positive path before any U7 work: selection over live, eligible, repository-backed descriptors freezes a page, the page is enqueued atomically with its scheduler slot, a ready job is claimed, and the coordinator publishes an inspectable proposal from it without touching canonical memory.
#[tokio::test]
async fn a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path() {
    use daemon::curator::selection::{SelectionScope, select_review_targets};
    use memory_store::curator_jobs::{
        CuratorJobState, EnqueueOutcome, FrozenSelectionState, ProducerBinding,
    };
    let fixture = Fixture::open(CASES[0].sources);
    let tip_before = fixture.store.tip().unwrap();
    // Selection: both published commits are live and eligible; the fixture's own job has other causal inputs, so both are selected.
    let budget = kernel::applicability::EvalBudget::new(None, Arc::default());
    let selection = select_review_targets(
        &fixture.store,
        &fixture.ledger,
        &SelectionScope {
            project: &kernel::ProjectScope::new(PROJECT).unwrap(),
            project_digest: PROJECT,
            classes: &[kernel::source_identity::OccurrenceClass::GitCommits],
            policy_versions: &BTreeMap::new(),
        },
        None,
        &budget,
    )
    .unwrap();
    assert_eq!(selection.references.len(), 2);
    assert_eq!(selection.next_cursor, None, "one pass covers the inventory");
    assert!(
        selection.references.iter().all(|inputs| matches!(
            &inputs.target,
            ReviewTarget::Memory { object_id, .. } if fixture.sources.iter().any(|(id, _)| *id == *object_id)
        ))
    );
    // Freeze under a scheduler slot and enqueue atomically with the slot's completion.
    let frozen = fixture
        .ledger
        .freeze_selection(
            PROJECT,
            "curator-review-selection",
            "curator-review-selection@1",
            &selection,
            fixture.now,
        )
        .unwrap();
    let registration = fixture
        .ledger
        .next_memory_classifier_scheduler_generation("eidnara-memory_classifier-scheduler")
        .unwrap();
    let memory_store::LeaseAcquireOutcome::Claim { claim, .. } = fixture
        .ledger
        .acquire_memory_classifier_task(
            PROJECT,
            "curator-review-selection@1",
            "eidnara-memory_classifier-scheduler",
            2,
            registration,
            3,
            fixture.now,
            fixture.now,
        )
        .unwrap()
    else {
        panic!("the selection slot is leased")
    };
    let producer = ProducerBinding {
        producer: "memory-classifier-selection".to_string(),
        firing_id: "curator-review-selection@1".to_string(),
        ordinal: 0,
    };
    let enqueued = fixture
        .ledger
        .enqueue_frozen_selection(
            PROJECT,
            &claim.claim_id,
            "curator-review-selection@1:complete",
            "eidnara-memory_classifier-scheduler",
            2,
            &frozen,
            &producer,
            fixture.now + 1,
        )
        .unwrap();
    assert_eq!(
        enqueued,
        EnqueueOutcome::Enqueued {
            jobs: 2,
            replayed: 0,
            next_cursor: None
        }
    );
    assert_eq!(
        fixture
            .ledger
            .lookup_frozen_selection(
                PROJECT,
                "curator-review-selection",
                "curator-review-selection@1"
            )
            .unwrap()
            .unwrap()
            .state,
        FrozenSelectionState::Enqueued
    );
    // A second selection over the same inventory finds nothing: every target already has a job.
    let again = select_review_targets(
        &fixture.store,
        &fixture.ledger,
        &SelectionScope {
            project: &kernel::ProjectScope::new(PROJECT).unwrap(),
            project_digest: PROJECT,
            classes: &[kernel::source_identity::OccurrenceClass::GitCommits],
            policy_versions: &BTreeMap::new(),
        },
        None,
        &budget,
    )
    .unwrap();
    assert!(again.references.is_empty());
    // The ready job for the subject commit is claimed and run through the coordinator.
    let ready = fixture.ledger.ready_curator_jobs(PROJECT, 16).unwrap();
    let job = ready
        .iter()
        .find(|job| {
            matches!(&job.target, ReviewTarget::Memory { object_id, .. } if *object_id == fixture.sources[0].0)
                && job.producer == producer
        })
        .unwrap()
        .clone();
    let CuratorJobState::Ready(input) = &job.state else {
        panic!("{job:?}")
    };
    let memory_store::LeaseAcquireOutcome::Claim {
        claim: job_claim, ..
    } = fixture
        .ledger
        .acquire_curator_task(
            PROJECT,
            "acq-selected",
            "worker-b",
            1,
            fixture.registration(),
            &job.causal_identity,
            fixture.now + 2,
        )
        .unwrap()
    else {
        panic!("the ready job is claimed")
    };
    let CuratorBeginOutcome::Begun(receipt) = fixture
        .ledger
        .begin_curator_receipt(
            PROJECT,
            &job.causal_identity,
            &fixture.kernel_incarnation(),
            &job_claim.claim_id,
            fixture.now + 2,
        )
        .unwrap()
    else {
        panic!("first claim begins")
    };
    let mut peer = Peer::start().await;
    let server = peer.serve_script(vec![text_response(
        r#"{"v":1,"step":{"kind":"propose","action":"retain","support":[{"alias":"ref-1"}],"contradictions":[],"limitations":["only the subject itself was available"],"uncertainty":"medium"}}"#,
    )]);
    let coordinator = fixture.coordinator(&peer, Some(fixture.approval()));
    let binding = ReviewBinding {
        owner: ReviewOwner::Job {
            job_id: job.causal_identity.clone(),
        },
        ..fixture.binding()
    };
    let task_claim = TaskClaim {
        claim_id: job_claim.claim_id.clone(),
        worker_instance: "worker-b".to_string(),
        slot: 1,
    };
    let settled = coordinator
        .investigate(
            JobContext {
                job: &job,
                input,
                receipt: &receipt,
                claim: &task_claim,
                binding: &binding,
                question: QuestionTemplate::ExtractedFacts,
                project_root: None,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let Settled::Published(reference) = settled else {
        panic!("{settled:?}")
    };
    assert_eq!(server.await.unwrap().len(), 1);
    let selected = read_selected_proposal(
        &fixture.store,
        &fixture.ledger,
        PROJECT,
        &job.causal_identity,
        &binding,
        fixture.now + 6,
    )
    .unwrap();
    assert_eq!(selected.reference, reference);
    assert_eq!(selected.proposal.action, kernel::ProposalAction::Retain);
    assert!(matches!(
        &selected.proposal.target,
        kernel::ProposalTarget::Memory(target) if target.object_id == fixture.sources[0].0
    ));
    assert_eq!(
        selected.proposal.support[0].evidence_id,
        fixture.evidence(0)
    );
    // Nothing canonical changed: every descriptor reads the same before and after.
    let tip_after = fixture.store.tip().unwrap();
    for (object_id, _) in &fixture.sources {
        let before = fixture
            .store
            .observation_for_object_as_of(object_id, tip_before)
            .unwrap()
            .unwrap();
        let after = fixture
            .store
            .observation_for_object_as_of(object_id, tip_after)
            .unwrap()
            .unwrap();
        assert_eq!(before, after);
    }
}
