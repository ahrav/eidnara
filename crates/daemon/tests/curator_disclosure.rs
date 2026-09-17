//! Real-store proofs for the authorized, durably charged disclosure: the captured request bytes equal the provenance-tagged prepared body and its marker digest; commit failure, post-commit lapse, revocation, missing approval, cancellation, deadlines, and exhaustion send nothing; a provider failure or a mismatched model never triggers a second send; and the Kernel guard, marker commit, and handoff release both owners before the network wait, measured under a controlled commit stall.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant as StdInstant};

use daemon::curator::broker::{
    EvidenceBroker, OriginClass, QuestionTemplate, ReferenceExpectation, RefusalCode,
    RenderedBuffer, RunBinding,
};
use daemon::curator::disclosure::{
    AttemptBinding, Disclosure, DisclosureApproval, DisclosureRefusal, ModelProfile, PreparedBody,
    prepare_body,
};
use daemon::curator::model_request::{ANTHROPIC_VERSION, MESSAGES_PATH, SendError};
use daemon::git_sources::{GitReadBounds, RepositoryBinding, read_selection};
use daemon::harness_sources::SourcePublisher;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitIntent, CuratorHoldBinding, Dimension, DomainSpec, KernelStore, ProjectScope,
    ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity, SourceDescriptorDetail,
};
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, EvidenceAvailability, ProducerBinding, ReserveOutcome,
    ReviewTarget,
};
use memory_store::curator_ledger::{
    CURATOR_MAX_ATTEMPTS, CuratorAttemptTerminal, CuratorBeginOutcome, CuratorLedgerRefusal,
};
use memory_store::{LeaseAcquireOutcome, MemoryStore};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use support::tls_peer::{Peer, json_response, message, no_wait};

const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
/// A commit message with a trailing newline, retained exactly as the disclosed evidence.
const COMMIT_MESSAGE: &str = "Build the workspace with bun\n";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HOUR_MS: i64 = 60 * 60 * 1_000;
const MODEL: &str = "claude-canonical-1";
const CREDENTIAL_ID: &str = "cred-1";

/// The identity every local peer's sender reports: the dialed host, the API surface, and the version.
fn provider_identity() -> String {
    format!("localhost{MESSAGES_PATH}@{ANTHROPIC_VERSION}")
}

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
        producer: "curator-disclosure-test".to_string(),
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

