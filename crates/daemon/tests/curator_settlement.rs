//! Settlement against real stores: only a completed receipt publishes a staged proposal; unknown, partial, declined, revoked, and uncited runs complete without content; conflicting content at the provisional identity is refused; reads copy the receipt, release the Memory Store, and pass every Kernel check before returning content; and the both-store crash windows leave no unselected result visible.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use daemon::curator::broker::{
    EvidenceBroker, HeldUnder, QuestionTemplate, ReferenceExpectation, RunBinding,
};
use daemon::curator::project_text::{InspectionBinding, ProjectText, ProtectedLocations};
use daemon::curator::settlement::{
    ReadRefusal, RunResult, SETTLEMENT_PRODUCER, SelectedProposal, Settled, Settlement,
    SettlementError, TaskClaim, list_review_outcomes, read_selected_proposal,
    read_selected_proposal_with_hook_for_test,
};
use daemon::curator::steps::Step;
use daemon::git_sources::{GitReadBounds, RepositoryBinding, read_selection};
use daemon::harness_sources::SourcePublisher;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactDestination,
    CommitIntent, CuratorHoldBinding, CuratorHoldError, CuratorHoldKind, CuratorHoldRefusal,
    Dimension, DomainSpec, EvidenceReference, ExtractedFact, KernelStore, ManifestReference,
    PolicyDependencies, ProposalAction, ProposalTarget, ProviderEgress, REVIEW_EXPIRY_MAX_MS,
    ReviewBinding, ReviewOwner, ReviewPayload, ReviewProposal, ReviewQuestionTemplate,
    ReviewReadRefusal, ReviewStagedReference, ReviewStagingSpec, ReviewSubject, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceDependency, SourceDescriptorDetail, SourceSpan,
    StagingTerminalState, Uncertainty, provisional_result_identity,
};
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, EvidenceAvailability, ProducerBinding, ReserveOutcome,
    ReviewTarget,
};
use memory_store::curator_ledger::{
    AbstainReason, AttemptMarker, CURATOR_RUN_DEADLINE_MS, CURATOR_TASK_LEASE_MS,
    CuratorAttemptTerminal, CuratorBeginOutcome, CuratorReceipt, CuratorReceiptTerminal,
    DispatchOutcome,
};
use memory_store::{LeaseAcquireOutcome, MemoryStore};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
const COMMIT_MESSAGE: &str = "Build the workspace with bun\n";
const SECOND_MESSAGE: &str = "Pin the toolchain\n";
const PROJECT_FILE: &str = "fn main() { println!(\"the workspace builds with bun\"); }\n";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HOUR_MS: i64 = 60 * 60 * 1_000;

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
        producer: "curator-settlement-test".to_string(),
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

