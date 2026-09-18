//! The context application protocol's review operations against a real daemon: `disabled` until the route's project holds MODULE memories authority, a bounded outcome page that lists an abstention from its row with its reason and no payload and omits a receipt still in progress, a keyset walk, a read that answers `not_selected` for an abstained, in-progress, or unknown receipt, the selected proposal of a published settlement field for field, and strict bodies.

mod support;

use std::path::Path;
use std::sync::Arc;

use daemon::curator::broker::{EvidenceBroker, QuestionTemplate, RunBinding};
use daemon::curator::handoff::review_policy_versions;
use daemon::curator::settlement::{RunResult, Settled, Settlement, TaskClaim};
use daemon::curator::worker::job_binding;
use daemon::dispatch::PreparedOutcome;
use kernel::{
    CuratorHoldBinding, KernelStore, ManifestReference, PolicyDependencies, ProposalAction,
    ProposalTarget, ReviewProposal, ReviewQuestionTemplate, Uncertainty,
};
use memory_store::LeaseAcquireOutcome;
use memory_store::MemoryStore;
use memory_store::curator_jobs::{
    CausalInputs, CuratorJob, CuratorJobInput, ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::curator_ledger::{
    AbstainReason, AttemptMarker, CuratorAttemptTerminal, CuratorBeginOutcome, CuratorReceipt,
    DispatchOutcome, ReceiptCompletion,
};
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, SESSION};

const PROJECT: &str = "git:proj";