/// Writes one commit into a fresh repository at `root` and publishes its message through the Git publisher as remote-eligible evidence; returns the descriptor's object id and detail.
fn publish_commit(
    store: &KernelStore,
    root: &std::path::Path,
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
        message: COMMIT_MESSAGE.into(),
        extra_headers: Vec::new(),
    };
    let oid = repo.write_object(&commit).unwrap().detach().to_string();
    let selection = read_selection(
        &support::projection_gate::open_gate(),
        &RepositoryBinding {
            repository_id: "repo-fixture".to_string(),
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

/// A Kernel store holding one commit message published through the byte-verifying Git publisher (the only producer of remote-eligible evidence a Curator fixture may use), a Memory Store with a ready job under a live claim and receipt, and a remote-destination broker that has disclosed the commit.
struct Fixture {
    kernel_dir: tempfile::TempDir,
    _ledger_dir: tempfile::TempDir,
    _repo_dir: tempfile::TempDir,
    store: Arc<KernelStore>,
    ledger: Arc<MemoryStore>,
    now: i64,
    identity: String,
    claim: String,
    /// The published descriptor's object id and detail.
    source: (String, SourceDescriptorDetail),
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
        let source = publish_commit(&store, repo_dir.path(), now);
        let ledger_dir = tempfile::tempdir().unwrap();
        let ledger = MemoryStore::open(&MemoryStore::test_descriptor(
            ledger_dir.path(),
            "eidnara-curator-disclosure-test",
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
        let CuratorBeginOutcome::Begun(_) = ledger
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
            _ledger_dir: ledger_dir,
            _repo_dir: repo_dir,
            store: Arc::new(store),
            ledger: Arc::new(ledger),
            now,
            identity: job.causal_identity,
            claim: claim.claim_id,
            source,
        }
    }

    fn kernel_incarnation(&self) -> String {
        kernel_incarnation(self.kernel_dir.path())
    }

    fn memstore_incarnation(&self) -> String {
        self.ledger.curator_store_incarnation().unwrap()
    }

    fn hold_binding(&self) -> CuratorHoldBinding {
        CuratorHoldBinding {
            project_digest: PROJECT.to_string(),
            kernel_incarnation: self.kernel_incarnation(),
            memstore_incarnation: self.memstore_incarnation(),
            subject: self.identity.clone(),
            generation: 1,
        }
    }

    fn source_expectation(&self) -> ReferenceExpectation {
        let (object_id, detail) = &self.source;
        ReferenceExpectation::NativeSource {
            object_id: object_id.clone(),
            class: OccurrenceClass::GitCommits,
            source_revision: detail.revision.parse().unwrap(),
            artifact_digest: detail.artifact_digest.clone(),
            evidence_id: detail.evidence_id.clone(),
            occurrence_tuple: detail.occurrence_tuple.clone(),
        }
    }

    /// A remote-destination broker that has disclosed the published commit, with its rendered buffer.
    fn broker(&self) -> (EvidenceBroker, Vec<RenderedBuffer>) {
        self.broker_for(kernel::ArtifactDestination::Remote)
    }

    fn broker_for(
        &self,
        destination: kernel::ArtifactDestination,
    ) -> (EvidenceBroker, Vec<RenderedBuffer>) {
        let binding = self.hold_binding();
        let hold = self
            .store
            .acquire_execution_hold(
                &binding,
                std::slice::from_ref(&self.source.1.evidence_id),
                self.now + 2 * HOUR_MS,
            )
            .unwrap();
        let mut broker = EvidenceBroker::new(
            RunBinding {
                project: ProjectScope::new(PROJECT).unwrap(),
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        );
        let commit = broker.aliases.issue(self.source_expectation());
        let commit = broker
            .read(&self.store, commit.as_str(), None, self.now + 2)
            .unwrap()
            .buffer;
        (broker, vec![commit])
    }

    fn prepared(&self, broker: &EvidenceBroker, turn: Vec<RenderedBuffer>) -> PreparedBody {
        let system = broker.render_host_text("Extract facts.").unwrap();
        prepare_body(broker, &profile(), system, turn).unwrap()
    }

    fn approval(&self) -> DisclosureApproval {
        DisclosureApproval {
            provider: provider_identity(),
            model: MODEL.to_string(),
            credential_id: CREDENTIAL_ID.to_string(),
            kernel_incarnation: self.kernel_incarnation(),
            memstore_incarnation: self.memstore_incarnation(),
        }
    }

    fn binding(&self) -> AttemptBinding {
        AttemptBinding {
            project: PROJECT.to_string(),
            causal_identity: self.identity.clone(),
            generation: 1,
            claim_id: self.claim.clone(),
            credential_id: CREDENTIAL_ID.to_string(),
        }
    }

    fn attempts(&self) -> Vec<memory_store::curator_ledger::CuratorAttempt> {
        self.ledger
            .list_curator_attempts(PROJECT, &self.identity)
            .unwrap()
    }
}

fn profile() -> ModelProfile {
    ModelProfile {
        model: MODEL.to_string(),
        max_tokens: 256,
        temperature: None,
    }
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn answer(model: &str) -> Vec<u8> {
    json_response(
        "200 OK",
        &message("the workspace builds with bun")
            .replace(r#""model":"claude""#, &format!(r#""model":"{model}""#)),
        "",
    )
}

/// One attempt against `peer` with the fixture's stores, broker, approval, and a clock that `advance` may move.
async fn attempt(
    fixture: &Fixture,
    peer: &Peer,
    broker: &EvidenceBroker,
    prepared: &PreparedBody,
    approval: Option<&DisclosureApproval>,
    now: &(dyn Fn() -> i64 + Sync),
    cancel: &CancellationToken,
) -> Result<daemon::curator::disclosure::Disclosed, DisclosureRefusal> {
    let sender = peer.sender_with_credential(CREDENTIAL_ID);
    Disclosure {
        store: &fixture.store,
        ledger: &fixture.ledger,
        broker,
        sender: &sender,
        approval,
        binding: &fixture.binding(),
        now_ms: now,
    }
    .disclose(prepared, cancel, deadline())
    .await
}

#[tokio::test]
async fn the_captured_request_is_the_tagged_prepared_body_the_marker_binds() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // Every byte of the prompt is covered by exactly one tag, in order, and the union names both disclosed inputs.
    let mut offset = 0;
    for tag in prepared.tags() {
        assert_eq!(tag.prompt_range.start, offset);
        offset = tag.prompt_range.end;
    }
    assert_eq!(prepared.tags()[0].origin, OriginClass::HostAuthored);
    assert_eq!(prepared.tags()[1].origin, OriginClass::NativeSource);
    assert_eq!(prepared.tags().len(), 2);
    assert_eq!(offset, "Extract facts.".len() + COMMIT_MESSAGE.len());
    assert!(prepared.policy_union_canonical().contains("native_source"));

    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    let now = fixture.now + 3;
    let disclosed = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &move || now,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let observed = server.await.unwrap();
    assert_eq!(
        observed.body,
        prepared.body(),
        "the wire body is the prepared body"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&observed.body)),
        prepared.body_digest()
    );
    let wire: serde_json::Value = serde_json::from_slice(&observed.body).unwrap();
    assert_eq!(wire["model"], MODEL);
    assert_eq!(wire["max_tokens"], 256);
    assert!(
        wire.get("temperature").is_none(),
        "the sampling omission is recorded as an omission"
    );
    assert_eq!(wire["system"], "Extract facts.");
    assert!(
        wire["messages"][0]["content"]
            .as_str()
            .unwrap()
            .ends_with(COMMIT_MESSAGE)
    );
    assert!(
        observed
            .head
            .to_ascii_lowercase()
            .contains("x-api-key: cred-1")
    );
    assert!(!observed.reconnected);
    assert_eq!(disclosed.text.text, "the workspace builds with bun");
    assert_eq!(disclosed.attempt_index, 0);
    // The committed marker binds the digest, size, provider profile, credential, and union of exactly this body.
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1);
    let marker = &attempts[0].marker;
    assert_eq!(marker.body_digest, prepared.body_digest());
    assert_eq!(marker.request_bytes, prepared.body().len() as u64);
    assert_eq!(
        marker.provider,
        peer.sender_with_credential(CREDENTIAL_ID)
            .provider_identity()
    );
    assert_eq!(marker.provider, provider_identity());
    assert_eq!(marker.model, MODEL);
    assert_eq!(marker.credential_id, CREDENTIAL_ID);
    assert_eq!(marker.policy_union_digest, prepared.policy_union_digest());
    assert_eq!(
        attempts[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Complete)
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nothing_is_sent_without_approval_under_cancellation_or_when_the_marker_cannot_commit() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    let now = fixture.now + 3;
    let clock = move || now;
    let fresh = CancellationToken::new();
    // Missing approval: unavailable before any connection.
    let peer = Peer::start().await;
    assert_eq!(
        attempt(&fixture, &peer, &broker, &prepared, None, &clock, &fresh)
            .await
            .unwrap_err(),
        DisclosureRefusal::Unavailable
    );
    // An approval that differs in any one of its five terms is no approval.
    let approved = fixture.approval();
    type Mutate = fn(&mut DisclosureApproval);
    let mismatches: [(&str, Mutate); 5] = [
        ("provider", |approval| {
            approval.provider = format!("api.anthropic.com{MESSAGES_PATH}@{ANTHROPIC_VERSION}");
        }),
        ("model", |approval| {
            approval.model = "claude-other".to_string()
        }),
        ("credential", |approval| {
            approval.credential_id = "cred-2".to_string();
        }),
        ("kernel incarnation", |approval| {
            approval.kernel_incarnation = "x".repeat(32);
        }),
        ("memstore incarnation", |approval| {
            approval.memstore_incarnation = "x".repeat(32);
        }),
    ];
    for (term, mutate) in mismatches {
        let mut stale = approved.clone();
        mutate(&mut stale);
        assert_ne!(stale, approved, "{term}");
        assert_eq!(
            attempt(
                &fixture,
                &peer,
                &broker,
                &prepared,
                Some(&stale),
                &clock,
                &fresh
            )
            .await
            .unwrap_err(),
            DisclosureRefusal::Unavailable,
            "{term}"
        );
    }
    // A broker whose disclosures were judged for the local destination never reaches the provider.
    let (local, local_turn) = fixture.broker_for(kernel::ArtifactDestination::Local);
    let local_prepared = fixture.prepared(&local, local_turn);
    assert_eq!(
        attempt(
            &fixture,
            &peer,
            &local,
            &local_prepared,
            Some(&approved),
            &clock,
            &fresh
        )
        .await
        .unwrap_err(),
        DisclosureRefusal::DestinationNotRemote
    );
    // Cancellation before the attempt commits nothing.
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        attempt(
            &fixture,
            &peer,
            &broker,
            &prepared,
            Some(&approved),
            &clock,
            &cancelled
        )
        .await
        .unwrap_err(),
        DisclosureRefusal::Cancelled
    );
    assert_eq!(
        peer.connections.load(Ordering::SeqCst),
        0,
        "no connection was opened"
    );
    assert!(fixture.attempts().is_empty());
    // A marker that cannot commit sends nothing: a stale claim is refused after the handshake and the connection carries no request byte.
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    let sender = peer.sender_with_credential(CREDENTIAL_ID);
    let mut stale_binding = fixture.binding();
    stale_binding.claim_id = "claim-stale".to_string();
    let outcome = Disclosure {
        store: &fixture.store,
        ledger: &fixture.ledger,
        broker: &broker,
        sender: &sender,
        approval: Some(&approved),
        binding: &stale_binding,
        now_ms: &clock,
    }
    .disclose(&prepared, &fresh, deadline())
    .await
    .unwrap_err();
    assert_eq!(
        outcome,
        DisclosureRefusal::Ledger(CuratorLedgerRefusal::ClaimInvalid)
    );
    let observed = server.await.unwrap();
    assert!(observed.head.is_empty(), "no request reached the peer");
    assert!(
        fixture.attempts().is_empty(),
        "a failed commit charges nothing"
    );
}

#[tokio::test]
async fn a_post_commit_lapse_stays_charged_and_sends_nothing() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // The clock jumps past the attempt deadline between the commit and the recheck: the post-commit barrier flips it while the ledger still owns its connection.
    let lapsed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip = Arc::clone(&lapsed);
    storage::after_commit_for_test(move || flip.store(true, Ordering::SeqCst));
    let base = fixture.now + 3;
    let clock = move || {
        if lapsed.load(Ordering::SeqCst) {
            base + 2 * HOUR_MS
        } else {
            base
        }
    };
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    let refusal = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &clock,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            refusal,
            DisclosureRefusal::ChargedNotDispatched {
                attempt_index: 0,
                ..
            }
        ),
        "{refusal:?}"
    );
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1, "the marker stays charged");
    assert_eq!(
        attempts[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::NotDispatched)
    );
    let observed = server.await.unwrap();
    assert!(observed.head.is_empty(), "nothing was sent");
}

