//! Internal MemoryReviewer runs under the ModelExecution supervisor: invisible to every public session operation, one launch per attempt identity, and sharing run slots, backend permits, retained bytes, cancellation, and joined shutdown with public runs. Paused Tokio time drives the cutoff cases.

// The scripted-backend support module is Linux-only, like the rest of the ModelExecution conformance suite.
#![cfg(target_os = "linux")]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use host_runtime::CancellationToken;
use host_runtime::model_execution::backend::{
    BackendError, BackendEvent, BackendTerminal, ErrorClass, EventSink, FinishReason, Harness,
};
use host_runtime::model_execution::config::ModelExecutionLimits;
use host_runtime::model_execution::protocol::{RequestError, SendRequest};
use host_runtime::model_execution::supervisor::{
    INTERNAL_LAUNCH_ABORT_GRACE, InternalOutcome, InternalRunKey, Launch,
    MAX_INTERNAL_KEY_FIELD_BYTES, SessionKey, Supervisor,
};
use tokio::sync::Semaphore;
use tokio::time::Instant;

use support::model_execution::{ScriptedBackend, send_params};

fn internal_key(job: &str, attempt: u32) -> InternalRunKey {
    InternalRunKey {
        task_kind: "memory_reviewer_investigation".to_string(),
        project_digest: "a".repeat(64),
        job_id: job.to_string(),
        receipt_generation: 1,
        attempt,
        kernel_incarnation: "k".repeat(32),
        memstore_incarnation: "m".repeat(32),
    }
}

/// A public key whose strings match the internal run's identity everywhere a string could be compared.
fn matching_public_key(job: &str) -> SessionKey {
    SessionKey {
        project_root: "a".repeat(64).into(),
        harness: Harness::OpenCode,
        session: job.to_string(),
    }
}

fn public_key(session: &str) -> SessionKey {
    SessionKey {
        project_root: "/workspace/project".into(),
        harness: Harness::OpenCode,
        session: session.to_owned(),
    }
}

fn try_public_send(supervisor: &Supervisor, session: &str) -> Result<String, RequestError> {
    let request = SendRequest {
        prompt: "public".to_owned(),
        system: None,
        provider: "prov".to_owned(),
        model: "model-a".to_owned(),
        max_output_tokens: 1_000,
        temperature: 0.1,
    };
    let body = serde_json::to_vec(&serde_json::json!({
        "method": "session.send",
        "params": send_params("public", None, "prov/model-a"),
    }))
    .unwrap();
    supervisor.send(&public_key(session), request, &body)
}

fn public_send(supervisor: &Supervisor, session: &str) -> String {
    try_public_send(supervisor, session).expect("public send admits")
}

async fn until(mut cond: impl FnMut() -> bool, what: &str) {
    for _ in 0..100_000 {
        if cond() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition not reached: {what}");
}

/// A launch that counts its invocations, waits on a gate, and honours cancellation.
struct Scripted {
    starts: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    gate: Arc<Semaphore>,
}

impl Scripted {
    fn new() -> Self {
        Self {
            starts: Arc::new(AtomicUsize::new(0)),
            cancels: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Semaphore::new(0)),
        }
    }

    fn launch(&self, text: &str, honour_cancel: bool) -> Launch {
        let text = text.to_owned();
        let starts = self.starts.clone();
        let cancels = self.cancels.clone();
        let gate = self.gate.clone();
        Box::new(move |events: EventSink, cancel: CancellationToken| {
            starts.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if honour_cancel {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => {
                            cancels.fetch_add(1, Ordering::SeqCst);
                            return BackendTerminal::Failed(BackendError {
                                class: ErrorClass::Permanent,
                                message: "launch observed cancellation".to_owned(),
                                retry_after_secs: None,
                                provider_code: None,
                            });
                        }
                        permit = gate.acquire() => permit.unwrap().forget(),
                    }
                } else {
                    gate.acquire().await.unwrap().forget();
                }
                events.emit(BackendEvent::AssistantText {
                    text,
                    finish_reason: None,
                });
                BackendTerminal::Completed {
                    finish_reason: FinishReason::Completed,
                }
            })
        })
    }
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

