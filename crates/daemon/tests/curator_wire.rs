//! The context application protocol's review operations against a real daemon: `disabled` until the route's project holds MODULE memories authority, a bounded outcome page that lists an abstention from its row with its reason and no payload and omits a receipt still in progress, a keyset walk, a read that answers `not_selected` for an abstained, in-progress, or unknown receipt, the selected proposal of a published settlement field for field, and strict bodies.

mod support;

use std::path::Path;

use daemon::dispatch::PreparedOutcome;
use kernel::ReviewProposal;
use serde_json::{Value, json};
use support::curator_publish::{
    abstain, activate_module_authority, begin_job, commit_memory_domain, kernel_incarnation,
    now_ms, publish,
};
use support::kernel_daemon::{KernelDaemon, SESSION};

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
    commit_memory_domain(&kernel);
    let kernel_incarnation = kernel_incarnation(&kernel);
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
