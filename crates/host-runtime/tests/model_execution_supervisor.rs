//! These tests use paused Tokio time and backend gates to verify deterministic ModelExecution supervisor behavior without wall-clock sleeps.

// ModelExecution conformance drives a LocalEmbeddings primary, which ships only for `linux-x64-gnu`.
#![cfg(target_os = "linux")]
mod support;

use std::sync::Arc;
use std::time::Duration;

use host_runtime::CancellationToken;
use host_runtime::CompositeComponent;
use host_runtime::model_execution::ModelExecutionComponent;
use host_runtime::model_execution::backend::{BackendError, BackendTerminal, ErrorClass, Harness};
use host_runtime::model_execution::config::{self, ModelExecutionLimits, TERMINAL_RETENTION};
use host_runtime::model_execution::protocol::{self, SendRequest};
use host_runtime::model_execution::subprocess::PrivateDir;
use host_runtime::model_execution::subprocess::group_registry::{
    self, GroupRecord, OwnResidue, StateRoot,
};
use host_runtime::model_execution::supervisor::{
    SessionKey, Subscription, Supervisor, SupervisorMetrics,
};

use support::model_execution::{
    ScriptedBackend, open_model_execution_route, send_call, send_params, start_model_execution_host,
};

fn key(session: &str) -> SessionKey {
    SessionKey {
        project_root: "/workspace/project".into(),
        harness: Harness::OpenCode,
        session: session.to_owned(),
    }
}

/// The fingerprint uses the exact serialized request body.
fn send_pair(prompt: &str) -> (SendRequest, Vec<u8>) {
    let request = SendRequest {
        prompt: prompt.to_owned(),
        system: None,
        provider: "prov".to_owned(),
        model: "model-a".to_owned(),
        max_output_tokens: 1_000,
        temperature: Some(0.1),
    };
    let body = serde_json::to_vec(&serde_json::json!({
        "method": "session.send",
        "params": send_params(prompt, None, "prov/model-a"),
    }))
    .expect("body serializes");
    (request, body)
}

fn send(supervisor: &Supervisor, session: &str, prompt: &str) -> String {
    let (request, body) = send_pair(prompt);
    supervisor
        .send(&key(session), request, &body)
        .expect("send admits")
}

/// `until` yields at most 100,000 times, preventing paused-clock tests from spinning indefinitely when `cond` remains false.
async fn until(mut cond: impl FnMut() -> bool, what: &str) {
    for _ in 0..100_000 {
        if cond() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition not reached: {what}");
}

async fn drain_bytes(mut subscription: Subscription) -> Vec<Vec<u8>> {
    let mut units = Vec::new();
    while let Some(unit) = subscription.next().await {
        units.push(unit.to_vec());
    }
    units
}

fn unit_json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("unit is JSON")
}

fn assert_baseline(metrics: SupervisorMetrics, limits: &ModelExecutionLimits, sessions: usize) {
    assert_eq!(metrics.free_command_permits, limits.max_command_callbacks);
    assert_eq!(metrics.free_run_slots, limits.max_active_runs);
    assert_eq!(
        metrics.free_subscriber_permits,
        limits.max_total_subscribers
    );
    assert_eq!(metrics.free_backend_permits, limits.max_backend_processes);
    assert_eq!(metrics.sessions, sessions);
    if sessions == 0 {
        assert_eq!(
            metrics.retained_bytes_available, metrics.retained_bytes_capacity,
            "all retained charges must be released"
        );
        assert_eq!(metrics.live_runs, 0);
        assert_eq!(metrics.tombstones, 0);
    }
}

#[test]
fn default_limits_and_resource_declaration_match_the_fixed_caps() {
    let limits = ModelExecutionLimits::default();
    assert_eq!(limits.max_active_runs, 32);
    assert_eq!(limits.max_command_callbacks, 32);
    assert_eq!(limits.max_subscribers_per_run, 2);
    assert_eq!(limits.max_total_subscribers, 64);
    assert_eq!(limits.max_backend_processes, 8);
    assert_eq!(limits.max_run_replay_bytes, 1024 * 1024);
    assert_eq!(limits.max_retained_bytes, 64 * 1024 * 1024);
    assert_eq!(limits.max_terminal_sessions, 256);
    assert_eq!(limits.terminal_retention, Duration::from_secs(15 * 60));
    assert_eq!(
        host_runtime::model_execution::config::MAX_SEND_BODY_BYTES,
        512 * 1024
    );

    let component = ModelExecutionComponent::new(
        ScriptedBackend::completing("out"),
        support::model_execution::state_root(),
    );
    let resources = component.resources();
    assert_eq!(resources.reserved_pending_requests, 96);
    assert_eq!(resources.reserved_handler_tasks, 96);
    // `Supervisor` charges its 64 MiB retention budget only for retained run data; route metadata, backend capture, tombstones, environment snapshots, and adapter-owned spawn variables are outside that budget.
    // Each term reads its owner (`SubprocessLimits::default`, the identity bounds, the key overhead) so a change to any owner changes the expected value here.
    let capture = host_runtime::model_execution::subprocess::SubprocessLimits::default();
    let identity_bytes =
        (config::MAX_ROUTE_PROJECT_ROOT_BYTES + config::MAX_ROUTE_SESSION_BYTES) as u64;
    assert_eq!(
        resources.retained_resident_bytes,
        limits.max_retained_bytes
            + config::MAX_BOUND_ROUTES as u64
                * (identity_bytes
                    + config::MAX_ROUTE_CREDENTIAL_FINGERPRINTS as u64 * (16 + 64)
                    + 1024)
            + limits.max_backend_processes as u64
                * ((capture.max_stdout_bytes + capture.max_stderr_bytes) as u64 * 5
                    + config::MAX_SEND_BODY_BYTES as u64)
            + limits.max_terminal_sessions as u64
                * (identity_bytes * config::SESSION_IDENTITY_COPIES as u64
                    + config::KEY_META_OVERHEAD_BYTES as u64)
            + (1 + 3 * limits.max_backend_processes as u64) * config::MAX_ENV_SNAPSHOT_BYTES as u64
            + 3 * limits.max_backend_processes as u64
                * (config::MAX_OPENCODE_CONFIG_BYTES as u64 + 8 * 1024)
    );
    assert_eq!(capture.max_stdout_bytes, 4 * 1024 * 1024);
    assert_eq!(capture.max_stderr_bytes, 64 * 1024);
    assert_eq!(config::MAX_ROUTE_PROJECT_ROOT_BYTES, 4096);
    assert_eq!(config::MAX_ROUTE_SESSION_BYTES, 256);
    // One fingerprint row per supported provider: anthropic, amazon-bedrock, gemini, openai.
    assert_eq!(config::MAX_ROUTE_CREDENTIAL_FINGERPRINTS, 4);
    assert_eq!(resources.retained_resident_bytes, 292_782_080);
    assert_eq!(resources.route_class, host_runtime::RouteClass::Reserved);
}

#[tokio::test]
async fn identical_resend_dedups_and_any_byte_difference_conflicts() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let supervisor = Supervisor::new(Arc::clone(&backend) as Arc<_>);
    let (request, body) = send_pair("same prompt");

    let first = supervisor
        .send(&key("s1"), request.clone(), &body)
        .expect("first send admits");
    let second = supervisor
        .send(&key("s1"), request.clone(), &body)
        .expect("identical resend returns the existing run");
    assert_eq!(first, second);

    // Idempotency fingerprints exact body bytes, so bodies differing by one inserted space conflict even when they parse identically.
    let mut spaced = body.clone();
    spaced.insert(1, b' ');
    let respaced = supervisor
        .send(&key("s1"), request, &spaced)
        .expect_err("a byte-different body for the key conflicts");
    assert_eq!(respaced.code, "idempotency_conflict");

    let (different, different_body) = send_pair("different prompt");
    let conflict = supervisor
        .send(&key("s1"), different, &different_body)
        .expect_err("a differing send for the key conflicts");
    assert_eq!(conflict.code, "idempotency_conflict");

    until(|| backend.starts() == 1, "backend starts once").await;
    gate.add_permits(1);
    until(
        || supervisor.status(&key("s1"), &first) == Ok("completed"),
        "run completes",
    )
    .await;
    assert_eq!(backend.starts(), 1, "the resend started no second backend");
}

/// The barrier releases identical `Supervisor::send` calls concurrently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_identical_sends_converge_on_one_run_and_one_backend_start() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let supervisor = Arc::new(Supervisor::new(Arc::clone(&backend) as Arc<_>));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut racers = Vec::new();
    for _ in 0..2 {
        let supervisor = Arc::clone(&supervisor);
        let barrier = Arc::clone(&barrier);
        let (request, body) = send_pair("same prompt");
        racers.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            supervisor.send(&key("s1"), request, &body)
        }));
    }
    let mut run_ids = Vec::new();
    for racer in racers {
        run_ids.push(
            racer
                .await
                .expect("racer joins")
                .expect("both racing identical sends succeed"),
        );
    }
    assert_eq!(run_ids[0], run_ids[1], "both racers observe one shared run");
    assert_eq!(supervisor.metrics().sessions, 1);
    assert_eq!(supervisor.metrics().live_runs, 1);

    gate.add_permits(1);
    until(
        || supervisor.status(&key("s1"), &run_ids[0]) == Ok("completed"),
        "the shared run completes",
    )
    .await;
    assert_eq!(
        backend.starts(),
        1,
        "the race admitted exactly one backend start"
    );
}

#[tokio::test]
async fn status_reports_exact_states_without_aliases() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let limits = ModelExecutionLimits {
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);

    let running = send(&supervisor, "s-run", "p1");
    until(|| backend.starts() == 1, "first run starts").await;
    // The second admitted run remains queued until the held backend permit is released.
    let queued = send(&supervisor, "s-queue", "p2");
    assert_eq!(supervisor.status(&key("s-run"), &running), Ok("running"));
    assert_eq!(supervisor.status(&key("s-queue"), &queued), Ok("queued"));

    supervisor
        .cancel(&key("s-queue"), &queued)
        .await
        .expect("cancel queued run");
    assert_eq!(supervisor.status(&key("s-queue"), &queued), Ok("cancelled"));

    gate.add_permits(1);
    until(
        || supervisor.status(&key("s-run"), &running) == Ok("completed"),
        "gated run completes",
    )
    .await;

    let failing = ScriptedBackend::failing(BackendError {
        class: ErrorClass::Permanent,
        message: "model does not exist".to_owned(),
        retry_after_secs: None,
        provider_code: Some("model_not_found".to_owned()),
    });
    let fail_supervisor = Supervisor::new(failing as Arc<_>);
    let failed = send(&fail_supervisor, "s-fail", "p3");
    until(
        || fail_supervisor.status(&key("s-fail"), &failed) == Ok("failed"),
        "failing run fails",
    )
    .await;

    // Unknown, foreign-incarnation, and alias-shaped IDs return `missing` and never match a live run.
    for id in [
        "",
        "unknown",
        "model_execution-deadbeef00-1",
        "run-1",
        "active",
    ] {
        assert_eq!(
            supervisor.status(&key("s-run"), id),
            Ok("missing"),
            "{id:?}"
        );
    }
}

