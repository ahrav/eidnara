//! The attestation write set over a fixture store holding an anchored, document-citing model
//! decision and a git checkout whose commit holds the cited document: the `Match` envelope and its
//! companion rows, the `Mismatch` stale path, replay and conflict, the recheck after the model
//! decision changes, the pre-commit write-set check, a degraded witness, a rollback after the
//! admission, and a reopened store.

use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::process::Command;

use daemon::attestation_verifier::attest::{
    CITATION_EVENT_KIND, Identity, Outcome, PRODUCER, Skip, WITNESS_DETAIL_VERSION, WriteSet,
    attest,
};
use daemon::attestation_verifier::classify::{
    BoundRoot, Class, Classification, Unverifiable, VERIFIER_SOURCE_KIND, VerifierInput, classify,
};
use daemon::attestation_verifier::{Evaluation, Reason, Verdict, verify};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, AnchorSpec, AnchorValue, ArtifactIngestRequest, CausalClass,
    ClaimFactBounds, ClaimFacts, CommitIntent, DecisionEventPayload, DecisionEventSpec,
    DecisionPayload, DecisionSpec, Disposition, DomainSpec, EventKind, KernelError, KernelStore,
    Maturity, Outcome as AdmissionOutcome, ParentReference, ProviderEgress, Sensitivity,
    ServedStanding, SourceClass, SourceDescriptorPolicy, SourceDescriptorRequest, Surface,
    SurfaceVisibility, TaintClass,
};
use sha2::{Digest as _, Sha256};

