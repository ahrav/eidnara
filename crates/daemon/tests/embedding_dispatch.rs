//! Durable embedding dispatch against a real kernel, a real projection, and an in-process Synapse component.
//! Pending rows are the only queue: they reach the job table once, their results are polled rather than re-admitted, a host restart returns admitted work to pending with its attempts kept, every named disposition survives reopen without resuming on its own, and the attempt ledger moves with the state it accounts for.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use daemon::embedding_dispatch::{
    Blocked, DispatchBounds, DispatchEvent, EmbeddingDispatcher, lane_binding,
};
use daemon::embedding_publication::{ObsoleteCause, Publication};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    EmbedTokens, EmbeddingEngine, LaneInfo, LaneUnavailableState, PollOutcome, SynapseComponent,
    SynapseLimits,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, ArtifactIngestRequest, CommitIntent,
    Dimension, DomainSpec, EligibilityBinding, EventKind, ExportWindow, KernelStore, ProjectScope,
    ProviderEgress, RepositoryProvenance, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass,
    SourceDescriptorRequest, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds,
    SourcePageBounds, SourceRow, StaleInput, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::dispatch::{
    Admission, BindingOutcome, EpisodeGrant, Recovery, authorize_recovery, charge_admission,
    job_ledger,
};
use retrieval::vectors::encode;
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

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

const CHILD_ROOT: &str = "EIDNARA_EMBEDDING_DISPATCH_CHILD_ROOT";
const CHILD_BARRIER: &str = "EIDNARA_EMBEDDING_DISPATCH_BARRIER";

type Gate = Arc<(Mutex<bool>, Condvar)>;

/// A deterministic engine: one whitespace word is one token, the vector is derived from the text, and the next inference can be made to fail, block, or return a malformed vector.
struct TestEngine {
    calls: AtomicUsize,
    count_calls: AtomicUsize,
    fail_next: Mutex<Option<InferenceError>>,
    malformed: Mutex<bool>,
    gate: Mutex<Option<Gate>>,
}

impl TestEngine {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            count_calls: AtomicUsize::new(0),
            fail_next: Mutex::new(None),
            malformed: Mutex::new(false),
            gate: Mutex::new(None),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn count_calls(&self) -> usize {
        self.count_calls.load(Ordering::SeqCst)
    }

    fn fail_next(&self, error: InferenceError) {
        *self.fail_next.lock().unwrap() = Some(error);
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
        self.count_calls.fetch_add(1, Ordering::SeqCst);
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
        if let Some(error) = self.fail_next.lock().unwrap().take() {
            return Err(error);
        }
        if *self.malformed.lock().unwrap() {
            return Ok(texts.iter().map(|_| vec![0.5; DIMS]).collect());
        }
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
    published: RefCell<Vec<(String, String)>>,
}

impl Corpus {
    fn open(root: &Path) -> Self {
        Self {
            kernel: Arc::new(KernelStore::open(root.join("kernel")).unwrap()),
            published: RefCell::new(Vec::new()),
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
        self.publish_class(key, text, "messages")
    }

    /// Publishes one scoped, admitted descriptor of `class` and returns its object id.
    fn publish_class(&self, key: &str, text: &str, class: &str) -> String {
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
                let (identity, representation): (Vec<(&str, &str)>, &str) = match class {
                    "raw_tool_spans" => (
                        vec![
                            ("project_id", "proj-a"),
                            ("harness", "pi"),
                            ("session_id", "sess-01"),
                            ("parent_message_id", "msg-2"),
                            ("tool_call_id", key),
                            ("result_revision", "1"),
                            ("block_index", "0"),
                        ],
                        "tool_output",
                    ),
                    _ => (
                        vec![
                            ("project_id", "proj-a"),
                            ("harness", "opencode"),
                            ("session_id", "sess-01"),
                            ("message_id", key),
                            ("block_index", "0"),
                        ],
                        "text",
                    ),
                };
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        occurrence: Occurrence {
                            class,
                            identity: &identity,
                            revision: "1",
                            representation,
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
        self.published
            .borrow_mut()
            .push((object.clone(), text.to_string()));
        object
    }

    fn retire(&self, object_id: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object_id}")), |envelope| {
                envelope.retire_observation(object_id)?;
                Ok(String::new())
            })
            .unwrap();
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

/// The durable ledger of one occurrence's job, read outside every API under test.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ledger {
    job_id: String,
    state: String,
    attempts: u32,
    allowance: u32,
    deadline: Option<i64>,
    episode: Option<String>,
    host_job_id: Option<String>,
    host_incarnation: Option<String>,
    next_attempt_at: Option<i64>,
    last_failure_kind: Option<String>,
    stop_reason: Option<String>,
    authorization_ref: Option<String>,
    vector: Option<Vec<u8>>,
    /// The `(input_bytes, input_tokens)` the durable vector was charged for.
    charged_input: Option<(i64, i64)>,
}

fn ledger(data_home: &Path, occurrence_id: &str) -> Ledger {
    let conn = inspect(data_home);
    let stored: Option<(Vec<u8>, i64, i64)> = conn
        .query_row(
            "SELECT vector,input_bytes,input_tokens FROM occurrence_vectors WHERE occurrence_id=?1",
            [occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap();
    let vector = stored.as_ref().map(|(vector, _, _)| vector.clone());
    let charged_input = stored.map(|(_, bytes, tokens)| (bytes, tokens));
    conn.query_row(
        "SELECT job_id,state,attempts,episode_allowance,episode_deadline,episode_id,host_job_id,
                host_incarnation,next_attempt_at,last_failure_kind,stop_reason,authorization_ref
         FROM embedding_jobs WHERE occurrence_id=?1",
        [occurrence_id],
        |row| {
            Ok(Ledger {
                job_id: row.get(0)?,
                state: row.get(1)?,
                attempts: row.get(2)?,
                allowance: row.get(3)?,
                deadline: row.get(4)?,
                episode: row.get(5)?,
                host_job_id: row.get(6)?,
                host_incarnation: row.get(7)?,
                next_attempt_at: row.get(8)?,
                last_failure_kind: row.get(9)?,
                stop_reason: row.get(10)?,
                authorization_ref: row.get(11)?,
                vector,
                charged_input,
            })
        },
    )
    .unwrap()
}

/// One real dispatch pass on a blocking thread of the test runtime, so the component's inference workers run while the pass polls.
fn pass(
    corpus: &Corpus,
    projection: &SearchProjection,
    synapse: &SynapseComponent,
    bounds: &DispatchBounds,
    wait: Duration,
    now: i64,
) -> (Option<Blocked>, Vec<DispatchEvent>) {
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut events = Vec::new();
    let bounds = DispatchBounds {
        result_wait: wait,
        ..*bounds
    };
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, projection, synapse);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds,
                &budget(Duration::from_secs(30)),
                now,
                &mut |event| events.push(event),
            )
            .unwrap()
    });
    (end, events)
}