/// Writes one commit with `message` into a fresh repository at `root` and publishes it through the Git publisher as remote-eligible evidence.
fn publish_commit(
    store: &KernelStore,
    root: &std::path::Path,
    message: &str,
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
        sensitivity: Sensitivity::Normal,
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

/// A Kernel with one remote-eligible published commit, a Memory Store with a ready job under a live claim and begun receipt, and the pieces a settlement needs.
struct Fixture {
    kernel_dir: tempfile::TempDir,
    _ledger_dir: tempfile::TempDir,
    _repo_dir: tempfile::TempDir,
    store: Arc<KernelStore>,
    ledger: Arc<MemoryStore>,
    now: i64,
    registration: i64,
    identity: String,
    claim: TaskClaim,
    source: (String, SourceDescriptorDetail),
    /// A second published commit, disclosed by `broker_with_second`.
    second: (String, SourceDescriptorDetail),
    _second_repo_dir: tempfile::TempDir,
    project_dir: tempfile::TempDir,
}

impl Fixture {
    fn open() -> Self {
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
        let repo_dir = tempfile::tempdir().unwrap();
        let source = publish_commit(&store, repo_dir.path(), COMMIT_MESSAGE, now);
        let second_repo_dir = tempfile::tempdir().unwrap();
        let second = publish_commit(&store, second_repo_dir.path(), SECOND_MESSAGE, now);
        let ledger_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project_dir.path().join("src")).unwrap();
        std::fs::write(project_dir.path().join("src/main.rs"), PROJECT_FILE).unwrap();
        let ledger = MemoryStore::open(&MemoryStore::test_descriptor(
            ledger_dir.path(),
            "eidnara-curator-settlement-test",
        ))
        .unwrap();
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
        let target = ReviewTarget::StagedSubject {
            kernel_incarnation: kernel_id.clone(),
            candidate_id: "subject-1".to_string(),
            payload_digest: "0d".repeat(32),
        };
        let producer = ProducerBinding {
            producer: "history-summarizer".to_string(),
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
                    required_evidence: vec![EvidenceAvailability {
                        evidence_id: "ev-1".to_string(),
                        available: true,
                    }],
                    policy_versions: BTreeMap::new(),
                },
                now,
            )
            .unwrap()
        else {
            panic!("fresh inputs reserve")
        };
        ledger
            .activate_curator_job(
                PROJECT,
                &job.causal_identity,
                &producer,
                &CuratorJobInput {
                    subject: target,
                    starting_references: vec![],
                    question_template: "extracted_facts".to_string(),
                },
                now,
            )
            .unwrap();
        let mut fixture = Self {
            kernel_dir,
            _ledger_dir: ledger_dir,
            _repo_dir: repo_dir,
            store: Arc::new(store),
            ledger: Arc::new(ledger),
            now,
            registration,
            identity: job.causal_identity,
            claim: TaskClaim {
                claim_id: String::new(),
                worker_instance: String::new(),
                slot: 0,
            },
            source,
            second,
            _second_repo_dir: second_repo_dir,
            project_dir,
        };
        fixture.claim = fixture.claim_task("acq-1", "worker-a", now).unwrap();
        let CuratorBeginOutcome::Begun(_) = fixture
            .ledger
            .begin_curator_receipt(
                PROJECT,
                &fixture.identity,
                &kernel_id,
                &fixture.claim.claim_id,
                now,
            )
            .unwrap()
        else {
            panic!("first claim begins")
        };
        fixture
    }

    fn claim_task(&self, acquisition: &str, worker: &str, now: i64) -> Option<TaskClaim> {
        match self
            .ledger
            .acquire_curator_task(
                PROJECT,
                acquisition,
                worker,
                0,
                self.registration,
                &self.identity,
                now,
            )
            .unwrap()
        {
            LeaseAcquireOutcome::Claim { claim, .. } => Some(TaskClaim {
                claim_id: claim.claim_id,
                worker_instance: worker.to_string(),
                slot: 0,
            }),
            LeaseAcquireOutcome::NoWork { .. } => None,
            other => panic!("unexpected acquisition {other:?}"),
        }
    }

    fn kernel_incarnation(&self) -> String {
        kernel_incarnation(self.kernel_dir.path())
    }

    fn memstore_incarnation(&self) -> String {
        self.ledger.curator_store_incarnation().unwrap()
    }

    fn evidence_id(&self) -> String {
        self.source.1.evidence_id.clone()
    }

    fn hold_binding(&self, generation: u64) -> CuratorHoldBinding {
        CuratorHoldBinding {
            project_digest: PROJECT.to_string(),
            kernel_incarnation: self.kernel_incarnation(),
            memstore_incarnation: self.memstore_incarnation(),
            subject: self.identity.clone(),
            generation,
        }
    }

    /// The job's staging binding, owned by the job; settlement substitutes the proposal owner.
    fn review_binding(&self) -> ReviewBinding {
        ReviewBinding {
            project_digest: PROJECT.to_string(),
            domain_id: DOMAIN.to_string(),
            owner: ReviewOwner::Job {
                job_id: self.identity.clone(),
            },
            subject_source: SourceDependency {
                source_kind: "conversation".to_string(),
                source_id: "session-1".to_string(),
                source_revision: 1,
            },
            reference_sources: vec![],
        }
    }

    fn source_expectation(&self) -> ReferenceExpectation {
        Self::expectation(&self.source)
    }

    fn expectation((object_id, detail): &(String, SourceDescriptorDetail)) -> ReferenceExpectation {
        ReferenceExpectation::NativeSource {
            object_id: object_id.clone(),
            class: OccurrenceClass::GitCommits,
            source_revision: detail.revision.parse().unwrap(),
            artifact_digest: detail.artifact_digest.clone(),
            evidence_id: detail.evidence_id.clone(),
            occurrence_tuple: detail.occurrence_tuple.clone(),
        }
    }

    /// A remote-destination broker for `generation` that has disclosed the published commit.
    fn broker(&self, generation: u64) -> EvidenceBroker {
        self.broker_to(generation, ArtifactDestination::Remote)
    }

    fn broker_to(&self, generation: u64, destination: ArtifactDestination) -> EvidenceBroker {
        let binding = self.hold_binding(generation);
        let hold = self
            .store
            .acquire_execution_hold(
                &binding,
                std::slice::from_ref(&self.evidence_id()),
                self.now + 2 * HOUR_MS,
            )
            .unwrap();
        let mut broker = EvidenceBroker::new(
            RunBinding {
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap();
        let commit = broker.aliases.issue(self.source_expectation());
        broker
            .read(&self.store, commit.as_str(), None, self.now + 2)
            .unwrap();
        broker
    }

    /// A local broker with the commit and one project-text capture disclosed; the capture's acquisition reference ends at `now + HOUR_MS`. Returns the capture's evidence id.
    fn local_broker_with_capture(&self, generation: u64) -> (EvidenceBroker, String) {
        let mut broker = self.broker_to(generation, ArtifactDestination::Local);
        let protected = ProtectedLocations::new([self.kernel_dir.path().to_path_buf()]).unwrap();
        let mut text = ProjectText::open(
            self.project_dir.path(),
            &protected,
            InspectionBinding {
                hold: broker.binding().hold.clone(),
                domain_id: DOMAIN.to_string(),
                scope_id: Some(SCOPE.to_string()),
                retain_until: self.now + HOUR_MS,
            },
        )
        .unwrap();
        let read = text
            .read(&self.store, &mut broker, "src/main.rs", None, self.now + 2)
            .unwrap();
        let capture = match broker.aliases.resolve(read.alias.as_str()).unwrap().1 {
            ReferenceExpectation::TemporaryCapture { evidence_id, .. } => evidence_id.clone(),
            other => panic!("a project-text read issues a capture alias, not {other:?}"),
        };
        (broker, capture)
    }

    /// A local broker that has disclosed the commit and the job's staged subject, which the fixture seals here under the job binding; a staged row is sensitive, so only a local destination admits it.
    fn broker_with_subject(&self, generation: u64) -> EvidenceBroker {
        let mut broker = self.broker_to(generation, ArtifactDestination::Local);
        let reference = self
            .store
            .stage_review_input(ReviewStagingSpec {
                extraction_run_id: "run-subject-1".to_string(),
                candidate_id: "subject-1".to_string(),
                producer: "history-summarizer".to_string(),
                binding: self.review_binding(),
                payload: ReviewPayload::Subject(ReviewSubject {
                    facts: vec![ExtractedFact {
                        text: "the workspace builds with bun".to_string(),
                        spans: vec![SourceSpan {
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
                recorded_at: self.now,
                queue_deadline_at: self.now + 24 * HOUR_MS,
            })
            .unwrap();
        self.store
            .finish_staging_run(
                "run-subject-1",
                StagingTerminalState::Completed,
                self.now + 1,
            )
            .unwrap();
        let subject = broker.aliases.issue(ReferenceExpectation::StagedSubject {
            reference,
            binding: self.review_binding(),
        });
        broker
            .read(&self.store, subject.as_str(), None, self.now + 2)
            .unwrap();
        broker
    }

    /// A broker that has disclosed both commits.
    fn broker_with_second(&self, generation: u64) -> EvidenceBroker {
        let mut broker = self.broker(generation);
        let second = broker.aliases.issue(Self::expectation(&self.second));
        broker
            .read(&self.store, second.as_str(), None, self.now + 2)
            .unwrap();
        broker
    }

    /// A proposal citing `support`, with empty policy dependencies: settlement replaces them with the broker's.
    fn proposal(&self, support: &[&str]) -> ReviewProposal {
        ReviewProposal {
            action: ProposalAction::Create,
            target: ProposalTarget::StagedCandidate {
                candidate_id: "subject-1".to_string(),
            },
            new_text: Some("the workspace builds with bun".to_string()),
            support: support
                .iter()
                .map(|id| EvidenceReference {
                    evidence_id: id.to_string(),
                    span: None,
                })
                .collect(),
            contradictions: vec![],
            limitations: vec![],
            uncertainty: Uncertainty::Low,
            manifest: ManifestReference {
                manifest_id: "manifest-1".to_string(),
                digest: "ab".repeat(32),
            },
            policy_dependencies: PolicyDependencies {
                question_template: ReviewQuestionTemplate::ExtractedFacts,
                disclosed_inputs: vec![],
                uncited_disclosed_inputs: vec![],
                ancestry: vec![],
            },
        }
    }

    /// The proposal as settlement stages it: dependencies bound to `disclosed` (sorted), with the uncited set derived from `support`.
    fn bound_proposal(&self, support: &[&str]) -> ReviewProposal {
        self.bound_proposal_over(support, &[&self.evidence_id()])
    }

    fn bound_proposal_over(&self, support: &[&str], disclosed: &[&str]) -> ReviewProposal {
        let mut proposal = self.proposal(support);
        let mut disclosed: Vec<&str> = disclosed.to_vec();
        disclosed.sort_unstable();
        let reference = |id: &&str| EvidenceReference {
            evidence_id: id.to_string(),
            span: None,
        };
        proposal.policy_dependencies = PolicyDependencies {
            question_template: ReviewQuestionTemplate::ExtractedFacts,
            disclosed_inputs: disclosed.iter().map(reference).collect(),
            uncited_disclosed_inputs: disclosed
                .iter()
                .filter(|id| !support.contains(id))
                .map(reference)
                .collect(),
            ancestry: vec![],
        };
        proposal
    }

    fn settlement<'a>(
        &'a self,
        binding: &'a ReviewBinding,
        claim: &'a TaskClaim,
        now: &'a (dyn Fn() -> i64 + Sync),
    ) -> Settlement<'a> {
        Settlement {
            store: &self.store,
            ledger: &self.ledger,
            project: PROJECT,
            binding,
            claim,
            now_ms: now,
            before_completion_for_test: None,
        }
    }

    fn settle(
        &self,
        broker: &EvidenceBroker,
        result: RunResult,
    ) -> Result<Settled, SettlementError> {
        let binding = self.review_binding();
        let now = self.now + 5;
        self.settlement(&binding, &self.claim, &move || now)
            .settle(broker, result)
    }

    fn read(&self, now: i64) -> Result<SelectedProposal, ReadRefusal> {
        read_selected_proposal(
            &self.store,
            &self.ledger,
            PROJECT,
            &self.identity,
            &self.review_binding(),
            now,
        )
    }

    /// Commits one attempt marker under the fixture's claim and, when `terminal` is given, records it.
    fn attempt(&self, generation: u64, terminal: Option<CuratorAttemptTerminal>) {
        self.attempt_under(generation, &self.claim, terminal, self.now + 3);
    }

    /// [`Self::attempt`] under another claim, dated `now`.
    fn attempt_under(
        &self,
        generation: u64,
        claim: &TaskClaim,
        terminal: Option<CuratorAttemptTerminal>,
        now: i64,
    ) {
        let outcome = self
            .ledger
            .dispatch_curator_attempt(
                PROJECT,
                &self.identity,
                generation,
                &claim.claim_id,
                &self.kernel_incarnation(),
                &AttemptMarker {
                    body_digest: "b".repeat(64),
                    request_bytes: 100,
                    provider: "localhost/v1/messages@2023-06-01".to_string(),
                    model: "claude-canonical-1".to_string(),
                    credential_id: "cred-1".to_string(),
                    policy_union_digest: "e".repeat(64),
                },
                (),
                || now,
                |()| (),
            )
            .unwrap();
        let DispatchOutcome::Handed { attempt_index, .. } = outcome else {
            panic!("{outcome:?}");
        };
        if let Some(terminal) = terminal {
            self.ledger
                .finish_curator_attempt(
                    PROJECT,
                    &self.identity,
                    generation,
                    &claim.claim_id,
                    attempt_index,
                    terminal,
                    now + 1,
                )
                .unwrap();
        }
    }

    fn receipt(&self) -> CuratorReceipt {
        self.ledger
            .lookup_curator_receipt(PROJECT, &self.identity)
            .unwrap()
            .unwrap()
    }

    fn staged_reference(
        &self,
        generation: u64,
        proposal: &ReviewProposal,
    ) -> ReviewStagedReference {
        ReviewStagedReference {
            database_incarnation_id: self.kernel_incarnation(),
            candidate_id: provisional_result_identity(&self.identity, generation).candidate_id,
            payload_digest: ReviewPayload::Proposal(Box::new(proposal.clone()))
                .digest()
                .unwrap(),
        }
    }

    fn proposal_binding(&self, generation: u64) -> ReviewBinding {
        ReviewBinding {
            owner: ReviewOwner::Proposal {
                job_id: self.identity.clone(),
                generation,
            },
            ..self.review_binding()
        }
    }

    fn review_hold_binding(&self, generation: u64, candidate_id: &str) -> CuratorHoldBinding {
        CuratorHoldBinding {
            subject: candidate_id.to_string(),
            ..self.hold_binding(generation)
        }
    }

    /// The Kernel half of a settlement, as a worker that then crashed would have left it: the proposal staged and sealed at the provisional identity and retention moved to the review hold.
    fn kernel_half(
        &self,
        broker: &EvidenceBroker,
        proposal: &ReviewProposal,
    ) -> ReviewStagedReference {
        let generation = broker.binding().hold.generation;
        let identity = provisional_result_identity(&self.identity, generation);
        let queue_deadline = self
            .ledger
            .lookup_curator_job(PROJECT, &self.identity)
            .unwrap()
            .unwrap()
            .queue_deadline_ms;
        let reference = self
            .store
            .stage_review_input(ReviewStagingSpec {
                extraction_run_id: identity.extraction_run_id.clone(),
                candidate_id: identity.candidate_id.clone(),
                producer: SETTLEMENT_PRODUCER.to_string(),
                binding: self.proposal_binding(generation),
                payload: ReviewPayload::Proposal(Box::new(proposal.clone())),
                recorded_at: self.now + 5,
                queue_deadline_at: queue_deadline,
            })
            .unwrap();
        self.store
            .finish_staging_run(
                &identity.extraction_run_id,
                StagingTerminalState::Completed,
                self.now + 5,
            )
            .unwrap();
        let created_at = self
            .store
            .read_review_input(&reference, &self.proposal_binding(generation), self.now + 5)
            .unwrap()
            .lifecycle
            .created_at;
        self.store
            .transfer_execution_to_review(
                &broker.binding().hold_id,
                &broker.binding().hold,
                &self.review_hold_binding(generation, &identity.candidate_id),
                created_at + REVIEW_EXPIRY_MAX_MS,
            )
            .unwrap();
        reference
    }
}

#[test]
fn a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel() {
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    // Before settlement nothing is selected, so nothing is readable, though the job is claimed and the attempt is complete.
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
    let settled = fixture
        .settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        )
        .unwrap();
    let expected = fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence]));
    assert_eq!(settled, Settled::Published(expected.clone()));
    // The receipt selects exactly this identity, digest, and generation under both incarnations.
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Complete));
    assert_eq!(receipt.abstained_reason, None);
    let (generation, selection) = receipt.selected.clone().unwrap();
    assert_eq!(generation, 1);
    assert_eq!(selection.candidate_id, expected.candidate_id);
    assert_eq!(selection.payload_digest, expected.payload_digest);
    assert_eq!(receipt.kernel_incarnation_id, fixture.kernel_incarnation());
    assert_eq!(
        receipt.database_incarnation_id,
        fixture.memstore_incarnation()
    );
    // The execution hold is gone and the review hold owns the evidence for at most seven days from the result's creation.
    assert!(
        fixture
            .store
            .validate_held_evidence(
                &broker.binding().hold_id,
                CuratorHoldKind::Execution,
                &broker.binding().hold,
                std::slice::from_ref(&evidence),
                fixture.now + 6,
            )
            .is_err()
    );
    let review = fixture.review_hold_binding(1, &expected.candidate_id);
    let hold = fixture
        .store
        .lookup_review_hold(&review, fixture.now + 6)
        .unwrap()
        .unwrap();
    assert_eq!(hold.references, 1);
    // The list reports the outcome without content; the read returns the proposal with the broker's dependencies, not the model's.
    let page = list_review_outcomes(&fixture.ledger, PROJECT, None, 10).unwrap();
    assert_eq!(page.outcomes.len(), 1);
    assert_eq!(page.outcomes[0].causal_identity, fixture.identity);
    assert_eq!(page.outcomes[0].terminal, CuratorReceiptTerminal::Complete);
    assert!(page.outcomes[0].selected);
    assert_eq!(page.next, None);
    let selected = fixture.read(fixture.now + 6).unwrap();
    assert_eq!(selected.reference, expected);
    assert_eq!(selected.proposal, fixture.bound_proposal(&[&evidence]));
    assert_eq!(selected.review_expires_at, hold.expires_at);
    let created_at = fixture
        .store
        .read_review_input(&expected, &fixture.proposal_binding(1), fixture.now + 6)
        .unwrap()
        .lifecycle
        .created_at;
    assert_eq!(hold.expires_at, created_at + REVIEW_EXPIRY_MAX_MS);
    // A second settlement of a completed receipt is fenced and stages nothing new.
    assert_eq!(
        fixture.settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
        ),
        Err(SettlementError::Fenced)
    );
    // Selection moved the row's deadline with the hold: readable past the 24-hour queue deadline, refused at the review expiry.
    assert!(fixture.read(fixture.now + 25 * HOUR_MS).is_ok());
    assert!(fixture.read(hold.expires_at - 1).is_ok());
    assert!(matches!(
        fixture.read(hold.expires_at),
        Err(ReadRefusal::Kernel(ReviewReadRefusal::Expired) | ReadRefusal::ReviewExpired)
    ));
    // The list pages by causal identity: a full page carries a cursor, and the page after it is empty.
    let page = list_review_outcomes(&fixture.ledger, PROJECT, None, 1).unwrap();
    assert_eq!(page.outcomes.len(), 1);
    assert_eq!(page.next.as_deref(), Some(fixture.identity.as_str()));
    let rest = list_review_outcomes(&fixture.ledger, PROJECT, page.next.as_deref(), 1).unwrap();
    assert!(rest.outcomes.is_empty() && rest.next.is_none());
    assert!(
        list_review_outcomes(&fixture.ledger, &"b".repeat(64), None, 10)
            .unwrap()
            .outcomes
            .is_empty(),
        "another project sees nothing"
    );
}

