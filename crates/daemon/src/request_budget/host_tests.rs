use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use host_runtime::{
    BindOutcome, CancelSignal, Client, ClientRoute, HealthReport, HostConfig, HostError,
    HostHandler, HostInit, InitError, ManifestSnapshot, RequestCtx, RequestOptions, RequestOutcome,
    ResourceDeclaration, RouteHandle, RouteIdentity, RouteTarget, TargetKind,
};
use storage::{GuardedConn, StoreError};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::request_budget::{BlockingFailure, Exhaustion, RequestBudget};
use crate::search_projection::{SearchProjection, SearchProjectionError};

const HELD_BODY: &[u8] = b"budget-held-read";
/// The budget derives from a token the test never cancels, so only the guard's drop can raise the interrupt.
const DROP_ONLY_BODY: &[u8] = b"budget-drop-only";
const PANIC_BODY: &[u8] = b"budget-panic";
const LONG_SCAN: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000000) SELECT count(*) FROM c";
const CEILING: Duration = Duration::from_secs(30);

struct ReadReport {
    finished: Instant,
    exhausted: bool,
    exhaustion: Option<Exhaustion>,
}

struct BudgetHandler {
    handler: Handler,
    projection: Arc<SearchProjection>,
    activated: Mutex<Option<oneshot::Sender<()>>>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    report: Mutex<Option<oneshot::Sender<ReadReport>>>,
    failure: Mutex<Option<oneshot::Sender<BlockingFailure>>>,
}

impl HostHandler for BudgetHandler {
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
        let _ = self.activated.lock().unwrap().take().unwrap().send(());
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
        if ctx.body.as_slice() == PANIC_BODY {
            let outcome = ctx
                .run_blocking(|| -> () { panic!("budget probe panics") })
                .await;
            let failure = BlockingFailure::from(outcome.unwrap_err());
            let _ = self.failure.lock().unwrap().take().unwrap().send(failure);
            return RequestOutcome::error("internal_error", "blocking work failed");
        }
        let cancel = match ctx.body.as_slice() {
            HELD_BODY => ctx.cancel_signal(),
            DROP_ONLY_BODY => CancelSignal::observing(CancellationToken::new()),
            _ => return self.handler.handle(ctx).await,
        };
        let budget = RequestBudget::derive(cancel, Some(60_000), Some(CEILING)).unwrap();
        let shared = budget.shared().clone();
        let projection = Arc::clone(&self.projection);
        let report = self.report.lock().unwrap().take().unwrap();
        let entered = self.entered.lock().unwrap().take().unwrap();
        let read = ctx.run_blocking(move || {
            let _ = entered.send(());
            let result = projection.read_under(&shared, |conn: &GuardedConn<'_>| {
                Ok(conn.query_row(LONG_SCAN, [], |row| row.get::<_, i64>(0))?)
            });
            let _ = report.send(ReadReport {
                finished: Instant::now(),
                exhausted: matches!(
                    result,
                    Err(SearchProjectionError::Store(StoreError::Deadline))
                ),
                exhaustion: shared.exhaustion(),
            });
        });
        // No `select!` arm here; the host aborts this future on cancel and the guard drops with it.
        match read.await {
            Ok(()) => RequestOutcome::error("unexpected", "the held read completed"),
            Err(_) => RequestOutcome::error("internal_error", "blocking work failed"),
        }
    }

    async fn route_gone(&self, route: RouteHandle) {
        self.handler.route_gone(route).await;
    }

    async fn health(&self) -> HealthReport {
        self.handler.health().await
    }

    async fn shutdown(&self) {
        self.handler.shutdown().await.unwrap();
    }
}

struct BudgetHost {
    projection: Arc<SearchProjection>,
    client: Client,
    route: ClientRoute,
    entered: oneshot::Receiver<()>,
    report: oneshot::Receiver<ReadReport>,
    failure: oneshot::Receiver<BlockingFailure>,
    error_published: oneshot::Receiver<Instant>,
    join: tokio::task::JoinHandle<Result<(), HostError>>,
    stop_on_drop: tokio_util::sync::DropGuard,
    _dir: tempfile::TempDir,
}