fn admitted(events: &[DispatchEvent]) -> Vec<(String, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Admitted {
                job_id, attempts, ..
            } => Some((job_id.clone(), *attempts)),
            _ => None,
        })
        .collect()
}

/// The host job the dispatcher reported admitting `job_id` to.
fn admitted_host(events: &[DispatchEvent], job_id: &str) -> Option<String> {
    events.iter().find_map(|event| match event {
        DispatchEvent::Admitted {
            job_id: id,
            host_job_id,
            ..
        } if id == job_id => Some(host_job_id.clone()),
        _ => None,
    })
}

fn published(events: &[DispatchEvent]) -> Vec<(String, Publication)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Published { job_id, outcome } => Some((job_id.clone(), outcome.clone())),
            _ => None,
        })
        .collect()
}

fn stopped(events: &[DispatchEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Stopped { job_id, reason } => Some((job_id.clone(), reason.clone())),
            _ => None,
        })
        .collect()
}

fn retried(events: &[DispatchEvent]) -> Vec<(String, &'static str)> {
    events
        .iter()
        .filter_map(|event| match event {
            DispatchEvent::Retried { job_id, kind } => Some((job_id.clone(), *kind)),
            _ => None,
        })
        .collect()
}

/// Closes and reopens the projection, asserting the ledger of every occurrence is exactly what the previous open left.
fn reopen(
    data_home: &Path,
    projection: SearchProjection,
    occurrences: &[&str],
) -> (SearchProjection, Vec<Ledger>) {
    let before: Vec<Ledger> = occurrences
        .iter()
        .map(|occurrence| ledger(data_home, occurrence))
        .collect();
    drop(projection);
    let projection = SearchProjection::open(data_home).unwrap();
    let after: Vec<Ledger> = occurrences
        .iter()
        .map(|occurrence| ledger(data_home, occurrence))
        .collect();
    assert_eq!(before, after, "reopen changes no ledger");
    (projection, after)
}