const REPOSITORY: &str = "git:0000000000000000000000000000000000000001";
const DOC_PATH: &str = "ARCHITECTURE.md";
const DOC: &str = "# Architecture\n\nThe daemon owns the store.\n";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "attestation-verifier-attest-test".into(),
        operation_key: key.into(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".into(),
        cause: "proof".into(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A repository whose one commit holds `DOC` at `DOC_PATH`.
struct Checkout {
    dir: tempfile::TempDir,
    head: String,
    blob: String,
}

impl Checkout {
    fn new(text: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(dir.path().join(DOC_PATH), text).unwrap();
        git(dir.path(), &["add", DOC_PATH]);
        git(dir.path(), &["commit", "--quiet", "-m", "doc"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        let blob = git(dir.path(), &["rev-parse", &format!("HEAD:{DOC_PATH}")]);
        Self { dir, head, blob }
    }

    fn roots(&self) -> [BoundRoot<'_>; 1] {
        [BoundRoot {
            root: self.dir.path(),
            repository_id: REPOSITORY,
        }]
    }
}

struct Evidence {
    id: String,
}

struct Store {
    root: tempfile::TempDir,
    kernel: KernelStore,
}

impl Store {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path()).unwrap();
        kernel
            .commit(intent("domains"), |envelope| {
                for domain in ["memory", "source"] {
                    envelope.insert_domain(DomainSpec {
                        domain_id: domain.into(),
                        object_id: format!("{domain}-domain"),
                        name: domain.into(),
                        source_kind: "test".into(),
                        source_id: domain.into(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                }
                Ok(String::new())
            })
            .unwrap();
        Self { root, kernel }
    }

    /// Drops the store and opens its data home again.
    fn reopen(self) -> Self {
        let Self { root, kernel } = self;
        drop(kernel);
        let kernel = KernelStore::open(root.path()).unwrap();
        Self { root, kernel }
    }

    fn sqlite(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.sqlite().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn tip(&self) -> i64 {
        self.kernel.tip().unwrap()
    }

    /// Retains `text` as `Sensitive` evidence.
    fn retain(&self, key: &str, text: &str) -> kernel::ArtifactHandle {
        self.kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{key}"),
                object_id: format!("srcev-object:{key}"),
                object_kind: "evidence".into(),
                domain_id: "source".into(),
                source_kind: "test".into(),
                source_id: format!("source/{key}"),
                source_revision: 1,
                media_type: "text/plain".into(),
                retention_class: "canonical".into(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap()
    }

    /// Retains `text` and publishes the document descriptor citing it.
    fn document(&self, key: &str, text: &str) -> (Evidence, String) {
        let handle = self.retain(key, text);
        let identity = [
            ("project_id", "proj-a"),
            ("repository_id", REPOSITORY),
            ("relative_path", DOC_PATH),
        ];
        let descriptor = self
            .kernel
            .commit(intent(&format!("descriptor-{key}")), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: OccurrenceClass::RepositoryDocuments.code(),
                            identity: &identity,
                            revision: "1",
                            representation: "text",
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: "source",
                        scope_id: None,
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Sensitive,
                        observed_at: 1,
                        expected_head: None,
                        outcome: None,
                    })
                    .map_err(|_| KernelError::InvalidInput)?;
                Ok(outcome.object_id)
            })
            .unwrap()
            .result;
        (
            Evidence {
                id: handle.evidence_id,
            },
            descriptor,
        )
    }

    /// A model decision anchored `reachable_from` `oid`, citing `evidence` through a decision event
    /// or, with `row`, through its own `evidence_id`.
    fn model(&self, name: &str, oid: &str, evidence: &Evidence, row: bool) -> String {
        let object_id = format!("mem_{name}");
        let decision_id = format!("dec_{name}");
        self.kernel
            .commit(intent(&format!("decision-{name}")), |envelope| {
                let anchor_id = format!("anchor_{name}");
                envelope.insert_anchor(AnchorSpec {
                    anchor_id: anchor_id.clone(),
                    object_id: format!("anchorobj_{name}"),
                    domain_id: "memory".into(),
                    source_kind: "assistant".into(),
                    source_id: "memory_capture".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    value: AnchorValue::ReachableFrom {
                        oid: oid.into(),
                        captures: Vec::new(),
                    },
                })?;
                envelope.insert_decision(model_spec(name, Some(anchor_id), evidence, row, 1))?;
                if !row {
                    envelope.append_decision_event(
                        &decision_id,
                        DecisionEventSpec {
                            event_kind: "memory_capture_citation".into(),
                            payload: DecisionEventPayload {
                                summary: "cites the document".into(),
                            },
                            evidence_id: Some(evidence.id.clone()),
                            recorded_at: 1,
                        },
                    )?;
                }
                model_admission(envelope, &object_id, EventKind::Other)?;
                Ok(String::new())
            })
            .unwrap();
        object_id
    }

    fn classification(&self, object_id: &str, checkout: &Checkout) -> Classification {
        classify(
            &self.kernel,
            object_id,
            self.kernel.capture_commit_read_target().unwrap(),
            &checkout.roots(),
            &EvalBudget::unbounded(),
        )
        .unwrap()
    }

    fn facts(&self, object_id: &str) -> ClaimFacts {
        let bounds = ClaimFactBounds {
            max_claims: NonZeroUsize::MIN,
            max_causal_payload_bytes: NonZeroU64::new(1 << 16).unwrap(),
        };
        self.kernel
            .claim_facts_as_of(&[object_id.to_owned()], self.tip(), bounds)
            .unwrap()
            .claims
            .pop()
            .unwrap()
    }

    fn visible(&self, surface: Surface) -> Vec<String> {
        self.kernel
            .visible_as_of(surface, self.tip())
            .unwrap()
            .rows
            .into_iter()
            .map(|row| row.object.object_id)
            .collect()
    }

    fn verifier_rows(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM object_registry WHERE source_kind='verifier'")
    }

    /// Rows any attestation write set adds: verifier objects, causality records, citation events,
    /// and admissions of verifier subjects.
    fn write_set_rows(&self) -> i64 {
        self.verifier_rows()
            + self.count(
                "SELECT COUNT(*) FROM object_registry WHERE object_id GLOB 'claimcauseobj:*'",
            )
            + self.count(&format!(
                "SELECT COUNT(*) FROM decision_events WHERE event_kind='{CITATION_EVENT_KIND}'"
            ))
            + self.count("SELECT COUNT(*) FROM admission_decisions WHERE source_kind='verifier'")
    }

    fn admissions_of(&self, object_id: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM admission_decisions WHERE subject_object_id='{object_id}'"
        ))
    }
}

fn model_spec(
    name: &str,
    anchor_id: Option<String>,
    evidence: &Evidence,
    row: bool,
    revision: i64,
) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("dec_{name}"),
        object_id: format!("mem_{name}"),
        domain_id: "memory".into(),
        proposition_id: None,
        scope_id: None,
        anchor_id,
        evidence_id: row.then(|| evidence.id.clone()),
        decision_kind: "ARCHITECTURE".into(),
        payload: DecisionPayload {
            summary: format!("The daemon owns the store ({name})."),
            rationale: "{}".into(),
        },
        source_kind: "assistant".into(),
        source_id: "memory_capture".into(),
        source_revision: revision,
        sensitivity: Sensitivity::Normal,
    }
}

