//! Decision-derived MemoryReviewer subjects over real stores, shared by the worker witnesses and the direct-host fixture's review witness: an admitted decision, the `Sensitive` forms the production claim materializer publishes of it, fixture forms over evidence whose repository provenance the caller supplies, the production selector over one project, the selection slot's frozen-page enqueue, and a scripted provider step.

use std::num::{NonZeroU64, NonZeroUsize};

use daemon::memory_reviewer::selection::{SelectionScope, select_review_targets};
use kernel::KernelStore;
use kernel::source_identity::OccurrenceClass;
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, EnqueueOutcome, EvidenceAvailability, FrozenSelectionPage, ProducerBinding,
    ReviewTarget,
};
use sha2::Digest;

use super::host_ingest::{HostEvidence, ingest};
use super::tls_peer::json_response;

/// The decision [`materialize_decision`] commits.
const MATERIALIZED_DECISION: &str = "decision-materialized";
/// The decision [`publish_fixture_forms`] commits.
const FIXTURE_DECISION: &str = "decision-fixture";
/// A `retain` step citing the first issued alias, the only one a decision-derived subject's run issues.
pub const RETAIN_STEP: &str = r#"{"v":1,"step":{"kind":"propose","action":"retain","new_text":null,"support":[{"alias":"ref-1"}],"contradictions":[],"limitations":["only the form itself was read"],"uncertainty":"medium"}}"#;

pub fn intent(key: &str) -> kernel::CommitIntent {
    kernel::CommitIntent {
        producer: "memory_reviewer-forms".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", sha2::Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "fixture".to_string(),
    }
}

