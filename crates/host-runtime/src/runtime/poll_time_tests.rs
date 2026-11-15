use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use super::*;
use crate::handler::{
    BindOutcome, RequestCtx, RequestOutcome, RouteHandle, RouteIdentity, RouteTarget,
};
use crate::lifecycle_deadline::test_cpu;

const POLL_TIME: Duration = Duration::from_secs(3);

async fn spinning() {
    let polls = AtomicUsize::new(0);
    std::future::poll_fn(|cx| {
        assert!(
            polls.fetch_add(1, Ordering::SeqCst) < 100,
            "the deadline expires first"
        );
        test_cpu::spend(POLL_TIME);
        let mut advance = std::pin::pin!(tokio::time::advance(POLL_TIME));
        let _ = advance.as_mut().poll(cx);
        cx.waker().wake_by_ref();
        Poll::<()>::Pending
    })
    .await;
}

#[derive(Clone, Default)]
struct SpinningHandler {
    spin_initialize: bool,
    spin_shutdown: bool,
    published: Arc<AtomicBool>,
}

impl HostHandler for SpinningHandler {
    fn manifests(&self) -> Vec<crate::ManifestSnapshot> {
        vec![crate::ManifestSnapshot {
            module_id: "context".to_owned(),
            module_version: "0.1.0-test".to_owned(),
            provides: vec![serde_json::json!({
                "role": "tool_provider",
                "tools": [{"name": "eidnara_reduce", "execution_mode": "pure", "schema": {"type": "object"}}],
                "identity_scope": ["session", "project"],
                "concurrency": "module_managed",
                "emits_push": false,
                "sub_supervises": false
            })],
            control_ops: Vec::new(),
        }]
    }

    async fn initialize(&self, _: crate::HostInit) -> Result<(), crate::InitError> {
        if self.spin_initialize {
            spinning().await;
        }
        Ok(())
    }

    async fn bind(&self, _: RouteHandle, _: RouteTarget, _: RouteIdentity) -> BindOutcome {
        BindOutcome::Accept
    }

    async fn handle(&self, _: RequestCtx) -> RequestOutcome {
        RequestOutcome::error("unsupported", "the handler serves no request")
    }

    async fn route_gone(&self, _: RouteHandle) {}

    async fn health(&self) -> HealthReport {
        self.published.store(true, Ordering::SeqCst);
        HealthReport::ok()
    }

    async fn shutdown(&self) {
        if self.spin_shutdown {
            spinning().await;
        }
    }
}

fn config(data_root: &std::path::Path) -> HostConfig {
    HostConfig {
        data_dir: Some(data_root.to_path_buf()),
        daemon_ver: "eidnara-host/test".to_owned(),
        limits: HostLimits {
            max_resident_bytes: crate::config::MIN_RESIDENT_BYTES * 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test(start_paused = true)]
async fn an_initializer_whose_polls_spend_the_deadline_fails_startup() {
    test_cpu::spend(Duration::ZERO);
    let data_root = tempfile::tempdir().expect("temp data root");
    let handler = SpinningHandler {
        spin_initialize: true,
        ..Default::default()
    };
    let result = run(handler, config(data_root.path()), CancellationToken::new()).await;
    assert!(
        matches!(&result, Err(HostError::InitFailed(message)) if message == "initialize deadline expired"),
        "{result:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_callback_whose_polls_spend_the_deadline_is_fatal() {
    test_cpu::spend(Duration::ZERO);
    let data_root = tempfile::tempdir().expect("temp data root");
    let handler = SpinningHandler {
        spin_shutdown: true,
        ..Default::default()
    };
    let published = Arc::clone(&handler.published);
    let shutdown = CancellationToken::new();
    let host = tokio::spawn(run(handler, config(data_root.path()), shutdown.clone()));
    while !published.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    shutdown.cancel();
    let result = host.await.expect("run task");
    assert!(
        matches!(&result, Err(HostError::LifecycleFatal(message)) if message == "shutdown callback deadline expired"),
        "{result:?}"
    );
}