/// AC1, AC4: the real pending → job table → leased result → guarded completion path; the durable ledger matches the admissions, and the stored binding matches the verified lane and the serving host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_rows_reach_guarded_completion_through_one_job_table() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let texts = ["first message", "second message here", "third"];
    let objects: Vec<String> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| corpus.publish(&format!("m{i}"), text))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let tip = corpus.tip();

    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(end, None);
    assert_eq!(events[0], DispatchEvent::Bound(BindingOutcome::Bound));
    let admitted = admitted(&events);
    assert_eq!(admitted.len(), 3, "{events:?}");
    assert!(admitted.iter().all(|(_, attempts)| *attempts == 1));
    let published = published(&events);
    assert_eq!(published.len(), 3);
    assert!(
        published
            .iter()
            .all(|(_, outcome)| *outcome == Publication::Embedded)
    );
    assert_eq!(engine.calls(), 3, "one inference per admitted job");
    assert_eq!(
        engine.count_calls(),
        6,
        "every job is counted exactly before admission and again before its result is trusted"
    );
    assert_eq!(corpus.tip(), tip, "dispatch commits nothing to the kernel");

    for (object, text) in objects.iter().zip(texts) {
        let ledger = ledger(dir.path(), occurrence_of(&rows, object));
        assert_eq!(ledger.state, "embedded");
        assert_eq!(ledger.attempts, 1);
        assert_eq!(ledger.allowance, 3);
        assert_eq!(ledger.deadline, Some(NOW + DAY_MS));
        assert_eq!(
            ledger.episode.as_deref(),
            Some(format!("{}/1", ledger.job_id).as_str())
        );
        assert_eq!(ledger.vector, Some(encode(&TestEngine::vector_for(text))));
        assert_eq!(
            ledger.charged_input,
            Some((text.len() as i64, text.split_whitespace().count() as i64)),
            "the completion is charged the exact bytes and untruncated count"
        );
        assert_eq!(
            admitted_host(&events, &ledger.job_id),
            ledger.host_job_id,
            "the durable row names the host job the dispatcher admitted"
        );
        assert!(
            ledger
                .host_job_id
                .as_deref()
                .is_some_and(|id| id.starts_with(synapse.host_incarnation())),
            "{ledger:?}"
        );
        assert_eq!(
            ledger.host_incarnation.as_deref(),
            Some(synapse.host_incarnation()),
            "the row records the host that holds it"
        );
    }
    // The identity the projection was built under is the lane the dispatcher bound to.
    let identity = projection.read(retrieval::read_identity).unwrap().unwrap();
    let bound = lane_binding(&lane(FINGERPRINT), synapse.host_incarnation());
    assert_eq!(
        (
            identity.embedding_model,
            identity.tokenizer_fingerprint,
            identity.vector_dimension,
            identity.generation_epoch
        ),
        (
            bound.embedding_model,
            bound.bundle_fingerprint,
            bound.vector_dimension,
            bound.table_epoch
        )
    );

    // A second pass finds nothing eligible and touches neither the engine nor the ledgers.
    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(end, None);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(engine.calls(), 3);
}

/// AC2, AC7: a result the host has not produced yet leaves the row admitted, a later pass polls the same job instead of re-admitting it, and the vector completes once the worker finishes; nothing is Embedded on the host's word alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outstanding_results_are_polled_by_identity_and_never_readmitted() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "held message");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(&engine, SynapseLimits::default());
    let short = Duration::from_millis(50);

    let (end, events) = pass(&corpus, &projection, &synapse, &bounds(), short, NOW);
    assert_eq!(end, None);
    assert_eq!(admitted(&events).len(), 1);
    assert!(published(&events).is_empty());
    let held = ledger(dir.path(), occurrence);
    assert_eq!(held.state, "admitted");
    assert_eq!(held.attempts, 1);
    assert_eq!(held.vector, None, "Ready is not Embedded");
    assert_eq!(engine.count_calls(), 1, "no result, so no second judgement");

    // Duplicate passes poll the held job under its stored identity; they neither admit nor charge again.
    for _ in 0..2 {
        let (_, events) = pass(&corpus, &projection, &synapse, &bounds(), short, NOW);
        assert!(admitted(&events).is_empty(), "{events:?}");
        assert_eq!(ledger(dir.path(), occurrence).attempts, 1);
    }
    assert_eq!(engine.calls(), 1);

    TestEngine::release(&gate);
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(
        published(&events),
        vec![(held.job_id.clone(), Publication::Embedded)]
    );
    let done = ledger(dir.path(), occurrence);
    assert_eq!(done.state, "embedded");
    assert_eq!(done.attempts, 1);
    assert_eq!(
        done.host_job_id, held.host_job_id,
        "the same host job produced the vector"
    );
    assert_eq!(
        done.vector,
        Some(encode(&TestEngine::vector_for("held message")))
    );
    assert_eq!(engine.calls(), 1, "no second inference");
}

