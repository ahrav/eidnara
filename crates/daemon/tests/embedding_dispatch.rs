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
    Blocked, DispatchBounds, DispatchError, DispatchEvent, DispatchFault, EmbeddingDispatcher,
    lane_binding,
};
use daemon::embedding_publication::{EmbeddingPublisher, Publication};
use daemon::search_projection::SearchProjection;
use daemon::search_writer::QuarantineKind;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    EmbedTokens, EmbeddingEngine, LaneInfo, LaneUnavailableState, PollOutcome, SubmitOutcome,
    SynapseComponent, SynapseLimits,
};
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, ArtifactIngestRequest, CommitIntent,
    Dimension, DomainSpec, EligibilityBinding, EventKind, ExportWindow, KernelStore,
    MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ProviderEgress, RepositoryProvenance, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, SourceDescriptorRequest, SourceHoldAdmission,
    SourceHoldBinding, SourceHoldBounds, SourcePageBounds, SourceRow, TaintClass,
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
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCOPE: &str = "project:a";
const SCOPE_B: &str = "project:b";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const DIMS: usize = 8;
const GENERATION: &str = "gen-1";
const NOW: i64 = 1_000;

const CHILD_ROOT: &str = "EIDNARA_EMBEDDING_DISPATCH_CHILD_ROOT";
const CHILD_BARRIER: &str = "EIDNARA_EMBEDDING_DISPATCH_BARRIER";

type Gate = Arc<(Mutex<bool>, Condvar)>;

struct GateGuard(Gate);

impl Drop for GateGuard {
    fn drop(&mut self) {
        TestEngine::release(&self.0);
    }
}

/// A deterministic engine: one whitespace word is one token, the vector is derived from the text, and the next inference can be made to fail, block, or return a malformed vector.
struct TestEngine {
    calls: AtomicUsize,
    count_calls: AtomicUsize,
    count_failure: Mutex<Option<InferenceError>>,
    fail_next: Mutex<Option<InferenceError>>,
    malformed: Mutex<bool>,
    gate: Mutex<Option<Gate>>,
}

impl TestEngine {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            count_calls: AtomicUsize::new(0),
            count_failure: Mutex::new(None),
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

    fn block_calls(&self) -> GateGuard {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        *self.gate.lock().unwrap() = Some(Arc::clone(&gate));
        GateGuard(gate)
    }

