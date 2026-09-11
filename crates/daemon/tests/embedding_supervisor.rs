//! The embedding maintenance supervisor against a real kernel, projection, and in-process Synapse host.
//! One budget bounds every stage of a pass and stops it at its deadline or cancellation; slices alternate so a sweep runs while backfill work is held; shutdown joins its slices, keeps native work owned until it exits, reports panics, and leaves durable unfinished work for the next incarnation.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use daemon::embedding_dispatch::{
    Blocked, DispatchBounds, DispatchEvent, EmbeddingDispatcher, Stage,
};
use daemon::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, SliceKind, SliceOutcome, Stop, SupervisorEvent,
    Unresolved,
};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    EmbedTokens, EmbeddingEngine, LaneInfo, SynapseComponent, SynapseLimits,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, ArtifactIngestRequest, CommitIntent,
    Dimension, DomainSpec, EligibilityBinding, EventKind, ExportWindow, KernelStore, ProjectScope,
    ProviderEgress, RepositoryProvenance, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass,
    SourceDescriptorRequest, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds,
    SourcePageBounds, SourceRow, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::dispatch::EpisodeGrant;
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const CONSUMER: &str = "search";
const KERNEL_INCARNATION: &str = "kernel-1";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const DIMS: usize = 8;
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;

type Gate = Arc<(Mutex<bool>, Condvar)>;

/// A deterministic engine: one whitespace word is one token, the vector is derived from the text, and inference can be held at a gate; `completed` counts inferences that returned.
struct TestEngine {
    calls: AtomicUsize,
    completed: AtomicUsize,
    gate: Mutex<Option<Gate>>,
}

impl TestEngine {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    fn block_calls(&self) -> Gate {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        *self.gate.lock().unwrap() = Some(Arc::clone(&gate));
        gate
    }

    fn release(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    fn vector_for(text: &str) -> Vec<f32> {
        let digest = Sha256::digest(text.as_bytes());
        let mut vector: Vec<f32> = digest
            .iter()
            .take(DIMS)
            .map(|b| f32::from(*b) + 1.0)
            .collect();
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        for v in &mut vector {
            *v /= norm;
        }
        vector
    }
}

impl EmbeddingEngine for TestEngine {
    fn untruncated_token_len(&self, text: &str) -> Result<EmbedTokens, InferenceError> {
        Ok(EmbedTokens::new(text.split_whitespace().count() as u32))
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.gate.lock().unwrap().clone() {
            let mut released = gate.0.lock().unwrap();
            while !*released {
                released = gate.1.wait(released).unwrap();
            }
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(texts.iter().map(|text| Self::vector_for(text)).collect())
    }
}

fn lane(fingerprint: &str) -> LaneInfo {
    LaneInfo {
        model: MODEL.to_owned(),
        fingerprint: fingerprint.to_owned(),
        table_epoch: 1,
        dims: DIMS,
        execution_provider: "cpu",
        max_tokens: 512,
        max_text_bytes: 1024 * 1024,
        provenance: serde_json::json!({"source": "deterministic test engine"}),
        recommended_rows: 16,
        recommended_token_budget: 8192,
    }
}

fn component(engine: &Arc<TestEngine>, limits: SynapseLimits) -> SynapseComponent {
    SynapseComponent::ready_with_engine(
        lane(FINGERPRINT),
        Arc::clone(engine) as Arc<dyn EmbeddingEngine>,
        limits,
    )
    .unwrap()
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS as u32,
        generation_epoch: 1,
    }
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS as u32,
        generation_epoch: 1,
    }
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn grant(allowance: u32, deadline: i64) -> EpisodeGrant {
    EpisodeGrant {
        allowance: NonZeroU32::new(allowance).unwrap(),
        deadline,
    }
}

fn bounds() -> DispatchBounds {
    DispatchBounds {
        max_jobs: NonZeroUsize::new(16).unwrap(),
        grant: grant(3, NOW + DAY_MS),
        retry_after: 10,
        result_wait: Duration::from_secs(5),
    }
}

/// One pass budget: an absolute deadline `wait` from now, with its own sticky cancellation.
fn budget(wait: Duration) -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + wait),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

fn eligibility(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Remote,
    }
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-embedding-dispatch-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

/// A kernel with one scoped, admitted project and the descriptors the test publishes into it.
struct Corpus {
    kernel: Arc<KernelStore>,
}

impl Corpus {
    fn open(root: &Path) -> Self {
        Self {
            kernel: Arc::new(KernelStore::open(root.join("kernel")).unwrap()),
        }
    }