/// AC2, AC4, AC6: a new host incarnation cannot satisfy work the old one admitted; rebinding returns it to pending with its attempt kept, a lane with a different fingerprint blocks admission, and the stored binding follows the host that actually serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_restart_reconciles_admitted_work_and_wrong_lanes_block() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "message across restart");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let first = component(&engine, SynapseLimits::default());
    let short = Duration::from_millis(50);
    let (_, events) = pass(&corpus, &projection, &first, &bounds(), short, NOW);
    assert_eq!(admitted(&events).len(), 1);
    let held = ledger(dir.path(), occurrence);
    assert_eq!(held.state, "admitted");
    let first_host = first.host_incarnation().to_owned();
    TestEngine::release(&gate);
    drop(first);

    // A lane differing in any one identity field is refused before any row is touched.
    let wrong_lanes = [
        LaneInfo {
            model: "other-model".to_owned(),
            ..lane(FINGERPRINT)
        },
        lane("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"),
        LaneInfo {
            dims: 16,
            ..lane(FINGERPRINT)
        },
        LaneInfo {
            table_epoch: 2,
            ..lane(FINGERPRINT)
        },
    ];
    for wrong in wrong_lanes {
        let wrong = SynapseComponent::ready_with_engine(
            wrong,
            Arc::clone(&engine) as Arc<dyn EmbeddingEngine>,
            SynapseLimits::default(),
        )
        .unwrap();
        let (end, events) = pass(&corpus, &projection, &wrong, &bounds(), short, NOW);
        assert_eq!(end, Some(Blocked::BindingMismatch));
        assert!(events.is_empty());
        assert_eq!(ledger(dir.path(), occurrence), held);
    }
    assert_eq!(held.host_incarnation.as_deref(), Some(first_host.as_str()));

    // The old host's result can never satisfy the new incarnation: its job identifier polls as restarted there.
    let second = component(&engine, SynapseLimits::default());
    assert!(matches!(
        second.poll_admitted(
            held.host_job_id.as_deref().unwrap(),
            held.episode.as_deref().unwrap(),
            "message across restart"
        ),
        PollOutcome::Restarted
    ));

    // The next incarnation of the right lane reconciles first: the held job is unreachable, so it is pending again with its attempt kept, then re-admitted under the same job identity.
    let (end, events) = pass(
        &corpus,
        &projection,
        &second,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(end, None);
    assert_eq!(
        events[0],
        DispatchEvent::Bound(BindingOutcome::Rebound { released: 1 })
    );
    assert_eq!(admitted(&events), vec![(held.job_id.clone(), 2)]);
    assert_eq!(
        published(&events),
        vec![(held.job_id.clone(), Publication::Embedded)]
    );
    let done = ledger(dir.path(), occurrence);
    assert_eq!(done.state, "embedded");
    assert_eq!(done.attempts, 2, "the restart replenished nothing");
    assert_eq!(done.episode, held.episode, "the restart renewed no episode");
    assert_eq!(
        done.host_incarnation.as_deref(),
        Some(second.host_incarnation()),
        "the row follows the host that served it"
    );
    let _ = reopen(dir.path(), projection, &[occurrence]);
    // The reopened projection still records the product identity the lane was bound to.
    let identity: (String, String, u32, i64) = inspect(dir.path())
        .query_row(
            "SELECT embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch FROM projection_identity",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let bound = lane(FINGERPRINT);
    assert_eq!(
        identity,
        (
            bound.model,
            bound.fingerprint,
            bound.dims as u32,
            bound.table_epoch as i64
        )
    );
}

/// AC3: input the lane cannot embed makes zero inference calls, stops with a reason that names no content, and leaves the lexical occurrence in place; reopening does not resume it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_limit_input_stops_without_inference_and_keeps_lexical_state() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let long = vec!["word"; 600].join(" ");
    let over = corpus.publish("m0", &long);
    let fine = corpus.publish("m1", "fits the window");
    let raw = corpus.publish_class("t0", "raw tool output", "raw_tool_spans");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let raw_jobs: i64 = inspect(dir.path())
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence_of(&rows, &raw)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_jobs, 0, "raw tool spans queue no dense work");
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());

    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(end, None);
    let over_ledger = ledger(dir.path(), occurrence_of(&rows, &over));
    assert_eq!(
        stopped(&events),
        vec![(over_ledger.job_id.clone(), "input_over_limit".to_string())]
    );
    assert_eq!(over_ledger.state, "failed");
    assert_eq!(over_ledger.attempts, 0, "a refused input is never charged");
    assert_eq!(over_ledger.stop_reason.as_deref(), Some("input_over_limit"));
    assert!(
        !format!("{events:?}").contains("word word"),
        "no reason carries content"
    );
    assert_eq!(engine.calls(), 1, "only the admissible text was embedded");
    assert_eq!(published(&events).len(), 1);
    assert!(
        !format!("{events:?}").contains(occurrence_of(&rows, &raw)),
        "the raw tool span is never dispatched"
    );
    let lexical: i64 = inspect(dir.path())
        .query_row(
            "SELECT COUNT(*) FROM occurrences WHERE occurrence_id=?1",
            [occurrence_of(&rows, &over)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        lexical, 1,
        "the lexical occurrence survives the dense refusal"
    );

    let (projection, _) = reopen(
        dir.path(),
        projection,
        &[occurrence_of(&rows, &over), occurrence_of(&rows, &fine)],
    );
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(engine.calls(), 1);
}

/// A prepared scenario: its temporary root, corpus, projection, the one occurrence's id, the engine, the component, and the first pass's events and end.
type Scenario = (
    tempfile::TempDir,
    Corpus,
    SearchProjection,
    String,
    Arc<TestEngine>,
    SynapseComponent,
    Vec<DispatchEvent>,
    Option<Blocked>,
);

/// One disposition scenario: a fresh corpus with one message, the engine prepared by `arrange`, one pass, then reopen and a second pass that must resume nothing on its own.
fn scenario(
    text: &str,
    limits: SynapseLimits,
    bounds: DispatchBounds,
    wait: Duration,
    arrange: impl FnOnce(&Corpus, &SearchProjection, &Arc<TestEngine>, &str, &str),
) -> Scenario {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", text);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let engine = TestEngine::new();
    arrange(&corpus, &projection, &engine, &object, &occurrence);
    let synapse = component(&engine, limits);
    let (end, events) = pass(&corpus, &projection, &synapse, &bounds, wait, NOW);
    (
        dir, corpus, projection, occurrence, engine, synapse, events, end,
    )
}

