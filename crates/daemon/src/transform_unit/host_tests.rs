use super::*;
use host_runtime::{
    Client, ClientRoute, HostConfig, HostError, HostHandler, HostInit, RequestOptions,
    ResponseStream, RouteTarget, SendOutcome, TargetKind,
};
use tokio::sync::oneshot::error::TryRecvError;

const PANIC_CANARY: &str = "CANARY-TRANSFORM-PANIC-7a5e";
const PANIC_CHILD_ENV: &str = "EIDNARA_TRANSFORM_PANIC_CHILD";
const PROBE_BODY: &[u8] = b"resident-probe";

struct ObservedHandler {
    handler: Handler,
    activated: Mutex<Option<oneshot::Sender<()>>>,
    probe: Mutex<Option<oneshot::Sender<RequestCtx>>>,
    request_seen: Mutex<Option<oneshot::Sender<(CancelSignal, usize)>>>,
    gone: Mutex<Option<oneshot::Sender<(RouteHandle, usize)>>>,
}

impl HostHandler for ObservedHandler {
    fn manifests(&self) -> Vec<ManifestSnapshot> {
        vec![self.handler.manifest()]
    }

    fn resource_declarations(&self) -> Vec<ResourceDeclaration> {
        vec![self.handler.resources()]
    }

    fn install_connection_key(&self, key: [u8; 32]) {
        self.handler.install_connection_key(key);
    }

    async fn initialize(&self, init: HostInit) -> Result<(), InitError> {
        self.handler.initialize(init).await
    }

    async fn activate(&self) -> Result<(), InitError> {
        self.handler.activate().await?;
        self.activated
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        Ok(())
    }

    async fn bind(
        &self,
        route: RouteHandle,
        _target: RouteTarget,
        identity: RouteIdentity,
    ) -> BindOutcome {
        self.handler.bind(route, identity).await
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        if ctx.body.as_slice() == PROBE_BODY {
            self.probe
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(ctx)
                .unwrap();
            return RequestOutcome::Streamed;
        }
        if let Some(seen) = self.request_seen.lock().unwrap().take() {
            let (ingress_bytes, _) = ctx.body.ingress_charge_for_test();
            let _ = seen.send((ctx.cancel_signal(), ingress_bytes));
        }
        self.handler.handle(ctx).await
    }

    async fn route_gone(&self, route: RouteHandle) {
        let permits = self.handler.transform_units.available_permits();
        self.handler.route_gone(route).await;
        if let Some(gone) = self.gone.lock().unwrap().take() {
            let _ = gone.send((route, permits));
        }
    }

    async fn health(&self) -> HealthReport {
        self.handler.health().await
    }

    async fn shutdown(&self) {
        self.handler.shutdown().await.unwrap();
    }
}

struct DaemonHost {
    core: Arc<HandlerCore>,
    store: Arc<MemoryStore>,
    client: Client,
    route: ClientRoute,
    probe: RequestCtx,
    request_seen: oneshot::Receiver<(CancelSignal, usize)>,
    gone: oneshot::Receiver<(RouteHandle, usize)>,
    error_published: oneshot::Receiver<(u16, usize)>,
    join: tokio::task::JoinHandle<Result<(), HostError>>,
    stop_on_drop: tokio_util::sync::DropGuard,
    _dir: tempfile::TempDir,
}