    fn seed(&self) {
        self.kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "Name".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: "domain".to_string(),
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
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    /// Publishes one scoped, admitted message descriptor and returns its object id.
    fn publish(&self, key: &str, text: &str) -> String {
        let handle = self
            .kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: "domain".to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        let mut object = String::new();
        self.kernel
            .commit(intent(&format!("publish-{key}")), |envelope| {
                let identity = [
                    ("project_id", "proj-a"),
                    ("harness", "opencode"),
                    ("session_id", "sess-01"),
                    ("message_id", key),
                    ("block_index", "0"),
                ];
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        occurrence: Occurrence {
                            class: "messages",
                            identity: &identity,
                            revision: "1",
                            representation: "text",
                            span: None,
                        },
                        domain_id: "domain",
                        scope_id: Some(SCOPE),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(outcome.object_id.clone()),
                    source_class: Some(SourceClass::ExplicitUser),
                    taint_class: Some(TaintClass::UserExplicit),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "test".to_string(),
                    },
                })?;
                object = outcome.object_id;
                Ok(String::new())
            })
            .unwrap();
        object
    }

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    /// Exports every descriptor live at a fresh S.
    fn export(&self) -> Vec<SourceRow> {
        let binding = self.binding();
        let hold = self
            .kernel
            .capture_source_hold(
                &binding,
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: SourceHoldAdmission {
                        max_references: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                    },
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap();
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .kernel
                .export_source_page(
                    &binding,
                    &hold.hold_id,
                    hold.captured_at,
                    ExportWindow::Snapshot,
                    cursor.as_ref(),
                    SourcePageBounds {
                        max_rows: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
                    },
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        self.kernel
            .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
            .unwrap();
        rows
    }

    /// Builds the projection from the current export and queues one pending job per message.
    fn bootstrap(&self, data_home: &Path) -> (SearchProjection, Vec<SourceRow>) {
        let rows = self.export();
        let projection = SearchProjection::open(data_home).unwrap();
        projection
            .write(|conn| {
                install_identity(conn, &identity(), 1)?;
                register_generation(conn, &generation(), 1)?;
                Ok(())
            })
            .unwrap();
        let identities = row_identities(&rows);
        let snapshot = self.tip();
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
                hold_id: "0123456789abcdef0123456789abcdef".to_string(),
                snapshot_commit_seq: snapshot,
                through_commit_seq: snapshot,
            },
            Some(GENERATION),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
        (projection, rows)
    }
}

fn occurrence_of<'a>(rows: &'a [SourceRow], object_id: &str) -> &'a str {
    &rows
        .iter()
        .find(|row| row.object_id == object_id)
        .unwrap()
        .detail
        .occurrence_id
}

fn search_path(data_home: &Path) -> PathBuf {
    data_home.join("search").join("search.sqlite")
}

fn inspect(data_home: &Path) -> Connection {
    Connection::open_with_flags(search_path(data_home), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

/// The durable `(state, attempts, host_job_id)` of one occurrence's job, read outside every API under test.
fn row(data_home: &Path, occurrence: &str) -> (String, u32, Option<String>) {
    inspect(data_home)
        .query_row(
            "SELECT state,attempts,host_job_id FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

fn stages(events: &[DispatchEvent]) -> Vec<(String, Stage, Option<Instant>)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Stage {
                job_id,
                stage,
                deadline,
            } => Some((job_id.clone(), *stage, *deadline)),
            _ => None,
        })
        .collect()
}

/// AC1: every stage of every job under one pass observes the budget's own deadline, and a budget already exhausted admits nothing. A renewed deadline would appear as a different instant in a stage event and fail the equality.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_budget_bounds_every_stage_of_every_job() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<String> = ["a", "b"]
        .iter()
        .map(|name| corpus.publish(name, &format!("{name} text")))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();

    // An exhausted budget admits nothing and runs no inference.
    let spent = EvalBudget::new(
        Some(Instant::now() - Duration::from_millis(1)),
        Arc::new(AtomicBool::new(false)),
    );
    let mut events = Vec::new();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), &bounds(), &spent, NOW, &mut |e| {
                events.push(e)
            })
            .unwrap()
    });
    assert_eq!(end, Some(Blocked::BudgetExhausted));
    assert!(stages(&events).is_empty(), "{events:?}");
    assert_eq!(engine.calls(), 0);
    for object in &objects {
        assert_eq!(row(dir.path(), occurrence_of(&rows, object)).0, "pending");
    }

    // A live budget: every stage of both jobs carries the same deadline, in order.
    let live = budget(Duration::from_secs(5));
    let mut events = Vec::new();
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), &bounds(), &live, NOW, &mut |e| {
                events.push(e)
            })
            .unwrap()
    });
    assert_eq!(end, None);
    let observed = stages(&events);
    assert_eq!(observed.len(), 6);
    assert!(
        observed
            .iter()
            .all(|(_, _, deadline)| *deadline == live.deadline())
    );
    for object in &objects {
        let job = row(dir.path(), occurrence_of(&rows, object));
        let own: Vec<Stage> = observed
            .iter()
            .filter(|(id, _, _)| {
                inspect(dir.path())
                    .query_row(
                        "SELECT job_id FROM embedding_jobs WHERE occurrence_id=?1",
                        [occurrence_of(&rows, object)],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap()
                    == *id
            })
            .map(|(_, stage, _)| *stage)
            .collect();
        assert_eq!(own, vec![Stage::Admit, Stage::Poll, Stage::Publish]);
        assert_eq!(job.0, "embedded");
    }
}