#[tokio::test]
async fn revoking_a_disclosed_input_prevents_the_handoff_and_commits_no_marker() {
    let fixture = Fixture::open();
    let peer = Peer::start().await;
    // An expired execution hold refuses at the Kernel guard even though the reference itself still passes.
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    let expired = fixture.now + 2 * HOUR_MS + 1;
    let refusal = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &move || expired,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal, DisclosureRefusal::Hold(RefusalCode::HoldInvalid));
    // The descriptor is retired after it was rendered into the body; the revalidation refuses before any connection is opened and before the ledger is entered.
    fixture
        .store
        .commit(intent("retire-source"), |envelope| {
            envelope.retire_observation(&fixture.source.0)?;
            Ok(String::new())
        })
        .unwrap();
    let now = fixture.now + 3;
    let refusal = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &move || now,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    let DisclosureRefusal::Revalidation(refusal) = refusal else {
        panic!("{refusal:?}");
    };
    assert_eq!(refusal.code, RefusalCode::ExpectationChanged);
    assert_eq!(refusal.alias, prepared.tags()[1].alias);
    assert!(fixture.attempts().is_empty(), "no marker was committed");
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_failure_and_a_mismatched_model_end_the_attempt_without_a_second_send() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    let now = fixture.now + 3;
    let clock = move || now;
    let fresh = CancellationToken::new();
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| {
        json_response(
            "529 Overloaded",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
            "",
        )
    });
    let refusal = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &clock,
        &fresh,
    )
    .await
    .unwrap_err();
    assert_eq!(
        refusal,
        DisclosureRefusal::Send {
            attempt_index: Some(0),
            error: SendError::Status(529),
            sent: true,
        }
    );
    let observed = server.await.unwrap();
    assert!(
        !observed.reconnected,
        "a provider failure never triggers another send"
    );
    assert_eq!(peer.connections.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.attempts()[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Failed)
    );
    // A success under another model than the one requested is withheld and the attempt fails.
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer("claude-substitute"));
    let refusal = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &clock,
        &fresh,
    )
    .await
    .unwrap_err();
    assert_eq!(
        refusal,
        DisclosureRefusal::ModelMismatch { attempt_index: 1 }
    );
    server.await.unwrap();
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[1].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Failed)
    );
    // Every further physical request is a fresh validation and a fresh charged attempt until the allowance is spent.
    let allowance = usize::try_from(CURATOR_MAX_ATTEMPTS).unwrap();
    for index in 2..allowance {
        let mut peer = Peer::start().await;
        let server = peer.serve(no_wait(), |_| answer(MODEL));
        let disclosed = attempt(
            &fixture,
            &peer,
            &broker,
            &prepared,
            Some(&fixture.approval()),
            &clock,
            &fresh,
        )
        .await
        .unwrap();
        assert_eq!(disclosed.attempt_index, u32::try_from(index).unwrap());
        server.await.unwrap();
    }
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    assert_eq!(
        attempt(
            &fixture,
            &peer,
            &broker,
            &prepared,
            Some(&fixture.approval()),
            &clock,
            &fresh
        )
        .await
        .unwrap_err(),
        DisclosureRefusal::Ledger(CuratorLedgerRefusal::AttemptsExhausted)
    );
    assert!(
        server.await.unwrap().head.is_empty(),
        "a refused fifth attempt sends nothing"
    );
    assert_eq!(fixture.attempts().len(), allowance);
}