impl BudgetHost {
    async fn start() -> Self {
        let (handler, _store, dir, project) =
            handler_with_store(Arc::new(ProducerState::default()), default_test_config());
        handler.unbind_route(test_route(7));
        let projection = Arc::new(SearchProjection::open(&dir.path().join("search")).unwrap());
        let (activated, ready) = oneshot::channel();
        let (entered_tx, entered) = oneshot::channel();
        let (report_tx, report) = oneshot::channel();
        let (failure_tx, failure) = oneshot::channel();
        let observed = BudgetHandler {
            handler,
            projection: Arc::clone(&projection),
            activated: Mutex::new(Some(activated)),
            entered: Mutex::new(Some(entered_tx)),
            report: Mutex::new(Some(report_tx)),
            failure: Mutex::new(Some(failure_tx)),
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
        config.timing.route_close_budget = Duration::from_secs(5);
        config.timing.shutdown_deadline = Duration::from_secs(10);
        let publication = host_runtime::runtime_dir_path(config.data_dir.as_deref())
            .unwrap()
            .join(host_runtime::CONNECTION_FILE_NAME);
        let shutdown = CancellationToken::new();
        let stop_on_drop = shutdown.clone().drop_guard();
        let (error_tx, error_published) = oneshot::channel();
        let error_tx = Mutex::new(Some(error_tx));
        let publish_hook = Arc::new(move |kind, channel| {
            if kind == host_runtime::wire::FrameType::Error
                && channel != 0
                && let Some(send) = error_tx.lock().unwrap().take()
            {
                let _ = send.send(Instant::now());
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
        Self {
            projection,
            client,
            route,
            entered,
            report,
            failure,
            error_published,
            join,
            stop_on_drop,
            _dir: dir,
        }
    }

    fn connection_is_held(&self) -> bool {
        matches!(
            self.projection
                .read_within(Instant::now() + Duration::from_millis(50), |_| Ok(())),
            Err(SearchProjectionError::Store(StoreError::Deadline))
        )
    }

    async fn wait_until_held(&self, held: bool) {
        watchdog(async {
            while self.connection_is_held() != held {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }

    async fn stop(self) {
        watchdog(self.client.close()).await.unwrap();
        drop(self.stop_on_drop);
        watchdog(self.join)
            .await
            .unwrap()
            .expect("host must shut down cleanly");
    }
}

async fn watchdog<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .expect("test operation must finish before the watchdog")
}

async fn cancel_held_read(body: &[u8]) {
    let mut host = BudgetHost::start().await;
    let mut response = watchdog(host.client.request_stream(
        host.route,
        body.to_vec(),
        RequestOptions::default(),
    ))
    .await
    .unwrap();
    watchdog(&mut host.entered).await.unwrap();
    host.wait_until_held(true).await;
    assert!(host.report.try_recv().is_err());

    let cancelled_at = Instant::now();
    response.cancel().unwrap();
    let published = watchdog(&mut host.error_published).await.unwrap();
    let report = watchdog(&mut host.report).await.unwrap();
    assert!(report.exhausted, "the read reports exhaustion, not a value");
    assert_eq!(report.exhaustion, Some(Exhaustion::Cancelled));
    assert!(report.finished >= cancelled_at);
    assert!(
        report.finished <= published,
        "the blocking read must be joined before the request settles"
    );
    assert!(
        report.finished - cancelled_at < Duration::from_secs(5),
        "the interrupt lands at a polled step, not at the deadline"
    );
    host.wait_until_held(false).await;
    assert_eq!(
        host.projection
            .read_within(Instant::now() + Duration::from_secs(5), |conn| Ok(
                conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?
            ))
            .unwrap(),
        1
    );
    host.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_suspended_handler_interrupts_the_held_read_and_joins_it_before_settling() {
    cancel_held_read(HELD_BODY).await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_guard_drop_alone_interrupts_the_held_read_when_the_host_aborts_the_handler() {
    cancel_held_read(DROP_ONLY_BODY).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_panic_in_tracked_blocking_work_is_typed_and_still_settles() {
    let mut host = BudgetHost::start().await;
    let mut response = watchdog(host.client.request_stream(
        host.route,
        PANIC_BODY.to_vec(),
        RequestOptions::default(),
    ))
    .await
    .unwrap();
    let terminal = watchdog(response.next()).await.unwrap_err();
    assert_eq!(terminal.code(), "host.internal_error");
    assert_eq!(
        watchdog(&mut host.failure).await.unwrap(),
        BlockingFailure::Panicked
    );
    host.stop().await;
}