/// AC5, AC6: a transient failure keeps the row pending under the same episode with its attempt charged, and it is eligible again only when its retry time comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_failure_retries_under_the_same_episode() {
    let standard = Duration::from_secs(5);

    // Transient execution failure: pending again under the same episode, one attempt charged, eligible only after `retry_after`.
    let (dir, corpus, projection, occurrence, _, synapse, events, end) = scenario(
        "retry me",
        SynapseLimits::default(),
        bounds(),
        standard,
        |_, _, engine, _, _| {
            engine.fail_next(InferenceError::Execution("transient".to_owned()));
        },
    );
    assert_eq!(end, None);
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        retried(&events),
        vec![(job.job_id.clone(), "execution_failure")]
    );
    assert_eq!(
        (job.state.as_str(), job.attempts, job.next_attempt_at),
        ("pending", 1, Some(NOW + 10))
    );
    assert_eq!(job.last_failure_kind.as_deref(), Some("execution_failure"));
    let (projection, ledgers) = reopen(dir.path(), projection, &[&occurrence]);
    let job = ledgers.into_iter().next().unwrap();
    let (_, events) = pass(&corpus, &projection, &synapse, &bounds(), standard, NOW + 9);
    assert!(admitted(&events).is_empty(), "not yet eligible: {events:?}");
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        standard,
        NOW + 10,
    );
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 2)]);
    assert_eq!(published(&events).len(), 1);
    let done = ledger(dir.path(), &occurrence);
    assert_eq!(
        (done.state.as_str(), done.attempts, done.episode),
        ("embedded", 2, job.episode)
    );
    drop((synapse, projection, corpus, dir));
}