#[tokio::test]
async fn internal_runs_are_invisible_to_every_public_operation() {
    let backend = ScriptedBackend::completing("public");
    let supervisor = Supervisor::new(backend as Arc<_>);
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            512,
            far(),
            script.launch("internal", true),
        )
        .unwrap();
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch runs",
    )
    .await;
    let public = matching_public_key("job-1");
    assert_eq!(
        supervisor.status(&public, run.run_id()).unwrap(),
        "missing",
        "matching strings cannot see an internal run"
    );
    supervisor.cancel(&public, run.run_id()).await.unwrap();
    assert!(
        supervisor.subscribe(&public).is_err(),
        "no public subscription reaches an internal run"
    );
    supervisor.delete(&public).await.unwrap();
    assert_eq!(
        script.cancels.load(Ordering::SeqCst),
        0,
        "public cancel and delete did not touch the internal run"
    );
    assert_eq!(supervisor.metrics().live_runs, 1);
    script.gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    assert!(!run.work_unresolved());
    // The public key is still free to hold its own run afterwards.
    let request = SendRequest {
        prompt: "p".to_owned(),
        system: None,
        provider: "prov".to_owned(),
        model: "model-a".to_owned(),
        max_output_tokens: 10,
        temperature: 0.0,
    };
    supervisor.send(&public, request, b"body").unwrap();
    assert_eq!(supervisor.metrics().live_runs, 2);
}

#[tokio::test]
async fn an_attempt_identity_admits_exactly_one_launch() {
    let supervisor = Supervisor::new(ScriptedBackend::completing("public") as Arc<_>);
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("one", true),
        )
        .unwrap();
    let duplicate = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("two", true),
        )
        .unwrap_err();
    assert_eq!(duplicate.code, "internal_run_exists");
    // A different attempt of the same job is a different identity.
    let next = supervisor
        .launch_internal(
            internal_key("job-1", 2),
            0,
            far(),
            script.launch("three", true),
        )
        .unwrap();
    until(
        || script.starts.load(Ordering::SeqCst) == 2,
        "both admitted launches run",
    )
    .await;
    script.gate.add_permits(2);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    assert_eq!(next.settled().await, InternalOutcome::Completed);
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        2,
        "the refused launch never ran"
    );
    // A retained terminal still owns the identity.
    assert_eq!(
        supervisor
            .launch_internal(
                internal_key("job-1", 1),
                0,
                far(),
                script.launch("four", true)
            )
            .unwrap_err()
            .code,
        "internal_run_exists"
    );
}

