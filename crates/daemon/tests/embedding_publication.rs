//! Guarded vector publication against a real kernel and a real projection.
//! Canonical mutations wait behind the guard until the local transaction releases, stale inputs make jobs obsolete instead of completing them, and the vector and its completion survive every crash cut together or not at all.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use daemon::embedding_publication::{
    EmbeddingPublisher, ObsoleteCause, Publication, PublicationError, PublicationEvent,
    PublicationFault, VectorPublication,
};
use daemon::search_projection::SearchProjection;
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, ArtifactIngestRequest, BackupRequest,
    CommitIntent, CurrentInputExpectation, Dimension, DomainSpec, EligibilityBinding,
    EligibilityVerdict, EventKind, ExportWindow, KernelStore, ProjectScope, ProviderEgress,
    RemediationTarget, RepositoryProvenance, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass,
    SourceDescriptorRequest, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds,
    SourcePageBounds, SourceRow, StaleInput, TaintClass,
};
use retrieval::ProjectionError;
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::vectors::{
    CompletionOutcome, ObsoleteReason, VectorCompletion, complete_embedding_observed,
    completion_status, encode,
};
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

const CONSUMER: &str = "search";
const KERNEL_INCARNATION: &str = "kernel-1";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

const CHILD_ROOT: &str = "EIDNARA_EMBEDDING_PUBLICATION_CHILD_ROOT";
const CHILD_CUT: &str = "EIDNARA_EMBEDDING_PUBLICATION_CHILD_CUT";
const CHILD_EXPECTATION: &str = "EIDNARA_EMBEDDING_PUBLICATION_CHILD_EXPECTATION";
const CHILD_BARRIER: &str = "EIDNARA_EMBEDDING_PUBLICATION_BARRIER";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-embedding-publication-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn generation(dimension: u32) -> VectorGeneration {
    VectorGeneration {
        generation_id: format!("gen-{dimension}"),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: dimension,
        generation_epoch: 1,
    }
}

fn identity(dimension: u32) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: KERNEL_INCARNATION.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: dimension,
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

fn unit(dimension: u32) -> Vec<f32> {
    let mut vector = vec![0.0; dimension as usize];
    vector[0] = 1.0;
    vector
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn eligibility(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Remote,
    }
}

/// A kernel with one scoped, admitted project and the descriptors the test publishes into it.
struct Corpus {
    kernel: Arc<KernelStore>,
    published: RefCell<Vec<(String, String, String)>>,
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

    fn ingest(&self, key: &str, text: &str, sensitivity: Sensitivity) -> kernel::ArtifactHandle {
        self.kernel
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
            .unwrap()
    }

