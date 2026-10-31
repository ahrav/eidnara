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
use daemon::attestation_verifier::{Claim, Evaluation, Reason, Verdict, verify};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, AnchorSpec, AnchorValue, ArtifactDeletionIdentity,
    ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactIngestRequest, CausalClass,
    CitingDescriptor, ClaimFactBounds, ClaimFacts, CommitIntent, DecisionEventPayload,
    DecisionEventSpec, DecisionPayload, DecisionSpec, Disposition, DomainSpec, EventKind,
    KernelError, KernelStore, Maturity, Outcome as AdmissionOutcome, ParentReference,
    ProviderEgress, Sensitivity, ServedStanding, SourceClass, SourceDescriptorPolicy,
    SourceDescriptorRequest, Surface, SurfaceVisibility, TaintClass,
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
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
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

/// A repository whose one commit holds one document.
struct Checkout {
    dir: tempfile::TempDir,
    head: String,
    blob: String,
}

impl Checkout {
    /// A commit holding `text` at `DOC_PATH`.
    fn new(text: &str) -> Self {
        Self::at(DOC_PATH, text)
    }

    /// A commit holding `text` at `path`.
    fn at(path: &str, text: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(dir.path().join(path), text).unwrap();
        git(dir.path(), &["add", path]);
        git(dir.path(), &["commit", "--quiet", "-m", "doc"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        let blob = git(dir.path(), &["rev-parse", &format!("HEAD:{path}")]);
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

    /// Closes the store and opens its data home again. The store holds its data home's writer
    /// lock while it is live, so a second open needs the first closed.
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

    /// Retains `text` and publishes the document descriptor at `DOC_PATH` citing it.
    fn document(&self, key: &str, text: &str) -> (Evidence, String) {
        self.document_at(key, DOC_PATH, text)
    }

    /// Retains `text` and publishes the document descriptor at `path` citing it.
    fn document_at(&self, key: &str, path: &str, text: &str) -> (Evidence, String) {
        let handle = self.retain(key, text);
        let identity = [
            ("project_id", "proj-a"),
            ("repository_id", REPOSITORY),
            ("relative_path", path),
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
    // `verify` names no blob before the path resolves, and names it for a reason found after.
    for (reason, blob_oid) in [
        (Reason::RepositoryUnbound, None),
        (Reason::PathAbsent, None),
        (Reason::OverBudget, Some(checkout.blob.clone())),
    ] {
        let evaluation = Evaluation {
            verdict: Verdict::Unavailable(reason),
            bytes_read: 0,
            blob_oid,
        };
        assert_eq!(
            attest_now(&store, &classification, &evaluation),
            Outcome::Skipped(Skip::Unavailable)
        );
    }
    // A `Match` or `Mismatch` without the blob it was decided on is no write set.
    for verdict in [Verdict::Match, Verdict::Mismatch] {
        let evaluation = Evaluation {
            verdict,
            bytes_read: 0,
            blob_oid: None,
        };
        assert_eq!(
            attest_now(&store, &classification, &evaluation),
            Outcome::InvalidWriteSet,
            "{verdict:?}"
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// A write set built at a sequence before the model decision existed is `Changed`.
#[test]
fn a_write_set_built_before_the_model_decision_existed_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let before = store.kernel.capture_commit_read_target().unwrap();
    let model = store.model("later", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let write_set = WriteSet::new(
        &store.kernel,
        before,
        input(&classification),
        &checkout.blob,
        &EvalBudget::unbounded(),
    );
    assert_eq!(write_set.unwrap_err(), Outcome::Changed);
}

/// AC9: a witness at another revision, a detail or citation naming another evidence row, a detail
/// at another version, an id other than the pair's [`Identity`], a domain, sensitivity, or anchor
/// other than the model decision's, a row-level evidence citation, or a witness kind other than
/// the document's is refused before the commit.
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
    assert!(detail.contains(&format!("\"witness_version\":{WITNESS_DETAIL_VERSION}")));
    let mutations: [(&str, Mutation); 21] = [
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
        ("attestation sensitivity", |set, _, _| {
            set.attestation.sensitivity = Sensitivity::Sensitive;
        }),
        ("witness sensitivity", |set, _, _| {
            set.witness.sensitivity = Sensitivity::Sensitive;
        }),
        ("anchor", |set, _, _| set.attestation.anchor_id = None),
        ("attestation domain", |set, _, _| {
            set.attestation.domain_id = "source".into();
        }),
        ("witness domain", |set, _, _| {
            set.witness.domain_id = "source".into();
        }),
        ("object id", |set, _, _| {
            set.attestation.object_id = "verattest:other".into();
        }),
        ("decision id", |set, _, _| {
            set.attestation.decision_id = "verattestdec:other".into();
        }),
        ("shared source id", |set, _, _| {
            set.attestation.source_id = "shared".into();
            set.witness.source_id = "shared".into();
        }),
        ("witness observation id", |set, _, _| {
            set.witness.observation_id = "verwitness:other".into();
        }),
        ("witness object id", |set, _, _| {
            set.witness.object_id = "verwitnessobj:other".into();
        }),
        ("attestation evidence", |set, _, other| {
            set.attestation.evidence_id = Some(other.into());
        }),
        ("witness evidence", |set, _, other| {
            set.witness.evidence_id = Some(other.into());
        }),
        ("detail version", |set, detail, _| {
            let version = format!("\"witness_version\":{WITNESS_DETAIL_VERSION}");
            set.witness.payload.detail = Some(detail.replace(&version, "\"witness_version\":2"));
        }),
        ("blob", |set, _, _| set.blob_oid = "0".repeat(40)),
        ("detail commit", |set, detail, _| {
            set.witness.payload.detail =
                Some(detail.replace("\"commit_oid\":\"", "\"commit_oid\":\"0"));
        }),
        ("detail digest", |set, detail, _| {
            set.witness.payload.detail =
                Some(detail.replace("\"artifact_digest\":\"", "\"artifact_digest\":\"0"));
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

    // F3: the compared evidence row deleted after classification aborts the set too.
    let store = Store::open();
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("deleted_evidence", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    store
        .kernel
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-evidence"),
            identity: ArtifactDeletionIdentity::EvidenceId(evidence.id.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 1,
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

/// Timing: attesting one model decision commits nothing the other's classification rechecks, so a
/// second decision classified at the same sequence still commits on its older classification.
#[test]
fn an_unrelated_attestation_does_not_abort_an_older_classification() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let a = store.model("a", &checkout.head, &evidence, false);
    let b = store.model("b", &checkout.head, &evidence, false);
    let first = store.classification(&a, &checkout);
    let second = store.classification(&b, &checkout);
    assert_eq!(first.target, second.target);
    assert!(matches!(
        attest_now(&store, &first, &evaluate(&first)),
        Outcome::Committed(_)
    ));
    let outcome = attest_now(&store, &second, &evaluate(&second));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let attestation = store.facts(&Identity::of(&b, &checkout.head).object_id);
    assert_eq!(
        attestation.own_admission.unwrap().effective_maturity,
        Maturity::Verified
    );
}

/// AC6 for a configuration document: the witness is `config_present`, the admission
/// `ConfigObserved` under `(TrustedLocalCode, CurrentConfig)`.
#[test]
fn a_match_on_a_configuration_document_commits_under_current_config() {
    const CONFIG_PATH: &str = "config.toml";
    const CONFIG: &str = "[store]\nowner = \"daemon\"\n";
    let store = Store::open();
    let checkout = Checkout::at(CONFIG_PATH, CONFIG);
    let (evidence, _) = store.document_at("config", CONFIG_PATH, CONFIG);
    let model = store.model("config", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Match);
    let outcome = attest_now(&store, &classification, &evaluation);
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let ids = Identity::of(&model, &checkout.head);
    let admission = store.facts(&ids.object_id).own_admission.unwrap();
    assert_eq!(
        (admission.source_class, admission.taint_class),
        (SourceClass::TrustedLocalCode, TaintClass::CurrentConfig)
    );
    assert_eq!(admission.event_kind, EventKind::ConfigObserved);
    assert_eq!(admission.outcome, AdmissionOutcome::Admit);
    assert_eq!(admission.effective_maturity, Maturity::Verified);
    assert_eq!(admission.disposition, Disposition::Active);
    let kind: String = store
        .sqlite()
        .query_row(
            "SELECT observation_kind FROM observations WHERE object_id=?1",
            [&ids.witness_object_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "config_present");
    assert_eq!(
        witness_detail(&store, &ids.witness_object_id)["relative_path"],
        CONFIG_PATH
    );
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
/// observation, causality, or citation row and the model decision unchanged; after a commit, a
/// close, and a fresh open of the data home, every companion row and the receipt remain.
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
        panic!("the store opened after the close replays the receipt");
    };
    assert_eq!(replayed.commit_seq, receipt.commit_seq);
}

/// AC15: the verifier's sources name no model client. The test reads every `.rs` file under
/// `src/attestation_verifier/` and `src/attestation_verifier.rs` at run time. Every `crate::` path
/// they reference is in a closed allowlist of pure modules, and no model-client module or network
/// crate appears; the scanner flags a source that names one.
#[test]
fn no_model_client_is_reachable_from_the_verifier_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = vec![src.join("attestation_verifier.rs")];
    let mut pending = vec![src.join("attestation_verifier")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                paths.push(path);
            }
        }
    }
    assert!(paths.len() >= 3, "{paths:?}");
    let sources: Vec<String> = paths
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
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
    for (path, source) in paths.iter().zip(&sources) {
        assert_eq!(
            violations(source),
            Vec::<String>::new(),
            "{}",
            path.display()
        );
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
    let all: String = sources.concat();
    for allowed in ALLOWED {
        assert!(all.contains(allowed), "{allowed}");
    }
}

/// A model decision like [`Store::model`]'s, citing `evidence` through a decision event, admitted
/// under `taint`.
fn model_with_taint(
    store: &Store,
    name: &str,
    oid: &str,
    evidence: &Evidence,
    taint: TaintClass,
) -> String {
    let object_id = format!("mem_{name}");
    store
        .kernel
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
            envelope.insert_decision(model_spec(name, Some(anchor_id), evidence, false, 1))?;
            envelope.append_decision_event(
                &format!("dec_{name}"),
                DecisionEventSpec {
                    event_kind: "memory_capture_citation".into(),
                    payload: DecisionEventPayload {
                        summary: "cites the document".into(),
                    },
                    evidence_id: Some(evidence.id.clone()),
                    recorded_at: 1,
                },
            )?;
            envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(object_id.clone()),
                source_class: Some(SourceClass::ModelInference),
                taint_class: Some(taint),
                event: AdmissionEvent {
                    kind: EventKind::Other,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "fixture".into(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap();
    object_id
}

/// The Match and Mismatch evaluations for `evaluation`'s blob.
fn both_verdicts(evaluation: &Evaluation) -> [Evaluation; 2] {
    [Verdict::Match, Verdict::Mismatch].map(|verdict| Evaluation {
        verdict,
        ..evaluation.clone()
    })
}

/// AC6, C14: a model decision whose registry class is `Normal` but whose `Personal` taint floor
/// makes the Kernel serve it `Sensitive` gets an attestation served `Sensitive`, hidden on
/// `AutoInject` like the model decision.
#[test]
fn the_attestation_serves_at_the_model_decisions_folded_class() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = model_with_taint(
        &store,
        "personal",
        &checkout.head,
        &evidence,
        TaintClass::Personal,
    );
    let model_served = served(&store.facts(&model));
    assert_eq!(model_served.0, Sensitivity::Sensitive);
    let classification = store.classification(&model, &checkout);
    assert_eq!(
        input(&classification).decision.sensitivity,
        Sensitivity::Normal
    );
    assert_eq!(
        input(&classification).decision.served_sensitivity,
        Sensitivity::Sensitive
    );
    let outcome = attest_now(&store, &classification, &evaluate(&classification));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let attestation = store.facts(&Identity::of(&model, &checkout.head).object_id);
    assert_eq!(
        served(&attestation),
        (Sensitivity::Sensitive, SurfaceVisibility::Hidden)
    );
    assert_eq!(served(&attestation), model_served);
}

/// A classification whose served class no longer matches what the Kernel serves the model
/// decision at is `Changed` inside the envelope, after the write set's own check passed.
#[test]
fn a_served_class_other_than_the_kernels_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("served", &checkout.head, &evidence, false);
    let mut classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let Class::Verifiable(input) = &mut classification.class else {
        panic!("expected a verifiable fixture");
    };
    input.decision.served_sensitivity = Sensitivity::Sensitive;
    let budget = EvalBudget::unbounded();
    let write_set = WriteSet::new(
        &store.kernel,
        classification.target,
        input,
        &checkout.blob,
        &budget,
    )
    .unwrap();
    assert!(write_set.check());
    let tip = store.tip();
    assert_eq!(write_set.commit(&store.kernel, &budget), Outcome::Changed);
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Changed
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// AC9: the witness detail's path, descriptor, lineage, and revision, the witness scope, the
/// citation kind, the shared source revision, and the shared source kind are bound before the
/// commit.
#[test]
fn a_write_set_naming_another_document_or_revision_is_refused_before_the_commit() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("bound", &checkout.head, &evidence, false);
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
    let mutations: [(&str, Mutation); 9] = [
        ("detail path", |set, detail, _| {
            set.witness.payload.detail =
                Some(detail.replace("\"relative_path\":\"", "\"relative_path\":\"docs/"));
        }),
        ("detail descriptor", |set, detail, _| {
            set.witness.payload.detail = Some(detail.replace(
                "\"descriptor_object_id\":\"",
                "\"descriptor_object_id\":\"0",
            ));
        }),
        ("detail lineage", |set, detail, _| {
            set.witness.payload.detail =
                Some(detail.replace("\"lineage_id\":\"", "\"lineage_id\":\"0"));
        }),
        ("detail descriptor revision", |set, detail, _| {
            set.witness.payload.detail =
                Some(detail.replace("\"descriptor_revision\":\"", "\"descriptor_revision\":\"0"));
        }),
        ("witness scope", |set, _, _| {
            set.witness.scope_id = Some("scope".into());
        }),
        ("citation kind", |set, _, _| {
            set.citation.event_kind = "memory_capture_citation".into();
        }),
        ("shared revision", |set, _, _| {
            set.attestation.source_revision = 2;
            set.witness.source_revision = 2;
        }),
        ("shared source kind", |set, _, _| {
            set.attestation.source_kind = "assistant".into();
            set.witness.source_kind = "assistant".into();
        }),
        ("detail absent", |set, _, _| {
            set.witness.payload.detail = None;
        }),
    ];
    let tip = store.tip();
    for (name, mutate) in mutations {
        let mut mutated = write_set.clone();
        mutate(&mut mutated, &detail, "");
        assert_ne!(mutated, write_set, "{name}");
        assert!(!mutated.check(), "{name}");
        assert_eq!(
            mutated.commit(&store.kernel, &budget),
            Outcome::InvalidWriteSet,
            "{name}"
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// A classification from another store incarnation, or naming other admission classes, is
/// `Changed` on both verdicts with nothing written.
#[test]
fn a_classification_from_another_incarnation_or_with_other_classes_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("foreign", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let foreign = Classification {
        target: kernel::CommitReadTarget {
            incarnation: Store::open()
                .kernel
                .capture_commit_read_target()
                .unwrap()
                .incarnation,
            ..classification.target
        },
        class: classification.class.clone(),
    };
    let mut reclassed = classification.clone();
    let Class::Verifiable(input) = &mut reclassed.class else {
        panic!("expected a verifiable fixture");
    };
    input.decision.taint_class = TaintClass::UserInferred;
    let tip = store.tip();
    let admissions = store.admissions_of(&model);
    for attempted in both_verdicts(&evaluation) {
        for changed in [&foreign, &reclassed] {
            assert_eq!(
                attest_now(&store, changed, &attempted),
                Outcome::Changed,
                "{:?}",
                attempted.verdict
            );
        }
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.admissions_of(&model), admissions);
    assert_eq!(store.write_set_rows(), 0);
}

/// F2 on the stale path: a mismatch for a model decision re-disposed after its classification is
/// `Changed` and records no `MarkStale`.
#[test]
fn a_mismatch_for_a_decision_re_disposed_after_classification_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new("# Other\n");
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("quarantined", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Mismatch);
    store
        .kernel
        .commit(intent("quarantine"), |envelope| {
            model_admission(envelope, &model, EventKind::Quarantine)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip();
    let admissions = store.admissions_of(&model);
    assert_eq!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Changed
    );
    assert_eq!(store.tip(), tip);
    assert_eq!(store.admissions_of(&model), admissions);
}

/// F3: a newer live revision of the cited descriptor's lineage, citing the same evidence or
/// other evidence, aborts both verdicts' envelopes.
#[test]
fn a_descriptor_republished_after_classification_is_changed() {
    for (key, text) in [("same", DOC), ("other", "# Architecture\n\nRevised.\n")] {
        let store = Store::open();
        let checkout = Checkout::new(DOC);
        let (evidence, _) = store.document("doc", DOC);
        let model = store.model(key, &checkout.head, &evidence, false);
        let classification = store.classification(&model, &checkout);
        let evaluation = evaluate(&classification);
        let handle = if key == "same" {
            None
        } else {
            Some(store.retain("revised", text))
        };
        let (evidence_id, digest) = match &handle {
            Some(handle) => (handle.evidence_id.clone(), handle.digest.clone()),
            None => (evidence.id.clone(), digest_hex(DOC)),
        };
        let identity = [
            ("project_id", "proj-a"),
            ("repository_id", REPOSITORY),
            ("relative_path", DOC_PATH),
        ];
        store
            .kernel
            .commit(intent("descriptor-revision-2"), |envelope| {
                envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: OccurrenceClass::RepositoryDocuments.code(),
                            identity: &identity,
                            revision: "2",
                            representation: "text",
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: "source",
                        scope_id: None,
                        evidence_id: &evidence_id,
                        artifact_digest: &digest,
                        buffer: text,
                        sensitivity: Sensitivity::Sensitive,
                        observed_at: 2,
                        expected_head: Some(1),
                        outcome: None,
                    })
                    .map_err(|_| KernelError::InvalidInput)?;
                Ok(String::new())
            })
            .unwrap();
        let tip = store.tip();
        for attempted in both_verdicts(&evaluation) {
            assert_eq!(
                attest_now(&store, &classification, &attempted),
                Outcome::Changed,
                "{key} {:?}",
                attempted.verdict
            );
        }
        assert_eq!(store.tip(), tip, "{key}");
        assert_eq!(store.write_set_rows(), 0, "{key}");
    }
}

/// F10: a `Match` after a `MarkStale` recorded for another blob conflicts through the opposite
/// key's digest refusal.
#[test]
fn a_match_after_a_stale_path_under_another_blob_conflicts() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("flip_blob", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let mismatch = Evaluation {
        verdict: Verdict::Mismatch,
        blob_oid: Some("0".repeat(40)),
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

/// D1: the identity is the SHA-256 of the model object id and the oid, so another oid gives
/// another identity.
#[test]
fn the_identity_depends_on_the_model_and_the_oid() {
    let oid = "1".repeat(40);
    let identity = Identity::of("mem_a", &oid);
    let hash = format!("{:x}", Sha256::digest(format!("mem_a\n{oid}")));
    assert_eq!(identity.source_id, hash);
    assert_eq!(identity.object_id, format!("verattest:{hash}"));
    let other = Identity::of("mem_a", &"2".repeat(40));
    assert_ne!(identity.source_id, other.source_id);
    assert_ne!(identity.object_id, other.object_id);
    assert_ne!(identity.witness_object_id, other.witness_object_id);
}

/// The `Match` envelope reads the model decision's row through the writer, so the attestation
/// commits while every pooled reader is held, where a reader-side read waits out its budget.
#[test]
fn a_match_commits_while_every_pooled_reader_is_held() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("readers", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let budget = || {
        EvalBudget::new(
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    };
    let (built, outcome) = store.kernel.with_readers_held_for_test(|| {
        let built = WriteSet::new(
            &store.kernel,
            classification.target,
            input(&classification),
            &checkout.blob,
            &budget(),
        );
        (
            built,
            attest(&store.kernel, &classification, &evaluation, &budget()),
        )
    });
    assert_eq!(built.unwrap_err(), Outcome::Refused(KernelError::Deadline));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    assert_eq!(
        store
            .facts(&Identity::of(&model, &checkout.head).object_id)
            .own_admission
            .unwrap()
            .effective_maturity,
        Maturity::Verified
    );
}

/// F2 on the stale path: a model decision superseded or retired after its classification leaves
/// its `Mismatch` envelope `Changed` with no `MarkStale` row, the recheck's liveness predicates
/// being what refuses it.
#[test]
fn a_mismatch_for_a_decision_superseded_or_retired_after_classification_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new("# Other\n");
    let (evidence, _) = store.document("doc", DOC);
    let classified: Vec<_> = ["stale_superseded", "stale_retired"]
        .iter()
        .map(|name| {
            let model = store.model(name, &checkout.head, &evidence, false);
            let classification = store.classification(&model, &checkout);
            let evaluation = evaluate(&classification);
            assert_eq!(evaluation.verdict, Verdict::Mismatch);
            (model, classification, evaluation)
        })
        .collect();
    store
        .kernel
        .commit(intent("change"), |envelope| {
            envelope.supersede_decision(
                "mem_stale_superseded",
                DecisionSpec {
                    decision_id: "dec_stale_superseded_2".into(),
                    object_id: "mem_stale_superseded_2".into(),
                    ..model_spec(
                        "stale_superseded",
                        Some("anchor_stale_superseded".into()),
                        &evidence,
                        false,
                        2,
                    )
                },
            )?;
            model_admission(envelope, "mem_stale_superseded_2", EventKind::Other)?;
            envelope.retire_decision("mem_stale_retired")?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip();
    for (model, classification, evaluation) in &classified {
        let admissions = store.admissions_of(model);
        assert_eq!(
            attest_now(&store, classification, evaluation),
            Outcome::Changed,
            "{model}"
        );
        assert_eq!(store.admissions_of(model), admissions, "{model}");
    }
    assert_eq!(store.tip(), tip);
}

/// A classification whose sequence precedes the model decision's creating commit is `Changed` on
/// both verdicts with nothing written, as `WriteSet::new` is for a write set built there.
#[test]
fn a_classification_at_a_sequence_before_the_model_decision_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let before = store.kernel.capture_commit_read_target().unwrap();
    let model = store.model("early", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let early = Classification {
        target: before,
        class: classification.class.clone(),
    };
    let tip = store.tip();
    for evaluation in both_verdicts(&evaluation) {
        assert_eq!(
            attest_now(&store, &early, &evaluation),
            Outcome::Changed,
            "{:?}",
            evaluation.verdict
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
    assert!(matches!(
        attest_now(&store, &classification, &evaluation),
        Outcome::Committed(_)
    ));
}

/// The attestation copies the model decision's decision kind, scope, and summary from the row its
/// envelope reads.
#[test]
fn the_attestation_copies_the_model_decisions_kind_scope_and_summary() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = "mem_copied".to_owned();
    store
        .kernel
        .commit(intent("decision-copied"), |envelope| {
            envelope.insert_scope(kernel::ScopeSpec {
                scope_id: "scope_copied".into(),
                object_id: "scopeobj_copied".into(),
                domain_id: "memory".into(),
                source_kind: "assistant".into(),
                source_id: "memory_capture".into(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: Vec::new(),
            })?;
            envelope.insert_anchor(AnchorSpec {
                anchor_id: "anchor_copied".into(),
                object_id: "anchorobj_copied".into(),
                domain_id: "memory".into(),
                source_kind: "assistant".into(),
                source_id: "memory_capture".into(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                value: AnchorValue::ReachableFrom {
                    oid: checkout.head.clone(),
                    captures: Vec::new(),
                },
            })?;
            envelope.insert_decision(DecisionSpec {
                decision_kind: "SECURITY".into(),
                scope_id: Some("scope_copied".into()),
                ..model_spec("copied", Some("anchor_copied".into()), &evidence, false, 1)
            })?;
            envelope.append_decision_event(
                "dec_copied",
                DecisionEventSpec {
                    event_kind: "memory_capture_citation".into(),
                    payload: DecisionEventPayload {
                        summary: "cites the document".into(),
                    },
                    evidence_id: Some(evidence.id.clone()),
                    recorded_at: 1,
                },
            )?;
            model_admission(envelope, &model, EventKind::Other)?;
            Ok(String::new())
        })
        .unwrap();
    let classification = store.classification(&model, &checkout);
    let outcome = attest_now(&store, &classification, &evaluate(&classification));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let attestation = Identity::of(&model, &checkout.head).object_id;
    let rows = store
        .kernel
        .decisions_for_objects_as_of(&[model.clone(), attestation.clone()], store.tip())
        .unwrap();
    let row = |id: &str| rows.iter().find(|row| row.object_id == id).unwrap();
    let (model_row, attested) = (row(&model), row(&attestation));
    assert_eq!(attested.decision_kind, model_row.decision_kind);
    assert_eq!(attested.payload.summary, model_row.payload.summary);
    assert_eq!(attested.scope_id, model_row.scope_id);
    assert_eq!(
        (
            model_row.decision_kind.as_str(),
            model_row.scope_id.as_deref(),
            model_row.payload.summary.as_str(),
        ),
        (
            "SECURITY",
            Some("scope_copied"),
            "The daemon owns the store (copied).",
        )
    );
}

#[test]
fn a_write_set_with_content_other_than_the_model_decisions_is_refused_in_the_envelope() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("content", &checkout.head, &evidence, false);
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
    let mutations: [(&str, Mutation); 6] = [
        ("decision kind", |set, _, _| {
            set.attestation.decision_kind = "SECURITY".into();
        }),
        ("summary", |set, _, _| {
            set.attestation.payload.summary = "The plugin owns the store.".into();
        }),
        ("rationale", |set, _, _| {
            set.attestation.payload.rationale = "{}".into();
        }),
        ("witness summary", |set, _, _| {
            set.witness.payload.summary = "the bytes are present".into();
        }),
        ("witness classification", |set, _, _| {
            set.witness.payload.classification = "config_present".into();
        }),
        ("citation summary", |set, _, _| {
            set.citation.payload.summary = "cites another document".into();
        }),
    ];
    let tip = store.tip();
    for (name, mutate) in mutations {
        let mut mutated = write_set.clone();
        mutate(&mut mutated, "", "");
        assert_ne!(mutated, write_set, "{name}");
        assert!(mutated.check(), "{name}");
        assert_eq!(
            mutated.commit(&store.kernel, &budget),
            Outcome::InvalidWriteSet,
            "{name}"
        );
        assert_eq!(store.tip(), tip, "{name}");
    }
    assert_eq!(store.write_set_rows(), 0);
    assert!(matches!(
        write_set.commit(&store.kernel, &budget),
        Outcome::Committed(_)
    ));
}

/// A classification whose decision facts name a live anchor other than the model decision's row
/// anchor, at the same oid, is `Changed` on both verdicts with nothing written.
#[test]
fn a_classification_naming_another_live_anchor_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("anchored", &checkout.head, &evidence, false);
    store.model("neighbour", &checkout.head, &evidence, false);
    let mut classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let Class::Verifiable(input) = &mut classification.class else {
        panic!("expected a verifiable decision");
    };
    assert_eq!(input.decision.anchor_id.as_deref(), Some("anchor_anchored"));
    input.decision.anchor_id = Some("anchor_neighbour".into());
    let tip = store.tip();
    for attempted in both_verdicts(&evaluation) {
        assert_eq!(
            attest_now(&store, &classification, &attempted),
            Outcome::Changed,
            "{:?}",
            attempted.verdict
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// A classification whose claim names a path, digest, or byte length other than the cited
/// descriptor's, at the same oid, is `Changed` on both verdicts with nothing written.
#[test]
fn a_classification_whose_claim_differs_from_the_cited_descriptor_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("claimed", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let claim = &input(&classification).claim;
    let (path, digest, length, oid) = (
        claim.relative_path(),
        *claim.digest(),
        claim.byte_length(),
        claim.oid(),
    );
    let edited: [(&str, Claim); 3] = [
        (
            "path",
            Claim::new("docs/other.md", digest, length, oid).unwrap(),
        ),
        ("digest", Claim::new(path, [0; 32], length, oid).unwrap()),
        (
            "byte length",
            Claim::new(path, digest, length + 1, oid).unwrap(),
        ),
    ];
    let tip = store.tip();
    for (name, claim) in edited {
        let mut classification = classification.clone();
        let Class::Verifiable(input) = &mut classification.class else {
            panic!("expected a verifiable decision");
        };
        input.claim = claim;
        for attempted in both_verdicts(&evaluation) {
            assert_eq!(
                attest_now(&store, &classification, &attempted),
                Outcome::Changed,
                "{name} {:?}",
                attempted.verdict
            );
        }
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// A classification whose evidence, descriptor, and claim all name a live document the model
/// decision never cited is `Changed` on both verdicts with nothing written.
#[test]
fn a_classification_citing_a_document_the_model_decision_does_not_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("cites", &checkout.head, &evidence, false);
    let other_text = "# Other\n\nAnother document the model never cited.\n";
    let (other, other_descriptor) = store.document_at("other", "docs/other.md", other_text);
    let mut classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let CitingDescriptor::Live(descriptor) = store
        .kernel
        .source_descriptor_citing_evidence(
            OccurrenceClass::RepositoryDocuments,
            &other.id,
            store.tip(),
            &EvalBudget::unbounded(),
        )
        .unwrap()
    else {
        panic!("expected the other document's descriptor live");
    };
    assert_eq!(descriptor.object_id, other_descriptor);
    let Class::Verifiable(input) = &mut classification.class else {
        panic!("expected a verifiable decision");
    };
    assert_eq!(descriptor.detail.artifact_digest, digest_hex(other_text));
    input.claim = Claim::new(
        "docs/other.md",
        Sha256::digest(other_text).into(),
        descriptor.byte_length,
        input.claim.oid(),
    )
    .unwrap();
    input.evidence_id = other.id.clone();
    input.descriptor_object_id = descriptor.object_id.clone();
    input.lineage_id = descriptor.detail.lineage_id.clone();
    input.descriptor_revision = descriptor.detail.revision.clone();
    let tip = store.tip();
    for attempted in both_verdicts(&evaluation) {
        assert_eq!(
            attest_now(&store, &classification, &attempted),
            Outcome::Changed,
            "{:?}",
            attempted.verdict
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// A model decision whose row cites the compared document and whose events then cite
/// `MAX_EVENT_CITATIONS + 1` other evidence rows still attests and still goes stale: the recheck
/// reads the row-level citation first, as `classify` does, outside the recency-bounded event window.
#[test]
fn a_row_level_citation_older_than_the_event_window_still_attests() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("row_cited", &checkout.head, &evidence, true);
    let window = daemon::attestation_verifier::classify::MAX_EVENT_CITATIONS.get();
    let newer: Vec<kernel::ArtifactHandle> = (0..=window)
        .map(|index| {
            store.retain(
                &format!("newer-{index}"),
                &format!("newer evidence {index}\n"),
            )
        })
        .collect();
    store
        .kernel
        .commit(intent("newer-citations"), |envelope| {
            for (index, handle) in newer.iter().enumerate() {
                envelope.append_decision_event(
                    "dec_row_cited",
                    DecisionEventSpec {
                        event_kind: "memory_capture_citation".into(),
                        payload: DecisionEventPayload {
                            summary: format!("cites newer evidence {index}"),
                        },
                        evidence_id: Some(handle.evidence_id.clone()),
                        recorded_at: 2 + index as i64,
                    },
                )?;
            }
            Ok(String::new())
        })
        .unwrap();
    let classification = store.classification(&model, &checkout);
    assert_eq!(input(&classification).evidence_id, evidence.id);
    let evaluation = evaluate(&classification);
    assert_eq!(evaluation.verdict, Verdict::Match);
    let outcome = attest_now(&store, &classification, &evaluation);
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let other = store.model("row_cited_stale", &checkout.head, &evidence, true);
    store
        .kernel
        .commit(intent("newer-citations-stale"), |envelope| {
            for (index, handle) in newer.iter().enumerate() {
                envelope.append_decision_event(
                    "dec_row_cited_stale",
                    DecisionEventSpec {
                        event_kind: "memory_capture_citation".into(),
                        payload: DecisionEventPayload {
                            summary: format!("cites newer evidence {index}"),
                        },
                        evidence_id: Some(handle.evidence_id.clone()),
                        recorded_at: 2 + index as i64,
                    },
                )?;
            }
            Ok(String::new())
        })
        .unwrap();
    let classification = store.classification(&other, &checkout);
    let mismatch = Evaluation {
        verdict: Verdict::Mismatch,
        ..evaluation
    };
    let outcome = attest_now(&store, &classification, &mismatch);
    assert!(matches!(outcome, Outcome::StaleCommitted(_)), "{outcome:?}");
}

/// A classification whose descriptor revision differs from the live descriptor's, at the same
/// descriptor object id, is `Changed` on both verdicts with nothing written.
#[test]
fn a_classification_naming_another_descriptor_revision_is_changed() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("revised", &checkout.head, &evidence, false);
    let mut classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let Class::Verifiable(input) = &mut classification.class else {
        panic!("expected a verifiable decision");
    };
    assert_eq!(input.descriptor_revision, "1");
    input.descriptor_revision = "2".into();
    let tip = store.tip();
    for attempted in both_verdicts(&evaluation) {
        assert_eq!(
            attest_now(&store, &classification, &attempted),
            Outcome::Changed,
            "{:?}",
            attempted.verdict
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(store.write_set_rows(), 0);
}

/// AC8: the `MarkStale` row carries the classes of the model decision's latest admission, read in
/// the envelope, for a decision admitted under a taint other than the fixture's default.
#[test]
fn a_mismatch_marks_stale_under_the_model_decisions_own_classes() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = model_with_taint(
        &store,
        "personal_stale",
        &checkout.head,
        &evidence,
        TaintClass::Personal,
    );
    let classification = store.classification(&model, &checkout);
    let mismatch = Evaluation {
        verdict: Verdict::Mismatch,
        ..evaluate(&classification)
    };
    let outcome = attest_now(&store, &classification, &mismatch);
    assert!(matches!(outcome, Outcome::StaleCommitted(_)), "{outcome:?}");
    let stale = store.facts(&model).own_admission.unwrap();
    assert_eq!(stale.event_kind, EventKind::MarkStale);
    assert_eq!(
        (stale.source_class, stale.taint_class),
        (SourceClass::ModelInference, TaintClass::Personal)
    );
}

/// The committed attestation's rationale names the model object, the commit oid, the path, and
/// the compared digest, and the witness detail names the descriptor's lineage and revision.
#[test]
fn the_committed_rationale_and_detail_name_the_compared_claim() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("rationale", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let input = input(&classification).clone();
    let outcome = attest_now(&store, &classification, &evaluate(&classification));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let ids = Identity::of(&model, &checkout.head);
    let rationale: String = store
        .sqlite()
        .query_row(
            "SELECT json_extract(decision_payload, '$.rationale') FROM decisions WHERE object_id=?1",
            [&ids.object_id],
            |row| row.get(0),
        )
        .unwrap();
    let rationale: serde_json::Value = serde_json::from_str(&rationale).unwrap();
    assert_eq!(rationale["source"], PRODUCER);
    assert_eq!(rationale["attests"], model.as_str());
    assert_eq!(rationale["commit_oid"], checkout.head.as_str());
    assert_eq!(rationale["relative_path"], DOC_PATH);
    assert_eq!(rationale["artifact_digest"], digest_hex(DOC).as_str());
    let detail = witness_detail(&store, &ids.witness_object_id);
    assert_eq!(detail["lineage_id"], input.lineage_id.as_str());
    assert_eq!(
        detail["descriptor_revision"],
        input.descriptor_revision.as_str()
    );
}

/// D1: an attestation id already occupied in the registry refuses the write set as
/// `Refused(Conflict)` with nothing written, on every retry.
#[test]
fn an_occupied_attestation_id_refuses_the_write_set() {
    let store = Store::open();
    let checkout = Checkout::new(DOC);
    let (evidence, _) = store.document("doc", DOC);
    let model = store.model("occupied", &checkout.head, &evidence, false);
    let ids = Identity::of(&model, &checkout.head);
    store
        .kernel
        .commit(intent("squat"), |envelope| {
            envelope.insert_decision(DecisionSpec {
                decision_id: "dec_squat".into(),
                object_id: ids.object_id.clone(),
                ..model_spec("squat", None, &evidence, false, 1)
            })?;
            Ok(String::new())
        })
        .unwrap();
    let classification = store.classification(&model, &checkout);
    let evaluation = evaluate(&classification);
    let tip = store.tip();
    for _ in 0..2 {
        assert_eq!(
            attest_now(&store, &classification, &evaluation),
            Outcome::Refused(KernelError::Conflict)
        );
    }
    assert_eq!(store.tip(), tip);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions WHERE source_kind='verifier'"),
        0
    );
}

/// The request digest is a tagged, length-prefixed SHA-256 over the frozen claim, evidence, blob,
/// and verdict, pinned here so a layout change is a deliberate format bump.
#[test]
fn the_request_digest_is_pinned() {
    let store = Store::open();
    let checkout = Checkout::at("PINNED.md", "pinned\n");
    let (evidence, _) = store.document_at("pinned", "PINNED.md", "pinned\n");
    let model = store.model("pinned", &checkout.head, &evidence, false);
    let classification = store.classification(&model, &checkout);
    let input = input(&classification);
    let mut digest = Sha256::new();
    for part in [
        "verifier-request.v1",
        model.as_str(),
        "1",
        checkout.head.as_str(),
        "PINNED.md",
        digest_hex("pinned\n").as_str(),
        "7",
        evidence.id.as_str(),
        checkout.blob.as_str(),
        "match",
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    let expected = format!("{:x}", digest.finalize());
    let outcome = attest_now(&store, &classification, &evaluate(&classification));
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
    let recorded: String = store
        .sqlite()
        .query_row(
            "SELECT request_digest FROM operation_receipts WHERE operation_key=?1",
            [format!("attest:{model}:{}", checkout.head)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, expected);
    assert_eq!(input.claim.byte_length(), 7);
}
