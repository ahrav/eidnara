//! Real-store proofs for the MemoryReviewer evidence broker: scope and staleness refusal, live-origin revocation, canonical and promoted deduplication, native identity sharing across spans, uncited-context lineage, unsupported bound questions (Q24), the render check over small and windowed artifacts, the verdict-before-hold and verdict-after-load order, hold growth, and the accounting bounds; and for the coordinator's canonical resolution over the same store: descriptors resolve through their originating decision, proposals target that decision, and a moved, missing, or wrong-kind owner refuses before disclosure.

use daemon::memory_reviewer::broker::{
    EvidenceBroker, JudgedAt, MAX_ISSUED_INSPECTIONS, MAX_MODEL_VISIBLE_BYTES,
    MAX_OPERATIONS_PER_BATCH, OriginClass, QuestionTemplate, ReferenceExpectation, RefusalCode,
    RunBinding, decision_derived,
};
use daemon::memory_reviewer::coordinator::{
    InvestigationError, proposal_target, resolve_descriptor,
};
use daemon::memory_reviewer::selection::{MEMORY_CLASSES, PRODUCTION_SELECTION_OPEN};
use kernel::source_identity::{Occurrence, OccurrenceClass, Span};
use kernel::{
    ArtifactIngestRequest, CommitIntent, DecisionPayload, DecisionSpec, Dimension, DomainSpec,
    ExtractedFact, KernelStore, MAX_MEMORY_REVIEWER_HOLD_REFERENCES,
    MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS, MemoryReviewerHoldBinding, ProviderEgress,
    ReviewBinding, ReviewOwner, ReviewPayload, ReviewStagingSpec, ReviewSubject, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceDependency, SourceDescriptorPolicy, SourceDescriptorRequest,
    SourceSpan, StagingTerminalState,
};
use kernel::{EligibilityVerdict, SurfaceVisibility};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_PROJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCOPE: &str = "project:a";
const HOUR_MS: i64 = 60 * 60 * 1_000;
const AWS_KEY: &str = "AKIAQ7RSTUVWXYZ23456";

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
        producer: "memory_reviewer-broker-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn incarnation(root: &std::path::Path) -> String {
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

struct Fixture {
    directory: tempfile::TempDir,
    store: KernelStore,
    now: i64,
}

/// One descriptor publication over `buffer`, or `span` of it, at revision 1.
struct Publish<'a> {
    key: &'a str,
    class: &'a str,
    representation: &'a str,
    identity: &'a [(&'a str, &'a str)],
    evidence: &'a (String, String),
    buffer: &'a str,
    span: Option<Span>,
}

