//! Ledger fixtures for MemoryReviewer lifecycle tests over real stores: a project raised to MODULE memories authority, a History Summarizer job claimed with its receipt begun, an abstention recorded from the ledger alone, and a proposal published through the real settlement with nothing disclosed. Every helper writes what production writes and nothing else.

use std::path::Path;

use daemon::memory_reviewer::broker::{EvidenceBroker, QuestionTemplate, RunBinding};
use daemon::memory_reviewer::handoff::{review_binding, review_policy_versions};
use daemon::memory_reviewer::settlement::{RunResult, Settled, Settlement, TaskClaim};
use kernel::{
    KernelStore, ManifestReference, MemoryReviewerHoldBinding, PolicyDependencies, ProposalAction,
    ProposalTarget, ReviewProposal, ReviewQuestionTemplate, Uncertainty,
};
use memory_store::LeaseAcquireOutcome;
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, MemoryReviewerJob, MemoryReviewerJobInput, ProducerBinding, ReserveOutcome,
    ReviewTarget,
};
use memory_store::memory_reviewer_ledger::{
    AbstainReason, AttemptMarker, DispatchOutcome, MemoryReviewerAttemptTerminal,
    MemoryReviewerBeginOutcome, MemoryReviewerReceipt, ReceiptCompletion,
};

pub const PROJECT: &str = "git:proj";

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The route's project becomes the MODULE memories authority `PROJECT`, as the operator's prepare/verify/ack sequence leaves it.
pub fn activate_module_authority(store: &MemoryStore, root: &Path) -> u64 {
    let preparing = store
        .authority_begin_prepare("ctx", PROJECT, "memories")
        .unwrap();
    let checksum = store
        .authority_seed_checksum("ctx", PROJECT, "memories")
        .unwrap();
    store
        .authority_verify_prepare(
            "ctx",
            PROJECT,
            "memories",
            preparing.generation,
            &checksum,
            &checksum,
        )
        .unwrap();
    let module = store
        .authority_ack_prepare("ctx", PROJECT, "memories", preparing.generation)
        .unwrap();
    assert_eq!(module.state, "MODULE");
    store
        .bind_authority_route("ctx", PROJECT, root.to_str().unwrap())
        .unwrap();
    module.generation
}

/// One History Summarizer job at firing `firing`, claimed by `worker-a` with its receipt begun; the claim id and the receipt come back with the job.
pub struct BegunJob {
    pub job: MemoryReviewerJob,
    pub claim_id: String,
    pub receipt: MemoryReviewerReceipt,
}

/// The staged subject binding for `firing`: the handoff stages under the real session id and the chunk ordinal, and the job row carries neither.
fn subject_binding(digest: &str, firing: u64, causal_identity: &str) -> kernel::ReviewBinding {
    review_binding(digest, "memory", "ses", firing, causal_identity)
}