impl DaemonHost {
    async fn start(hook: Box<dyn FnOnce() + Send>) -> Self {
        let (handler, store, dir, project) =
            handler_with_store(Arc::new(ProducerState::default()), default_test_config());
        handler.unbind_route(test_route(7));
        assert!(handler.bindings.lock().unwrap().by_route.is_empty());
        *handler.after_transform_commit.lock().unwrap() = Some(hook);
        let core = Arc::clone(&handler.core);
        let (activated, ready) = oneshot::channel();
        let (probe_tx, probe_rx) = oneshot::channel();
        let (seen_tx, request_seen) = oneshot::channel();
        let (gone_tx, gone) = oneshot::channel();
        let observed = ObservedHandler {
            handler,
            activated: Mutex::new(Some(activated)),
            probe: Mutex::new(Some(probe_tx)),
            request_seen: Mutex::new(Some(seen_tx)),
            gone: Mutex::new(Some(gone_tx)),
        };
        let mut config = HostConfig {
            data_dir: Some(dir.path().join("host")),
            ..HostConfig::default()
        };
        config.init.storage = Some(
            serde_json::to_value(dev_descriptor_at(dir.path().join("data").to_str().unwrap()))
                .unwrap(),
        );
        config.limits.max_resident_bytes += DECLARED_RETAINED_RESIDENT_BYTES;
        config.timing.route_close_budget = Duration::from_secs(2);
        config.timing.shutdown_deadline = Duration::from_secs(5);
        let publication = host_runtime::runtime_dir_path(config.data_dir.as_deref())
            .unwrap()
            .join(host_runtime::CONNECTION_FILE_NAME);
        let shutdown = CancellationToken::new();
        let stop_on_drop = shutdown.clone().drop_guard();
        let (error_tx, error_published) = oneshot::channel();
        let error_tx = Mutex::new(Some(error_tx));
        let publish_core = Arc::clone(&core);
        let publish_hook = Arc::new(move |kind, channel| {
            if kind == host_runtime::wire::FrameType::Error
                && channel != 0
                && let Some(send) = error_tx.lock().unwrap().take()
            {
                let permits = publish_core.transform_units.available_permits();
                let _ = send.send((channel, permits));
            }
        });
        let mut join = tokio::spawn(host_runtime::run_with_publish_hook(
            observed,
            config,
            shutdown,
            Some(publish_hook),
        ));
        watchdog(async {
            tokio::select! {
                result = ready => result.expect("host must activate"),
                result = &mut join => panic!("host stopped before activation: {result:?}"),
            }
        })
        .await;
        let client = watchdog(Client::connect(publication)).await.unwrap();
        let route = watchdog(client.open_route(
            RouteTarget {
                module_id: DEFAULT_MODULE_ID.to_owned(),
                kind: TargetKind::ToolProvider,
            },
            RouteIdentity {
                project_root: project,
                harness: "daemon-test".to_owned(),
                session: "ses".to_owned(),
                consumer_module_id: None,
                consumer_launch_nonce: None,
                consumer_capabilities: Vec::new(),
                admission_facts: None,
                credential_fingerprints: Default::default(),
            },
        ))
        .await
        .unwrap();
        let mut response =
            watchdog(client.request_stream(route, PROBE_BODY.to_vec(), RequestOptions::default()))
                .await
                .unwrap();
        let probe = watchdog(probe_rx).await.unwrap();
        assert_eq!(watchdog(response.next()).await.unwrap(), None);
        Self {
            core,
            store,
            client,
            route,
            probe,
            request_seen,
            gone,
            error_published,
            join,
            stop_on_drop,
            _dir: dir,
        }
    }

    async fn transform(&self) -> ResponseStream {
        watchdog(self.client.request_stream(
            self.route,
            serde_json::to_vec(&request(vec![ck("m1", 1, "durable host transform")])).unwrap(),
            RequestOptions::default(),
        ))
        .await
        .unwrap()
    }

    fn assert_scratch_released(&self) {
        let _all = self
            .probe
            .try_reserve_resident(self.probe.resident_capacity())
            .expect("all scratch bytes must return after transform completion");
        assert!(
            self.probe.try_reserve_resident(1).is_none(),
            "release must not grow the pool"
        );
    }

    async fn stop(self) {
        drop(self.probe);
        watchdog(self.client.close()).await.unwrap();
        drop(self.stop_on_drop);
        watchdog(self.join)
            .await
            .unwrap()
            .expect("host must shut down cleanly");
    }
}

#[derive(Clone, Copy)]
enum Interruption {
    RouteClose,
    RequestCancel,
}