impl Fixture {
    fn open() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
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
        Self {
            directory,
            store,
            now: now_ms(),
        }
    }

    fn hold_binding(&self, project: &str) -> MemoryReviewerHoldBinding {
        MemoryReviewerHoldBinding {
            project_digest: project.to_string(),
            kernel_incarnation: incarnation(self.directory.path()),
            memstore_incarnation: "m".repeat(32),
            subject: "job-1".to_string(),
            generation: 1,
        }
    }

    fn ingest(&self, key: &str, payload: &[u8], capture: bool) -> (String, String) {
        let handle = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(key),
                payload: payload.to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: if capture {
                    "local_file"
                } else {
                    "conversation"
                }
                .to_string(),
                source_id: format!("src/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: if capture {
                    MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string()
                } else {
                    "canonical".to_string()
                },
                retain_until: capture.then_some(self.now + HOUR_MS),
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    /// A broker whose execution hold already covers `evidence` for this project, disclosing to a local destination unless the test says otherwise.
    fn broker(&self, project: &str, evidence: &[String]) -> EvidenceBroker {
        self.broker_for(project, evidence, kernel::ArtifactDestination::Local)
    }

    fn broker_for(
        &self,
        project: &str,
        evidence: &[String],
        destination: kernel::ArtifactDestination,
    ) -> EvidenceBroker {
        let binding = self.hold_binding(project);
        let hold = self
            .store
            .acquire_execution_hold(&binding, evidence, self.now + 2 * HOUR_MS)
            .unwrap();
        EvidenceBroker::new(
            RunBinding {
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .unwrap()
    }

    fn review_binding(&self) -> ReviewBinding {
        ReviewBinding {
            project_digest: PROJECT.to_string(),
            domain_id: DOMAIN.to_string(),
            owner: ReviewOwner::Job {
                job_id: "job-1".to_string(),
            },
            subject_source: SourceDependency {
                source_kind: "conversation".to_string(),
                source_id: "session-1".to_string(),
                source_revision: 1,
            },
            reference_sources: vec![],
        }
    }

    fn staged_subject(&self, text: &str) -> ReferenceExpectation {
        let reference = self
            .store
            .stage_review_input(ReviewStagingSpec {
                extraction_run_id: "run-1".to_string(),
                candidate_id: "subject-1".to_string(),
                producer: "history-summarizer".to_string(),
                binding: self.review_binding(),
                payload: ReviewPayload::Subject(ReviewSubject {
                    facts: vec![ExtractedFact {
                        text: text.to_string(),
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
            .finish_staging_run("run-1", StagingTerminalState::Completed, self.now + 1)
            .unwrap();
        ReferenceExpectation::StagedSubject {
            reference,
            binding: self.review_binding(),
        }
    }

    /// The reference of the subject `staged_subject` sealed, for a second broker.
    fn staged_reference(&self) -> ReferenceExpectation {
        let payload = ReviewPayload::Subject(ReviewSubject {
            facts: vec![ExtractedFact {
                text: "bun builds the workspace".to_string(),
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
        });
        ReferenceExpectation::StagedSubject {
            reference: kernel::ReviewStagedReference {
                database_incarnation_id: incarnation(self.directory.path()),
                candidate_id: "subject-1".to_string(),
                payload_digest: payload.digest().unwrap(),
            },
            binding: self.review_binding(),
        }
    }

    fn decision(&self, object: &str) {
        self.decision_in(object, SCOPE);
    }

    /// A live, admitted decision at `object` inside `scope`.
    fn decision_in(&self, object: &str, scope: &str) {
        self.store
            .commit(intent(&format!("decision-{object}")), |envelope| {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("decision-{object}"),
                    object_id: object.to_string(),
                    domain_id: DOMAIN.to_string(),
                    proposition_id: None,
                    scope_id: Some(scope.to_string()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "architecture".to_string(),
                    payload: DecisionPayload {
                        summary: format!("summary {object}"),
                        rationale: format!("rationale {object}"),
                    },
                    source_kind: "repo".to_string(),
                    source_id: format!("src/{object}"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.record_admission(kernel::AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(object.to_string()),
                    source_class: Some(kernel::SourceClass::ExplicitUser),
                    taint_class: Some(kernel::TaintClass::UserExplicit),
                    event: kernel::AdmissionEvent {
                        kind: kernel::EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "test".to_string(),
                    },
                })?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Supersedes the live decision at `old` with a successor at source revision 2, invalidating `old`.
    fn revise_decision(&self, old: &str, successor: &str) {
        self.store
            .commit(intent(&format!("revise-{old}")), |envelope| {
                envelope.correct_decision(
                    old,
                    DecisionSpec {
                        decision_id: format!("decision-{successor}"),
                        object_id: successor.to_string(),
                        domain_id: DOMAIN.to_string(),
                        proposition_id: None,
                        scope_id: Some(SCOPE.to_string()),
                        anchor_id: None,
                        evidence_id: None,
                        decision_kind: "architecture".to_string(),
                        payload: DecisionPayload {
                            summary: format!("summary {successor}"),
                            rationale: format!("rationale {successor}"),
                        },
                        source_kind: "repo".to_string(),
                        source_id: format!("src/{old}"),
                        source_revision: 2,
                        sensitivity: Sensitivity::Normal,
                    },
                )?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// The coordinator's resolution of a published descriptor, asserting the hold protects exactly its evidence.
    fn resolve(&self, object_id: &str) -> Result<ReferenceExpectation, InvestigationError> {
        resolve_descriptor(&self.store, object_id, None).map(|(expectation, protected)| {
            assert_eq!(
                protected,
                vec![expectation.evidence_id().unwrap().to_string()]
            );
            expectation
        })
    }

    /// The registry tip and every live decision and descriptor object's state, for before/after equality.
    fn canonical_state(&self, objects: &[&str]) -> (i64, Vec<Option<kernel::ObjectState>>) {
        let ids: Vec<String> = objects.iter().map(|id| id.to_string()).collect();
        self.store.object_states(&ids).unwrap()
    }

    /// Publishes one descriptor and returns its object id and the descriptor's identity tuple bytes.
    fn descriptor(&self, publish: Publish<'_>) -> (String, Vec<u8>) {
        let mut published = None;
        self.store
            .commit(intent(&format!("descriptor-{}", publish.key)), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: publish.class,
                            identity: publish.identity,
                            revision: "1",
                            representation: publish.representation,
                            span: publish.span,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: DOMAIN,
                        scope_id: Some(SCOPE),
                        evidence_id: &publish.evidence.0,
                        artifact_digest: &publish.evidence.1,
                        buffer: publish.buffer,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap_or_else(|error| panic!("descriptor publication: {error:?}"));
                published = Some(outcome.object_id);
                Ok(String::new())
            })
            .unwrap();
        let object_id = published.unwrap();
        let tip = self.store.tip().unwrap();
        let row = self
            .store
            .observation_for_object_as_of(&object_id, tip)
            .unwrap()
            .unwrap();
        let detail: kernel::SourceDescriptorDetail =
            serde_json::from_str(row.payload.detail.as_deref().unwrap()).unwrap();
        (object_id, detail.occurrence_tuple)
    }
}

#[test]
fn staged_subjects_and_captures_read_through_kernel_expectations_and_grow_the_hold() {
    let fixture = Fixture::open();
    let subject = fixture.staged_subject("bun builds the workspace");
    let (capture_id, capture_digest) = fixture.ingest("capture", b"captured file text", true);
    let (other_id, _) = fixture.ingest("other", b"another artifact", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&other_id));
    let subject_alias = broker.aliases.issue(subject);
    let capture_alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id.clone(),
            artifact_digest: capture_digest.clone(),
            byte_length: 18,
            retain_until: fixture.now + HOUR_MS,
        });

    let read = broker
        .read(
            &fixture.store,
            subject_alias.as_str(),
            None,
            fixture.now + 2,
        )
        .unwrap();
    assert_eq!(read.buffer.bytes(), b"bun builds the workspace");
    assert_eq!(read.buffer.tag().origin, OriginClass::StagedSubject);
    assert_eq!(read.buffer.tag().charged_bytes, 24);
    assert_eq!(
        read.sensitivity,
        Sensitivity::Sensitive,
        "a private read grants no upgrade"
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 24);
    assert_eq!(broker.accounting.issued_inspections(), 1);
    // A range applies to a subject too, and an out-of-bounds range is refused before it is charged.
    let ranged = broker
        .read(
            &fixture.store,
            subject_alias.as_str(),
            Some(0..3),
            fixture.now + 2,
        )
        .unwrap();
    assert_eq!(ranged.buffer.bytes(), b"bun");
    assert_eq!(
        broker
            .read(
                &fixture.store,
                subject_alias.as_str(),
                Some(0..99),
                fixture.now + 2
            )
            .unwrap_err()
            .code,
        RefusalCode::InvalidRange
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 27);
    // A staged subject is Sensitive, so a remote destination never receives it.
    let mut remote = fixture.broker_for(
        PROJECT,
        std::slice::from_ref(&other_id),
        kernel::ArtifactDestination::Remote,
    );
    let remote_alias = remote.aliases.issue(fixture.staged_reference());
    assert_eq!(
        remote
            .read(&fixture.store, remote_alias.as_str(), None, fixture.now + 2)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked
    );
    assert_eq!(remote.accounting.model_visible_bytes(), 0);

    // The capture is not yet held; reading it grows the execution hold before any byte is loaded.
    let before = fixture.store.validate_held_evidence(
        &broker_hold_id(&broker),
        kernel::MemoryReviewerHoldKind::Execution,
        &fixture.hold_binding(PROJECT),
        std::slice::from_ref(&capture_id),
        fixture.now,
    );
    assert!(
        before.is_err(),
        "the capture is outside the hold until it is read"
    );
    let read = broker
        .read(
            &fixture.store,
            capture_alias.as_str(),
            Some(9..13),
            fixture.now + 2,
        )
        .unwrap();
    assert_eq!(read.buffer.bytes(), b"file");
    assert_eq!(read.buffer.tag().origin, OriginClass::TemporaryCapture);
    assert_eq!(broker.buffers.loaded(), 1);
    fixture
        .store
        .validate_held_evidence(
            &broker_hold_id(&broker),
            kernel::MemoryReviewerHoldKind::Execution,
            &fixture.hold_binding(PROJECT),
            std::slice::from_ref(&capture_id),
            fixture.now,
        )
        .unwrap();
    // A second range reuses the loaded buffer and is charged again.
    broker
        .read(
            &fixture.store,
            capture_alias.as_str(),
            Some(0..8),
            fixture.now + 3,
        )
        .unwrap();
    assert_eq!(broker.buffers.loaded(), 1);
    assert_eq!(broker.accounting.model_visible_bytes(), 27 + 4 + 8);
    assert_eq!(
        broker
            .read(
                &fixture.store,
                capture_alias.as_str(),
                Some(5..5),
                fixture.now + 3
            )
            .unwrap_err()
            .code,
        RefusalCode::InvalidRange
    );
    assert_eq!(
        broker
            .read(
                &fixture.store,
                capture_alias.as_str(),
                Some(0..99),
                fixture.now + 3
            )
            .unwrap_err()
            .code,
        RefusalCode::InvalidRange
    );
    // The union carries every disclosed input and the question lineage, cited or not.
    broker.ledger.record_citation(&subject_alias).unwrap();
    assert_eq!(
        broker.ledger.uncited_disclosed().collect::<Vec<_>>(),
        vec![&capture_alias]
    );
    let union = broker.ledger.union().encode().unwrap();
    assert_eq!(union.members, 3, "question template, subject, capture");
    assert!(union.canonical.contains("\"kind\":\"temporary_capture\""));
    assert!(union.canonical.contains("\"kind\":\"question_template\""));
    let fabricated = broker.aliases.resolve("ref-99").unwrap_err();
    assert_eq!(
        (fabricated.alias, fabricated.code),
        (None, RefusalCode::UnknownAlias)
    );
    assert_eq!(
        broker
            .read(&fixture.store, "ref-99", None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::UnknownAlias
    );
    let uncited = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id,
            artifact_digest: capture_digest,
            byte_length: 18,
            retain_until: fixture.now + HOUR_MS,
        });
    assert_eq!(
        broker.ledger.record_citation(&uncited).unwrap_err().code,
        RefusalCode::UnknownAlias,
        "a citation must name a disclosed alias"
    );
}

fn broker_hold_id(broker: &EvidenceBroker) -> String {
    broker.hold_id().to_string()
}

#[test]
fn scope_staleness_and_expiry_refuse_before_any_disclosure() {
    let fixture = Fixture::open();
    let subject = fixture.staged_subject("scoped subject");
    let (capture_id, capture_digest) = fixture.ingest("capture", b"captured file text", true);
    let mut broker = fixture.broker(OTHER_PROJECT, std::slice::from_ref(&capture_id));
    // The subject belongs to another project.
    let alias = broker.aliases.issue(subject.clone());
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now + 2)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 0);
    assert!(broker.ledger.disclosed().next().is_none());
    // A changed digest is a changed expectation.
    let ReferenceExpectation::StagedSubject {
        mut reference,
        binding,
    } = subject
    else {
        unreachable!()
    };
    reference.payload_digest = "0".repeat(64);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&capture_id));
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::StagedSubject { reference, binding });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now + 2)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    // An expired acquisition reference or a mismatched capture identity refuses too.
    let expired = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id.clone(),
            artifact_digest: capture_digest.clone(),
            byte_length: 18,
            retain_until: fixture.now - 1,
        });
    assert_eq!(
        broker
            .read(&fixture.store, expired.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    let wrong_length = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id,
            artifact_digest: capture_digest,
            byte_length: 19,
            retain_until: fixture.now + HOUR_MS,
        });
    assert_eq!(
        broker
            .read(&fixture.store, wrong_length.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    assert_eq!(
        broker.buffers.loaded(),
        0,
        "nothing was loaded for a refused read"
    );
    assert_eq!(
        broker.accounting.issued_inspections(),
        3,
        "refused-after-admission reads still count"
    );
}

#[test]
fn render_check_refuses_secrets_and_placeholders_without_redacting() {
    let fixture = Fixture::open();
    // Ingestion redacts a detected secret into a placeholder token, so the stored bytes carry the token the render check refuses.
    let (secret_id, secret_digest) =
        fixture.ingest("secret", format!("token {AWS_KEY}").as_bytes(), true);
    let placeholder = format!("x {} y", kernel::OPERATOR_REDACTION_PLACEHOLDER);
    let (placeholder_id, placeholder_digest) =
        fixture.ingest("placeholder", placeholder.as_bytes(), true);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&placeholder_id));
    let stored_len = |evidence_id: &str, digest: &str| {
        fixture
            .store
            .read_artifact(&kernel::ArtifactHandle {
                digest: digest.to_string(),
                evidence_id: evidence_id.to_string(),
            })
            .unwrap()
            .len() as u64
    };
    let secret_len = stored_len(&secret_id, &secret_digest);
    let placeholder_len = stored_len(&placeholder_id, &placeholder_digest);
    // A detected secret makes the artifact Secret at ingest, so the Kernel's egress verdict refuses it for every destination before any render; the operator placeholder passes classification and is refused by the render check itself.
    for (evidence_id, digest, len, code) in [
        (
            secret_id.clone(),
            secret_digest,
            secret_len,
            RefusalCode::PolicyBlocked,
        ),
        (
            placeholder_id,
            placeholder_digest,
            placeholder_len,
            RefusalCode::RenderCheck,
        ),
    ] {
        let alias = broker
            .aliases
            .issue(ReferenceExpectation::TemporaryCapture {
                evidence_id,
                artifact_digest: digest,
                byte_length: len,
                retain_until: fixture.now + HOUR_MS,
            });
        let refusal = broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err();
        assert_eq!(refusal.code, code);
        assert_eq!(refusal.alias.as_ref(), Some(&alias));
        assert_eq!(refusal.to_string(), format!("{code} ({})", alias.as_str()));
    }
    assert_eq!(
        broker.accounting.model_visible_bytes(),
        placeholder_len,
        "bytes are charged before they are loaded, so a render-check refusal keeps its charge while a policy refusal never reaches the charge"
    );
    assert!(
        fixture
            .store
            .validate_held_evidence(
                &broker_hold_id(&broker),
                kernel::MemoryReviewerHoldKind::Execution,
                &fixture.hold_binding(PROJECT),
                std::slice::from_ref(&secret_id),
                fixture.now,
            )
            .is_err(),
        "a policy-blocked artifact is refused before the hold grows over it"
    );
    // A range that stops inside the placeholder cannot slip past the check: the whole buffer is checked when it is first loaded.
    let (split_id, split_digest) = fixture.ingest("split", placeholder.as_bytes(), true);
    let mut split_broker = fixture.broker(PROJECT, std::slice::from_ref(&split_id));
    let split = split_broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: split_id,
            artifact_digest: split_digest,
            byte_length: placeholder_len,
            retain_until: fixture.now + HOUR_MS,
        });
    assert_eq!(
        split_broker
            .read(&fixture.store, split.as_str(), Some(0..4), fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::RenderCheck
    );
    assert_eq!(
        split_broker.buffers.loaded(),
        1,
        "the buffer is retained but nothing renders from it"
    );
    assert!(split_broker.ledger.disclosed().next().is_none());
    // A refused artifact stays refused: the repeat is not charged and the whole buffer is not rescanned.
    assert_eq!(
        split_broker
            .read(&fixture.store, split.as_str(), Some(0..4), fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::RenderCheck
    );
    assert_eq!(split_broker.accounting.model_visible_bytes(), 4);
    assert!(broker.ledger.disclosed().next().is_none());
    // An artifact above the scanner's single-pass input limit is checked in windows, as the Kernel checked it at ingest, so a clean large artifact renders.
    let large_len = context_core::redaction::MAX_REDACTABLE_BYTES + 4096;
    let (large_id, large_digest) = fixture.ingest("large", &vec![b'x'; large_len], true);
    let mut large_broker = fixture.broker(PROJECT, std::slice::from_ref(&large_id));
    let large = large_broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: large_id,
            artifact_digest: large_digest,
            byte_length: large_len as u64,
            retain_until: fixture.now + HOUR_MS,
        });
    let rendered = large_broker
        .read(&fixture.store, large.as_str(), Some(0..16), fixture.now)
        .unwrap();
    assert_eq!(rendered.buffer.bytes(), [b'x'; 16]);
    assert!(
        broker
            .render_host_text(&format!("question {AWS_KEY}"))
            .is_err()
    );
    let host = broker
        .render_host_text(QuestionTemplate::ExtractedFacts.text())
        .unwrap();
    assert_eq!(host.tag().origin, OriginClass::HostAuthored);
    assert_eq!(host.tag().charged_bytes, 0);
    assert_eq!(
        QuestionTemplate::parse("extracted_facts"),
        Ok(QuestionTemplate::ExtractedFacts)
    );
    let unsupported = QuestionTemplate::parse("Is the deploy key rotated? extracted_facts")
        .expect_err("source-derived question text is not a supported input");
    assert_eq!(
        (unsupported.alias, unsupported.code),
        (None, RefusalCode::UnsupportedQuestion)
    );
}

#[test]
fn canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let claim_text = "the claim as canonical text";
    let claim_evidence = fixture.ingest("claim", claim_text.as_bytes(), false);
    let promoted_text = "the claim as a promoted memory";
    let promoted_evidence = fixture.ingest("promoted", promoted_text.as_bytes(), false);
    let (claim_object, _) = fixture.descriptor(Publish {
        key: "claim",
        class: "canonical_claims",
        representation: "decision_summary",
        identity: &[("object_id", "decision-a")],
        evidence: &claim_evidence,
        buffer: claim_text,
        span: None,
    });
    let (promoted_object, _) = fixture.descriptor(Publish {
        key: "promoted",
        class: "promoted_memory",
        representation: "summary",
        identity: &[("decision_object_id", "decision-a")],
        evidence: &promoted_evidence,
        buffer: promoted_text,
        span: None,
    });
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let native_identity = [
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        ("message_id", "msg-001"),
        ("block_index", "0"),
    ];
    let (native_object, native_tuple) = fixture.descriptor(Publish {
        key: "native",
        class: "messages",
        representation: "text",
        identity: &native_identity,
        evidence: &native_evidence,
        buffer: native_text,
        span: None,
    });
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&claim_evidence.0));
    let canonical =
        |object: &str, evidence: &(String, String), class| ReferenceExpectation::CanonicalSource {
            object_id: object.to_string(),
            class,
            source_revision: 1,
            artifact_digest: evidence.1.clone(),
            evidence_id: evidence.0.clone(),
            originating_decision_id: "decision-a".to_string(),
            decision_source_revision: 1,
        };
    let claim_alias = broker.aliases.issue(canonical(
        &claim_object,
        &claim_evidence,
        OccurrenceClass::CanonicalClaims,
    ));
    let promoted_alias = broker.aliases.issue(canonical(
        &promoted_object,
        &promoted_evidence,
        OccurrenceClass::PromotedMemory,
    ));
    let native_alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: native_object.clone(),
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1.clone(),
        evidence_id: native_evidence.0.clone(),
        occurrence_tuple: native_tuple.clone(),
    });
    let first = broker
        .read(&fixture.store, claim_alias.as_str(), None, fixture.now)
        .unwrap();
    assert_eq!(first.buffer.bytes(), claim_text.as_bytes());
    assert_eq!(first.origin_key, "decision:decision-a");
    assert_eq!(
        first.buffer.tag().verdict,
        Some(JudgedAt {
            verdict: EligibilityVerdict::Hidden,
            visibility: SurfaceVisibility::Hidden,
            tip: fixture.store.tip().unwrap(),
        }),
        "the tag records the descriptor's own judgement; the decision's standing gated the read"
    );
    assert_eq!(broker.shared_origin(claim_alias.as_str()).unwrap(), None);
    let second = broker
        .read(&fixture.store, promoted_alias.as_str(), None, fixture.now)
        .unwrap();
    assert_eq!(
        second.origin_key, first.origin_key,
        "two forms of one decision are one origin"
    );
    assert_eq!(
        broker.shared_origin(promoted_alias.as_str()).unwrap(),
        Some(&claim_alias)
    );
    let native = broker
        .read(&fixture.store, native_alias.as_str(), None, fixture.now)
        .unwrap();
    assert_ne!(native.origin_key, first.origin_key);
    assert!(native.origin_key.starts_with("native:messages:"));
    assert!(
        !native.origin_key.contains(&native_evidence.1),
        "an origin key is identity, not a hash"
    );
    assert_eq!(broker.shared_origin(native_alias.as_str()).unwrap(), None);
    // Spans of one message share one origin despite distinct descriptors and occurrence tuples.
    let (span_object, span_tuple) = fixture.descriptor(Publish {
        key: "native-span",
        class: "messages",
        representation: "text",
        identity: &native_identity,
        evidence: &native_evidence,
        buffer: native_text,
        span: Some(Span { start: 0, end: 8 }),
    });
    assert_ne!(
        span_tuple, native_tuple,
        "a span changes the occurrence tuple"
    );
    let span_alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: span_object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1.clone(),
        evidence_id: native_evidence.0.clone(),
        occurrence_tuple: span_tuple,
    });
    let span_read = broker
        .read(&fixture.store, span_alias.as_str(), Some(0..8), fixture.now)
        .unwrap();
    assert_eq!(span_read.buffer.bytes(), b"a native");
    assert_eq!(
        span_read.origin_key, native.origin_key,
        "two spans of one message are one origin"
    );
    assert_eq!(
        broker.shared_origin(span_alias.as_str()).unwrap(),
        Some(&native_alias),
        "the shared-origin lookup must use the key the disclosure recorded"
    );
    assert_eq!(broker.shared_origin(native_alias.as_str()).unwrap(), None);
    // A fabricated identity tuple is refused before bytes are read.
    let mut forged = native_tuple.clone();
    forged.push(0);
    let forged_alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: native_object.clone(),
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1.clone(),
        evidence_id: native_evidence.0.clone(),
        occurrence_tuple: forged,
    });
    assert_eq!(
        broker
            .read(&fixture.store, forged_alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    // A remote model never receives unproven evidence: the same references are policy-blocked there.
    let mut remote = fixture.broker_for(
        PROJECT,
        std::slice::from_ref(&claim_evidence.0),
        kernel::ArtifactDestination::Remote,
    );
    let remote_alias = remote.aliases.issue(canonical(
        &claim_object,
        &claim_evidence,
        OccurrenceClass::CanonicalClaims,
    ));
    assert_eq!(
        remote
            .read(&fixture.store, remote_alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked
    );
    assert_eq!(remote.accounting.model_visible_bytes(), 0);
    let members: Vec<_> = broker
        .ledger
        .union()
        .members()
        .filter(|member| member.kind == "canonical_source")
        .collect();
    assert_eq!(members.len(), 2);
    assert!(
        members
            .iter()
            .all(|member| member.owner_id.as_deref() == Some("decision-a"))
    );

    // Retiring the originating decision revokes both forms on the next render, and a stale revision is refused too.
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision("decision-a")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        broker
            .read(&fixture.store, claim_alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::OriginRevoked
    );
    assert_eq!(
        broker
            .read(&fixture.store, promoted_alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::OriginRevoked
    );
    let stale = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: native_object,
        class: OccurrenceClass::Messages,
        source_revision: 7,
        artifact_digest: native_evidence.1,
        evidence_id: native_evidence.0,
        occurrence_tuple: native_tuple,
    });
    assert_eq!(
        broker
            .read(&fixture.store, stale.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
}

#[test]
fn accounting_bounds_operations_bytes_and_uncertain_disclosure() {
    let fixture = Fixture::open();
    let (capture_id, capture_digest) = fixture.ingest("capture", &[b'x'; 4096], true);
    let mut broker = fixture
        .broker(PROJECT, std::slice::from_ref(&capture_id))
        .with_inspection_limit(10);
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id.clone(),
            artifact_digest: capture_digest.clone(),
            byte_length: 4096,
            retain_until: fixture.now + HOUR_MS,
        });
    for _ in 0..MAX_OPERATIONS_PER_BATCH {
        broker
            .read(&fixture.store, alias.as_str(), Some(0..1), fixture.now)
            .unwrap();
    }
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), Some(0..1), fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::BatchLimit
    );
    assert!(
        broker.ledger.conclusions_usable(),
        "a batch bound paces the run; it does not truncate the evidence set"
    );
    broker.accounting.end_batch();
    broker
        .read(&fixture.store, alias.as_str(), Some(0..1), fixture.now)
        .unwrap();
    broker
        .read(&fixture.store, alias.as_str(), Some(0..1), fixture.now)
        .unwrap();
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), Some(0..1), fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::InspectionLimit,
        "a lowered ceiling proves the 32-inspection assertion"
    );
    assert!(
        !broker.ledger.conclusions_usable(),
        "the run-level inspection bound truncates the evidence set"
    );
    // The test knob only lowers the ceiling: asking for more than the production bound still refuses at the bound.
    let mut raised = fixture
        .broker(PROJECT, std::slice::from_ref(&capture_id))
        .with_inspection_limit(1_000);
    let raised_alias = raised
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id.clone(),
            artifact_digest: capture_digest.clone(),
            byte_length: 4096,
            retain_until: fixture.now + HOUR_MS,
        });
    for inspection in 0..MAX_ISSUED_INSPECTIONS {
        if inspection % MAX_OPERATIONS_PER_BATCH == 0 {
            raised.accounting.end_batch();
        }
        raised
            .read(
                &fixture.store,
                raised_alias.as_str(),
                Some(0..1),
                fixture.now,
            )
            .unwrap();
    }
    raised.accounting.end_batch();
    assert_eq!(
        raised
            .read(
                &fixture.store,
                raised_alias.as_str(),
                Some(0..1),
                fixture.now
            )
            .unwrap_err()
            .code,
        RefusalCode::InspectionLimit,
        "the ceiling cannot be raised above MAX_ISSUED_INSPECTIONS"
    );
    // Rendered bytes are charged on every render; the bound refuses whole rather than truncating.
    let (big_id, big_digest) = fixture.ingest("big", &[b'y'; 32 * 1024], true);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&big_id));
    let big = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: big_id.clone(),
            artifact_digest: big_digest.clone(),
            byte_length: 32 * 1024,
            retain_until: fixture.now + HOUR_MS,
        });
    let renders = MAX_MODEL_VISIBLE_BYTES / (32 * 1024);
    for _ in 0..renders {
        broker.accounting.end_batch();
        broker
            .read(&fixture.store, big.as_str(), None, fixture.now)
            .unwrap();
    }
    assert_eq!(
        broker.accounting.model_visible_bytes(),
        MAX_MODEL_VISIBLE_BYTES
    );
    broker.accounting.end_batch();
    assert_eq!(
        broker
            .read(&fixture.store, big.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ByteLimit
    );
    assert_eq!(
        broker.buffers.loaded(),
        1,
        "one artifact loads once however often it renders"
    );
    assert!(
        !broker.ledger.conclusions_usable(),
        "a capacity refusal after a disclosure leaves the evidence set partial"
    );
    // A fresh run: an artifact beyond the run buffer ceiling is refused before loading, and that refusal too is partial once anything was disclosed.
    let (small_id, small_digest) = fixture.ingest("small", b"tiny", true);
    let mut broker = fixture
        .broker(PROJECT, &[small_id.clone(), big_id.clone()])
        .with_buffer_limit(64);
    let small = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: small_id,
            artifact_digest: small_digest,
            byte_length: 4,
            retain_until: fixture.now + HOUR_MS,
        });
    let oversized = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: big_id,
            artifact_digest: big_digest,
            byte_length: 32 * 1024,
            retain_until: fixture.now + HOUR_MS,
        });
    broker
        .read(&fixture.store, small.as_str(), None, fixture.now)
        .unwrap();
    assert!(broker.ledger.conclusions_usable());
    assert_eq!(
        broker
            .read(&fixture.store, oversized.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::BufferLimit
    );
    assert_eq!(
        broker.buffers.loaded(),
        1,
        "the oversized artifact was never loaded"
    );
    assert!(!broker.ledger.conclusions_usable());
    let mut uncertain = broker;
    uncertain.ledger.record_uncertain_disclosure();
    assert!(!uncertain.ledger.conclusions_usable());
}