/// AC1, AC2: a budget cancelled mid-poll stops the pass at once, before its deadline; the host keeps running the admitted call, the row stays admitted with its charge, and the next pass finishes it without a second inference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sticky_cancellation_stops_the_pass_and_keeps_native_work_owned() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("c", "c text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let gate = engine.block_calls();
    let cancelled = budget(Duration::from_secs(30));
    let canceller = cancelled.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel();
    });
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let started = Instant::now();
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &cancelled,
                NOW,
                &mut |_| {},
            )
            .unwrap()
    });
    assert_eq!(end, Some(Blocked::BudgetExhausted));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancellation, not the deadline, ended the pass"
    );
    let held = row(dir.path(), occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    assert_eq!(
        synapse.job_status(held.2.as_deref().unwrap()),
        Some("running")
    );
    TestEngine::release(&gate);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(),
                &budget(Duration::from_secs(5)),
                NOW,
                &mut |_| {},
            )
            .unwrap()
    });
    assert_eq!(end, None);
    assert_eq!(row(dir.path(), occurrence).0, "embedded");
    assert_eq!(
        engine.calls(),
        1,
        "the held call was finished, not repeated"
    );
}

fn slice_bounds(slice: Duration) -> SliceBounds {
    SliceBounds {
        dispatch: bounds(),
        sweep_candidates: NonZeroUsize::new(16).unwrap(),
        slice,
        idle: Duration::from_millis(20),
    }
}

fn maintained(
    corpus: &Corpus,
    projection: Arc<SearchProjection>,
    synapse: Arc<SynapseComponent>,
) -> Maintained {
    Maintained {
        kernel: Arc::clone(&corpus.kernel),
        projection,
        synapse,
        project: ProjectScope::new(PROJECT).unwrap(),
        destination: ArtifactDestination::Remote,
    }
}

async fn next_event(events: &mut UnboundedReceiver<SupervisorEvent>) -> SupervisorEvent {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("an event within ten seconds")
        .expect("the supervisor is alive")
}