fn admission(object_id: &str) -> kernel::AdmissionRequest {
    kernel::AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(kernel::SourceClass::ExplicitUser),
        taint_class: Some(kernel::TaintClass::UserExplicit),
        event: kernel::AdmissionEvent {
            kind: kernel::EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

/// Inserts and admits one positive-category decision summarized as `summary` in the memory domain under `scope`. Ingest keys artifacts by their bytes, so two decisions whose forms share text share those artifacts and their sensitivity.
pub fn commit_decision(kernel: &KernelStore, object_id: &str, scope: &str, summary: &str) {
    kernel
        .commit(intent(&format!("decision-{object_id}")), |envelope| {
            envelope.insert_decision(kernel::DecisionSpec {
                decision_id: format!("{object_id}-id"),
                object_id: object_id.to_string(),
                domain_id: "memory".to_string(),
                proposition_id: None,
                scope_id: Some(scope.to_string()),
                anchor_id: None,
                evidence_id: None,
                decision_kind: "ARCHITECTURE".to_string(),
                payload: kernel::DecisionPayload {
                    summary: summary.to_string(),
                    rationale: "the lockfile is bun's".to_string(),
                },
                source_kind: "assistant".to_string(),
                source_id: format!("{object_id}-lineage"),
                source_revision: 1,
                sensitivity: kernel::Sensitivity::Normal,
            })?;
            envelope.record_admission(admission(object_id))?;
            Ok(String::new())
        })
        .unwrap();
}

/// Commits [`MATERIALIZED_DECISION`] under `scope`, then runs the production claim materializer over it with the egress `search_lifecycle_owner` gives it, `LocalOnly`. The materializer publishes one `canonical_claims` descriptor per nonempty representation and one `promoted_memory` descriptor, each over an artifact ingest stores `Sensitive` because it has no repository provenance. Returns the decision's object id and the number of descriptors the episode published.
pub fn materialize_decision(kernel: &KernelStore, scope: &str, now: i64) -> (String, usize) {
    use daemon::claim_sources::ClaimMaterializer;
    ClaimMaterializer::register(kernel, now).unwrap();
    commit_decision(
        kernel,
        MATERIALIZED_DECISION,
        scope,
        "the workspace pins its bun version",
    );
    // A host's consumer checkpoint can trail its tip by every commit since the host registered it, so one episode spans far more pages than a fresh store needs.
    let report = ClaimMaterializer::new(kernel, kernel::ProviderEgress::LocalOnly)
        .run_episode(
            kernel::CommitPageBounds {
                max_commits: NonZeroUsize::new(256).unwrap(),
                max_rows: NonZeroUsize::new(1024).unwrap(),
                max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
            },
            now,
        )
        .unwrap();
    (MATERIALIZED_DECISION.to_string(), report.published)
}

/// Fixture only: [`FIXTURE_DECISION`] under `scope` with one `canonical_claims` and one `promoted_memory` descriptor, each over evidence whose repository provenance the caller supplies, which ingest keeps `Normal`. No production path issues provenance to a decision's evidence. Returns the decision's object id and each descriptor's evidence id.
pub fn publish_fixture_forms(kernel: &KernelStore, scope: &str) -> (String, Vec<String>) {
    let decision = FIXTURE_DECISION;
    commit_decision(kernel, decision, scope, "bun builds the workspace");
    let forms = [
        (
            OccurrenceClass::CanonicalClaims,
            "decision_summary",
            "bun builds the workspace",
        ),
        (
            OccurrenceClass::PromotedMemory,
            "summary",
            "the workspace builds with bun",
        ),
    ];
    let evidence = forms
        .iter()
        .map(|(class, representation, text)| {
            let key = class.code();
            let handle = ingest(
                kernel,
                HostEvidence {
                    intent: intent(&format!("fixture-evidence-{key}")),
                    payload: text,
                    evidence_id: format!("evidence-fixture-{key}"),
                    object_id: format!("evidence-object-fixture-{key}"),
                    domain_id: "memory",
                    source_kind: "conversation",
                    source_id: format!("src/fixture-{key}"),
                    repository_id: Some("repo-fixture"),
                },
            );
            kernel
                .commit(intent(&format!("fixture-descriptor-{key}")), |envelope| {
                    let outcome = envelope
                        .publish_source_descriptor(&kernel::SourceDescriptorRequest {
                            occurrence: kernel::source_identity::Occurrence {
                                class: key,
                                identity: &[(class.identity_fields()[0], decision)],
                                revision: "1",
                                representation,
                                span: None,
                            },
                            source_policy: kernel::SourceDescriptorPolicy::Native,
                            domain_id: "memory",
                            scope_id: Some(scope),
                            evidence_id: &handle.evidence_id,
                            artifact_digest: &handle.digest,
                            buffer: text,
                            sensitivity: kernel::Sensitivity::Normal,
                            observed_at: 1,
                        })
                        .unwrap_or_else(|error| panic!("descriptor publication: {error:?}"));
                    // Eligibility judges the descriptor through the reads that serve it, so the fixture admits the descriptor itself.
                    envelope.record_admission(admission(&outcome.object_id))?;
                    Ok(String::new())
                })
                .unwrap();
            handle.evidence_id
        })
        .collect();
    (decision.to_string(), evidence)
}

/// Every live descriptor of `classes` at the tip, in walk order.
pub fn live_descriptors(
    kernel: &KernelStore,
    classes: &[OccurrenceClass],
) -> Vec<(OccurrenceClass, kernel::LiveDescriptor)> {
    let tip = kernel.tip().unwrap();
    classes
        .iter()
        .flat_map(|class| {
            let page = kernel
                .live_source_descriptors(
                    *class,
                    tip,
                    None,
                    NonZeroUsize::new(64).unwrap(),
                    &kernel::applicability::EvalBudget::unbounded(),
                )
                .unwrap();
            assert!(page.next.is_none(), "one page holds the inventory");
            page.rows.into_iter().map(|row| (*class, row))
        })
        .collect()
}

/// The production selector over `classes` for the ledger project `project`, whose Kernel project is `digest`, from a fresh cursor under the production policy versions.
pub fn select(
    kernel: &KernelStore,
    store: &MemoryStore,
    project: &str,
    digest: &str,
    classes: &[OccurrenceClass],
) -> FrozenSelectionPage {
    select_review_targets(
        kernel,
        store,
        &SelectionScope {
            project: &kernel::ProjectScope::new(digest).unwrap(),
            ledger_project: project,
            classes,
            policy_versions: &daemon::memory_reviewer::handoff::review_policy_versions(),
        },
        None,
        &kernel::applicability::EvalBudget::unbounded(),
    )
    .unwrap()
}

/// The inputs the selector freezes for each of `descriptors`, asserted against the selector's own page by the fixture witness: its memory at its revision, the fixed question, its own evidence available, and the production policy versions. A decision that cites no evidence links no rows to them.
pub fn frozen_inputs(
    descriptors: &[(OccurrenceClass, kernel::LiveDescriptor)],
) -> FrozenSelectionPage {
    FrozenSelectionPage {
        references: descriptors
            .iter()
            .map(|(_, row)| CausalInputs {
                target: ReviewTarget::Memory {
                    object_id: row.object_id.clone(),
                    source_revision: row.detail.revision.parse().unwrap(),
                },
                question_template:
                    daemon::memory_reviewer::broker::QuestionTemplate::ExtractedFacts
                        .id()
                        .to_string(),
                signals: vec![],
                required_evidence: vec![EvidenceAvailability {
                    evidence_id: row.detail.evidence_id.clone(),
                    available: true,
                }],
                policy_versions: daemon::memory_reviewer::handoff::review_policy_versions(),
            })
            .collect(),
        next_cursor: None,
    }
}

/// Freezes `page` and enqueues it under the scheduler's selection slot identities: a lease, the frozen page, and one atomic enqueue that completes the slot. Every reference becomes a ready job. `now` names the slot's command, so two enqueues need two instants.
pub fn enqueue_selection(store: &MemoryStore, project: &str, page: &FrozenSelectionPage, now: i64) {
    use daemon::selection_slot;
    let command = format!("{}@{now}", selection_slot::TASK);
    let generation = store
        .next_memory_classifier_scheduler_generation(selection_slot::INSTANCE)
        .unwrap();
    let memory_store::LeaseAcquireOutcome::Claim { claim, .. } = store
        .acquire_memory_classifier_task(
            project,
            &command,
            selection_slot::INSTANCE,
            selection_slot::LEDGER_SLOT,
            generation,
            selection_slot::TASK_ID,
            now,
            now,
        )
        .unwrap()
    else {
        panic!("the selection slot is claimed")
    };
    let frozen = store
        .freeze_selection(project, selection_slot::TASK, &command, page, now)
        .unwrap();
    let enqueued = store
        .enqueue_frozen_selection(
            project,
            &claim.claim_id,
            &format!("{command}:complete"),
            selection_slot::INSTANCE,
            selection_slot::LEDGER_SLOT,
            &frozen,
            &ProducerBinding {
                producer: selection_slot::PRODUCER.to_string(),
                firing_id: command.clone(),
                ordinal: 0,
            },
            now,
        )
        .unwrap();
    assert!(
        matches!(enqueued, EnqueueOutcome::Enqueued { jobs, .. } if jobs == page.references.len()),
        "{enqueued:?}"
    );
}

/// The provider's message answering with one step as its only text block.
pub fn stepping_as(model: &str, step: &str) -> Vec<u8> {
    let text = serde_json::to_string(step).unwrap();
    let body = format!(
        r#"{{"id":"msg_1","type":"message","role":"assistant","model":"{model}","content":[{{"type":"text","text":{text}}}],"stop_reason":"end_turn","usage":{{"input_tokens":3,"output_tokens":4}}}}"#
    );
    json_response("200 OK", &body, "")
}
