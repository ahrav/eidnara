//! `retrieval.query` through the daemon handler.

mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daemon::dispatch::PreparedOutcome;
use daemon::projection_gates::ProjectionHook;
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, Transition,
};
use daemon::query_route::{
    DenseLimits, EmbedFailure, EmbedResult, LimitsRefusal, QueryEmbedder, QueryRouteLimits,
};
use host_runtime::local_embeddings::inference::InferenceError;
use kernel::{KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::fusion::{FusionParameters, LaneWeights};
use serde_json::{Value, json};
use support::embedding_fixtures::{GENERATION, identity, kernel_incarnation_id};
use support::kernel_daemon::{KernelDaemon, SESSION};
use support::projection_gate::{campaign_json, manifest_json_with, open_gate, write_records};

fn limits() -> QueryRouteLimits {
    QueryRouteLimits {
        query_bytes: NonZeroUsize::new(256).unwrap(),
        probes: NonZeroUsize::new(16).unwrap(),
        lexical_scan_rows: NonZeroUsize::new(256).unwrap(),
        lexical_accepted: NonZeroUsize::new(64).unwrap(),
        lexical_qualifying_matches: NonZeroUsize::new(20_000).unwrap(),
        lexical_rank_budget: NonZeroUsize::new(30_000).unwrap(),
        validation_batch: NonZeroUsize::new(16).unwrap(),
        exact_page_rows: NonZeroUsize::new(16).unwrap(),
        exact_pages: NonZeroUsize::new(4).unwrap(),
        fused_union: NonZeroUsize::new(64).unwrap(),
        result_rows: NonZeroUsize::new(32).unwrap(),
        response_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        deadline_ceiling: Duration::from_secs(20),
        fusion: FusionParameters::new(
            LaneWeights {
                exact: 1.0,
                lexical: 1.0,
                dense: 1.0,
            },
            60.0,
        )
        .unwrap(),
        dense: None,
    }
}

fn request(project: &Path, query: &str) -> Value {
    json!({
        "method": "retrieval.query",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "query": query,
        "remaining_ms": 5_000,
        "destination": "local",
    })
}

fn body(outcome: PreparedOutcome) -> Value {
    match outcome {
        PreparedOutcome::Response(output) => output.json_for_test().unwrap().clone(),
        PreparedOutcome::Error { code, message } => panic!("{code}: {message}"),
        PreparedOutcome::Streamed => panic!("streamed"),
    }
}

fn error_code(outcome: PreparedOutcome) -> String {
    match outcome {
        PreparedOutcome::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn terminal(value: &Value) -> &str {
    assert_eq!(value["kind"], "terminal", "{value}");
    value["terminal"].as_str().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scope_harness_and_disable_are_decided_before_any_candidate_read() {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_owned();

    let mut foreign = request(&project, "id:rule");
    foreign["project_root"] = json!(project.join("elsewhere").to_str().unwrap());
    let refused = body(daemon.outcome(foreign).await);
    assert_eq!(refused["state"]["kind"], "invalid", "{refused}");
    assert_eq!(refused["state"]["reason"], "project_mismatch");

    let mut unbound = request(&project, "id:rule");
    unbound["session_id"] = json!("someone-else");
    assert!(matches!(
        daemon.outcome(unbound).await,
        PreparedOutcome::Error { .. }
    ));

    let disabled = body(daemon.outcome(request(&project, "id:rule")).await);
    assert_eq!(terminal(&disabled), "disabled");

    let mut over = limits();
    over.validation_batch = NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES + 1).unwrap();
    assert_eq!(
        daemon.handler().set_query_route_limits(Some(over)),
        Err(LimitsRefusal::ValidationBatch {
            value: MAX_ELIGIBILITY_CANDIDATES + 1
        })
    );
    assert_eq!(
        terminal(&body(daemon.outcome(request(&project, "id:rule")).await)),
        "disabled",
        "a refused limit set installs nothing"
    );
    daemon
        .handler()
        .set_query_route_limits(Some(limits()))
        .unwrap();

    let mut mismatched = request(&project, "id:rule");
    mismatched["harness"] = json!("another-harness");
    assert_eq!(
        terminal(&body(daemon.outcome(mismatched).await)),
        "unauthorized"
    );

    let mut claimed = request(&project, "id:rule");
    claimed["harness"] = json!("test");
    let passed = body(daemon.outcome(claimed).await);
    assert_eq!(terminal(&passed), "lane_unavailable");
    // No admission records are installed, so the refusal names the missing manifest.
    assert_eq!(passed["reason"], "no_manifest");

    let mut missing = request(&project, "id:rule");
    missing.as_object_mut().unwrap().remove("remaining_ms");
    assert_eq!(error_code(daemon.outcome(missing).await), "invalid_params");

    let mut destination = request(&project, "id:rule");
    destination["destination"] = json!("cloud");
    assert_eq!(
        error_code(daemon.outcome(destination.clone()).await),
        "invalid_params"
    );
    destination["destination"] = json!("remote");
    assert_eq!(
        terminal(&body(daemon.outcome(destination).await)),
        "lane_unavailable"
    );

    let mut destination = request(&project, "id:rule");
    destination["destination"] = json!(7);
    assert_eq!(
        error_code(daemon.outcome(destination).await),
        "invalid_params"
    );

    let long = request(&project, &"x".repeat(300));
    assert_eq!(error_code(daemon.outcome(long).await), "invalid_params");

    let mut unknown = request(&project, "id:rule");
    unknown["occurrence_ids"] = json!(["deadbeef"]);
    assert_eq!(error_code(daemon.outcome(unknown).await), "invalid_params");

    daemon.handler().set_query_route_limits(None).unwrap();
    assert_eq!(
        terminal(&body(daemon.outcome(request(&project, "id:rule")).await)),
        "disabled"
    );
    daemon.shutdown().await;
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A daemon whose scheduled slices converged a family over an empty kernel, with the route enabled.
async fn converged_daemon() -> KernelDaemon {
    converged_daemon_with(&[]).await
}

/// [`converged_daemon`] under a fixture manifest whose `overrides` replace the named limits.
async fn converged_daemon_with(overrides: &[(&str, u64)]) -> KernelDaemon {
    let data = tempfile::tempdir().unwrap();
    let home = data.path().to_owned();
    let kernel_root = home.join("eidnara").join("context");
    {
        let seed = KernelStore::open(kernel_root.join("kernel")).unwrap();
        drop(seed);
    }
    let incarnation = kernel_incarnation_id(&kernel_root);
    let identity = identity(&incarnation);
    write_records(
        &home,
        &manifest_json_with(&identity, &ProjectionHook::ALL, overrides),
        &campaign_json(&identity),
    );
    let request_record = LifecycleRequest {
        transition: Transition::Rebuilding,
        selected_generation: "unregistered".to_owned(),
        kernel_incarnation_id: incarnation,
        consumer: ConsumerBinding {
            consumer_id: "search-lifecycle".to_owned(),
            generation_id: GENERATION.to_owned(),
        },
        cause: Cause::DeletedAfterPruning,
        attempt_id: "rebuild-attempt".to_owned(),
        recovery_target: None,
        allowance: 3,
        deadline: now() + 60_000,
        authorization_ref: None,
    };
    let lifecycle = ProjectionLifecycle::open(&home).unwrap();
    lifecycle
        .record(&open_gate(), &request_record, now())
        .unwrap();

    let daemon = KernelDaemon::start_in(data, None).await;
    daemon
        .handler()
        .set_query_route_limits(Some(limits()))
        .unwrap();
    let started = Instant::now();
    loop {
        if matches!(lifecycle.read(), ControlState::Current(_)) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "scheduled slices did not reach Current: {:?}",
            lifecycle.read()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    daemon
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_running_daemon_serves_the_route_from_its_converged_family() {
    let daemon = converged_daemon().await;
    let project = daemon.project().to_owned();
    let fused = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(fused["kind"], "fused", "{fused}");
    assert_eq!(fused["degraded"], false);
    assert_eq!(fused["truncated"], false);
    assert_eq!(fused["lanes"]["exact"]["status"], "complete");
    assert_eq!(fused["lanes"]["lexical"]["status"], "complete");
    assert_eq!(fused["lanes"]["dense"]["status"], "undeclared");
    assert!(fused["entries"].as_array().unwrap().is_empty());

    let mut claimed = request(&project, "id:rule explicit contract");
    claimed["harness"] = json!("test");
    assert_eq!(body(daemon.outcome(claimed).await)["kind"], "fused");

    let store = daemon.store();
    let canonical = (store.tip().unwrap(), store.lease_epoch());
    daemon.handler().set_query_route_limits(None).unwrap();
    assert_eq!(
        terminal(&body(
            daemon
                .outcome(request(&project, "id:rule explicit contract"))
                .await
        )),
        "disabled"
    );
    assert_eq!((store.tip().unwrap(), store.lease_epoch()), canonical);
    daemon
        .handler()
        .set_query_route_limits(Some(limits()))
        .unwrap();
    let again = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(again, fused, "re-enabling needs no data change");
    assert_eq!((store.tip().unwrap(), store.lease_epoch()), canonical);

    let mut lapsed = request(&project, "id:rule explicit contract");
    lapsed["remaining_ms"] = json!(0);
    assert_eq!(error_code(daemon.outcome(lapsed).await), "invalid_params");
    daemon.shutdown().await;
}

struct ScriptedEmbedder {
    outcome: EmbedResult,
    calls: AtomicUsize,
    delay: Duration,
}

impl QueryEmbedder for ScriptedEmbedder {
    fn embed<'a>(
        &'a self,
        _text: &'a str,
        _deadline: tokio::time::Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = EmbedResult> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.outcome.clone()
        })
    }
}

fn dense_limits() -> QueryRouteLimits {
    let mut limits = limits();
    limits.dense = Some(DenseLimits {
        k: NonZeroUsize::new(8).unwrap(),
        page_rows: NonZeroUsize::new(4).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        unit_norm_tolerance: 1e-3,
    });
    limits
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_query_is_embedded_by_the_lane_before_the_scan_and_the_lane_degrades_typed() {
    let daemon = converged_daemon().await;
    let project = daemon.project().to_owned();
    daemon
        .handler()
        .set_query_route_limits(Some(dense_limits()))
        .unwrap();

    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(answer["kind"], "fused", "{answer}");
    assert_eq!(answer["degraded"], false);
    assert_eq!(answer["lanes"]["dense"]["status"], "complete");

    for reason in ["busy", "starting", "disabled"] {
        let embedder = Arc::new(ScriptedEmbedder {
            outcome: Err(EmbedFailure::Unavailable(reason)),
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        daemon
            .handler()
            .set_query_embedder_for_test(Some(Arc::clone(&embedder) as Arc<dyn QueryEmbedder>));
        let started = Instant::now();
        let answer = body(
            daemon
                .outcome(request(&project, "id:rule explicit contract"))
                .await,
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(answer["kind"], "fused", "{answer}");
        assert_eq!(answer["degraded"], true);
        assert_eq!(answer["lanes"]["dense"]["status"], "unavailable");
        assert_eq!(answer["lanes"]["dense"]["reason"], reason);
        assert_eq!(answer["lanes"]["exact"]["status"], "complete");
        assert_eq!(embedder.calls.load(Ordering::SeqCst), 1);
    }

    let faulted = Arc::new(ScriptedEmbedder {
        outcome: Err(EmbedFailure::Faulted),
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::clone(&faulted) as Arc<dyn QueryEmbedder>));
    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(terminal(&answer), "lane_unavailable");
    assert_eq!(answer["reason"], "embedding_failed");

    let slow = Arc::new(ScriptedEmbedder {
        outcome: Ok(vec![1.0; 8]),
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(400),
    });
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::clone(&slow) as Arc<dyn QueryEmbedder>));
    let mut lapsing = request(&project, "id:rule explicit contract");
    lapsing["remaining_ms"] = json!(200);
    let (outcome, units) = daemon.outcome_observed(lapsing, false).await;
    let answer = body(outcome);
    assert_eq!(
        terminal(&answer),
        "deadline",
        "a deadline that lapses during the embedding await ends the request before any scan"
    );
    assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        units, 0,
        "no scan unit is submitted after the deadline lapsed"
    );

    let queued = Arc::new(ScriptedEmbedder {
        outcome: Ok(vec![1.0; 8]),
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::clone(&queued) as Arc<dyn QueryEmbedder>));
    let (outcome, units) = daemon
        .outcome_observed(request(&project, "id:rule explicit contract"), true)
        .await;
    let answer = body(outcome);
    assert_eq!(terminal(&answer), "cancelled");
    assert_eq!(
        queued.calls.load(Ordering::SeqCst),
        0,
        "a request cancelled before its embedding step starts is not embedded"
    );
    assert_eq!(units, 0, "no scan unit is submitted after cancellation");

    daemon.handler().set_query_embedder_for_test(None);
    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(answer["lanes"]["dense"]["status"], "complete");
    daemon.shutdown().await;
}

/// A request that is one selector has no prose to embed; the handler skips the embedding step and the dense lane stays undeclared while the exact lane serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_selector_only_request_is_never_embedded_and_leaves_the_dense_lane_undeclared() {
    let daemon = converged_daemon().await;
    let project = daemon.project().to_owned();
    daemon
        .handler()
        .set_query_route_limits(Some(dense_limits()))
        .unwrap();
    let embedder = Arc::new(ScriptedEmbedder {
        outcome: Ok(vec![1.0 / 8f32.sqrt(); 8]),
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    });
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::clone(&embedder) as Arc<dyn QueryEmbedder>));

    let answer = body(daemon.outcome(request(&project, "id:rule")).await);
    assert_eq!(answer["kind"], "fused", "{answer}");
    assert_eq!(answer["degraded"], false);
    assert_eq!(answer["lanes"]["exact"]["status"], "complete");
    assert_eq!(answer["lanes"]["lexical"]["status"], "undeclared");
    assert_eq!(answer["lanes"]["dense"]["status"], "undeclared");
    assert_eq!(
        embedder.calls.load(Ordering::SeqCst),
        0,
        "a selector-only request is not embedded"
    );

    let refused = daemon.outcome(request(&project, "   ")).await;
    assert_eq!(error_code(refused), "invalid_params");
    assert_eq!(
        embedder.calls.load(Ordering::SeqCst),
        0,
        "a request the selector classifier refuses is not embedded"
    );

    let mut over_bound: Vec<String> = (0..=limits().probes.get())
        .map(|i| format!("id:rule{i}"))
        .collect();
    over_bound.push("explicit contract".to_string());
    let refused = daemon
        .outcome(request(&project, &over_bound.join(" ")))
        .await;
    assert_eq!(error_code(refused), "invalid_params");
    assert_eq!(
        embedder.calls.load(Ordering::SeqCst),
        0,
        "a request over the probe bound is refused before it is embedded"
    );

    let refused = daemon.outcome(request(&project, "!!!")).await;
    assert_eq!(error_code(refused), "invalid_params");
    let answer = body(daemon.outcome(request(&project, "id:rule,id:other")).await);
    assert_eq!(answer["lanes"]["dense"]["status"], "undeclared", "{answer}");
    assert_eq!(
        embedder.calls.load(Ordering::SeqCst),
        0,
        "punctuation outside selector mentions is not prose and is not embedded"
    );

    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(answer["lanes"]["dense"]["status"], "complete");
    assert_eq!(embedder.calls.load(Ordering::SeqCst), 1);
    daemon.shutdown().await;
}

/// Inference that runs and declares its artifact unusable is a fault, not a busy lane: the request ends `embedding_failed`, and the lane it disabled reports `disabled` to the next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn an_artifact_fault_during_inference_is_embedding_failed_and_the_lane_is_disabled_after() {
    let daemon = converged_daemon().await;
    let project = daemon.project().to_owned();
    daemon
        .handler()
        .set_query_route_limits(Some(dense_limits()))
        .unwrap();
    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(answer["lanes"]["dense"]["status"], "complete", "{answer}");

    daemon
        .engine()
        .fail_next(InferenceError::Artifact("artifact unusable".to_owned()));
    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(terminal(&answer), "lane_unavailable");
    assert_eq!(answer["reason"], "embedding_failed");

    let answer = body(
        daemon
            .outcome(request(&project, "id:rule explicit contract"))
            .await,
    );
    assert_eq!(answer["kind"], "fused", "{answer}");
    assert_eq!(answer["degraded"], true);
    assert_eq!(answer["lanes"]["dense"]["status"], "unavailable");
    assert_eq!(answer["lanes"]["dense"]["reason"], "disabled");
    daemon.shutdown().await;
}