fn model_admission(
    envelope: &mut kernel::Envelope<'_>,
    object_id: &str,
    kind: EventKind,
) -> Result<kernel::AdmissionDecision, KernelError> {
    envelope.record_admission(AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.into()),
        source_class: Some(SourceClass::ModelInference),
        taint_class: Some(TaintClass::AssistantInference),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".into(),
        },
    })
}

fn input(classification: &Classification) -> &VerifierInput {
    match &classification.class {
        Class::Verifiable(input) => input,
        other => panic!("expected a verifiable decision, found {other:?}"),
    }
}

fn evaluate(classification: &Classification) -> Evaluation {
    let input = input(classification);
    verify(
        input.root.as_ref().unwrap(),
        &input.claim,
        &EvalBudget::unbounded(),
    )
}

fn attest_now(store: &Store, classification: &Classification, evaluation: &Evaluation) -> Outcome {
    attest(
        &store.kernel,
        classification,
        evaluation,
        &EvalBudget::unbounded(),
    )
}

fn served(facts: &ClaimFacts) -> (Sensitivity, SurfaceVisibility) {
    match &facts.served {
        ServedStanding::Served(served) => (served.sensitivity, served.auto_inject),
        other => panic!("expected a served claim, found {other:?}"),
    }
}

/// The witness observation's stored detail.
fn witness_detail(store: &Store, object_id: &str) -> serde_json::Value {
    let payload: Vec<u8> = store
        .sqlite()
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [object_id],
            |row| row.get(0),
        )
        .unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap()
}

/// Edits a write set given the witness detail and another evidence id.
type Mutation = fn(&mut WriteSet, &str, &str);