    /// Publishes one scoped, admitted message descriptor and returns its object id.
    fn publish(&self, key: &str, message_id: &str, revision: &str, text: &str) -> String {
        let handle = self.ingest(key, text, Sensitivity::Normal);
        let mut object = String::new();
        self.kernel
            .commit(intent(&format!("publish-{key}")), |envelope| {
                let identity = [
                    ("project_id", "proj-a"),
                    ("harness", "opencode"),
                    ("session_id", "sess-01"),
                    ("message_id", message_id),
                    ("block_index", "0"),
                ];
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        occurrence: Occurrence {
                            class: "messages",
                            identity: &identity,
                            revision,
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
        self.published
            .borrow_mut()
            .push((key.to_string(), object.clone(), text.to_string()));
        object
    }

    fn retire(&self, object_id: &str) -> i64 {
        self.kernel
            .commit(intent(&format!("retire-{object_id}")), |envelope| {
                envelope.retire_observation(object_id)?;
                Ok(String::new())
            })
            .unwrap()
            .commit_seq
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

    /// Builds the projection from the current export and queues one job per message under `generation`.
    fn bootstrap(
        &self,
        data_home: &Path,
        generation: &VectorGeneration,
    ) -> (SearchProjection, Vec<SourceRow>) {
        let rows = self.export();
        let projection = SearchProjection::open(data_home).unwrap();
        projection
            .write(|conn| {
                install_identity(conn, &identity(generation.vector_dimension), 1)?;
                register_generation(conn, generation, 1)?;
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
            Some(&generation.generation_id),
        )
        .unwrap();
        projection.apply_batch(&batch, batch_bounds(), 2).unwrap();
        (projection, rows)
    }
}

fn expectation_for(row: &SourceRow) -> CurrentInputExpectation {
    CurrentInputExpectation {
        object_id: row.object_id.clone(),
        source_revision: row.revision,
        occurrence_id: row.detail.occurrence_id.clone(),
        payload_id: row.detail.payload_id.clone(),
        artifact_digest: row.detail.artifact_digest.clone(),
    }
}

fn row_for<'a>(rows: &'a [SourceRow], object_id: &str) -> &'a SourceRow {
    rows.iter().find(|row| row.object_id == object_id).unwrap()
}

fn inspect(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn mutate(path: &Path) -> Connection {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection
}

fn search_path(data_home: &Path) -> PathBuf {
    data_home.join("search").join("search.sqlite")
}

/// The projection's durable job state and vector bytes for one occurrence, read outside every API under test.
fn durable(data_home: &Path, occurrence_id: &str) -> (Option<String>, Option<Vec<u8>>) {
    let conn = inspect(&search_path(data_home));
    use rusqlite::OptionalExtension;
    let state = conn
        .query_row(
            "SELECT state FROM embedding_jobs WHERE occurrence_id=?1",
            [occurrence_id],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    let vectors: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM occurrence_vectors WHERE occurrence_id=?1",
            [occurrence_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        vectors <= 1,
        "one vector row per occurrence in this fixture"
    );
    let vector = conn
        .query_row(
            "SELECT vector FROM occurrence_vectors WHERE occurrence_id=?1",
            [occurrence_id],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    (state, vector)
}

fn publication<'a>(
    row: &SourceRow,
    generation: &'a VectorGeneration,
    vector: &'a [f32],
) -> VectorPublication<'a> {
    VectorPublication {
        expectation: expectation_for(row),
        generation,
        vector,
        input_bytes: row.text.as_ref().map_or(0, |text| text.len() as u64),
        input_tokens: 3,
    }
}

/// A kernel commit must complete promptly: the guard is released.
fn assert_kernel_writable(corpus: &Corpus, key: &str) {
    corpus
        .kernel
        .commit_before(Instant::now() + Duration::from_secs(2), intent(key), |_| {
            Ok(String::new())
        })
        .expect("the kernel writer is free");
}

/// Runs `publish` while a canonical mutation is attempted from another thread once the guard is held, and proves the mutation completes only after the guard is released.
fn publish_while_mutating<T: Send + 'static>(
    publisher: &mut EmbeddingPublisher<'_>,
    publication: &VectorPublication<'_>,
    project: &ProjectScope,
    mutation: impl FnOnce() -> T + Send + 'static,
) -> (
    Result<Publication, PublicationError>,
    Vec<PublicationEvent>,
    T,
) {
    let done = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut events = Vec::new();
    let mut mutation = Some(mutation);
    let done_for_thread = Arc::clone(&done);
    let started_for_thread = Arc::clone(&started);
    let result = publisher.publish(
        publication,
        eligibility(project),
        deadline(),
        3,
        &mut |event| {
            events.push(event);
            match event {
                PublicationEvent::GuardAcquired => {
                    let mutation = mutation.take().unwrap();
                    let done = Arc::clone(&done_for_thread);
                    let started = Arc::clone(&started_for_thread);
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        started.store(true, Ordering::SeqCst);
                        let value = mutation();
                        done.store(true, Ordering::SeqCst);
                        let _ = tx.send(value);
                    });
                }
                PublicationEvent::LocalReleased => {
                    let wait = Instant::now();
                    while !started.load(Ordering::SeqCst) {
                        assert!(
                            wait.elapsed() < Duration::from_secs(5),
                            "the mutation thread starts"
                        );
                        std::thread::yield_now();
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    assert!(
                        !done.load(Ordering::SeqCst),
                        "the canonical mutation waits behind the guard"
                    );
                }
                _ => {}
            }
        },
    );
    let value = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the mutation completes once the guard is released");
    assert!(done.load(Ordering::SeqCst));
    (result, events, value)
}

fn assert_guarded_order(events: &[PublicationEvent]) {
    let wanted = [
        PublicationEvent::VectorValidated,
        PublicationEvent::GuardRequested,
        PublicationEvent::GuardAcquired,
        PublicationEvent::VectorStaged,
        PublicationEvent::LocalStaged,
        PublicationEvent::LocalReleased,
        PublicationEvent::GuardReleased,
    ];
    assert_eq!(events, wanted, "{events:?}");
}

fn publish_once(
    publisher: &mut EmbeddingPublisher<'_>,
    publication: &VectorPublication<'_>,
    project: &ProjectScope,
) -> (Result<Publication, PublicationError>, Vec<PublicationEvent>) {
    let mut events = Vec::new();
    let result = publisher.publish(
        publication,
        eligibility(project),
        deadline(),
        3,
        &mut |event| events.push(event),
    );
    (result, events)
}

/// The independently computed identity of the exact input bytes.
fn payload_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[test]
fn canonical_mutations_wait_behind_the_guard_and_stale_inputs_become_obsolete() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let revised = corpus.publish("a", "msg-a", "1", "first message");
    let retired = corpus.publish("b", "msg-b", "1", "second message");
    let remediated = corpus.publish("c", "msg-c", "1", "third message");
    let reclassified = corpus.publish("d", "msg-d", "1", "fourth message");
    let restored = corpus.publish("e", "msg-e", "1", "fifth message");
    let generation = generation(8);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = unit(8);
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);