    fn release(gate: &Gate) {
        *gate.0.lock().unwrap_or_else(|error| error.into_inner()) = true;
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
        if let Some(error) = self.count_failure.lock().unwrap().take() {
            return Err(error);
        }
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

#[test]
fn a_gate_owner_unwinding_releases_the_inference_worker() {
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let emergency = engine.gate.lock().unwrap().as_ref().unwrap().clone();
    let worker_engine = Arc::clone(&engine);
    let (done, completion) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let result = worker_engine.embed(&["blocked input"]);
        done.send(result).unwrap();
    });
    let unwound = std::panic::catch_unwind(move || {
        let _owner = gate;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while engine.calls() == 0 {
            assert!(std::time::Instant::now() < deadline, "worker never entered");
            std::thread::yield_now();
        }
        panic!("force unwind after worker entry");
    });
    let completed = completion.recv_timeout(Duration::from_secs(1));
    TestEngine::release(&emergency);
    worker.join().unwrap();
    assert_eq!(
        unwound.unwrap_err().downcast_ref::<&str>(),
        Some(&"force unwind after worker entry")
    );
    assert!(completed.is_ok(), "unwinding left inference parked");
    assert!(completed.unwrap().is_ok());
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

fn wait_for_host_result(
    synapse: &SynapseComponent,
    host_job: &str,
    item: &str,
    text: &str,
) -> PollOutcome {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let result = synapse.poll_admitted(&lane(FINGERPRINT), host_job, item, text);
        if !matches!(result, PollOutcome::Pending { .. }) {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "host job stayed pending"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
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

fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
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

fn bounds(result_wait: Duration) -> DispatchBounds {
    DispatchBounds {
        max_jobs: NonZeroUsize::new(16).unwrap(),
        grant: grant(3, NOW + DAY_MS),
        retry_after: 10,
        result_wait,
        guard_deadline: Duration::from_secs(10),
    }
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
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE_B.to_string(),
                    object_id: SCOPE_B.to_string(),
                    source_id: SCOPE_B.to_string(),
                    domain_id: "domain".to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT_B.to_string()),
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
        self.publish_scoped(key, text, SCOPE)
    }

    fn publish_scoped(&self, key: &str, text: &str, scope: &str) -> String {
        self.publish_class_scoped_with(key, text, "messages", scope, Sensitivity::Normal, true)
    }

    /// Publishes one scoped, admitted descriptor of `class` and returns its object id.
    fn publish_class(&self, key: &str, text: &str, class: &str) -> String {
        self.publish_class_scoped_with(key, text, class, SCOPE, Sensitivity::Normal, true)
    }

    fn publish_hidden(&self, key: &str, text: &str) -> String {
        self.publish_class_scoped_with(key, text, "messages", SCOPE, Sensitivity::Normal, false)
    }

    fn publish_sensitive(&self, key: &str, text: &str) -> String {
        self.publish_class_scoped_with(key, text, "messages", SCOPE, Sensitivity::Sensitive, true)
    }

    fn publish_class_scoped_with(
        &self,
        key: &str,
        text: &str,
        class: &str,
        scope: &str,
        sensitivity: Sensitivity,
        admitted: bool,
    ) -> String {
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
                asserted_sensitivity: sensitivity,
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
                        scope_id: Some(scope),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity,
                        observed_at: 1,
                    })
                    .unwrap();
                if admitted {
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
                }
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
        let kernel_incarnation_id: String = Connection::open_with_flags(
            data_home.join("kernel/kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
        projection
            .write(|conn| {
                install_identity(conn, &identity(&kernel_incarnation_id), 1)?;
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
                kernel_incarnation_id,
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

fn insert_wrong_scope_jobs(data_home: &Path, occurrence_id: &str, count: usize) {
    let conn = Connection::open(search_path(data_home)).unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=0 WHERE occurrence_id=?1",
        [occurrence_id],
    )
    .unwrap();
    conn.execute(
        "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<?1)
         INSERT INTO vector_generations(generation_id,embedding_model,tokenizer_fingerprint,
             vector_dimension,generation_epoch,state,created_at,updated_at)
         SELECT printf('wrong-scope-generation-%04d',n),?2,?3,?4,1,'building',0,0 FROM ids",
        rusqlite::params![count as i64, MODEL, FINGERPRINT, DIMS as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,created_at,updated_at)
         SELECT 'wrong-scope-job-'||generation_id,?1,generation_id,'pending',1,0
         FROM vector_generations WHERE generation_id GLOB 'wrong-scope-generation-*'",
        [occurrence_id],
    )
    .unwrap();
}

/// One real dispatch pass on a blocking thread of the test runtime, so the component's inference workers run while the pass polls.
fn pass(
    corpus: &Corpus,
    projection: &SearchProjection,
    synapse: &SynapseComponent,
    bounds: &DispatchBounds,
    now: i64,
) -> (Option<Blocked>, Vec<DispatchEvent>) {
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut events = Vec::new();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, projection, synapse);
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), bounds, now, &mut |event| {
                events.push(event)
            })
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payload_corruption_quarantines_before_tokenization_or_inference() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("integrity", "hello");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let before = ledger(dir.path(), occurrence);
    Connection::open(search_path(dir.path()))
        .unwrap()
        .execute("UPDATE payloads SET bytes=?1", [b"jello".as_slice()])
        .unwrap();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let result = dispatcher.run_pass(
        eligibility(&project),
        &bounds(Duration::from_secs(5)),
        NOW,
        &mut |_| {},
    );
    let Err(DispatchError::Quarantined(quarantine)) = result else {
        panic!("{result:?}");
    };
    assert_eq!(quarantine.kind, QuarantineKind::Integrity);
    assert_eq!(projection.quarantine(), Some(quarantine.clone()));

    let mut fresh = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let again = fresh.run_pass(
        eligibility(&project),
        &bounds(Duration::from_secs(5)),
        NOW,
        &mut |_| panic!("shared quarantine permits no work"),
    );
    assert!(matches!(again, Err(DispatchError::Quarantined(q)) if q == quarantine));
    let publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);
    assert_eq!(publisher.quarantine(), Some(quarantine));
    assert_eq!(ledger(dir.path(), occurrence), before);
    assert_eq!(engine.count_calls(), 0);
    assert_eq!(engine.calls(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quarantine_entered_after_binding_stops_dispatch_before_submission_or_charge() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("shared-quarantine", "never submitted");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let before = ledger(dir.path(), occurrence);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut entered = None;

    let result = dispatcher.run_pass(
        eligibility(&project),
        &bounds(Duration::from_secs(5)),
        NOW,
        &mut |event| {
            if matches!(event, DispatchEvent::Bound(_)) {
                entered = Some(projection.enter_quarantine_for_test(
                    QuarantineKind::Storage,
                    &"concurrent projection writer",
                ));
            }
        },
    );

    let quarantine = entered.expect("binding observer enters quarantine");
    assert!(matches!(result, Err(DispatchError::Quarantined(q)) if q == quarantine));
    assert_eq!(ledger(dir.path(), occurrence), before);
    assert_eq!(engine.count_calls(), 0, "quarantine precedes preflight");
    assert_eq!(engine.calls(), 0, "quarantine precedes inference");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_search_deadline_preserves_admission_without_recharging() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("publication-blocked", "admitted input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut bounds = bounds(Duration::from_secs(5));
    bounds.guard_deadline = Duration::from_millis(300);
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut events = Vec::new();
    let mut at_admission = None;
    let mut write_lock = None;
    let end = dispatcher
        .run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
            if matches!(event, DispatchEvent::Admitted { .. }) {
                at_admission = Some(ledger(dir.path(), occurrence));
                let conn = Connection::open(search_path(dir.path())).unwrap();
                conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                write_lock = Some(conn);
            }
            events.push(event);
        })
        .unwrap();
    assert_eq!(end, Some(Blocked::SearchDeadline));
    let admitted_row = at_admission.expect("publication follows admission");
    assert_eq!(admitted_row.state, "admitted");
    assert_eq!(admitted_row.attempts, 1);
    assert_eq!(ledger(dir.path(), occurrence), admitted_row);
    assert!(stopped(&events).is_empty());
    assert!(retried(&events).is_empty());
    assert!(published(&events).is_empty());
    assert_eq!(engine.calls(), 1);
    assert!(projection.quarantine().is_none());

    drop(write_lock);
    events.clear();
    assert_eq!(
        dispatcher
            .run_pass(eligibility(&project), &bounds, NOW + 1, &mut |event| {
                events.push(event)
            })
            .unwrap(),
        None
    );
    let completed = ledger(dir.path(), occurrence);
    assert_eq!(completed.state, "embedded");
    assert_eq!(completed.attempts, admitted_row.attempts);
    assert_eq!(completed.host_job_id, admitted_row.host_job_id);
    assert_eq!(completed.episode, admitted_row.episode);
    assert_eq!(
        completed.vector,
        Some(encode(&TestEngine::vector_for("admitted input")))
    );
    assert!(admitted(&events).is_empty());
    assert_eq!(
        published(&events),
        vec![(completed.job_id, Publication::Embedded)]
    );
    assert_eq!(engine.calls(), 1, "publication reuses the retained result");
}

fn assert_lane_swap_blocked(after_admission: bool, replacement_max_tokens: u32) {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("lane-swap", "frozen lane input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine_a = TestEngine::new();
    let engine_b = TestEngine::new();
    let gate = engine_a.block_calls();
    let synapse = component(&engine_a, SynapseLimits::default());
    let before = ledger(dir.path(), occurrence);
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut events = Vec::new();
    let mut at_swap = None;
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let end = dispatcher
        .run_pass(
            eligibility(&project),
            &bounds(Duration::from_secs(5)),
            NOW,
            &mut |event| {
                if (matches!(event, DispatchEvent::Bound(_)) && !after_admission)
                    || (matches!(event, DispatchEvent::Admitted { .. }) && after_admission)
                {
                    let mut replacement = lane(&"b2".repeat(32));
                    replacement.model = "replacement-model".to_owned();
                    replacement.table_epoch = 2;
                    replacement.max_tokens = replacement_max_tokens;
                    at_swap = Some(ledger(dir.path(), occurrence));
                    synapse
                        .replace_ready_with_engine_for_test(
                            replacement,
                            Arc::clone(&engine_b) as Arc<dyn EmbeddingEngine>,
                        )
                        .unwrap();
                    TestEngine::release(&gate.0);
                }
                events.push(event);
            },
        )
        .unwrap();
    assert_eq!(
        end,
        Some(Blocked::IdentityChanged),
        "a pass cannot use a replacement lane"
    );
    assert_eq!(
        ledger(dir.path(), occurrence),
        at_swap.unwrap(),
        "identity refusal changes no ledger"
    );
    assert_eq!(engine_b.calls(), 0, "replacement engine must not run");
    assert_eq!(
        engine_b.count_calls(),
        0,
        "identity mismatch must refuse before the replacement tokenizer runs"
    );
    assert!(published(&events).is_empty());
    if after_admission {
        let job = ledger(dir.path(), occurrence);
        assert_eq!(events.len(), 2);
        assert_eq!(admitted(&events), vec![(job.job_id.clone(), 1)]);
        assert_eq!((job.state.as_str(), job.attempts), ("admitted", 1));
        assert!(job.host_job_id.is_some());
        assert!(job.vector.is_none());
        assert_eq!(engine_a.calls(), 1, "the admitted worker retains engine A");
        assert_eq!(engine_a.count_calls(), 1);
    } else {
        assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
        assert_eq!(ledger(dir.path(), occurrence), before);
        assert_eq!((before.state.as_str(), before.attempts), ("pending", 0));
        assert!(before.host_job_id.is_none());
        assert!(before.vector.is_none());
        assert_eq!(engine_a.calls(), 0);
        assert_eq!(engine_a.count_calls(), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lane_swap_after_binding_blocks_before_admission() {
    for max_tokens in [1, 512] {
        assert_lane_swap_blocked(false, max_tokens);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lane_swap_after_admission_blocks_completion() {
    for max_tokens in [1, 512] {
        assert_lane_swap_blocked(true, max_tokens);
    }
}

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
        &bounds(Duration::from_secs(5)),
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
        &bounds(Duration::from_secs(5)),
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
    let short = bounds(Duration::from_millis(50));

    let (end, events) = pass(&corpus, &projection, &synapse, &short, NOW);
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
        let (_, events) = pass(&corpus, &projection, &synapse, &short, NOW);
        assert!(admitted(&events).is_empty(), "{events:?}");
        assert_eq!(ledger(dir.path(), occurrence).attempts, 1);
    }
    assert_eq!(engine.calls(), 1);

    TestEngine::release(&gate.0);
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_secs(5)),
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
    let short = bounds(Duration::from_millis(50));
    let (_, events) = pass(&corpus, &projection, &first, &short, NOW);
    assert_eq!(admitted(&events).len(), 1);
    let held = ledger(dir.path(), occurrence);
    assert_eq!(held.state, "admitted");
    let first_host = first.host_incarnation().to_owned();
    TestEngine::release(&gate.0);
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
        let (end, events) = pass(&corpus, &projection, &wrong, &short, NOW);
        assert_eq!(end, Some(Blocked::BindingMismatch));
        assert!(events.is_empty());
        assert_eq!(ledger(dir.path(), occurrence), held);
    }
    assert_eq!(held.host_incarnation.as_deref(), Some(first_host.as_str()));

    // The old host's result can never satisfy the new incarnation: its job identifier polls as restarted there.
    let second = component(&engine, SynapseLimits::default());
    assert!(matches!(
        second.poll_admitted(
            &lane(FINGERPRINT),
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
        &bounds(Duration::from_secs(5)),
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
        &bounds(Duration::from_secs(5)),
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
        &bounds(Duration::from_secs(5)),
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
    let (end, events) = pass(&corpus, &projection, &synapse, &bounds, NOW);
    (
        dir, corpus, projection, occurrence, engine, synapse, events, end,
    )
}

/// AC5, AC6: a transient failure keeps the row pending under the same episode with its attempt charged, and it is eligible again only when its retry time comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_failure_retries_under_the_same_episode() {
    let standard = bounds(Duration::from_secs(5));

    // Transient execution failure: pending again under the same episode, one attempt charged, eligible only after `retry_after`.
    let (dir, corpus, projection, occurrence, _, synapse, events, end) = scenario(
        "retry me",
        SynapseLimits::default(),
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
    let (_, events) = pass(&corpus, &projection, &synapse, &standard, NOW + 9);
    assert!(admitted(&events).is_empty(), "not yet eligible: {events:?}");
    let (_, events) = pass(&corpus, &projection, &synapse, &standard, NOW + 10);
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
    let standard = bounds(Duration::from_secs(5));

    // An artifact fault is the lane's disposition, not the input's: the lane goes down, the pass blocks, and the attempted row keeps its charge as admitted work for the next serving host to reconcile.
    let (dir, corpus, projection, occurrence, engine, synapse, events, end) = scenario(
        "artifact",
        SynapseLimits::default(),
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
    let (end, events) = pass(&corpus, &projection, &synapse, &standard, NOW + DAY_MS / 2);
    assert!(matches!(end, Some(Blocked::LaneUnavailable(_))), "{end:?}");
    assert!(events.is_empty());
    let fresh = component(&engine, SynapseLimits::default());
    let (_, events) = pass(&corpus, &projection, &fresh, &standard, NOW + DAY_MS / 2);
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
        let (end, events) = pass(&corpus, &projection, &lane, &standard, NOW);
        assert_eq!(
            end,
            Some(Blocked::LaneUnavailable(LaneUnavailableState::Failing))
        );
        assert_eq!(admitted(&events), vec![(job.job_id.clone(), attempt)]);
    }
    let lane = component(&engine, SynapseLimits::default());
    let (end, events) = pass(&corpus, &projection, &lane, &standard, NOW);
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
    let (_, events) = pass(&corpus, &projection, &synapse, &standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    drop((synapse, projection, corpus, dir));

    // A different durable vector under the same pair is an idempotency conflict.
    let (dir, corpus, projection, occurrence, _, synapse, events, _) = scenario(
        "conflict",
        SynapseLimits::default(),
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
    let (_, events) = pass(&corpus, &projection, &synapse, &standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    drop((synapse, projection, corpus, dir));

    // Retired input is obsolete, not embedded, and never resumes.
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "retired",
        SynapseLimits::default(),
        standard,
        |corpus, _, _, object, _| {
            corpus.retire(object);
        },
    );
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "retracted".to_string())]
    );
    assert_eq!(
        (job.state.as_str(), job.vector.is_none()),
        ("obsolete", true)
    );
    let (projection, _) = reopen(dir.path(), projection, &[&occurrence]);
    let (_, events) = pass(&corpus, &projection, &synapse, &standard, NOW);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(engine.calls(), 0);
    drop((synapse, projection, corpus, dir));

    // An expired episode deadline stops before any charge.
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "late",
        SynapseLimits::default(),
        DispatchBounds {
            grant: grant(3, NOW - 1),
            ..standard
        },
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
    let tight = DispatchBounds {
        grant: grant(1, NOW + DAY_MS),
        ..bounds(Duration::from_secs(5))
    };
    let (dir, corpus, projection, occurrence, engine, synapse, events, _) = scenario(
        "exhaust me",
        SynapseLimits::default(),
        tight,
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
    let (_, events) = pass(&corpus, &projection, &synapse, &generous, NOW + DAY_MS);
    assert_eq!(events, vec![DispatchEvent::Bound(BindingOutcome::Bound)]);
    assert_eq!(
        ledger(dir.path(), &occurrence),
        after,
        "no fresh episode without authorization"
    );
    assert_eq!(engine.calls(), 1);

    let recovered = projection
        .write(|conn| authorize_recovery(conn, &job.job_id, "op-1", grant(2, NOW + DAY_MS), NOW))
        .unwrap();
    assert_eq!(
        recovered,
        Recovery::Granted {
            episode_id: format!("{}/auth/op-1", job.job_id)
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
    assert_eq!(reopened.authorization_ref.as_deref(), Some("op-1"));
    let replayed = projection
        .write(|conn| {
            authorize_recovery(conn, &job.job_id, "op-1", grant(5, NOW + 3 * DAY_MS), NOW)
        })
        .unwrap();
    assert_eq!(replayed, Recovery::Replayed);
    assert_eq!(
        ledger(dir.path(), &occurrence),
        reopened,
        "a replayed authorization grants nothing"
    );
    // A reference that could not be a host item identity is refused before any row is read.
    for invalid in [
        "",
        "has space",
        "slash/inside",
        &"x".repeat(retrieval::dispatch::MAX_AUTHORIZATION_REF_BYTES + 1),
    ] {
        let refused = projection
            .write(|conn| authorize_recovery(conn, &job.job_id, invalid, grant(2, NOW), NOW))
            .unwrap();
        assert_eq!(refused, Recovery::InvalidReference, "{invalid:?}");
    }
    assert_eq!(ledger(dir.path(), &occurrence), reopened);

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
    let (_, events) = pass(&corpus, &projection, &synapse, &generous, NOW);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "deadline_expired".to_string())]
    );
    assert_eq!(ledger(dir.path(), &occurrence).attempts, 0);
    assert_eq!(engine.calls(), 1);
    let granted_again = projection
        .write(|conn| authorize_recovery(conn, &job.job_id, "op-2", grant(2, NOW + DAY_MS), NOW))
        .unwrap();
    assert!(matches!(granted_again, Recovery::Granted { .. }));

    let (_, events) = pass(&corpus, &projection, &synapse, &tight, NOW);
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
        Some(format!("{}/auth/op-2", job.job_id).as_str())
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
    let short = bounds(Duration::from_millis(50));

    let (end, events) = pass(&corpus, &projection, &synapse, &short, NOW);
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
                held.episode.as_deref().unwrap(),
                &host,
                held.host_job_id.as_deref().unwrap(),
                short.grant,
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

    TestEngine::release(&gate.0);
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_secs(5)),
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
    dispatcher.inject_fault_for_test(DispatchFault::LoseChargeReply);
    let mut events = Vec::new();
    let bounds = bounds(Duration::from_secs(5));
    let end = tokio::task::block_in_place(|| {
        dispatcher
            .run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
                events.push(event)
            })
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
            .run_pass(eligibility(&project), &bounds, NOW, &mut |_| {})
            .unwrap()
    });
    assert_eq!(end, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn charge_rollback_cannot_skip_a_failed_host_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let text = "failed first attempt";
    let object = corpus.publish("rollback", text);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    engine.fail_next(InferenceError::Execution("controlled failure".to_owned()));
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.inject_fault_for_test(DispatchFault::RefuseChargeStatement);
    let mut tight = DispatchBounds {
        grant: grant(1, NOW + DAY_MS),
        ..bounds(Duration::ZERO)
    };
    let mut events = Vec::new();
    dispatcher
        .run_pass(eligibility(&project), &tight, NOW, &mut |event| {
            events.push(event)
        })
        .unwrap();
    let charged = ledger(dir.path(), occurrence);
    let item = retrieval::dispatch::first_episode_id(&charged.job_id);
    // The worker is parked, so identical submission can only return its existing identity.
    let input = synapse
        .preflight_embedding_for_lane(&lane(FINGERPRINT), text)
        .unwrap();
    let SubmitOutcome::Queued { job_id: h1 } = synapse.submit_admitted(&input, &item).unwrap()
    else {
        panic!("the parked host job must be retained");
    };
    drop(gate);
    assert!(matches!(
        wait_for_host_result(&synapse, &h1, &item, text),
        PollOutcome::Failed { .. }
    ));
    tight.result_wait = Duration::from_secs(5);
    dispatcher
        .run_pass(eligibility(&project), &tight, NOW + 10, &mut |event| {
            events.push(event)
        })
        .unwrap();
    let stopped_row = ledger(dir.path(), occurrence);
    assert_eq!(
        engine.calls(),
        1,
        "rollback must not permit a replacement H2"
    );
    assert_eq!((charged.state.as_str(), charged.attempts), ("admitted", 1));
    assert_eq!(charged.host_job_id.as_deref(), Some(h1.as_str()));
    assert_eq!(
        (stopped_row.state.as_str(), stopped_row.attempts),
        ("failed", 1)
    );
    assert_eq!(stopped_row.stop_reason.as_deref(), Some("exhausted"));
    assert_eq!(stopped_row.episode, charged.episode);
    assert!(stopped_row.vector.is_none());
    assert_eq!(admitted(&events), vec![(charged.job_id, 1)]);
    assert!(published(&events).is_empty());
    dispatcher
        .run_pass(eligibility(&project), &tight, NOW + 20, &mut |_| {})
        .unwrap();
    assert_eq!(ledger(dir.path(), occurrence), stopped_row);
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_search_deadline_preserves_the_candidate_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("terminal-blocked", "obsolete input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    corpus.retire(&object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut limits = bounds(Duration::from_secs(5));
    limits.guard_deadline = Duration::from_millis(300);
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut write_lock = None;
    let mut events = Vec::new();

    let end = dispatcher
        .run_pass(eligibility(&project), &limits, NOW, &mut |event| {
            if matches!(event, DispatchEvent::Bound(_)) {
                let conn = Connection::open(search_path(dir.path())).unwrap();
                conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                write_lock = Some(conn);
            }
            events.push(event);
        })
        .unwrap();
    assert_eq!(end, Some(Blocked::SearchDeadline));
    assert_eq!(ledger(dir.path(), occurrence).state, "pending");
    assert!(stopped(&events).is_empty());
    assert!(projection.quarantine().is_none());

    drop(write_lock);
    events.clear();
    assert_eq!(
        dispatcher
            .run_pass(eligibility(&project), &limits, NOW + 1, &mut |event| {
                events.push(event)
            })
            .unwrap(),
        None
    );
    assert_eq!(ledger(dir.path(), occurrence).state, "obsolete");
    let stopped = stopped(&events);
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].1, "retracted");
    assert_eq!(engine.calls(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_terminal_commit_emits_no_attributed_stop() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("terminal-lost-reply", "obsolete input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    corpus.retire(&object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.inject_fault_for_test(DispatchFault::LoseObsoletionReply);
    let mut events = Vec::new();

    assert_eq!(
        dispatcher
            .run_pass(
                eligibility(&project),
                &bounds(Duration::from_secs(5)),
                NOW,
                &mut |event| events.push(event),
            )
            .unwrap(),
        None
    );
    assert_eq!(ledger(dir.path(), occurrence).state, "obsolete");
    assert!(stopped(&events).is_empty(), "{events:?}");
    assert_eq!(engine.calls(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actionable_rows_are_repolled_before_later_pending_rows() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    for index in 0..3 {
        corpus.publish(&format!("actionable-{index}"), &format!("input {index}"));
    }
    let (projection, _rows) = corpus.bootstrap(dir.path());
    let ordered_occurrences: Vec<String> = {
        let conn = inspect(dir.path());
        let mut statement = conn
            .prepare("SELECT occurrence_id FROM embedding_jobs ORDER BY created_at,job_id")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut limits = bounds(Duration::ZERO);
    limits.max_jobs = NonZeroUsize::new(2).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut events = Vec::new();

    dispatcher
        .run_pass(eligibility(&project), &limits, NOW, &mut |event| {
            events.push(event)
        })
        .unwrap();
    assert_eq!(admitted(&events).len(), 2);
    assert_eq!(
        ledger(dir.path(), &ordered_occurrences[0]).state,
        "admitted"
    );
    assert_eq!(
        ledger(dir.path(), &ordered_occurrences[1]).state,
        "admitted"
    );
    assert_eq!(ledger(dir.path(), &ordered_occurrences[2]).state, "pending");

    drop(gate);
    events.clear();
    limits.result_wait = Duration::from_secs(5);
    dispatcher
        .run_pass(eligibility(&project), &limits, NOW + 1, &mut |event| {
            events.push(event)
        })
        .unwrap();
    assert_eq!(
        ledger(dir.path(), &ordered_occurrences[0]).state,
        "embedded"
    );
    assert_eq!(
        ledger(dir.path(), &ordered_occurrences[1]).state,
        "embedded"
    );
    assert_eq!(ledger(dir.path(), &ordered_occurrences[2]).state, "pending");
    assert_eq!(engine.calls(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eligibility_cursor_resets_when_project_changes() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object_b = corpus.publish_scoped("binding-b", "project b input", SCOPE_B);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence_b = occurrence_of(&rows, &object_b);
    insert_wrong_scope_jobs(dir.path(), occurrence_b, 2 * MAX_ELIGIBILITY_CANDIDATES);
    let target = ledger(dir.path(), occurrence_b);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project_a = ProjectScope::new(PROJECT).unwrap();
    let project_b = ProjectScope::new(PROJECT_B).unwrap();
    let mut limits = bounds(Duration::from_secs(5));
    limits.max_jobs = NonZeroUsize::new(1).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);

    assert_eq!(
        dispatcher
            .run_pass(eligibility(&project_a), &limits, NOW, &mut |_| {})
            .unwrap(),
        None
    );
    assert_eq!(ledger(dir.path(), occurrence_b).state, "pending");
    let cursor = dispatcher
        .eligibility_cursor_for_test()
        .expect("two full WrongScope pages carry a cursor");
    let conn = inspect(dir.path());
    let target_created_at: i64 = conn
        .query_row(
            "SELECT created_at FROM embedding_jobs WHERE job_id=?1",
            [&target.job_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        (target_created_at, target.job_id.as_str()) < (cursor.created_at, cursor.job_id.as_str()),
        "project B's oldest target must precede project A's carried cursor"
    );
    let rows_through_cursor: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs
             WHERE created_at<?1 OR (created_at=?1 AND job_id<=?2)",
            rusqlite::params![cursor.created_at, cursor.job_id],
            |row| row.get(0),
        )
        .unwrap();
    let rows_after_cursor: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs
             WHERE created_at>?1 OR (created_at=?1 AND job_id>?2)",
            rusqlite::params![cursor.created_at, cursor.job_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rows_through_cursor,
        i64::try_from(2 * MAX_ELIGIBILITY_CANDIDATES).unwrap()
    );
    assert_eq!(rows_after_cursor, 1, "fixture exceeds the two-page cap");
    drop(conn);
    let mut events = Vec::new();
    dispatcher
        .run_pass(eligibility(&project_b), &limits, NOW + 1, &mut |event| {
            events.push(event)
        })
        .unwrap();
    assert_eq!(ledger(dir.path(), occurrence_b).state, "embedded");
    assert_eq!(admitted(&events), vec![(target.job_id.clone(), 1)]);
    assert_eq!(
        published(&events),
        vec![(target.job_id, Publication::Embedded)]
    );
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresolved_charge_retry_quarantines_without_resubmission() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("charge-quarantine", "charge refused twice");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let before = ledger(dir.path(), occurrence);
    Connection::open(search_path(dir.path()))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_charge BEFORE UPDATE OF attempts ON embedding_jobs
         BEGIN SELECT RAISE(ABORT,'database is locked'); END;",
        )
        .unwrap();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.inject_fault_for_test(DispatchFault::RefuseChargeStatement);
    for _ in 0..2 {
        let result = dispatcher.run_pass(
            eligibility(&project),
            &bounds(Duration::ZERO),
            NOW,
            &mut |_| {},
        );
        assert!(
            matches!(result, Err(DispatchError::Quarantined(_))),
            "{result:?}"
        );
        assert_eq!(ledger(dir.path(), occurrence), before);
        assert_eq!(
            engine.count_calls(),
            1,
            "quarantine prevents another submission"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_charge_statement_is_retried_not_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "charge refused once");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.inject_fault_for_test(DispatchFault::RefuseChargeStatement);
    let bounds = bounds(Duration::from_secs(5));
    let mut events = Vec::new();
    let end = tokio::task::block_in_place(|| {
        dispatcher.run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
            events.push(event)
        })
    })
    .unwrap();
    assert_eq!(end, None);
    let charged = ledger(dir.path(), occurrence);
    assert_eq!((charged.state.as_str(), charged.attempts), ("embedded", 1));
    assert_eq!(admitted(&events), vec![(charged.job_id.clone(), 1)]);
    assert_eq!(admitted_host(&events, &charged.job_id), charged.host_job_id);
    assert_eq!(
        published(&events),
        vec![(charged.job_id.clone(), Publication::Embedded)]
    );

    let mut events = Vec::new();
    let end = tokio::task::block_in_place(|| {
        dispatcher.run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
            events.push(event)
        })
    })
    .unwrap();
    assert_eq!(end, None);
    assert!(admitted(&events).is_empty());
    assert!(published(&events).is_empty());
    assert_eq!(ledger(dir.path(), occurrence), charged);
    assert_eq!(
        engine.calls(),
        1,
        "the retained host job is reused, not re-run"
    );
}

/// A store failure on the lane binding decides nothing, so the pass reports it as retryable and the next pass runs as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_binding_is_retryable_not_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "bound on the second try");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    dispatcher.inject_fault_for_test(DispatchFault::RefuseBinding);
    let bounds = bounds(Duration::from_secs(5));
    let mut events = Vec::new();
    let refused = tokio::task::block_in_place(|| {
        dispatcher.run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
            events.push(event)
        })
    });
    assert!(
        matches!(refused, Err(DispatchError::Retryable(_))),
        "{refused:?}"
    );
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(ledger(dir.path(), occurrence).state, "pending");

    let mut events = Vec::new();
    let end = tokio::task::block_in_place(|| {
        dispatcher.run_pass(eligibility(&project), &bounds, NOW, &mut |event| {
            events.push(event)
        })
    })
    .unwrap();
    assert_eq!(end, None);
    assert_eq!(published(&events).len(), 1, "{events:?}");
    assert_eq!(ledger(dir.path(), occurrence).state, "embedded");
}

/// A valid reference yields an episode identity within the host's item identity bound.
#[test]
fn an_authorized_episode_identity_fits_the_host_item_bound() {
    let job_id = "0".repeat(64);
    let longest = format!(
        "{job_id}/auth/{}",
        "x".repeat(retrieval::dispatch::MAX_AUTHORIZATION_REF_BYTES)
    );
    assert!(longest.len() <= host_runtime::synapse::jobs::MAX_ITEM_ID_BYTES);
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
                &bounds(Duration::from_secs(600)),
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
        &bounds(Duration::from_secs(5)),
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

/// A charged admitted attempt is polled until it completes; exhaustion refuses only new attempts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_final_attempt_of_an_episode_completes_across_passes() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("m0", "last attempt");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(&engine, SynapseLimits::default());
    let tight = DispatchBounds {
        grant: grant(1, NOW + DAY_MS),
        ..bounds(Duration::from_millis(50))
    };

    let (end, events) = pass(&corpus, &projection, &synapse, &tight, NOW);
    assert_eq!(end, None);
    let held = ledger(dir.path(), occurrence);
    assert_eq!(admitted(&events), vec![(held.job_id.clone(), 1)]);
    assert_eq!((held.state.as_str(), held.attempts), ("admitted", 1));

    // The result is still outstanding: the row is polled, not judged exhausted.
    let (end, events) = pass(&corpus, &projection, &synapse, &tight, NOW);
    let still = ledger(dir.path(), occurrence);
    TestEngine::release(&gate.0);
    assert_eq!(end, None);
    assert!(stopped(&events).is_empty(), "{events:?}");
    assert!(admitted(&events).is_empty(), "{events:?}");
    assert_eq!(
        (still.state.as_str(), still.attempts, still.stop_reason),
        ("admitted", 1, None)
    );

    assert!(matches!(
        wait_for_host_result(
            &synapse,
            held.host_job_id.as_deref().unwrap(),
            held.episode.as_deref().unwrap(),
            "last attempt",
        ),
        PollOutcome::Page(_)
    ));
    *engine.count_failure.lock().unwrap() = Some(InferenceError::Execution(
        "completion count failed".to_owned(),
    ));
    let (end, events) = pass(&corpus, &projection, &synapse, &tight, NOW);
    assert_eq!(end, None);
    assert_eq!(ledger(dir.path(), occurrence), held);
    assert!(stopped(&events).is_empty());
    assert!(retried(&events).is_empty());

    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &DispatchBounds {
            grant: grant(1, NOW + DAY_MS),
            ..bounds(Duration::from_secs(5))
        },
        NOW,
    );
    assert_eq!(
        published(&events),
        vec![(held.job_id.clone(), Publication::Embedded)]
    );
    let done = ledger(dir.path(), occurrence);
    assert_eq!(
        (done.state.as_str(), done.attempts, done.host_job_id),
        ("embedded", 1, held.host_job_id)
    );
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_count_lane_failures_preserve_the_final_attempt() {
    for failure in [
        InferenceError::Artifact("completion artifact failure".to_owned()),
        InferenceError::Invariant("completion invariant failure".to_owned()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(dir.path());
        corpus.seed();
        let text = "retained final result";
        let object = corpus.publish("completion-lane-failure", text);
        let (projection, rows) = corpus.bootstrap(dir.path());
        let occurrence = occurrence_of(&rows, &object);
        let engine = TestEngine::new();
        let gate = engine.block_calls();
        let synapse = component(&engine, SynapseLimits::default());
        let tight = DispatchBounds {
            grant: grant(1, NOW + DAY_MS),
            ..bounds(Duration::from_millis(50))
        };
        pass(&corpus, &projection, &synapse, &tight, NOW);
        let held = ledger(dir.path(), occurrence);
        drop(gate);
        assert!(matches!(
            wait_for_host_result(
                &synapse,
                held.host_job_id.as_deref().unwrap(),
                held.episode.as_deref().unwrap(),
                text,
            ),
            PollOutcome::Page(_)
        ));
        *engine.count_failure.lock().unwrap() = Some(failure.clone());

        let (end, events) = pass(&corpus, &projection, &synapse, &tight, NOW);

        assert!(matches!(
            end,
            Some(Blocked::LaneUnavailable(
                LaneUnavailableState::Disabled | LaneUnavailableState::Failing
            ))
        ));
        assert_eq!(ledger(dir.path(), occurrence), held);
        assert!(stopped(&events).is_empty());
        assert!(retried(&events).is_empty());
        assert!(published(&events).is_empty());
        assert_eq!(engine.calls(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_evicted_result_is_readmitted_under_the_charged_attempt() {
    let mut failures = Vec::new();
    for allowance in [1, 2] {
        for count_failure in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let corpus = Corpus::open(dir.path());
            corpus.seed();
            let text = "evicted before polled";
            let object = corpus.publish("m0", text);
            let (projection, rows) = corpus.bootstrap(dir.path());
            let occurrence = occurrence_of(&rows, &object);
            let engine = TestEngine::new();
            let gate = engine.block_calls();
            let synapse = component(
                &engine,
                SynapseLimits {
                    max_retained_jobs: 1,
                    max_queued_jobs: 1,
                    ..SynapseLimits::default()
                },
            );
            let tight = DispatchBounds {
                grant: grant(allowance, NOW + DAY_MS),
                ..bounds(Duration::from_millis(50))
            };
            let (_, events) = pass(&corpus, &projection, &synapse, &tight, NOW);
            let held = ledger(dir.path(), occurrence);
            assert_eq!(admitted(&events), vec![(held.job_id.clone(), 1)]);
            assert_eq!(held.state, "admitted");
            let evicted_host_job = held.host_job_id.clone().unwrap();
            let item = held.episode.clone().unwrap();

            drop(gate);
            assert!(matches!(
                wait_for_host_result(&synapse, &evicted_host_job, &item, text),
                PollOutcome::Page(_)
            ));
            assert_eq!(engine.calls(), 1);

            let sentinel = synapse
                .preflight_embedding_for_lane(&lane(FINGERPRINT), "eviction sentinel")
                .unwrap();
            let SubmitOutcome::Queued {
                job_id: sentinel_host,
            } = synapse.submit_admitted(&sentinel, "sentinel").unwrap()
            else {
                panic!("sentinel must be admitted");
            };
            assert!(matches!(
                wait_for_host_result(&synapse, &sentinel_host, "sentinel", "eviction sentinel"),
                PollOutcome::Page(_)
            ));
            assert_eq!(engine.calls(), 2, "one original and one sentinel inference");
            assert!(matches!(
                synapse.poll_admitted(&lane(FINGERPRINT), &evicted_host_job, &item, text),
                PollOutcome::Restarted
            ));

            let blocker = if count_failure {
                *engine.count_failure.lock().unwrap() =
                    Some(InferenceError::Execution("count failed".to_owned()));
                None
            } else {
                let gate = engine.block_calls();
                let input = synapse
                    .preflight_embedding_for_lane(&lane(FINGERPRINT), "queue holder")
                    .unwrap();
                let SubmitOutcome::Queued { job_id } =
                    synapse.submit_admitted(&input, "queue-holder").unwrap()
                else {
                    panic!("queue holder must be admitted");
                };
                assert!(matches!(
                    synapse.poll_admitted(
                        &lane(FINGERPRINT),
                        &job_id,
                        "queue-holder",
                        "queue holder"
                    ),
                    PollOutcome::Pending { .. }
                ));
                Some((gate, job_id))
            };
            let (end, deferred_events) = pass(&corpus, &projection, &synapse, &tight, NOW);
            let deferred = ledger(dir.path(), occurrence);
            if let Some((gate, host)) = blocker {
                drop(gate);
                assert!(matches!(
                    wait_for_host_result(&synapse, &host, "queue-holder", "queue holder"),
                    PollOutcome::Page(_)
                ));
            }
            assert_eq!(end, None);
            if deferred != held
                || !retried(&deferred_events).is_empty()
                || !stopped(&deferred_events).is_empty()
            {
                failures.push((
                    allowance,
                    count_failure,
                    deferred.state,
                    deferred.attempts,
                    deferred.stop_reason,
                ));
                continue;
            }

            let (end, events) = pass(
                &corpus,
                &projection,
                &synapse,
                &DispatchBounds {
                    grant: grant(allowance, NOW + DAY_MS),
                    ..bounds(Duration::from_secs(5))
                },
                NOW,
            );
            assert_eq!(end, None);
            assert!(stopped(&events).is_empty(), "{events:?}");
            assert!(retried(&events).is_empty(), "{events:?}");
            assert_eq!(
                admitted(&events),
                vec![(held.job_id.clone(), 1)],
                "the re-admission reports the attempt already charged"
            );
            assert_eq!(
                published(&events),
                vec![(held.job_id.clone(), Publication::Embedded)]
            );
            let done = ledger(dir.path(), occurrence);
            assert_eq!(
                (done.state.as_str(), done.attempts, done.episode),
                ("embedded", 1, held.episode)
            );
            assert_ne!(done.host_job_id.as_deref(), Some(evicted_host_job.as_str()));
            assert_eq!(done.vector, Some(encode(&TestEngine::vector_for(text))));
            assert_eq!(
                engine.calls(),
                3 + usize::from(!count_failure),
                "two runs of the charged attempt plus the sentinel and any queue holder"
            );
        }
    }
    assert!(
        failures.is_empty(),
        "temporary readmission refusal changed the charged ledger: {failures:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_selection_skips_wrong_scope_without_starving_eligible_work() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object_b = corpus.publish_scoped("project-b", "project B input", SCOPE_B);
    let object_a = corpus.publish_scoped("project-a", "project A input", SCOPE);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence_a = occurrence_of(&rows, &object_a);
    let occurrence_b = occurrence_of(&rows, &object_b);
    let job_b = ledger(dir.path(), occurrence_b).job_id;
    let before_b = projection
        .read(|conn| job_ledger(conn, &job_b))
        .unwrap()
        .unwrap();
    insert_wrong_scope_jobs(dir.path(), occurrence_b, 1_024);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::from_secs(5))
    };

    let project_a = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut events = Vec::new();
    let end = dispatcher
        .run_pass(eligibility(&project_a), &one, NOW, &mut |event| {
            events.push(event)
        })
        .unwrap();
    assert_eq!(end, None);
    assert_eq!(
        projection
            .read(|conn| job_ledger(conn, &job_b))
            .unwrap()
            .unwrap(),
        before_b
    );
    assert_eq!(ledger(dir.path(), occurrence_a).state, "embedded");
    assert_eq!(published(&events).len(), 1, "{events:?}");
    assert_eq!(
        engine.calls(),
        1,
        "wrong-scope input never reaches inference"
    );

    let project_b = ProjectScope::new(PROJECT_B).unwrap();
    events.clear();
    let end = dispatcher
        .run_pass(eligibility(&project_b), &one, NOW, &mut |event| {
            events.push(event)
        })
        .unwrap();
    assert_eq!(end, None);
    assert_eq!(
        projection
            .read(|conn| job_ledger(conn, &job_b))
            .unwrap()
            .unwrap()
            .state,
        "embedded"
    );
    assert_eq!(published(&events).len(), 1, "{events:?}");
    assert_eq!(engine.calls(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_scan_cursor_advances_across_more_than_two_wrong_scope_pages() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object_b = corpus.publish_scoped("project-b-many", "project B input", SCOPE_B);
    let object_a = corpus.publish_scoped("project-a-after-many", "project A input", SCOPE);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence_a = occurrence_of(&rows, &object_a);
    let occurrence_b = occurrence_of(&rows, &object_b);
    insert_wrong_scope_jobs(dir.path(), occurrence_b, 2_048);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::from_secs(5))
    };
    let project_a = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);

    let first = dispatcher
        .run_pass(eligibility(&project_a), &one, NOW, &mut |_| {})
        .unwrap();
    assert_eq!(first, None);
    assert_eq!(ledger(dir.path(), occurrence_a).state, "pending");
    assert_eq!(engine.calls(), 0);

    let second = dispatcher
        .run_pass(eligibility(&project_a), &one, NOW, &mut |_| {})
        .unwrap();
    assert_eq!(second, None);
    assert_eq!(ledger(dir.path(), occurrence_a).state, "embedded");
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_retries_do_not_restart_another_projects_scan() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object_b = corpus.publish_scoped("foreign-retry", "project B input", SCOPE_B);
    let object_a = corpus.publish_scoped("ready-after-foreign", "project A input", SCOPE);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence_a = occurrence_of(&rows, &object_a);
    let occurrence_b = occurrence_of(&rows, &object_b);
    insert_wrong_scope_jobs(dir.path(), occurrence_b, 2_047);
    let conn = Connection::open(search_path(dir.path())).unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET next_attempt_at=?1 WHERE created_at=0",
        [NOW + 1],
    )
    .unwrap();
    drop(conn);

    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        retry_after: 1,
        ..bounds(Duration::from_secs(5))
    };
    let project_a = ProjectScope::new(PROJECT).unwrap();
    let project_b = ProjectScope::new(PROJECT_B).unwrap();
    let mut dispatcher_a = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let mut dispatcher_b = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    assert_eq!(
        dispatcher_a
            .run_pass(eligibility(&project_a), &one, NOW, &mut |_| {})
            .unwrap(),
        None
    );
    assert_eq!(ledger(dir.path(), occurrence_a).state, "pending");
    for tick in 1..=3 {
        *engine.count_failure.lock().unwrap() =
            Some(InferenceError::Execution("retry count".to_owned()));
        assert_eq!(
            dispatcher_b
                .run_pass(eligibility(&project_b), &one, NOW + tick, &mut |_| {})
                .unwrap(),
            None
        );
        assert_eq!(
            ledger(dir.path(), occurrence_b).next_attempt_at,
            Some(NOW + tick + 1)
        );
        assert_eq!(
            dispatcher_a
                .run_pass(eligibility(&project_a), &one, NOW + tick, &mut |_| {})
                .unwrap(),
            None
        );
    }
    assert_eq!(
        ledger(dir.path(), occurrence_a).state,
        "embedded",
        "foreign retry deadlines must not restart the project's forward scan"
    );
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_row_is_revisited_when_its_retry_becomes_due() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let deferred = corpus.publish_scoped("deferred", "deferred input", SCOPE);
    let wrong_scope = corpus.publish_scoped("wrong-scope", "other project", SCOPE_B);
    let actionable = corpus.publish_scoped("actionable", "held input", SCOPE);
    let (projection, rows) = corpus.bootstrap(dir.path());
    let deferred_occurrence = occurrence_of(&rows, &deferred);
    let wrong_scope_occurrence = occurrence_of(&rows, &wrong_scope);
    let actionable_occurrence = occurrence_of(&rows, &actionable);
    let conn = Connection::open(search_path(dir.path())).unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=0,next_attempt_at=?2 WHERE occurrence_id=?1",
        rusqlite::params![deferred_occurrence, NOW + 1],
    )
    .unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=1 WHERE occurrence_id=?1",
        [wrong_scope_occurrence],
    )
    .unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=2 WHERE occurrence_id=?1",
        [actionable_occurrence],
    )
    .unwrap();
    drop(conn);

    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::ZERO)
    };
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);

    dispatcher
        .run_pass(eligibility(&project), &one, NOW, &mut |_| {})
        .unwrap();
    assert_eq!(ledger(dir.path(), deferred_occurrence).attempts, 0);
    assert_eq!(ledger(dir.path(), actionable_occurrence).state, "admitted");

    dispatcher
        .run_pass(eligibility(&project), &one, NOW + 1, &mut |_| {})
        .unwrap();
    assert_eq!(
        ledger(dir.path(), deferred_occurrence).attempts,
        1,
        "the scan must revisit a deferred row when its retry becomes due"
    );
    drop(gate);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_jobs_bounds_terminal_dispositions() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<_> = (0..3)
        .map(|index| corpus.publish(&format!("terminal-{index}"), "obsolete input"))
        .collect();
    let (projection, _rows) = corpus.bootstrap(dir.path());
    for object in &objects {
        corpus.retire(object);
    }
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::from_secs(5))
    };

    let (end, events) = pass(&corpus, &projection, &synapse, &one, NOW);

    assert_eq!(end, None);
    let conn = inspect(dir.path());
    let obsolete: i64 = conn
        .query_row(
            "SELECT count(*) FROM embedding_jobs WHERE state='obsolete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(obsolete, 1);
    assert_eq!(stopped(&events).len(), 1);
    assert_eq!(engine.calls(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_candidate_is_obsoleted_without_poisoning_valid_work() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let malformed = corpus.publish("malformed-candidate", "malformed input");
    let valid = corpus.publish("valid-after-malformed", "valid input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let malformed_occurrence = occurrence_of(&rows, &malformed);
    let valid_occurrence = occurrence_of(&rows, &valid);
    let conn = Connection::open(search_path(dir.path())).unwrap();
    conn.execute(
        "UPDATE occurrences SET source_object_id='' WHERE occurrence_id=?1",
        [malformed_occurrence],
    )
    .unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=0 WHERE occurrence_id=?1",
        [malformed_occurrence],
    )
    .unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=1 WHERE occurrence_id=?1",
        [valid_occurrence],
    )
    .unwrap();
    drop(conn);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let one = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::from_secs(5))
    };

    let (end, events) = pass(&corpus, &projection, &synapse, &one, NOW);

    assert_eq!(end, None);
    assert_eq!(ledger(dir.path(), malformed_occurrence).state, "obsolete");
    assert_eq!(ledger(dir.path(), valid_occurrence).state, "pending");
    assert!(
        stopped(&events)
            .iter()
            .any(|(_, reason)| reason == "invalid_identity"),
        "{events:?}"
    );
    assert_eq!(engine.calls(), 0);

    let (end, events) = pass(&corpus, &projection, &synapse, &one, NOW + 1);
    assert_eq!(end, None);
    assert_eq!(ledger(dir.path(), valid_occurrence).state, "embedded");
    assert_eq!(published(&events).len(), 1, "{events:?}");
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_jobs_are_hydrated_only_when_they_are_driven() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let first = corpus.publish("hydrate-first", "first input");
    let corrupt = corpus.publish("hydrate-corrupt", "later input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let first_occurrence = occurrence_of(&rows, &first);
    let corrupt_occurrence = occurrence_of(&rows, &corrupt);
    let conn = Connection::open(search_path(dir.path())).unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=0 WHERE occurrence_id=?1",
        [first_occurrence],
    )
    .unwrap();
    conn.execute(
        "UPDATE embedding_jobs SET created_at=1 WHERE occurrence_id=?1",
        [corrupt_occurrence],
    )
    .unwrap();
    conn.execute(
        "UPDATE payloads SET bytes=?1 WHERE payload_id=(SELECT payload_id FROM occurrences WHERE occurrence_id=?2)",
        rusqlite::params![b"other input".as_slice(), corrupt_occurrence],
    )
    .unwrap();
    drop(conn);

    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
    let result = dispatcher.run_pass(
        eligibility(&project),
        &DispatchBounds {
            max_jobs: NonZeroUsize::new(2).unwrap(),
            ..bounds(Duration::from_secs(5))
        },
        NOW,
        &mut |_| {},
    );

    assert!(matches!(result, Err(DispatchError::Quarantined(_))));
    assert_eq!(
        ledger(dir.path(), first_occurrence).state,
        "embedded",
        "the first payload must complete before the next payload is hydrated"
    );
    assert_eq!(ledger(dir.path(), corrupt_occurrence).state, "pending");
    assert_eq!(engine.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_and_sensitive_inputs_are_obsoleted_without_inference() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let hidden = corpus.publish_hidden("hidden-before-dispatch", "hidden input");
    let sensitive = corpus.publish_sensitive("sensitive-before-dispatch", "sensitive input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let hidden_occurrence = occurrence_of(&rows, &hidden);
    let sensitive_occurrence = occurrence_of(&rows, &sensitive);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());

    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_secs(5)),
        NOW,
    );

    assert_eq!(end, None);
    assert_eq!(ledger(dir.path(), hidden_occurrence).state, "obsolete");
    assert_eq!(ledger(dir.path(), sensitive_occurrence).state, "obsolete");
    assert_eq!(engine.count_calls(), 0, "terminal inputs skip tokenization");
    assert_eq!(engine.calls(), 0, "terminal inputs skip inference");
    let reasons: Vec<_> = stopped(&events)
        .into_iter()
        .map(|(_, reason)| reason)
        .collect();
    assert!(reasons.contains(&"hidden".to_owned()), "{events:?}");
    assert!(
        reasons.contains(&"provider_sensitive".to_owned()),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_selection_still_obsoletes_retracted_input_without_inference() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("retracted-before-dispatch", "obsolete input");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    corpus.retire(&object);
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());

    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_secs(5)),
        NOW,
    );

    assert_eq!(end, None);
    assert_eq!(ledger(dir.path(), occurrence).state, "obsolete");
    assert_eq!(engine.count_calls(), 0, "obsolete input skips tokenization");
    assert_eq!(engine.calls(), 0, "obsolete input skips inference");
    assert!(published(&events).is_empty());
}