#[test]
fn a_hold_capacity_refusal_after_a_disclosure_marks_the_evidence_set_partial() {
    let fixture = Fixture::open();
    let held: Vec<(String, String)> = (0..MAX_MEMORY_REVIEWER_HOLD_REFERENCES)
        .map(|index| {
            fixture.ingest(
                &format!("held-{index}"),
                format!("held {index}").as_bytes(),
                true,
            )
        })
        .collect();
    let (extra_id, extra_digest) = fixture.ingest("extra", b"one more", true);
    let held_ids: Vec<String> = held.iter().map(|(id, _)| id.clone()).collect();
    let mut broker = fixture.broker(PROJECT, &held_ids);
    let first = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: held[0].0.clone(),
            artifact_digest: held[0].1.clone(),
            byte_length: 6,
            retain_until: fixture.now + HOUR_MS,
        });
    broker
        .read(&fixture.store, first.as_str(), None, fixture.now)
        .unwrap();
    assert!(broker.ledger.conclusions_usable());
    // The hold is full: one more reference is a capacity refusal, and the model asked for evidence it will not see.
    let extra = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: extra_id,
            artifact_digest: extra_digest,
            byte_length: 8,
            retain_until: fixture.now + HOUR_MS,
        });
    assert_eq!(
        broker
            .read(&fixture.store, extra.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldLimit
    );
    assert_eq!(
        broker.buffers.loaded(),
        1,
        "the refused artifact never loaded"
    );
    assert!(
        !broker.ledger.conclusions_usable(),
        "a hold capacity refusal truncates the evidence set like every other capacity refusal"
    );
}