/// AC2, AC3, AC4: slices alternate under their own budgets while native work is held; shutdown joins its slices but stays unresolved while the host still runs an admitted call, repeated requests neither duplicate work nor release anything, and once the call exits shutdown resolves with the row still admitted for the next incarnation, which finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn shutdown_joins_slices_and_stays_unresolved_while_native_work_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("held", "held text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let projection = Arc::new(projection);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), Arc::clone(&synapse)),
        slice_bounds(Duration::from_secs(2)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // Backfill admits the job, waits for its result until the slice budget ends, and yields; a sweep slice then runs even though backfill work is still held.
    let started_at = Instant::now();
    let SupervisorEvent::SliceStarted {
        kind: SliceKind::Backfill,
        deadline,
    } = next_event(&mut events).await
    else {
        panic!()
    };
    assert!(deadline <= started_at + Duration::from_secs(2) + Duration::from_millis(100));
    match next_event(&mut events).await {
        SupervisorEvent::SliceEnded {
            kind: SliceKind::Backfill,
            outcome:
                SliceOutcome::Backfill {
                    end,
                    admitted,
                    published,
                },
        } => {
            assert_eq!(
                (end, admitted, published),
                (Some(Blocked::BudgetExhausted), 1, 0)
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceEnded {
            kind: SliceKind::Sweep,
            ..
        }
    ));
    let held = row(dir.path(), &occurrence);
    assert_eq!((held.0.as_str(), held.1), ("admitted", 1));
    let host_job = held.2.clone().unwrap();
    assert_eq!(synapse.job_status(&host_job), Some("running"));

    // Shutdown joins the slice threads but the native call is still owned: unresolved, twice, with no second inference and nothing released.
    let first = supervisor.shutdown(Duration::from_secs(2)).await;
    assert_eq!(
        first,
        Err(Unresolved {
            slices: 0,
            native: 1
        })
    );
    let again = supervisor.shutdown(Duration::from_millis(100)).await;
    assert_eq!(
        again,
        Err(Unresolved {
            slices: 0,
            native: 1
        })
    );
    assert_eq!(engine.calls(), 1);
    assert_eq!(synapse.job_status(&host_job), Some("running"));
    assert_eq!(
        row(dir.path(), &occurrence),
        held,
        "cancellation preserves unfinished pending work"
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    let mut saw_stop = false;
    while let Ok(event) = events.try_recv() {
        if event == SupervisorEvent::Stopped(Stop::Shutdown) {
            saw_stop = true;
        }
    }
    assert!(saw_stop);

    // Native exit resolves the drain; the result is a held lease, not a published vector.
    TestEngine::release(&gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while synapse.job_status(&host_job) == Some("running") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.completed(), 1, "the native call exited");
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.held_results, 1);
    assert!(report.slices >= 2);
    assert_eq!(row(dir.path(), &occurrence).0, "admitted");

    // The next incarnation reconciles the admitted row and finishes it.
    drop(supervisor);
    let next = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let restarted = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::clone(&projection), next),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&restarted).run());
    loop {
        match next_event(&mut events).await {
            SupervisorEvent::SliceEnded {
                outcome:
                    SliceOutcome::Backfill {
                        published: 1,
                        admitted: 1,
                        end: None,
                    },
                ..
            } => break,
            SupervisorEvent::Stopped(stop) => panic!("{stop:?}"),
            _ => {}
        }
    }
    let done = row(dir.path(), &occurrence);
    assert_eq!((done.0.as_str(), done.1), ("embedded", 2));
    let report = restarted.shutdown(Duration::from_secs(5)).await.unwrap();
    assert_eq!(report.held_results, 0);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 2);
}

/// AC3: a slice that panics is reported with its payload, stops the supervisor, and is joined by shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_slice_is_reported_and_stops_the_supervisor() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(5)),
        Arc::new(|| NOW),
        sender,
    );
    supervisor.panic_next_slice_for_test();
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted { .. }
    ));
    assert_eq!(
        next_event(&mut events).await,
        SupervisorEvent::Stopped(Stop::Panicked(
            "maintenance slice panicked for the test".to_owned()
        ))
    );
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    let report = supervisor.shutdown(Duration::from_secs(2)).await.unwrap();
    assert_eq!(
        report.stop,
        Some(Stop::Panicked(
            "maintenance slice panicked for the test".to_owned()
        ))
    );
    assert_eq!(report.slices, 1);
    assert_eq!(engine.calls(), 0, "the panicking slice admitted nothing");
}

/// AC3: a slice held inside a projection write outlives the grace: shutdown reports the unjoined slice, releases nothing, and a later request joins it once the store is free.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn grace_expiry_reports_an_unjoined_slice_and_a_later_request_joins_it() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("a", "a text");
    let (projection, _) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    // Another writer holds the projection file, so the slice's first write waits on the store's busy timeout.
    let blocker = Connection::open(search_path(dir.path())).unwrap();
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        maintained(&corpus, Arc::new(projection), synapse),
        slice_bounds(Duration::from_secs(10)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert!(matches!(
        next_event(&mut events).await,
        SupervisorEvent::SliceStarted {
            kind: SliceKind::Backfill,
            ..
        }
    ));
    // Give the slice time to reach the held write before asking it to stop; the store's busy wait is what keeps it from joining.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let unresolved = supervisor.shutdown(Duration::from_millis(100)).await;
    assert_eq!(
        unresolved,
        Err(Unresolved {
            slices: 1,
            native: 0
        })
    );
    assert_eq!(
        engine.calls(),
        0,
        "nothing was admitted while the store was held"
    );
    blocker.execute_batch("COMMIT").unwrap();
    let report = supervisor.shutdown(Duration::from_secs(10)).await.unwrap();
    assert_eq!(report.stop, Some(Stop::Shutdown));
    assert_eq!(report.slices, 1);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        engine.calls(),
        0,
        "the cancelled slice admitted nothing once the store was free"
    );
}
