//! Bounded identity sweeps against a real kernel, a real projection, and an in-process Synapse host.
//! An independent reference and holder ledger predicts every survivor; a genuinely unreferenced identity is gone after reopen; live work, shared payloads, and host-held jobs survive selection races and rechecks; a lost reclamation reply is reconciled without a second effect.

use std::collections::BTreeSet;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use daemon::embedding_dispatch::{Blocked, DispatchBounds, DispatchEvent, EmbeddingDispatcher};
use daemon::identity_sweep::{IdentitySweeper, SweepReport};
use daemon::search_projection::SearchProjection;
use host_runtime::synapse::PollOutcome;
use host_runtime::synapse::inference::InferenceError;
use host_runtime::synapse::{
    EmbedTokens, EmbeddingEngine, LaneInfo, SynapseComponent, SynapseLimits,
};
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
use retrieval::identity_sweep::{Candidate, candidates, reclaim};
use retrieval::vectors::encode;
use retrieval::{
    PersistBounds, ProjectionIdentity, Tombstone, TombstoneReason, install_identity,
    tombstone_occurrence,
};
use rusqlite::{Connection, OpenFlags};
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
/// The retired generation the sweep may reclaim from.
const OLD_GENERATION: &str = "gen-0";
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

fn ten() -> NonZeroUsize {
    NonZeroUsize::new(10).unwrap()
}

/// The kind of the row an identity pair still has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Row {
    Job,
    Vector,
}

/// Every `(occurrence, generation, row)` the projection holds, read outside the API under test.
fn inventory(data_home: &Path) -> BTreeSet<(String, String, Row)> {
    let conn = inspect(data_home);
    let mut rows = BTreeSet::new();
    let mut jobs = conn
        .prepare("SELECT occurrence_id,generation_id FROM embedding_jobs")
        .unwrap();
    for pair in jobs
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
    {
        let (occurrence, generation) = pair.unwrap();
        rows.insert((occurrence, generation, Row::Job));
    }
    let mut vectors = conn
        .prepare("SELECT occurrence_id,generation_id FROM occurrence_vectors")
        .unwrap();
    for pair in vectors
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
    {
        let (occurrence, generation) = pair.unwrap();
        rows.insert((occurrence, generation, Row::Vector));
    }
    rows
}

/// One row of the independent reference ledger: what the test knows about an identity pair, from which it predicts survival on its own.
#[derive(Debug, Clone)]
struct Reference {
    occurrence: String,
    generation: String,
    state: String,
    tombstoned: bool,
    retired_with_receipt: bool,
    /// The host still answers for the job and its result was never consumed by a completion.
    held_by_host: bool,
}

/// The test's own rule, in the ticket's words: a row with an obligation left on it (pending, admitted, or stopped awaiting an operator), a row whose occurrence is live under a generation that is not retired, and a row whose native work or leased result the host still holds all survive.
fn survives(reference: &Reference) -> bool {
    let obligation = matches!(reference.state.as_str(), "pending" | "admitted" | "failed");
    let referenced = !reference.tombstoned && !reference.retired_with_receipt;
    obligation || referenced || reference.held_by_host
}