    // A new revision published while the guard is held waits until the local transaction released.
    let row = row_for(&rows, &revised);
    let kernel = Arc::clone(&corpus.kernel);
    let (result, events, replaced) = publish_while_mutating(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
        move || {
            let handle = kernel
                .ingest_exact_artifact(ArtifactIngestRequest {
                    intent: intent("artifact-a2"),
                    payload: b"first message, revised".to_vec(),
                    evidence_id: "evidence-a2".to_string(),
                    object_id: "evidence-object-a2".to_string(),
                    object_kind: "evidence".to_string(),
                    domain_id: "domain".to_string(),
                    source_kind: "tool_output".to_string(),
                    source_id: "native/a2".to_string(),
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
            kernel
                .commit(intent("publish-a2"), |envelope| {
                    let identity = [
                        ("project_id", "proj-a"),
                        ("harness", "opencode"),
                        ("session_id", "sess-01"),
                        ("message_id", "msg-a"),
                        ("block_index", "0"),
                    ];
                    let outcome = envelope
                        .publish_source_descriptor(&SourceDescriptorRequest {
                            source_policy: kernel::SourceDescriptorPolicy::Native,
                            occurrence: Occurrence {
                                class: "messages",
                                identity: &identity,
                                revision: "2",
                                representation: "text",
                                span: None,
                            },
                            domain_id: "domain",
                            scope_id: Some(SCOPE),
                            evidence_id: &handle.evidence_id,
                            artifact_digest: &handle.digest,
                            buffer: "first message, revised",
                            sensitivity: Sensitivity::Normal,
                            observed_at: 1,
                        })
                        .unwrap();
                    Ok(outcome.replaced_object_id.unwrap_or_default())
                })
                .unwrap()
                .result
        },
    );
    assert_guarded_order(&events);
    assert_eq!(result.unwrap(), Publication::Embedded);
    assert_eq!(
        replaced, revised,
        "the supersession landed after the guard released"
    );
    let (state, bytes) = durable(dir.path(), &row.detail.occurrence_id);
    assert_eq!(
        (state.as_deref(), bytes),
        (Some("embedded"), Some(encode(&vector)))
    );
    // The same result offered again for the now-superseded revision is refused by the guard, not completed.
    let (again, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        again.unwrap(),
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Superseded))
    );
    assert!(
        !events.contains(&PublicationEvent::GuardAcquired),
        "{events:?}"
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("embedded".to_string()), Some(encode(&vector))),
        "a stale re-delivery cannot demote a completed job"
    );
    assert_kernel_writable(&corpus, "after-revision");

    // A retirement waits the same way, and a retired input can never complete afterwards.
    let row = row_for(&rows, &retired);
    let kernel = Arc::clone(&corpus.kernel);
    let object = retired.clone();
    let (result, events, _) = publish_while_mutating(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
        move || {
            kernel
                .commit(intent("retire-b"), |envelope| {
                    envelope.retire_observation(&object)?;
                    Ok(String::new())
                })
                .unwrap();
        },
    );
    assert_guarded_order(&events);
    assert_eq!(result.unwrap(), Publication::Embedded);
    let (again, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        again.unwrap(),
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Retracted))
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("embedded".to_string()), Some(encode(&vector)))
    );

    // Name-only domain remediation changes no input byte: the independently computed hash still matches and completion proceeds.
    corpus
        .kernel
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "domain-object".to_string(),
                },
                "operator",
                5,
            )?;
            Ok(String::new())
        })
        .unwrap();
    let row = row_for(&rows, &remediated);
    assert_eq!(row.detail.payload_id, payload_digest("third message"));
    let current = corpus.export();
    assert_eq!(
        row_for(&current, &remediated).detail.payload_id,
        payload_digest("third message")
    );
    let (result, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_guarded_order(&events);
    assert_eq!(result.unwrap(), Publication::Embedded);

    // A classification tightened by an exact ingest replay waits behind the guard like any other mutation, changes no descriptor row, yet withdraws eligibility afterwards.
    let row = row_for(&rows, &reclassified);
    let kernel = Arc::clone(&corpus.kernel);
    let (result, events, _) = publish_while_mutating(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
        move || {
            kernel
                .ingest_exact_artifact(ArtifactIngestRequest {
                    intent: intent("artifact-d"),
                    payload: b"fourth message".to_vec(),
                    evidence_id: "evidence-d".to_string(),
                    object_id: "evidence-object-d".to_string(),
                    object_kind: "evidence".to_string(),
                    domain_id: "domain".to_string(),
                    source_kind: "tool_output".to_string(),
                    source_id: "native/d".to_string(),
                    source_revision: 1,
                    media_type: "text/plain".to_string(),
                    retention_class: "canonical".to_string(),
                    retain_until: None,
                    asserted_sensitivity: Sensitivity::Sensitive,
                    provider_egress: ProviderEgress::RemoteAllowed,
                    provenance: Some(RepositoryProvenance {
                        repository_id: "repo".to_string(),
                        revision: "abc123".to_string(),
                    }),
                })
                .unwrap();
        },
    );
    assert_guarded_order(&events);
    assert_eq!(result.unwrap(), Publication::Embedded);
    let current = row_for(&corpus.export(), &reclassified).clone();
    assert_eq!(
        (
            current.revision,
            &current.detail.payload_id,
            current.invalidated_commit_seq
        ),
        (row.revision, &row.detail.payload_id, None),
        "the descriptor itself did not move"
    );
    let (result, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        result.unwrap(),
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Ineligible(
            EligibilityVerdict::ProviderSensitive
        )))
    );
    assert!(!events.contains(&PublicationEvent::GuardAcquired));
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("embedded".to_string()), Some(encode(&vector))),
        "the vector completed before the classification changed stays"
    );

    // A restore waits behind the guard too, and afterwards the input it removed cannot complete.
    let backup_dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(backup_dir.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let manifest = corpus
        .kernel
        .backup(BackupRequest {
            destination_directory: backup_dir.path().to_path_buf(),
            deadline: deadline(),
            capture_pin_expires_at: None,
        })
        .unwrap();
    let after_backup = corpus.publish("f", "msg-f", "1", "sixth message");
    let current = corpus.export();
    let fresh_row = row_for(&current, &after_backup);
    let row = row_for(&rows, &restored);
    let kernel = Arc::clone(&corpus.kernel);
    let backup_path = manifest.destination_path.clone();
    let (result, events, restored_tip) = publish_while_mutating(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
        move || kernel.restore(&backup_path).unwrap(),
    );
    assert_guarded_order(&events);
    assert_eq!(result.unwrap(), Publication::Embedded);
    assert_eq!(restored_tip, manifest.captured_commit_seq);
    // The descriptor published after the backup is gone; the guard refuses it, and the projection, which never queued it, has nothing to mark.
    let (again, events) = publish_once(
        &mut publisher,
        &publication(fresh_row, &generation, &vector),
        &project,
    );
    assert!(
        matches!(
            again,
            Err(PublicationError::Refused(
                ProjectionError::NoPendingWork { .. }
            ))
        ),
        "{again:?}"
    );
    assert!(!events.contains(&PublicationEvent::GuardAcquired));
    assert_kernel_writable(&corpus, "after-restore");
}