/// Run IDs are sequential within an incarnation, so another session can guess a live run ID.
/// A route may observe or cancel only runs in its bound session.
/// Runs in another session return `missing`.
/// Cancelling a run outside the bound session is a no-op.
#[tokio::test]
async fn status_and_cancel_are_scoped_to_the_bound_session() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let supervisor = Supervisor::new(Arc::clone(&backend) as Arc<_>);

    let victim = send(&supervisor, "s-victim", "p1");
    until(|| backend.starts() == 1, "victim run starts").await;
    assert_eq!(supervisor.status(&key("s-victim"), &victim), Ok("running"));

    // Different sessions in the same project and sessions in other projects return `missing`.
    assert_eq!(
        supervisor.status(&key("s-attacker"), &victim),
        Ok("missing"),
        "another session must not observe this run's state"
    );
    let foreign_project = SessionKey {
        project_root: "/workspace/other".into(),
        harness: Harness::OpenCode,
        session: "s-victim".to_owned(),
    };
    assert_eq!(
        supervisor.status(&foreign_project, &victim),
        Ok("missing"),
        "another project must not observe this run's state"
    );

    // Cancelling an unknown run is a no-op, so the owning run remains running.
    supervisor
        .cancel(&key("s-attacker"), &victim)
        .await
        .expect("foreign cancel is a no-op");
    assert_eq!(
        supervisor.status(&key("s-victim"), &victim),
        Ok("running"),
        "another session must not interrupt this run"
    );

    // The owning session can still cancel its run.
    supervisor
        .cancel(&key("s-victim"), &victim)
        .await
        .expect("owner cancels");
    assert_eq!(
        supervisor.status(&key("s-victim"), &victim),
        Ok("cancelled")
    );
    gate.add_permits(1);
    supervisor.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn early_and_late_subscribers_replay_byte_identical_units() {
    let (backend, gate) = ScriptedBackend::gated("streamed text");
    let limits = ModelExecutionLimits {
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);

    // The held backend permit keeps the observed run queued.
    send(&supervisor, "s-block", "blocker");
    until(|| backend.starts() == 1, "blocker starts").await;
    let run_id = send(&supervisor, "s-watch", "watched");

    let mut early = supervisor
        .subscribe(&key("s-watch"))
        .expect("early subscriber attaches");
    // A subscriber attached before `run_started` has no output to replay.
    // A subscriber attached before `run_started` waits for that event instead of receiving fabricated output.
    let pending = tokio::time::timeout(Duration::from_millis(50), early.next()).await;
    assert!(pending.is_err(), "early subscriber waits for run_started");

    gate.add_permits(2);
    let mut early_units = Vec::new();
    while let Some(unit) = early.next().await {
        early_units.push(unit.to_vec());
    }
    until(
        || supervisor.status(&key("s-watch"), &run_id) == Ok("completed"),
        "watched run completes",
    )
    .await;

    let late = supervisor
        .subscribe(&key("s-watch"))
        .expect("late subscriber attaches");
    let late_units = drain_bytes(late).await;
    assert_eq!(early_units, late_units, "replay must be byte-identical");
    assert_eq!(early_units.len(), 3);
    assert_eq!(unit_json(&early_units[0])["unit"]["type"], "run_started");
    assert_eq!(
        unit_json(&early_units[1])["unit"]["message"]["content"][0]["text"],
        "streamed text"
    );
    assert_eq!(unit_json(&early_units[2])["unit"]["type"], "run_finished");
}

#[tokio::test]
async fn subscriber_caps_enforce_per_run_and_total_without_leaking_permits() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let limits = ModelExecutionLimits {
        max_total_subscribers: 5,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits.clone());
    for session in ["a", "b", "c"] {
        send(&supervisor, session, session);
    }

    let _a1 = supervisor.subscribe(&key("a")).expect("first on a");
    let _a2 = supervisor.subscribe(&key("a")).expect("second on a");
    let third = supervisor
        .subscribe(&key("a"))
        .expect_err("third on one run is over the per-run cap");
    assert_eq!(third.code, "queue_full");
    // The rejected third subscriber must return its total-pool permit.
    assert_eq!(supervisor.metrics().free_subscriber_permits, 3);

    let _b1 = supervisor.subscribe(&key("b")).expect("first on b");
    let _b2 = supervisor.subscribe(&key("b")).expect("second on b");
    let _c1 = supervisor.subscribe(&key("c")).expect("first on c");
    assert_eq!(supervisor.metrics().free_subscriber_permits, 0);
    // Per-run capacity remains on c, but the total pool is exhausted.
    let total_full = supervisor
        .subscribe(&key("c"))
        .expect_err("subscriber over the total cap fails");
    assert_eq!(total_full.code, "queue_full");

    drop(_a1);
    let _c2 = supervisor
        .subscribe(&key("c"))
        .expect("a released permit re-admits");

    drop((_a2, _b1, _b2, _c1, _c2));
    assert_eq!(
        supervisor.metrics().free_subscriber_permits,
        limits.max_total_subscribers
    );
    gate.add_permits(3);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn thirty_two_blocked_commands_admit_and_command_33_fails_fast() {
    let (backend, gate) = ScriptedBackend::gated_ignoring_cancel("out");
    let supervisor = Arc::new(Supervisor::new(Arc::clone(&backend) as Arc<_>));
    let run_id = send(&supervisor, "s1", "p");
    until(|| backend.starts() == 1, "run starts").await;

    // Each cancel holds one command permit until the backend stops; the fixture ignores cancellation until the gate releases.
    let mut blocked = Vec::new();
    for _ in 0..32 {
        let supervisor = Arc::clone(&supervisor);
        let run_id = run_id.clone();
        blocked.push(tokio::spawn(async move {
            supervisor.cancel(&key("s1"), &run_id).await
        }));
    }
    until(
        || supervisor.metrics().free_command_permits == 0,
        "32 commands hold every permit",
    )
    .await;

    let overflow = supervisor
        .status(&key("s1"), &run_id)
        .expect_err("command 33 fails fast");
    assert_eq!(overflow.code, "queue_full");

    gate.add_permits(1);
    for handle in blocked {
        handle
            .await
            .expect("cancel task joins")
            .expect("blocked cancel settles");
    }
    assert_eq!(supervisor.metrics().free_command_permits, 32);
    // After committing a cancellation terminal, the run ignores the backend's late completion.
    assert_eq!(supervisor.status(&key("s1"), &run_id), Ok("cancelled"));
}

#[tokio::test]
async fn thirty_two_runs_queue_behind_eight_backends_and_run_33_fails_without_state() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let supervisor = Supervisor::new(Arc::clone(&backend) as Arc<_>);
    let mut runs = Vec::new();
    for index in 0..32 {
        runs.push(send(
            &supervisor,
            &format!("s{index}"),
            &format!("p{index}"),
        ));
    }
    until(|| backend.starts() == 8, "exactly eight backends run").await;
    let metrics = supervisor.metrics();
    assert_eq!(metrics.free_run_slots, 0);
    assert_eq!(metrics.free_backend_permits, 0);
    assert_eq!(metrics.live_runs, 32);

    let (request, body) = send_pair("p33");
    let overflow = supervisor
        .send(&key("s33"), request, &body)
        .expect_err("run 33 fails");
    assert_eq!(overflow.code, "queue_full");
    assert_eq!(
        supervisor.metrics().live_runs,
        32,
        "the rejected run must leave no index entry"
    );
    assert_eq!(supervisor.metrics().sessions, 32);

    gate.add_permits(32);
    for (index, run_id) in runs.iter().enumerate() {
        until(
            || supervisor.status(&key(&format!("s{index}")), run_id) == Ok("completed"),
            "queued run completes after permits release",
        )
        .await;
    }
    // Reaping a backend restores its permit, and reaching a terminal state releases its active-run slot.
    assert_eq!(supervisor.metrics().free_backend_permits, 8);
    assert_eq!(supervisor.metrics().free_run_slots, 32);
    assert_eq!(backend.starts(), 32);
}

#[tokio::test]
async fn dropping_a_subscriber_detaches_only_the_waiter() {
    let (backend, gate) = ScriptedBackend::gated("late output");
    let supervisor = Supervisor::new(Arc::clone(&backend) as Arc<_>);
    let run_id = send(&supervisor, "s1", "p");
    until(|| backend.starts() == 1, "run starts").await;

    let mut early = supervisor
        .subscribe(&key("s1"))
        .expect("subscriber attaches");
    let first = early.next().await.expect("run_started replays");
    assert_eq!(unit_json(&first)["unit"]["type"], "run_started");
    drop(early);

    gate.add_permits(1);
    until(
        || supervisor.status(&key("s1"), &run_id) == Ok("completed"),
        "run completes after its waiter detached",
    )
    .await;
    assert_eq!(
        backend.cancels_observed(),
        0,
        "waiter detach must never cancel the backend"
    );
    let replay = drain_bytes(supervisor.subscribe(&key("s1")).expect("re-subscribe")).await;
    assert_eq!(replay.len(), 3, "full replay survives the detach");
}

#[tokio::test]
async fn cancel_covers_queued_and_running_runs_and_stays_idempotent() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let limits = ModelExecutionLimits {
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);

    let running = send(&supervisor, "s-running", "p1");
    until(|| backend.starts() == 1, "first run starts").await;
    let queued = send(&supervisor, "s-queued", "p2");

    supervisor
        .cancel(&key("s-queued"), &queued)
        .await
        .expect("cancel queued");
    assert_eq!(
        supervisor.status(&key("s-queued"), &queued),
        Ok("cancelled")
    );
    // A queued cancelled run replays `run_started` followed by exactly one cancellation terminal.
    let replay = drain_bytes(supervisor.subscribe(&key("s-queued")).expect("subscribe")).await;
    assert_eq!(replay.len(), 2);
    assert_eq!(unit_json(&replay[0])["unit"]["type"], "run_started");
    let terminal = unit_json(&replay[1]);
    assert_eq!(terminal["unit"]["type"], "error");
    assert_eq!(terminal["unit"]["error"]["class"], "permanent");

    supervisor
        .cancel(&key("s-running"), &running)
        .await
        .expect("cancel running");
    assert_eq!(
        supervisor.status(&key("s-running"), &running),
        Ok("cancelled")
    );
    assert_eq!(
        backend.cancels_observed(),
        1,
        "the running backend saw the cancel"
    );
    supervisor
        .cancel(&key("s-running"), &running)
        .await
        .expect("repeated cancel is idempotent");
    assert_eq!(
        supervisor.status(&key("s-running"), &running),
        Ok("cancelled")
    );

    // A queued run does not consume the backend permit it awaits.
    assert_eq!(supervisor.metrics().free_backend_permits, 1);
    assert_eq!(
        supervisor.metrics().free_run_slots,
        ModelExecutionLimits::default().max_active_runs
    );
    drop(gate);
}