/// Reads every job pair's ledger facts outside the API under test, asking the host directly whether it still answers for the row's job.
fn references(data_home: &Path, synapse: &SynapseComponent) -> Vec<Reference> {
    let conn = inspect(data_home);
    let mut statement = conn
        .prepare(
            "SELECT j.occurrence_id,j.generation_id,j.state,j.host_job_id,j.episode_id,CAST(p.bytes AS TEXT),
                    EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=j.occurrence_id),
                    EXISTS(SELECT 1 FROM retirement_receipts r WHERE r.generation_id=j.generation_id)
                    AND (SELECT state FROM vector_generations g WHERE g.generation_id=j.generation_id)='retired'
             FROM embedding_jobs j
             JOIN occurrences o ON o.occurrence_id=j.occurrence_id
             JOIN payloads p ON p.payload_id=o.payload_id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            let state: String = row.get(2)?;
            let host_job_id: Option<String> = row.get(3)?;
            let episode: Option<String> = row.get(4)?;
            let text: String = row.get(5)?;
            // A completed row consumed its result; anything else the host still answers for is held.
            let held_by_host = state != "embedded"
                && host_job_id
                    .as_deref()
                    .zip(episode.as_deref())
                    .is_some_and(|(job, episode)| {
                        !matches!(
                            synapse.poll_admitted(job, episode, &text),
                            PollOutcome::Restarted
                        )
                    });
            Ok(Reference {
                occurrence: row.get(0)?,
                generation: row.get(1)?,
                state,
                tombstoned: row.get(6)?,
                retired_with_receipt: row.get(7)?,
                held_by_host,
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// What the ledger predicts remains after a sweep of every candidate.
fn predicted(data_home: &Path, synapse: &SynapseComponent) -> BTreeSet<(String, String, Row)> {
    let before = inventory(data_home);
    let survivors: BTreeSet<(String, String)> = references(data_home, synapse)
        .into_iter()
        .filter(survives)
        .map(|reference| (reference.occurrence, reference.generation))
        .collect();
    before
        .into_iter()
        .filter(|(occurrence, generation, _)| {
            survivors.contains(&(occurrence.clone(), generation.clone()))
        })
        .collect()
}

/// The projection's checkpoint and identity rows, which no sweep may touch.
fn singletons(data_home: &Path) -> (Vec<(String, i64)>, i64) {
    let conn = inspect(data_home);
    let checkpoint = conn
        .query_row(
            "SELECT snapshot_commit_seq,checkpoint_commit_seq,updated_at FROM projection_checkpoint",
            [],
            |row| {
                Ok(vec![
                    ("snapshot".to_string(), row.get::<_, i64>(0)?),
                    ("checkpoint".to_string(), row.get::<_, i64>(1)?),
                    ("updated".to_string(), row.get::<_, i64>(2)?),
                ])
            },
        )
        .unwrap();
    let installed: i64 = conn
        .query_row("SELECT installed_at FROM projection_identity", [], |row| {
            row.get(0)
        })
        .unwrap();
    (checkpoint, installed)
}

fn payload_count(data_home: &Path) -> i64 {
    inspect(data_home)
        .query_row("SELECT COUNT(*) FROM payloads", [], |row| row.get(0))
        .unwrap()
}

/// Vectors of live occurrences under the current generation: the set a sweep must never change.
fn required_vectors(data_home: &Path) -> BTreeSet<(String, Vec<u8>)> {
    let conn = inspect(data_home);
    let mut statement = conn
        .prepare(
            "SELECT v.occurrence_id,v.vector FROM occurrence_vectors v
             WHERE v.generation_id=?1
               AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=v.occurrence_id)",
        )
        .unwrap();
    statement
        .query_map([GENERATION], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Retires `OLD_GENERATION` with a receipt and gives `occurrences` an embedded job and a vector under it, as a finished model roll leaves behind.
/// `unfinished` rows are planted in the given states, each an obligation the sweep must keep.
fn plant_retired_generation(
    projection: &SearchProjection,
    occurrences: &[(&str, &str)],
    unfinished: &[(&str, &str)],
) {
    let old = VectorGeneration {
        generation_id: OLD_GENERATION.to_string(),
        generation_epoch: 0,
        ..generation()
    };
    projection
        .write(|conn| {
            register_generation(conn, &old, 1)?;
            conn.execute(
                "UPDATE vector_generations SET state='retired',updated_at=2 WHERE generation_id=?1",
                [OLD_GENERATION],
            )?;
            conn.execute(
                "INSERT INTO retirement_receipts(receipt_id,generation_id,reason,operator_id,retired_at,recorded_at)
                 VALUES ('receipt-gen-0',?1,'superseded by gen-1','operator',2,2)",
                [OLD_GENERATION],
            )?;
            for (occurrence, text) in occurrences {
                conn.execute(
                    "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,created_at,updated_at)
                     VALUES (?1,?2,?3,'embedded',1,1,1)",
                    rusqlite::params![format!("old-{occurrence}"), occurrence, OLD_GENERATION],
                )?;
                conn.execute(
                    "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at)
                     VALUES (?1,?2,?3,8,1,1,1)",
                    rusqlite::params![occurrence, OLD_GENERATION, encode(&TestEngine::vector_for(text))],
                )?;
            }
            for (occurrence, state) in unfinished {
                conn.execute(
                    "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,attempts,host_job_id,stop_reason,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,1,?5,?6,1,1)",
                    rusqlite::params![
                        format!("old-{occurrence}"),
                        occurrence,
                        OLD_GENERATION,
                        state,
                        (*state == "admitted").then_some("dead-host-1"),
                        (*state == "failed").then_some("operator_stop"),
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
}

fn tombstone(projection: &SearchProjection, occurrence: &str, commit_seq: i64) {
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
            // The batch applier obsoletes open work for an occurrence that stopped being live; the same transition is applied here.
            conn.execute(
                "UPDATE embedding_jobs SET state='obsolete',updated_at=?2 WHERE occurrence_id=?1 AND state IN ('pending','admitted')",
                rusqlite::params![occurrence, NOW],
            )?;
            Ok(())
        })
        .unwrap();
}

fn sweep(
    projection: &SearchProjection,
    synapse: &SynapseComponent,
    limit: NonZeroUsize,
) -> SweepReport {
    let mut sweeper = IdentitySweeper::new(projection, synapse);
    tokio::task::block_in_place(|| sweeper.run_sweep(limit).unwrap())
}

fn embed_all(corpus: &Corpus, projection: &SearchProjection, synapse: &SynapseComponent) -> usize {
    let (_, events) = pass(
        corpus,
        projection,
        synapse,
        &bounds(Duration::from_secs(5)),
        NOW,
    );
    events
        .iter()
        .filter(|event| matches!(event, DispatchEvent::Published { .. }))
        .count()
}

/// AC1, AC2, AC5: the independent ledger predicts the survivors of a sweep over current, obsolete, pending, stopped, and retired states; the unreferenced identities are gone after reopen; a sweep with nothing eligible reclaims nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reference_ledger_predicts_survivors_and_eligible_identities_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let names = ["alpha", "beta", "gamma", "delta", "echo", "foxtrot"];
    let texts = [
        "alpha text",
        "shared bytes",
        "shared bytes",
        "delta text",
        "echo text",
        "foxtrot text",
    ];
    let objects: Vec<String> = names
        .iter()
        .zip(texts)
        .map(|(name, text)| corpus.publish(name, text))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occ: Vec<&str> = objects
        .iter()
        .map(|object| occurrence_of(&rows, object))
        .collect();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());

    // Nothing is eligible on a fresh projection: a sweep is a witnessed no-op.
    let untouched = inventory(dir.path());
    let report = sweep(&projection, &synapse, ten());
    assert_eq!(report, SweepReport::default());
    assert_eq!(inventory(dir.path()), untouched);

    // Current generation: alpha, beta, gamma, delta embedded; echo left pending; foxtrot stopped.
    projection
        .write(|conn| {
            retrieval::dispatch::stop_job(
                conn,
                &job_id_of(dir.path(), occ[5]),
                "operator_stop",
                NOW,
            )?;
            conn.execute(
                "UPDATE embedding_jobs SET next_attempt_at=?2 WHERE occurrence_id=?1",
                rusqlite::params![occ[4], NOW + DAY_MS],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        embed_all(&corpus, &projection, &synapse),
        4,
        "echo is held back below"
    );
    // Retired generation: alpha and delta carry old vectors; echo, foxtrot, and gamma carry pending, stopped, and admitted-by-a-dead-host obligations.
    plant_retired_generation(
        &projection,
        &[(occ[0], texts[0]), (occ[3], texts[3])],
        &[
            (occ[4], "pending"),
            (occ[5], "failed"),
            (occ[2], "admitted"),
        ],
    );
    // beta stops being live; gamma shares its bytes and stays live.
    tombstone(&projection, occ[1], 50);
    let required = required_vectors(dir.path());
    let payloads = payload_count(dir.path());
    let expected = predicted(dir.path(), &synapse);
    assert!(
        expected.len() < inventory(dir.path()).len(),
        "the ledger predicts real reclamation"
    );
    let fixed = singletons(dir.path());

    let report = sweep(&projection, &synapse, ten());
    assert_eq!(report.held, Vec::<Candidate>::new());
    assert_eq!(report.survivors, 0);
    assert_eq!(
        (
            report.candidates,
            report.jobs_reclaimed,
            report.vectors_reclaimed
        ),
        (3, 3, 3)
    );
    assert_eq!(inventory(dir.path()), expected);
    for (occurrence, generation) in [
        (occ[0], OLD_GENERATION),
        (occ[3], OLD_GENERATION),
        (occ[1], GENERATION),
    ] {
        assert!(
            !expected
                .iter()
                .any(|(o, g, _)| o == occurrence && g == generation),
            "{occurrence} {generation} reclaimed"
        );
    }
    for (occurrence, what) in [
        (occ[4], "pending"),
        (occ[5], "stopped"),
        (occ[2], "admitted by a dead host"),
    ] {
        assert!(
            expected
                .iter()
                .any(|(o, g, r)| o == occurrence && g == OLD_GENERATION && *r == Row::Job),
            "a {what} job under a retired generation survives"
        );
    }
    assert!(
        expected
            .iter()
            .any(|(o, g, r)| o == occ[5] && g == GENERATION && *r == Row::Job),
        "a stopped job survives"
    );
    assert_eq!(
        singletons(dir.path()),
        fixed,
        "checkpoint and identity are untouched"
    );
    assert_eq!(
        required_vectors(dir.path()),
        required,
        "current required vectors are unchanged"
    );
    assert_eq!(
        payload_count(dir.path()),
        payloads,
        "beta's shared payload stays with gamma"
    );
    let tombstones: i64 = inspect(dir.path())
        .query_row("SELECT COUNT(*) FROM occurrence_tombstones", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(tombstones, 1, "the tombstone is a replay record and stays");

    drop(projection);
    let projection = SearchProjection::open(dir.path()).unwrap();
    assert_eq!(inventory(dir.path()), expected, "reopen changes nothing");
    let again = sweep(&projection, &synapse, ten());
    assert_eq!(
        again,
        SweepReport::default(),
        "a second sweep finds nothing left"
    );
    assert_eq!(engine.calls(), 4);
}

fn job_id_of(data_home: &Path, occurrence: &str) -> String {
    inspect(data_home)
        .query_row(
            "SELECT job_id FROM embedding_jobs WHERE occurrence_id=?1 AND generation_id=?2",
            [occurrence, GENERATION],
            |row| row.get(0),
        )
        .unwrap()
}

/// AC1, AC2: a bounded sweep reclaims at most its limit and leaves the rest eligible for the next.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bounded_sweep_reclaims_at_most_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|name| corpus.publish(name, &format!("{name} text")))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    assert_eq!(embed_all(&corpus, &projection, &synapse), 3);
    for (seq, object) in objects.iter().enumerate() {
        tombstone(&projection, occurrence_of(&rows, object), 50 + seq as i64);
    }
    let one = NonZeroUsize::new(1).unwrap();
    let first = sweep(&projection, &synapse, one);
    assert_eq!(
        (
            first.candidates,
            first.jobs_reclaimed,
            first.vectors_reclaimed
        ),
        (1, 1, 1)
    );
    assert_eq!(inventory(dir.path()).len(), 4);
    let rest = sweep(&projection, &synapse, ten());
    assert_eq!(
        (rest.candidates, rest.jobs_reclaimed, rest.vectors_reclaimed),
        (2, 2, 2)
    );
    assert!(inventory(dir.path()).is_empty());
}

/// AC3: admission, completion, tombstoning, and a reader race candidate selection. Live work and the shared payload survive; a candidate that a concurrent writer reopened survives the recheck; an identity released after selection is reclaimable by the next sweep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn races_with_selection_preserve_live_work_and_release_makes_candidates_reclaimable() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let names = ["old", "twin", "late", "fresh"];
    let texts = ["shared bytes", "shared bytes", "late text", "fresh text"];
    let objects: Vec<String> = names
        .iter()
        .zip(texts)
        .map(|(name, text)| corpus.publish(name, text))
        .collect();
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occ: Vec<&str> = objects
        .iter()
        .map(|object| occurrence_of(&rows, object))
        .collect();
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    // old, twin, late are embedded; fresh stays pending for the race.
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE embedding_jobs SET next_attempt_at=?2 WHERE occurrence_id=?1",
                rusqlite::params![occ[3], NOW + DAY_MS],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(embed_all(&corpus, &projection, &synapse), 3);
    tombstone(&projection, occ[0], 50);
    plant_retired_generation(&projection, &[(occ[2], texts[2])], &[]);

    // Selection observes two candidates: old under the current generation, late under the retired one.
    let selected = projection.read(|conn| candidates(conn, ten())).unwrap();
    let mut pairs: Vec<(&str, &str)> = selected
        .iter()
        .map(|candidate| {
            (
                candidate.occurrence_id.as_str(),
                candidate.generation_id.as_str(),
            )
        })
        .collect();
    pairs.sort_unstable();
    let mut wanted = vec![(occ[0], GENERATION), (occ[2], OLD_GENERATION)];
    wanted.sort_unstable();
    assert_eq!(pairs, wanted);

    // Between selection and reclamation: fresh is admitted and completed, late's old job is reopened by another writer, and twin's identity is released by a tombstone.
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE embedding_jobs SET next_attempt_at=NULL WHERE occurrence_id=?1",
                [occ[3]],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(embed_all(&corpus, &projection, &synapse), 1);
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE embedding_jobs SET state='pending' WHERE occurrence_id=?1 AND generation_id=?2",
                [occ[2], OLD_GENERATION],
            )?;
            Ok(())
        })
        .unwrap();
    tombstone(&projection, occ[1], 60);
    let payloads = payload_count(dir.path());
    let required = required_vectors(dir.path());

    // A reader holds a snapshot while the reclamation runs.
    let reader =
        Connection::open_with_flags(search_path(dir.path()), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let seen_by_reader: i64 = reader
        .query_row("SELECT COUNT(*) FROM occurrence_vectors", [], |row| {
            row.get(0)
        })
        .unwrap();
    let reclaimed = projection.write(|conn| reclaim(conn, &selected)).unwrap();
    assert_eq!(
        (reclaimed.jobs, reclaimed.vectors, reclaimed.survivors),
        (1, 1, 1)
    );
    assert_eq!(
        reader
            .query_row("SELECT COUNT(*) FROM occurrence_vectors", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        seen_by_reader,
        "the reader keeps its snapshot"
    );
    reader.execute_batch("COMMIT").unwrap();

    let after = inventory(dir.path());
    assert!(
        !after.iter().any(|(o, g, _)| o == occ[0] && g == GENERATION),
        "old is reclaimed"
    );
    assert!(
        after.contains(&(occ[2].to_string(), OLD_GENERATION.to_string(), Row::Job)),
        "the reopened job survives the recheck"
    );
    assert!(after.contains(&(occ[2].to_string(), OLD_GENERATION.to_string(), Row::Vector)));
    assert!(
        after.contains(&(occ[3].to_string(), GENERATION.to_string(), Row::Vector)),
        "fresh's new vector survives"
    );
    assert!(
        after.contains(&(occ[1].to_string(), GENERATION.to_string(), Row::Vector)),
        "twin, released after selection, waits for the next sweep"
    );
    assert_eq!(
        payload_count(dir.path()),
        payloads,
        "the shared payload survives its first occurrence"
    );
    assert_eq!(required_vectors(dir.path()), required);

    // The next sweep reclaims what release made eligible and nothing else.
    let report = sweep(&projection, &synapse, ten());
    assert_eq!(
        (
            report.candidates,
            report.jobs_reclaimed,
            report.vectors_reclaimed,
            report.survivors
        ),
        (1, 1, 1, 0)
    );
    let after = inventory(dir.path());
    assert!(!after.iter().any(|(o, _, _)| o == occ[1]));
    assert_eq!(payload_count(dir.path()), payloads);
    assert_eq!(required_vectors(dir.path()), required);
}

/// AC4: a job the host still holds is not reclaimed even though its row is obsolete; the holder outlives the caller's pass and the native result, and only its physical exit from the table makes the identity reclaimable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_native_work_survives_until_the_host_releases_it() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("held", "held text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object);
    let engine = TestEngine::new();
    let gate = engine.block_calls();
    let synapse = component(
        &engine,
        SynapseLimits {
            retention: Duration::from_millis(200),
            ..SynapseLimits::default()
        },
    );
    // The caller's pass returns while the host still runs the job.
    let (_, events) = pass(
        &corpus,
        &projection,
        &synapse,
        &bounds(Duration::from_millis(50)),
        NOW,
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, DispatchEvent::Admitted { .. }))
            .count(),
        1
    );
    tombstone(&projection, occurrence, 50);
    let state: String = inspect(dir.path())
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "obsolete");

    // Native work still running: held.
    let report = sweep(&projection, &synapse, ten());
    assert_eq!(
        (report.candidates, report.held.len(), report.jobs_reclaimed),
        (1, 1, 0)
    );
    assert_eq!(inventory(dir.path()).len(), 1);
    assert_eq!(predicted(dir.path(), &synapse), inventory(dir.path()));

    // Native exit with the result leased in the table: still held, and the host proves the result is ready.
    TestEngine::release(&gate);
    let settled = std::time::Instant::now();
    while engine.completed() == 0 && settled.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (host_job, episode): (String, String) = inspect(dir.path())
        .query_row(
            "SELECT host_job_id,episode_id FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let ready = std::time::Instant::now();
    while !matches!(
        synapse.poll_admitted(&host_job, &episode, "held text"),
        PollOutcome::Page(_)
    ) && ready.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        synapse.poll_admitted(&host_job, &episode, "held text"),
        PollOutcome::Page(_)
    ));
    let report = sweep(&projection, &synapse, ten());
    assert_eq!(
        (report.candidates, report.held.len(), report.jobs_reclaimed),
        (1, 1, 0)
    );
    assert_eq!(predicted(dir.path(), &synapse), inventory(dir.path()));

    // Once the table forgets the job, nothing holds the identity.
    std::thread::sleep(Duration::from_millis(400));
    let report = sweep(&projection, &synapse, ten());
    assert_eq!(
        (report.candidates, report.held.len(), report.jobs_reclaimed),
        (1, 0, 1)
    );
    assert!(inventory(dir.path()).is_empty());
    assert_eq!(engine.calls(), 1);
}

