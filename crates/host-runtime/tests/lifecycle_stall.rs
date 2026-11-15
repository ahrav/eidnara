mod support;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use host_runtime::{CancellationToken, HostConfig, HostError, HostHandler, HostLimits, HostTiming};

use support::raw_client::{self, RawClient, TY_RESPONSE};
use support::{LINKED_MODULE_ID, TestHandler, connection_file};

const BUDGET: Duration = Duration::from_secs(20);
const ROOT: &str = "/workspace/project";

#[derive(Clone, Default)]
struct Gate {
    armed: Arc<AtomicBool>,
    entered: Arc<tokio::sync::watch::Sender<usize>>,
    open: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Gate {
    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn pass(&self) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        self.entered.send_modify(|entered| *entered += 1);
        self.open
            .subscribe()
            .wait_for(|open| *open)
            .await
            .expect("the gate outlives its callbacks");
    }

    async fn entered(&self, count: usize) {
        self.entered
            .subscribe()
            .wait_for(|entered| *entered >= count)
            .await
            .expect("the gate outlives its callbacks");
    }

    fn open(&self) {
        self.open.send_replace(true);
    }
}

#[derive(Clone)]
struct GatedHandler {
    inner: TestHandler,
    initialize: Gate,
    bind: Gate,
    route_gone: Gate,
    health: Gate,
    shutdown: Gate,
}

impl GatedHandler {
    fn new() -> Self {
        Self {
            inner: TestHandler::new(),
            initialize: Gate::default(),
            bind: Gate::default(),
            route_gone: Gate::default(),
            health: Gate::default(),
            shutdown: Gate::default(),
        }
    }
}

impl HostHandler for GatedHandler {
    fn manifests(&self) -> Vec<host_runtime::ManifestSnapshot> {
        self.inner.manifests()
    }

    async fn initialize(
        &self,
        init: host_runtime::HostInit,
    ) -> Result<(), host_runtime::InitError> {
        self.initialize.pass().await;
        self.inner.initialize(init).await
    }

    async fn bind(
        &self,
        route: host_runtime::RouteHandle,
        target: host_runtime::RouteTarget,
        identity: host_runtime::RouteIdentity,
    ) -> host_runtime::BindOutcome {
        self.bind.pass().await;
        self.inner.bind(route, target, identity).await
    }

    async fn handle(&self, ctx: host_runtime::RequestCtx) -> host_runtime::RequestOutcome {
        self.inner.handle(ctx).await
    }

    async fn route_gone(&self, route: host_runtime::RouteHandle) {
        self.route_gone.pass().await;
        self.inner.route_gone(route).await;
    }

    async fn health(&self) -> host_runtime::HealthReport {
        self.health.pass().await;
        self.inner.health().await
    }

    async fn shutdown(&self) {
        self.shutdown.pass().await;
        self.inner.shutdown().await;
    }
}

fn host_config(data_root: &std::path::Path, timing: HostTiming) -> HostConfig {
    HostConfig {
        data_dir: Some(data_root.to_path_buf()),
        daemon_ver: "eidnara-host/test".to_owned(),
        limits: HostLimits {
            max_resident_bytes: host_runtime::config::MIN_RESIDENT_BYTES * 2,
            ..Default::default()
        },
        timing,
        ..Default::default()
    }
}

struct RunningHost {
    handler: GatedHandler,
    shutdown: CancellationToken,
    run: tokio::task::JoinHandle<Result<(), HostError>>,
    data_root: tempfile::TempDir,
}

impl RunningHost {
    fn start(handler: GatedHandler, timing: HostTiming) -> Self {
        let data_root = tempfile::tempdir().expect("temp data root");
        let shutdown = CancellationToken::new();
        let config = host_config(data_root.path(), timing);
        let run = tokio::spawn(host_runtime::run(handler.clone(), config, shutdown.clone()));
        Self {
            handler,
            shutdown,
            run,
            data_root,
        }
    }

    async fn until<T>(&mut self, reached: impl Future<Output = T>) -> T {
        tokio::select! {
            result = &mut self.run => panic!("the host stopped early: {result:?}"),
            output = reached => output,
        }
    }