#[test]
fn a_classification_tightened_between_the_verdict_and_the_load_is_not_disclosed() {
    let fixture = Fixture::open();
    let payload = b"bytes reclassified while loading";
    let (evidence_id, digest) = fixture.ingest("racy", payload, true);
    let now = fixture.now;
    // Ingesting the same bytes under a Secret assertion tightens every row of the digest, the way a concurrent classification change would.
    let tighten = Box::new(move |store: &KernelStore| {
        store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent("tighten"),
                payload: payload.to_vec(),
                evidence_id: "evidence-tighten".to_string(),
                object_id: "evidence-object-tighten".to_string(),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "local_file".to_string(),
                source_id: "src/tighten".to_string(),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
                retain_until: Some(now + HOUR_MS),
                asserted_sensitivity: Sensitivity::Secret,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
    });
    let mut broker = fixture
        .broker(PROJECT, std::slice::from_ref(&evidence_id))
        .with_after_load_hook_for_test(tighten);
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id,
            artifact_digest: digest,
            byte_length: payload.len() as u64,
            retain_until: now + HOUR_MS,
        });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, now)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked,
        "the verdict is re-read on the bytes that were loaded"
    );
    assert!(broker.ledger.disclosed().next().is_none());
    assert!(broker.ledger.conclusions_usable());
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, now)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked,
        "the tightened class refuses before the hold on every later read"
    );
}