/// AC5: a reclamation whose COMMIT reply is lost is reconciled from the rows: the report matches what the store applied, a second sweep has no second effect, and the sweeper is not quarantined.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_reclaim_reply_is_reconciled_without_a_second_effect() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let gone = corpus.publish("gone", "gone text");
    let kept = corpus.publish("kept", "kept text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let engine = TestEngine::new();
    let synapse = component(&engine, SynapseLimits::default());
    assert_eq!(embed_all(&corpus, &projection, &synapse), 2);
    tombstone(&projection, occurrence_of(&rows, &gone), 50);
    let expected = predicted(dir.path(), &synapse);
    let required = required_vectors(dir.path());
    let before = inventory(dir.path());

    // A reply lost because the write never applied: another writer holds the file, the store gives up, and the rows say nothing changed.
    let blocker = Connection::open(search_path(dir.path())).unwrap();
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut sweeper = IdentitySweeper::new(&projection, &synapse);
    let report = tokio::task::block_in_place(|| sweeper.run_sweep(ten()).unwrap());
    assert_eq!(
        (
            report.candidates,
            report.jobs_reclaimed,
            report.vectors_reclaimed,
            report.survivors
        ),
        (1, 0, 0, 1)
    );
    assert_eq!(inventory(dir.path()), before, "nothing was reclaimed");
    blocker.execute_batch("COMMIT").unwrap();

    // A reply lost after the write applied: the rows say the identity is gone, once.
    let mut sweeper = IdentitySweeper::new(&projection, &synapse);
    sweeper.lose_next_reclaim_reply_for_test();
    let report = tokio::task::block_in_place(|| sweeper.run_sweep(ten()).unwrap());
    assert_eq!(
        (
            report.candidates,
            report.jobs_reclaimed,
            report.vectors_reclaimed,
            report.survivors
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(inventory(dir.path()), expected);
    assert!(
        inventory(dir.path())
            .iter()
            .any(|(o, _, _)| o == occurrence_of(&rows, &kept))
    );
    assert_eq!(required_vectors(dir.path()), required);
    let again = tokio::task::block_in_place(|| sweeper.run_sweep(ten()).unwrap());
    assert_eq!(again, SweepReport::default());
    drop(projection);
    let _ = SearchProjection::open(dir.path()).unwrap();
    assert_eq!(inventory(dir.path()), expected);
}