/// Reserves, stages the subject in the Kernel as the handoff does, activates, claims, and begins the receipt of one History Summarizer job.
pub fn begin_job(
    kernel: &KernelStore,
    store: &MemoryStore,
    digest: &str,
    kernel_incarnation: &str,
    generation: u64,
    firing: u64,
    now: i64,
) -> BegunJob {
    let producer = ProducerBinding {
        producer: "history_summarizer".to_string(),
        firing_id: format!("{}#{firing}", "5".repeat(32)),
        ordinal: firing,
    };
    let payload = kernel::ReviewPayload::Subject(kernel::ReviewSubject {
        facts: vec![kernel::ExtractedFact {
            text: format!("bun builds the workspace {firing}"),
            spans: vec![kernel::SourceSpan {
                alias: "s1".to_string(),
                start: 0,
                end: 4,
            }],
        }],
        origins: vec![kernel::SubjectOrigin {
            alias: "s1".to_string(),
            message_id: "m2".to_string(),
            ordinal: 2,
            block_ids: vec!["m2#0".to_string()],
            block_hashes: vec!["0".repeat(64)],
            ranges: vec![kernel::ByteRange { start: 0, end: 4 }],
        }],
    });
    let payload_digest = payload.digest().unwrap();
    let candidate_id = format!("hs-ses-{}", &payload_digest[..32]);
    let inputs = CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: kernel_incarnation.to_string(),
            candidate_id: candidate_id.clone(),
            payload_digest,
        },
        question_template: "extracted_facts".to_string(),
        signals: Vec::new(),
        required_evidence: Vec::new(),
        policy_versions: review_policy_versions(),
    };
    let ReserveOutcome::Reserved(job) = store
        .reserve_memory_reviewer_job(PROJECT, &producer, &inputs, now)
        .unwrap()
    else {
        panic!("fresh inputs reserve")
    };
    let run_id = format!("hs-run-{firing}");
    kernel
        .stage_review_input(kernel::ReviewStagingSpec {
            extraction_run_id: run_id.clone(),
            candidate_id,
            producer: "history_summarizer".to_string(),
            binding: subject_binding(digest, firing, &job.causal_identity),
            payload,
            recorded_at: now,
            queue_deadline_at: job.queue_deadline_ms,
        })
        .unwrap();
    kernel
        .finish_staging_run(&run_id, kernel::StagingTerminalState::Completed, now)
        .unwrap();
    store
        .activate_memory_reviewer_job(
            PROJECT,
            &job.causal_identity,
            &producer,
            &MemoryReviewerJobInput {
                subject: job.target.clone(),
                starting_references: Vec::new(),
                question_template: "extracted_facts".to_string(),
            },
            now,
        )
        .unwrap();
    let LeaseAcquireOutcome::Claim { claim, .. } = store
        .acquire_memory_reviewer_task(
            PROJECT,
            &format!("acq-{firing}"),
            "worker-a",
            i64::try_from(firing).unwrap(),
            i64::try_from(generation).unwrap(),
            &job.causal_identity,
            now,
        )
        .unwrap()
    else {
        panic!("the job is claimed")
    };
    let MemoryReviewerBeginOutcome::Begun(receipt) = store
        .begin_memory_reviewer_receipt(
            PROJECT,
            &job.causal_identity,
            kernel_incarnation,
            &claim.claim_id,
            now,
        )
        .unwrap()
    else {
        panic!("the receipt begins")
    };
    let job = store
        .lookup_memory_reviewer_job(PROJECT, &job.causal_identity)
        .unwrap()
        .unwrap();
    BegunJob {
        job,
        claim_id: claim.claim_id,
        receipt,
    }
}

/// The run abstained on a Sensitive subject, recorded in the ledger alone.
pub fn abstain(store: &MemoryStore, kernel_incarnation: &str, begun: &BegunJob, now: i64) {
    store
        .complete_memory_reviewer_receipt(
            PROJECT,
            &begun.job.causal_identity,
            &begun.claim_id,
            &format!("completion-{}", begun.job.causal_identity),
            "worker-a",
            begun.job.producer.ordinal as i64,
            begun.receipt.generation,
            kernel_incarnation,
            &ReceiptCompletion::Abstained(AbstainReason::OwnerSensitive),
            now,
        )
        .unwrap();
}

pub fn proposal() -> ReviewProposal {
    ReviewProposal {
        action: ProposalAction::Create,
        target: ProposalTarget::StagedCandidate {
            candidate_id: "subject-1".to_string(),
        },
        new_text: Some("the workspace builds with bun".to_string()),
        support: vec![],
        contradictions: vec![],
        limitations: vec!["only the subject itself was available".to_string()],
        uncertainty: Uncertainty::Medium,
        manifest: ManifestReference {
            manifest_id: "manifest-1".to_string(),
            digest: "ab".repeat(32),
        },
        policy_dependencies: PolicyDependencies {
            question_template: ReviewQuestionTemplate::ExtractedFacts,
            disclosed_inputs: vec![],
            uncited_disclosed_inputs: vec![],
            ancestry: vec![],
        },
    }
}