#[tokio::test]
async fn completion_cannot_overwrite_a_committed_cancellation() {
    let (backend, gate) = ScriptedBackend::gated_ignoring_cancel("late completion");
    let supervisor = Arc::new(Supervisor::new(Arc::clone(&backend) as Arc<_>));
    let run_id = send(&supervisor, "s1", "p");
    until(|| backend.starts() == 1, "run starts").await;

    let canceller = {
        let supervisor = Arc::clone(&supervisor);
        let run_id = run_id.clone();
        tokio::spawn(async move { supervisor.cancel(&key("s1"), &run_id).await })
    };
    until(
        || supervisor.status(&key("s1"), &run_id) == Ok("cancelled"),
        "cancellation terminal commits",
    )
    .await;

    // The supervisor ignores the backend's completion after committing a cancellation terminal.
    gate.add_permits(1);
    canceller
        .await
        .expect("cancel task joins")
        .expect("cancel settles after the backend stops");
    assert_eq!(supervisor.status(&key("s1"), &run_id), Ok("cancelled"));
    let replay = drain_bytes(supervisor.subscribe(&key("s1")).expect("subscribe")).await;
    let last = unit_json(replay.last().expect("terminal unit"));
    assert_eq!(last["unit"]["type"], "error");
    assert_eq!(
        replay
            .iter()
            .filter(|unit| {
                let ty = unit_json(unit)["unit"]["type"].as_str().map(str::to_owned);
                ty.as_deref() == Some("error") || ty.as_deref() == Some("run_finished")
            })
            .count(),
        1,
        "exactly one in-band terminal"
    );
}

#[tokio::test]
async fn delete_during_running_waits_purges_and_installs_an_idempotent_tombstone() {
    let (backend, _gate) = ScriptedBackend::gated("out");
    let supervisor = Supervisor::new(Arc::clone(&backend) as Arc<_>);
    let run_id = send(&supervisor, "s1", "prompt bytes");
    until(|| backend.starts() == 1, "run starts").await;

    supervisor.delete(&key("s1")).await.expect("delete settles");
    assert_eq!(
        backend.cancels_observed(),
        1,
        "delete must wait for backend cancellation"
    );
    assert_eq!(supervisor.status(&key("s1"), &run_id), Ok("missing"));
    let metrics = supervisor.metrics();
    assert_eq!(metrics.live_runs, 0);
    assert_eq!(metrics.tombstones, 1);
    assert!(
        metrics.retained_bytes_capacity - metrics.retained_bytes_available
            < host_runtime::model_execution::config::TERMINAL_HEADROOM_BYTES,
        "replay and request charges are gone; only tombstone metadata remains"
    );

    let after_first = supervisor.metrics();
    supervisor
        .delete(&key("s1"))
        .await
        .expect("repeated delete is side-effect free");
    assert_eq!(supervisor.metrics(), after_first);

    let (request, body) = send_pair("prompt bytes");
    let resurrect = supervisor
        .send(&key("s1"), request, &body)
        .expect_err("the tombstone blocks resurrection");
    assert_eq!(resurrect.code, "session_deleted");
    // The failed resurrection returns its candidate reservations.
    assert_eq!(supervisor.metrics(), after_first);
}

/// `work_done` proves only task completion; `cancel` and `delete` must return an error until process-tree teardown is confirmed.
#[tokio::test]
async fn unproven_teardown_fails_cancel_and_delete() {
    let backend = ScriptedBackend::with_behavior(|_request, _events, _cancel| {
        Box::pin(async {
            BackendTerminal::FailedUnresolved(BackendError {
                class: ErrorClass::Transient,
                message: "pi backend process group teardown was not confirmed".to_owned(),
                retry_after_secs: None,
                provider_code: None,
            })
        })
    });
    let supervisor = Supervisor::new(backend as Arc<_>);

    let run_id = send(&supervisor, "s-unproven", "p1");
    until(
        || supervisor.status(&key("s-unproven"), &run_id) == Ok("failed"),
        "the unresolved run still commits its failed terminal",
    )
    .await;

    // `cancel` returns `teardown_unconfirmed` after the terminal commits when teardown remains unproven.
    let cancelled = supervisor
        .cancel(&key("s-unproven"), &run_id)
        .await
        .expect_err("cancel cannot claim an unproven teardown");
    assert_eq!(cancelled.code, "teardown_unconfirmed");

    // `delete` purges and tombstones the session before reporting unproven teardown.
    let deleted = supervisor
        .delete(&key("s-unproven"))
        .await
        .expect_err("delete cannot claim an unproven teardown");
    assert_eq!(deleted.code, "teardown_unconfirmed");
    let (request, body) = send_pair("p2");
    let resurrect = supervisor
        .send(&key("s-unproven"), request, &body)
        .expect_err("the tombstone blocks resurrection");
    assert_eq!(
        resurrect.code, "session_deleted",
        "the tombstone must exist despite the reported failure"
    );
}

/// `cancel` and `delete` preserve teardown, cleanup, and record-retention failures after cancellation sets the terminal.
#[tokio::test]
async fn cancellation_winning_the_terminal_still_reports_every_residue_verdict() {
    type Counter = fn(&Supervisor) -> usize;
    let cases: [(&str, BackendTerminal, &str, Option<Counter>); 3] = [
        (
            "s-cancel-unproven",
            BackendTerminal::FailedUnresolved(BackendError {
                class: ErrorClass::Transient,
                message: "process group teardown was not confirmed".to_owned(),
                retry_after_secs: None,
                provider_code: None,
            }),
            "teardown_unconfirmed",
            None,
        ),
        (
            "s-cancel-residue",
            // The message mirrors `merge_cleanup` output for a cancelled run whose private-directory removal failed.
            BackendTerminal::Failed(BackendError {
                class: ErrorClass::Permanent,
                message: "pi run stopped; additionally sensitive temp cleanup failed \
                          (PermissionDenied)"
                    .to_owned(),
                retry_after_secs: None,
                provider_code: None,
            }),
            "cleanup_unconfirmed",
            Some(Supervisor::cleanup_unresolved_runs),
        ),
        (
            "s-cancel-record",
            // The message mirrors `merge_record_retained` output for a run whose record removal failed.
            BackendTerminal::Failed(BackendError {
                class: ErrorClass::Transient,
                message: "pi backend could not remove its crash-ownership record".to_owned(),
                retry_after_secs: None,
                provider_code: None,
            }),
            "record_unconfirmed",
            Some(Supervisor::record_unresolved_runs),
        ),
    ];
    for (session, terminal, code, counter) in cases {
        let backend = ScriptedBackend::with_behavior(move |_request, _events, cancel| {
            let terminal = terminal.clone();
            Box::pin(async move {
                cancel.cancelled().await;
                terminal
            })
        });
        let supervisor = Supervisor::new(backend as Arc<_>);

        let run_id = send(&supervisor, session, "p1");
        until(
            || supervisor.status(&key(session), &run_id) == Ok("running"),
            "the backend starts before the cancel races it",
        )
        .await;

        let cancelled = supervisor
            .cancel(&key(session), &run_id)
            .await
            .expect_err("cancel cannot claim success over the backend's verdict");
        assert_eq!(cancelled.code, code, "{session}");
        assert_eq!(
            supervisor.status(&key(session), &run_id),
            Ok("cancelled"),
            "{session}"
        );

        let deleted = supervisor
            .delete(&key(session))
            .await
            .expect_err("delete reports the same verdict");
        assert_eq!(deleted.code, code, "{session}");
        if let Some(counter) = counter {
            // The counter, not the index, keeps the verdict for shutdown reporting.
            assert_eq!(counter(&supervisor), 1, "{session}");
        }
    }
}

/// Unproven teardown outranks cleanup residue: live descendants can still bill a request, so that verdict must not be hidden.
#[tokio::test]
async fn unproven_teardown_outranks_cleanup_residue() {
    let backend = ScriptedBackend::with_behavior(|_request, _events, _cancel| {
        Box::pin(async {
            // One terminal reports both failures, as `merge_cleanup` produces for an unresolved teardown whose cleanup also failed.
            BackendTerminal::FailedUnresolved(BackendError {
                class: ErrorClass::Transient,
                message: "process group teardown was not confirmed; additionally sensitive \
                          temp cleanup failed (PermissionDenied)"
                    .to_owned(),
                retry_after_secs: None,
                provider_code: None,
            })
        })
    });
    let supervisor = Supervisor::new(backend as Arc<_>);

    let run_id = send(&supervisor, "s-both", "p1");
    until(
        || supervisor.status(&key("s-both"), &run_id) == Ok("failed"),
        "the run commits its failed terminal",
    )
    .await;

    let cancelled = supervisor
        .cancel(&key("s-both"), &run_id)
        .await
        .expect_err("cancel reports the ranking verdict");
    assert_eq!(cancelled.code, "teardown_unconfirmed");
    let deleted = supervisor
        .delete(&key("s-both"))
        .await
        .expect_err("delete reports the ranking verdict");
    assert_eq!(deleted.code, "teardown_unconfirmed");
    // Both latches are set; only the ranking changes which one is reported.
    assert_eq!(supervisor.cleanup_unresolved_runs(), 1);
}

/// A panicking backend leaves its process group unproven: unwinding kills only the leader, so settlement must not report success.
/// `finish` still frees the run slot, so a fresh run in another session admits and reaches its own terminal.
#[tokio::test]
async fn backend_panic_reports_unconfirmed_teardown() {
    let backend = ScriptedBackend::with_behavior(|_request, _events, _cancel| {
        Box::pin(async { panic!("backend bug") })
    });
    let supervisor = Supervisor::new(backend as Arc<_>);

    let run_id = send(&supervisor, "s-panic-teardown", "p1");
    until(
        || supervisor.status(&key("s-panic-teardown"), &run_id) == Ok("failed"),
        "the panicked run commits its failed terminal",
    )
    .await;
    let second = send(&supervisor, "s-after-panic", "p2");
    until(
        || supervisor.status(&key("s-after-panic"), &second) == Ok("failed"),
        "post-panic run still executes",
    )
    .await;

    let cancelled = supervisor
        .cancel(&key("s-panic-teardown"), &run_id)
        .await
        .expect_err("cancel cannot claim a teardown the panic left unproven");
    assert_eq!(cancelled.code, "teardown_unconfirmed");
    let deleted = supervisor
        .delete(&key("s-panic-teardown"))
        .await
        .expect_err("delete reports the same unproven teardown");
    assert_eq!(deleted.code, "teardown_unconfirmed");
    assert_eq!(
        supervisor.shutdown().await,
        2,
        "shutdown must surface every panicked run's unproven teardown"
    );
}