    async fn client(&mut self) -> RawClient {
        let publication = connection_file(self.data_root.path());
        self.until(async {
            while !publication.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let info = raw_client::discover(&publication).expect("publication validates");
        RawClient::connect(&info).await.expect("client connects")
    }

    async fn stop(self) -> Result<(), HostError> {
        self.shutdown.cancel();
        self.run.await.expect("run task")
    }

    async fn stopped_within(self, budget: Duration) -> Result<(), HostError> {
        tokio::time::timeout(budget, self.run)
            .await
            .expect("the host stops inside the budget")
            .expect("run task")
    }
}

async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

async fn jump(by: Duration) {
    tokio::time::pause();
    tokio::time::advance(by).await;
    settle().await;
    tokio::time::resume();
}

fn short_deadline() -> HostTiming {
    HostTiming {
        lifecycle_callback_deadline: Duration::from_secs(2),
        ..Default::default()
    }
}

fn route_open() -> serde_json::Value {
    serde_json::json!({
        "op": "route.open",
        "target": {"kind": "tool_provider", "module_id": LINKED_MODULE_ID},
        "identity": {"project_root": ROOT, "harness": "opencode", "session": "stalled"}
    })
}

fn expect_health_fatal(result: Result<(), HostError>) {
    match result {
        Err(HostError::LifecycleFatal(message)) => {
            assert_eq!(message, "health callback deadline expired");
        }
        other => panic!("a hung health callback must be lifecycle-fatal, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_clock_jump_past_the_deadline_during_a_health_probe_leaves_the_host_running() {
    let handler = GatedHandler::new();
    handler.health.arm();
    let mut host = RunningHost::start(handler, HostTiming::default());
    let deadline = HostTiming::default().lifecycle_callback_deadline;
    let entered = host.handler.health.clone();
    host.until(entered.entered(1)).await;

    tokio::time::advance(deadline * 3).await;
    settle().await;
    assert!(
        !host.run.is_finished(),
        "a clock jump while the callback waits must leave the host running"
    );

    host.handler.health.open();
    host.until(entered.entered(2)).await;
    host.stop()
        .await
        .expect("the host shuts down gracefully after the stall");
}

#[tokio::test(start_paused = true)]
async fn a_health_callback_that_never_returns_is_fatal_at_the_deadline() {
    let handler = GatedHandler::new();
    handler.health.arm();
    let mut host = RunningHost::start(handler, HostTiming::default());
    let deadline = HostTiming::default().lifecycle_callback_deadline;
    let entered = host.handler.health.clone();
    host.until(entered.entered(1)).await;
    let started = tokio::time::Instant::now();

    expect_health_fatal(host.stopped_within(deadline * 4).await);
    let waited = started.elapsed();
    assert!(
        waited >= deadline && waited < deadline + Duration::from_secs(1),
        "the latch trips once the callback has waited {deadline:?}, waited {waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_health_callback_that_hangs_through_a_clock_jump_is_fatal_one_deadline_later() {
    let handler = GatedHandler::new();
    handler.health.arm();
    let mut host = RunningHost::start(handler, HostTiming::default());
    let deadline = HostTiming::default().lifecycle_callback_deadline;
    let entered = host.handler.health.clone();
    host.until(entered.entered(1)).await;

    tokio::time::advance(deadline * 3).await;
    settle().await;
    assert!(!host.run.is_finished(), "the jump alone is not fatal");
    let resumed = tokio::time::Instant::now();

    expect_health_fatal(host.stopped_within(deadline * 4).await);
    let waited = resumed.elapsed();
    assert!(
        waited >= deadline - Duration::from_secs(1) && waited < deadline + Duration::from_secs(1),
        "the latch trips within one deadline of the resume, waited {waited:?}"
    );
}

#[tokio::test]
async fn a_clock_jump_during_initialization_leaves_startup_running() {
    let handler = GatedHandler::new();
    handler.initialize.arm();
    let timing = short_deadline();
    let deadline = timing.lifecycle_callback_deadline;
    let mut host = RunningHost::start(handler, timing);
    let entered = host.handler.initialize.clone();
    host.until(entered.entered(1)).await;

    jump(deadline * 3).await;
    assert!(!host.run.is_finished(), "the jump fails no initialization");

    host.handler.initialize.open();
    drop(host.client().await);
    host.stop().await.expect("graceful shutdown");
}

#[tokio::test]
async fn a_clock_jump_during_a_bind_callback_leaves_the_route_opening() {
    let handler = GatedHandler::new();
    handler.bind.arm();
    let timing = short_deadline();
    let deadline = timing.lifecycle_callback_deadline;
    let mut host = RunningHost::start(handler, timing);
    let mut client = host.client().await;
    let corr = client.control(&route_open()).await.expect("route open");
    let entered = host.handler.bind.clone();
    host.until(entered.entered(1)).await;

    jump(deadline * 3).await;
    assert!(!host.run.is_finished(), "the jump fails no bind");

    host.handler.bind.open();
    let frame = client.frame_within(BUDGET).await.expect("route reply");
    assert_eq!((frame.corr, frame.ty), (corr, TY_RESPONSE));
    drop(client);
    host.stop().await.expect("graceful shutdown");
}

#[tokio::test]
async fn a_clock_jump_during_a_route_gone_callback_leaves_the_host_running() {
    let handler = GatedHandler::new();
    handler.route_gone.arm();
    let timing = short_deadline();
    let deadline = timing.lifecycle_callback_deadline;
    let mut host = RunningHost::start(handler, timing);
    let mut client = host.client().await;
    client
        .route_open(LINKED_MODULE_ID, ROOT, "opencode", "stalled")
        .await
        .expect("route opens");
    drop(client);
    let entered = host.handler.route_gone.clone();
    host.until(entered.entered(1)).await;

    jump(deadline * 3).await;
    assert!(!host.run.is_finished(), "the jump fails no route-gone");

    host.handler.route_gone.open();
    let handler = host.handler.inner.clone();
    host.stop().await.expect("graceful shutdown");
    assert_eq!(handler.route_gones().len(), 1);
}

#[tokio::test]
async fn a_clock_jump_during_the_shutdown_callback_leaves_the_shutdown_graceful() {
    let handler = GatedHandler::new();
    handler.shutdown.arm();
    let timing = short_deadline();
    let deadline = timing.lifecycle_callback_deadline;
    let mut host = RunningHost::start(handler, timing);
    drop(host.client().await);
    host.shutdown.cancel();
    let entered = host.handler.shutdown.clone();
    host.until(entered.entered(1)).await;

    jump(deadline * 3).await;
    assert!(!host.run.is_finished(), "the callback still runs");

    host.handler.shutdown.open();
    host.run
        .await
        .expect("run task")
        .expect("graceful shutdown");
}

const ROLE_ENV: &str = "EIDNARA_HOST_STALL_ROLE";
const ROLE_SLOW_PROBE: &str = "slow-probe";
const ROLE_HUNG_PROBE: &str = "hung-probe";
const ROLE_SPINNING_PROBE: &str = "spinning-probe";
const SPIN: Duration = Duration::from_millis(300);
const CHILD_DEADLINE: Duration = Duration::from_secs(2);

struct TimedHealthHandler {
    inner: TestHandler,
    probes: AtomicUsize,
    hang_from: Option<usize>,
    spin: bool,
}

impl HostHandler for TimedHealthHandler {
    fn manifests(&self) -> Vec<host_runtime::ManifestSnapshot> {
        self.inner.manifests()
    }

    async fn initialize(
        &self,
        init: host_runtime::HostInit,
    ) -> Result<(), host_runtime::InitError> {
        self.inner.initialize(init).await
    }

    async fn bind(
        &self,
        route: host_runtime::RouteHandle,
        target: host_runtime::RouteTarget,
        identity: host_runtime::RouteIdentity,
    ) -> host_runtime::BindOutcome {
        self.inner.bind(route, target, identity).await
    }

    async fn handle(&self, ctx: host_runtime::RequestCtx) -> host_runtime::RequestOutcome {
        self.inner.handle(ctx).await
    }

    async fn route_gone(&self, route: host_runtime::RouteHandle) {
        self.inner.route_gone(route).await;
    }

    async fn health(&self) -> host_runtime::HealthReport {
        let probe = self.probes.fetch_add(1, Ordering::SeqCst) + 1;
        announce(&format!("ENTER {probe}"));
        if self.hang_from.is_some_and(|first| probe >= first) {
            std::future::pending::<()>().await;
        }
        if self.spin {
            let started = std::time::Instant::now();
            while started.elapsed() < SPIN {
                std::hint::spin_loop();
            }
            announce(&format!("SPUN {probe}"));
        }
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        announce(&format!("PROBE {probe}"));
        self.inner.health().await
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

fn announce(message: &str) {
    println!("{message}");
    std::io::stdout().flush().expect("flush role line");
}

#[test]
#[ignore = "host role for the stopped-process tests"]
fn stall_role_host() {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        return;
    };
    let data_root = tempfile::tempdir().expect("temp data root");
    let handler = TimedHealthHandler {
        inner: TestHandler::new(),
        probes: AtomicUsize::new(0),
        hang_from: (role == ROLE_HUNG_PROBE).then_some(3),
        spin: role == ROLE_SPINNING_PROBE,
    };
    let config = host_config(
        data_root.path(),
        HostTiming {
            lifecycle_callback_deadline: CHILD_DEADLINE,
            health_interval: Duration::from_millis(10),
            ..Default::default()
        },
    );
    let shutdown = CancellationToken::new();
    let stdin_closed = shutdown.clone();
    std::thread::spawn(move || {
        let mut sink = String::new();
        while std::io::stdin().read_line(&mut sink).is_ok_and(|n| n > 0) {}
        stdin_closed.cancel();
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("host runtime");
    match runtime.block_on(host_runtime::run(handler, config, shutdown)) {
        Ok(()) => announce("STOPPED graceful"),
        Err(err) => announce(&format!("STOPPED {err}")),
    }
}

struct HostProcess {
    child: Child,
    lines: Receiver<String>,
}

impl HostProcess {
    fn spawn(role: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--ignored", "--exact", "stall_role_host", "--nocapture"])
            .env(ROLE_ENV, role)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn host role");
        let stdout = child.stdout.take().expect("host stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines }
    }

    fn expect_line(&self, prefix: &str) -> String {
        let deadline = std::time::Instant::now() + BUDGET;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.starts_with(prefix) => return line,
                Ok(line) => assert!(
                    !line.starts_with("STOPPED"),
                    "the host stopped while the test waited for `{prefix}`: {line}"
                ),
                Err(RecvTimeoutError::Timeout) => panic!("no `{prefix}` line within {BUDGET:?}"),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the host closed its output before a `{prefix}` line")
                }
            }
        }
    }

    fn signal(&self, signal: rustix::process::Signal) {
        let pid = rustix::process::Pid::from_child(&self.child);
        rustix::process::kill_process(pid, signal).expect("signal the host process");
    }

    fn stop_before(&self, after_entry: Duration, marker: &str) -> usize {
        for _ in 0..10 {
            let entered = self.expect_line("ENTER ");
            let probe: usize = entered["ENTER ".len()..].parse().expect("probe number");
            std::thread::sleep(after_entry);
            self.signal(rustix::process::Signal::STOP);
            std::thread::sleep(Duration::from_millis(100));
            let reached = format!("{marker} {probe}");
            if self.lines.try_iter().all(|line| line != reached) {
                return probe;
            }
            self.signal(rustix::process::Signal::CONT);
        }
        panic!("no stop landed before a `{marker}` line");
    }

    fn resume_after(&mut self, stopped_in: usize, stopped_for: Duration) {
        std::thread::sleep(stopped_for);
        assert_eq!(
            self.lines.try_iter().next(),
            None,
            "a stopped host writes nothing"
        );
        self.signal(rustix::process::Signal::CONT);

        self.expect_line(&format!("PROBE {stopped_in}"));
        self.expect_line(&format!("PROBE {}", stopped_in + 2));
        drop(self.child.stdin.take());
        assert_eq!(self.expect_line("STOPPED"), "STOPPED graceful");
        let status = self.child.wait().expect("host exit status");
        assert!(status.success(), "the host role exits cleanly: {status:?}");
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_host_process_stopped_past_the_deadline_during_a_health_probe_keeps_serving() {
    let mut host = HostProcess::spawn(ROLE_SLOW_PROBE);
    host.expect_line("PROBE 1");
    let stopped_in = host.stop_before(Duration::from_millis(20), "PROBE");
    host.resume_after(stopped_in, CHILD_DEADLINE * 2);
}

#[test]
fn a_host_process_stopped_past_the_deadline_inside_a_health_callback_poll_keeps_serving() {
    let mut host = HostProcess::spawn(ROLE_SPINNING_PROBE);
    host.expect_line("PROBE 1");
    let stopped_in = host.stop_before(SPIN / 3, "SPUN");
    host.resume_after(stopped_in, CHILD_DEADLINE * 2);
}

#[test]
fn a_host_process_whose_health_callback_hangs_stops_with_the_fatal_diagnostic() {
    let host = HostProcess::spawn(ROLE_HUNG_PROBE);
    host.expect_line("ENTER 3");
    let hung = std::time::Instant::now();
    let stopped = host.expect_line("STOPPED");
    assert_eq!(
        stopped,
        "STOPPED fatal lifecycle callback failure: health callback deadline expired"
    );
    assert!(
        hung.elapsed() >= CHILD_DEADLINE - Duration::from_millis(500),
        "the latch trips once the callback has hung for the deadline"
    );
}
