//! `retrieval.query` through the daemon handler.

mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daemon::dispatch::PreparedOutcome;
use daemon::projection_gates::ProjectionHook;
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, Transition,
};
use daemon::query_route::{LimitsRefusal, QueryRouteLimits};
use kernel::{KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::fusion::{FusionParameters, LaneWeights};
use serde_json::{Value, json};
use support::embedding_fixtures::{GENERATION, identity, kernel_incarnation_id};
use support::kernel_daemon::{KernelDaemon, SESSION};
use support::projection_gate::{campaign_json, manifest_json, open_gate, write_records};

fn limits() -> QueryRouteLimits {
    QueryRouteLimits {
        query_bytes: NonZeroUsize::new(256).unwrap(),
        probes: NonZeroUsize::new(16).unwrap(),
        lexical_scan_rows: NonZeroUsize::new(256).unwrap(),
        lexical_accepted: NonZeroUsize::new(64).unwrap(),
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
    assert_eq!(passed["reason"], "no_family");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_running_daemon_serves_the_route_from_its_converged_family() {
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
        &manifest_json(&identity, &ProjectionHook::ALL),
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
    ProjectionLifecycle::open(&home)
        .unwrap()
        .record(&open_gate(), &request_record, now())
        .unwrap();

    let daemon = KernelDaemon::start_in(data, None).await;
    let project = daemon.project().to_owned();
    daemon
        .handler()
        .set_query_route_limits(Some(limits()))
        .unwrap();
    let started = Instant::now();
    loop {
        if let Ok(lifecycle) = ProjectionLifecycle::open(&home)
            && matches!(lifecycle.read(), ControlState::Current(_))
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "scheduled slices did not reach Current: {:?}",
            ProjectionLifecycle::open(&home).map(|l| l.read())
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