#[test]
fn an_unguarded_publication_of_a_stale_pre_read_is_the_control_the_guard_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let guarded = corpus.publish("a", "msg-a", "1", "first message");
    let unguarded = corpus.publish("b", "msg-b", "1", "second message");
    let generation = generation(8);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = unit(8);
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);

    // Both inputs are read, then both are retired before publication.
    let stale_guarded = publication(row_for(&rows, &guarded), &generation, &vector);
    let stale_unguarded = publication(row_for(&rows, &unguarded), &generation, &vector);
    corpus.retire(&guarded);
    corpus.retire(&unguarded);

    // The control commits the stale pre-read with no guard: the projection has not seen the retirement, so nothing local stops it.
    let control = projection
        .write(|conn| {
            complete_embedding_observed(
                conn,
                &VectorCompletion {
                    occurrence_id: &stale_unguarded.expectation.occurrence_id,
                    generation: &generation,
                    payload_id: &stale_unguarded.expectation.payload_id,
                    vector: &vector,
                    input_bytes: 14,
                    input_tokens: 3,
                },
                3,
                &mut |_| {},
            )
        })
        .unwrap();
    assert_eq!(
        control,
        CompletionOutcome::Embedded,
        "the unguarded control completes retired input"
    );

    // The guarded path refuses the same kind of stale pre-read and marks the job obsolete.
    let (result, events) = publish_once(&mut publisher, &stale_guarded, &project);
    assert_eq!(
        result.unwrap(),
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Retracted))
    );
    assert_eq!(
        events,
        [
            PublicationEvent::VectorValidated,
            PublicationEvent::GuardRequested
        ]
    );
    let (state, bytes) = durable(dir.path(), &stale_guarded.expectation.occurrence_id);
    assert_eq!((state.as_deref(), bytes), (Some("obsolete"), None));
    assert_kernel_writable(&corpus, "after-control");
}

fn f32_fixture() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vectors-f32.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn fixture_vector(value: &serde_json::Value) -> Vec<f32> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|component| match component {
            serde_json::Value::String(special) if special == "nan" => f32::NAN,
            serde_json::Value::String(special) if special == "inf" => f32::INFINITY,
            other => other.as_f64().unwrap() as f32,
        })
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

#[test]
fn f32_fixtures_pin_the_stored_representation_and_invalid_vectors_never_embed() {
    let fixture = f32_fixture();
    let dimension = fixture["dimension"].as_u64().unwrap() as u32;
    let valid = fixture["valid"].as_array().unwrap();
    let invalid = fixture["invalid"].as_array().unwrap();
    assert_eq!((valid.len(), invalid.len()), (5, 7));
    let reasons: std::collections::BTreeSet<&str> = invalid
        .iter()
        .map(|case| case["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        ["dimension", "nonfinite", "norm"].into_iter().collect()
    );

    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let objects: Vec<String> = (0..valid.len() + 1)
        .map(|index| {
            corpus.publish(
                &format!("v{index}"),
                &format!("msg-{index}"),
                "1",
                &format!("message {index}"),
            )
        })
        .collect();
    let generation = generation(dimension);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);

    // Every valid vector is stored as the pinned little-endian bytes.
    for (case, object) in valid.iter().zip(&objects) {
        let vector = fixture_vector(&case["vector"]);
        let expected = unhex(case["bytes"].as_str().unwrap());
        assert_eq!(encode(&vector), expected, "{}", case["label"]);
        let row = row_for(&rows, object);
        let (result, _) = publish_once(
            &mut publisher,
            &publication(row, &generation, &vector),
            &project,
        );
        assert_eq!(result.unwrap(), Publication::Embedded, "{}", case["label"]);
        let (state, bytes) = durable(dir.path(), &row.detail.occurrence_id);
        assert_eq!(
            (state.as_deref(), bytes),
            (Some("embedded"), Some(expected)),
            "{}",
            case["label"]
        );
    }

    // No invalid vector produces Embedded, takes the guard, or touches the job.
    let row = row_for(&rows, objects.last().unwrap());
    for case in invalid {
        let vector = fixture_vector(&case["vector"]);
        let label = case["label"].as_str().unwrap();
        let (result, events) = publish_once(
            &mut publisher,
            &publication(row, &generation, &vector),
            &project,
        );
        let error = result.unwrap_err();
        let PublicationError::InvalidVector(message) = &error else {
            panic!("{label}: {error:?}");
        };
        let expected_reason = match case["reason"].as_str().unwrap() {
            "dimension" => "dimensions",
            "nonfinite" => "non-finite",
            "norm" => "not L2-normalized",
            other => panic!("{other}"),
        };
        assert!(message.contains(expected_reason), "{label}: {message}");
        assert!(events.is_empty(), "{label}: {events:?}");
        assert_eq!(
            durable(dir.path(), &row.detail.occurrence_id),
            (Some("pending".to_string()), None),
            "{label}"
        );
        // The projection refuses the shape on its own too, without the daemon's norm check.
        if case["reason"] != "norm" {
            let refused = projection.write(|conn| {
                complete_embedding_observed(
                    conn,
                    &VectorCompletion {
                        occurrence_id: &row.detail.occurrence_id,
                        generation: &generation,
                        payload_id: &row.detail.payload_id,
                        vector: &vector,
                        input_bytes: 1,
                        input_tokens: 1,
                    },
                    3,
                    &mut |_| {},
                )
            });
            assert!(
                matches!(
                    refused,
                    Err(
                        daemon::search_projection::SearchProjectionError::Projection(
                            ProjectionError::InvalidVector { .. }
                        )
                    )
                ),
                "{label}: {refused:?}"
            );
        }
    }
    assert_kernel_writable(&corpus, "after-fixtures");
}