/// Under the production limit set, memories committed to a running daemon reach the route through the claim sources and the qualified projection, and each served claim carries the canonical decision it validates to.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn committed_memories_are_served_with_canonical_references_under_production_limits() {
    let daemon = converged_daemon_with(&[("catchup_lag_commits", 1_000)]).await;
    daemon
        .handler()
        .set_query_route_limits(Some(QueryRouteLimits::production()))
        .unwrap();
    let created = daemon
        .commit(
            "create",
            vec![json!({"op": "insert_decision", "spec": {
                "decision_id": "rule-decision",
                "object_id": "rule",
                "domain_id": "memory",
                "decision_kind": "PROJECT_RULES",
                "payload": {"summary": "Keep the public contract explicit.", "rationale": "because rule"},
                "source_id": "rule-lineage",
                "source_revision": 1,
            }})],
        )
        .await;
    assert_eq!(created["state"]["kind"], "available", "{created}");
    daemon.handler().resume_claim_sources_for_test();
    let project = daemon.project().to_owned();
    let started = Instant::now();
    let served = loop {
        let fused = body(
            daemon
                .outcome(request(&project, "id:rule explicit contract"))
                .await,
        );
        // The family is briefly unavailable while the lifecycle catches up with the commit.
        if fused["kind"] == "fused" && !fused["entries"].as_array().unwrap().is_empty() {
            break fused;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the committed memory never reached the route: {fused}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    for entry in served["entries"].as_array().unwrap() {
        assert_eq!(entry["canonical"]["decision_object_id"], "rule", "{served}");
        assert_eq!(entry["canonical"]["source_revision"], 1, "{served}");
    }
    assert!(serde_json::to_vec(&served).unwrap().len() <= 65_536);
    daemon.shutdown().await;
}

/// The `search_admission` block `session.status` reports.
async fn search_admission(daemon: &KernelDaemon) -> Value {
    let status = daemon
        .call(json!({"method": "session.status", "v": 1, "session_id": SESSION}))
        .await;
    status["search_admission"].clone()
}

/// Polls `session.status` until its `search_admission` block equals `expected`.
async fn await_admission(daemon: &KernelDaemon, expected: Value) {
    let started = Instant::now();
    loop {
        let admission = search_admission(daemon).await;
        if admission == expected {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "search admission stayed {admission}, expected {expected}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// AC2: installed records that are absent, malformed, readable by others, bound to another identity, or carry failed harness evidence enable no hook, and both `session.status` and the route's refusal name the admission failure by its code. Neither carries the records' content.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn refused_records_name_the_admission_failure_in_status_and_route() {
    for (label, expected) in [
        ("absent", "no_manifest"),
        ("malformed", "records_malformed"),
        ("wrongly owned", "records_refused"),
        ("stale identity", ""),
        ("failed harness", "evidence_failed"),
    ] {
        let data = tempfile::tempdir().unwrap();
        let home = data.path().to_owned();
        let kernel_root = home.join("eidnara").join("context");
        drop(KernelStore::open(kernel_root.join("kernel")).unwrap());
        let current = identity(&kernel_incarnation_id(&kernel_root));
        let mut written = None;
        match label {
            "failed harness" => {
                let mut campaign = campaign_json(&current);
                let harness = campaign["harness_runs"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap();
                *harness = json!({"outcome": "failed"});
                let manifest = manifest_json_with(&current, &ProjectionHook::ALL, &[]);
                write_records(&home, &manifest, &campaign);
                written = Some(campaign);
            }
            "malformed" => {
                support::projection_gate::write_record(
                    &home,
                    daemon::projection_admission::MANIFEST_RECORD,
                    b"{\"protocol_version\": sentinel-record-content",
                );
                write_records(
                    &home,
                    &json!({"sentinel-record-content": true}),
                    &campaign_json(&current),
                );
            }
            "wrongly owned" => {
                write_records(
                    &home,
                    &manifest_json_with(&current, &ProjectionHook::ALL, &[]),
                    &campaign_json(&current),
                );
                let manifest = home
                    .join(daemon::projection_admission::ADMISSION_DIR)
                    .join(daemon::projection_admission::MANIFEST_RECORD);
                std::fs::set_permissions(
                    &manifest,
                    std::os::unix::fs::PermissionsExt::from_mode(0o644),
                )
                .unwrap();
            }
            "stale identity" => {
                // Records gathered under another vector generation epoch than the lane's.
                let mut stale = current.clone();
                stale.generation_epoch += 1;
                let campaign = campaign_json(&stale);
                write_records(
                    &home,
                    &manifest_json_with(&stale, &ProjectionHook::ALL, &[]),
                    &campaign,
                );
                written = Some(campaign);
            }
            _ => {}
        }
        let daemon = KernelDaemon::start_in(data, None).await;
        daemon
            .handler()
            .set_query_route_limits(Some(limits()))
            .unwrap();
        // The stale case is refused on the manifest's or the evidence's identity, whichever the gate reaches first.
        let accepts = |reason: &str| match expected {
            "" => ["manifest_identity", "evidence_identity"].contains(&reason),
            expected => reason == expected,
        };
        let started = Instant::now();
        let reason = loop {
            let admission = search_admission(&daemon).await;
            if admission["state"] == "refused" && admission["reason"].as_str().is_some_and(accepts)
            {
                break admission["reason"].as_str().unwrap().to_owned();
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "{label}: admission stayed {admission}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let project = daemon.project().to_owned();
        let refused = body(daemon.outcome(request(&project, "anything")).await);
        assert_eq!(terminal(&refused), "lane_unavailable", "{label}: {refused}");
        assert_eq!(refused["reason"], reason.as_str(), "{label}: {refused}");
        let shown = refused.to_string() + &search_admission(&daemon).await.to_string();
        assert!(
            !shown.contains("sentinel-record-content"),
            "{label}: {shown}"
        );
        if written.is_some() {
            for content in [
                current.tokenizer_fingerprint.as_str(),
                current.embedding_model.as_str(),
                "passed",
            ] {
                assert!(
                    !shown.contains(content),
                    "{label}: the refusal exposes record content {content:?}: {shown}"
                );
            }
        }
        daemon.shutdown().await;
    }
}

/// AC1, AC5: installed records alone, with no recorded request and no test registration helper, register the projection through the daemon's own slice loop; `session.status` moves from refused to admitted and the route serves a fused answer from the registered family. An always-disabled registration would leave the route refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn installed_records_alone_register_a_family_the_route_serves() {
    let data = tempfile::tempdir().unwrap();
    let home = data.path().to_owned();
    let kernel_root = home.join("eidnara").join("context");
    drop(KernelStore::open(kernel_root.join("kernel")).unwrap());
    let current = identity(&kernel_incarnation_id(&kernel_root));
    write_records(
        &home,
        &manifest_json_with(&current, &ProjectionHook::ALL, &[]),
        &campaign_json(&current),
    );
    let daemon = KernelDaemon::start_in(data, None).await;
    daemon
        .handler()
        .set_query_route_limits(Some(limits()))
        .unwrap();
    await_admission(&daemon, json!({"state": "admitted"})).await;
    let lifecycle = ProjectionLifecycle::open(&home).unwrap();
    let started = Instant::now();
    let project = daemon.project().to_owned();
    let fused = loop {
        let answer = body(
            daemon
                .outcome(request(&project, "id:rule explicit contract"))
                .await,
        );
        if answer["kind"] == "fused" {
            break answer;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the registered family never served: {answer}, {:?}",
            lifecycle.read()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(fused["lanes"]["exact"]["status"], "complete", "{fused}");
    let current = loop {
        if let ControlState::Current(current) = lifecycle.read() {
            break current;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the registration never reached Current: {:?}",
            lifecycle.read()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(current.cause, Cause::Registration);
    assert_eq!(
        body(
            daemon
                .outcome(request(&project, "id:rule explicit contract"))
                .await
        )["kind"],
        "fused"
    );
    daemon.shutdown().await;
}