/// `shutdown` reports runs with unproven teardown.
#[tokio::test]
async fn shutdown_counts_runs_with_unproven_teardown() {
    let backend = ScriptedBackend::with_behavior(|_request, _events, cancel| {
        Box::pin(async move {
            cancel.cancelled().await;
            BackendTerminal::FailedUnresolved(BackendError {
                class: ErrorClass::Transient,
                message: "process group teardown was not confirmed".to_owned(),
                retry_after_secs: None,
                provider_code: None,
            })
        })
    });
    let supervisor = Supervisor::new(backend as Arc<_>);
    let run_id = send(&supervisor, "s-shutdown-unproven", "p1");
    until(
        || supervisor.status(&key("s-shutdown-unproven"), &run_id) == Ok("running"),
        "the backend starts before shutdown cancels it",
    )
    .await;

    assert_eq!(
        supervisor.shutdown().await,
        1,
        "shutdown must surface the run whose teardown was never proven"
    );
}

/// The unproven-teardown verdict is counted when the backend returns, so evicting the run from the index later cannot make shutdown report a clean stop.
#[tokio::test]
async fn shutdown_counts_unproven_teardown_after_terminal_cap_eviction() {
    let backend = ScriptedBackend::with_behavior(|request, _events, _cancel| {
        Box::pin(async move {
            if request.session == "s-unproven" {
                return BackendTerminal::FailedUnresolved(BackendError {
                    class: ErrorClass::Transient,
                    message: "process group teardown was not confirmed".to_owned(),
                    retry_after_secs: None,
                    provider_code: None,
                });
            }
            BackendTerminal::Completed {
                finish_reason: host_runtime::model_execution::backend::FinishReason::Completed,
            }
        })
    });
    let limits = ModelExecutionLimits {
        max_terminal_sessions: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(backend as Arc<_>, limits);

    let unproven = send(&supervisor, "s-unproven", "p1");
    until(
        || supervisor.status(&key("s-unproven"), &unproven) == Ok("failed"),
        "the unproven run commits its terminal",
    )
    .await;
    // A second terminal session exceeds the one-session cap and evicts the older unproven run.
    let evictor = send(&supervisor, "s-evictor", "p2");
    until(
        || supervisor.status(&key("s-evictor"), &evictor) == Ok("completed"),
        "the evicting run commits its terminal",
    )
    .await;
    until(
        || supervisor.status(&key("s-unproven"), &unproven) == Ok("missing"),
        "the terminal cap evicts the unproven run",
    )
    .await;

    assert_eq!(
        supervisor.shutdown().await,
        1,
        "eviction must not erase the unproven-teardown verdict"
    );
}

/// A data root of this test's own, so its registry holds only what the test puts there.
fn own_state_root() -> (tempfile::TempDir, StateRoot) {
    let dir = tempfile::tempdir().expect("state root dir");
    let root = StateRoot::resolve(Some(dir.path())).expect("state root resolves");
    (dir, root)
}

/// The terminal of a run whose bounded waits on its private-directory removal and its record removal both ended first, as `merge_cleanup` and `merge_record_retained` word it.
fn removals_outlasted_their_waits() -> BackendTerminal {
    BackendTerminal::Failed(BackendError {
        class: ErrorClass::Transient,
        message: "pi run failed; additionally sensitive temp cleanup failed (TimedOut); \
                  additionally pi backend could not remove its crash-ownership record"
            .to_owned(),
        retry_after_secs: None,
        provider_code: None,
    })
}

/// A component with one failed run that reported both removals.
async fn component_with_removals_that_outlasted_their_waits(
    root: StateRoot,
) -> ModelExecutionComponent {
    let backend = ScriptedBackend::with_behavior(|_request, _events, _cancel| {
        Box::pin(async { removals_outlasted_their_waits() })
    });
    let component = ModelExecutionComponent::new(backend as Arc<_>, root);
    let supervisor = component.supervisor();
    let run_id = send(&supervisor, "s-late-removal", "p1");
    until(
        || supervisor.status(&key("s-late-removal"), &run_id) == Ok("failed"),
        "the run commits its failed terminal",
    )
    .await;
    assert_eq!(
        (
            supervisor.cleanup_unresolved_runs(),
            supervisor.record_unresolved_runs()
        ),
        (1, 1),
        "the run's verdicts are counted when its backend returns"
    );
    component
}

/// A private run directory and a crash-ownership record that name this process, as a run leaves them while its removals are still in flight.
fn own_entries(root: &StateRoot) -> (PrivateDir, GroupRecord) {
    let dir = PrivateDir::create(root, "model_execution-pi").expect("private run dir");
    let nonce = group_registry::new_group_nonce().expect("group nonce");
    let record = GroupRecord::record(root, std::process::id() as i32, &nonce)
        .expect("crash-ownership record");
    (dir, record)
}

/// The removals a run stopped waiting for finish after the run ends. A host whose registry holds no entry of its own then shuts down cleanly, however long ago the run reported them.
#[tokio::test]
async fn shutdown_is_clean_once_the_removals_a_run_waited_on_have_left_the_registry() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    // The run's entries were on disk when its waits ended and are gone now.
    let (dir, record) = own_entries(&root);
    dir.cleanup().expect("the removal lands");
    record.remove().expect("the removal lands");

    CompositeComponent::shutdown(&component)
        .await
        .expect("no entry of this host remains, so shutdown reports no residue");
}

/// Entries that are still in the registry when the settle grace ends are reported, each class under its own count.
#[tokio::test(start_paused = true)]
async fn shutdown_reports_a_private_run_directory_and_a_record_that_remain_in_the_registry() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    let (dir, record) = own_entries(&root);

    let started = tokio::time::Instant::now();
    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("residue on disk fails the shutdown");
    let waited = started.elapsed();
    assert!(
        (Duration::from_millis(4_900)..=Duration::from_secs(5)).contains(&waited),
        "shutdown waits one termination grace for the registry to clear: {waited:?}"
    );
    assert!(
        failure
            .0
            .contains("1 run(s) could not remove their private run files"),
        "{failure}"
    );
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
    dir.cleanup().expect("test cleanup");
    record.remove().expect("test cleanup");
}

/// The run reported both removals, the private directory is gone, and the record is still in the registry: shutdown reports the record alone.
#[tokio::test(start_paused = true)]
async fn shutdown_reports_only_the_class_that_remains_in_the_registry() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    let (dir, record) = own_entries(&root);
    dir.cleanup().expect("the directory's removal lands");

    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("the record on disk fails the shutdown");
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
    assert!(!failure.0.contains("private run files"), "{failure}");
    record.remove().expect("test cleanup");
}

/// The mirror case: the record is gone and the private directory is still in the registry, so shutdown reports the directory alone.
#[tokio::test(start_paused = true)]
async fn shutdown_reports_a_private_run_directory_that_remains_alone() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    let (dir, record) = own_entries(&root);
    record.remove().expect("the record's removal lands");

    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("the directory on disk fails the shutdown");
    assert!(
        failure
            .0
            .contains("1 run(s) could not remove their private run files"),
        "{failure}"
    );
    assert!(!failure.0.contains("crash-ownership record"), "{failure}");
    dir.cleanup().expect("test cleanup");
}

/// The registry read is a blocking job with the settle grace as its bound. A read that is still queued when the grace ends proves nothing, so the runs' verdicts are reported.
#[test]
fn shutdown_keeps_the_runs_verdicts_when_the_registry_read_outlasts_the_grace() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (_dir, root) = own_state_root();
        let component = component_with_removals_that_outlasted_their_waits(root).await;
        // The one blocking thread is busy, so the registry read waits in the pool's queue.
        let (release, parked) = std::sync::mpsc::channel::<()>();
        let busy = tokio::task::spawn_blocking(move || {
            let _ = parked.recv();
        });
        let mut shutdown = std::pin::pin!(CompositeComponent::shutdown(&component));
        let mut waited = Duration::ZERO;
        let outcome = loop {
            tokio::select! {
                biased;
                outcome = &mut shutdown => break outcome,
                () = tokio::time::advance(Duration::from_secs(1)) => waited += Duration::from_secs(1),
            }
            assert!(waited <= Duration::from_secs(10), "the read is bounded");
        };
        release.send(()).expect("the blocking thread is released");
        busy.await.expect("blocking task");

        let failure = outcome.expect_err("an unfinished read keeps the verdicts");
        assert!(
            failure
                .0
                .contains("1 run(s) could not remove their private run files"),
            "{failure}"
        );
        assert!(
            failure
                .0
                .contains("1 crash-ownership record(s) could not be removed"),
            "{failure}"
        );
    });
}

/// A run that shutdown itself stops reports its removals while they can still be in flight, so its verdicts stand whatever the registry holds at that instant.
#[tokio::test]
async fn shutdown_keeps_the_verdicts_of_a_run_it_stopped() {
    let (_dir, root) = own_state_root();
    let backend = ScriptedBackend::with_behavior(|_request, _events, cancel| {
        Box::pin(async move {
            cancel.cancelled().await;
            removals_outlasted_their_waits()
        })
    });
    let component = ModelExecutionComponent::new(backend as Arc<_>, root.clone());
    let supervisor = component.supervisor();
    let run_id = send(&supervisor, "s-stopped", "p1");
    until(
        || supervisor.status(&key("s-stopped"), &run_id) == Ok("running"),
        "the backend starts before shutdown cancels it",
    )
    .await;
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue::default()
    );

    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("verdicts reached during shutdown are reported");
    assert!(
        failure
            .0
            .contains("1 run(s) could not remove their private run files"),
        "{failure}"
    );
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
}

/// A host with no counted verdict reads no registry: entries that name this process, as another component of the same process leaves them while it runs, leave its shutdown clean.
#[tokio::test]
async fn shutdown_without_a_counted_verdict_reads_no_registry() {
    let (_dir, root) = own_state_root();
    let component = ModelExecutionComponent::new(
        Arc::clone(&ScriptedBackend::completing("out")) as Arc<_>,
        root.clone(),
    );
    let (dir, record) = own_entries(&root);

    CompositeComponent::shutdown(&component)
        .await
        .expect("no run reported a removal");
    dir.cleanup().expect("test cleanup");
    record.remove().expect("test cleanup");
}