/// Holds a write lock on `path` until dropped.
fn hold_write_lock(path: &Path) -> Connection {
    let blocker = mutate(path);
    blocker.busy_timeout(Duration::ZERO).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    blocker
}

#[test]
fn uncertain_and_duplicate_outcomes_retain_ownership_and_release_the_guard() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let lost = corpus.publish("a", "msg-a", "1", "first message");
    let busy = corpus.publish("b", "msg-b", "1", "second message");
    let conflicting = corpus.publish("c", "msg-c", "1", "third message");
    let generation = generation(8);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = unit(8);
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);

    // A lost local commit reply: the durable vector says the completion landed.
    let row = row_for(&rows, &lost);
    let mut events = Vec::new();
    let result = publisher
        .publish_with_fault_for_test(
            &publication(row, &generation, &vector),
            eligibility(&project),
            deadline(),
            3,
            &mut |event| events.push(event),
            PublicationFault::LoseLocalCommitReply,
        )
        .unwrap();
    assert_eq!(result, Publication::Embedded);
    // The durable rows are consulted only after the guard has dropped.
    let (guarded, reconciled) = events.split_at(7);
    assert_guarded_order(guarded);
    assert_eq!(reconciled, [PublicationEvent::Reconciling]);
    assert_kernel_writable(&corpus, "after-lost");

    // Duplicate delivery of the same result is a replay; a different vector for the same job is a conflict that stops automatic dispatch.
    let (again, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(again.unwrap(), Publication::Replayed);
    let mut other = unit(8);
    other[0] = 0.0;
    other[1] = 1.0;
    let (conflict, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &other),
        &project,
    );
    assert!(
        matches!(conflict, Err(PublicationError::IdempotencyConflict)),
        "{conflict:?}"
    );
    assert_eq!(events.last(), Some(&PublicationEvent::GuardReleased));
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id).1,
        Some(encode(&vector)),
        "the first vector stays"
    );
    assert!(publisher.quarantine().is_none());
    assert_kernel_writable(&corpus, "after-conflict");

    let row = row_for(&rows, &busy);
    let mut events = Vec::new();
    let result = publisher.publish_with_fault_for_test(
        &publication(row, &generation, &vector),
        eligibility(&project),
        deadline(),
        3,
        &mut |event| events.push(event),
        PublicationFault::LoseLocalCommit,
    );
    assert!(
        matches!(result, Err(PublicationError::LocalCommitUnresolved)),
        "a rolled-back commit whose reply is lost is unresolved: {result:?}"
    );
    assert_eq!(
        &events[events.len() - 2..],
        [
            PublicationEvent::GuardReleased,
            PublicationEvent::Reconciling
        ],
        "the guard is released before the durable rows are consulted"
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("pending".to_string()), None)
    );
    assert!(publisher.quarantine().is_none());
    assert_kernel_writable(&corpus, "after-lost-commit");

    let blocker = hold_write_lock(&search_path(dir.path()));
    let mut events = Vec::new();
    let mut held = None;
    let started = Instant::now();
    let result = publisher.publish(
        &publication(row, &generation, &vector),
        eligibility(&project),
        started + Duration::from_millis(300),
        3,
        &mut |event| {
            match event {
                PublicationEvent::GuardAcquired => held = Some(Instant::now()),
                PublicationEvent::GuardReleased => {
                    let held = held.take().expect("the guard was acquired");
                    assert!(
                        held.elapsed() < Duration::from_secs(2),
                        "the kernel writer was held for {:?} behind a held search write lock",
                        held.elapsed()
                    );
                }
                _ => {}
            }
            events.push(event);
        },
    );
    assert!(
        matches!(result, Err(PublicationError::SearchDeadline)),
        "a search write lock held past the deadline writes nothing: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "publish took {:?}",
        started.elapsed()
    );
    assert_eq!(
        events,
        [
            PublicationEvent::VectorValidated,
            PublicationEvent::GuardRequested,
            PublicationEvent::GuardAcquired,
            PublicationEvent::LocalReleased,
            PublicationEvent::GuardReleased,
        ],
        "nothing is staged and nothing is reconciled"
    );
    drop(blocker);
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("pending".to_string()), None)
    );
    assert!(publisher.quarantine().is_none());
    assert_kernel_writable(&corpus, "after-busy");
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(result.unwrap(), Publication::Embedded);

    // A durable vector is coverage even when the job bookkeeping lags behind it; a job with no vector is not.
    let row = row_for(&rows, &conflicting);
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(result.unwrap(), Publication::Embedded);
    mutate(&search_path(dir.path()))
        .execute(
            "UPDATE embedding_jobs SET state='pending' WHERE occurrence_id=?1",
            [&row.detail.occurrence_id],
        )
        .unwrap();
    projection
        .read(|conn| {
            let lagging =
                completion_status(conn, &row.detail.occurrence_id, &generation.generation_id)?;
            assert_eq!(lagging.job_state.as_deref(), Some("pending"));
            assert!(
                lagging.has_durable_vector(&vector),
                "the vector is the coverage"
            );
            assert!(
                !lagging.has_durable_vector(&other),
                "coverage is the vector's own bytes, not any vector"
            );
            Ok(())
        })
        .unwrap();
    // Replaying the completion repairs the lagging job row without a second vector.
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(result.unwrap(), Publication::Replayed);
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id).0.as_deref(),
        Some("embedded")
    );
    // A job row that says embedded with no vector behind it is a ready-only result: not coverage.
    mutate(&search_path(dir.path()))
        .execute(
            "DELETE FROM occurrence_vectors WHERE occurrence_id=?1",
            [&row.detail.occurrence_id],
        )
        .unwrap();
    projection
        .read(|conn| {
            let ready_only =
                completion_status(conn, &row.detail.occurrence_id, &generation.generation_id)?;
            assert_eq!(ready_only.job_state.as_deref(), Some("embedded"));
            assert!(!ready_only.has_durable_vector(&vector));
            Ok(())
        })
        .unwrap();

    // A held kernel writer past the deadline retains the job and grants nothing.
    let row = row_for(&rows, &busy);
    corpus.retire(&busy);
    let (held_tx, held_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let kernel = Arc::clone(&corpus.kernel);
    let holder = std::thread::spawn(move || {
        kernel
            .commit(intent("hold-writer"), |_| {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(String::new())
            })
            .unwrap();
    });
    held_rx.recv().unwrap();
    let mut events = Vec::new();
    let timed_out = publisher.publish(
        &publication(row, &generation, &vector),
        eligibility(&project),
        Instant::now() + Duration::from_millis(100),
        8,
        &mut |event| events.push(event),
    );
    assert!(
        matches!(timed_out, Err(PublicationError::GuardDeadline)),
        "{timed_out:?}"
    );
    assert_eq!(
        events,
        [
            PublicationEvent::VectorValidated,
            PublicationEvent::GuardRequested
        ]
    );
    assert!(publisher.quarantine().is_none());
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert_kernel_writable(&corpus, "after-deadline");

    // A search write lock held past the deadline defers the obsoletion.
    let blocker = hold_write_lock(&search_path(dir.path()));
    let started = Instant::now();
    let deferred = publisher.publish(
        &publication(row, &generation, &vector),
        eligibility(&project),
        started + Duration::from_millis(300),
        9,
        &mut |_| {},
    );
    assert!(
        matches!(deferred, Err(PublicationError::SearchDeadline)),
        "a stale input whose obsoletion cannot be written yet keeps its lease: {deferred:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the obsoletion waited {:?}",
        started.elapsed()
    );
    assert!(
        publisher.quarantine().is_none(),
        "a held lock is contention, not a storage failure"
    );
    let completed = (Some("embedded".to_string()), Some(encode(&vector)));
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        completed,
        "nothing changed while the lock was held"
    );
    drop(blocker);
    let (marked, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        marked.unwrap(),
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Retracted))
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        completed,
        "a completed job keeps its vector when its input is retired"
    );

    // A stale input whose obsoletion fails in the store quarantines the publisher, and the quarantine is sticky.
    mutate(&search_path(dir.path()))
        .execute_batch("ALTER TABLE embedding_jobs RENAME TO embedding_jobs_unavailable")
        .unwrap();
    let (quarantined, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    let Err(PublicationError::Quarantined(quarantine)) = quarantined else {
        panic!("{quarantined:?}");
    };
    assert_eq!(
        quarantine.kind,
        daemon::search_writer::QuarantineKind::Storage
    );
    mutate(&search_path(dir.path()))
        .execute_batch("ALTER TABLE embedding_jobs_unavailable RENAME TO embedding_jobs")
        .unwrap();
    let (again, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    match again {
        Err(PublicationError::Quarantined(q)) => assert_eq!(q, quarantine),
        other => panic!("{other:?}"),
    }
    assert!(events.is_empty(), "a quarantined publisher does no work");
    assert_kernel_writable(&corpus, "after-quarantine");
}

#[test]
fn generation_identity_and_missing_work_are_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("a", "msg-a", "1", "first message");
    let generation = generation(8);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = unit(8);
    let row = row_for(&rows, &object);
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);
    let other_model = VectorGeneration {
        embedding_model: "model-b".to_string(),
        ..generation.clone()
    };
    let other_epoch = VectorGeneration {
        generation_epoch: 2,
        ..generation.clone()
    };
    let other_fingerprint = VectorGeneration {
        tokenizer_fingerprint: "fp-b".to_string(),
        ..generation.clone()
    };
    let unregistered = VectorGeneration {
        generation_id: "gen-unregistered".to_string(),
        ..generation.clone()
    };
    for (label, candidate, expected) in [
        ("model", &other_model, "identity"),
        ("epoch", &other_epoch, "identity"),
        ("fingerprint", &other_fingerprint, "identity"),
        ("unregistered", &unregistered, "unknown"),
    ] {
        let (result, events) = publish_once(
            &mut publisher,
            &publication(row, candidate, &vector),
            &project,
        );
        match (expected, result) {
            ("identity", Err(PublicationError::Refused(ProjectionError::IdentityMismatch))) => {}
            (
                "unknown",
                Err(PublicationError::Refused(ProjectionError::UnknownGeneration { .. })),
            ) => {}
            (_, other) => panic!("{label}: {other:?}"),
        }
        assert_eq!(
            events.last(),
            Some(&PublicationEvent::GuardReleased),
            "{label}"
        );
        assert_eq!(
            durable(dir.path(), &row.detail.occurrence_id),
            (Some("pending".to_string()), None),
            "{label}"
        );
        assert!(publisher.quarantine().is_none(), "{label}");
    }
    // A binding for another project judges the input `WrongScope`; the verdict follows the binding, so the job is left open rather than obsoleted.
    let other_project = ProjectScope::new(&"b".repeat(64)).unwrap();
    let (result, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &other_project,
    );
    assert!(
        matches!(result, Err(PublicationError::WrongScope)),
        "{result:?}"
    );
    assert_eq!(
        events,
        [
            PublicationEvent::VectorValidated,
            PublicationEvent::GuardRequested
        ],
        "the guard is released without being handed out"
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("pending".to_string()), None),
        "a job judged under a wrong binding keeps its work"
    );
    assert!(publisher.quarantine().is_none());

    // A retired generation rejects new vectors.
    let search = search_path(dir.path());
    mutate(&search)
        .execute(
            "UPDATE vector_generations SET state='retired' WHERE generation_id=?1",
            [&generation.generation_id],
        )
        .unwrap();
    let (result, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert!(
        matches!(
            result,
            Err(PublicationError::Refused(
                ProjectionError::RetiredGeneration { .. }
            ))
        ),
        "{result:?}"
    );
    assert_eq!(events.last(), Some(&PublicationEvent::GuardReleased));
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("pending".to_string()), None),
        "a retired generation leaves the job and its vector table untouched"
    );
    mutate(&search)
        .execute(
            "UPDATE vector_generations SET state='building' WHERE generation_id=?1",
            [&generation.generation_id],
        )
        .unwrap();

    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        result.unwrap(),
        Publication::Embedded,
        "the same result completes under the right binding"
    );

    // A job the batch never queued has nothing to complete.
    let other = corpus.publish("b", "msg-b", "1", "second message");
    let fresh = row_for(&corpus.export(), &other).clone();
    let (result, _) = publish_once(
        &mut publisher,
        &publication(&fresh, &generation, &vector),
        &project,
    );
    assert!(
        matches!(
            result,
            Err(PublicationError::Refused(
                ProjectionError::UnknownOccurrence { .. }
            ))
        ),
        "{result:?}"
    );
    assert_kernel_writable(&corpus, "after-refusals");

    // Taking the guard never waits on the search store: a held search write lock does not delay it.
    let blocker = hold_write_lock(&search_path(dir.path()));
    let started = Instant::now();
    let guard = corpus
        .kernel
        .guard_current_input(
            &expectation_for(row),
            eligibility(&project),
            Instant::now() + Duration::from_millis(500),
        )
        .unwrap()
        .expect("the descriptor is current");
    assert!(started.elapsed() < Duration::from_millis(500));
    drop(guard);
    drop(blocker);
}