#[tokio::test]
async fn internal_and_public_runs_share_slots_permits_and_bytes() {
    let (backend, public_gate) = ScriptedBackend::gated("public");
    let limits = ModelExecutionLimits {
        max_active_runs: 2,
        max_backend_processes: 1,
        max_retained_bytes: 64 * 1024,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);
    public_send(&supervisor, "s1");
    until(
        || backend.starts() == 1,
        "the public run holds the only backend permit",
    )
    .await;
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("internal", true),
        )
        .unwrap();
    assert_eq!(
        supervisor.metrics().free_run_slots,
        0,
        "both run slots are taken"
    );
    assert_eq!(supervisor.metrics().free_backend_permits, 0);
    tokio::task::yield_now().await;
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        0,
        "the internal launch waits for the permit"
    );
    // The third run, public or internal, is refused for slots.
    assert_eq!(
        supervisor
            .launch_internal(internal_key("job-2", 1), 0, far(), script.launch("x", true))
            .unwrap_err()
            .code,
        "queue_full"
    );
    // A request larger than the retained budget is refused before anything is published.
    assert_eq!(
        supervisor
            .launch_internal(
                internal_key("job-3", 1),
                1024 * 1024,
                far(),
                script.launch("x", true)
            )
            .unwrap_err()
            .code,
        "queue_full"
    );
    assert_eq!(supervisor.metrics().live_runs, 2);
    public_gate.add_permits(1);
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the released permit reaches the internal run",
    )
    .await;
    script.gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    // Emitted text is charged to the shared budget: an internal run whose output exceeds the per-run replay cap fails like a public one.
    let big = Supervisor::with_limits(
        ScriptedBackend::completing("public") as Arc<_>,
        ModelExecutionLimits {
            max_run_replay_bytes: 8 * 1024,
            ..ModelExecutionLimits::default()
        },
    );
    let text = "y".repeat(16 * 1024);
    let overflow = big
        .launch_internal(
            internal_key("job-4", 1),
            0,
            far(),
            Box::new(move |events, _| {
                Box::pin(async move {
                    events.emit(BackendEvent::AssistantText {
                        text,
                        finish_reason: None,
                    });
                    BackendTerminal::Completed {
                        finish_reason: FinishReason::Completed,
                    }
                })
            }),
        )
        .unwrap();
    assert_eq!(overflow.settled().await, InternalOutcome::Failed);
    let metrics = big.metrics();
    assert_eq!(
        metrics.free_run_slots,
        ModelExecutionLimits::default().max_active_runs
    );
    assert_eq!(
        metrics.free_backend_permits,
        ModelExecutionLimits::default().max_backend_processes
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_propagates_and_permits_return_only_after_physical_completion() {
    let limits = ModelExecutionLimits {
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Arc::new(Supervisor::with_limits(
        ScriptedBackend::completing("public") as Arc<_>,
        limits,
    ));
    // A launch that ignores its token keeps the permit until it physically returns; the cancel call waits for that.
    let script = Scripted::new();
    let run = Arc::new(
        supervisor
            .launch_internal(
                internal_key("job-1", 1),
                0,
                far(),
                script.launch("late", false),
            )
            .unwrap(),
    );
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch is running",
    )
    .await;
    let cancelling = tokio::spawn({
        let run = Arc::clone(&run);
        async move { run.cancel().await }
    });
    until(
        || supervisor.metrics().free_backend_permits == 0,
        "the permit is still held",
    )
    .await;
    tokio::task::yield_now().await;
    assert!(
        !cancelling.is_finished(),
        "cancellation resolves only with physical completion"
    );
    assert_eq!(supervisor.metrics().free_backend_permits, 0);
    script.gate.add_permits(1);
    cancelling.await.unwrap().unwrap();
    assert_eq!(
        run.settled().await,
        InternalOutcome::Cancelled,
        "a late completion cannot overwrite the cancellation"
    );
    assert_eq!(supervisor.metrics().free_backend_permits, 1);
    // A cooperative launch settles through the handle's own cancel and reports no residue.
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-2", 1),
            0,
            far(),
            script.launch("coop", true),
        )
        .unwrap();
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch is running",
    )
    .await;
    run.cancel().await.unwrap();
    assert_eq!(script.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(run.settled().await, InternalOutcome::Cancelled);
    assert!(run.residue().is_ok());
    assert_eq!(supervisor.metrics().free_backend_permits, 1);
    // A launch that ignores its token past the grace period is aborted and its teardown reported unproven.
    let script = Scripted::new();
    let run = Arc::new(
        supervisor
            .launch_internal(
                internal_key("job-3", 1),
                0,
                far(),
                script.launch("stuck", false),
            )
            .unwrap(),
    );
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch is running",
    )
    .await;
    let cancelling = tokio::spawn({
        let run = Arc::clone(&run);
        async move { run.cancel().await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(INTERNAL_LAUNCH_ABORT_GRACE + Duration::from_millis(1)).await;
    assert_eq!(
        cancelling.await.unwrap().unwrap_err().code,
        "teardown_unconfirmed"
    );
    assert_eq!(run.settled().await, InternalOutcome::Cancelled);
    assert!(run.work_unresolved());
    assert_eq!(supervisor.metrics().free_backend_permits, 1);
    // A launch that panics is lost with unproven teardown, not reported as a clean outcome.
    let run = supervisor
        .launch_internal(
            internal_key("job-4", 1),
            0,
            far(),
            Box::new(|_, _| Box::pin(async { panic!("launch panicked") })),
        )
        .unwrap();
    assert_eq!(run.settled().await, InternalOutcome::Failed);
    assert!(run.work_unresolved());
    assert_eq!(run.residue().unwrap_err().code, "teardown_unconfirmed");
}

#[tokio::test(start_paused = true)]
async fn shutdown_joins_internal_runs() {
    let (backend, _public_gate) = ScriptedBackend::gated("public");
    let limits = ModelExecutionLimits {
        max_backend_processes: 3,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);
    public_send(&supervisor, "s1");
    let script = Scripted::new();
    let running = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("internal", true),
        )
        .unwrap();
    // The stuck launch ignores cancellation, and its forced teardown outlasts the cutoff.
    let stuck_script = Scripted::new();
    let stuck = supervisor
        .launch_internal(
            internal_key("job-2", 1),
            0,
            Instant::now() + Duration::from_secs(1),
            stuck_script.launch("stuck", false),
        )
        .unwrap();
    until(
        || {
            script.starts.load(Ordering::SeqCst) == 1
                && stuck_script.starts.load(Ordering::SeqCst) == 1
                && backend.starts() == 1
        },
        "all three run",
    )
    .await;
    // Every backend permit is held, so this launch is still queued at shutdown.
    let queued = supervisor
        .launch_internal(
            internal_key("job-3", 1),
            0,
            far(),
            script.launch("queued", true),
        )
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        1,
        "the queued launch waits for a permit"
    );
    let unresolved = supervisor.shutdown().await;
    assert_eq!(
        unresolved, 1,
        "only the aborted launch has unproven teardown"
    );
    assert_eq!(
        script.cancels.load(Ordering::SeqCst),
        1,
        "shutdown cancelled the running cooperative launch once"
    );
    assert_eq!(backend.cancels_observed(), 1, "and the public one");
    // Every phase reports the host shutdown, not a coordinator cancel or the cutoff.
    assert_eq!(running.settled().await, InternalOutcome::Shutdown);
    assert_eq!(
        stuck.settled().await,
        InternalOutcome::Shutdown,
        "a teardown that outlasts the cutoff is still a shutdown"
    );
    assert!(stuck.work_unresolved());
    assert_eq!(queued.settled().await, InternalOutcome::Shutdown);
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        1,
        "the queued launch never ran"
    );
    let metrics = supervisor.metrics();
    assert_eq!(metrics.sessions, 0);
    assert_eq!(metrics.live_runs, 0);
    assert_eq!(
        metrics.retained_bytes_available,
        metrics.retained_bytes_capacity
    );
    // A closed supervisor refuses later internal launches.
    assert_eq!(
        supervisor
            .launch_internal(internal_key("job-4", 1), 0, far(), script.launch("x", true))
            .unwrap_err()
            .code,
        "cancelled"
    );
}