/// A removal that is still running when shutdown begins has the settle grace to land.
#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_a_removal_that_lands_inside_the_settle_grace() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    let (dir, record) = own_entries(&root);
    let removal = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        dir.cleanup().expect("the removal lands");
        record.remove().expect("the removal lands");
    });

    CompositeComponent::shutdown(&component)
        .await
        .expect("the registry cleared inside the grace");
    removal.await.expect("removal task");
}

/// A registry that cannot be read proves nothing about what remains, so the runs' own verdicts are reported.
#[tokio::test]
async fn shutdown_keeps_the_runs_verdicts_when_the_registry_cannot_be_read() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    // A regular file where the run root belongs fails the registry's hardened traversal.
    let runs = root.run_root().expect("run root");
    std::fs::remove_dir(&runs).expect("empty run root");
    std::fs::write(&runs, b"").expect("file in the run root's place");
    assert!(group_registry::own_residue(&root).is_err());

    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("an unreadable registry keeps the verdicts");
    assert!(
        failure
            .0
            .contains("1 run(s) could not remove their private run files"),
        "{failure}"
    );
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
}

/// A published record this host cannot interpret may be the one whose removal a run waited on, so the registry read answers unknown and the runs' verdicts are reported. The startup sweep keeps the same record for the same reason.
#[tokio::test]
async fn shutdown_keeps_the_runs_verdicts_when_a_published_record_cannot_be_interpreted() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root.clone()).await;
    let (dir, record) = own_entries(&root);
    let registry = dir
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .expect("registry dir")
        .to_owned();
    dir.cleanup().expect("the removal lands");
    record.remove().expect("the removal lands");
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue::default()
    );
    // A record still being written sits under a dot-prefixed name and is no published record.
    std::fs::write(registry.join(".unpublished.tmp"), b"v4\nno record\n").expect("temp");
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue::default()
    );
    std::fs::write(registry.join("damaged"), b"v4\nno record\n").expect("damaged record");
    assert!(group_registry::own_residue(&root).is_err());

    let failure = CompositeComponent::shutdown(&component)
        .await
        .expect_err("a record of unknown owner keeps the verdicts");
    assert!(
        failure
            .0
            .contains("1 run(s) could not remove their private run files"),
        "{failure}"
    );
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
}

/// The registry read interprets a record whole and through one pinned file, as the startup sweep does. A published symlink to a regular file and a record longer than the read cap both fail the read, so shutdown keeps the verdicts it counted.
#[test]
fn a_published_symlink_and_a_record_past_the_read_cap_fail_the_registry_read() {
    use sha2::{Digest, Sha256};

    let (state, root) = own_state_root();
    let (dir, record) = own_entries(&root);
    let registry = dir
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .expect("registry dir")
        .to_owned();
    let record_path = std::fs::read_dir(&registry)
        .expect("registry lists")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.is_file())
        .expect("own record");
    let record_text = std::fs::read_to_string(&record_path).expect("record text");
    dir.cleanup().expect("own run dir removed");
    record.remove().expect("own record removed");
    let clear = || {
        assert_eq!(
            group_registry::own_residue(&root).expect("registry reads"),
            OwnResidue::default()
        );
    };
    clear();

    // A symlink to a directory resolves to no record and is passed over; one to a regular file is read and refused.
    let linked = registry.join("linked");
    std::os::unix::fs::symlink(state.path(), &linked).expect("symlink to a directory");
    clear();
    std::fs::remove_file(&linked).expect("symlink removed");
    let target = state.path().join("target");
    std::fs::write(&target, &record_text).expect("link target");
    std::os::unix::fs::symlink(&target, &linked).expect("symlink to a file");
    assert!(group_registry::own_residue(&root).is_err());
    std::fs::remove_file(&linked).expect("symlink removed");
    clear();

    // This process's record, padded with a body line so that the record and its checksum end exactly at `length` bytes.
    let padded = |length: usize| {
        let mut lines: Vec<&str> = record_text.lines().collect();
        lines.pop();
        let mut body = lines.join("\n");
        body.push('\n');
        let padding = length - body.len() - 1 - 64 - 1;
        body.push_str(&"x".repeat(padding));
        body.push('\n');
        let checksum: String = Sha256::digest(body.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{body}{checksum}\n")
    };
    const CAP: usize = 64 * 1024;
    let at_cap = padded(CAP);
    assert_eq!(at_cap.len(), CAP);
    let oversized = registry.join("oversized");
    std::fs::write(&oversized, &at_cap).expect("record at the cap");
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue {
            run_dirs: 0,
            records: 1
        },
        "a record of exactly the cap is read whole"
    );
    // The same valid record with bytes after it is longer than the cap, and so is a valid record one byte longer.
    std::fs::write(&oversized, format!("{at_cap}trailing\n")).expect("record past the cap");
    assert!(group_registry::own_residue(&root).is_err());
    std::fs::write(&oversized, padded(CAP + 1)).expect("record one byte past the cap");
    assert!(group_registry::own_residue(&root).is_err());
}

/// A credential refresh whose helper record's removal outlasted its wait. It reports at once until `until_cancelled` is set, and from then on when shutdown cancels it. `started` counts the refreshes begun.
struct RecordRetainingRefresh {
    until_cancelled: Arc<std::sync::atomic::AtomicBool>,
    started: Arc<std::sync::atomic::AtomicUsize>,
}

impl host_runtime::model_execution::aws_refresh::Refresh for RecordRetainingRefresh {
    fn refresh(
        &self,
        _: host_runtime::model_execution::aws_refresh::RefreshRequest,
        cancel: CancellationToken,
    ) -> host_runtime::model_execution::aws_refresh::RefreshFuture {
        use host_runtime::model_execution::aws_transaction::{RenewalEvidence, TransactionOutcome};
        let until_cancelled = self
            .until_cancelled
            .load(std::sync::atomic::Ordering::SeqCst);
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if until_cancelled {
                cancel.cancelled().await;
            }
            TransactionOutcome {
                identity: Some(source_identity()),
                row: None,
                successor: None,
                observation: None,
                renewal: RenewalEvidence::NotStarted,
                lost_succession: false,
                record_retained: true,
                failure: None,
            }
        })
    }
}

/// A component over `root` whose profile source runs `RecordRetainingRefresh`.
struct RefreshingComponent {
    component: ModelExecutionComponent,
    owner: host_runtime::model_execution::aws_refresh::SourceOwner,
    until_cancelled: Arc<std::sync::atomic::AtomicBool>,
    started: Arc<std::sync::atomic::AtomicUsize>,
}

fn component_with_record_retaining_refresh(root: StateRoot) -> RefreshingComponent {
    use host_runtime::model_execution::aws_refresh::{SourceOwner, SystemClock};
    use host_runtime::model_execution::source_health::{
        SourceHealthCell, SourceKind, SourceObservation,
    };
    let until_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresh = RecordRetainingRefresh {
        until_cancelled: Arc::clone(&until_cancelled),
        started: Arc::clone(&started),
    };
    let health = SourceHealthCell::new(SourceObservation::unknown(SourceKind::Profile));
    let owner = SourceOwner::new(refresh, SystemClock, source_identity(), health);
    let component = ModelExecutionComponent::new(
        Arc::clone(&ScriptedBackend::completing("out")) as Arc<_>,
        root,
    )
    .with_source_owner(Some(owner.clone()));
    RefreshingComponent {
        component,
        owner,
        until_cancelled,
        started,
    }
}

/// Runs one refresh of `owner` to its end.
async fn refresh_once(owner: &host_runtime::model_execution::aws_refresh::SourceOwner) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let _ = owner.acquire(deadline, &CancellationToken::new()).await;
}

/// A credential refresh's helper is a run of its own with a record in the same registry. Its removal verdict follows the rule of a run's: a registry that holds no record of this host leaves the shutdown clean.
#[tokio::test]
async fn shutdown_is_clean_once_a_refreshs_late_record_removal_has_landed() {
    let (_dir, root) = own_state_root();
    let host = component_with_record_retaining_refresh(root);
    refresh_once(&host.owner).await;
    assert_eq!(host.owner.records_retained(), 1);

    CompositeComponent::shutdown(&host.component)
        .await
        .expect("no record of this host remains");
}