/// With room for one job, the pass takes the row created earliest even when its identifier sorts last.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eligible_rows_are_taken_oldest_first_not_by_identifier() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<String> = (0..3)
        .map(|i| corpus.publish(&format!("m{i}"), &format!("message number {i}")))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let mut job_ids: Vec<String> = objects
        .iter()
        .map(|object| ledger(dir.path(), occurrence_of(&rows, object)).job_id)
        .collect();
    job_ids.sort();
    let last_by_identifier = job_ids.last().unwrap().clone();
    drop(projection);
    Connection::open(search_path(dir.path()))
        .unwrap()
        .execute(
            "UPDATE embedding_jobs SET created_at=1 WHERE job_id=?1",
            [&last_by_identifier],
        )
        .unwrap();
    let projection = SearchProjection::open(dir.path()).unwrap();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());

    let one_at_a_time = DispatchBounds {
        max_jobs: NonZeroUsize::new(1).unwrap(),
        ..bounds(Duration::from_secs(5))
    };
    let (end, events) = pass(&corpus, &projection, &synapse, &one_at_a_time, NOW);
    assert_eq!(end, None);
    assert_eq!(
        published(&events),
        vec![(last_by_identifier, Publication::Embedded)],
        "the oldest row goes first regardless of its identifier"
    );
}