#[test]
fn the_staged_dependencies_are_the_brokers_union_including_uncited_inputs_and_ancestry() {
    let fixture = Fixture::open();
    let mut broker = fixture.broker_with_second(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let second = fixture.second.1.evidence_id.clone();
    // A disclosure whose union member names an owning decision contributes that decision to the ancestry, once, however many times it was disclosed.
    let alias = broker.aliases.issue(fixture.source_expectation());
    for _ in 0..2 {
        broker.ledger.record_disclosure(
            &alias,
            context_core::curator_policy_union::PolicyUnionMember {
                kind: "canonical_source".to_string(),
                id: "memory-9".to_string(),
                revision: "1".to_string(),
                owner_id: Some("decision-a".to_string()),
                owner_revision: Some("1".to_string()),
            },
            0..0,
        );
    }
    let Settled::Published(reference) = fixture
        .settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        )
        .unwrap()
    else {
        panic!("published")
    };
    let selected = fixture.read(fixture.now + 6).unwrap();
    assert_eq!(selected.reference, reference);
    let deps = &selected.proposal.policy_dependencies;
    let mut expected = fixture.bound_proposal_over(&[&evidence], &[&evidence, &second]);
    expected.policy_dependencies.ancestry = vec!["decision-a".to_string()];
    assert_eq!(selected.proposal, expected);
    assert_eq!(deps.disclosed_inputs.len(), 2);
    assert_eq!(
        deps.uncited_disclosed_inputs
            .iter()
            .map(|input| input.evidence_id.as_str())
            .collect::<Vec<_>>(),
        vec![second.as_str()],
        "the uncited disclosed input is recorded as uncited"
    );
    // The review hold covers both disclosed inputs, cited or not.
    let hold = fixture
        .store
        .lookup_review_hold(
            &fixture.review_hold_binding(1, &reference.candidate_id),
            fixture.now + 6,
        )
        .unwrap()
        .unwrap();
    assert_eq!(hold.references, 2);
}