#[test]
fn a_run_has_one_project_and_it_is_the_holds() {
    let fixture = Fixture::open();
    let (capture_id, _) = fixture.ingest("capture", b"capture", true);
    let hold_binding = fixture.hold_binding(PROJECT);
    let hold = fixture
        .store
        .acquire_execution_hold(&hold_binding, &[capture_id], fixture.now + 2 * HOUR_MS)
        .unwrap();
    // The scope eligibility is judged under is derived from the hold, not supplied beside it; a hold whose project is not a digest yields no broker.
    let mut malformed = hold_binding;
    malformed.project_digest = "project-a".to_string();
    assert!(
        EvidenceBroker::new(
            RunBinding {
                hold: malformed,
                hold_id: hold.hold_id,
                destination: kernel::ArtifactDestination::Local,
            },
            QuestionTemplate::ExtractedFacts,
        )
        .is_err()
    );
}

#[test]
fn a_native_reference_may_not_carry_a_decision_derived_class() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let claim_text = "the claim as canonical text";
    let claim_evidence = fixture.ingest("claim", claim_text.as_bytes(), false);
    let (claim_object, claim_tuple) = fixture.descriptor(Publish {
        key: "claim",
        class: "canonical_claims",
        representation: "decision_summary",
        identity: &[("object_id", "decision-a")],
        evidence: &claim_evidence,
        buffer: claim_text,
        span: None,
    });
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision("decision-a")?;
            Ok(String::new())
        })
        .unwrap();
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&claim_evidence.0));
    // Issued through the native variant, the retracted decision would never be judged.
    let native = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: claim_object.clone(),
        class: OccurrenceClass::CanonicalClaims,
        source_revision: 1,
        artifact_digest: claim_evidence.1.clone(),
        evidence_id: claim_evidence.0.clone(),
        occurrence_tuple: claim_tuple,
    });
    assert_eq!(
        broker
            .read(&fixture.store, native.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "a canonical class resolves only through its originating decision"
    );
    // And the converse: a canonical reference must name a decision-derived class.
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let (native_object, _) = fixture.descriptor(Publish {
        key: "native",
        class: "messages",
        representation: "text",
        identity: &[
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", "msg-001"),
            ("block_index", "0"),
        ],
        evidence: &native_evidence,
        buffer: native_text,
        span: None,
    });
    fixture.decision("decision-live");
    let canonical = broker.aliases.issue(ReferenceExpectation::CanonicalSource {
        object_id: native_object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1,
        evidence_id: native_evidence.0,
        originating_decision_id: "decision-live".to_string(),
        decision_source_revision: 1,
    });
    assert_eq!(
        broker
            .read(&fixture.store, canonical.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_capacity_refusal_before_the_first_disclosure_still_truncates_the_evidence_set() {
    let fixture = Fixture::open();
    let (small_id, small_digest) = fixture.ingest("small", b"tiny", true);
    let (big_id, big_digest) = fixture.ingest("big", &[b'y'; 32 * 1024], true);
    let mut broker = fixture
        .broker(PROJECT, &[small_id.clone(), big_id.clone()])
        .with_buffer_limit(64);
    let oversized = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: big_id,
            artifact_digest: big_digest,
            byte_length: 32 * 1024,
            retain_until: fixture.now + HOUR_MS,
        });
    let small = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: small_id,
            artifact_digest: small_digest,
            byte_length: 4,
            retain_until: fixture.now + HOUR_MS,
        });
    // The run asked for evidence it will not see before it saw anything at all.
    assert_eq!(
        broker
            .read(&fixture.store, oversized.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::BufferLimit
    );
    broker
        .read(&fixture.store, small.as_str(), None, fixture.now)
        .unwrap();
    assert!(
        !broker.ledger.conclusions_usable(),
        "the order of refusal and disclosure does not change what the model reasoned over"
    );
}