fn digest_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// AC6, AC7, F14: one envelope writes the attestation, its witness, the witness admission at
/// `Verified`, the causality record, and the citation event; the model decision is unchanged; the
/// attestation serves at the model decision's sensitivity.
#[test]
fn a_match_commits_the_attestation_with_every_companion_row() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, descriptor) = store.document("doc", DOC);
    let model = store.model("event", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Match);
    let model_before = store.facts(&model);

    let outcome = attest_now(&store, &classification, &evaluation);
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");

    let ids = Identity::of(&model, &checkout.head);
    let attestation = store.facts(&ids.object_id);
    assert_eq!(attestation.object.source_kind, VERIFIER_SOURCE_KIND);
    assert_eq!(attestation.object.source_id, ids.source_id);
    assert_eq!(attestation.object.source_revision, 1);
    assert_eq!(attestation.decision.decision_id, ids.decision_id);
    assert_eq!(
        attestation.decision.anchor_id,
        model_before.decision.anchor_id
    );
    assert_eq!(attestation.decision.evidence_id, None);
    let admission = attestation.own_admission.clone().unwrap();
    assert_eq!(
        (admission.source_class, admission.taint_class),
        (SourceClass::TrustedLocalCode, TaintClass::CurrentCode)
    );
    assert_eq!(admission.event_kind, EventKind::CodeObserved);
    assert_eq!(admission.outcome, AdmissionOutcome::Admit);
    assert_eq!(admission.effective_maturity, Maturity::Verified);
    assert_eq!(admission.disposition, Disposition::Active);
    assert_eq!(
        admission.trigger_object_id.as_deref(),
        Some(ids.witness_object_id.as_str())
    );
    assert_eq!(admission.evidence_id, None);
    assert_eq!(
        attestation.causality,
        CausalClass::DerivedReinjection {
            parents: vec![ParentReference {
                object_id: model.clone(),
                revision: 1,
            }],
        }
    );
    // The one citation event names the compared evidence, the witness detail the same row.
    let cited = store
        .kernel
        .linked_decision_evidence_within_budget(
            &ids.object_id,
            store.tip(),
            NonZeroUsize::new(4).unwrap(),
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(cited.len(), 1);
    assert_eq!(cited[0].evidence_id, evidence.id);
    assert_eq!(
        store.count(&format!(
            "SELECT COUNT(*) FROM decision_events WHERE decision_id='{}'",
            ids.decision_id
        )),
        1
    );
    let detail = witness_detail(&store, &ids.witness_object_id);
    assert_eq!(detail["witness_version"], WITNESS_DETAIL_VERSION);
    assert_eq!(detail["evidence_id"], evidence.id.as_str());
    assert_eq!(detail["artifact_digest"], digest_hex(DOC).as_str());
    assert_eq!(detail["commit_oid"], checkout.head.as_str());
    assert_eq!(detail["blob_oid"], checkout.blob.as_str());
    assert_eq!(detail["relative_path"], DOC_PATH);
    assert_eq!(detail["descriptor_object_id"], descriptor.as_str());
    let (kind, witness_evidence, source_kind, source_id, revision): (
        String,
        Option<String>,
        String,
        String,
        i64,
    ) = store
        .sqlite()
        .query_row(
            "SELECT b.observation_kind,b.evidence_id,o.source_kind,o.source_id,o.source_revision
               FROM observations b JOIN object_registry o ON o.object_id=b.object_id
              WHERE b.object_id=?1",
            [&ids.witness_object_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(kind, "code_present");
    assert_eq!(
        witness_evidence, None,
        "the witness cites no evidence by FK"
    );
    assert_eq!(
        (source_kind.as_str(), source_id.as_str(), revision),
        (VERIFIER_SOURCE_KIND, ids.source_id.as_str(), 1)
    );

    // F14: the attestation serves at the model decision's sensitivity and is visible on
    // `AutoInject`; the model decision is unchanged and stays below the automatic ceiling.
    let model_after = store.facts(&model);
    assert_eq!(served(&attestation).0, served(&model_after).0);
    assert_eq!(served(&attestation).0, Sensitivity::Normal);
    assert_eq!(served(&attestation).1, SurfaceVisibility::Visible);
    assert_eq!(model_after.object, model_before.object);
    assert_eq!(model_after.own_admission, model_before.own_admission);
    assert_eq!(
        model_after.own_admission.unwrap().effective_maturity,
        Maturity::Candidate
    );
    let explicit = store.visible(Surface::ExplicitSearch);
    assert!(explicit.contains(&model) && explicit.contains(&ids.object_id));
    let automatic = store.visible(Surface::AutoInject);
    assert!(automatic.contains(&ids.object_id));
    assert!(!automatic.contains(&model));

    // AC15: the attestation is the verifier's own output and enters no write set.
    let own = store.classification(&ids.object_id, &checkout);
    assert_eq!(own.class, Class::Unverifiable(Unverifiable::SelfSource));
    let tip = store.tip();
    assert_eq!(
        attest_now(&store, &own, &evaluation),
        Outcome::Skipped(Skip::NotVerifiable)
    );
    assert_eq!(store.tip(), tip);
}

/// F14: a model decision citing the `Sensitive` document row by its own `evidence_id` serves
/// `Sensitive`, and its attestation serves at that same class, hidden on `AutoInject`; the
/// attestation's rows still cite the evidence only through the event and the detail.
#[test]
fn the_attestation_serves_at_the_model_decisions_sensitivity() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("row", &checkout.head, &evidence, true);
    let classification = store.classification(&model, &checkout);
    let outcome = attest_now(&store, &classification, &evaluate(&classification));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let ids = Identity::of(&model, &checkout.head);
    let attestation = store.facts(&ids.object_id);
    assert_eq!(served(&attestation).0, served(&store.facts(&model)).0);
    assert_eq!(
        served(&attestation),
        (Sensitivity::Sensitive, SurfaceVisibility::Hidden)
    );
    assert_eq!(attestation.decision.evidence_id, None);
    assert_eq!(
        attestation.own_admission.unwrap().effective_maturity,
        Maturity::Verified
    );
}

/// AC8: a mismatch writes exactly one `MarkStale` on the model decision under its stored classes,
/// naming the oid and both digests, and no verifier object; a second run writes nothing.
#[test]
fn a_mismatch_marks_the_model_decision_stale_once() {
    let store = Store::open();
    let committed = "# Architecture\n\nThe plugin owns the store.\n";
    let checkout = Checkout::new(committed);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("stale", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Mismatch);
    let admissions = store.admissions_of(&model);

    let outcome = attest_now(&store, &classification, &evaluation);
    assert!(matches!(outcome, Outcome::StaleCommitted(_)), "{outcome:?}");
    assert_eq!(store.admissions_of(&model), admissions + 1);
    let stale = store.facts(&model).own_admission.unwrap();
    assert_eq!(stale.event_kind, EventKind::MarkStale);
    assert_eq!(stale.disposition, Disposition::Stale);
    assert_eq!(
        (stale.source_class, stale.taint_class),
        (SourceClass::ModelInference, TaintClass::AssistantInference)
    );
    assert_eq!(stale.evidence_id, None);
    let reason: String = store
        .sqlite()
        .query_row(
            "SELECT reason FROM admission_decisions WHERE subject_object_id=?1
              ORDER BY commit_seq DESC LIMIT 1",
            [&model],
            |row| row.get(0),
        )
        .unwrap();
    assert!(reason.contains(&checkout.head), "{reason}");
    assert!(reason.contains(&digest_hex(DOC)), "{reason}");
    assert!(reason.contains(&checkout.blob), "{reason}");
    assert_eq!(store.verifier_rows(), 0);
    let producer: String = store
        .sqlite()
        .query_row(
            "SELECT operation_key FROM operation_receipts WHERE producer=?1",
            [PRODUCER],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(producer, format!("stale:{model}:{}", checkout.head));

    // A second run: the stale decision is outside the input, and the old classification replays.
    let tip = store.tip();
    let again = store.classification(&model, &checkout);
    assert_eq!(again.class, Class::NotInput);
    assert_eq!(
        attest_now(&store, &again, &evaluation),
        Outcome::Skipped(Skip::NotVerifiable)
    );
    assert!(matches!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Replayed(_)
    ));
    assert_eq!(store.tip(), tip);
    assert_eq!(store.admissions_of(&model), admissions + 1);
}

/// A model decision already `Stale` when its mismatch commits is skipped before any write.
#[test]
fn a_mismatch_on_a_decision_already_stale_is_skipped() {
    let store = Store::open();
    let checkout = Checkout::new("# Other\n");
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("prior", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Mismatch);
    store
        .kernel
        .commit(intent("stale-first"), |envelope| {
            model_admission(envelope, &model, EventKind::MarkStale)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip();
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Skipped(Skip::AlreadyStale)
    );
    assert_eq!(store.tip(), tip);
}

/// An `Unavailable` verdict writes nothing.
#[test]
fn an_unavailable_verdict_writes_nothing() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("unbound", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let tip = store.tip();
    for reason in [Reason::RepositoryUnbound, Reason::PathAbsent] {
        let evaluation = Evaluation {
            verdict: Verdict::Unavailable(reason),
            bytes_read: 0,
            blob_oid: Some(checkout.blob.clone()),
        };
        assert_eq!(
            attest_now(&store, &classification, &evaluation),
            Outcome::Skipped(Skip::Unavailable)
        );
    }
    assert_eq!(store.tip(), tip);
}

/// AC9: a witness at another revision, a detail or citation naming another evidence row, or a
/// witness kind other than the document's is refused before the commit.
#[test]
fn a_write_set_citing_other_evidence_or_another_revision_is_refused_before_the_commit() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let other = store.retain("other", "# Other\n");
    let model = store.model("refused", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let budget = EvalBudget::unbounded();
    let write_set = WriteSet::new(
        &store.kernel,
        classification.target,
        input(&classification),
        &checkout.blob,
        &budget,
    )
    .unwrap();
    assert!(write_set.check());
    let detail = write_set.witness.payload.detail.clone().unwrap();
    let mutations: [(&str, Mutation); 5] = [
        ("revision", |set, _, _| set.witness.source_revision = 2),
        ("source id", |set, _, _| {
            set.witness.source_id = "shared".into()
        }),
        ("detail", |set, detail, other| {
            set.witness.payload.detail = Some(detail.replace("srcev:doc", other));
        }),
        ("citation", |set, _, other| {
            set.citation.evidence_id = Some(other.into());
        }),
        ("kind", |set, _, _| {
            set.witness.observation_kind = "config_present".into();
        }),
    ];
    let tip = store.tip();
    for (name, mutate) in mutations {
        let mut mutated = write_set.clone();
        mutate(&mut mutated, &detail, &other.evidence_id);
        assert!(!mutated.check(), "{name}");
        assert_eq!(
            mutated.commit(&store.kernel, &budget),
            Outcome::InvalidWriteSet,
            "{name}"
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
    assert!(matches!(
        write_set.commit(&store.kernel, &budget),
        Outcome::Committed(_)
    ));
}

/// AC9: a witness admission returned below `Verified` aborts the envelope and leaves no verifier
/// row. The fault rewrites the witness's observation kind as the Kernel inserts it, so its
/// trigger fails validation and the event is admitted as `Other`.
#[test]
fn a_witness_admitted_below_verified_aborts_the_envelope() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("degraded", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    store
        .sqlite()
        .execute_batch(
            "CREATE TRIGGER degrade_witness AFTER INSERT ON observations
               WHEN NEW.object_id GLOB 'verwitnessobj:*'
             BEGIN
               UPDATE observations SET observation_kind='code_absent'
                WHERE object_id=NEW.object_id;
             END;",
        )
        .unwrap();
    let model_before = store.facts(&model);
    let tip = store.tip();
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::WitnessDegraded
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
    assert_eq!(store.facts(&model), model_before);

    store
        .sqlite()
        .execute_batch("DROP TRIGGER degrade_witness;")
        .unwrap();
    assert!(matches!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Committed(_)
    ));
}

/// F10: the same key and verdict replay the receipt with one attestation; the same key with
/// another request digest, and the opposite verdict for the same decision and oid, conflict.
#[test]
fn a_replay_returns_the_receipt_and_a_different_verdict_conflicts() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("replay", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let Outcome::Committed(first) = attest_now(&store, &classification, &evaluation) else {
        panic!("the first attestation commits");
    };
    let Outcome::Replayed(replayed) = attest_now(&store, &classification, &evaluation) else {
        panic!("the repeat replays");
    };
    assert_eq!(replayed.commit_seq, first.commit_seq);
    let attestations = "SELECT COUNT(*) FROM decisions d
        JOIN object_registry o ON o.object_id=d.object_id WHERE o.source_kind='verifier'";
    assert_eq!(store.count(attestations), 1);
    let tip = store.tip();
    let mismatch = Evaluation {
        verdict: Verdict::Mismatch,
        ..evaluation.clone()
    };
    assert_eq!(
        attest_now(&store, &classification, &mismatch),
        Outcome::Conflict
    );
    let other_blob = Evaluation {
        blob_oid: Some("0".repeat(40)),
        ..evaluation
    };
    assert_eq!(
        attest_now(&store, &classification, &other_blob),
        Outcome::Conflict
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(
        store.facts(&model).own_admission.unwrap().disposition,
        Disposition::Active
    );
}

/// F10, opposite direction: after a `MarkStale`, a `Match` for the same decision and oid conflicts.
#[test]
fn a_match_after_the_stale_path_for_the_same_oid_conflicts() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("flip", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let mismatch = Evaluation {
        verdict: Verdict::Mismatch,
        ..evaluation.clone()
    };
    assert!(matches!(
        attest_now(&store, &classification, &mismatch),
        Outcome::StaleCommitted(_)
    ));
    let tip = store.tip();
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Conflict
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(store.verifier_rows(), 0);
}

/// F2, F3: a model decision superseded, retired, or re-disposed after its classification, or an
/// anchor or descriptor retired in that window, aborts the set with no verifier row.
#[test]
fn a_model_decision_changed_after_classification_aborts_the_set() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, descriptor) = store.document("doc", DOC);
    let names = ["superseded", "retired", "disputed", "stale", "unanchored"];
    let classified: Vec<_> = names
        .iter()
        .map(|name| {
            let model = store.model(name, &checkout.head, &evidence, false);
            let classification = store.classification(&model, &checkout);
            let evaluation = evaluate(&classification);
            (model, classification, evaluation)
        })
        .collect();
    store
        .kernel
        .commit(intent("change"), |envelope| {
            envelope.supersede_decision(
                "mem_superseded",
                DecisionSpec {
                    decision_id: "dec_superseded_2".into(),
                    object_id: "mem_superseded_2".into(),
                    ..model_spec(
                        "superseded",
                        Some("anchor_superseded".into()),
                        &evidence,
                        false,
                        2,
                    )
                },
            )?;
            model_admission(envelope, "mem_superseded_2", EventKind::Other)?;
            envelope.retire_decision("mem_retired")?;
            model_admission(envelope, "mem_disputed", EventKind::Quarantine)?;
            model_admission(envelope, "mem_stale", EventKind::MarkStale)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip();
    store
        .sqlite()
        .execute(
            "UPDATE anchors SET invalidated_commit_seq=?1 WHERE anchor_id='anchor_unanchored'",
            [tip],
        )
        .unwrap();
    for (model, classification, evaluation) in &classified {
        assert_eq!(
            attest_now(&store, classification, evaluation),
            Outcome::Changed,
            "{model}"
        );
    }
    // A descriptor retired after classification aborts the set too.
    let model = store.model("retired_descriptor", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    store
        .kernel
        .commit(intent("retire-descriptor"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip();
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Changed
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// F1: the source triple binds a witness to its own attestation, so attestation A's witness
/// leaves B's admission below `Verified`.
#[test]
fn one_attestations_witness_cannot_promote_another() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let first = store.model("first", &checkout.head, &evidence, false);
    let classification = store.classification(&first, &checkout);
    assert!(matches!(
        attest_now(&store, &classification, &evaluate(&classification)),
        Outcome::Committed(_)
    ));
    let a = Identity::of(&first, &checkout.head);
    let b = Identity::of("mem_second", &checkout.head);
    assert_ne!(a.source_id, b.source_id);
    let admitted = store
        .kernel
        .commit(intent("borrowed-witness"), |envelope| {
            envelope.insert_decision(DecisionSpec {
                decision_id: b.decision_id.clone(),
                object_id: b.object_id.clone(),
                source_kind: VERIFIER_SOURCE_KIND.into(),
                source_id: b.source_id.clone(),
                ..model_spec("second", None, &evidence, false, 1)
            })?;
            let decision = envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(b.object_id.clone()),
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::CodeObserved,
                    trigger_object_id: Some(a.witness_object_id.clone()),
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "borrowed witness".into(),
                },
            })?;
            Ok(decision.effective_maturity.as_str().to_owned())
        })
        .unwrap();
    assert_eq!(admitted.result, Maturity::Candidate.as_str());
}

/// F11: a fault after the witness admission rolls the envelope back with no attestation,
/// observation, causality, or citation row and the model decision unchanged; after a commit and a
/// fresh open every companion row and the receipt remain.
#[test]
fn a_rollback_after_the_admission_leaves_no_row_and_a_reopened_store_keeps_every_row() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("fault", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    store
        .sqlite()
        .execute_batch(&format!(
            "CREATE TRIGGER fail_citation BEFORE INSERT ON decision_events
               WHEN NEW.event_kind='{CITATION_EVENT_KIND}'
             BEGIN SELECT RAISE(ABORT,'fault after the admission'); END;"
        ))
        .unwrap();
    let model_before = store.facts(&model);
    let tip = store.tip();
    let outcome = attest_now(&store, &classification, &evaluation);
    assert!(matches!(outcome, Outcome::Refused(_)), "{outcome:?}");
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM observations WHERE object_id GLOB 'verwitnessobj:*'"),
        0
    );
    assert_eq!(store.facts(&model), model_before);
    store
        .sqlite()
        .execute_batch("DROP TRIGGER fail_citation;")
        .unwrap();

    let Outcome::Committed(receipt) = attest_now(&store, &classification, &evaluation) else {
        panic!("the attestation commits");
    };
    let rows = store.write_set_rows();
    let store = store.reopen();
    assert_eq!(store.write_set_rows(), rows);
    let ids = Identity::of(&model, &checkout.head);
    let attestation = store.facts(&ids.object_id);
    assert_eq!(
        attestation.own_admission.unwrap().effective_maturity,
        Maturity::Verified
    );
    assert!(matches!(
        attestation.causality,
        CausalClass::DerivedReinjection { .. }
    ));
    assert_eq!(
        witness_detail(&store, &ids.witness_object_id)["evidence_id"],
        evidence.id.as_str()
    );
    // The receipt is durable: a fresh classification at the new tip replays it.
    let target = store.kernel.capture_commit_read_target().unwrap();
    let reclassified = Classification {
        target,
        class: classification.class.clone(),
    };
    let Outcome::Replayed(replayed) = attest_now(&store, &reclassified, &evaluation) else {
        panic!("the reopened store replays the receipt");
    };
    assert_eq!(replayed.commit_seq, receipt.commit_seq);
}

/// AC15: the verifier's sources name no model client. Every `crate::` path they reference is in a
/// closed allowlist of pure modules, and no model-client module or network crate appears; the
/// scanner flags a source that names one.
#[test]
fn no_model_client_is_reachable_from_the_verifier_module() {
    const SOURCES: [&str; 3] = [
        include_str!("../src/attestation_verifier.rs"),
        include_str!("../src/attestation_verifier/classify.rs"),
        include_str!("../src/attestation_verifier/attest.rs"),
    ];
    const ALLOWED: [&str; 6] = [
        "crate::confined_path",
        "crate::git_reconcile::identity_field",
        "crate::git_sources",
        "crate::project_docs",
        "crate::memory_capture::cite::digest_bytes",
        "crate::now_ms",
    ];
    const FORBIDDEN: [&str; 9] = [
        "memory_reviewer",
        "memory_classifier",
        "history_summarizer",
        "embedding_dispatch",
        "embedding_supervisor",
        "hyper",
        "reqwest",
        "tokio",
        "bedrock",
    ];
    fn violations(source: &str) -> Vec<String> {
        let mut found: Vec<String> = source
            .match_indices("crate::")
            .map(|(at, _)| {
                source[at..]
                    .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                    .next()
                    .unwrap()
                    .trim_end_matches(':')
                    .to_owned()
            })
            .filter(|path| !ALLOWED.iter().any(|allowed| path.starts_with(allowed)))
            .collect();
        found.extend(
            FORBIDDEN
                .iter()
                .filter(|name| source.contains(*name))
                .map(|name| (*name).to_owned()),
        );
        found
    }
    for source in SOURCES {
        assert_eq!(violations(source), Vec::<String>::new());
    }
    assert_eq!(
        violations("use crate::memory_reviewer::bedrock::Client;"),
        [
            "crate::memory_reviewer::bedrock::Client",
            "memory_reviewer",
            "bedrock"
        ]
    );
    assert!(!violations("let x = crate::history_summarizer::run();").is_empty());
    // The allowlisted paths are the ones the sources use, so the allowlist is not vacuous.
    let all: String = SOURCES.concat();
    for allowed in ALLOWED {
        assert!(all.contains(allowed), "{allowed}");
    }
}