#[test]
fn unknown_partial_and_declined_runs_complete_without_content() {
    // An attempt that never recorded a terminal makes the disclosure unknown: the receipt says so and nothing is staged.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, None);
    let evidence = fixture.evidence_id();
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Unknown
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Unknown));
    assert_eq!(receipt.selected, None);
    assert_eq!(
        fixture.store.read_review_input(
            &fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence])),
            &fixture.proposal_binding(1),
            fixture.now + 6,
        ),
        Err(ReviewReadRefusal::Missing.into())
    );
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
    assert!(
        fixture
            .store
            .validate_held_evidence(
                &broker.binding().hold_id,
                CuratorHoldKind::Execution,
                &broker.binding().hold,
                &[],
                fixture.now + 6,
            )
            .is_err(),
        "the execution hold is released on the trusted terminal"
    );
    let page = list_review_outcomes(&fixture.ledger, PROJECT, None, 10).unwrap();
    assert_eq!(page.outcomes[0].terminal, CuratorReceiptTerminal::Unknown);
    assert!(!page.outcomes[0].selected);

    // A recorded unknown terminal is the same scalar.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Unknown));
    assert_eq!(
        fixture.settle(&broker, RunResult::Declined).unwrap(),
        Settled::Unknown
    );
    // A broker that recorded an uncertain disclosure itself is unknown too, not partial.
    let fixture = Fixture::open();
    let mut broker = fixture.broker(1);
    broker.ledger.record_uncertain_disclosure();
    assert_eq!(
        fixture.settle(&broker, RunResult::Declined).unwrap(),
        Settled::Unknown
    );

    // A truncated disclosure abstains with its reason, whatever the model produced.
    let fixture = Fixture::open();
    let mut broker = fixture.broker(1);
    broker.ledger.record_partial_disclosure();
    let evidence = fixture.evidence_id();
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Abstained(AbstainReason::PartialDisclosure)
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Abstained));
    assert_eq!(
        receipt.abstained_reason,
        Some(AbstainReason::PartialDisclosure)
    );
    assert_eq!(
        list_review_outcomes(&fixture.ledger, PROJECT, None, 10)
            .unwrap()
            .outcomes[0]
            .abstained_reason,
        Some(AbstainReason::PartialDisclosure)
    );

    // A model that declines abstains too; the abstention lists from the receipt alone.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    assert_eq!(
        fixture.settle(&broker, RunResult::Declined).unwrap(),
        Settled::Abstained(AbstainReason::ModelDeclined)
    );
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));

    // A run bound to another Kernel incarnation than the receipt names is fenced before any write.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let binding = fixture.review_binding();
    let foreign = TaskClaim {
        claim_id: fixture.claim.claim_id.clone(),
        ..fixture.claim.clone()
    };
    let mut other_kernel = fixture.hold_binding(1);
    other_kernel.kernel_incarnation = "x".repeat(32);
    let stranger = EvidenceBroker::new(
        RunBinding {
            hold: other_kernel,
            hold_id: broker.binding().hold_id.clone(),
            destination: kernel::ArtifactDestination::Remote,
        },
        QuestionTemplate::ExtractedFacts,
    )
    .unwrap();
    let now = fixture.now + 5;
    assert_eq!(
        fixture
            .settlement(&binding, &foreign, &move || now)
            .settle(&stranger, RunResult::Declined),
        Err(SettlementError::Fenced)
    );
    assert_eq!(fixture.receipt().terminal, None);
}