#[tokio::test(start_paused = true)]
async fn shutdown_reports_a_refreshs_record_that_remains_in_the_registry() {
    let (_dir, root) = own_state_root();
    let host = component_with_record_retaining_refresh(root.clone());
    refresh_once(&host.owner).await;
    let (dir, record) = own_entries(&root);
    dir.cleanup().expect("no run directory remains");

    let failure = CompositeComponent::shutdown(&host.component)
        .await
        .expect_err("the record on disk fails the shutdown");
    assert!(
        failure
            .0
            .contains("1 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
    assert!(!failure.0.contains("AWS credential refresh"), "{failure}");
    record.remove().expect("test cleanup");
}

/// A refresh that shutdown itself stops reports its record while the removal can still be in flight, so its verdict stands. An earlier refresh of the same owner has reported a record whose removal has landed, so the stopped refresh's verdict is the owner's second.
#[tokio::test(start_paused = true)]
async fn shutdown_keeps_the_record_verdict_of_a_refresh_it_stopped() {
    let (_dir, root) = own_state_root();
    let host = component_with_record_retaining_refresh(root.clone());
    refresh_once(&host.owner).await;
    assert_eq!(host.owner.records_retained(), 1);
    // The first refresh returned no credential row, so the owner paces the next one.
    tokio::time::advance(Duration::from_secs(600)).await;
    host.until_cancelled
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let waiter = tokio::spawn({
        let owner = host.owner.clone();
        async move { refresh_once(&owner).await }
    });
    until(
        || host.started.load(std::sync::atomic::Ordering::SeqCst) == 2,
        "the second refresh starts before shutdown cancels it",
    )
    .await;

    let failure = CompositeComponent::shutdown(&host.component)
        .await
        .expect_err("a verdict reached during shutdown is reported");
    assert!(
        failure
            .0
            .contains("2 crash-ownership record(s) could not be removed"),
        "{failure}"
    );
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue::default()
    );
    waiter.await.expect("waiter");
}

/// A shutdown request reaches a host whose earlier run reported both removals and whose registry has since cleared: the shutdown callback returns, and the host ends without a lifecycle failure.
#[tokio::test]
async fn host_shutdown_is_graceful_once_a_runs_late_removals_have_landed() {
    let (_dir, root) = own_state_root();
    let component = component_with_removals_that_outlasted_their_waits(root).await;
    let host = start_model_execution_host(component).await;

    host.shutdown().await.expect("graceful shutdown");
}

/// A registry shared with another host incarnation counts only the entries that name this process: a run directory by boot, PID, and start time, and a record by the owner PID and start time it carries.
#[test]
fn entries_another_host_owns_are_no_residue_of_this_one() {
    use sha2::{Digest, Sha256};

    let (_dir, root) = own_state_root();
    let (dir, record) = own_entries(&root);
    let own = OwnResidue {
        run_dirs: 1,
        records: 1,
    };
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        own
    );

    let registry = dir
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .expect("registry dir")
        .to_owned();
    let record_text = std::fs::read_dir(&registry)
        .expect("registry lists")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.is_file())
        .map(|path| std::fs::read_to_string(path).expect("record text"))
        .expect("own record");
    let own_pid = std::process::id();
    let (_, own_start) = record_text
        .lines()
        .nth(3)
        .and_then(|owner| owner.split_once(' '))
        .expect("owner line");
    let own_start: u64 = own_start.parse().expect("owner start");
    // A record like this process's own, written for the owner `pid` that started at `start`. `edit` changes its lines before the checksum is taken.
    let forge_with = |name: &str, pid: u32, start: u64, edit: &dyn Fn(&mut Vec<String>)| {
        let mut lines: Vec<String> = record_text.lines().map(str::to_owned).collect();
        lines.pop();
        lines[3] = format!("{pid} {start}");
        edit(&mut lines);
        let mut body = lines.join("\n");
        body.push('\n');
        let checksum: String = Sha256::digest(body.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = registry.join(name);
        std::fs::write(&path, format!("{body}{checksum}\n")).expect("forged record");
        path
    };
    let forge = |name: &str, pid: u32, start: u64| forge_with(name, pid, start, &|_| {});
    // The forged form is one the registry reads: a second record of this owner is counted.
    let twin = forge("twin", own_pid, own_start);
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue { records: 2, ..own }
    );
    std::fs::remove_file(twin).expect("twin removed");

    // A record still being written sits under a dot-prefixed name until its rename publishes it.
    forge(".unpublished.tmp", own_pid, own_start);
    forge("other-pid", own_pid + 1, own_start);
    forge("other-start", own_pid, own_start + 1);
    // A record of another boot names another owner even with this process's PID and start time, and whatever its schema version.
    forge_with("other-boot", own_pid, own_start, &|lines| {
        lines[1].push('x')
    });
    forge_with("other-boot-other-version", own_pid, own_start, &|lines| {
        lines[0] = "v9".to_owned();
        lines[1].push('x');
    });
    // The directory name carries `<boot>-<pid>-<start>-<nonce>` after its prefix.
    let name = dir
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run dir name");
    let mut fields = name.rsplitn(5, '-');
    let (nonce, start, pid, boot, prefix) = (
        fields.next().expect("nonce"),
        fields.next().expect("start"),
        fields.next().expect("pid"),
        fields.next().expect("boot"),
        fields.next().expect("prefix"),
    );
    assert_eq!(
        (pid, start),
        (&*own_pid.to_string(), &*own_start.to_string())
    );
    for foreign in [
        format!("{prefix}-{boot}-{}-{start}-{nonce}", own_pid + 1),
        format!("{prefix}-{boot}-{pid}-{}-{nonce}", own_start + 1),
        format!("{prefix}-x{boot}-{pid}-{start}-{nonce}"),
    ] {
        std::fs::create_dir(dir.path().with_file_name(foreign)).expect("foreign run dir");
    }
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        own,
        "entries that differ in boot, PID, or start time name another owner"
    );

    dir.cleanup().expect("own run dir removed");
    record.remove().expect("own record removed");
    assert_eq!(
        group_registry::own_residue(&root).expect("registry reads"),
        OwnResidue::default()
    );
}