/// AC5: terminal dispositions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_dispositions_stop_dispatch_until_authorized() {
    let standard = Duration::from_secs(5);

    // An artifact fault is the lane's disposition, not the input's: the lane goes down, the pass blocks, and the attempted row keeps its charge as admitted work for the next serving host to reconcile.
    let (dir, corpus, projection, occurrence, engine, synapse, events, end) = scenario(
        "artifact",
        SynapseLimits::default(),
        bounds(),
        standard,
        |_, _, engine, _, _| {
            engine.fail_next(InferenceError::Artifact("bad artifact".to_owned()));
        },
    );
    assert!(matches!(end, Some(Blocked::LaneUnavailable(_))), "{end:?}");
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
    assert!(stopped(&events).is_empty() && published(&events).is_empty());
    assert_eq!(
        (job.state.as_str(), job.attempts, job.vector.is_none()),
        ("admitted", 1, true)
    );
    let (projection, _) = reopen(dir.path(), projection, &[&occurrence]);
    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        standard,
        NOW + DAY_MS / 2,
    );
    assert!(matches!(end, Some(Blocked::LaneUnavailable(_))), "{end:?}");
    assert!(events.is_empty());
    let fresh = component(&engine, SynapseLimits::default());
    let (_, events) = pass(
        &corpus,
        &projection,
        &fresh,
        &bounds(),
        standard,
        NOW + DAY_MS / 2,
    );
    assert_eq!(
        events[0],
        DispatchEvent::Bound(BindingOutcome::Rebound { released: 1 })
    );
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 2)]);
    assert_eq!(
        published(&events),
        vec![(job.job_id.clone(), Publication::Embedded)]
    );
    assert_eq!(engine.calls(), 2);
    drop((synapse, fresh, projection, corpus, dir));

    // A persistently malformed vector fails every lane it meets; each incarnation charges one attempt until the episode is exhausted, and nothing is ever Embedded.
    let (dir, corpus, projection, occurrence, engine, synapse, events, end) = scenario(
        "malformed",
        SynapseLimits::default(),
        bounds(),
        standard,
        |_, _, engine, _, _| {
            *engine.malformed.lock().unwrap() = true;
        },
    );
    assert_eq!(
        end,
        Some(Blocked::LaneUnavailable(LaneUnavailableState::Failing))
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
    for attempt in 2..=3 {
        let lane = component(&engine, SynapseLimits::default());
        let (end, events) = pass(&corpus, &projection, &lane, &bounds(), standard, NOW);
        assert_eq!(
            end,
            Some(Blocked::LaneUnavailable(LaneUnavailableState::Failing))
        );
        assert_eq!(admitted(&events), vec![(job.job_id.clone(), attempt)]);
    }
    let lane = component(&engine, SynapseLimits::default());
    let (end, events) = pass(&corpus, &projection, &lane, &bounds(), standard, NOW);
    assert_eq!(end, None);
    assert_eq!(
        events[0],
        DispatchEvent::Bound(BindingOutcome::Rebound { released: 1 })
    );
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "exhausted".to_string())]
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        (job.state.as_str(), job.attempts, job.vector.is_none()),
        ("failed", 3, true)
    );
    assert_eq!(engine.calls(), 3);
    drop((synapse, lane, projection, corpus, dir));

    // A job queued under a generation the lane does not serve is a model mismatch: obsolete, uncharged, never embedded.
    let (dir, corpus, projection, occurrence, engine, synapse, events, end) = scenario(
        "wrong generation",
        SynapseLimits::default(),
        bounds(),
        standard,
        |_, projection, _, _, occurrence| {
            let other = VectorGeneration {
                generation_id: "gen-other".to_string(),
                embedding_model: "other-model".to_string(),
                ..generation()
            };
            projection
                .write(|conn| {
                    register_generation(conn, &other, 1)?;
                    conn.execute(
                        "UPDATE embedding_jobs SET generation_id=?2 WHERE occurrence_id=?1",
                        rusqlite::params![occurrence, other.generation_id],
                    )?;
                    Ok(())
                })
                .unwrap();
        },
    );
    assert_eq!(end, None);
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "generation_mismatch".to_string())]
    );
    assert_eq!(
        (job.state.as_str(), job.attempts, job.vector.is_none()),
        ("obsolete", 0, true)
    );
    assert_eq!(engine.calls(), 0);
    let (projection, _) = reopen(dir.path(), projection, &[&occurrence]);
    let (_, events) = pass(&corpus, &projection, &synapse, &bounds(), standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    drop((synapse, projection, corpus, dir));

    // A different durable vector under the same pair is an idempotency conflict.
    let (dir, corpus, projection, occurrence, _, synapse, events, _) = scenario(
        "conflict",
        SynapseLimits::default(),
        bounds(),
        standard,
        |_, projection, _, _, occurrence| {
            let other = encode(&TestEngine::vector_for("something else"));
            projection
                .write(|conn| {
                    conn.execute(
                        "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,8,1,1,1)",
                        rusqlite::params![occurrence, GENERATION, other],
                    )?;
                    Ok(())
                })
                .unwrap();
        },
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "idempotency_conflict".to_string())]
    );
    assert_eq!(job.state, "failed");
    let (projection, _) = reopen(dir.path(), projection, &[&occurrence]);
    let (_, events) = pass(&corpus, &projection, &synapse, &bounds(), standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    drop((synapse, projection, corpus, dir));

    // Retired input is obsolete, not embedded, and never resumes.
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "retired",
        SynapseLimits::default(),
        bounds(),
        standard,
        |corpus, _, _, object, _| {
            corpus.retire(object);
        },
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        published(&events),
        vec![(
            job.job_id.clone(),
            Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Retracted))
        )]
    );
    assert_eq!(
        (job.state.as_str(), job.vector.is_none()),
        ("obsolete", true)
    );
    let (projection, _) = reopen(dir.path(), projection, &[&occurrence]);
    let (_, events) = pass(&corpus, &projection, &synapse, &bounds(), standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(engine.calls(), 1);
    drop((synapse, projection, corpus, dir));

    // An expired episode deadline stops before any charge.
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "late",
        SynapseLimits::default(),
        DispatchBounds {
            grant: grant(3, NOW - 1),
            ..bounds()
        },
        standard,
        |_, _, _, _, _| {},
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "deadline_expired".to_string())]
    );
    assert_eq!((job.state.as_str(), job.attempts), ("failed", 0));
    assert_eq!(engine.calls(), 0);
    drop((synapse, projection, corpus, dir));
}

