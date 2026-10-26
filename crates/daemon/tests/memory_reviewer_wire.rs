//! The context application protocol's review operations against a real daemon: `disabled` until the route's project holds MODULE memories authority, a bounded outcome page that lists an abstention from its row with its reason and no payload and omits a receipt still in progress, a keyset walk, a read that answers `not_selected` for an abstained, in-progress, or unknown receipt, the selected proposal of a published settlement field for field, and strict bodies.

mod support;

use daemon::dispatch::PreparedOutcome;
use kernel::ReviewProposal;
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, envelope};
use support::memory_reviewer_publish::{
    PROJECT, abstain, activate_module_authority, begin_job, commit_memory_domain,
    kernel_incarnation, now_ms, proposal, publish, publish_proposal,
};

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
    let observer = daemon.bind_another(9, daemon::OBSERVATIONAL_HARNESS).await;
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

/// Writes `name` under the owner-only activation directory of `home`, as the deployment owner does.
fn write_owner_record(home: &std::path::Path, name: &str, value: &Value) {
    use std::os::unix::fs::PermissionsExt;
    let dir = home.join(daemon::memory_reviewer::activation::ACTIVATION_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// `review.accept` end to end: `disabled` until the owner's acceptance record names the live store pair, whatever the disclosure record says; then the selected create proposal settles through the Kernel into one decision, a second operation key replays the same settlement, the first key reused for another target conflicts, removing the record stops acceptance again, and bodies are strict.
#[tokio::test]
async fn review_accept_applies_the_selected_proposal_only_under_the_owners_acceptance_record() {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_path_buf();
    let store = daemon.memory_store().unwrap();
    let kernel = daemon.store();
    let digest = daemon.project_digest();
    let generation = activate_module_authority(&store, &project);
    commit_memory_domain(&kernel);
    let scope = format!("project:{digest}");
    kernel
        .commit(
            kernel::CommitIntent {
                producer: "memory_reviewer-test".to_string(),
                operation_key: "scope".to_string(),
                request_digest: "1".repeat(64),
                actor: "test".to_string(),
                cause: "fixture".to_string(),
            },
            |envelope| {
                // The project scope as the kernel routes materialize it, in their own domain.
                envelope.insert_domain(kernel::DomainSpec {
                    domain_id: "project-scopes".to_string(),
                    object_id: "domain-project-scopes".to_string(),
                    name: "project scopes".to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_id: "project-scopes".to_string(),
                    source_revision: 1,
                    sensitivity: kernel::Sensitivity::Normal,
                })?;
                envelope.insert_scope(kernel::ScopeSpec {
                    scope_id: scope.clone(),
                    object_id: scope.clone(),
                    source_id: scope.clone(),
                    domain_id: "project-scopes".to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: kernel::Sensitivity::Normal,
                    terms: vec![kernel::ScopeTermSpec {
                        dimension: kernel::Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(digest.clone()),
                        ..kernel::ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            },
        )
        .unwrap();
    let kernel_incarnation = kernel_incarnation(&kernel);
    let now = now_ms();
    let begun = begin_job(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        generation,
        1,
        now,
    );
    let memory_store::memory_reviewer_jobs::ReviewTarget::StagedSubject { candidate_id, .. } =
        &begun.job.target
    else {
        panic!("{:?}", begun.job.target)
    };
    let created = publish_proposal(
        &kernel,
        &store,
        &digest,
        &kernel_incarnation,
        &begun,
        now,
        ReviewProposal {
            target: kernel::ProposalTarget::StagedCandidate {
                candidate_id: candidate_id.clone(),
            },
            ..proposal()
        },
    );
    let target = serde_json::to_value(&created.target).unwrap();
    let accept = |key: &str, target: &Value| {
        envelope(
            "review.accept",
            &project,
            json!({
                "causal_identity": begun.job.causal_identity,
                "operation_key": key,
                "target": target,
            }),
        )
    };
    let disabled = json!({ "kind": "terminal", "terminal": "disabled" });

    // A disclosure record admits no acceptance; only the acceptance record does.
    write_owner_record(
        daemon.data_home(),
        daemon::memory_reviewer::activation::IDENTITY_RECORD,
        &json!({ "schema": 1 }),
    );
    let tip = kernel.tip().unwrap();
    assert_eq!(daemon.call(accept("caller-1", &target)).await, disabled);
    let memstore_incarnation = store.memory_reviewer_store_incarnation().unwrap();
    let record = |kernel_incarnation: &str| {
        json!({
            "schema": 1,
            "kernel_incarnation": kernel_incarnation,
            "memstore_incarnation": memstore_incarnation,
            "evidence": "owner review of the actual-evidence record",
        })
    };
    write_owner_record(
        daemon.data_home(),
        daemon::memory_reviewer::activation::ACCEPTANCE_RECORD,
        &record("another-incarnation"),
    );
    assert_eq!(
        daemon.call(accept("caller-1", &target)).await,
        disabled,
        "a record for another store pair is closed"
    );
    assert_eq!(kernel.tip().unwrap(), tip, "a closed gate writes nothing");
    write_owner_record(
        daemon.data_home(),
        daemon::memory_reviewer::activation::ACCEPTANCE_RECORD,
        &record(&kernel_incarnation),
    );

    let settled = daemon.call(accept("caller-1", &target)).await;
    assert_eq!(
        (&settled["kind"], &settled["replayed"]),
        (&json!("settled"), &json!(false)),
        "{settled}"
    );
    let settlement = &settled["settlement"];
    assert_eq!(settlement["action"], json!("create"));
    assert_eq!(settlement["target"], target);
    assert_eq!(settlement["effect"]["kind"], json!("created"));
    assert_eq!(
        settlement["effect"]["admission"]["effective_maturity"],
        json!("candidate")
    );
    let object_id = settlement["effect"]["object_id"].as_str().unwrap();
    let (_, states) = kernel.object_states(&[object_id.to_string()]).unwrap();
    let created_row = states[0].clone().unwrap();
    assert_eq!(
        (
            created_row.object.object_kind.as_str(),
            created_row.scope_id.as_deref()
        ),
        ("decision", Some(scope.as_str())),
        "the decision is scoped to the bound project"
    );

    // Another key replays the same settlement, and so does the committing key; the first key for another target conflicts.
    let replay = daemon.call(accept("caller-2", &target)).await;
    assert_eq!(replay["replayed"], json!(true));
    assert_eq!(&replay["settlement"], settlement);
    let credential_shaped = daemon.call(accept("AKIAQ7RSTUVWXYZ23456", &target)).await;
    assert_eq!(
        (
            &credential_shaped["replayed"],
            &credential_shaped["settlement"]
        ),
        (&json!(true), settlement),
        "a key shaped like a credential is an idempotency key like any other: {credential_shaped}"
    );
    let own = daemon.call(accept("caller-1", &target)).await;
    assert_eq!(
        (&own["replayed"], &own["settlement"]),
        (&json!(true), settlement)
    );
    let other = json!({ "kind": "staged_candidate", "candidate_id": "another" });
    assert_eq!(
        daemon.call(accept("caller-1", &other)).await,
        json!({ "kind": "terminal", "terminal": "conflict" })
    );
    assert_eq!(
        daemon.call(accept("caller-3", &other)).await,
        json!({ "kind": "terminal", "terminal": "changed" }),
        "a target that is not the proposal's cannot redirect the settlement"
    );

    // An unknown job has no selection; a settled proposal replays after its review hold ends.
    let unknown = envelope(
        "review.accept",
        &project,
        json!({ "causal_identity": "b".repeat(64), "operation_key": "k".repeat(128), "target": target }),
    );
    assert_eq!(
        daemon.call(unknown).await,
        json!({ "kind": "terminal", "terminal": "not_selected" })
    );
    let review = kernel::MemoryReviewerHoldBinding {
        project_digest: digest.clone(),
        kernel_incarnation: kernel_incarnation.clone(),
        memstore_incarnation: memstore_incarnation.clone(),
        subject: settlement["proposal_id"].as_str().unwrap().to_string(),
        generation: 1,
    };
    let hold = kernel.lookup_review_hold(&review, now).unwrap().unwrap();
    kernel.release_review_hold(&hold.hold_id, &review).unwrap();
    let after_hold = daemon.call(accept("caller-5", &target)).await;
    assert_eq!(
        (&after_hold["replayed"], &after_hold["settlement"]),
        (&json!(true), settlement)
    );

    // Removing the record is the stop path: acceptance is disabled again, and the recorded decision stands.
    std::fs::remove_file(
        daemon
            .data_home()
            .join(daemon::memory_reviewer::activation::ACTIVATION_DIR)
            .join(daemon::memory_reviewer::activation::ACCEPTANCE_RECORD),
    )
    .unwrap();
    assert_eq!(daemon.call(accept("caller-4", &target)).await, disabled);
    assert!(
        kernel.object_states(&[object_id.to_string()]).unwrap().1[0]
            .as_ref()
            .unwrap()
            .object
            .invalidated_commit_seq
            .is_none()
    );

    // Bodies are strict.
    for body in [
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "k", "target": target, "extra": 1 }),
        json!({ "causal_identity": "z", "operation_key": "k", "target": target }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "", "target": target }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "with space", "target": target }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "k" }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "k".repeat(129), "target": target }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "ké", "target": target }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "k", "target": { "kind": "staged_candidate", "candidate_id": "c", "extra": 1 } }),
        json!({ "causal_identity": begun.job.causal_identity, "operation_key": "k", "target": target, "v": 1 }),
    ] {
        match daemon
            .outcome(envelope("review.accept", &project, body.clone()))
            .await
        {
            PreparedOutcome::Error { code, .. } => assert_eq!(code, "invalid_params", "{body}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}

/// Records one serve of `objects` for `session` under the daemon's project, read at the current tip.
fn serve(daemon: &KernelDaemon, key: &str, session: &str, objects: &[&str], recorded_at: i64) {
    let kernel = daemon.store();
    let served_commit_seq = kernel.tip().unwrap();
    kernel
        .commit(daemon::kernel_route_fixtures::intent(key), |envelope| {
            envelope.insert_use_outcome(kernel::UseOutcomeSpec {
                project_digest: daemon.project_digest(),
                harness: "test".to_owned(),
                session_id: session.to_owned(),
                served_set_digest: kernel::served_set_digest(objects.iter().copied()),
                composition_revision: 1,
                served_commit_seq,
                session_role: kernel::SessionRole::Primary,
                recorded_at,
                decision_object_ids: objects.iter().map(|id| (*id).to_owned()).collect(),
            })?;
            Ok(String::new())
        })
        .unwrap();
}

/// `review.use_outcome` counts the unretired serves of one decision and those a later `contradict` followed, at one snapshot: two serves with a contradiction between them answer (2, 1), a retired serve is not counted, and a decision never served answers (0, 0). It reads through the kernel-route scope alone, so a root without MODULE memories authority, where `review.list` is `disabled`, still answers; an observational route receives the same answer; an unknown or misspelled field is `invalid_params`.
#[tokio::test]
async fn review_use_outcome_counts_serves_and_later_disputes_of_one_decision() {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_path_buf();
    let kernel = daemon.store();
    daemon::kernel_route_fixtures::seed_domain(&kernel);
    let created = daemon
        .commit("create", vec![support::kernel_daemon::insert_decision(1)])
        .await;
    assert_eq!(created["state"]["kind"], "available", "{created}");
    let decision = "decision-object-1";
    let now = now_ms();
    serve(&daemon, "serve-1", "ses-1", &[decision], now);
    let contradicted = daemon
        .commit(
            "contradict",
            vec![json!({"op": "disposition", "object_id": decision, "event": "contradict"})],
        )
        .await;
    assert_eq!(contradicted["state"]["kind"], "available", "{contradicted}");
    serve(&daemon, "serve-2", "ses-2", &[decision], now);
    serve(&daemon, "serve-3", "ses-3", &[decision], 0);
    assert_eq!(
        kernel
            .expire_use_outcomes(kernel::USE_OUTCOME_RETENTION_MS + 1)
            .unwrap()
            .retired,
        1
    );

    let counts = |object_id: &str| {
        envelope(
            "review.use_outcome",
            &project,
            json!({ "object_id": object_id }),
        )
    };
    assert_eq!(
        daemon
            .call(envelope("review.list", &project, json!({})))
            .await,
        json!({ "kind": "terminal", "terminal": "disabled" })
    );
    let answer = daemon.call(counts(decision)).await;
    assert_eq!(
        answer,
        json!({
            "kind": "counts",
            "object_id": decision,
            "served_sessions": 2,
            "disputed_after_serve": 1,
        })
    );
    assert_eq!(
        daemon.call(counts("never-served")).await,
        json!({
            "kind": "counts",
            "object_id": "never-served",
            "served_sessions": 0,
            "disputed_after_serve": 0,
        })
    );
    let observer = daemon.bind_another(9, daemon::OBSERVATIONAL_HARNESS).await;
    assert_eq!(daemon.call_on(observer, counts(decision)).await, answer);
    for body in [
        json!({ "object_id": decision, "limit": 1 }),
        json!({ "object": decision }),
        json!({ "object_id": "" }),
    ] {
        let request = envelope("review.use_outcome", &project, body.clone());
        let PreparedOutcome::Error { code, .. } = daemon.outcome(request).await else {
            panic!("{body}");
        };
        assert_eq!(code, "invalid_params", "{body}");
    }
}