#[tokio::test(start_paused = true)]
async fn a_coordinator_cancel_keeps_the_ranked_stop_reason() {
    // Shutdown has begun, but the launch ignores its token, so no terminal is committed when the coordinator cancels.
    let supervisor = Arc::new(Supervisor::new(
        ScriptedBackend::completing("public") as Arc<_>
    ));
    let script = Scripted::new();
    let run = Arc::new(
        supervisor
            .launch_internal(
                internal_key("job-1", 1),
                0,
                far(),
                script.launch("stuck", false),
            )
            .unwrap(),
    );
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch is running",
    )
    .await;
    let shutdown = tokio::spawn({
        let supervisor = Arc::clone(&supervisor);
        async move { supervisor.shutdown().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        try_public_send(&supervisor, "probe").unwrap_err().code,
        "cancelled",
        "shutdown has begun"
    );
    let cancelling = tokio::spawn({
        let run = Arc::clone(&run);
        async move { run.cancel().await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(INTERNAL_LAUNCH_ABORT_GRACE + Duration::from_millis(1)).await;
    cancelling.await.unwrap().unwrap_err();
    shutdown.await.unwrap();
    assert_eq!(
        run.settled().await,
        InternalOutcome::Shutdown,
        "a coordinator cancel during shutdown is still a shutdown"
    );
    // The cutoff has passed and cancelled the token, but the launch ignores it and the coordinator cancels before the abort.
    let supervisor = Supervisor::new(ScriptedBackend::completing("public") as Arc<_>);
    let script = Scripted::new();
    let run = Arc::new(
        supervisor
            .launch_internal(
                internal_key("job-2", 1),
                0,
                Instant::now() + Duration::from_secs(10),
                script.launch("stuck", false),
            )
            .unwrap(),
    );
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch is running",
    )
    .await;
    tokio::time::advance(Duration::from_secs(11)).await;
    let cancelling = tokio::spawn({
        let run = Arc::clone(&run);
        async move { run.cancel().await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(INTERNAL_LAUNCH_ABORT_GRACE).await;
    cancelling.await.unwrap().unwrap_err();
    assert_eq!(
        run.settled().await,
        InternalOutcome::Cutoff,
        "a coordinator cancel after the cutoff is still a cutoff"
    );
}

#[tokio::test(start_paused = true)]
async fn the_cutoff_bounds_the_backend_permit_wait_without_a_launch() {
    let (backend, public_gate) = ScriptedBackend::gated("public");
    let limits = ModelExecutionLimits {
        max_backend_processes: 1,
        ..ModelExecutionLimits::default()
    };
    let supervisor = Supervisor::with_limits(Arc::clone(&backend) as Arc<_>, limits);
    let public_run = public_send(&supervisor, "s1");
    until(
        || backend.starts() == 1,
        "the public run holds the only permit",
    )
    .await;
    let script = Scripted::new();
    let cutoff = Instant::now() + Duration::from_secs(30);
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            cutoff,
            script.launch("internal", true),
        )
        .unwrap();
    // The permit is still held past the cutoff: the queued internal run terminates without dispatch.
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(run.settled().await, InternalOutcome::Cutoff);
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        0,
        "no launch happened"
    );
    assert_eq!(backend.starts(), 1, "the public run was untouched");
    // Late acquisition: the permit released after the cutoff reaches nothing.
    public_gate.add_permits(1);
    until(
        || supervisor.status(&public_key("s1"), &public_run).unwrap() == "completed",
        "the public run completes",
    )
    .await;
    tokio::task::yield_now().await;
    assert_eq!(script.starts.load(Ordering::SeqCst), 0);
    assert_eq!(supervisor.metrics().free_backend_permits, 1);
    // Acquisition racing the cutoff: the permit and the cutoff become ready in the same instant, and the cutoff wins.
    let (backend, public_gate) = ScriptedBackend::gated("public");
    let supervisor = Supervisor::with_limits(
        Arc::clone(&backend) as Arc<_>,
        ModelExecutionLimits {
            max_backend_processes: 1,
            ..ModelExecutionLimits::default()
        },
    );
    public_send(&supervisor, "s1");
    until(
        || backend.starts() == 1,
        "the public run holds the only permit",
    )
    .await;
    let script = Scripted::new();
    let cutoff = Instant::now() + Duration::from_secs(5);
    let run = supervisor
        .launch_internal(
            internal_key("job-2", 1),
            0,
            cutoff,
            script.launch("internal", true),
        )
        .unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    public_gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Cutoff);
    assert_eq!(
        script.starts.load(Ordering::SeqCst),
        0,
        "a permit acquired at the cutoff launches nothing"
    );
    // A cutoff still ahead lets the same shape launch.
    let (backend, public_gate) = ScriptedBackend::gated("public");
    let supervisor = Supervisor::with_limits(
        Arc::clone(&backend) as Arc<_>,
        ModelExecutionLimits {
            max_backend_processes: 1,
            ..ModelExecutionLimits::default()
        },
    );
    public_send(&supervisor, "s1");
    until(
        || backend.starts() == 1,
        "the public run holds the only permit",
    )
    .await;
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-3", 1),
            0,
            Instant::now() + Duration::from_secs(60),
            script.launch("internal", true),
        )
        .unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    public_gate.add_permits(1);
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch runs before its cutoff",
    )
    .await;
    script.gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    // A launch still running at its cutoff has its token cancelled and ends as a cutoff, not a plain cancellation.
    let supervisor = Supervisor::new(ScriptedBackend::completing("public") as Arc<_>);
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-4", 1),
            0,
            Instant::now() + Duration::from_secs(10),
            script.launch("internal", true),
        )
        .unwrap();
    until(
        || script.starts.load(Ordering::SeqCst) == 1,
        "the launch runs",
    )
    .await;
    tokio::time::advance(Duration::from_secs(11)).await;
    assert_eq!(run.settled().await, InternalOutcome::Cutoff);
    assert_eq!(script.cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn an_identity_is_single_use_only_while_its_run_is_retained() {
    let supervisor = Supervisor::with_limits(
        ScriptedBackend::completing("public") as Arc<_>,
        ModelExecutionLimits {
            terminal_retention: Duration::from_secs(1),
            ..ModelExecutionLimits::default()
        },
    );
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("one", true),
        )
        .unwrap();
    script.gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    assert_eq!(
        supervisor
            .launch_internal(
                internal_key("job-1", 1),
                0,
                far(),
                script.launch("two", true)
            )
            .unwrap_err()
            .code,
        "internal_run_exists"
    );
    // Once retention has swept the terminal, the identity is free again; durable attempt identity is the Kernel's, not the supervisor's.
    tokio::time::advance(Duration::from_secs(2)).await;
    let again = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("two", true),
        )
        .unwrap();
    script.gate.add_permits(1);
    assert_eq!(again.settled().await, InternalOutcome::Completed);
    // An over-long key field is refused before anything is reserved.
    let mut long = internal_key("job-2", 1);
    long.job_id = "j".repeat(MAX_INTERNAL_KEY_FIELD_BYTES + 1);
    assert_eq!(
        supervisor
            .launch_internal(long, 0, far(), script.launch("x", true))
            .unwrap_err()
            .code,
        "invalid_request"
    );
    let metrics = supervisor.metrics();
    assert_eq!(
        metrics.free_run_slots,
        ModelExecutionLimits::default().max_active_runs
    );
}

