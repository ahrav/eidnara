//! Real-store proofs for the Curator evidence broker: scope and staleness refusal, live-origin revocation, canonical and promoted deduplication, uncited-context lineage, unsupported bound questions (Q24), the render check, hold growth, and the accounting bounds.

use daemon::curator::broker::{
    EvidenceBroker, JudgedAt, MAX_MODEL_VISIBLE_BYTES, MAX_OPERATIONS_PER_BATCH, OriginClass,
    QuestionTemplate, ReferenceExpectation, RefusalCode, RunBinding,
};
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    ArtifactIngestRequest, CURATOR_CAPTURE_RETENTION_CLASS, CommitIntent, CuratorHoldBinding,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, ExtractedFact, KernelStore, ProjectScope,
    ProviderEgress, ReviewBinding, ReviewOwner, ReviewPayload, ReviewStagingSpec, ReviewSubject,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceDependency, SourceDescriptorPolicy,
    SourceDescriptorRequest, SourceSpan, StagingTerminalState,
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
        producer: "curator-broker-test".to_string(),
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

    fn hold_binding(&self, project: &str) -> CuratorHoldBinding {
        CuratorHoldBinding {
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
                    CURATOR_CAPTURE_RETENTION_CLASS.to_string()
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
                project: ProjectScope::new(project).unwrap(),
                hold: binding,
                hold_id: hold.hold_id,
                destination,
            },
            QuestionTemplate::ExtractedFacts,
        )
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
                        span: SourceSpan {
                            alias: "s1".to_string(),
                            start: 0,
                            end: 4,
                        },
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
                span: SourceSpan {
                    alias: "s1".to_string(),
                    start: 0,
                    end: 4,
                },
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
        self.store
            .commit(intent(&format!("decision-{object}")), |envelope| {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("decision-{object}"),
                    object_id: object.to_string(),
                    domain_id: DOMAIN.to_string(),
                    proposition_id: None,
                    scope_id: Some(SCOPE.to_string()),
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

    /// Publishes one descriptor over `buffer` and returns its object id and the descriptor's identity tuple bytes.
    fn descriptor(
        &self,
        key: &str,
        class: &str,
        representation: &str,
        identity: &[(&str, &str)],
        evidence: &(String, String),
        buffer: &str,
    ) -> (String, Vec<u8>) {
        let mut published = None;
        self.store
            .commit(intent(&format!("descriptor-{key}")), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class,
                            identity,
                            revision: "1",
                            representation,
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: DOMAIN,
                        scope_id: Some(SCOPE),
                        evidence_id: &evidence.0,
                        artifact_digest: &evidence.1,
                        buffer,
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
    assert_eq!(read.buffer.bytes, b"bun builds the workspace");
    assert_eq!(read.buffer.tag.origin, OriginClass::StagedSubject);
    assert_eq!(read.buffer.tag.charged_bytes, 24);
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
    assert_eq!(ranged.buffer.bytes, b"bun");
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
        kernel::CuratorHoldKind::Execution,
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
    assert_eq!(read.buffer.bytes, b"file");
    assert_eq!(read.buffer.tag.origin, OriginClass::TemporaryCapture);
    assert_eq!(broker.buffers.loaded(), 1);
    fixture
        .store
        .validate_held_evidence(
            &broker_hold_id(&broker),
            kernel::CuratorHoldKind::Execution,
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
    let mut broker = fixture.broker(PROJECT, &[secret_id.clone(), placeholder_id.clone()]);
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
            secret_id,
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
    assert!(broker.ledger.disclosed().next().is_none());
    assert!(
        broker
            .render_host_text(&format!("question {AWS_KEY}"))
            .is_err()
    );
    let host = broker
        .render_host_text(QuestionTemplate::ExtractedFacts.text())
        .unwrap();
    assert_eq!(host.tag.origin, OriginClass::HostAuthored);
    assert_eq!(host.tag.charged_bytes, 0);
    assert_eq!(
        QuestionTemplate::parse("extracted_facts"),
        Some(QuestionTemplate::ExtractedFacts)
    );
    assert_eq!(
        QuestionTemplate::parse("Is the deploy key rotated? extracted_facts"),
        None,
        "source-derived question text is not a supported input"
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
    let (claim_object, _) = fixture.descriptor(
        "claim",
        "canonical_claims",
        "decision_summary",
        &[("object_id", "decision-a")],
        &claim_evidence,
        claim_text,
    );
    let (promoted_object, _) = fixture.descriptor(
        "promoted",
        "promoted_memory",
        "summary",
        &[("decision_object_id", "decision-a")],
        &promoted_evidence,
        promoted_text,
    );
    let native_text = "a native message";
    let native_evidence = fixture.ingest("native", native_text.as_bytes(), false);
    let (native_object, native_tuple) = fixture.descriptor(
        "native",
        "messages",
        "text",
        &[
            ("project_id", "proj-a"),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", "msg-001"),
            ("block_index", "0"),
        ],
        &native_evidence,
        native_text,
    );
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
    assert_eq!(first.buffer.bytes, claim_text.as_bytes());
    assert_eq!(first.origin_key, "decision:decision-a");
    assert_eq!(
        first.buffer.tag.verdict,
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
    // A fabricated identity tuple is refused before bytes are read.
    let mut forged = native_tuple.clone();
    forged.push(0);
    let forged_alias = broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: native_object,
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
        object_id: native_alias.as_str().to_string(),
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
            evidence_id: capture_id,
            artifact_digest: capture_digest,
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
    // Rendered bytes are charged on every render; the bound refuses whole rather than truncating.
    let (big_id, big_digest) = fixture.ingest("big", &[b'y'; 4096], true);
    let mut broker = fixture
        .broker(PROJECT, std::slice::from_ref(&big_id))
        .with_inspection_limit(1_000);
    let big = broker
        .aliases
        .issue(ReferenceExpectation::TemporaryCapture {
            evidence_id: big_id.clone(),
            artifact_digest: big_digest.clone(),
            byte_length: 4096,
            retain_until: fixture.now + HOUR_MS,
        });
    let renders = MAX_MODEL_VISIBLE_BYTES / 4096;
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
            byte_length: 4096,
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