#[test]
fn the_projection_itself_obsoletes_tombstoned_or_replaced_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("a", "msg-a", "1", "first message");
    let tombstoned = corpus.publish("b", "msg-b", "1", "second message");
    let completed = corpus.publish("c", "msg-c", "1", "third message");
    let generation = generation(8);
    let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = unit(8);

    // A tombstone the projection already holds refuses completion under the guard; the kernel still judges the input current.
    let row = row_for(&rows, &tombstoned);
    mutate(&search_path(dir.path()))
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
             VALUES (?1, 99, 'retired', 4)",
            [&row.detail.occurrence_id],
        )
        .unwrap();
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, &projection);
    let (result, events) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        result.unwrap(),
        Publication::Obsolete(ObsoleteCause::Projected(ObsoleteReason::Tombstoned))
    );
    assert_eq!(events.last(), Some(&PublicationEvent::GuardReleased));
    assert!(!events.contains(&PublicationEvent::VectorStaged));
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("obsolete".to_string()), None)
    );
    assert_kernel_writable(&corpus, "after-tombstone");

    // A tombstone recorded after the job completed reports what the redelivered vector is, not a transition that did not happen: the completed job keeps its state and its vector.
    let row = row_for(&rows, &completed);
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(result.unwrap(), Publication::Embedded);
    mutate(&search_path(dir.path()))
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at)
             VALUES (?1, 99, 'retired', 5)",
            [&row.detail.occurrence_id],
        )
        .unwrap();
    let embedded = (Some("embedded".to_string()), Some(encode(&vector)));
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &vector),
        &project,
    );
    assert_eq!(
        result.unwrap(),
        Publication::Replayed,
        "the same vector for a completed job is a replay, tombstone or not"
    );
    assert_eq!(durable(dir.path(), &row.detail.occurrence_id), embedded);
    let mut other = unit(8);
    other[0] = 0.0;
    other[2] = 1.0;
    let (result, _) = publish_once(
        &mut publisher,
        &publication(row, &generation, &other),
        &project,
    );
    assert!(
        matches!(result, Err(PublicationError::IdempotencyConflict)),
        "a different vector for a completed job is a conflict, tombstone or not: {result:?}"
    );
    assert_eq!(durable(dir.path(), &row.detail.occurrence_id), embedded);
    assert!(publisher.quarantine().is_none());

    let row = row_for(&rows, &object);
    // A payload the vector was not produced from is obsolete, not completed.
    let outcome = projection
        .write(|conn| {
            complete_embedding_observed(
                conn,
                &VectorCompletion {
                    occurrence_id: &row.detail.occurrence_id,
                    generation: &generation,
                    payload_id: &payload_digest("other bytes"),
                    vector: &vector,
                    input_bytes: 1,
                    input_tokens: 1,
                },
                3,
                &mut |_| {},
            )
        })
        .unwrap();
    assert_eq!(
        outcome,
        CompletionOutcome::Obsolete(ObsoleteReason::PayloadChanged)
    );
    assert_eq!(
        durable(dir.path(), &row.detail.occurrence_id),
        (Some("obsolete".to_string()), None)
    );
}