#[test]
fn revoked_or_uncited_dependencies_abstain_and_conflicting_content_is_refused() {
    // A dependency retired after disclosure fails the post-output revalidation.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let evidence = fixture.evidence_id();
    fixture
        .store
        .commit(intent("retire-source"), |envelope| {
            envelope.retire_observation(&fixture.source.0)?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Abstained(AbstainReason::ExpectationChanged)
    );
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));

    // A citation of evidence the run never disclosed is refused; a leaked credential in the model text abstains as a secret.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&["srcev:never-disclosed"])))
            )
            .unwrap(),
        Settled::Abstained(AbstainReason::UndisclosedCitation)
    );
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let mut leaking = fixture.proposal(&[]);
    leaking.new_text = Some("sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678".to_string());
    assert_eq!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(leaking)))
            .unwrap(),
        Settled::Abstained(AbstainReason::Secret)
    );
    // The Kernel scans every payload field, identities included; a credential in a model-controlled identity abstains the same way instead of leaving the receipt in progress behind a staging refusal no retry can pass.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let mut leaking = fixture.proposal(&[]);
    leaking.manifest.manifest_id =
        "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678".to_string();
    assert_eq!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(leaking)))
            .unwrap(),
        Settled::Abstained(AbstainReason::Secret)
    );
    assert_eq!(
        fixture.receipt().abstained_reason,
        Some(AbstainReason::Secret)
    );
    // A proposal that breaks a payload rule (here `Create` without text) can never be staged; the run abstains instead of leaving the receipt in progress for retries that fail the same way.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let mut malformed = fixture.proposal(&[]);
    malformed.new_text = None;
    assert_eq!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(malformed)))
            .unwrap(),
        Settled::Abstained(AbstainReason::InvalidProposal)
    );
    assert_eq!(
        fixture.receipt().abstained_reason,
        Some(AbstainReason::InvalidProposal)
    );

    // Different content already at the provisional identity conflicts: nothing completes and nothing is readable.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let evidence = fixture.evidence_id();
    let mut other = fixture.bound_proposal(&[&evidence]);
    other.new_text = Some("something else entirely".to_string());
    let identity = provisional_result_identity(&fixture.identity, 1);
    fixture
        .store
        .stage_review_input(ReviewStagingSpec {
            extraction_run_id: identity.extraction_run_id.clone(),
            candidate_id: identity.candidate_id.clone(),
            producer: SETTLEMENT_PRODUCER.to_string(),
            binding: fixture.proposal_binding(1),
            payload: ReviewPayload::Proposal(Box::new(other)),
            recorded_at: fixture.now + 4,
            queue_deadline_at: fixture.now + 20 * HOUR_MS,
        })
        .unwrap();
    assert_eq!(
        fixture.settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
        ),
        Err(SettlementError::ConflictingContent)
    );
    assert_eq!(fixture.receipt().terminal, None);
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
    assert!(
        list_review_outcomes(&fixture.ledger, PROJECT, None, 10)
            .unwrap()
            .outcomes
            .is_empty()
    );
    // A sealed row with different bytes conflicts the same way: recovery adopts only the identical result.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let evidence = fixture.evidence_id();
    let mut other = fixture.bound_proposal(&[&evidence]);
    other.new_text = Some("something else entirely".to_string());
    fixture.kernel_half(&broker, &other);
    assert_eq!(
        fixture.settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
        ),
        Err(SettlementError::ConflictingContent)
    );
    assert_eq!(fixture.receipt().terminal, None);
}

#[test]
fn a_proposal_the_kernel_cannot_stage_completes_without_content() {
    // 32,700 quote characters decode under the text bound and pass the step schema, but every one of them serializes as two bytes, so the encoded payload exceeds the Kernel's bound. The model's oversized proposal completes the receipt as an abstention; it is not a Kernel failure that leaves the receipt open.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let text = "\"".repeat(32_700);
    let step = format!(
        r#"{{"v":1,"step":{{"kind":"propose","action":"create","new_text":{},"uncertainty":"low"}}}}"#,
        serde_json::to_string(&text).unwrap()
    );
    assert!(Step::parse(&step, &broker, false).is_ok());
    let mut oversized = fixture.proposal(&[]);
    oversized.new_text = Some(text);
    assert_eq!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(oversized)))
            .unwrap(),
        Settled::Abstained(AbstainReason::InvalidProposal)
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Abstained));
    assert_eq!(
        receipt.abstained_reason,
        Some(AbstainReason::InvalidProposal)
    );
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
}

#[test]
fn kernel_results_stay_private_until_the_receipt_selects_them() {
    // The Kernel half committed and the worker crashed before the Memory Store selected anything: completed staging is not publication.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let bound = fixture.bound_proposal(&[&evidence]);
    let reference = fixture.kernel_half(&broker, &bound);
    assert!(
        fixture
            .store
            .read_review_input(&reference, &fixture.proposal_binding(1), fixture.now + 6)
            .is_ok(),
        "the Kernel row is sealed and readable by identity"
    );
    assert_eq!(
        fixture.read(fixture.now + 6),
        Err(ReadRefusal::NotSelected),
        "no completed receipt selects it, so the API discloses nothing"
    );
    assert!(
        list_review_outcomes(&fixture.ledger, PROJECT, None, 10)
            .unwrap()
            .outcomes
            .is_empty()
    );
    // Same-generation recovery under the still-valid claim completes without another model call: the identical staging replays, the review hold is found, the receipt selects.
    let settled = fixture
        .settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        )
        .unwrap();
    assert_eq!(settled, Settled::Published(reference.clone()));
    assert_eq!(fixture.read(fixture.now + 6).unwrap().reference, reference);
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(
                &fixture.review_hold_binding(1, &reference.candidate_id),
                fixture.now + 6,
            )
            .unwrap()
            .map(|hold| hold.references),
        Some(1),
        "recovery reuses the review hold rather than acquiring a second"
    );
    // A recovery that must abstain releases the review hold it found, so no retention outlives a content-free terminal.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let evidence = fixture.evidence_id();
    let reference = fixture.kernel_half(&broker, &fixture.bound_proposal(&[&evidence]));
    fixture
        .store
        .commit(intent("retire-source"), |envelope| {
            envelope.retire_observation(&fixture.source.0)?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Abstained(AbstainReason::ExpectationChanged)
    );
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(
                &fixture.review_hold_binding(1, &reference.candidate_id),
                fixture.now + 6
            )
            .unwrap(),
        None
    );
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
}

#[test]
fn recovery_adopts_captures_whose_acquisition_reference_moved_to_the_review_expiry() {
    // The committed hold transfer moves every live capture's `retain_until` to the review expiry, so the retry sees a longer reference than the one the alias was issued with; the capture is still the one the run disclosed.
    let fixture = Fixture::open();
    let (broker, capture) = fixture.local_broker_with_capture(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let bound = fixture.bound_proposal_over(&[&evidence], &[&evidence, &capture]);
    let reference = fixture.kernel_half(&broker, &bound);
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let hold = fixture
        .store
        .lookup_review_hold(&review, fixture.now + 6)
        .unwrap()
        .unwrap();
    let held = fixture
        .store
        .validate_held_evidence(
            &hold.hold_id,
            CuratorHoldKind::Review,
            &review,
            std::slice::from_ref(&capture),
            fixture.now + 6,
        )
        .unwrap();
    assert_eq!(
        held[0].retain_until,
        Some(hold.expires_at),
        "the transfer moved the capture's reference from one hour to the review expiry"
    );
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Published(reference.clone())
    );
    assert_eq!(fixture.read(fixture.now + 6).unwrap().reference, reference);
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(&review, fixture.now + 6)
            .unwrap()
            .map(|hold| hold.references),
        Some(2),
        "the review hold still covers both disclosed inputs"
    );
}

#[test]
fn a_citation_span_must_name_the_disclosed_alias_of_its_evidence() {
    // A span says which rendered alias the cited bytes came from. An alias the run never issued, or one that resolves to other evidence, is a citation to bytes the model was not shown under that name, however real the evidence id is.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let cite = |alias: &str| {
        let mut proposal = fixture.proposal(&[&evidence]);
        proposal.support[0].span = Some(SourceSpan {
            alias: alias.to_string(),
            start: 0,
            end: 4,
        });
        proposal
    };
    assert_eq!(
        fixture.settle(&broker, RunResult::Proposal(Box::new(cite("ref-99")))),
        Ok(Settled::Abstained(AbstainReason::UndisclosedCitation)),
        "an alias the run never issued cannot anchor a citation"
    );
    // The receipt is complete now; a fresh job shows the disclosed alias is accepted.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let disclosed = broker
        .ledger
        .disclosed()
        .next()
        .unwrap()
        .as_str()
        .to_string();
    let mut proposal = fixture.proposal(&[&evidence]);
    proposal.support[0].span = Some(SourceSpan {
        alias: disclosed,
        start: 0,
        end: 4,
    });
    assert!(matches!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(proposal)))
            .unwrap(),
        Settled::Published(_)
    ));
}

