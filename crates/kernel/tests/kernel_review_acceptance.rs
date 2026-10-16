#![cfg(feature = "test-support")]

//! Real-store proofs for exact-target acceptance of a MemoryReviewer proposal: each action family's canonical effect and its actual admission, proposal-level settlement replay across caller keys and after the review hold ends, caller-key conflict, and refusal of a changed target, another project, a released hold, deleted or tightened evidence, and a moved owner, each writing nothing.

use context_core::memory_reviewer_policy_union::{PolicyUnion, PolicyUnionMember};
use kernel::source_identity::Occurrence;
use kernel::{
    AcceptanceError, AcceptanceRefusal, AdmissionEvent, AdmissionRequest, ArtifactDeletionIdentity,
    ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactDestination, ArtifactIngestRequest,
    CanonicalTarget, CommitIntent, DecisionEventPayload, DecisionEventSpec, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EligibilityCandidate, EventKind, EvidenceReference,
    KernelStore, ManifestReference, MemoryReviewerHoldBinding, PolicyDependencies, ProjectScope,
    ProposalAcceptance, ProposalAction, ProposalTarget, ProviderEgress,
    REVIEW_DEPENDENCIES_VERSION, ReviewBinding, ReviewDependencies, ReviewOwner, ReviewPayload,
    ReviewProposal, ReviewQuestionTemplate, ReviewStagingSpec, ScopeSpec, ScopeTermSpec,
    Sensitivity, Settlement, SettlementEffect, SourceClass, SourceDependency,
    SourceDescriptorPolicy, SourceDescriptorRequest, StagingTerminalState, Surface, TaintClass,
    Uncertainty, provisional_result_identity,
};
use sha2::{Digest, Sha256};

const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_PROJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MEMSTORE: &str = "memstore-incarnation";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-review-acceptance-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn admission(object_id: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_string(),
        },
    }
}

/// A proposal staged, sealed, and transferred to its review hold, with what its receipt would carry.
struct Staged {
    run: MemoryReviewerHoldBinding,
    reference: kernel::ReviewStagedReference,
    target: ProposalTarget,
}