#[tokio::test]
async fn an_internal_terminal_never_forces_a_public_eviction() {
    let supervisor = Supervisor::with_limits(
        ScriptedBackend::completing("public") as Arc<_>,
        ModelExecutionLimits {
            max_terminal_sessions: 1,
            ..ModelExecutionLimits::default()
        },
    );
    // One public deletion guard fills the cap.
    public_send(&supervisor, "s1");
    supervisor.delete(&public_key("s1")).await.unwrap();
    assert_eq!(supervisor.metrics().tombstones, 1);
    // The internal run commits while the cap is full, its own entry is not yet evictable, and no other internal terminal is retained.
    let script = Scripted::new();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            script.launch("one", true),
        )
        .unwrap();
    script.gate.add_permits(1);
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    assert_eq!(
        supervisor.metrics().tombstones,
        1,
        "the deletion guard survived the internal commit"
    );
    // The guard still refuses the deleted session, and the cap sweep that send runs evicts the internal terminal instead.
    assert_eq!(
        try_public_send(&supervisor, "s1").unwrap_err().code,
        "session_deleted"
    );
    let metrics = supervisor.metrics();
    assert_eq!(metrics.tombstones, 1);
    assert_eq!(metrics.internal_runs, 0);
    assert_eq!(metrics.sessions, 1);
}

#[tokio::test]
async fn internal_output_is_bounded_but_never_retained() {
    let supervisor = Supervisor::new(ScriptedBackend::completing("public") as Arc<_>);
    let baseline = supervisor.metrics().retained_bytes_available;
    let text = "z".repeat(64 * 1024);
    let emitted = text.len();
    let run = supervisor
        .launch_internal(
            internal_key("job-1", 1),
            0,
            far(),
            Box::new(move |events, _| {
                Box::pin(async move {
                    events.emit(BackendEvent::AssistantText {
                        text,
                        finish_reason: None,
                    });
                    BackendTerminal::Completed {
                        finish_reason: FinishReason::Completed,
                    }
                })
            }),
        )
        .unwrap();
    assert_eq!(run.settled().await, InternalOutcome::Completed);
    // No subscriber can read an internal replay, so the retained terminal keeps only its key metadata and terminal headroom.
    let held = baseline - supervisor.metrics().retained_bytes_available;
    assert!(
        held < emitted,
        "the emitted text stayed charged: {held} bytes held for a {emitted}-byte message"
    );
    assert_eq!(supervisor.metrics().internal_runs, 1);
}