#[test]
fn a_citation_span_must_lie_within_the_bytes_rendered_under_its_alias() {
    // An excerpt read discloses a byte range, not the artifact. A span outside every range rendered under the alias cites bytes the model never saw.
    let fixture = Fixture::open();
    let mut broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let second = fixture.second.1.evidence_id.clone();
    let alias = broker.aliases.issue(Fixture::expectation(&fixture.second));
    broker
        .read(&fixture.store, alias.as_str(), Some(0..8), fixture.now + 2)
        .unwrap();
    let cite = |start: u64, end: u64| {
        let mut proposal = fixture.proposal(&[&second]);
        proposal.support[0].span = Some(SourceSpan {
            alias: alias.as_str().to_string(),
            start,
            end,
        });
        proposal
    };
    let binding = fixture.review_binding();
    let now = fixture.now + 5;
    let clock = move || now;
    assert_eq!(
        fixture
            .settlement(&binding, &fixture.claim, &clock)
            .settle(&broker, RunResult::Proposal(Box::new(cite(2, 40)))),
        Ok(Settled::Abstained(AbstainReason::UndisclosedCitation)),
        "a span past the rendered excerpt is not a citation to disclosed bytes"
    );
    // A span of no bytes lies within any range and cites nothing.
    let fixture = Fixture::open();
    let mut broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let second = fixture.second.1.evidence_id.clone();
    let alias = broker.aliases.issue(Fixture::expectation(&fixture.second));
    broker
        .read(&fixture.store, alias.as_str(), Some(0..8), fixture.now + 2)
        .unwrap();
    let mut empty = fixture.proposal(&[&second]);
    empty.support[0].span = Some(SourceSpan {
        alias: alias.as_str().to_string(),
        start: 4,
        end: 4,
    });
    assert_eq!(
        fixture.settle(&broker, RunResult::Proposal(Box::new(empty))),
        Ok(Settled::Abstained(AbstainReason::UndisclosedCitation)),
        "an empty span identifies no disclosed bytes"
    );
    let fixture = Fixture::open();
    let mut broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let second = fixture.second.1.evidence_id.clone();
    let alias = broker.aliases.issue(Fixture::expectation(&fixture.second));
    broker
        .read(&fixture.store, alias.as_str(), Some(0..8), fixture.now + 2)
        .unwrap();
    let mut proposal = fixture.proposal(&[&second]);
    proposal.support[0].span = Some(SourceSpan {
        alias: alias.as_str().to_string(),
        start: 2,
        end: 8,
    });
    assert!(matches!(
        fixture
            .settle(&broker, RunResult::Proposal(Box::new(proposal)))
            .unwrap(),
        Settled::Published(_)
    ));
}

#[test]
fn a_completion_the_ledger_refuses_as_clock_behind_keeps_the_review_hold_for_the_retry() {
    // The ledger refuses a completion dated before its newest event and leaves the claim live for a retry. That refusal is not a fence: the review hold this settlement moved retention to must survive it, because the Kernel will not open another execution hold for a transferred generation.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let binding = fixture.review_binding();
    let behind = fixture.now + 2;
    let clock = move || behind;
    let refused = fixture.settlement(&binding, &fixture.claim, &clock).settle(
        &broker,
        RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
    );
    assert!(
        matches!(refused, Err(SettlementError::Store(_))),
        "a clock-behind refusal is retryable, not a fence: {refused:?}"
    );
    let reference = fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence]));
    assert!(
        fixture
            .store
            .lookup_review_hold(
                &fixture.review_hold_binding(1, &reference.candidate_id),
                fixture.now + 6
            )
            .unwrap()
            .is_some(),
        "the review hold outlives a retryable refusal"
    );
    assert_eq!(fixture.receipt().terminal, None);
    // Once the clock has caught up, the same run publishes through the recovered review hold.
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Published(reference.clone())
    );
    assert_eq!(fixture.read(fixture.now + 6).unwrap().reference, reference);
}

#[test]
fn recovery_releases_a_review_hold_a_purge_degraded() {
    // The transfer committed, then a purge degraded the review pin before the completion. The retry must still find that pin: it protects nothing, so revalidation abstains, and the abstention releases it instead of leaving a degraded pin on the active-hold count until expiry.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let reference = fixture.kernel_half(&broker, &fixture.bound_proposal(&[&evidence]));
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let hold = fixture
        .store
        .lookup_review_hold(&review, fixture.now + 6)
        .unwrap()
        .unwrap();
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge-1"),
            identity: ArtifactDeletionIdentity::Digest(fixture.source.1.artifact_digest.clone()),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("retired".to_string()),
            deleted_at: fixture.now + 4,
        })
        .unwrap();
    let settled = fixture
        .settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        )
        .unwrap();
    assert!(
        matches!(settled, Settled::Abstained(_)),
        "a purged input publishes nothing: {settled:?}"
    );
    let release = fixture.store.release_review_hold(&hold.hold_id, &review);
    assert!(
        matches!(
            release,
            Err(CuratorHoldError::Refused(CuratorHoldRefusal::Released))
        ),
        "the settlement released the degraded review pin: {release:?}"
    );
}

#[test]
fn recovery_after_the_acquisition_reference_lapsed_reads_the_moved_reference() {
    // The transfer moved the capture's reference to the review expiry, so a recovery that runs after the reference the alias was issued with has passed judges the moved one, not the stale one.
    let fixture = Fixture::open();
    let (broker, capture) = fixture.local_broker_with_capture(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let bound = fixture.bound_proposal_over(&[&evidence], &[&evidence, &capture]);
    let reference = fixture.kernel_half(&broker, &bound);
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let hold = fixture
        .store
        .lookup_review_hold(&review, fixture.now + 6)
        .unwrap()
        .unwrap();
    let alias = broker
        .ledger
        .disclosed()
        .find(|alias| {
            broker
                .aliases
                .resolve(alias.as_str())
                .unwrap()
                .1
                .evidence_id()
                == Some(&capture)
        })
        .unwrap()
        .as_str()
        .to_string();
    let after_reference = fixture.now + HOUR_MS + 10;
    assert_eq!(
        broker.revalidate_under(
            &fixture.store,
            &alias,
            after_reference,
            HeldUnder::Review {
                hold: &hold,
                binding: &review,
            },
        ),
        Ok(Some(capture.clone()))
    );
    // Under the execution hold the issue-time reference is the only one, and it has lapsed.
    assert!(
        broker
            .revalidate_under(
                &fixture.store,
                &alias,
                after_reference,
                HeldUnder::Execution(broker.binding()),
            )
            .is_err()
    );
}

#[test]
fn a_publication_whose_receipt_cannot_be_read_back_keeps_its_review_hold() {
    // The completion applied; the read that reports what it recorded then fails. The receipt may select this proposal, and readers need the hold to reach it, so a store failure is not a reason to release.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let shadow = || {
        // After the completion commits, the connection's view of the receipts is a shadow the row reader refuses.
        storage::after_commit_for_test(|conn| {
            conn.execute("CREATE TEMP TABLE curator_receipts (x)", [])
                .expect("plant a shadow after the commit");
        });
    };
    // The shadow also blocks the fixture's ledger reads afterwards, so the review binding is computed first.
    let reference = fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence]));
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let binding = fixture.review_binding();
    let now = fixture.now + 5;
    let clock = move || now;
    let mut settlement = fixture.settlement(&binding, &fixture.claim, &clock);
    settlement.before_completion_for_test = Some(&shadow);
    let settled = settlement.settle(
        &broker,
        RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
    );
    assert!(
        matches!(settled, Err(SettlementError::Store(_))),
        "the read-back failed, not the completion: {settled:?}"
    );
    assert!(
        fixture
            .store
            .lookup_review_hold(&review, fixture.now + 6)
            .unwrap()
            .is_some(),
        "the review hold outlives a failed read-back"
    );
}