struct Fixture {
    _directory: tempfile::TempDir,
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
                for (scope, project) in [(SCOPE, PROJECT), ("project:b", OTHER_PROJECT)] {
                    envelope.insert_scope(ScopeSpec {
                        scope_id: scope.to_string(),
                        object_id: scope.to_string(),
                        source_id: scope.to_string(),
                        domain_id: DOMAIN.to_string(),
                        source_kind: "kernel_route".to_string(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                        terms: vec![ScopeTermSpec {
                            dimension: Dimension::Project.as_str().to_string(),
                            operator: "exact".to_string(),
                            exact_value: Some(project.to_string()),
                            ..ScopeTermSpec::default()
                        }],
                    })?;
                }
                Ok(String::new())
            })
            .unwrap();
        Self {
            _directory: directory,
            store,
            now: now_ms(),
        }
    }

    fn incarnation(&self) -> String {
        self.store
            .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::unbounded())
            .unwrap()
    }

    /// Ingests `claim {key}` as evidence; returns its id and digest.
    fn ingest(&self, key: &str) -> (String, String) {
        let handle = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(&format!("evidence-{key}")),
                payload: format!("claim {key}").into_bytes(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "conversation".to_string(),
                source_id: format!("src/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    fn tighten(&self, key: &str) {
        self.store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(&format!("tighten-{key}")),
                payload: format!("claim {key}").into_bytes(),
                evidence_id: format!("evidence-tighten-{key}"),
                object_id: format!("evidence-object-tighten-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "conversation".to_string(),
                source_id: format!("src/tighten-{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Secret,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
    }

    /// An admitted, scoped decision with one canonical descriptor over its own evidence; returns the descriptor and the evidence.
    fn decision(&self, object: &str) -> (String, String) {
        self.decision_of_kind(object, "architecture")
    }

    fn decision_of_kind(&self, object: &str, kind: &str) -> (String, String) {
        self.decision_admitted(object, kind, admission(object))
    }

    /// [`Self::decision`] of `kind`, admitted by `request`.
    fn decision_admitted(
        &self,
        object: &str,
        kind: &str,
        request: AdmissionRequest,
    ) -> (String, String) {
        let (evidence, digest) = self.ingest(object);
        let mut descriptor = None;
        self.store
            .commit(intent(&format!("decision-{object}")), |envelope| {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("{object}-id"),
                    object_id: object.to_string(),
                    domain_id: DOMAIN.to_string(),
                    proposition_id: None,
                    scope_id: Some(SCOPE.to_string()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: kind.to_string(),
                    payload: DecisionPayload {
                        summary: format!("summary {object}"),
                        rationale: format!("rationale {object}"),
                    },
                    source_kind: "repo".to_string(),
                    source_id: format!("src/{object}"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.record_admission(request)?;
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: "canonical_claims",
                            identity: &[("object_id", object)],
                            revision: "1",
                            representation: "decision_summary",
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: DOMAIN,
                        scope_id: Some(SCOPE),
                        evidence_id: &evidence,
                        artifact_digest: &digest,
                        buffer: &format!("claim {object}"),
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
                descriptor = Some(outcome.object_id);
                Ok(String::new())
            })
            .unwrap();
        (descriptor.unwrap(), evidence)
    }

    /// The decision's exact tokens at the tip.
    fn memory_target(&self, object: &str) -> ProposalTarget {
        let (tip, states) = self.store.object_states(&[object.to_string()]).unwrap();
        let state = states[0].clone().unwrap();
        ProposalTarget::Memory(CanonicalTarget {
            object_id: object.to_string(),
            source_revision: state.object.source_revision,
            known_as_of: tip,
            commit_token: state.latest_change_commit_seq.unwrap(),
        })
    }

    /// A sealed staged subject owned by `job`; returns its candidate id.
    fn subject(&self, job: &str) -> String {
        let candidate_id = format!("subject-{job}");
        let run = format!("subject-run-{job}");
        self.store
            .stage_review_input(ReviewStagingSpec {
                extraction_run_id: run.clone(),
                candidate_id: candidate_id.clone(),
                producer: "history-summarizer".to_string(),
                binding: self.review_binding(ReviewOwner::Job {
                    job_id: job.to_string(),
                }),
                payload: ReviewPayload::Subject(kernel::ReviewSubject {
                    facts: vec![kernel::ExtractedFact {
                        text: "bun builds the workspace".to_string(),
                        spans: vec![kernel::SourceSpan {
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
                recorded_at: self.now - 2_000,
                queue_deadline_at: self.now + DAY_MS - 2_000,
                dependencies: None,
            })
            .unwrap();
        self.store
            .finish_staging_run(&run, StagingTerminalState::Completed, self.now - 1_500)
            .unwrap();
        candidate_id
    }

    fn review_binding(&self, owner: ReviewOwner) -> ReviewBinding {
        ReviewBinding {
            project_digest: PROJECT.to_string(),
            domain_id: DOMAIN.to_string(),
            owner,
            subject_source: SourceDependency {
                source_kind: "conversation".to_string(),
                source_id: "session-1".to_string(),
                source_revision: 1,
            },
            reference_sources: vec![],
        }
    }

    /// Stages, seals, and transfers `job`'s proposal of `action` over `target`, disclosing and citing `evidence` with `members` in its policy union.
    fn propose(
        &self,
        job: &str,
        action: ProposalAction,
        target: ProposalTarget,
        new_text: Option<&str>,
        evidence: &[String],
        members: Vec<PolicyUnionMember>,
    ) -> Staged {
        let run = MemoryReviewerHoldBinding {
            project_digest: PROJECT.to_string(),
            kernel_incarnation: self.incarnation(),
            memstore_incarnation: MEMSTORE.to_string(),
            subject: job.to_string(),
            generation: 1,
        };
        let hold = self
            .store
            .acquire_execution_hold(&run, evidence, self.now + HOUR_MS)
            .unwrap();
        let identity = provisional_result_identity(job, 1);
        let review = MemoryReviewerHoldBinding {
            subject: identity.candidate_id.clone(),
            ..run.clone()
        };
        let mut union = PolicyUnion::new();
        union.insert(PolicyUnionMember {
            kind: "question_template".to_string(),
            id: "extracted_facts".to_string(),
            revision: "1".to_string(),
            owner_id: None,
            owner_revision: None,
        });
        for member in members {
            union.insert(member);
        }
        let union = union.encode().unwrap();
        let references: Vec<EvidenceReference> = evidence
            .iter()
            .map(|evidence_id| EvidenceReference {
                evidence_id: evidence_id.clone(),
                span: None,
            })
            .collect();
        let sealed = self
            .store
            .stage_sealed_review_proposal(
                ReviewStagingSpec {
                    extraction_run_id: identity.extraction_run_id,
                    candidate_id: identity.candidate_id,
                    producer: "memory_reviewer".to_string(),
                    binding: self.review_binding(ReviewOwner::Proposal {
                        job_id: job.to_string(),
                        generation: 1,
                    }),
                    payload: ReviewPayload::Proposal(Box::new(ReviewProposal {
                        action,
                        target: target.clone(),
                        new_text: new_text.map(str::to_string),
                        support: references.clone(),
                        contradictions: vec![],
                        limitations: vec![],
                        uncertainty: Uncertainty::Medium,
                        manifest: ManifestReference {
                            manifest_id: "manifest-1".to_string(),
                            digest: "d".repeat(64),
                        },
                        policy_dependencies: PolicyDependencies {
                            question_template: ReviewQuestionTemplate::ExtractedFacts,
                            disclosed_inputs: references,
                            uncited_disclosed_inputs: vec![],
                            ancestry: vec![],
                        },
                    })),
                    recorded_at: self.now - 1_000,
                    queue_deadline_at: self.now + DAY_MS - 1_000,
                    dependencies: Some(ReviewDependencies {
                        version: REVIEW_DEPENDENCIES_VERSION,
                        union_canonical: union.canonical,
                        union_digest: union.digest,
                        generation: 1,
                        attempt_index: 0,
                        body_digest: "b".repeat(64),
                        marker_union_digest: "e".repeat(64),
                    }),
                },
                &hold.hold_id,
                &run,
                &review,
            )
            .unwrap();
        Staged {
            run,
            reference: sealed.reference,
            target,
        }
    }

    /// A proposal over `object` whose union names its descriptor and owner, citing the descriptor's evidence.
    fn propose_over(
        &self,
        job: &str,
        action: ProposalAction,
        object: &str,
        descriptor: &(String, String),
        new_text: Option<&str>,
    ) -> Staged {
        self.propose(
            job,
            action,
            self.memory_target(object),
            new_text,
            std::slice::from_ref(&descriptor.1),
            vec![PolicyUnionMember {
                kind: "canonical_source".to_string(),
                id: descriptor.0.clone(),
                revision: "1".to_string(),
                owner_id: Some(object.to_string()),
                owner_revision: Some("1".to_string()),
            }],
        )
    }

    fn accept(
        &self,
        key: &str,
        staged: &Staged,
    ) -> Result<kernel::AcceptanceOutcome, AcceptanceError> {
        self.accept_as(key, staged, staged.run.clone(), staged.target.clone())
    }

    fn accept_as(
        &self,
        key: &str,
        staged: &Staged,
        run: MemoryReviewerHoldBinding,
        target: ProposalTarget,
    ) -> Result<kernel::AcceptanceOutcome, AcceptanceError> {
        self.store
            .accept_review_proposal(&self.acceptance(key, staged, run, target))
    }

    fn acceptance(
        &self,
        key: &str,
        staged: &Staged,
        run: MemoryReviewerHoldBinding,
        target: ProposalTarget,
    ) -> ProposalAcceptance {
        ProposalAcceptance {
            intent: intent(key),
            run,
            proposal: staged.reference.clone(),
            selected_at: self.now,
            target,
            scope_id: Some(SCOPE.to_string()),
        }
    }

    /// The effective maturity of `object`'s latest admission.
    fn maturity(&self, object: &str) -> String {
        rusqlite::Connection::open(self._directory.path().join("kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT effective_maturity FROM admission_decisions WHERE subject_object_id=?1
                 ORDER BY commit_seq DESC,admission_decision_id DESC LIMIT 1",
                [object],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// The scope and kind of `object`'s decision row.
    fn decision_row(&self, object: &str) -> (Option<String>, String) {
        rusqlite::Connection::open(self._directory.path().join("kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT scope_id,decision_kind FROM decisions WHERE object_id=?1",
                [object],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    /// Runs `sql` against the store's database file directly.
    fn execute(&self, sql: &str) {
        rusqlite::Connection::open(self._directory.path().join("kernel.sqlite"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }

    fn state(&self, object: &str) -> kernel::ObjectState {
        self.store.object_states(&[object.to_string()]).unwrap().1[0]
            .clone()
            .unwrap()
    }

    /// Whether `object` is served on `surface` to a local reader of the project.
    fn served(&self, object: &str, surface: Surface) -> bool {
        let state = self.state(object);
        self.store
            .judge_surface_eligibility_within_budget(
                &ProjectScope::new(PROJECT).unwrap(),
                ArtifactDestination::Local,
                surface,
                &[EligibilityCandidate {
                    object_id: object.to_string(),
                    source_revision: state.object.source_revision,
                    artifact_digest: None,
                }],
                &kernel::applicability::EvalBudget::unbounded(),
            )
            .unwrap()
            .verdicts[0]
            .permits()
    }

    fn release_review_hold(&self, staged: &Staged) {
        let review = MemoryReviewerHoldBinding {
            subject: staged.reference.candidate_id.clone(),
            ..staged.run.clone()
        };
        let hold = self
            .store
            .lookup_review_hold(&review, self.now)
            .unwrap()
            .unwrap();
        self.store
            .release_review_hold(&hold.hold_id, &review)
            .unwrap();
    }
}

fn settled(outcome: &kernel::AcceptanceOutcome) -> &Settlement {
    &outcome.settlement
}

#[test]
fn retain_settles_once_without_a_canonical_change_and_replays_across_keys_and_after_the_hold() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retain,
        "decision-a",
        &descriptor,
        None,
    );
    let before = fixture.state("decision-a");
    let served = |fixture: &Fixture| {
        (
            fixture.served("decision-a", Surface::ExplicitSearch),
            fixture.served("decision-a", Surface::AutoInject),
        )
    };
    let served_before = served(&fixture);
    let first = fixture.accept("caller-1", &staged).unwrap();
    assert!(!first.replayed);
    let settlement = settled(&first);
    assert_eq!(settlement.effect, SettlementEffect::Unchanged);
    assert_eq!(
        (
            settlement.proposal_id.as_str(),
            settlement.payload_digest.as_str(),
            settlement.receipt_generation,
            settlement.project_digest.as_str(),
            settlement.memstore_incarnation.as_str(),
        ),
        (
            staged.reference.candidate_id.as_str(),
            staged.reference.payload_digest.as_str(),
            1,
            PROJECT,
            MEMSTORE,
        )
    );
    assert_eq!(settlement.target, staged.target);
    assert_eq!(settlement.commit_seq, fixture.store.tip().unwrap());
    // No canonical write, maturity change, or corroboration: the decision's row and last change are as before.
    assert_eq!(fixture.state("decision-a"), before);
    assert_eq!(served(&fixture), served_before, "no maturity change");
    let tip = fixture.store.tip().unwrap();
    // Another caller key replays the same settlement without a commit.
    let second = fixture.accept("caller-2", &staged).unwrap();
    assert!(second.replayed);
    assert_eq!(second.settlement, first.settlement);
    assert_eq!(fixture.store.tip().unwrap(), tip);
    // The replay is answered before the review hold: a released hold still replays.
    fixture.release_review_hold(&staged);
    let third = fixture.accept("caller-3", &staged).unwrap();
    assert_eq!(
        (third.replayed, &third.settlement),
        (true, &first.settlement)
    );
    assert_eq!(
        fixture
            .store
            .review_settlement(&staged.run, &staged.reference)
            .unwrap(),
        Some(first.settlement.clone())
    );
    // A reused caller key with other input conflicts.
    let reused = fixture.store.accept_review_proposal(&ProposalAcceptance {
        intent: CommitIntent {
            request_digest: "c".repeat(64),
            ..intent("caller-1")
        },
        run: staged.run.clone(),
        proposal: staged.reference.clone(),
        selected_at: fixture.now,
        target: staged.target.clone(),
        scope_id: None,
    });
    assert_eq!(
        reused,
        Err(AcceptanceError::Refused(AcceptanceRefusal::Conflict))
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
}

#[test]
fn create_inserts_one_decision_admitted_as_model_inference_with_durable_support() {
    let fixture = Fixture::open();
    let subject = fixture.subject("job-1");
    let (support, support_digest) = fixture.ingest("support");
    let staged = fixture.propose(
        "job-1",
        ProposalAction::Create,
        ProposalTarget::StagedCandidate {
            candidate_id: subject.clone(),
        },
        Some("bun builds the workspace"),
        std::slice::from_ref(&support),
        vec![PolicyUnionMember {
            kind: "temporary_capture".to_string(),
            id: support.clone(),
            revision: support_digest,
            owner_id: None,
            owner_revision: None,
        }],
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    let SettlementEffect::Created {
        object_id,
        admission,
    } = &outcome.settlement.effect
    else {
        panic!("{:?}", outcome.settlement.effect)
    };
    // The admission actually recorded: model inference at the candidate ceiling, explicit-only, whatever the outcome word.
    assert_eq!(
        (
            admission.source_class.as_str(),
            admission.taint_class.as_str(),
            admission.effective_maturity.as_str(),
            admission.disposition.as_str(),
            admission.visibility.as_str(),
        ),
        (
            "model_inference",
            "memory_classifier_inference",
            "candidate",
            "active",
            "explicit_labeled"
        )
    );
    let state = fixture.state(object_id);
    assert_eq!(
        (
            state.object.object_kind.as_str(),
            state.object.source_kind.as_str(),
            state.object.source_id.as_str(),
            state.object.source_revision,
            state.object.invalidated_commit_seq,
        ),
        (
            "decision",
            kernel::ACCEPTED_SOURCE_KIND,
            staged.reference.candidate_id.as_str(),
            1,
            None
        )
    );
    // Explicit retrieval serves it; automatic retrieval respects the candidate ceiling.
    assert!(fixture.served(object_id, Surface::ExplicitSearch));
    assert!(!fixture.served(object_id, Surface::AutoInject));
    // The support is cited by the decision itself, so it outlives the review hold.
    let linked = fixture
        .store
        .linked_decision_evidence_within_budget(
            object_id,
            fixture.store.tip().unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
            &kernel::applicability::EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(
        linked
            .iter()
            .map(|link| link.evidence_id.as_str())
            .collect::<Vec<_>>(),
        vec![support.as_str()]
    );
    fixture.release_review_hold(&staged);
    let retired = fixture.store.commit(intent("retire-support"), |envelope| {
        envelope.retire_evidence("evidence-object-support")?;
        Ok(String::new())
    });
    assert_eq!(
        retired.unwrap_err(),
        kernel::KernelError::Conflict,
        "cited support is kept"
    );
    // A second acceptance replays: one decision, not two.
    let replay = fixture.accept("caller-2", &staged).unwrap();
    assert_eq!(
        (replay.replayed, &replay.settlement),
        (true, &outcome.settlement)
    );
}

#[test]
fn a_capture_member_stands_only_live_at_its_digest_whether_or_not_it_was_disclosed() {
    let fixture = Fixture::open();
    let (support, support_digest) = fixture.ingest("support");
    let (other, other_digest) = fixture.ingest("other");
    let (gone, gone_digest) = fixture.ingest("gone");
    fixture
        .store
        .commit(intent("retire-gone"), |envelope| {
            envelope.retire_evidence("evidence-object-gone")?;
            Ok(String::new())
        })
        .unwrap();
    let capture = |id: &str, revision: &str| PolicyUnionMember {
        kind: "temporary_capture".to_string(),
        id: id.to_string(),
        revision: revision.to_string(),
        owner_id: None,
        owner_revision: None,
    };
    let create = |job: &str, members: Vec<PolicyUnionMember>| {
        let subject = fixture.subject(job);
        fixture.propose(
            job,
            ProposalAction::Create,
            ProposalTarget::StagedCandidate {
                candidate_id: subject,
            },
            Some("bun builds the workspace"),
            std::slice::from_ref(&support),
            members,
        )
    };
    let refused = |key: &str, staged: &Staged| {
        let tip = fixture.store.tip().unwrap();
        let error = fixture.accept(key, staged).unwrap_err();
        assert_eq!(
            fixture.store.tip().unwrap(),
            tip,
            "a refusal commits nothing"
        );
        error
    };
    let changed = AcceptanceError::Refused(AcceptanceRefusal::Changed);
    let staged = create("job-1", vec![capture(&support, &other_digest)]);
    assert_eq!(
        refused("caller-1", &staged),
        changed,
        "a disclosed capture named at another digest"
    );
    let staged = create(
        "job-2",
        vec![
            capture(&support, &support_digest),
            capture(&gone, &gone_digest),
        ],
    );
    assert_eq!(
        refused("caller-2", &staged),
        changed,
        "an undisclosed capture whose evidence was retired"
    );
    let staged = create(
        "job-3",
        vec![
            capture(&support, &support_digest),
            capture(&other, &other_digest),
        ],
    );
    let outcome = fixture.accept("caller-3", &staged).unwrap();
    assert!(
        matches!(outcome.settlement.effect, SettlementEffect::Created { .. }),
        "an undisclosed capture live at its digest stands"
    );
}

#[test]
fn create_cites_each_support_row_in_its_own_ordered_event() {
    let fixture = Fixture::open();
    let subject = fixture.subject("job-1");
    let mut support = Vec::new();
    let mut members = Vec::new();
    for key in ["support-c", "support-a", "support-b"] {
        let (evidence, digest) = fixture.ingest(key);
        members.push(PolicyUnionMember {
            kind: "temporary_capture".to_string(),
            id: evidence.clone(),
            revision: digest,
            owner_id: None,
            owner_revision: None,
        });
        support.push(evidence);
    }
    let staged = fixture.propose(
        "job-1",
        ProposalAction::Create,
        ProposalTarget::StagedCandidate {
            candidate_id: subject,
        },
        Some("bun builds the workspace"),
        &support,
        members,
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    let SettlementEffect::Created { object_id, .. } = &outcome.settlement.effect else {
        panic!("{:?}", outcome.settlement.effect)
    };
    let connection =
        rusqlite::Connection::open(fixture._directory.path().join("kernel.sqlite")).unwrap();
    // One event per supporting row, in evidence-id order, at consecutive ordinals of the acceptance commit.
    let events = connection
        .prepare(
            "SELECT e.event_ordinal,e.event_kind,e.evidence_id,e.commit_seq
             FROM decision_events e JOIN decisions d ON d.decision_id=e.decision_id
             WHERE d.object_id=?1 ORDER BY e.event_ordinal",
        )
        .unwrap()
        .query_map([object_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let commit_seq = outcome.settlement.commit_seq;
    assert_eq!(
        events,
        ["support-a", "support-b", "support-c"]
            .into_iter()
            .zip(1..)
            .map(|(key, ordinal)| (
                ordinal,
                "memory_reviewer_support".to_string(),
                format!("evidence-{key}"),
                commit_seq
            ))
            .collect::<Vec<_>>()
    );
    // Each citation is its own change record of the decision.
    let appended: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM change_event
             WHERE commit_seq=?1 AND object_id=?2 AND change_kind='decision_event_append'",
            rusqlite::params![commit_seq, object_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(appended, 3);
}

#[test]
fn an_accepted_decision_serves_no_lower_than_the_support_it_cites_reads_today() {
    let fixture = Fixture::open();
    let subject = fixture.subject("job-1");
    let mut support = Vec::new();
    let mut members = Vec::new();
    for key in ["support-a", "support-b"] {
        let (evidence, digest) = fixture.ingest(key);
        members.push(PolicyUnionMember {
            kind: "temporary_capture".to_string(),
            id: evidence.clone(),
            revision: digest,
            owner_id: None,
            owner_revision: None,
        });
        support.push(evidence);
    }
    let staged = fixture.propose(
        "job-1",
        ProposalAction::Create,
        ProposalTarget::StagedCandidate {
            candidate_id: subject,
        },
        Some("bun builds the workspace"),
        &support,
        members,
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    let SettlementEffect::Created { object_id, .. } = &outcome.settlement.effect else {
        panic!("{:?}", outcome.settlement.effect)
    };
    assert!(fixture.served(object_id, Surface::ExplicitSearch));
    fixture.tighten("support-b");
    assert!(
        !fixture.served(object_id, Surface::ExplicitSearch),
        "a created decision is withheld once its second support row reads secret"
    );

    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-2",
        ProposalAction::Revise,
        "decision-a",
        &descriptor,
        Some("bun builds and tests the workspace"),
    );
    let outcome = fixture.accept("caller-2", &staged).unwrap();
    let SettlementEffect::Revised { object_id, .. } = &outcome.settlement.effect else {
        panic!("{:?}", outcome.settlement.effect)
    };
    assert!(fixture.served(object_id, Surface::ExplicitSearch));
    fixture.tighten("decision-a");
    assert!(
        !fixture.served(object_id, Surface::ExplicitSearch),
        "a revised successor is withheld once the support it cites reads secret"
    );
}

#[test]
fn revise_writes_a_fresh_successor_that_inherits_no_maturity_and_supersedes_the_target() {
    let fixture = Fixture::open();
    // An accepted ADR verifies `decision-a`, so it serves automatically before the revise.
    fixture.decision_of_kind("decision-adr", "adr_accepted");
    let mut adr = admission("decision-adr");
    adr.event.kind = EventKind::AcceptedAdr;
    fixture
        .store
        .commit(intent("adr"), |envelope| {
            envelope.record_admission(adr)?;
            Ok(String::new())
        })
        .unwrap();
    let mut verified = admission("decision-a");
    verified.source_class = Some(SourceClass::ModelInference);
    verified.taint_class = Some(TaintClass::AssistantInference);
    verified.event.kind = EventKind::Verify;
    verified.event.approval_object_id = Some("decision-adr".to_string());
    let descriptor = fixture.decision_admitted("decision-a", "architecture", verified);
    assert_eq!(fixture.maturity("decision-a"), "verified");
    assert!(fixture.served("decision-a", Surface::AutoInject));
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Revise,
        "decision-a",
        &descriptor,
        Some("bun builds and tests the workspace"),
    );
    let predecessor = fixture.state("decision-a");
    // The caller's scope is for a create; a revise keeps the predecessor's.
    let outcome = fixture
        .store
        .accept_review_proposal(&ProposalAcceptance {
            scope_id: Some("project:b".to_string()),
            ..fixture.acceptance(
                "caller-1",
                &staged,
                staged.run.clone(),
                staged.target.clone(),
            )
        })
        .unwrap();
    let SettlementEffect::Revised {
        object_id,
        predecessor_object_id,
        predecessor_source_revision,
        source_revision,
        predecessor_disposition,
        admission,
    } = &outcome.settlement.effect
    else {
        panic!("{:?}", outcome.settlement.effect)
    };
    assert_ne!(object_id, "decision-a", "a fresh successor, never a fold");
    assert_eq!(
        (
            predecessor_object_id.as_str(),
            *predecessor_source_revision,
            *source_revision,
            predecessor_disposition.as_str(),
        ),
        ("decision-a", 1, 2, "superseded")
    );
    let successor = fixture.state(object_id);
    assert_eq!(
        (
            successor.object.domain_id.as_str(),
            successor.object.source_kind.as_str(),
            successor.object.source_id.as_str(),
            successor.object.source_revision,
        ),
        (
            predecessor.object.domain_id.as_str(),
            predecessor.object.source_kind.as_str(),
            predecessor.object.source_id.as_str(),
            2
        ),
        "domain and source identity are preserved at the next revision"
    );
    let replaced = fixture.state("decision-a");
    assert!(replaced.object.invalidated_commit_seq.is_some());
    assert_eq!(
        replaced.object.superseded_by.as_deref(),
        Some(object_id.as_str())
    );
    assert_eq!(
        fixture.decision_row(object_id),
        (Some(SCOPE.to_string()), "architecture".to_string()),
        "scope and kind come from the predecessor"
    );
    // The predecessor was verified; the successor is admitted on its own as a model-inference candidate.
    assert_eq!(
        (
            admission.effective_maturity.as_str(),
            admission.disposition.as_str(),
            admission.visibility.as_str(),
            admission.source_class.as_str(),
            admission.taint_class.as_str(),
        ),
        (
            "candidate",
            "active",
            "explicit_labeled",
            "model_inference",
            "memory_classifier_inference"
        )
    );
    assert_eq!(fixture.maturity(object_id), "candidate");
    assert!(fixture.served(object_id, Surface::ExplicitSearch));
    assert!(!fixture.served(object_id, Surface::AutoInject));
    assert!(!fixture.served("decision-a", Surface::ExplicitSearch));
    let linked = fixture
        .store
        .linked_decision_evidence_within_budget(
            object_id,
            fixture.store.tip().unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
            &kernel::applicability::EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(
        linked
            .iter()
            .map(|link| link.evidence_id.as_str())
            .collect::<Vec<_>>(),
        vec![descriptor.1.as_str()],
        "the successor cites the proposal's support"
    );
    // A second revise over the superseded target is stale and is not redirected to the successor.
    let second = fixture.propose_over(
        "job-2",
        ProposalAction::Revise,
        "decision-a",
        &descriptor,
        Some("another revision"),
    );
    let tip = fixture.store.tip().unwrap();
    let successor_before = fixture.state(object_id);
    assert_eq!(
        fixture.accept("caller-2", &second),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Changed))
    );
    assert_eq!(
        (fixture.store.tip().unwrap(), fixture.state(object_id)),
        (tip, successor_before)
    );
}

#[test]
fn an_approval_is_not_revised_into_model_written_text() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision_of_kind("decision-adr", "adr_accepted");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Revise,
        "decision-adr",
        &descriptor,
        Some("rewritten"),
    );
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        fixture.accept("caller", &staged),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Changed))
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
}

#[test]
fn retire_runs_the_existing_retirement_and_reports_its_cascade() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retire,
        "decision-a",
        &descriptor,
        None,
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    assert_eq!(
        outcome.settlement.effect,
        SettlementEffect::Retired {
            object_id: "decision-a".to_string(),
            object_kind: "decision".to_string(),
            invalidated_commit_seq: outcome.settlement.commit_seq,
            granted_authority_before: false,
            demoted: 0,
            deferred: 0,
        }
    );
    assert_eq!(
        fixture.state("decision-a").object.invalidated_commit_seq,
        Some(outcome.settlement.commit_seq)
    );
}

#[test]
fn a_moved_target_another_project_a_released_hold_and_changed_evidence_refuse_and_write_nothing() {
    let fixture = Fixture::open();
    let refused = |staged: &Staged, run: MemoryReviewerHoldBinding, target: ProposalTarget| {
        let tip = fixture.store.tip().unwrap();
        let error = fixture
            .accept_as("caller", staged, run, target)
            .unwrap_err();
        assert_eq!(
            fixture.store.tip().unwrap(),
            tip,
            "a refusal commits nothing"
        );
        assert_eq!(
            fixture
                .store
                .review_settlement(&staged.run, &staged.reference)
                .unwrap(),
            None,
            "a refusal records no settlement"
        );
        error
    };
    let changed = AcceptanceError::Refused(AcceptanceRefusal::Changed);

    // The target changed after the proposal read it.
    let a = fixture.decision("decision-a");
    let staged = fixture.propose_over("job-a", ProposalAction::Retain, "decision-a", &a, None);
    fixture
        .store
        .commit(intent("touch-a"), |envelope| {
            envelope.append_decision_event(
                "decision-a-id",
                DecisionEventSpec {
                    event_kind: "note".to_string(),
                    payload: DecisionEventPayload {
                        summary: "touched".to_string(),
                    },
                    evidence_id: None,
                    recorded_at: 1,
                },
            )?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        refused(&staged, staged.run.clone(), staged.target.clone()),
        changed
    );

    // A caller target that is not the proposal's cannot redirect it.
    let b = fixture.decision("decision-b");
    let staged = fixture.propose_over("job-b", ProposalAction::Retain, "decision-b", &b, None);
    assert_eq!(
        refused(
            &staged,
            staged.run.clone(),
            fixture.memory_target("decision-a")
        ),
        changed
    );
    // Another project's run, or a run of another job, names no proposal: one answer for both.
    let unavailable = AcceptanceError::Refused(AcceptanceRefusal::Unavailable);
    let foreign = MemoryReviewerHoldBinding {
        project_digest: OTHER_PROJECT.to_string(),
        ..staged.run.clone()
    };
    assert_eq!(
        refused(&staged, foreign, staged.target.clone()),
        unavailable
    );
    // A run of another job derives another proposal id than the reference names: malformed input, refused before any row is read.
    let other_job = MemoryReviewerHoldBinding {
        subject: "job-unknown".to_string(),
        ..staged.run.clone()
    };
    assert_eq!(
        refused(&staged, other_job, staged.target.clone()),
        AcceptanceError::Refused(AcceptanceRefusal::Invalid)
    );
    // The review hold ended before acceptance.
    fixture.release_review_hold(&staged);
    assert_eq!(
        refused(&staged, staged.run.clone(), staged.target.clone()),
        AcceptanceError::Refused(AcceptanceRefusal::Expired)
    );

    // Disclosed evidence deleted under the live hold.
    let c = fixture.decision("decision-c");
    let staged = fixture.propose_over("job-c", ProposalAction::Retain, "decision-c", &c, None);
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-c"),
            identity: ArtifactDeletionIdentity::EvidenceId(c.1.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: fixture.now,
        })
        .unwrap();
    assert_eq!(
        refused(&staged, staged.run.clone(), staged.target.clone()),
        changed
    );

    // Disclosed evidence reclassified Secret.
    let d = fixture.decision("decision-d");
    let staged = fixture.propose_over("job-d", ProposalAction::Retain, "decision-d", &d, None);
    fixture.tighten("decision-d");
    assert_eq!(
        refused(&staged, staged.run.clone(), staged.target.clone()),
        changed
    );

    // The union's owner moved: the descriptor's decision was retired.
    let e = fixture.decision("decision-e");
    fixture.decision("decision-f");
    let staged = fixture.propose(
        "job-e",
        ProposalAction::Retain,
        fixture.memory_target("decision-f"),
        None,
        std::slice::from_ref(&e.1),
        vec![PolicyUnionMember {
            kind: "canonical_source".to_string(),
            id: e.0.clone(),
            revision: "1".to_string(),
            owner_id: Some("decision-e".to_string()),
            owner_revision: Some("1".to_string()),
        }],
    );
    fixture
        .store
        .commit(intent("retire-e"), |envelope| {
            envelope.retire_decision("decision-e")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        refused(&staged, staged.run.clone(), staged.target.clone()),
        changed
    );
}

#[test]
fn a_settlement_survives_a_reopen_and_replays_under_a_new_key() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retire,
        "decision-a",
        &descriptor,
        None,
    );
    let first = fixture.accept("caller-1", &staged).unwrap();
    // A fresh handle over the same files, as after a crash, finds the durable settlement.
    let Fixture {
        _directory: directory,
        store,
        now,
    } = fixture;
    drop(store);
    let fixture = Fixture {
        store: KernelStore::open(directory.path()).unwrap(),
        _directory: directory,
        now,
    };
    let again = fixture.accept("caller-2", &staged).unwrap();
    assert_eq!(
        (again.replayed, &again.settlement),
        (true, &first.settlement)
    );
}

/// Each action that targets a memory checks the exact target itself: a decision event after the proposal read the target refuses retain, revise, and retire alike.
#[test]
fn every_memory_action_refuses_a_target_that_moved_after_the_proposal() {
    let fixture = Fixture::open();
    for (index, (action, text)) in [
        (ProposalAction::Retain, None),
        (ProposalAction::Revise, Some("revised")),
        (ProposalAction::Retire, None),
    ]
    .into_iter()
    .enumerate()
    {
        let object = format!("decision-{index}");
        let descriptor = fixture.decision(&object);
        let staged =
            fixture.propose_over(&format!("job-{index}"), action, &object, &descriptor, text);
        fixture
            .store
            .commit(intent(&format!("touch-{index}")), |envelope| {
                envelope.append_decision_event(
                    &format!("{object}-id"),
                    DecisionEventSpec {
                        event_kind: "note".to_string(),
                        payload: DecisionEventPayload {
                            summary: "touched".to_string(),
                        },
                        evidence_id: None,
                        recorded_at: 1,
                    },
                )?;
                Ok(String::new())
            })
            .unwrap();
        let before = fixture.state(&object);
        let tip = fixture.store.tip().unwrap();
        assert_eq!(
            fixture.accept("caller", &staged),
            Err(AcceptanceError::Refused(AcceptanceRefusal::Changed)),
            "{action:?}"
        );
        assert_eq!(
            (fixture.store.tip().unwrap(), fixture.state(&object)),
            (tip, before)
        );
    }
}

/// A target or union owner that is live but no longer served, here quarantined by admission, refuses like a retired one.
#[test]
fn a_quarantined_owner_refuses_without_being_invalidated() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retain,
        "decision-a",
        &descriptor,
        None,
    );
    let mut quarantine = admission("decision-a");
    quarantine.event.kind = EventKind::Quarantine;
    fixture
        .store
        .commit(intent("quarantine-a"), |envelope| {
            envelope.record_admission(quarantine)?;
            Ok(String::new())
        })
        .unwrap();
    assert!(
        fixture
            .state("decision-a")
            .object
            .invalidated_commit_seq
            .is_none()
    );
    let tip = fixture.store.tip().unwrap();
    assert_eq!(
        fixture.accept("caller", &staged),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Changed))
    );
    assert_eq!(fixture.store.tip().unwrap(), tip);
}

#[test]
fn create_needs_a_scope_of_the_runs_project_and_no_change_on_a_subject_writes_nothing() {
    let fixture = Fixture::open();
    let support = fixture.ingest("support").0;
    let propose = |job: &str, action, text| {
        let subject = fixture.subject(job);
        fixture.propose(
            job,
            action,
            ProposalTarget::StagedCandidate {
                candidate_id: subject,
            },
            text,
            std::slice::from_ref(&support),
            vec![],
        )
    };
    let staged = propose("job-1", ProposalAction::Create, Some("created"));
    let tip = fixture.store.tip().unwrap();
    for (scope, refusal) in [
        (None, AcceptanceRefusal::Invalid),
        (
            Some("project:b".to_string()),
            AcceptanceRefusal::Unavailable,
        ),
        (
            Some("scope-missing".to_string()),
            AcceptanceRefusal::Unavailable,
        ),
    ] {
        let acceptance = ProposalAcceptance {
            scope_id: scope,
            ..fixture.acceptance("caller", &staged, staged.run.clone(), staged.target.clone())
        };
        assert_eq!(
            fixture.store.accept_review_proposal(&acceptance),
            Err(AcceptanceError::Refused(refusal))
        );
        assert_eq!(fixture.store.tip().unwrap(), tip);
    }
    let unchanged = propose("job-2", ProposalAction::NoChange, None);
    let outcome = fixture.accept("caller-2", &unchanged).unwrap();
    assert_eq!(outcome.settlement.effect, SettlementEffect::Unchanged);
    // A subject row that is gone no longer anchors the proposal.
    let gone = propose("job-3", ProposalAction::NoChange, None);
    fixture.execute("DELETE FROM extraction_runs WHERE extraction_run_id='subject-run-job-3'");
    assert_eq!(
        fixture.accept("caller-3", &gone),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Changed))
    );
}

/// The queue deadline is judged against the selection time: a selection at or after it is refused, while a settled proposal replays whatever the selection time.
#[test]
fn a_selection_past_the_queue_deadline_is_expired_but_a_settlement_still_replays() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retain,
        "decision-a",
        &descriptor,
        None,
    );
    let deadline = fixture.now + DAY_MS - 1_000;
    let late = |key: &str| ProposalAcceptance {
        selected_at: deadline,
        ..fixture.acceptance(key, &staged, staged.run.clone(), staged.target.clone())
    };
    assert_eq!(
        fixture.store.accept_review_proposal(&late("caller-late")),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Expired))
    );
    let early = ProposalAcceptance {
        selected_at: fixture.now - 10_000,
        ..fixture.acceptance(
            "caller-early",
            &staged,
            staged.run.clone(),
            staged.target.clone(),
        )
    };
    assert_eq!(
        fixture.store.accept_review_proposal(&early),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Invalid)),
        "a selection before the row was sealed cannot be a receipt's"
    );
    let settled = fixture.accept("caller-1", &staged).unwrap();
    let replay = fixture
        .store
        .accept_review_proposal(&late("caller-2"))
        .unwrap();
    assert_eq!(
        (replay.replayed, replay.settlement),
        (true, settled.settlement)
    );
}

/// After acceptance, the review hold's end, the staged row's cleanup, and a purge of the cited support leave the decision and its settlement in place, and the revised decision's original evidence stays its own.
#[test]
fn cleanup_and_purge_after_acceptance_keep_the_settlement_and_the_originals() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision("decision-a");
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Revise,
        "decision-a",
        &descriptor,
        Some("revised text"),
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    let SettlementEffect::Revised { object_id, .. } = &outcome.settlement.effect else {
        panic!("{:?}", outcome.settlement.effect)
    };
    // The predecessor's own descriptor is an independent original: the revise neither retired nor rewrote it.
    assert!(
        fixture
            .state(&descriptor.0)
            .object
            .invalidated_commit_seq
            .is_none()
    );
    fixture.release_review_hold(&staged);
    let run = provisional_result_identity("job-1", 1).extraction_run_id;
    fixture.execute(&format!(
        "DELETE FROM extraction_runs WHERE extraction_run_id='{run}'"
    ));
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("purge-support"),
            identity: ArtifactDeletionIdentity::EvidenceId(descriptor.1.clone()),
            kind: ArtifactDeletionKind::Purge,
            operator_id: Some("operator".to_string()),
            target_locator: Some("incident://purge".to_string()),
            reason: Some("test".to_string()),
            deleted_at: fixture.now,
        })
        .unwrap();
    let replay = fixture.accept("caller-2", &staged).unwrap();
    assert_eq!(
        (replay.replayed, &replay.settlement),
        (true, &outcome.settlement)
    );
    assert!(
        fixture
            .state(object_id)
            .object
            .invalidated_commit_seq
            .is_none()
    );
    assert!(fixture.served(object_id, Surface::ExplicitSearch));
    let linked = fixture
        .store
        .linked_decision_evidence_within_budget(
            object_id,
            fixture.store.tip().unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
            &kernel::applicability::EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(
        linked
            .iter()
            .map(|link| (link.evidence_id.as_str(), link.live))
            .collect::<Vec<_>>(),
        vec![(descriptor.1.as_str(), false)],
        "the citation stays, naming purged evidence"
    );
}

/// Two callers accepting one proposal at once settle it once: one applies, the other replays the same settlement.
#[test]
fn concurrent_acceptances_of_one_proposal_settle_it_once() {
    let fixture = Fixture::open();
    let subject = fixture.subject("job-1");
    let support = fixture.ingest("support").0;
    let staged = fixture.propose(
        "job-1",
        ProposalAction::Create,
        ProposalTarget::StagedCandidate {
            candidate_id: subject,
        },
        Some("created once"),
        std::slice::from_ref(&support),
        vec![],
    );
    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = ["caller-a", "caller-b"]
            .into_iter()
            .map(|key| {
                let fixture = &fixture;
                let staged = &staged;
                scope.spawn(move || fixture.accept(key, staged).unwrap())
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    assert_eq!(
        outcomes.iter().filter(|outcome| !outcome.replayed).count(),
        1,
        "exactly one acceptance applies"
    );
    assert_eq!(outcomes[0].settlement, outcomes[1].settlement);
    let decisions: i64 =
        rusqlite::Connection::open(fixture._directory.path().join("kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM object_registry WHERE source_id=?1",
                [staged.reference.candidate_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(decisions, 1, "one decision for the proposal");
}

/// A caller key replays only the acceptance it committed: the same key and digest for another proposal conflicts.
#[test]
fn a_caller_key_reused_for_another_proposal_conflicts() {
    let fixture = Fixture::open();
    let a = fixture.decision("decision-a");
    let b = fixture.decision("decision-b");
    let first = fixture.propose_over("job-a", ProposalAction::Retain, "decision-a", &a, None);
    let second = fixture.propose_over("job-b", ProposalAction::Retain, "decision-b", &b, None);
    fixture.accept("caller", &first).unwrap();
    assert_eq!(
        fixture.accept("caller", &second),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Conflict))
    );
    assert_eq!(
        fixture
            .store
            .review_settlement(&second.run, &second.reference)
            .unwrap(),
        None
    );
    let reserved = ProposalAcceptance {
        intent: CommitIntent {
            producer: kernel::SETTLEMENT_PRODUCER.to_string(),
            ..intent("reserved")
        },
        ..fixture.acceptance("unused", &second, second.run.clone(), second.target.clone())
    };
    assert_eq!(
        fixture.store.accept_review_proposal(&reserved),
        Err(AcceptanceError::Refused(AcceptanceRefusal::Invalid)),
        "a caller cannot claim the settlement producer"
    );
}

/// Retiring a decision that grants authority runs the existing cascade, and the settlement reports what it did.
#[test]
fn retiring_an_approval_reports_the_dependents_its_cascade_demoted() {
    let fixture = Fixture::open();
    let descriptor = fixture.decision_of_kind("decision-a", "adr_accepted");
    // An accepted ADR makes `decision-a` an approval.
    let mut adr = admission("decision-a");
    adr.event.kind = EventKind::AcceptedAdr;
    fixture
        .store
        .commit(intent("adr-a"), |envelope| {
            envelope.record_admission(adr)?;
            Ok(String::new())
        })
        .unwrap();
    fixture
        .store
        .commit(intent("decision-b"), |envelope| {
            envelope.insert_decision(DecisionSpec {
                decision_id: "decision-b-id".to_string(),
                object_id: "decision-b".to_string(),
                domain_id: DOMAIN.to_string(),
                proposition_id: None,
                scope_id: Some(SCOPE.to_string()),
                anchor_id: None,
                evidence_id: None,
                decision_kind: "architecture".to_string(),
                payload: DecisionPayload {
                    summary: "summary decision-b".to_string(),
                    rationale: "rationale decision-b".to_string(),
                },
                source_kind: "repo".to_string(),
                source_id: "src/decision-b".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let mut verified = admission("decision-b");
    verified.source_class = Some(SourceClass::ModelInference);
    verified.taint_class = Some(TaintClass::AssistantInference);
    verified.event.kind = EventKind::Verify;
    verified.event.approval_object_id = Some("decision-a".to_string());
    fixture
        .store
        .commit(intent("verify-b"), |envelope| {
            envelope.record_admission(verified)?;
            Ok(String::new())
        })
        .unwrap();
    let staged = fixture.propose_over(
        "job-1",
        ProposalAction::Retire,
        "decision-a",
        &descriptor,
        None,
    );
    let outcome = fixture.accept("caller-1", &staged).unwrap();
    let SettlementEffect::Retired {
        granted_authority_before,
        demoted,
        deferred,
        ..
    } = outcome.settlement.effect
    else {
        panic!("{:?}", outcome.settlement.effect)
    };
    assert_eq!((granted_authority_before, demoted, deferred), (true, 1, 0));
}