#[test]
fn a_capture_past_its_retention_on_the_wall_clock_is_refused_whatever_now_the_caller_passes() {
    let fixture = Fixture::open();
    let retain_until = now_ms() + 50;
    let handle = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("short"),
            payload: b"short-lived capture".to_vec(),
            evidence_id: "evidence-short".to_string(),
            object_id: "evidence-object-short".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "local_file".to_string(),
            source_id: "src/short".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: Some(retain_until),
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&handle.evidence_id));
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: handle.evidence_id,
            artifact_digest: handle.digest,
            byte_length: 19,
            retain_until,
        });
    while now_ms() <= retain_until {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // A stale run timestamp still precedes the deadline; the wall clock does not.
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now - HOUR_MS)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "an expired acquisition reference is refused against the current clock"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_staged_subject_is_not_read_once_the_execution_hold_is_gone() {
    let fixture = Fixture::open();
    let subject = fixture.staged_subject("subject after cutoff");
    let (capture_id, _) = fixture.ingest("capture", b"capture", true);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&capture_id));
    let alias = broker.aliases.issue(subject);
    broker
        .read(&fixture.store, alias.as_str(), None, fixture.now + 2)
        .unwrap();
    // The run's cutoff passed: a trusted terminal receipt released its hold while the staged row's queue deadline is still live.
    fixture
        .store
        .release_execution_hold(&broker_hold_id(&broker), &fixture.hold_binding(PROJECT))
        .unwrap();
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now + 3)
            .unwrap_err()
            .code,
        RefusalCode::HoldInvalid,
        "a staged read is protected by the live execution hold like every other disclosure"
    );
}

#[test]
fn a_staged_subject_owned_by_another_job_is_out_of_scope() {
    let fixture = Fixture::open();
    let (capture_id, _) = fixture.ingest("capture", b"capture", true);
    let mut other_job = fixture.review_binding();
    other_job.owner = ReviewOwner::Job {
        job_id: "job-2".to_string(),
    };
    let reference = fixture
        .store
        .stage_review_input(ReviewStagingSpec {
            extraction_run_id: "run-2".to_string(),
            candidate_id: "subject-2".to_string(),
            producer: "history-summarizer".to_string(),
            binding: other_job.clone(),
            payload: ReviewPayload::Subject(ReviewSubject {
                facts: vec![ExtractedFact {
                    text: "another job's subject".to_string(),
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
            recorded_at: fixture.now,
            queue_deadline_at: fixture.now + 24 * HOUR_MS,
        })
        .unwrap();
    fixture
        .store
        .finish_staging_run("run-2", StagingTerminalState::Completed, fixture.now + 1)
        .unwrap();
    // The hold's subject is job-1; the row is owned by job-2 in the same project.
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&capture_id));
    let alias = broker.aliases.issue(ReferenceExpectation::StagedSubject {
        reference,
        binding: other_job,
    });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now + 2)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_decision_retired_between_the_verdict_and_the_load_is_not_disclosed() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let claim_text = "the claim as canonical text";
    let claim_evidence = fixture.ingest("claim", claim_text.as_bytes(), false);
    let (claim_object, _) = fixture.descriptor(Publish {
        key: "claim",
        class: "canonical_claims",
        representation: "decision_summary",
        identity: &[("object_id", "decision-a")],
        evidence: &claim_evidence,
        buffer: claim_text,
        span: None,
    });
    let retire = Box::new(move |store: &KernelStore| {
        store
            .commit(intent("retire"), |envelope| {
                envelope.retire_decision("decision-a")?;
                Ok(String::new())
            })
            .unwrap();
    });
    let mut broker = fixture
        .broker(PROJECT, std::slice::from_ref(&claim_evidence.0))
        .with_after_load_hook_for_test(retire);
    let alias = broker.aliases.issue(ReferenceExpectation::CanonicalSource {
        object_id: claim_object,
        class: OccurrenceClass::CanonicalClaims,
        source_revision: 1,
        artifact_digest: claim_evidence.1,
        evidence_id: claim_evidence.0,
        originating_decision_id: "decision-a".to_string(),
        decision_source_revision: 1,
    });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::OriginRevoked,
        "the object verdict is re-read on the bytes that were loaded, like the artifact verdict"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_span_descriptor_discloses_only_its_span() {
    let fixture = Fixture::open();
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let (span_object, span_tuple) = fixture.descriptor(Publish {
        key: "native-span",
        class: "messages",
        representation: "text",
        identity: &[
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", "msg-001"),
            ("block_index", "0"),
        ],
        evidence: &native_evidence,
        buffer: native_text,
        span: Some(Span { start: 0, end: 8 }),
    });
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&native_evidence.0));
    let alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: span_object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1,
        evidence_id: native_evidence.0,
        occurrence_tuple: span_tuple,
    });
    // No range means the descriptor's span, not the whole backing artifact.
    let whole = broker
        .read(&fixture.store, alias.as_str(), None, fixture.now)
        .unwrap();
    assert_eq!(whole.buffer.bytes(), b"a native");
    assert_eq!(whole.buffer.tag().charged_bytes, 8);
    // Bytes outside the span belong to other occurrences; the request is refused rather than translated.
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), Some(8..16), fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::InvalidRange
    );
    let inside = broker
        .read(&fixture.store, alias.as_str(), Some(2..8), fixture.now)
        .unwrap();
    assert_eq!(inside.buffer.bytes(), b"native");
}

#[test]
fn a_hold_released_between_the_verdict_and_the_load_is_not_disclosed() {
    let fixture = Fixture::open();
    let (capture_id, digest) = fixture.ingest("racy", b"released while loading", true);
    let hold_binding = fixture.hold_binding(PROJECT);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&capture_id));
    let hold_id = broker_hold_id(&broker);
    let release = Box::new(move |store: &KernelStore| {
        store
            .release_execution_hold(&hold_id, &hold_binding)
            .unwrap();
    });
    broker = broker.with_after_load_hook_for_test(release);
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: capture_id,
            artifact_digest: digest,
            byte_length: 22,
            retain_until: fixture.now + HOUR_MS,
        });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::HoldInvalid,
        "the hold is re-read on the bytes that were loaded, like the artifact verdict"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_capture_expiring_during_the_load_is_not_disclosed() {
    let fixture = Fixture::open();
    let retain_until = now_ms() + 300;
    let handle = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("short"),
            payload: b"expires while loading".to_vec(),
            evidence_id: "evidence-short".to_string(),
            object_id: "evidence-object-short".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "local_file".to_string(),
            source_id: "src/short".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS.to_string(),
            retain_until: Some(retain_until),
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    let wait = Box::new(move |_: &KernelStore| {
        while now_ms() <= retain_until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let mut broker = fixture
        .broker(PROJECT, std::slice::from_ref(&handle.evidence_id))
        .with_after_load_hook_for_test(wait);
    let alias = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: handle.evidence_id,
            artifact_digest: handle.digest,
            byte_length: 21,
            retain_until,
        });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, now_ms())
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "the acquisition deadline is re-read on the bytes that were loaded"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_descriptor_whose_stored_identity_does_not_reencode_is_refused() {
    let fixture = Fixture::open();
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let (span_object, span_tuple) = fixture.descriptor(Publish {
        key: "native-span",
        class: "messages",
        representation: "text",
        identity: &[
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", "msg-001"),
            ("block_index", "0"),
        ],
        evidence: &native_evidence,
        buffer: native_text,
        span: Some(Span { start: 0, end: 8 }),
    });
    // Widen the stored span while leaving the encoded tuple as it was: valid JSON, expected version, inconsistent identity.
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("kernel.sqlite")).unwrap();
    let payload: Vec<u8> = connection
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [span_object.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    let mut detail: serde_json::Value =
        serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
    detail["span"] = serde_json::json!([0, 16]);
    stored["detail"] = serde_json::Value::String(detail.to_string());
    connection
        .execute(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![stored.to_string().into_bytes(), span_object.as_str()],
        )
        .unwrap();
    drop(connection);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&native_evidence.0));
    let alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: span_object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: native_evidence.1,
        evidence_id: native_evidence.0,
        occurrence_tuple: span_tuple,
    });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "a stored identity that does not re-encode to itself is corruption, as every Kernel read treats it"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