async fn interrupt_held_transform(interruption: Interruption) {
    let (mut gate, hook) = BlockingGate::new();
    let mut host = DaemonHost::start(hook).await;
    host.assert_scratch_released();
    let baseline = watchdog(host.client.host_status()).await.unwrap();
    assert_eq!(baseline.shared_memory["state"], "healthy");
    assert!(baseline.shared_memory["accounting"]["active"].is_object());
    let (_, ingress_available) = host.probe.body.ingress_charge_for_test();
    let ingress_baseline = ingress_available();
    let mut response = host.transform().await;
    let (cancel, ingress_held) = watchdog(&mut host.request_seen).await.unwrap();
    gate.wait_entered().await;
    let committed = host.store.load("ses").unwrap();
    assert!(committed.row_version.is_some());
    assert!(committed.meta.initialized);
    assert!(!cancel.is_cancelled());
    assert!(ingress_held > 0);
    // Raw input can drop on handler abort; decoded scratch remains owned by the worker.
    assert_eq!(ingress_available(), ingress_baseline - ingress_held);

    match interruption {
        Interruption::RouteClose => watchdog(host.client.close_route(host.route)).await.unwrap(),
        Interruption::RequestCancel => response.cancel().unwrap(),
    }
    watchdog(cancel.cancelled()).await;
    assert_eq!(host.core.bindings.lock().unwrap().by_route.len(), 1);
    assert!(
        host.core
            .bindings
            .lock()
            .unwrap()
            .get(&host.route.handle())
            .is_some()
    );
    assert_eq!(
        host.core.transform_units.available_permits(),
        TRANSFORM_UNITS_AT_ONCE - 1
    );
    assert!(
        host.probe
            .try_reserve_resident(host.probe.resident_capacity())
            .is_none()
    );
    assert_eq!(host.gone.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(host.error_published.try_recv(), Err(TryRecvError::Empty));
    assert!(!host.join.is_finished());
    assert_eq!(
        host.store.load("ses").unwrap().row_version,
        committed.row_version
    );
    drop(gate);
    if matches!(interruption, Interruption::RequestCancel) {
        // ResponseStream::cancel settles its receiver locally. The publication hook proves a server Error was emitted, but does not expose the error code.
        assert_eq!(
            watchdog(&mut host.error_published).await.unwrap(),
            (host.route.handle().channel, TRANSFORM_UNITS_AT_ONCE),
            "server Error publication must follow transform unit completion"
        );
        assert_eq!(ingress_available(), ingress_baseline);
        assert_eq!(
            host.core.transform_units.available_permits(),
            TRANSFORM_UNITS_AT_ONCE
        );
        host.assert_scratch_released();
        assert_eq!(host.core.bindings.lock().unwrap().by_route.len(), 1);
        assert!(
            host.core
                .bindings
                .lock()
                .unwrap()
                .get(&host.route.handle())
                .is_some()
        );
        assert_eq!(host.gone.try_recv(), Err(TryRecvError::Empty));
        assert!(!host.join.is_finished());
        watchdog(host.client.close_route(host.route)).await.unwrap();
    }
    assert_eq!(
        watchdog(&mut host.gone).await.unwrap(),
        (host.route.handle(), TRANSFORM_UNITS_AT_ONCE),
        "route-gone must not overlap a live transform unit"
    );
    assert_eq!(ingress_available(), ingress_baseline);
    assert!(host.core.bindings.lock().unwrap().by_route.is_empty());
    assert!(
        host.core
            .transform_route_channels
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(
        !host
            .core
            .transform_session_roots
            .lock()
            .unwrap()
            .contains_key("ses")
    );
    assert_eq!(
        host.core.transform_units.available_permits(),
        TRANSFORM_UNITS_AT_ONCE
    );
    host.assert_scratch_released();
    let persisted = host.store.load("ses").unwrap();
    assert!(persisted.row_version >= committed.row_version);
    assert_eq!(
        persisted.core, committed.core,
        "late cancellation cannot undo a durable commit"
    );
    let after = watchdog(host.client.host_status()).await.unwrap();
    assert_eq!(after.shared_memory["state"], "healthy");
    assert_eq!(
        after.shared_memory["accounting"],
        baseline.shared_memory["accounting"]
    );
    host.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn route_close_keeps_binding_and_scratch_until_transform_finishes() {
    interrupt_held_transform(Interruption::RouteClose).await;
}

#[tokio::test(flavor = "current_thread")]
async fn request_cancel_waits_for_committed_transform_and_releases_scratch() {
    interrupt_held_transform(Interruption::RequestCancel).await;
}

#[tokio::test(flavor = "current_thread")]
async fn transform_panic_is_redacted_and_maps_to_wire_internal_error() {
    let child_name = format!(
        "{}::transform_panic_child",
        module_path!().split_once("::").unwrap().1,
    );
    let child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", &child_name, "--nocapture"])
        .env(PANIC_CHILD_ENV, "1")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let output = watchdog(child.wait_with_output()).await.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed"), "the exact child test must run");
    assert!(stderr.contains("eidnara-host handler callback panicked (details redacted)"));
    assert!(
        !stderr.contains(PANIC_CANARY),
        "panic payload escaped redaction"
    );
    assert!(
        !stdout.contains(PANIC_CANARY),
        "panic payload escaped to stdout"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "subprocess role launched by the panic-redaction parent test"]
async fn transform_panic_child() {
    assert_eq!(std::env::var(PANIC_CHILD_ENV).as_deref(), Ok("1"));
    let mut host = DaemonHost::start(Box::new(|| panic!("{PANIC_CANARY}"))).await;
    let (_, ingress_available) = host.probe.body.ingress_charge_for_test();
    let ingress_baseline = ingress_available();
    let mut response = host.transform().await;
    let terminal = watchdog(response.next()).await.unwrap_err();
    assert_eq!(terminal.code(), "host.internal_error");
    assert_eq!(terminal.outcome(), SendOutcome::Terminal);
    assert_eq!(ingress_available(), ingress_baseline);
    assert_eq!(
        watchdog(&mut host.error_published).await.unwrap(),
        (host.route.handle().channel, TRANSFORM_UNITS_AT_ONCE),
        "server Error publication must follow transform unit completion"
    );
    assert!(
        host.store.load("ses").unwrap().row_version.is_some(),
        "panic hook must follow the real commit"
    );
    assert_eq!(
        host.core.transform_units.available_permits(),
        TRANSFORM_UNITS_AT_ONCE
    );
    host.assert_scratch_released();
    watchdog(host.client.close_route(host.route)).await.unwrap();
    assert_eq!(
        watchdog(&mut host.gone).await.unwrap(),
        (host.route.handle(), TRANSFORM_UNITS_AT_ONCE),
        "route-gone must not overlap a live transform unit"
    );
    assert_eq!(ingress_available(), ingress_baseline);
    assert!(host.core.bindings.lock().unwrap().by_route.is_empty());
    host.stop().await;
}