/// AC6: an allowance of one is exhausted by one failure; reopen and a later deadline resume nothing; an explicit authorization opens exactly one new episode, and replaying it grants nothing more. Reopen resets neither deadline nor allowance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhaustion_holds_until_an_authorization_that_replays_idempotently() {
    let wait = Duration::from_secs(5);
    let tight = DispatchBounds {
        grant: grant(1, NOW + DAY_MS),
        ..bounds()
    };
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "exhaust me",
        SynapseLimits::default(),
        tight,
        wait,
        |_, _, engine, _, _| {
            engine.fail_next(InferenceError::Execution("transient".to_owned()));
        },
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "exhausted".to_string())]
    );
    assert_eq!(
        (job.state.as_str(), job.attempts, job.allowance),
        ("failed", 1, 1)
    );
    assert_eq!(job.stop_reason.as_deref(), Some("exhausted"));

    // Reopen with a larger grant and a later deadline: the stored episode keeps its own allowance and deadline.
    let (projection, ledgers) = reopen(dir.path(), projection, &[&occurrence]);
    let after = ledgers.into_iter().next().unwrap();
    let generous = DispatchBounds {
        grant: grant(9, NOW + 2 * DAY_MS),
        ..tight
    };
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &generous,
        wait,
        NOW + DAY_MS,
    );
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(
        ledger(dir.path(), &occurrence),
        after,
        "no fresh episode without authorization"
    );
    assert_eq!(engine.calls(), 1);

    let recovered = projection
        .write(|conn| authorize_recovery(conn, &job.job_id, "auth-1", grant(2, NOW + DAY_MS), NOW))
        .unwrap();
    assert_eq!(
        recovered,
        Recovery::Granted {
            episode_id: format!("{}/auth-1", job.job_id)
        }
    );
    let reopened = ledger(dir.path(), &occurrence);
    assert_eq!(
        (
            reopened.state.as_str(),
            reopened.attempts,
            reopened.allowance
        ),
        ("pending", 0, 2)
    );
    assert_eq!(reopened.authorization_ref.as_deref(), Some("auth-1"));
    let replayed = projection
        .write(|conn| {
            authorize_recovery(conn, &job.job_id, "auth-1", grant(5, NOW + 3 * DAY_MS), NOW)
        })
        .unwrap();
    assert_eq!(replayed, Recovery::Replayed);
    assert_eq!(
        ledger(dir.path(), &occurrence),
        reopened,
        "a replayed authorization grants nothing"
    );

    // The stored episode deadline, not the pass grant, bounds the row: an expired stored deadline stops it even under a generous grant.
    drop(projection);
    Connection::open(search_path(dir.path()))
        .unwrap()
        .execute(
            "UPDATE embedding_jobs SET episode_deadline=?2 WHERE job_id=?1",
            rusqlite::params![job.job_id, NOW - 1],
        )
        .unwrap();
    let projection = SearchProjection::open(dir.path()).unwrap();
    let (_, events) = pass(&corpus, &projection, &synapse, &generous, wait, NOW);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "deadline_expired".to_string())]
    );
    assert_eq!(ledger(dir.path(), &occurrence).attempts, 0);
    assert_eq!(engine.calls(), 1);
    let granted_again = projection
        .write(|conn| authorize_recovery(conn, &job.job_id, "auth-2", grant(2, NOW + DAY_MS), NOW))
        .unwrap();
    assert!(matches!(granted_again, Recovery::Granted { .. }));

    let (_, events) = pass(&corpus, &projection, &synapse, &tight, wait, NOW);
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
    assert_eq!(
        published(&events),
        vec![(job.job_id.clone(), Publication::Embedded)]
    );
    let done = ledger(dir.path(), &occurrence);
    assert_eq!(
        (done.state.as_str(), done.attempts, done.allowance),
        ("embedded", 1, 2)
    );
    assert_eq!(
        done.episode.as_deref(),
        Some(format!("{}/auth-2", job.job_id).as_str())
    );
}

/// AC5, AC6: a full job table leaves the second row pending under its episode with nothing charged; a lost admission reply is reconciled from the row rather than charged twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_full_and_lost_replies_never_charge_twice() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let first = corpus.publish("m0", "first in line");
    let second = corpus.publish("m1", "second in line");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(
        &engine,
        SynapseLimits {
            max_queued_jobs: 1,
            ..SynapseLimits::default()
        },
    );
    let short = Duration::from_millis(50);

    let (end, events) = pass(&corpus, &projection, &synapse, &bounds(), short, NOW);
    assert_eq!(end, None);
    assert_eq!(admitted(&events).len(), 1);
    assert_eq!(retried(&events).len(), 1);
    let (occ_a, occ_b) = (occurrence_of(&rows, &first), occurrence_of(&rows, &second));
    let (a, b) = (ledger(dir.path(), occ_a), ledger(dir.path(), occ_b));
    let (held, held_occurrence, waiting) = if a.state == "admitted" {
        (a, occ_a, b)
    } else {
        (b, occ_b, a)
    };
    assert_eq!(
        (
            waiting.state.as_str(),
            waiting.attempts,
            waiting.host_job_id
        ),
        ("pending", 0, None)
    );
    assert_eq!(waiting.last_failure_kind.as_deref(), Some("admission_full"));
    assert_eq!(waiting.next_attempt_at, Some(NOW + 10));
    assert_eq!(held.attempts, 1);

    // The same admission charged again, as after a lost COMMIT reply, is recognised by its host job and not re-charged.
    let host = lane_binding(&lane(FINGERPRINT), synapse.host_incarnation());
    let again = projection
        .write(|conn| {
            charge_admission(
                conn,
                &held.job_id,
                &host,
                held.host_job_id.as_deref().unwrap(),
                bounds().grant,
                NOW,
            )
        })
        .unwrap();
    assert_eq!(again, Admission::AlreadyCharged);
    assert_eq!(ledger(dir.path(), held_occurrence).attempts, 1);
    let ledgered = projection
        .read(|conn| job_ledger(conn, &held.job_id))
        .unwrap()
        .unwrap();
    assert_eq!(ledgered.attempts, 1);

    TestEngine::release(&gate);
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW + 10,
    );
    assert_eq!(published(&events).len(), 2, "{events:?}");
    assert_eq!(
        ledger(dir.path(), occurrence_of(&rows, &first)).state,
        "embedded"
    );
    assert_eq!(
        ledger(dir.path(), occurrence_of(&rows, &second)).state,
        "embedded"
    );
    assert_eq!(engine.calls(), 2);
}

