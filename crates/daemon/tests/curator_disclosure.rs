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
    Disclosure, DisclosureApproval, DisclosureRefusal, ModelProfile, PreparedBody, prepare_body,
};
use daemon::curator::model_request::{ANTHROPIC_VERSION, MESSAGES_PATH, SendError};
use daemon::git_sources::{GitReadBounds, RepositoryBinding, read_selection};
use daemon::harness_sources::SourcePublisher;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CommitIntent, CuratorHoldBinding, Dimension, DomainSpec, KernelStore, ProviderEgress,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceDescriptorDetail,
};
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, EvidenceAvailability, ProducerBinding, ReserveOutcome,
    ReviewTarget,
};
use memory_store::curator_ledger::{
    CURATOR_MAX_ATTEMPTS, CURATOR_TASK_LEASE_MS, CuratorAttemptTerminal, CuratorBeginOutcome,
    CuratorLedgerRefusal,
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

/// Reserves, activates, claims, and begins one job whose review target is the staged candidate `candidate_id`; returns its causal identity and live claim.
fn open_job(
    ledger: &MemoryStore,
    registration: i64,
    kernel_id: &str,
    candidate_id: &str,
    now: i64,
) -> (String, String) {
    let target = ReviewTarget::StagedSubject {
        kernel_incarnation: kernel_id.to_string(),
        candidate_id: candidate_id.to_string(),
        payload_digest: "0d".repeat(32),
    };
    let producer = ProducerBinding {
        producer: "history-summarizer".to_string(),
        firing_id: format!("firing-{candidate_id}"),
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
            &format!("acq-{candidate_id}"),
            &format!("worker-{candidate_id}"),
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
            kernel_id,
            &claim.claim_id,
            now,
        )
        .unwrap()
    else {
        panic!("first claim begins")
    };
    (job.causal_identity, claim.claim_id)
}

/// A Kernel store holding one commit message published through the byte-verifying Git publisher (the only producer of remote-eligible evidence a Curator fixture may use), a Memory Store with a ready job under a live claim and receipt, and a remote-destination broker that has disclosed the commit.
struct Fixture {
    kernel_dir: tempfile::TempDir,
    ledger_dir: tempfile::TempDir,
    _repo_dir: tempfile::TempDir,
    store: Arc<KernelStore>,
    ledger: Arc<MemoryStore>,
    now: i64,
    registration: i64,
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
        let (identity, claim) = open_job(&ledger, registration, &kernel_id, "subject-1", now);
        Self {
            kernel_dir,
            ledger_dir,
            _repo_dir: repo_dir,
            store: Arc::new(store),
            ledger: Arc::new(ledger),
            now,
            registration,
            identity,
            claim,
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
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap();
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
        claim_id: &fixture.claim,
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
    assert_eq!(prepared.broker(), broker.id());

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
    // The header carries the secret of the credential the marker names, never the identifier.
    let head = observed.head.to_ascii_lowercase();
    assert!(head.contains("x-api-key: sk-cred-1\r\n"));
    assert!(!head.contains("x-api-key: cred-1"));
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
    let outcome = Disclosure {
        store: &fixture.store,
        ledger: &fixture.ledger,
        broker: &broker,
        sender: &sender,
        approval: Some(&approved),
        claim_id: "claim-stale",
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
async fn a_buffer_or_body_judged_under_another_broker_is_refused_before_any_connection() {
    let fixture = Fixture::open();
    let (remote, _) = fixture.broker();
    let (mut local, local_turn) = fixture.broker_for(kernel::ArtifactDestination::Local);
    // Both brokers share the job's one live hold and both issued `ref-1` for the same commit, so only the broker stamp can tell whose judgement admitted the bytes.
    let alias = local_turn[0].tag().alias.clone().unwrap();
    assert!(remote.aliases.resolve(alias.as_str()).is_ok());
    let system = remote.render_host_text("Extract facts.").unwrap();
    assert_eq!(
        prepare_body(&remote, &profile(), system, local_turn).unwrap_err(),
        DisclosureRefusal::BrokerMismatch,
        "a buffer judged for the local destination is not assembled under the remote broker"
    );
    // Buffers are consumed by assembly; the refused body took the local read with it, so the local body needs a fresh charged read.
    let reread = local
        .read(&fixture.store, alias.as_str(), None, fixture.now + 2)
        .unwrap()
        .buffer;
    let local_prepared = fixture.prepared(&local, vec![reread]);
    assert_eq!(local_prepared.broker(), local.id());
    assert_ne!(local.id(), remote.id());
    assert_eq!(local.hold_id(), remote.hold_id(), "one binding, one hold");
    let peer = Peer::start().await;
    let refusal = attempt(
        &fixture,
        &peer,
        &remote,
        &local_prepared,
        Some(&fixture.approval()),
        &move || fixture.now + 3,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal, DisclosureRefusal::BrokerMismatch);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
}

#[tokio::test]
async fn a_post_commit_lapse_stays_charged_and_sends_nothing() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // The clock jumps past the attempt deadline between the commit and the recheck: the post-commit barrier flips it while the ledger still owns its connection.
    let lapsed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip = Arc::clone(&lapsed);
    storage::after_commit_for_test(move |_| flip.store(true, Ordering::SeqCst));
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
                terminal_recorded: true,
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
async fn a_lapsed_attempt_whose_terminal_cannot_be_recorded_reports_it() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // The clock lapses past the claim and a successor takes the receipt after the commit: the recheck withholds the handoff, and the ledger then refuses the predecessor's `NotDispatched` terminal.
    let lapsed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip = Arc::clone(&lapsed);
    let ledger_path = fixture.ledger_dir.path().join("memory.sqlite");
    let identity = fixture.identity.clone();
    storage::after_commit_for_test(move |_| {
        flip.store(true, Ordering::SeqCst);
        rusqlite::Connection::open(ledger_path)
            .unwrap()
            .execute(
                "UPDATE curator_receipts SET claim_id = 'claim-successor'
                  WHERE project = ?1 AND causal_identity = ?2",
                rusqlite::params![PROJECT, identity],
            )
            .unwrap();
    });
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
    // The successor's takeover rebinds the receipt's claim, so the recheck sees a fence before it asks whether the old claim is live.
    assert_eq!(
        refusal,
        DisclosureRefusal::ChargedNotDispatched {
            attempt_index: 0,
            reason: CuratorLedgerRefusal::Fenced,
            terminal_recorded: false,
        }
    );
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1, "the marker stays charged");
    assert_eq!(
        attempts[0].terminal, None,
        "the attempt is left without a terminal"
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
    storage::after_commit_for_test(move |_| {
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
        claim_id: &fixture.claim,
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
    // A held Kernel would cost the foreground the whole stall and a held ledger the whole send deadline; the bound is the stall itself, with room for a slow runner's fsync on the foreground commit.
    assert!(
        foreground < stall,
        "both owners are released during the network wait: {foreground:?}"
    );
}

/// Like [`attempt`], with the caller's sender.
async fn attempt_with_sender(
    fixture: &Fixture,
    sender: &daemon::curator::model_request::Sender,
    broker: &EvidenceBroker,
    prepared: &PreparedBody,
    now: &(dyn Fn() -> i64 + Sync),
) -> Result<daemon::curator::disclosure::Disclosed, DisclosureRefusal> {
    Disclosure {
        store: &fixture.store,
        ledger: &fixture.ledger,
        broker,
        sender,
        approval: Some(&fixture.approval()),
        claim_id: &fixture.claim,
        now_ms: now,
    }
    .disclose(prepared, &CancellationToken::new(), deadline())
    .await
}

#[tokio::test]
async fn the_approval_names_the_credential_the_sender_presents() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // The approval covers `cred-1`; this sender writes another credential into the header. The identity is the sender's own, not a caller-supplied label, so nothing connects.
    let peer = Peer::start().await;
    let sender = peer.sender_with_credential("cred-2");
    assert_eq!(sender.credential_id(), "cred-2");
    let refusal = attempt_with_sender(&fixture, &sender, &broker, &prepared, &move || {
        fixture.now + 3
    })
    .await
    .unwrap_err();
    assert_eq!(refusal, DisclosureRefusal::Unavailable);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    assert!(fixture.attempts().is_empty());
}

#[tokio::test]
async fn the_attempt_is_charged_under_the_holds_job_while_another_job_is_live() {
    let fixture = Fixture::open();
    // A second job is ready, claimed, and begun under the same worker registration; the broker's hold names the first.
    let (other_identity, _) = open_job(
        &fixture.ledger,
        fixture.registration,
        &fixture.kernel_incarnation(),
        "subject-2",
        fixture.now,
    );
    assert_ne!(other_identity, fixture.identity);
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    assert_eq!(broker.binding().hold.subject, fixture.identity);
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    let disclosed = attempt(
        &fixture,
        &peer,
        &broker,
        &prepared,
        Some(&fixture.approval()),
        &move || fixture.now + 3,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(disclosed.attempt_index, 0);
    server.await.unwrap();
    assert_eq!(fixture.attempts().len(), 1, "the hold's job is charged");
    assert!(
        fixture
            .ledger
            .list_curator_attempts(PROJECT, &other_identity)
            .unwrap()
            .is_empty(),
        "no other job is charged"
    );
}

#[test]
fn the_assembled_prompt_is_render_checked_across_buffer_boundaries() {
    let fixture = Fixture::open();
    let (broker, _) = fixture.broker();
    // Each half passes the render check alone; only their concatenation is a key.
    let head = broker.render_host_text("token AKIAQ7RSTU").unwrap();
    let tail = broker.render_host_text("VWXYZ23456 more").unwrap();
    let system = broker.render_host_text("Extract facts.").unwrap();
    assert_eq!(
        prepare_body(&broker, &profile(), system, vec![head, tail]).unwrap_err(),
        DisclosureRefusal::RenderCheck
    );
    // The system text and the first turn buffer are scanned as one prompt too.
    let system = broker.render_host_text("token AKIAQ7RSTU").unwrap();
    let tail = broker.render_host_text("VWXYZ23456 more").unwrap();
    assert_eq!(
        prepare_body(&broker, &profile(), system, vec![tail]).unwrap_err(),
        DisclosureRefusal::RenderCheck
    );
}

#[tokio::test]
async fn a_revocation_during_the_handshake_is_refused_before_the_marker_commits() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    let mut peer = Peer::start().await;
    let server = peer.serve(no_wait(), |_| answer(MODEL));
    // Before the connection the clock is read twice, by the revalidation and by the guard. The third read is the first after the handshake; the descriptor is retired there, as if a Kernel write landed while the TLS handshake was in flight.
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let store = Arc::clone(&fixture.store);
    let object = fixture.source.0.clone();
    let now = fixture.now + 3;
    let clock = move || {
        if reads.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            store
                .commit(intent("retire-during-handshake"), |envelope| {
                    envelope.retire_observation(&object)?;
                    Ok(String::new())
                })
                .unwrap();
        }
        now
    };
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
    let DisclosureRefusal::Revalidation(refusal) = refusal else {
        panic!("{refusal:?}");
    };
    assert_eq!(refusal.code, RefusalCode::ExpectationChanged);
    assert_eq!(refusal.alias, prepared.tags()[1].alias);
    let observed = server.await.unwrap();
    assert_eq!(
        peer.connections.load(Ordering::SeqCst),
        1,
        "the handshake happened"
    );
    assert!(observed.head.is_empty(), "no request byte followed it");
    assert!(fixture.attempts().is_empty(), "no marker was committed");
}

#[tokio::test]
async fn the_committed_attempt_deadline_bounds_the_response_wait() {
    let fixture = Fixture::open();
    let (broker, turn) = fixture.broker();
    let prepared = fixture.prepared(&broker, turn);
    // The claim expires 700 ms after this clock, so the ledger commits the attempt with a 700 ms bound; the caller's deadline is 10 s and the peer answers after 1.5 s.
    let now = fixture.now + CURATOR_TASK_LEASE_MS - 700;
    let mut peer = Peer::start().await;
    let server = peer.serve(
        Box::new(|_, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            })
        }),
        |_| answer(MODEL),
    );
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
    assert_eq!(
        refusal,
        DisclosureRefusal::Send {
            attempt_index: Some(0),
            error: SendError::Deadline,
            sent: true,
        }
    );
    let observed = server.await.unwrap();
    assert!(!observed.reconnected, "the bounded attempt is not retried");
    let attempts = fixture.attempts();
    assert_eq!(attempts[0].attempt_deadline_ms, now + 700);
    assert_eq!(
        attempts[0].terminal.map(|(terminal, _)| terminal),
        Some(CuratorAttemptTerminal::Failed)
    );
}