/// The run settled a proposal with nothing disclosed, exactly as the coordinator would after a model proposed from the subject alone: an empty execution hold, the staged proposal, the review transfer, and the fenced completion selecting it.
pub fn publish(
    kernel: &KernelStore,
    store: &MemoryStore,
    digest: &str,
    kernel_incarnation: &str,
    begun: &BegunJob,
    now: i64,
) -> ReviewProposal {
    let hold_binding = MemoryReviewerHoldBinding {
        project_digest: digest.to_string(),
        kernel_incarnation: kernel_incarnation.to_string(),
        memstore_incarnation: store.memory_reviewer_store_incarnation().unwrap(),
        subject: begun.job.causal_identity.clone(),
        generation: begun.receipt.generation,
    };
    let hold = kernel
        .acquire_execution_hold(&hold_binding, &[], begun.receipt.execution_cutoff_ms)
        .unwrap();
    let broker = EvidenceBroker::new(
        RunBinding {
            hold: hold_binding,
            hold_id: hold.hold_id,
            destination: kernel::ArtifactDestination::Remote,
        },
        QuestionTemplate::ExtractedFacts,
    )
    .unwrap();
    let claim = TaskClaim {
        claim_id: begun.claim_id.clone(),
        worker_instance: "worker-a".to_string(),
        slot: begun.job.producer.ordinal as i64,
    };
    // A publication must come from an attempt this generation closed complete; the ledger has no other evidence the selection exists.
    let outcome = store
        .dispatch_memory_reviewer_attempt(
            PROJECT,
            &begun.job.causal_identity,
            begun.receipt.generation,
            &claim.claim_id,
            kernel_incarnation,
            &AttemptMarker {
                body_digest: "b".repeat(64),
                request_bytes: 100,
                provider: "localhost/v1/messages@2023-06-01".to_string(),
                model: "claude-canonical-1".to_string(),
                credential_id: "cred-1".to_string(),
                policy_union_digest: "e".repeat(64),
            },
            (),
            || now,
            |()| (),
        )
        .unwrap();
    let DispatchOutcome::Handed { attempt_index, .. } = outcome else {
        panic!("{outcome:?}");
    };
    store
        .finish_memory_reviewer_attempt(
            PROJECT,
            &begun.job.causal_identity,
            begun.receipt.generation,
            &claim.claim_id,
            attempt_index,
            MemoryReviewerAttemptTerminal::Complete,
            now,
        )
        .unwrap();
    let binding = subject_binding(
        digest,
        begun.job.producer.ordinal,
        &begun.job.causal_identity,
    );
    let settled = Settlement {
        store: kernel,
        ledger: store,
        project: PROJECT,
        binding: &binding,
        claim: &claim,
        now_ms: &move || now,
        before_completion_for_test: None,
    }
    .settle(&broker, RunResult::Proposal(Box::new(proposal())))
    .unwrap();
    assert!(matches!(settled, Settled::Published(_)), "{settled:?}");
    proposal()
}

/// The memory domain every production Kernel commits before a job can exist; holds pin the commit snapshot.
pub fn commit_memory_domain(kernel: &KernelStore) {
    kernel
        .commit(
            kernel::CommitIntent {
                producer: "memory_reviewer-test".to_string(),
                operation_key: "domain".to_string(),
                request_digest: "0".repeat(64),
                actor: "test".to_string(),
                cause: "fixture".to_string(),
            },
            |envelope| {
                envelope.insert_domain(kernel::DomainSpec {
                    domain_id: "memory".to_string(),
                    object_id: "domain-memory".to_string(),
                    name: "memory".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "memory".to_string(),
                    source_revision: 1,
                    sensitivity: kernel::Sensitivity::Normal,
                })?;
                Ok("domain".to_string())
            },
        )
        .unwrap();
}

pub fn kernel_incarnation(kernel: &KernelStore) -> String {
    kernel
        .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::new(
            None,
            std::sync::Arc::default(),
        ))
        .unwrap()
}
