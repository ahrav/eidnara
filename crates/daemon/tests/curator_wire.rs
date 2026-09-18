//! The context application protocol's review operations against a real daemon: `disabled` until the route's project holds MODULE memories authority, a bounded receipt page that lists an abstention from its row with its reason and no payload, a read that answers `not_selected` for an abstained or unknown receipt, and strict bodies.

mod support;

use std::path::Path;

use daemon::curator::handoff::review_policy_versions;
use daemon::dispatch::PreparedOutcome;
use memory_store::LeaseAcquireOutcome;
use memory_store::curator_jobs::{
    CausalInputs, CuratorJobInput, ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::curator_ledger::{AbstainReason, CuratorBeginOutcome, ReceiptCompletion};
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
fn activate_module_authority(store: &memory_store::MemoryStore, root: &Path) -> u64 {
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

/// A History Summarizer job whose run abstained on a Sensitive subject, recorded in the ledger alone.
fn abstained_receipt(
    store: &memory_store::MemoryStore,
    kernel_incarnation: &str,
    generation: u64,
    now: i64,
) -> String {
    let producer = ProducerBinding {
        producer: "history_summarizer".to_string(),
        firing_id: "ses#1".to_string(),
        ordinal: 1,
    };
    let inputs = CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: kernel_incarnation.to_string(),
            candidate_id: "hs-ses-candidate".to_string(),
            payload_digest: "d".repeat(64),
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
            "acq-1",
            "worker-a",
            0,
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
    store
        .complete_curator_receipt(
            PROJECT,
            &job.causal_identity,
            &claim.claim_id,
            "completion-1",
            "worker-a",
            0,
            receipt.generation,
            kernel_incarnation,
            &ReceiptCompletion::Abstained(AbstainReason::OwnerSensitive),
            now,
        )
        .unwrap();
    job.causal_identity
}

#[tokio::test]
async fn review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it()
 {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_path_buf();
    let store = daemon
        .memory_store()
        .expect("the daemon installed its store");

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
    let kernel_incarnation = daemon
        .store()
        .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::new(
            None,
            std::sync::Arc::default(),
        ))
        .unwrap();
    assert_eq!(
        daemon
            .call(envelope("review.list", &project, json!({})))
            .await,
        json!({ "kind": "page", "items": [], "next": null })
    );
    let identity = abstained_receipt(&store, &kernel_incarnation, generation, now_ms());

    // The abstention lists from its row: identity, generation, outcome, reason, and nothing of the subject.
    let page = daemon
        .call(envelope("review.list", &project, json!({ "limit": 1 })))
        .await;
    assert_eq!(
        page,
        json!({
            "kind": "page",
            "items": [{
                "causal_identity": identity,
                "generation": 1,
                "outcome": "abstained",
                "reason": "owner_sensitive",
                "selected": false,
            }],
            "next": identity,
        })
    );
    assert_eq!(
        daemon
            .call(envelope(
                "review.list",
                &project,
                json!({ "after": identity })
            ))
            .await,
        json!({ "kind": "page", "items": [], "next": null })
    );
    // An oversized limit is clamped, not refused; a full page under the clamp still ends when the rows do.
    assert_eq!(
        daemon
            .call(envelope(
                "review.list",
                &project,
                json!({ "limit": 10_000 })
            ))
            .await["next"],
        Value::Null
    );

    // Nothing was selected, so nothing is readable; an unknown identity answers the same.
    for causal_identity in [identity.clone(), "b".repeat(64)] {
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
    for (method, body) in [
        ("review.list", json!({ "limit": 1, "page": 2 })),
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
