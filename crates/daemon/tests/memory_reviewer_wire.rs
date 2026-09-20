//! The context application protocol's review operations against a real daemon: `disabled` until the route's project holds MODULE memories authority, a bounded outcome page that lists an abstention from its row with its reason and no payload and omits a receipt still in progress, a keyset walk, a read that answers `not_selected` for an abstained, in-progress, or unknown receipt, the selected proposal of a published settlement field for field, and strict bodies.

mod support;

use std::path::Path;

use daemon::dispatch::PreparedOutcome;
use kernel::ReviewProposal;
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, SESSION};
use support::memory_reviewer_publish::{
    PROJECT, abstain, activate_module_authority, begin_job, commit_memory_domain,
    kernel_incarnation, now_ms, publish,
};

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
    let abstained = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        1,
        now,
    );
    abstain(&store, &kernel_incarnation, &abstained, now);
    let in_progress = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        2,
        now,
    );
    let published = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        3,
        now,
    );
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
    // A null limit is the absent one, clamped like an oversized one.
    assert_eq!(
        daemon
            .call(envelope("review.list", &project, json!({ "limit": null })))
            .await,
        all
    );
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

/// An observational route (harness `cli`) reads exactly what an ordinary route on the same project reads, byte for byte, and its malformed bodies keep the shared refusal shapes.
#[tokio::test]
async fn an_observational_route_reads_the_same_outcomes_as_an_ordinary_route() {
    let daemon = KernelDaemon::start().await;
    let root = daemon.project().to_path_buf();
    let store = daemon
        .memory_store()
        .expect("the daemon installed its store");
    let kernel = daemon.store();
    let generation = activate_module_authority(&store, &root);
    commit_memory_domain(&kernel);
    let kernel_incarnation = kernel_incarnation(&kernel);
    let now = now_ms();
    let digest = daemon.project_digest();
    let published = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        1,
        now,
    );
    publish(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        &published,
        now,
    );
    let observer = daemon.bind_another(9, "cli").await;
    let requests = [
        envelope("review.list", &root, json!({ "limit": 1 })),
        envelope(
            "review.read",
            &root,
            json!({ "causal_identity": published.job.causal_identity }),
        ),
    ];
    for request in requests {
        let ordinary = daemon.call(request.clone()).await;
        let observed = daemon.call_on(observer, request.clone()).await;
        assert_eq!(observed, ordinary, "{request}");
    }
    let read = daemon
        .call_on(
            observer,
            envelope(
                "review.read",
                &root,
                json!({ "causal_identity": published.job.causal_identity }),
            ),
        )
        .await;
    assert_eq!(read["kind"], json!("proposal"), "{read}");
    // Malformed bodies keep the shared transport refusal through the observational route.
    for request in [
        envelope(
            "review.read",
            &root,
            json!({ "causal_identity": "not-a-digest" }),
        ),
        envelope("review.list", &root, json!({ "limit": 1, "page": 2 })),
    ] {
        let ordinary = daemon.outcome(request.clone()).await;
        let observed = daemon.outcome_on(observer, request.clone()).await;
        let (
            daemon::dispatch::PreparedOutcome::Error { code, message },
            daemon::dispatch::PreparedOutcome::Error {
                code: observed_code,
                message: observed_message,
            },
        ) = (&ordinary, &observed)
        else {
            panic!("{request}: {ordinary:?} / {observed:?}");
        };
        assert_eq!(code, "invalid_params");
        assert_eq!((code, message), (observed_code, observed_message));
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
    commit_memory_domain(&kernel);
    let kernel_incarnation = kernel_incarnation(&kernel);
    let now = now_ms();
    let digest = daemon.project_digest();
    let published = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        1,
        now,
    );
    publish(
        &kernel,
        &store,
        &digest,
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