/// Recovering under the reference `1` opens an episode distinct from the first, so inference runs instead of replaying the stopped episode's retained failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authorization_reference_cannot_name_the_first_episode() {
    let (dir, corpus, projection, occurrence, engine, synapse, events, end) = scenario(
        "rejected by the model",
        SynapseLimits::default(),
        bounds(Duration::from_secs(5)),
        |_, _, engine, _, _| {
            engine.fail_next(InferenceError::Input("rejected".to_owned()));
        },
    );
    assert_eq!(end, None);
    let job = ledger(dir.path(), &occurrence);
    assert_eq!(
        stopped(&events),
        vec![(job.job_id.clone(), "schema_violation".to_string())]
    );
    assert_eq!((job.state.as_str(), job.attempts), ("failed", 1));
    assert_eq!(engine.calls(), 1);

    let recovered = projection
        .write(|conn| authorize_recovery(conn, &job.job_id, "1", grant(2, NOW + DAY_MS), NOW))
        .unwrap();
    let Recovery::Granted { episode_id } = recovered else {
        panic!("{recovered:?}");
    };
    assert_ne!(
        episode_id,
        format!("{}/1", job.job_id),
        "an authorized episode lives in its own namespace"
    );

    let (end, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_secs(5)),
        NOW,
    );
    assert_eq!(end, None);
    assert_eq!(
        published(&events),
        vec![(job.job_id.clone(), Publication::Embedded)],
        "{events:?}"
    );
    let done = ledger(dir.path(), &occurrence);
    assert_eq!(
        (done.state.as_str(), done.attempts, done.episode),
        ("embedded", 1, Some(episode_id))
    );
    assert_eq!(engine.calls(), 2, "the new episode ran its own inference");
}

/// A pass on a one-worker runtime must hand the worker off while it polls, or the inference it waits for never runs.
#[test]
fn a_pass_on_a_runtime_worker_yields_the_worker_to_the_inference_it_awaits() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (end, published_count, calls) = runtime.block_on(async move {
        tokio::spawn(async move {
            let corpus = Corpus::open(&root);
            corpus.seed();
            corpus.publish("m0", "embedded from a worker");
            let (projection, _) = corpus.bootstrap(&root);
            let engine = TestEngine::new();
            let synapse = component(&engine, SynapseLimits::default());
            let project = ProjectScope::new(PROJECT).unwrap();
            let mut dispatcher = EmbeddingDispatcher::new(&corpus.kernel, &projection, &synapse);
            let mut events = Vec::new();
            let end = dispatcher
                .run_pass(
                    eligibility(&project),
                    &bounds(Duration::from_millis(300)),
                    NOW,
                    &mut |event| events.push(event),
                )
                .unwrap();
            (end, published(&events).len(), engine.calls())
        })
        .await
        .unwrap()
    });
    assert_eq!(end, None);
    assert_eq!(
        published_count, 1,
        "the worker's inference ran while the pass polled"
    );
    assert_eq!(calls, 1);
}
