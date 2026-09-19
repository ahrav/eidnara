//! The production selector against real stores: a bounded keyset walk over live descriptors judged through the Kernel's remote egress fold, deduplicated against the Memory Store's review jobs, paged at eight references with a resumable cursor that wraps to a fresh pass, and blind to descriptors the remote fold denies or that already have a job.

mod support;

use std::collections::BTreeMap;

use daemon::git_sources::{GitReadBounds, RepositoryBinding, read_selection};
use daemon::harness_sources::SourcePublisher;
use daemon::memory_reviewer::selection::{
    MAX_EXAMINED_PER_PAGE, MEMORY_CLASSES, SelectionScope, select_review_targets,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    ArtifactIngestRequest, CommitIntent, DecisionPayload, DecisionSpec, Dimension, DomainSpec,
    KernelStore, ProjectScope, ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity,
    SourceDescriptorPolicy, SourceDescriptorRequest,
};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, MAX_SELECTION_REFERENCES, MemoryReviewerJobOutcome, ProducerBinding, ReviewTarget,
};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const SCOPE: &str = "project:a";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "memory_reviewer-selection-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    store: KernelStore,
    ledger: MemoryStore,
    now: i64,
}

impl Fixture {
    fn open() -> Self {
        let kernel_dir = tempfile::tempdir().unwrap();
        let store = KernelStore::open(kernel_dir.path()).unwrap();
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
        let ledger_dir = tempfile::tempdir().unwrap();
        let ledger = MemoryStore::open(&MemoryStore::test_descriptor(
            ledger_dir.path(),
            "eidnara-memory_reviewer-selection-test",
        ))
        .unwrap();
        Self {
            _dirs: vec![kernel_dir, ledger_dir],
            store,
            ledger,
            now: 1_700_000_000_000,
        }
    }

    /// Publishes `count` commits through the Git publisher, each in its own repository, as remote-eligible evidence; returns their descriptor object ids.
    fn publish_commits(&mut self, count: usize, protected: bool) -> Vec<String> {
        (0..count)
            .map(|index| {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path();
                gix::init(root).unwrap();
                let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
                let tree = repo
                    .write_object(gix::objs::Tree::empty())
                    .unwrap()
                    .detach();
                let signature = gix::actor::Signature {
                    name: "fixture".into(),
                    email: "fixture@example.com".into(),
                    time: gix::date::Time::new(1, 0),
                };
                let commit = gix::objs::Commit {
                    tree,
                    parents: Default::default(),
                    author: signature.clone(),
                    committer: signature,
                    encoding: None,
                    message: format!(
                        "commit {index} {}\n",
                        if protected { "sensitive" } else { "normal" }
                    )
                    .into(),
                    extra_headers: Vec::new(),
                };
                let oid = repo.write_object(&commit).unwrap().detach().to_string();
                let selection = read_selection(
                    &support::projection_gate::open_gate(),
                    &RepositoryBinding {
                        repository_id: format!(
                            "repo-{}",
                            root.file_name().unwrap().to_str().unwrap()
                        ),
                        path: root.to_path_buf(),
                    },
                    std::slice::from_ref(&oid),
                    GitReadBounds {
                        max_commits: std::num::NonZeroUsize::new(1).unwrap(),
                        max_object_bytes: std::num::NonZeroU64::new(4096).unwrap(),
                        max_total_object_bytes: std::num::NonZeroU64::new(4096).unwrap(),
                    },
                )
                .unwrap();
                let published = SourcePublisher {
                    kernel: &self.store,
                    domain_id: DOMAIN,
                    scope_id: Some(SCOPE),
                    egress: ProviderEgress::RemoteAllowed,
                    sensitivity: if protected {
                        Sensitivity::Sensitive
                    } else {
                        Sensitivity::Normal
                    },
                }
                .publish(&selection.units[0], self.now)
                .unwrap();
                self._dirs.push(dir);
                published.object_id
            })
            .collect()
    }