const NATIVE_IDENTITY: [(&str, &str); 5] = [
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-001"),
    ("block_index", "0"),
];

/// Rewrites the stored detail of `object_id` in place, keeping the encoded identity as published.
fn corrupt_detail(fixture: &Fixture, object_id: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("kernel.sqlite")).unwrap();
    let payload: Vec<u8> = connection
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [object_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    let mut detail: serde_json::Value =
        serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
    edit(&mut detail);
    stored["detail"] = serde_json::Value::String(detail.to_string());
    connection
        .execute(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![stored.to_string().into_bytes(), object_id],
        )
        .unwrap();
}

#[test]
fn a_descriptor_detail_naming_another_evidence_row_is_refused() {
    let fixture = Fixture::open();
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let other_evidence = fixture.ingest("other", b"an unrelated artifact", false);
    let (object, tuple) = fixture.descriptor(Publish {
        key: "native",
        class: "messages",
        representation: "text",
        identity: &NATIVE_IDENTITY,
        evidence: &native_evidence,
        buffer: native_text,
        span: None,
    });
    // The identity still re-encodes; only the evidence linkage was redirected at another live artifact.
    corrupt_detail(&fixture, &object, |detail| {
        detail["evidence_id"] = serde_json::Value::String(other_evidence.0.clone());
        detail["artifact_digest"] = serde_json::Value::String(other_evidence.1.clone());
    });
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&native_evidence.0));
    let alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: other_evidence.1,
        evidence_id: other_evidence.0,
        occurrence_tuple: tuple,
    });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "the detail must cite the evidence the observation row cites"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn a_canonical_owner_must_be_a_live_decision() {
    let fixture = Fixture::open();
    // A live, scoped, admitted observation: served like a decision on the search surface, but nothing about it is a decision.
    let owner = "note-object";
    fixture
        .store
        .commit(intent("note"), |envelope| {
            envelope.insert_observation(kernel::ObservationSpec {
                observation_id: "note".to_string(),
                object_id: owner.to_string(),
                domain_id: DOMAIN.to_string(),
                proposition_id: None,
                scope_id: Some(SCOPE.to_string()),
                anchor_id: None,
                evidence_id: None,
                observation_kind: "note".to_string(),
                payload: kernel::ObservationPayload {
                    summary: "a note".to_string(),
                    classification: "note".to_string(),
                    detail: None,
                },
                observed_at: 1,
                dependencies: Vec::new(),
                source_kind: "assistant".to_string(),
                source_id: "note-lineage".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.record_admission(kernel::AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(owner.to_string()),
                source_class: Some(kernel::SourceClass::ExplicitUser),
                taint_class: Some(kernel::TaintClass::UserExplicit),
                event: kernel::AdmissionEvent {
                    kind: kernel::EventKind::Other,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "test".to_string(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap();
    let claim_text = "a claim whose owner is not a decision";
    let claim_evidence = fixture.ingest("claim", claim_text.as_bytes(), false);
    let (claim_object, _) = fixture.descriptor(Publish {
        key: "claim",
        class: "canonical_claims",
        representation: "decision_summary",
        identity: &[("object_id", owner)],
        evidence: &claim_evidence,
        buffer: claim_text,
        span: None,
    });
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&claim_evidence.0));
    let alias = broker.aliases.issue(ReferenceExpectation::CanonicalSource {
        object_id: claim_object,
        class: OccurrenceClass::CanonicalClaims,
        source_revision: 1,
        artifact_digest: claim_evidence.1,
        evidence_id: claim_evidence.0,
        originating_decision_id: owner.to_string(),
        decision_source_revision: 1,
    });
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged,
        "a canonical form resolves only through a decision row"
    );
    assert!(broker.ledger.disclosed().next().is_none());
}

#[test]
fn an_elapsed_retention_floor_does_not_expire_non_capture_evidence() {
    let fixture = Fixture::open();
    let native_text = "a native message";
    let handle = fixture
        .store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("floor"),
            payload: native_text.as_bytes().to_vec(),
            evidence_id: "evidence-floor".to_string(),
            object_id: "evidence-object-floor".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "conversation".to_string(),
            source_id: "src/floor".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            // A GC retention floor already in the past; the hold, not this field, decides whether the bytes are readable.
            retain_until: Some(fixture.now - HOUR_MS),
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap();
    let evidence = (handle.evidence_id, handle.digest);
    let (object, tuple) = fixture.descriptor(Publish {
        key: "floor",
        class: "messages",
        representation: "text",
        identity: &NATIVE_IDENTITY,
        evidence: &evidence,
        buffer: native_text,
        span: None,
    });
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&evidence.0));
    let alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: object,
        class: OccurrenceClass::Messages,
        source_revision: 1,
        artifact_digest: evidence.1,
        evidence_id: evidence.0,
        occurrence_tuple: tuple,
    });
    let read = broker
        .read(&fixture.store, alias.as_str(), None, fixture.now)
        .unwrap();
    assert_eq!(read.buffer.bytes(), native_text.as_bytes());
}

