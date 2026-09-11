//! Fixtures shared by the embedding dispatch, identity sweep, and embedding supervisor suites.
//! All three suites drive one host contract, so `TestEngine` exposes their combined hooks.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use daemon::embedding_dispatch::DispatchBounds;
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
use retrieval::{
    PersistBounds, ProjectionIdentity, Tombstone, TombstoneReason, install_identity,
    tombstone_occurrence,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

pub const CONSUMER: &str = "search";
pub const KERNEL_INCARNATION: &str = "kernel-1";
pub const POLICY: &str = "source-policy.v1";
pub const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SCOPE: &str = "project:a";
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub const MODEL: &str = "tiny-test-model";
pub const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
pub const DIMS: usize = 8;
pub const GENERATION: &str = "gen-1";
pub const NOW: i64 = 1_000;

pub type Gate = Arc<(Mutex<bool>, Condvar)>;

/// Releases its gate on drop, including the unwind after a failed assertion.
pub struct GateGuard(pub Gate);

impl Drop for GateGuard {
    fn drop(&mut self) {
        TestEngine::release(&self.0);
    }
}

/// `TestEngine` tokenizes text by whitespace and derives deterministic vectors from text.
pub struct TestEngine {
    calls: AtomicUsize,
    count_calls: AtomicUsize,
    completed: AtomicUsize,
    fail_next: Mutex<Option<InferenceError>>,
    malformed: AtomicBool,
    gate: Mutex<Option<Gate>>,
}

impl TestEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            count_calls: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            fail_next: Mutex::new(None),
            malformed: AtomicBool::new(false),
            gate: Mutex::new(None),
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn count_calls(&self) -> usize {
        self.count_calls.load(Ordering::SeqCst)
    }

    /// Unlike `calls`, a gated inference counts here only after its release.
    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    pub fn fail_next(&self, error: InferenceError) {
        *self.fail_next.lock().unwrap() = Some(error);
    }

    /// Every later inference returns a vector of the right width whose values the publisher rejects.
    pub fn return_malformed(&self) {
        self.malformed.store(true, Ordering::SeqCst);
    }

    pub fn block_calls(&self) -> Gate {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        *self.gate.lock().unwrap() = Some(Arc::clone(&gate));
        gate
    }

    pub fn release(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    pub fn vector_for(text: &str) -> Vec<f32> {
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
        // The gate slot's guard is released before the wait, so `block_calls` can install a later gate while this call is held.
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            let mut released = gate.0.lock().unwrap();
            while !*released {
                released = gate.1.wait(released).unwrap();
            }
        }
        if let Some(error) = self.fail_next.lock().unwrap().take() {
            return Err(error);
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
        if self.malformed.load(Ordering::SeqCst) {
            return Ok(texts.iter().map(|_| vec![0.5; DIMS]).collect());
        }
        Ok(texts.iter().map(|text| Self::vector_for(text)).collect())
    }
}

pub fn lane(fingerprint: &str) -> LaneInfo {
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

pub fn component(engine: &Arc<TestEngine>, limits: SynapseLimits) -> SynapseComponent {
    SynapseComponent::ready_with_engine(
        lane(FINGERPRINT),
        Arc::clone(engine) as Arc<dyn EmbeddingEngine>,
        limits,
    )
    .unwrap()
}

pub fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMS as u32,
        generation_epoch: 1,
    }
}

pub fn identity() -> ProjectionIdentity {
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

pub fn batch_bounds() -> BatchBounds {
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

pub fn grant(allowance: u32, deadline: i64) -> EpisodeGrant {
    EpisodeGrant {
        allowance: NonZeroU32::new(allowance).unwrap(),
        deadline,
    }
}

pub fn bounds() -> DispatchBounds {
    DispatchBounds {
        max_jobs: NonZeroUsize::new(16).unwrap(),
        grant: grant(3, NOW + DAY_MS),
        retry_after: 10,
        result_wait: Duration::from_secs(5),
    }
}

pub fn budget(wait: Duration) -> EvalBudget {
    EvalBudget::new(
        Some(Instant::now() + wait),
        Arc::new(AtomicBool::new(false)),
    )
}

pub fn eligibility(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Remote,
    }
}

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-embedding-dispatch-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

pub struct Corpus {
    pub kernel: Arc<KernelStore>,
}

impl Corpus {
    pub fn open(root: &Path) -> Self {
        Self {
            kernel: Arc::new(KernelStore::open(root.join("kernel")).unwrap()),
        }
    }

    pub fn seed(&self) {
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

    pub fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    pub fn publish(&self, key: &str, text: &str) -> String {
        self.publish_class(key, text, "messages")
    }

    pub fn publish_class(&self, key: &str, text: &str, class: &str) -> String {
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
        object
    }

    pub fn retire(&self, object_id: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object_id}")), |envelope| {
                envelope.retire_observation(object_id)?;
                Ok(String::new())
            })
            .unwrap();
    }

    pub fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    pub fn export(&self) -> Vec<SourceRow> {
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

    pub fn bootstrap(&self, data_home: &Path) -> (SearchProjection, Vec<SourceRow>) {
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

pub fn occurrence_of<'a>(rows: &'a [SourceRow], object_id: &str) -> &'a str {
    &rows
        .iter()
        .find(|row| row.object_id == object_id)
        .unwrap()
        .detail
        .occurrence_id
}

pub fn search_path(data_home: &Path) -> PathBuf {
    data_home.join("search").join("search.sqlite")
}

pub fn inspect(data_home: &Path) -> Connection {
    Connection::open_with_flags(search_path(data_home), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

/// Retires `occurrence` at `commit_seq` and obsoletes its open embedding work, as the batch applier does for an occurrence that stopped being live.
pub fn tombstone(projection: &SearchProjection, occurrence: &str, commit_seq: i64) {
    projection
        .write(|conn| {
            tombstone_occurrence(
                conn,
                occurrence,
                Tombstone {
                    invalidated_commit_seq: commit_seq,
                    reason: TombstoneReason::Retired,
                },
                NOW,
            )?;
            conn.execute(
                "UPDATE embedding_jobs SET state='obsolete',updated_at=?2 WHERE occurrence_id=?1 AND state IN ('pending','admitted')",
                rusqlite::params![occurrence, NOW],
            )?;
            Ok(())
        })
        .unwrap();
}