/// Terminal runs with unproven teardown remain indexed so later control operations observe the teardown verdict.
#[tokio::test]
async fn terminal_cap_never_evicts_a_run_awaiting_teardown() {
    let (backend, gate) = ScriptedBackend::gated_ignoring_cancel("out");
    let limits = ModelExecutionLimits {
        max_terminal_sessions: 1,
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Arc::new(Supervisor::with_limits(
        Arc::clone(&backend) as Arc<_>,
        limits,
    ));

    let run_a = send(&supervisor, "s-del", "pa");
    until(|| backend.starts() == 1, "the deleted run's backend starts").await;
    let deleter = {
        let supervisor = Arc::clone(&supervisor);
        tokio::spawn(async move { supervisor.delete(&key("s-del")).await })
    };
    // `wait_work_done` must retain the terminal run while `delete` waits, even if another session reaches the terminal cap.
    until(
        || supervisor.status(&key("s-del"), &run_a) == Ok("cancelled"),
        "delete commits the cancellation terminal",
    )
    .await;

    // A second session's terminal commit enforces the one-session cap while delete remains parked.
    // The second run remains queued behind the backend's single permit, so cancellation settles promptly.
    let run_b = send(&supervisor, "s-evict", "pb");
    supervisor
        .cancel(&key("s-evict"), &run_b)
        .await
        .expect("cancel the second session's run");
    assert_eq!(
        supervisor.status(&key("s-del"), &run_a),
        Ok("cancelled"),
        "a run awaiting teardown must survive cap pressure"
    );

    gate.add_permits(1);
    deleter
        .await
        .expect("delete task joins")
        .expect("delete settles");
    assert_eq!(
        supervisor.status(&key("s-del"), &run_a),
        Ok("missing"),
        "the settled delete purged its run"
    );
    assert_eq!(supervisor.metrics().tombstones, 1);
    let (request, body) = send_pair("pa");
    let resurrect = supervisor
        .send(&key("s-del"), request, &body)
        .expect_err("the delete still blocks resurrection");
    assert_eq!(resurrect.code, "session_deleted");
}

#[tokio::test(start_paused = true)]
async fn terminal_expiry_and_oldest_eviction_enforce_the_session_caps() {
    let backend = ScriptedBackend::completing("out");
    let limits = ModelExecutionLimits {
        max_terminal_sessions: 2,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);

    let mut runs = Vec::new();
    for index in 0..3 {
        let run_id = send(&supervisor, &format!("s{index}"), &format!("p{index}"));
        until(
            || supervisor.status(&key(&format!("s{index}")), &run_id) == Ok("completed"),
            "run completes",
        )
        .await;
        runs.push(run_id);
        // Distinct commit instants make "oldest" deterministic.
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    assert_eq!(supervisor.status(&key("s0"), &runs[0]), Ok("missing"));
    assert_eq!(supervisor.status(&key("s1"), &runs[1]), Ok("completed"));
    assert_eq!(supervisor.status(&key("s2"), &runs[2]), Ok("completed"));
    assert_eq!(supervisor.metrics().sessions, 2);

    tokio::time::advance(TERMINAL_RETENTION).await;
    // Any command sweeps expired entries first, so both survivors report missing and their retained-byte charges are removed.
    assert_eq!(supervisor.status(&key("s1"), &runs[1]), Ok("missing"));
    assert_eq!(supervisor.status(&key("s2"), &runs[2]), Ok("missing"));
    let metrics = supervisor.metrics();
    assert_eq!(metrics.sessions, 0);
    assert_eq!(
        metrics.retained_bytes_available,
        metrics.retained_bytes_capacity
    );
}

#[tokio::test(start_paused = true)]
async fn retained_pressure_sweeps_expired_entries_and_retries_admission_once() {
    let backend = ScriptedBackend::completing("out");
    // The retained-byte limit admits one live request but rejects a live request plus an expired terminal's residue.
    // Admission succeeds only when the pressure path sweeps the expired entry and retries.
    let limits = ModelExecutionLimits {
        max_retained_bytes: 20 * 1024,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);

    let first = send(&supervisor, "s1", &"x".repeat(12 * 1024));
    until(
        || supervisor.status(&key("s1"), &first) == Ok("completed"),
        "first run completes",
    )
    .await;
    let retained_before = supervisor.metrics().retained_bytes_available;
    assert!(
        retained_before < supervisor.metrics().retained_bytes_capacity,
        "the terminal keeps a retained charge until expiry"
    );

    tokio::time::advance(TERMINAL_RETENTION).await;
    let second = send(&supervisor, "s2", &"y".repeat(12 * 1024));
    until(
        || supervisor.status(&key("s2"), &second) == Ok("completed"),
        "second run completes after the sweep freed the budget",
    )
    .await;
    assert_eq!(supervisor.metrics().sessions, 1);
}

#[tokio::test(start_paused = true)]
async fn replay_overflow_commits_one_failed_terminal_and_stops_growth() {
    let backend = ScriptedBackend::flooding(2_000, 10);
    let limits = ModelExecutionLimits {
        max_run_replay_bytes: 12 * 1024,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits.clone());
    let run_id = send(&supervisor, "s1", "p");
    until(
        || supervisor.status(&key("s1"), &run_id) == Ok("failed"),
        "the overflowing run fails",
    )
    .await;
    // The flooding backend emits no later frames, so retained frames are final.
    let retained_after_failure = supervisor.metrics().retained_bytes_available;

    let replay = drain_bytes(supervisor.subscribe(&key("s1")).expect("subscribe")).await;
    assert!(
        replay.len() < 12,
        "retention stopped before the ten flooded units all landed"
    );
    let last = unit_json(replay.last().expect("terminal"));
    assert_eq!(last["unit"]["type"], "error");
    assert_eq!(last["unit"]["error"]["class"], "permanent");
    let terminal_count = replay
        .iter()
        .filter(|unit| unit_json(unit)["unit"]["type"] == "error")
        .count();
    assert_eq!(terminal_count, 1, "exactly one failed terminal");
    assert_eq!(
        supervisor.metrics().retained_bytes_available,
        retained_after_failure,
        "no retained growth after the terminal"
    );

    // Deleting the session and expiring its tombstone must restore the retained-byte budget to its construction baseline.
    supervisor.delete(&key("s1")).await.expect("delete settles");
    tokio::time::advance(TERMINAL_RETENTION).await;
    // Commands sweep expired entries before status checks.
    assert_eq!(supervisor.status(&key("s1"), "gone"), Ok("missing"));
    assert_baseline(supervisor.metrics(), &limits, 0);
}

#[tokio::test(start_paused = true)]
async fn every_path_returns_permits_and_charges_to_baseline() {
    let (backend, gate) = ScriptedBackend::gated("out");
    let limits = ModelExecutionLimits {
        max_active_runs: 2,
        max_backend_processes: 1,
        max_run_replay_bytes: 12 * 1024,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits.clone());

    // Rejected candidates release their reservations.
    let run_a = send(&supervisor, "a", "prompt a");
    let (conflicting, conflicting_body) = send_pair("other prompt");
    supervisor
        .send(&key("a"), conflicting, &conflicting_body)
        .expect_err("conflict");

    // Run 3 is rejected by the two-run cap before creating an entry.
    let run_b = send(&supervisor, "b", "prompt b");
    let (third, third_body) = send_pair("prompt c");
    supervisor
        .send(&key("c"), third, &third_body)
        .expect_err("active-run cap");
    assert_eq!(supervisor.metrics().sessions, 2);

    let mut aborted = supervisor.subscribe(&key("a")).expect("subscriber");
    until(|| backend.starts() == 1, "first backend starts").await;
    let _ = aborted.next().await;
    drop(aborted);

    gate.add_permits(2);
    until(
        || supervisor.status(&key("a"), &run_a) == Ok("completed"),
        "a completes",
    )
    .await;
    supervisor
        .cancel(&key("b"), &run_b)
        .await
        .expect("cancel b");
    supervisor
        .cancel(&key("b"), &run_b)
        .await
        .expect("idempotent cancel");

    supervisor.delete(&key("b")).await.expect("delete b");
    tokio::time::advance(TERMINAL_RETENTION).await;
    assert_eq!(supervisor.status(&key("a"), &run_a), Ok("missing"));
    assert_eq!(supervisor.status(&key("b"), &run_b), Ok("missing"));

    assert_baseline(supervisor.metrics(), &limits, 0);

    // Shutdown from a state with live work must also land on baseline.
    let (gated_backend, _late_gate) = ScriptedBackend::gated("late");
    let closing = Supervisor::with_limits(gated_backend as Arc<_>, limits.clone());
    send(&closing, "z", "prompt z");
    closing.shutdown().await;
    assert_baseline(closing.metrics(), &limits, 0);
}

#[tokio::test]
async fn shutdown_refuses_new_work_stops_backends_and_wakes_subscribers() {
    let (backend, _gate) = ScriptedBackend::gated("out");
    let limits = ModelExecutionLimits {
        max_backend_processes: 2,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Arc::new(Supervisor::with_limits(
        Arc::clone(&backend) as Arc<_>,
        limits.clone(),
    ));
    for session in ["a", "b", "c"] {
        send(&supervisor, session, session);
    }
    until(|| backend.starts() == 2, "two backends run").await;

    let mut waiters = Vec::new();
    for session in ["a", "b", "c"] {
        let mut subscription = supervisor.subscribe(&key(session)).expect("subscriber");
        waiters.push(tokio::spawn(async move {
            let mut units = 0usize;
            while subscription.next().await.is_some() {
                units += 1;
            }
            units
        }));
    }

    supervisor.shutdown().await;
    assert_eq!(
        backend.cancels_observed(),
        2,
        "every running backend observed the stop"
    );
    for waiter in waiters {
        // The subscriber wakes and finishes; a hung subscriber would block this join.
        waiter.await.expect("subscriber task joins");
    }

    let (request, body) = send_pair("late");
    assert_eq!(
        supervisor
            .send(&key("late"), request, &body)
            .expect_err("send after shutdown")
            .code,
        "cancelled"
    );
    assert_eq!(
        supervisor
            .subscribe(&key("a"))
            .expect_err("subscribe after shutdown")
            .code,
        "cancelled"
    );
    assert_eq!(
        supervisor
            .status(&key("a"), "anything")
            .expect_err("status after shutdown")
            .code,
        "cancelled"
    );
    assert_baseline(supervisor.metrics(), &limits, 0);
}

/// Request cancellation, route closure, and whole-connection loss each detach only their subscriber while the gated run completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_detach_paths_leave_the_run_untouched() {
    let (backend, gate) = ScriptedBackend::gated("survivor output");
    let component = ModelExecutionComponent::new(
        Arc::clone(&backend) as Arc<_>,
        support::model_execution::state_root(),
    );
    let supervisor = component.supervisor();
    let host = start_model_execution_host(component).await;

    let mut client = host.client().await;
    let (command_ch, command_ep) = open_model_execution_route(&mut client, "opencode", "s1").await;
    let response = support::model_execution::call(
        &mut client,
        command_ch,
        command_ep,
        "session.send",
        send_params("detach fixture", None, "prov/model-a"),
    )
    .await;
    let run_id = response.json()["run_id"]
        .as_str()
        .expect("run id")
        .to_owned();

    // When the host cancels the handler task, the handler's waiter detaches and its stream settles without a run terminal.
    let (sub_ch, sub_ep) = open_model_execution_route(&mut client, "opencode", "s1").await;
    let corr = send_call(
        &mut client,
        sub_ch,
        sub_ep,
        "session.subscribe",
        serde_json::json!({ "from": "start" }),
    )
    .await;
    let first = client
        .frames_until_corr(corr, support::model_execution::BUDGET)
        .await
        .expect("run_started streams")
        .1;
    assert_eq!(first.ty, support::raw_client::TY_STREAM_DATA);
    client
        .send_frame(
            support::raw_client::TY_CANCEL,
            support::raw_client::FLAGS_INTERACTIVE,
            sub_ch,
            sub_ep,
            corr,
            &[],
        )
        .await
        .expect("cancel frame");
    let (_stream_units, terminal) =
        support::model_execution::drain_subscribe(&mut client, corr).await;
    assert_ne!(
        terminal.ty,
        support::raw_client::TY_STREAM_DATA,
        "the cancelled request settles"
    );

    let (gone_ch, gone_ep) = open_model_execution_route(&mut client, "opencode", "s1").await;
    let corr = send_call(
        &mut client,
        gone_ch,
        gone_ep,
        "session.subscribe",
        serde_json::json!({ "from": "start" }),
    )
    .await;
    client
        .frames_until_corr(corr, support::model_execution::BUDGET)
        .await
        .expect("run_started streams again");
    client
        .send_frame(
            support::raw_client::TY_GOODBYE,
            support::raw_client::FLAGS_INTERACTIVE,
            gone_ch,
            gone_ep,
            0,
            &[],
        )
        .await
        .expect("goodbye frame");
    // The closed route settles its in-flight subscription with a terminal frame.
    client
        .frames_until_corr(corr, support::model_execution::BUDGET)
        .await
        .expect("the goodbye'd subscription settles");

    // A subscriber whose connection drops entirely (no Goodbye) detaches the same way: its waiter is released without cancelling the run.
    {
        let mut doomed = host.client().await;
        let (lost_ch, lost_ep) = open_model_execution_route(&mut doomed, "opencode", "s1").await;
        let corr = send_call(
            &mut doomed,
            lost_ch,
            lost_ep,
            "session.subscribe",
            serde_json::json!({ "from": "start" }),
        )
        .await;
        doomed
            .frames_until_corr(corr, support::model_execution::BUDGET)
            .await
            .expect("run_started streams on the doomed connection");
    }

    // Every waiter is gone; the run itself never saw a cancellation.
    let deadline = tokio::time::Instant::now() + support::model_execution::BUDGET;
    loop {
        if supervisor.metrics().free_subscriber_permits == 64 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "detached waiters must release their permits"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(backend.cancels_observed(), 0);

    gate.add_permits(1);
    let (final_ch, final_ep) = open_model_execution_route(&mut client, "opencode", "s1").await;
    let corr = send_call(
        &mut client,
        final_ch,
        final_ep,
        "session.subscribe",
        serde_json::json!({ "from": "start" }),
    )
    .await;
    let (units, terminal) = support::model_execution::drain_subscribe(&mut client, corr).await;
    assert_eq!(terminal.ty, support::raw_client::TY_STREAM_END);
    assert_eq!(units.len(), 3, "full replay after every detach: {units:?}");
    assert_eq!(units[2]["unit"]["type"], "run_finished");
    assert_eq!(backend.starts(), 1);

    let status = support::model_execution::call(
        &mut client,
        command_ch,
        command_ep,
        "run.status",
        serde_json::json!({ "run_id": run_id }),
    )
    .await;
    assert_eq!(status.json()["state"], "completed");
    host.shutdown().await.expect("graceful shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_shutdown_drains_the_supervisor_to_zero_state() {
    let (backend, _gate) = ScriptedBackend::gated("out");
    let component = ModelExecutionComponent::new(
        Arc::clone(&backend) as Arc<_>,
        support::model_execution::state_root(),
    );
    let supervisor = component.supervisor();
    let host = start_model_execution_host(component).await;

    let mut client = host.client().await;
    let (command_ch, command_ep) = open_model_execution_route(&mut client, "pi", "s1").await;
    support::model_execution::call(
        &mut client,
        command_ch,
        command_ep,
        "session.send",
        send_params("shutdown fixture", None, "prov/model-a"),
    )
    .await;
    let (sub_ch, sub_ep) = open_model_execution_route(&mut client, "pi", "s1").await;
    let corr = send_call(
        &mut client,
        sub_ch,
        sub_ep,
        "session.subscribe",
        serde_json::json!({ "from": "start" }),
    )
    .await;
    client
        .frames_until_corr(corr, support::model_execution::BUDGET)
        .await
        .expect("subscriber is mid-stream");

    host.shutdown().await.expect("graceful shutdown");
    assert_eq!(backend.cancels_observed(), 1, "the running backend stopped");
    let metrics = supervisor.metrics();
    assert_eq!(metrics.sessions, 0);
    assert_eq!(metrics.live_runs, 0);
    assert_eq!(metrics.tombstones, 0);
    assert_eq!(
        metrics.retained_bytes_available,
        metrics.retained_bytes_capacity
    );
    assert_eq!(metrics.free_subscriber_permits, 64);
    assert_eq!(metrics.free_backend_permits, 8);
    assert_eq!(metrics.free_run_slots, 32);
}

/// A source failure before dispatch is an accepted terminal whose error unit names
/// `credential_source`; a model failure omits the scope, which reads as `model`.
#[tokio::test]
async fn a_source_failure_terminal_carries_the_credential_source_scope() {
    let error = BackendError {
        class: ErrorClass::AuthRequired,
        message: "pi AWS credential source needs a new `aws sso login`".to_owned(),
        retry_after_secs: None,
        provider_code: None,
    };
    for (terminal, scope) in [
        (
            BackendTerminal::SourceFailed(error.clone()),
            Some("credential_source"),
        ),
        (BackendTerminal::Failed(error.clone()), None),
    ] {
        let backend = ScriptedBackend::with_behavior(move |_request, _events, _cancel| {
            let terminal = terminal.clone();
            Box::pin(async move { terminal })
        });
        let supervisor = Supervisor::new(backend as Arc<_>);
        let run_id = send(&supervisor, "s-source", "p");
        until(
            || supervisor.status(&key("s-source"), &run_id) == Ok("failed"),
            "the run fails",
        )
        .await;
        let units = drain_bytes(supervisor.subscribe(&key("s-source")).expect("subscribe")).await;
        let error_unit = unit_json(units.last().expect("terminal unit"));
        assert_eq!(error_unit["unit"]["type"], "error");
        assert_eq!(error_unit["unit"]["error"]["class"], "auth_required");
        assert_eq!(error_unit["unit"]["error"]["scope"].as_str(), scope);
        assert_eq!(
            protocol::ErrorScope::decode(&error_unit["unit"]["error"]).expect("known scope"),
            if scope.is_some() {
                protocol::ErrorScope::CredentialSource
            } else {
                protocol::ErrorScope::Model
            }
        );
    }
}

/// A source failure on a later run leaves an earlier retained result readable.
#[tokio::test]
async fn retained_results_stay_readable_while_the_source_fails() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = ScriptedBackend::with_behavior({
        let calls = Arc::clone(&calls);
        move |_request, _events, _cancel| {
            let first = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    BackendTerminal::Completed {
                        finish_reason:
                            host_runtime::model_execution::backend::FinishReason::Completed,
                    }
                } else {
                    BackendTerminal::SourceFailed(BackendError {
                        class: ErrorClass::Transient,
                        message: "opencode AWS credential source is unavailable".to_owned(),
                        retry_after_secs: Some(60),
                        provider_code: None,
                    })
                }
            })
        }
    });
    let supervisor = Supervisor::new(backend as Arc<_>);
    let done = send(&supervisor, "s-done", "p1");
    until(
        || supervisor.status(&key("s-done"), &done) == Ok("completed"),
        "the first run completes",
    )
    .await;
    let failed = send(&supervisor, "s-later", "p2");
    until(
        || supervisor.status(&key("s-later"), &failed) == Ok("failed"),
        "the source-failed run fails",
    )
    .await;
    assert_eq!(supervisor.status(&key("s-done"), &done), Ok("completed"));
    let (request, body) = send_pair("p1");
    let resent = supervisor
        .send(&key("s-done"), request, &body)
        .expect("the identical send answers from the retained run");
    assert_eq!(
        resent, done,
        "idempotency lookup precedes any new-work source gate"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "no new backend run"
    );
    let units = drain_bytes(supervisor.subscribe(&key("s-done")).expect("subscribe")).await;
    assert_eq!(
        unit_json(units.last().unwrap())["unit"]["type"],
        "run_finished"
    );
}