/// AC6: a charge whose COMMIT reply is lost is reconciled from the row through the real pass: one attempt, one host job, one vector, no quarantine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_charge_reply_is_reconciled_from_the_row_not_recharged() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "charged once");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.lose_next_charge_reply_for_test();
    let mut events = Vec::new();
    let bounds = bounds();
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds,
                &budget(Duration::from_secs(5)),
                NOW,
                &mut |event| events.push(event),
            )
            .unwrap()
    });
    assert_eq!(end, None);
    let job = ledger(dir.path(), occurrence);
    assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
    assert_eq!(
        published(&events),
        vec![(job.job_id.clone(), Publication::Embedded)]
    );
    assert_eq!((job.state.as_str(), job.attempts), ("embedded", 1));
    assert_eq!(admitted_host(&events, &job.job_id), job.host_job_id);
    assert_eq!(engine.calls(), 1);
    // The dispatcher is not quarantined: a later pass runs and finds nothing to do.
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds,
                &budget(Duration::from_secs(5)),
                NOW,
                &mut |_| {},
            )
            .unwrap()
    });
    assert_eq!(end, None);
}

/// The child bootstraps nothing: it reopens the stores the parent prepared, admits the one pending job to a host whose worker never finishes, prints its barrier once the admission is charged, and parks until the parent kills it.
#[test]
#[ignore = "re-executed by the crash-cut test with its environment set"]
fn crash_child_entrypoint_reexecuted_by_the_parent() {
    let root = PathBuf::from(std::env::var(CHILD_ROOT).unwrap());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let kernel = KernelStore::open(root.join("kernel")).unwrap();
        let projection = SearchProjection::open(&root).unwrap();
        let engine = TestEngine::new();
        let _gate = engine.block_calls();
        let synapse = component(&engine, SynapseLimits::default());
        let project = ProjectScope::new(PROJECT).unwrap();
        let mut dispatcher = EmbeddingDispatcher::new(&kernel, &projection, &synapse);
        let result = tokio::task::block_in_place(|| {
            dispatcher.run_pass(
                eligibility(&project),
                &bounds(),
                &budget(Duration::from_secs(600)),
                NOW,
                &mut |event| {
                    if let DispatchEvent::Admitted { .. } = event {
                        let mut stdout = std::io::stdout().lock();
                        writeln!(stdout, "{CHILD_BARRIER}").unwrap();
                        stdout.flush().unwrap();
                        drop(stdout);
                        loop {
                            std::thread::park();
                        }
                    }
                },
            )
        });
        panic!("the child was not killed at its barrier: {result:?}");
    });
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn run_crash_child(root: &Path) {
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_child_entrypoint_reexecuted_by_the_parent",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, root)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let line = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find(|line| line.contains(CHILD_BARRIER));
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(120)).unwrap();
    assert!(line.is_some(), "the child never reached its barrier");
    // Dropping the guard kills and reaps the parked child.
    drop(child);
}

/// AC7: a process killed with its charge committed and its result never produced reopens with state, attempt, and host job together; the next incarnation reconciles and finishes under the same identity and episode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_after_charge_reopens_state_and_accounting_together() {
    let dir = tempfile::tempdir().unwrap();
    let occurrence = {
        let corpus = Corpus::open(dir.path());
        corpus.seed();
        let object = corpus.publish("m0", "survives a crash");
        let (projection, rows) = corpus.bootstrap(dir.path());
        drop(projection);
        occurrence_of(&rows, &object).to_string()
    };
    run_crash_child(dir.path());

    let cut = ledger(dir.path(), &occurrence);
    assert_eq!(
        (cut.state.as_str(), cut.attempts),
        ("admitted", 1),
        "{cut:?}"
    );
    assert!(cut.host_job_id.is_some() && cut.vector.is_none(), "{cut:?}");
    let dead_host = cut.host_incarnation.clone().unwrap();
    assert!(cut.host_job_id.as_deref().unwrap().starts_with(&dead_host));

    let corpus = Corpus::open(dir.path());
    let projection = SearchProjection::open(dir.path()).unwrap();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(),
        Duration::from_secs(5),
        NOW,
    );
    assert_eq!(end, None);
    assert_eq!(
        events[0],
        DispatchEvent::Bound(BindingOutcome::Rebound { released: 1 })
    );
    assert_eq!(admitted(&events), vec![(cut.job_id.clone(), 2)]);
    assert_eq!(
        published(&events),
        vec![(cut.job_id.clone(), Publication::Embedded)]
    );
    let done = ledger(dir.path(), &occurrence);
    assert_eq!(
        (done.state.as_str(), done.attempts, done.episode),
        ("embedded", 2, cut.episode)
    );
    assert_eq!(
        done.vector,
        Some(encode(&TestEngine::vector_for("survives a crash")))
    );
    assert_eq!(
        done.host_incarnation.as_deref(),
        Some(synapse.host_incarnation())
    );
    assert_ne!(done.host_incarnation.as_deref(), Some(dead_host.as_str()));
}