#[tokio::test]
async fn the_network_wait_begins_only_after_both_owners_release() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    let now = fixture.now + 3;
    // The peer answers only when released; foreground Kernel and ledger work must proceed while the send waits on it.
    let mut peer = Peer::start().await;
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = peer.serve(
        Box::new(move |_, _| {
            Box::pin(async move {
                release_rx.await.ok();
            })
        }),
        |_| answer(MODEL),
    );
    // A controlled stall after the marker commits, while the ledger still owns its connection. The hook releases a probe thread, waits until the probe is about to read the ledger, then stalls; the probe's read therefore waits for the whole stall if and only if the connection is still held. A Kernel read inside the stall shows the Kernel is not held.
    let stall = Duration::from_millis(400);
    let (stall_tx, stall_rx) = std::sync::mpsc::channel::<Duration>();
    let (probe_go_tx, probe_go_rx) = std::sync::mpsc::channel::<()>();
    let (probe_ready_tx, probe_ready_rx) = std::sync::mpsc::channel::<()>();
    let kernel_in_stall = Arc::clone(&fixture.store);
    storage::after_commit_for_test(move || {
        probe_go_tx.send(()).unwrap();
        probe_ready_rx.recv().unwrap();
        let read_started = StdInstant::now();
        kernel_in_stall.tip().unwrap();
        let kernel_read_wait = read_started.elapsed();
        std::thread::sleep(stall);
        stall_tx.send(kernel_read_wait).unwrap();
    });
    let ledger_probe = {
        let ledger = Arc::clone(&fixture.ledger);
        let identity = fixture.identity.clone();
        std::thread::spawn(move || {
            probe_go_rx.recv().unwrap();
            let started = StdInstant::now();
            probe_ready_tx.send(()).unwrap();
            let count = ledger
                .list_curator_attempts(PROJECT, &identity)
                .unwrap()
                .len();
            (count, started.elapsed())
        })
    };
    let sender = peer.sender_with_credential(CREDENTIAL_ID);
    let disclosure = Disclosure {
        store: &fixture.store,
        ledger: &fixture.ledger,
        broker: &broker,
        sender: &sender,
        approval: Some(&fixture.approval()),
        binding: &fixture.binding(),
        now_ms: &move || now,
    };
    let cancel = CancellationToken::new();
    let disclosing = disclosure.disclose(&prepared, &cancel, deadline());
    tokio::pin!(disclosing);
    let stall_observed = tokio::task::spawn_blocking(move || stall_rx.recv().unwrap());
    let kernel_read_wait = tokio::select! {
        biased;
        measured = stall_observed => measured.unwrap(),
        outcome = &mut disclosing => panic!("the send finished before the stall was observed: {outcome:?}"),
    };
    // Both owners have released and the send is waiting on the stalled peer; foreground Kernel writes and ledger reads complete meanwhile.
    let foreground_started = StdInstant::now();
    let tip_before = fixture.store.tip().unwrap();
    fixture
        .store
        .commit(intent("foreground-write"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "foreground".to_string(),
                object_id: "foreground-object".to_string(),
                name: "foreground".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "foreground".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    assert!(fixture.store.tip().unwrap() > tip_before);
    assert_eq!(
        fixture.attempts().len(),
        1,
        "the marker is committed while the network wait is pending"
    );
    let foreground = foreground_started.elapsed();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut disclosing)
            .await
            .is_err(),
        "the send is still waiting on the stalled peer"
    );
    release_tx.send(()).unwrap();
    let disclosed = disclosing.await.unwrap();
    assert_eq!(disclosed.attempt_index, 0);
    server.await.unwrap();
    let (ledger_count, ledger_wait) = ledger_probe.join().unwrap();
    assert_eq!(ledger_count, 1, "the probe read the committed marker");
    eprintln!(
        "marker-commit stall {stall:?}: kernel read inside the stall waited {kernel_read_wait:?}; ledger read issued inside the stall waited {ledger_wait:?}; foreground kernel write and ledger read after release took {foreground:?}"
    );
    assert!(
        kernel_read_wait < stall / 4,
        "a Kernel read is not blocked by the ledger's connection: {kernel_read_wait:?}"
    );
    assert!(
        ledger_wait >= stall,
        "a ledger read issued inside the stall waited for the connection: {ledger_wait:?}"
    );
    assert!(
        foreground < stall / 4,
        "both owners are released during the network wait: {foreground:?}"
    );
}