// ---- Named-boundary process crashes ----------------------------------------
// The child is killed with SIGKILL, so these cuts model a process crash with the
// operating system's page cache intact. They say nothing about a lost fsync, a
// torn page, or a host power cut; that evidence belongs to the storage owner.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// The vector row is inserted and the job row is not; the transaction is open.
    VectorStaged,
    /// Both rows are staged; the transaction is open.
    LocalStaged,
    /// The transaction committed and released; the guard is still held.
    LocalReleased,
}

impl Cut {
    fn name(self) -> &'static str {
        match self {
            Cut::VectorStaged => "vector",
            Cut::LocalStaged => "staged",
            Cut::LocalReleased => "released",
        }
    }

    fn parse(name: &str) -> Self {
        match name {
            "vector" => Cut::VectorStaged,
            "staged" => Cut::LocalStaged,
            "released" => Cut::LocalReleased,
            other => panic!("unknown cut {other}"),
        }
    }

    fn barrier(self, event: PublicationEvent) {
        let hit = matches!(
            (self, event),
            (Cut::VectorStaged, PublicationEvent::VectorStaged)
                | (Cut::LocalStaged, PublicationEvent::LocalStaged)
                | (Cut::LocalReleased, PublicationEvent::LocalReleased)
        );
        if hit {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{CHILD_BARRIER} {}", self.name()).unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            loop {
                std::thread::park();
            }
        }
    }
}