fn envelope(method: &str, project: &Path, body: Value) -> Value {
    let mut request = json!({
        "method": method,
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
    });
    for (key, value) in body.as_object().unwrap() {
        request[key] = value.clone();
    }
    request
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The route's project becomes the MODULE memories authority `PROJECT`, as the operator's prepare/verify/ack sequence leaves it.
fn activate_module_authority(store: &MemoryStore, root: &Path) -> u64 {
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
struct BegunJob {
    job: CuratorJob,
    claim_id: String,
    receipt: CuratorReceipt,
}

fn begin_job(
    store: &MemoryStore,
    kernel_incarnation: &str,
    generation: u64,
    firing: u64,
    now: i64,
) -> BegunJob {
    let producer = ProducerBinding {
        producer: "history_summarizer".to_string(),
        firing_id: format!("ses#{firing}"),
        ordinal: firing,
    };
    let inputs = CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: kernel_incarnation.to_string(),
            candidate_id: format!("hs-ses-candidate-{firing}"),
            payload_digest: format!("{firing:064}"),
        },
        question_template: "extracted_facts".to_string(),
        signals: Vec::new(),
        required_evidence: Vec::new(),
        policy_versions: review_policy_versions(),
    };
    let ReserveOutcome::Reserved(job) = store
        .reserve_curator_job(PROJECT, &producer, &inputs, now)
        .unwrap()
    else {
        panic!("fresh inputs reserve")
    };
    store
        .activate_curator_job(
            PROJECT,
            &job.causal_identity,
            &producer,
            &CuratorJobInput {
                subject: job.target.clone(),
                starting_references: Vec::new(),
                question_template: "extracted_facts".to_string(),
            },
            now,
        )
        .unwrap();
    let LeaseAcquireOutcome::Claim { claim, .. } = store
        .acquire_curator_task(
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
    let CuratorBeginOutcome::Begun(receipt) = store
        .begin_curator_receipt(
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
        .lookup_curator_job(PROJECT, &job.causal_identity)
        .unwrap()
        .unwrap();
    BegunJob {
        job,
        claim_id: claim.claim_id,
        receipt,
    }
}

/// The run abstained on a Sensitive subject, recorded in the ledger alone.
fn abstain(store: &MemoryStore, kernel_incarnation: &str, begun: &BegunJob, now: i64) {
    store
        .complete_curator_receipt(
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

fn proposal() -> ReviewProposal {
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
fn publish(
    kernel: &KernelStore,
    store: &MemoryStore,
    digest: &str,
    kernel_incarnation: &str,
    begun: &BegunJob,
    now: i64,
) -> ReviewProposal {
    let hold_binding = CuratorHoldBinding {
        project_digest: digest.to_string(),
        kernel_incarnation: kernel_incarnation.to_string(),
        memstore_incarnation: store.curator_store_incarnation().unwrap(),
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
        .dispatch_curator_attempt(
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
        .finish_curator_attempt(
            PROJECT,
            &begun.job.causal_identity,
            begun.receipt.generation,
            &claim.claim_id,
            attempt_index,
            CuratorAttemptTerminal::Complete,
            now,
        )
        .unwrap();
    let binding = job_binding(digest, &begun.job).expect("a History Summarizer job has a binding");
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

/// Kernel holds pin the commit snapshot, so the fixture domain commits before any job exists; returns the Kernel incarnation.
fn commit_memory_domain(kernel: &KernelStore) -> String {
    kernel
        .commit(
            kernel::CommitIntent {
                producer: "curator-wire-test".to_string(),
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
    kernel
        .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::new(
            None,
            Arc::default(),
        ))
        .unwrap()
}

#[tokio::test]
async fn review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it()
 {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_path_buf();
    let store = daemon
        .memory_store()
        .expect("the daemon installed its store");
    let kernel = daemon.store();
    let digest = daemon.project_digest();

    // No MODULE authority for the route's root: both operations are disabled, and nothing else is answered.
    for method in ["review.list", "review.read"] {
        let body = if method == "review.read" {
            json!({ "causal_identity": "a".repeat(64) })
        } else {
            json!({})
        };
        assert_eq!(
            daemon.call(envelope(method, &project, body)).await,
            json!({ "kind": "terminal", "terminal": "disabled" }),
            "{method}"
        );
    }

    let generation = activate_module_authority(&store, &project);
    let kernel_incarnation = commit_memory_domain(&kernel);
    assert_eq!(
        daemon
            .call(envelope("review.list", &project, json!({})))
            .await,
        json!({ "kind": "page", "items": [], "next": null })
    );
    let now = now_ms();
    let abstained = begin_job(&store, &kernel_incarnation, generation, 1, now);
    abstain(&store, &kernel_incarnation, &abstained, now);
    let in_progress = begin_job(&store, &kernel_incarnation, generation, 2, now);
    let published = begin_job(&store, &kernel_incarnation, generation, 3, now);
    let expected_proposal = publish(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        &published,
        now,
    );

    // The two completed receipts list in causal-identity order, one per page at limit 1; the receipt still in progress is not an outcome; the abstention carries its reason and nothing else does; no payload appears anywhere.
    let mut identities = [
        (abstained.job.causal_identity.clone(), "abstained", true),
        (published.job.causal_identity.clone(), "complete", false),
    ];
    identities.sort();
    let expected_items: Vec<Value> = identities
        .iter()
        .map(|(identity, outcome, abstained)| {
            let mut item = json!({
                "causal_identity": identity,
                "generation": 1,
                "outcome": outcome,
                "selected": *outcome == "complete",
            });
            if *abstained {
                item["reason"] = json!("owner_sensitive");
            }
            item
        })
        .collect();
    let first = daemon
        .call(envelope("review.list", &project, json!({ "limit": 1 })))
        .await;
    assert_eq!(first["kind"], json!("page"));
    assert_eq!(first["items"], json!(expected_items[..1]));
    assert_eq!(first["next"], json!(identities[0].0));
    // A null cursor is the absent one: the spelling a response's `next` uses for the end of a walk starts the next walk.
    assert_eq!(
        daemon
            .call(envelope(
                "review.list",
                &project,
                json!({ "limit": 1, "after": null })
            ))
            .await,
        first
    );
    let second = daemon
        .call(envelope(
            "review.list",
            &project,
            json!({ "limit": 1, "after": identities[0].0 }),
        ))
        .await;
    assert_eq!(second["items"], json!(expected_items[1..]));
    assert_eq!(
        second["next"],
        json!(identities[1].0),
        "a full page always carries a cursor"
    );
    assert_eq!(
        daemon
            .call(envelope(
                "review.list",
                &project,
                json!({ "limit": 1, "after": identities[1].0 })
            ))
            .await,
        json!({ "kind": "page", "items": [], "next": null })
    );
    // An oversized limit is clamped, not refused; the whole set fits one clamped page, so it ends.
    let all = daemon
        .call(envelope(
            "review.list",
            &project,
            json!({ "limit": 10_000 }),
        ))
        .await;
    assert_eq!(all["items"], json!(expected_items));
    assert_eq!(all["next"], Value::Null);
    let listed = serde_json::to_string(&all).unwrap();
    assert!(!listed.contains("bun"), "no payload text lists");
    assert!(
        !listed.contains(&in_progress.job.causal_identity),
        "a receipt in progress is not an outcome"
    );

    // Only the published receipt reads: its selected proposal as staged, its reference, and the live review hold's expiry. An abstained receipt, one in progress, and an unknown identity are all `not_selected`.
    let read = daemon
        .call(envelope(
            "review.read",
            &project,
            json!({ "causal_identity": published.job.causal_identity }),
        ))
        .await;
    assert_eq!(read["kind"], json!("proposal"));
    assert_eq!(
        read["causal_identity"],
        json!(published.job.causal_identity)
    );
    assert_eq!(
        read["reference"]["database_incarnation_id"],
        json!(kernel_incarnation)
    );
    assert_eq!(
        read["reference"]["payload_digest"].as_str().unwrap().len(),
        64
    );
    assert!(read["reference"]["candidate_id"].is_string());
    let staged: ReviewProposal = serde_json::from_value(read["proposal"].clone()).unwrap();
    assert_eq!(
        (
            staged.action,
            staged.target,
            staged.new_text,
            staged.limitations,
            staged.uncertainty
        ),
        (
            expected_proposal.action,
            expected_proposal.target,
            expected_proposal.new_text,
            expected_proposal.limitations,
            expected_proposal.uncertainty
        )
    );
    assert!(read["review_expires_at"].as_i64().unwrap() > now);
    assert_eq!(
        read.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "causal_identity",
            "kind",
            "proposal",
            "reference",
            "review_expires_at"
        ]
    );
    for causal_identity in [
        abstained.job.causal_identity.clone(),
        in_progress.job.causal_identity.clone(),
        "b".repeat(64),
    ] {
        assert_eq!(
            daemon
                .call(envelope(
                    "review.read",
                    &project,
                    json!({ "causal_identity": causal_identity })
                ))
                .await,
            json!({ "kind": "terminal", "terminal": "not_selected" })
        );
    }

    // Bodies are strict: an unknown field and a malformed identity are transport errors, never a page.
    // A malformed cursor is refused too: `after` that is not a causal identity would otherwise compare
    // as text and skip or return every outcome.
    for (method, body) in [
        ("review.list", json!({ "limit": 1, "page": 2 })),
        ("review.list", json!({ "after": "z" })),
        ("review.list", json!({ "after": "0" })),
        ("review.read", json!({ "causal_identity": "not-a-digest" })),
        ("review.read", json!({})),
    ] {
        match daemon
            .outcome(envelope(method, &project, body.clone()))
            .await
        {
            PreparedOutcome::Error { code, .. } => assert_eq!(code, "invalid_params", "{body}"),
            other => panic!("{method} {body}: {other:?}"),
        }
    }
}

/// Proposal reads use the digest recorded when the proposal was staged, not the newest bound root's.
#[tokio::test]
async fn a_published_proposal_reads_from_every_root_after_a_newer_root_binds() {
    let daemon = KernelDaemon::start().await;
    let first_root = daemon.project().to_path_buf();
    let store = daemon
        .memory_store()
        .expect("the daemon installed its store");
    let kernel = daemon.store();
    let generation = activate_module_authority(&store, &first_root);
    let kernel_incarnation = commit_memory_domain(&kernel);
    let now = now_ms();
    let published = begin_job(&store, &kernel_incarnation, generation, 1, now);
    publish(
        &kernel,
        &store,
        &daemon.project_digest(),
        &kernel_incarnation,
        &published,
        now,
    );

    let second_root = daemon.data_home().join("project-second");
    std::fs::create_dir_all(&second_root).unwrap();
    let second_route = daemon.bind_root(8, &second_root).await;
    store
        .bind_authority_route("ctx", PROJECT, second_root.to_str().unwrap())
        .unwrap();

    let body = json!({ "causal_identity": published.job.causal_identity });
    for (route, root) in [(None, &first_root), (Some(second_route), &second_root)] {
        let request = envelope("review.read", root, body.clone());
        let read = match route {
            Some(route) => daemon.call_on(route, request).await,
            None => daemon.call(request).await,
        };
        assert_eq!(
            read["kind"],
            json!("proposal"),
            "{}: {read}",
            root.display()
        );
        assert_eq!(read["causal_identity"], body["causal_identity"]);
    }
}