    /// Publishes a descriptor of `class` derived from an admitted decision, over evidence with no verified provenance.
    fn publish_derived(
        &self,
        key: &str,
        class: &str,
        identity_field: &str,
        representation: &str,
    ) -> String {
        let object = format!("decision-{key}");
        self.store
            .commit(intent(&format!("decision-{key}")), |envelope| {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("decision-id-{key}"),
                    object_id: object.clone(),
                    domain_id: DOMAIN.to_string(),
                    proposition_id: None,
                    scope_id: Some(SCOPE.to_string()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "architecture".to_string(),
                    payload: DecisionPayload {
                        summary: format!("summary {key}"),
                        rationale: format!("rationale {key}"),
                    },
                    source_kind: "repo".to_string(),
                    source_id: format!("src/{key}"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.record_admission(kernel::AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(object.clone()),
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
        let mut published = None;
        self.store
            .commit(intent(&format!("descriptor-{key}")), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class,
                            identity: &[(identity_field, object.as_str())],
                            revision: "1",
                            representation,
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: DOMAIN,
                        scope_id: Some(SCOPE),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: &format!("claim {key}"),
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
                published = Some(outcome.object_id);
                Ok(String::new())
            })
            .unwrap();
        published.unwrap()
    }

    fn select(
        &self,
        classes: &[OccurrenceClass],
        cursor: Option<&str>,
    ) -> memory_store::memory_reviewer_jobs::FrozenSelectionPage {
        select_review_targets(
            &self.store,
            &self.ledger,
            &SelectionScope {
                project: &ProjectScope::new(PROJECT).unwrap(),
                ledger_project: PROJECT,
                classes,
                policy_versions: &BTreeMap::new(),
            },
            cursor,
            &EvalBudget::unbounded(),
        )
        .unwrap()
    }

    fn targets(page: &memory_store::memory_reviewer_jobs::FrozenSelectionPage) -> Vec<String> {
        page.references
            .iter()
            .map(|inputs| match &inputs.target {
                ReviewTarget::Memory { object_id, .. } => object_id.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }
}

#[test]
fn selection_pages_at_eight_resumes_at_its_cursor_and_wraps_to_a_new_pass() {
    let mut fixture = Fixture::open();
    let mut ids = fixture.publish_commits(11, false);
    ids.sort();
    let first = fixture.select(&[OccurrenceClass::GitCommits], None);
    assert_eq!(first.references.len(), MAX_SELECTION_REFERENCES);
    assert_eq!(
        Fixture::targets(&first),
        ids[..8].to_vec(),
        "a keyset walk in object-id order"
    );
    let cursor = first
        .next_cursor
        .clone()
        .expect("the page stopped before the ninth row");
    let second = fixture.select(&[OccurrenceClass::GitCommits], Some(&cursor));
    assert_eq!(Fixture::targets(&second), ids[8..].to_vec());
    assert_eq!(second.next_cursor, None, "the pass reached the end");
    // A None cursor starts a fresh pass over everything again, so arrivals sorting before any cursor are reached.
    let again = fixture.select(
        &[OccurrenceClass::GitCommits],
        second.next_cursor.as_deref(),
    );
    assert_eq!(Fixture::targets(&again), ids[..8].to_vec());
    // Every reference carries the fixed question and the descriptor's evidence as required and available.
    for inputs in &first.references {
        assert_eq!(inputs.question_template, "extracted_facts");
        assert_eq!(inputs.required_evidence.len(), 1);
        assert!(inputs.required_evidence[0].available);
        assert!(inputs.signals.is_empty());
    }
}

#[test]
fn targets_with_a_job_and_targets_the_remote_fold_denies_are_passed_over() {
    let mut fixture = Fixture::open();
    let mut eligible = fixture.publish_commits(3, false);
    eligible.sort();
    // Sensitive commits are live and locally readable but remotely denied: never selected.
    fixture.publish_commits(2, true);
    let page = fixture.select(&[OccurrenceClass::GitCommits], None);
    assert_eq!(Fixture::targets(&page), eligible);
    // A job at the same causal inputs suppresses the target; a job at other inputs (a policy version) does not.
    let producer = ProducerBinding {
        producer: "other".to_string(),
        firing_id: "f".to_string(),
        ordinal: 0,
    };
    fixture
        .ledger
        .reserve_memory_reviewer_job(PROJECT, &producer, &page.references[0], fixture.now)
        .unwrap();
    let mut versioned = page.references[1].clone();
    versioned.policy_versions = BTreeMap::from([("egress".to_string(), "v2".to_string())]);
    fixture
        .ledger
        .reserve_memory_reviewer_job(PROJECT, &producer, &versioned, fixture.now)
        .unwrap();
    let first_inputs = page.references[0].clone();
    let page = fixture.select(&[OccurrenceClass::GitCommits], None);
    assert_eq!(Fixture::targets(&page), eligible[1..].to_vec());
    // A finished job stays a terminal receipt at those inputs, so the target is not re-selected under the same inputs; a changed policy version is one new job.
    fixture
        .ledger
        .finish_memory_reviewer_job(
            PROJECT,
            &first_inputs.causal_identity().unwrap(),
            MemoryReviewerJobOutcome::Failed,
            fixture.now + 1,
        )
        .unwrap_or_else(|_| panic!("the reserved row finishes"));
    let same = fixture.select(&[OccurrenceClass::GitCommits], None);
    assert_eq!(Fixture::targets(&same), eligible[1..].to_vec());
    let selection = select_review_targets(
        &fixture.store,
        &fixture.ledger,
        &SelectionScope {
            project: &ProjectScope::new(PROJECT).unwrap(),
            ledger_project: PROJECT,
            classes: &[OccurrenceClass::GitCommits],
            policy_versions: &BTreeMap::from([("egress".to_string(), "v3".to_string())]),
        },
        None,
        &EvalBudget::unbounded(),
    )
    .unwrap();
    assert_eq!(
        Fixture::targets(&selection),
        eligible,
        "a new policy version reopens every target once"
    );
}

#[test]
fn the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected()
{
    let fixture = Fixture::open();
    // Canonical-claim and promoted-memory descriptors over evidence without verified provenance are Sensitive by the store's own rule; the remote fold denies them and the walk passes them without a job, through both classes.
    for key in ["a", "b"] {
        fixture.publish_derived(key, "canonical_claims", "object_id", "decision_summary");
    }
    fixture.publish_derived("p", "promoted_memory", "decision_object_id", "summary");
    let tip = fixture.store.tip().unwrap();
    for class in MEMORY_CLASSES {
        let rows = fixture
            .store
            .live_source_descriptors(
                *class,
                tip,
                None,
                std::num::NonZeroUsize::new(8).unwrap(),
                &EvalBudget::unbounded(),
            )
            .unwrap();
        assert!(
            !rows.rows.is_empty(),
            "{class:?} has a live descriptor to walk"
        );
    }
    let page = fixture.select(MEMORY_CLASSES, None);
    assert!(page.references.is_empty());
    assert_eq!(
        page.next_cursor, None,
        "both classes were walked to the end in one page"
    );
    // The class order is fixed: a cursor into the second class resumes there and ends the pass.
    let page = fixture.select(MEMORY_CLASSES, Some("1\u{1f}"));
    assert!(page.references.is_empty());
    assert_eq!(page.next_cursor, None);
    let _ = MAX_EXAMINED_PER_PAGE;
    let _: Option<CausalInputs> = None;
}