fn encode_expectation(expectation: &CurrentInputExpectation) -> String {
    [
        expectation.object_id.as_str(),
        &expectation.source_revision.to_string(),
        &expectation.occurrence_id,
        &expectation.payload_id,
        &expectation.artifact_digest,
    ]
    .join("|")
}

fn decode_expectation(text: &str) -> CurrentInputExpectation {
    let fields: Vec<&str> = text.split('|').collect();
    CurrentInputExpectation {
        object_id: fields[0].to_string(),
        source_revision: fields[1].parse().unwrap(),
        occurrence_id: fields[2].to_string(),
        payload_id: fields[3].to_string(),
        artifact_digest: fields[4].to_string(),
    }
}

/// The child reopens both stores the parent prepared and publishes one vector, parking at the named boundary until the parent kills it.
#[test]
#[ignore = "re-executed by the crash-cut test with its environment set"]
fn crash_child_entrypoint_reexecuted_by_the_parent() {
    let root = PathBuf::from(std::env::var(CHILD_ROOT).unwrap());
    let cut = Cut::parse(&std::env::var(CHILD_CUT).unwrap());
    let expectation = decode_expectation(&std::env::var(CHILD_EXPECTATION).unwrap());
    let kernel = KernelStore::open(root.join("kernel")).unwrap();
    let projection = SearchProjection::open(&root).unwrap();
    let generation = generation(8);
    let vector = unit(8);
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut publisher = EmbeddingPublisher::new(&kernel, &projection);
    let result = publisher.publish(
        &VectorPublication {
            expectation,
            generation: &generation,
            vector: &vector,
            input_bytes: 13,
            input_tokens: 3,
        },
        eligibility(&project),
        deadline(),
        3,
        &mut |event| cut.barrier(event),
    );
    panic!("the child was not killed at its barrier: {result:?}");
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

fn run_crash_child(root: &Path, cut: Cut, expectation: &CurrentInputExpectation) {
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
            .env(CHILD_CUT, cut.name())
            .env(CHILD_EXPECTATION, encode_expectation(expectation))
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
    assert!(
        line.as_deref()
            .is_some_and(|line| line.ends_with(&format!("{CHILD_BARRIER} {}", cut.name()))),
        "{line:?}"
    );
    // Dropping the guard kills and reaps the parked child.
    drop(child);
}

#[test]
fn crash_cuts_reopen_to_both_durable_records_or_neither() {
    for cut in [Cut::VectorStaged, Cut::LocalStaged, Cut::LocalReleased] {
        let dir = tempfile::tempdir().unwrap();
        let (expectation, tip) = {
            let corpus = Corpus::open(dir.path());
            corpus.seed();
            let object = corpus.publish("a", "msg-a", "1", "first message");
            let generation = generation(8);
            let (projection, rows) = corpus.bootstrap(dir.path(), &generation);
            drop(projection);
            (expectation_for(row_for(&rows, &object)), corpus.tip())
        };
        run_crash_child(dir.path(), cut, &expectation);
        let expected = match cut {
            Cut::VectorStaged | Cut::LocalStaged => (Some("pending".to_string()), None),
            Cut::LocalReleased => (Some("embedded".to_string()), Some(encode(&unit(8)))),
        };
        for reopen in 0..2 {
            let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
            let projection = SearchProjection::open(dir.path()).unwrap();
            assert_eq!(
                durable(dir.path(), &expectation.occurrence_id),
                expected,
                "{cut:?} reopen {reopen}"
            );
            assert_eq!(
                kernel.tip().unwrap(),
                tip,
                "publication commits nothing to the kernel"
            );
            drop(projection);
            drop(kernel);
        }
        // Whatever the cut left behind, a later publication reaches the same durable pair.
        let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
        let projection = SearchProjection::open(dir.path()).unwrap();
        let generation = generation(8);
        let project = ProjectScope::new(PROJECT).unwrap();
        let mut publisher = EmbeddingPublisher::new(&kernel, &projection);
        let vector = unit(8);
        let (result, events) = publish_once(
            &mut publisher,
            &VectorPublication {
                expectation: expectation.clone(),
                generation: &generation,
                vector: &vector,
                input_bytes: 13,
                input_tokens: 3,
            },
            &project,
        );
        match cut {
            Cut::VectorStaged | Cut::LocalStaged => {
                assert_eq!(result.unwrap(), Publication::Embedded);
                assert_guarded_order(&events);
            }
            Cut::LocalReleased => assert_eq!(result.unwrap(), Publication::Replayed),
        }
        assert_eq!(
            durable(dir.path(), &expectation.occurrence_id),
            (Some("embedded".to_string()), Some(encode(&vector)))
        );
    }
}