#[test]
fn a_staging_binding_from_another_project_or_job_is_refused_before_any_kernel_write() {
    // The binding names the job whose receipt selects the row. One from another project or job would stage the proposal under that scope and let this receipt select a row its own binding can never read.
    let fixture = Fixture::open();
    let evidence = fixture.evidence_id();
    let now = fixture.now + 5;
    let clock = move || now;
    let mut other_project = fixture.review_binding();
    other_project.project_digest = "b".repeat(64);
    let mut other_job = fixture.review_binding();
    other_job.owner = ReviewOwner::Job {
        job_id: "c".repeat(64),
    };
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    for (label, foreign) in [("project", other_project), ("job", other_job)] {
        let settled = fixture.settlement(&foreign, &fixture.claim, &clock).settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        );
        assert_eq!(
            settled,
            Err(SettlementError::BindingMismatch),
            "a binding from another {label} must not stage"
        );
        assert_eq!(fixture.receipt().terminal, None);
        assert_eq!(
            fixture.store.read_review_input(
                &fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence])),
                &fixture.proposal_binding(1),
                now,
            ),
            Err(ReviewReadRefusal::Missing.into()),
            "nothing was staged under the {label} binding"
        );
    }
}

#[test]
fn recovery_revalidates_a_staged_subject_under_the_review_hold() {
    // A staged subject holds no evidence id, so its revalidation is a check that the run's hold is live. Once retention has moved to the review hold, that is the hold to check; the released execution hold would refuse and abstain a result that is already sealed.
    let fixture = Fixture::open();
    let broker = fixture.broker_with_subject(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let reference = fixture.kernel_half(&broker, &fixture.bound_proposal(&[&evidence]));
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Published(reference.clone())
    );
    assert_eq!(fixture.read(fixture.now + 6).unwrap().reference, reference);
}

#[test]
fn a_replayed_abstention_reports_the_reason_the_receipt_recorded() {
    // A retry after completion is fenced at the receipt copy, so a replayed completion id means two settlements of one claim passed that copy together. The one that completes second replays the first whatever reason it derived itself: here it cites evidence the run never disclosed while the other records that the model declined, and the receipt's reason is the one it reports.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let first = || {
        assert_eq!(
            fixture.settle(&broker, RunResult::Declined).unwrap(),
            Settled::Abstained(AbstainReason::ModelDeclined)
        );
    };
    let binding = fixture.review_binding();
    let now = fixture.now + 5;
    let clock = move || now;
    let mut settlement = fixture.settlement(&binding, &fixture.claim, &clock);
    settlement.before_completion_for_test = Some(&first);
    assert_eq!(
        settlement
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&"f".repeat(64)])))
            )
            .unwrap(),
        Settled::Abstained(AbstainReason::ModelDeclined)
    );
    assert_eq!(
        fixture.receipt().abstained_reason,
        Some(AbstainReason::ModelDeclined)
    );
}

#[test]
fn a_fenced_content_free_completion_releases_the_recovered_review_hold() {
    // Generation 1 staged and transferred, then crashed. Its retry revalidates under the recovered review hold but ends content-free, and a successor takes over in the window before its completion: the fenced write leaves the Kernel row private and must not leave the seven-day review hold behind it.
    let fixture = Fixture::open();
    let evidence = fixture.evidence_id();
    let loser = fixture.broker(1);
    let reference = fixture.kernel_half(&loser, &fixture.bound_proposal(&[&evidence]));
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let later = fixture.now + CURATOR_TASK_LEASE_MS + 1;
    assert!(
        fixture
            .store
            .lookup_review_hold(&review, later + 1)
            .unwrap()
            .is_some()
    );
    let takeover = || {
        let claimed = fixture.claim_task("acq-2", "worker-b", later).unwrap();
        fixture
            .ledger
            .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &claimed.claim_id, later)
            .unwrap();
    };
    let binding = fixture.review_binding();
    let clock = move || later - CURATOR_TASK_LEASE_MS + 5;
    let mut settlement = fixture.settlement(&binding, &fixture.claim, &clock);
    settlement.before_completion_for_test = Some(&takeover);
    assert_eq!(
        settlement.settle(&loser, RunResult::Declined),
        Err(SettlementError::Fenced)
    );
    assert_eq!(fixture.receipt().terminal, None);
    assert_eq!(fixture.read(later + 1), Err(ReadRefusal::NotSelected));
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(&review, later + 1)
            .unwrap(),
        None,
        "the fenced generation released the review hold it had moved retention to"
    );
}

#[test]
fn a_takeover_fences_the_losing_generation_and_selects_only_its_own_result() {
    let fixture = Fixture::open();
    let evidence = fixture.evidence_id();
    // Generation 1 stages and transfers; in the window before its Memory Store completion its claim lapses and a successor takes over at generation 2. The late losing write is fenced and its review hold released; the Kernel row stays private.
    let loser = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let later = fixture.now + CURATOR_TASK_LEASE_MS + 1;
    let successor = std::sync::Mutex::new(None);
    let takeover = || {
        let claimed = fixture.claim_task("acq-2", "worker-b", later).unwrap();
        let taken = fixture
            .ledger
            .take_over_curator_receipt(PROJECT, &fixture.identity, 1, &claimed.claim_id, later)
            .unwrap();
        assert_eq!(taken.generation, 2);
        *successor.lock().unwrap() = Some(claimed);
    };
    let binding = fixture.review_binding();
    let clock = move || later - CURATOR_TASK_LEASE_MS + 5;
    let mut settlement = fixture.settlement(&binding, &fixture.claim, &clock);
    settlement.before_completion_for_test = Some(&takeover);
    assert_eq!(
        settlement.settle(
            &loser,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
        ),
        Err(SettlementError::Fenced)
    );
    let successor = successor.into_inner().unwrap().unwrap();
    let losing = fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence]));
    assert_eq!(fixture.receipt().terminal, None);
    assert_eq!(fixture.read(later + 1), Err(ReadRefusal::NotSelected));
    assert!(
        fixture
            .store
            .read_review_input(&losing, &fixture.proposal_binding(1), later + 1)
            .is_ok(),
        "the losing row is sealed in the Kernel"
    );
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(
                &fixture.review_hold_binding(1, &losing.candidate_id),
                later + 1
            )
            .unwrap(),
        None,
        "the losing generation released the review hold it had moved retention to"
    );
    // A losing worker that only learns of the takeover at settlement is fenced before any Kernel write; its transferred generation cannot open another execution hold either.
    assert_eq!(
        fixture.settle(
            &loser,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
        ),
        Err(SettlementError::Fenced)
    );
    // The successor investigates again under generation 2 and selects a different identity.
    let winner = fixture.broker(2);
    fixture.attempt_under(
        2,
        &successor,
        Some(CuratorAttemptTerminal::Complete),
        later + 1,
    );
    let mut revised = fixture.proposal(&[&evidence]);
    revised.uncertainty = Uncertainty::Medium;
    let clock = move || later + 2;
    let settled = fixture
        .settlement(&binding, &successor, &clock)
        .settle(&winner, RunResult::Proposal(Box::new(revised)))
        .unwrap();
    let mut expected_winner = fixture.bound_proposal(&[&evidence]);
    expected_winner.uncertainty = Uncertainty::Medium;
    let winning = fixture.staged_reference(2, &expected_winner);
    assert_eq!(settled, Settled::Published(winning.clone()));
    assert_ne!(winning.candidate_id, losing.candidate_id);
    let selected = fixture.read(later + 3).unwrap();
    assert_eq!(selected.reference, winning);
    assert_eq!(selected.proposal.uncertainty, Uncertainty::Medium);
    let receipt = fixture.receipt();
    assert_eq!(receipt.generation, 2);
    assert_eq!(
        receipt.selected.as_ref().map(|(generation, _)| *generation),
        Some(2)
    );
}