fn resolution_refusal(error: InvestigationError) -> RefusalCode {
    match error {
        InvestigationError::Refused(code) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Publishes a canonical-claim or promoted-memory descriptor over fresh Normal evidence whose leading identity field names `owner`.
fn derived(
    fixture: &Fixture,
    key: &str,
    class: OccurrenceClass,
    owner: &str,
) -> (String, (String, String)) {
    let text = format!("{key}: a form of {owner}");
    let evidence = fixture.ingest(key, text.as_bytes(), false);
    let (object, _) = fixture.descriptor(Publish {
        key,
        class: class.code(),
        representation: class.representations()[0],
        identity: &[(class.identity_fields()[0], owner)],
        evidence: &evidence,
        buffer: &text,
        span: None,
    });
    (object, evidence)
}

// Opening the production gate must rewrite the witnesses below; a constant assertion makes that a compile error rather than a silent pass.
const _: () = assert!(!PRODUCTION_SELECTION_OPEN);

/// A class the selector walks but the resolver treats as native would be targeted at its own descriptor, so its owner's retraction would never be consulted; a class the resolver treats as decision-derived but the selector skips would never be reviewed. Every class must fall on the same side of both.
#[test]
fn the_selected_classes_are_exactly_the_decision_derived_classes() {
    for class in OccurrenceClass::ALL {
        assert_eq!(
            decision_derived(class),
            MEMORY_CLASSES.contains(&class),
            "{class:?} is selected and decision-derived together or not at all"
        );
    }
}

#[test]
fn canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let (claim, claim_evidence) = derived(
        &fixture,
        "claim",
        OccurrenceClass::CanonicalClaims,
        "decision-a",
    );
    let (promoted, promoted_evidence) = derived(
        &fixture,
        "promoted",
        OccurrenceClass::PromotedMemory,
        "decision-a",
    );
    // A second commit appending an event to the decision separates its last change from its creation, so a target built from `created_commit_seq` alone would differ.
    fixture
        .store
        .commit(intent("touch-decision-a"), |envelope| {
            envelope.append_decision_event(
                "decision-decision-a",
                kernel::DecisionEventSpec {
                    event_kind: "note".to_string(),
                    payload: kernel::DecisionEventPayload {
                        summary: "touched".to_string(),
                    },
                    evidence_id: None,
                    recorded_at: 1,
                },
            )?;
            Ok(String::new())
        })
        .unwrap();
    let (tip, before) = fixture.canonical_state(&["decision-a", &claim, &promoted]);
    let decision = before[0].clone().unwrap();
    let last_change = decision.latest_change_commit_seq.unwrap();
    assert!(
        last_change > decision.object.created_commit_seq,
        "the touch commit is the decision's last change"
    );
    let expected = kernel::ProposalTarget::Memory(kernel::CanonicalTarget {
        object_id: "decision-a".to_string(),
        source_revision: 1,
        known_as_of: tip,
        commit_token: last_change,
    });
    for (object, evidence, class) in [
        (&claim, &claim_evidence, OccurrenceClass::CanonicalClaims),
        (
            &promoted,
            &promoted_evidence,
            OccurrenceClass::PromotedMemory,
        ),
    ] {
        assert!(decision_derived(class) && MEMORY_CLASSES.contains(&class));
        let subject = fixture.resolve(object).unwrap();
        assert_eq!(
            subject,
            ReferenceExpectation::CanonicalSource {
                object_id: object.clone(),
                class,
                source_revision: 1,
                artifact_digest: evidence.1.clone(),
                evidence_id: evidence.0.clone(),
                originating_decision_id: "decision-a".to_string(),
                decision_source_revision: 1,
            },
            "{class:?} resolves to its decision, never to a native expectation"
        );
        // Two descriptors of one decision name one mutation target; neither descriptor's own id is the target.
        let target = proposal_target(&fixture.store, &subject).unwrap();
        assert_eq!(target, expected);
    }
    assert!(!MEMORY_CLASSES.contains(&OccurrenceClass::GitCommits));
    // Resolution and binding are reads: nothing canonical moved.
    assert_eq!(
        fixture.canonical_state(&["decision-a", &claim, &promoted]),
        (tip, before)
    );
}

#[test]
fn a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let (claim, claim_evidence) = derived(
        &fixture,
        "claim",
        OccurrenceClass::CanonicalClaims,
        "decision-a",
    );
    let bound = fixture.resolve(&claim).unwrap();
    // Supersession after binding: the bound target refuses as a revoked origin rather than retargeting to the successor, and a fresh resolution refuses too because the descriptor's identity still names the superseded decision.
    fixture.revise_decision("decision-a", "decision-a-r2");
    let (_, after_revision) = fixture.canonical_state(&["decision-a", "decision-a-r2", &claim]);
    assert_eq!(
        proposal_target(&fixture.store, &bound).unwrap_err().code,
        RefusalCode::OriginRevoked
    );
    assert_eq!(
        resolution_refusal(fixture.resolve(&claim).unwrap_err()),
        RefusalCode::OriginRevoked
    );
    // A stale descriptor revision under a live decision is refused by the broker before any byte is read.
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&claim_evidence.0));
    let stale_descriptor = broker.aliases.issue(ReferenceExpectation::CanonicalSource {
        object_id: claim.clone(),
        class: OccurrenceClass::CanonicalClaims,
        source_revision: 7,
        artifact_digest: claim_evidence.1.clone(),
        evidence_id: claim_evidence.0.clone(),
        originating_decision_id: "decision-a-r2".to_string(),
        decision_source_revision: 2,
    });
    assert_eq!(
        broker
            .read(&fixture.store, stale_descriptor.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 0);
    // An owner the registry has never seen has nothing to resolve through.
    let (orphan, _) = derived(
        &fixture,
        "orphan",
        OccurrenceClass::PromotedMemory,
        "decision-missing",
    );
    assert_eq!(
        resolution_refusal(fixture.resolve(&orphan).unwrap_err()),
        RefusalCode::NotFound
    );
    // A bound decision revision that disagrees with the live row refuses instead of retargeting.
    fixture.decision("decision-b");
    let (other, _) = derived(
        &fixture,
        "other",
        OccurrenceClass::CanonicalClaims,
        "decision-b",
    );
    let mut stale_decision = fixture.resolve(&other).unwrap();
    let ReferenceExpectation::CanonicalSource {
        decision_source_revision,
        ..
    } = &mut stale_decision
    else {
        panic!("canonical")
    };
    *decision_source_revision = 7;
    assert_eq!(
        proposal_target(&fixture.store, &stale_decision)
            .unwrap_err()
            .code,
        RefusalCode::ExpectationChanged
    );
    // An owner that is a registered object but not a decision: resolution binds it, the target refuses it, and the broker refuses the read with zero bytes.
    let (wrong_kind, wrong_evidence) = derived(
        &fixture,
        "wrong",
        OccurrenceClass::CanonicalClaims,
        "evidence-object-claim",
    );
    let subject = fixture.resolve(&wrong_kind).unwrap();
    assert_eq!(
        proposal_target(&fixture.store, &subject).unwrap_err().code,
        RefusalCode::ExpectationChanged
    );
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&wrong_evidence.0));
    let alias = broker.aliases.issue(subject);
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 0);
    // An owner scoped to another project: resolution binds it, and the broker refuses the read `Scope` on the decision candidate with zero bytes, even though the descriptor itself is in scope.
    fixture
        .store
        .commit(intent("scope-b"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: "project:b".to_string(),
                object_id: "project:b".to_string(),
                source_id: "project:b".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "kernel_route".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: vec![ScopeTermSpec {
                    dimension: Dimension::Project.as_str().to_string(),
                    operator: "exact".to_string(),
                    exact_value: Some(OTHER_PROJECT.to_string()),
                    ..ScopeTermSpec::default()
                }],
            })?;
            Ok(String::new())
        })
        .unwrap();
    fixture.decision_in("decision-elsewhere", "project:b");
    let (foreign, foreign_evidence) = derived(
        &fixture,
        "foreign",
        OccurrenceClass::CanonicalClaims,
        "decision-elsewhere",
    );
    let subject = fixture.resolve(&foreign).unwrap();
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&foreign_evidence.0));
    let alias = broker.aliases.issue(subject);
    assert_eq!(
        broker
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::Scope
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 0);
    // Every refusal above was a read; the tracked objects moved only where the test moved them.
    assert_eq!(
        fixture
            .canonical_state(&["decision-a", "decision-a-r2", &claim])
            .1,
        after_revision
    );
}

#[test]
fn a_resolved_canonical_subject_still_refuses_the_remote_destination() {
    let fixture = Fixture::open();
    fixture.decision("decision-a");
    let (claim, claim_evidence) = derived(
        &fixture,
        "claim",
        OccurrenceClass::CanonicalClaims,
        "decision-a",
    );
    // The artifact asserts Normal and RemoteAllowed, and carries no repository provenance; the store's own rule keeps it Sensitive, so the destination the production coordinator binds refuses it with zero bytes. This is the abstention a scheduled production job settles on, and why the production gate stays closed.
    let subject = fixture.resolve(&claim).unwrap();
    let mut remote = fixture.broker_for(
        PROJECT,
        std::slice::from_ref(&claim_evidence.0),
        kernel::ArtifactDestination::Remote,
    );
    let alias = remote.aliases.issue(subject);
    assert_eq!(
        remote
            .read(&fixture.store, alias.as_str(), None, fixture.now)
            .unwrap_err()
            .code,
        RefusalCode::PolicyBlocked
    );
    assert_eq!(remote.accounting.model_visible_bytes(), 0);
}