/// A refresh that holds until released and counts its physical starts.
struct GatedSource {
    starts: Arc<std::sync::atomic::AtomicUsize>,
    release: Arc<tokio::sync::Semaphore>,
}

impl host_runtime::model_execution::aws_refresh::Refresh for GatedSource {
    fn refresh(
        &self,
        _: host_runtime::model_execution::aws_refresh::RefreshRequest,
        _: CancellationToken,
    ) -> host_runtime::model_execution::aws_refresh::RefreshFuture {
        use host_runtime::model_execution::aws_transaction::{
            CredentialRow, RenewalEvidence, TransactionOutcome,
        };
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            release.acquire().await.expect("gate").forget();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            TransactionOutcome {
                identity: Some(source_identity()),
                row: Some(CredentialRow {
                    access_key_id: "ASIA".into(),
                    secret_access_key: zeroize::Zeroizing::new("s".into()),
                    session_token: zeroize::Zeroizing::new("t".into()),
                    expires_at_unix_seconds: now + 3_600,
                }),
                successor: None,
                observation: None,
                renewal: RenewalEvidence::NotStarted,
                lost_succession: false,
                record_retained: false,
                failure: None,
            }
        })
    }
}

fn source_identity() -> host_runtime::model_execution::aws_profile::GraphIdentity {
    use host_runtime::model_execution::aws_profile::{GraphIdentity, RootIdentity};
    GraphIdentity {
        profile: "dev".into(),
        region: "us-west-2".into(),
        roles: Vec::new(),
        root: RootIdentity::Static {
            profile: "base".into(),
            access_key_id: "AKIA".into(),
            secret_sha256: [0; 32],
        },
    }
}

fn gated_owner() -> (
    host_runtime::model_execution::aws_refresh::SourceOwner,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<tokio::sync::Semaphore>,
) {
    use host_runtime::model_execution::aws_refresh::{SourceOwner, SystemClock};
    use host_runtime::model_execution::source_health::{
        SourceHealthCell, SourceKind, SourceObservation,
    };
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let source = GatedSource {
        starts: Arc::clone(&starts),
        release: Arc::clone(&release),
    };
    let health = SourceHealthCell::new(SourceObservation::unknown(SourceKind::Profile));
    let owner = SourceOwner::new(source, SystemClock, source_identity(), health);
    (owner, starts, release)
}

/// Thirty-two admitted runs saturate the eight backend permits while one held refresh
/// serves them all; the waiting runs hold their permits and start no second refresh.
#[tokio::test]
async fn thirty_two_admitted_runs_share_one_refresh_under_saturated_permits() {
    use host_runtime::model_execution::config::ModelExecutionLimits;
    let (owner, starts, release) = gated_owner();
    let acquiring = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = ScriptedBackend::with_behavior({
        let (owner, acquiring) = (owner.clone(), Arc::clone(&acquiring));
        move |_request, _events, cancel| {
            let (owner, acquiring) = (owner.clone(), Arc::clone(&acquiring));
            Box::pin(async move {
                acquiring.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(660);
                match owner.acquire(deadline, &cancel).await {
                    Ok(_) => BackendTerminal::Completed {
                        finish_reason:
                            host_runtime::model_execution::backend::FinishReason::Completed,
                    },
                    Err(error) => BackendTerminal::SourceFailed(BackendError {
                        class: ErrorClass::Transient,
                        message: format!("{error:?}"),
                        retry_after_secs: None,
                        provider_code: None,
                    }),
                }
            })
        }
    });
    let limits = ModelExecutionLimits::default();
    assert_eq!(
        (limits.max_active_runs, limits.max_backend_processes),
        (32, 8)
    );
    let supervisor = Supervisor::with_limits(backend as Arc<_>, limits);
    let runs: Vec<_> = (0..32)
        .map(|n| {
            let session = format!("s-{n}");
            let run = send(&supervisor, &session, "p");
            (session, run)
        })
        .collect();
    until(
        || acquiring.load(std::sync::atomic::Ordering::SeqCst) == 8,
        "eight runs hold the eight backend permits",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(supervisor.metrics().free_backend_permits, 0);
    assert_eq!(
        acquiring.load(std::sync::atomic::Ordering::SeqCst),
        8,
        "the other runs queue on the permits while the refresh is held"
    );
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    release.add_permits(1);
    for (session, run) in &runs {
        until(
            || supervisor.status(&key(session), run) == Ok("completed"),
            "every run completes on the shared row",
        )
        .await;
    }
    assert_eq!(acquiring.load(std::sync::atomic::Ordering::SeqCst), 32);
    assert_eq!(
        starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one physical refresh"
    );
}

/// Health reads `ready` for a cached row, yet a run whose deadline the row cannot outlive
/// gets no row from it: the lifetime predicate, not health, gates dispatch.
#[tokio::test]
async fn a_ready_health_observation_never_authorizes_an_expiring_row() {
    use host_runtime::model_execution::source_health::SourceState;
    let (owner, starts, release) = gated_owner();
    release.add_permits(1);
    let never = CancellationToken::new();
    owner
        .acquire(
            tokio::time::Instant::now() + Duration::from_secs(60),
            &never,
        )
        .await
        .expect("row");
    assert_eq!(owner.health().get().state, SourceState::Ready);
    let beyond = tokio::time::Instant::now() + Duration::from_secs(3_600);
    let refused = tokio::time::timeout(Duration::from_secs(1), owner.acquire(beyond, &never)).await;
    assert!(
        refused.is_err(),
        "the long demand waits on a new refresh instead of the ready row"
    );
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Shutdown cancels a run that waits inside source acquisition and signals the held
/// refresh before it joins either; the joins wait for the refresh to settle.
#[tokio::test]
async fn shutdown_cancels_a_run_waiting_on_the_source_and_joins_the_refresh() {
    use host_runtime::model_execution::ModelExecutionComponent;
    let (owner, starts, release) = gated_owner();
    let waiting = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ended = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = ScriptedBackend::with_behavior({
        let (owner, waiting, ended) = (owner.clone(), Arc::clone(&waiting), Arc::clone(&ended));
        move |_request, _events, cancel| {
            let (owner, waiting, ended) = (owner.clone(), Arc::clone(&waiting), Arc::clone(&ended));
            Box::pin(async move {
                waiting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(660);
                let outcome = owner.acquire(deadline, &cancel).await;
                if outcome.is_err() {
                    ended.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                BackendTerminal::Failed(BackendError {
                    class: ErrorClass::Transient,
                    message: "cancelled during setup".to_owned(),
                    retry_after_secs: None,
                    provider_code: None,
                })
            })
        }
    });
    let component =
        ModelExecutionComponent::new(backend as Arc<_>, support::model_execution::state_root())
            .with_source_owner(Some(owner.clone()));
    let supervisor = component.supervisor();
    send(&supervisor, "s-wait", "p");
    until(
        || waiting.load(std::sync::atomic::Ordering::SeqCst) == 1,
        "the run waits in acquisition",
    )
    .await;
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    let shutdown =
        tokio::spawn(async move { host_runtime::CompositeComponent::shutdown(&component).await });
    until(
        || ended.load(std::sync::atomic::Ordering::SeqCst) == 1,
        "the run's cancellation ends its wait while the refresh is held",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !shutdown.is_finished(),
        "shutdown waits for the held refresh"
    );
    release.add_permits(1);
    shutdown
        .await
        .unwrap()
        .expect("a settled refresh shuts down cleanly");
}

/// The M2 warm-acquisition gate: p99 at most 1 ms on the warm path alone, apart from
/// descriptor and startup work. The qualified runner runs it in release with `--ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "performance measurement for the qualified runner; run in release with --ignored"]
async fn warm_acquisition_p99_stays_within_one_millisecond() {
    let (owner, starts, release) = gated_owner();
    release.add_permits(1);
    let never = CancellationToken::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(660);
    owner.acquire(deadline, &never).await.expect("first row");
    let mut warm = Vec::with_capacity(100_000);
    for _ in 0..100_000 {
        let started = std::time::Instant::now();
        owner.acquire(deadline, &never).await.expect("warm row");
        warm.push(started.elapsed());
    }
    warm.sort();
    let p99 = warm[warm.len() * 99 / 100];
    eprintln!(
        "warm acquisition p99 {p99:?}, max {:?}",
        warm[warm.len() - 1]
    );
    assert!(
        p99 <= Duration::from_millis(1),
        "warm acquisition p99 {p99:?}"
    );
    assert_eq!(
        starts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "warm hits start no refresh"
    );
}