#[test]
fn a_cancelled_or_expired_receipt_records_that_whatever_the_run_produced() {
    // The Memory Store decides these two terminals from the receipt itself; settlement reports what was recorded, and the retention it moved to review is released because nothing was selected.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    fixture
        .ledger
        .cancel_curator_receipt(PROJECT, &fixture.identity, fixture.now + 4)
        .unwrap();
    assert_eq!(
        fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence])))
            )
            .unwrap(),
        Settled::Cancelled
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Cancelled));
    assert_eq!(receipt.selected, None);
    assert_eq!(fixture.read(fixture.now + 6), Err(ReadRefusal::NotSelected));
    let reference = fixture.staged_reference(1, &fixture.bound_proposal(&[&evidence]));
    assert_eq!(
        fixture
            .store
            .lookup_review_hold(
                &fixture.review_hold_binding(1, &reference.candidate_id),
                fixture.now + 6
            )
            .unwrap(),
        None,
        "nothing was selected, so the review hold is released"
    );

    // A content-free completion at the run deadline records `expired`, not the abstention the run derived; the claim was renewed all the way there and never moved the deadline.
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    let binding = fixture.review_binding();
    let late = fixture.now + CURATOR_RUN_DEADLINE_MS;
    let mut renewed_at = fixture.now;
    while renewed_at + CURATOR_TASK_LEASE_MS <= late {
        renewed_at += CURATOR_TASK_LEASE_MS - 5;
        fixture
            .ledger
            .renew_curator_task(
                PROJECT,
                &fixture.claim.claim_id,
                &fixture.claim.worker_instance,
                fixture.claim.slot,
                fixture.registration,
                renewed_at,
            )
            .unwrap();
    }
    let clock = move || late;
    assert_eq!(
        fixture
            .settlement(&binding, &fixture.claim, &clock)
            .settle(&broker, RunResult::Declined)
            .unwrap(),
        Settled::Expired
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.terminal, Some(CuratorReceiptTerminal::Expired));
    assert_eq!(receipt.abstained_reason, None);
}

#[test]
fn a_selected_result_is_readable_only_through_its_live_review_hold() {
    let fixture = Fixture::open();
    let broker = fixture.broker(1);
    fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
    let evidence = fixture.evidence_id();
    let Settled::Published(reference) = fixture
        .settle(
            &broker,
            RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
        )
        .unwrap()
    else {
        panic!("published")
    };
    let review = fixture.review_hold_binding(1, &reference.candidate_id);
    let hold = fixture
        .store
        .lookup_review_hold(&review, fixture.now + 6)
        .unwrap()
        .unwrap();
    assert!(fixture.read(hold.expires_at - 1).is_ok());
    // The review hold is released while reads are under way: every read sees either the full content or the refusal, and once a read is refused no later read succeeds.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let reader = {
        let store = Arc::clone(&fixture.store);
        let ledger = Arc::clone(&fixture.ledger);
        let identity = fixture.identity.clone();
        let binding = fixture.review_binding();
        let barrier = Arc::clone(&barrier);
        let now = fixture.now + 7;
        std::thread::spawn(move || {
            barrier.wait();
            (0..200)
                .map(|_| {
                    read_selected_proposal(&store, &ledger, PROJECT, &identity, &binding, now)
                        .map(|selected| selected.reference)
                })
                .collect::<Vec<_>>()
        })
    };
    barrier.wait();
    fixture
        .store
        .release_review_hold(&hold.hold_id, &review)
        .unwrap();
    let reads = reader.join().unwrap();
    let first_refusal = reads.iter().position(Result::is_err);
    for (index, read) in reads.iter().enumerate() {
        match read {
            Ok(read) => {
                assert_eq!(*read, reference);
                assert!(
                    first_refusal.is_none_or(|first| index < first),
                    "a read succeeded after the hold was seen released"
                );
            }
            Err(ReadRefusal::ReviewExpired) => {}
            Err(other) => panic!("{other:?}"),
        }
    }
    assert!(
        reads.last().unwrap().is_err(),
        "the last read ran after the release and was refused"
    );
    assert_eq!(
        fixture.read(fixture.now + 8),
        Err(ReadRefusal::ReviewExpired),
        "a released review hold ends readability though the Kernel row is still sealed"
    );
    // The receipt still selects the result; selection never rescues a lapsed review window.
    assert_eq!(
        fixture.receipt().terminal,
        Some(CuratorReceiptTerminal::Complete)
    );
}

/// A hold that ends between the review-hold lookup and the evidence validation under it is the same review expiry the lookup reports a moment later, whether it was released or degraded by a purge. Without this, one read in the window reports a dependency refusal that no read before or after it reports.
#[test]
fn a_hold_ended_between_lookup_and_validation_reads_as_the_review_expiring() {
    for ending in ["release", "purge"] {
        let fixture = Fixture::open();
        let broker = fixture.broker(1);
        // Publication requires a completed attempt on the receipt.
        fixture.attempt(1, Some(CuratorAttemptTerminal::Complete));
        let evidence = fixture.evidence_id();
        let Settled::Published(reference) = fixture
            .settle(
                &broker,
                RunResult::Proposal(Box::new(fixture.proposal(&[&evidence]))),
            )
            .unwrap()
        else {
            panic!("published")
        };
        let review = fixture.review_hold_binding(1, &reference.candidate_id);
        let hold = fixture
            .store
            .lookup_review_hold(&review, fixture.now + 6)
            .unwrap()
            .unwrap();
        assert!(fixture.read(fixture.now + 6).is_ok());
        let end_hold = || match ending {
            "release" => fixture
                .store
                .release_review_hold(&hold.hold_id, &review)
                .unwrap(),
            "purge" => {
                fixture
                    .store
                    .delete_artifact(kernel::ArtifactDeletionRequest {
                        intent: intent("purge-mid-read"),
                        identity: kernel::ArtifactDeletionIdentity::EvidenceId(evidence.clone()),
                        kind: kernel::ArtifactDeletionKind::Purge,
                        operator_id: Some("operator".to_string()),
                        target_locator: Some("incident://purge".to_string()),
                        reason: Some("retired".to_string()),
                        deleted_at: fixture.now + 7,
                    })
                    .unwrap();
            }
            other => unreachable!("{other}"),
        };
        let mid_read = read_selected_proposal_with_hook_for_test(
            &fixture.store,
            &fixture.ledger,
            PROJECT,
            &fixture.identity,
            &fixture.review_binding(),
            fixture.now + 7,
            &end_hold,
        );
        assert_eq!(
            mid_read.map(|selected| selected.reference),
            Err(ReadRefusal::ReviewExpired),
            "a {ending} between the lookup and the validation reads as the review expiring"
        );
        assert_eq!(
            fixture.read(fixture.now + 8),
            Err(ReadRefusal::ReviewExpired),
            "every later read reports the same refusal after a {ending}"
        );
    }
}
