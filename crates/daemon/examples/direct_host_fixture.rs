//! Runs a test-only, directly linked `host-runtime` process on Unix.
//!
//! A line-delimited JSON control socket selects deterministic backend outcomes.
//! Control lines are capped at 64 KiB, socket and state paths are owner-only,
//! and blocked calls are released in FIFO registration order.

#![forbid(unsafe_code)]

#[cfg(unix)]
mod unix {
    use std::collections::VecDeque;
    use std::error::Error;
    use std::fs;
    use std::io::{self, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use host_runtime::broca::BrocaComponent;
    use host_runtime::broca::backend::{
        BackendError, BackendEvent, BackendFuture, BackendRequest, BackendTerminal, ErrorClass,
        EventSink, FinishReason, LlmExecutionBackend,
    };
    use host_runtime::synapse::inference::InferenceError;
    use host_runtime::synapse::{EmbeddingEngine, LaneInfo, SynapseComponent, SynapseLimits};
    use host_runtime::{CancellationToken, HostConfig, HostInit, StaticComposite};
    use serde::{Deserialize, Serialize};
    use sha2::Digest;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::oneshot;

    const CONTROL_FILE: &str = "direct-host-control.sock";
    const MAX_CONTROL_LINE: usize = 64 * 1024;
    const READY_TIMEOUT: Duration = Duration::from_secs(30);
    const CATALOG: [&str; 3] = [
        daemon::DEFAULT_MODULE_ID,
        host_runtime::synapse::SYNAPSE_MODULE_ID,
        host_runtime::broca::BROCA_MODULE_ID,
    ];

    #[derive(Debug, Clone, Copy)]
    enum NextBehavior {
        Success,
        Block,
        Failure,
    }

    /// Monotonic counters shared by backend futures and control connections.
    ///
    /// Sequential consistency keeps snapshots easy to interpret across threads;
    /// snapshots are observations, not atomic multi-counter transactions.
    #[derive(Default)]
    struct BackendCounters {
        started: AtomicU64,
        completed: AtomicU64,
        blocked: AtomicU64,
        active_blocked: AtomicU64,
        released: AtomicU64,
        failed: AtomicU64,
        cancelled: AtomicU64,
    }

    #[derive(Serialize)]
    struct CounterSnapshot {
        started: u64,
        completed: u64,
        blocked: u64,
        released: u64,
        failed: u64,
        cancelled: u64,
    }

    impl BackendCounters {
        fn snapshot(&self) -> CounterSnapshot {
            CounterSnapshot {
                started: self.started.load(Ordering::SeqCst),
                completed: self.completed.load(Ordering::SeqCst),
                blocked: self.blocked.load(Ordering::SeqCst),
                released: self.released.load(Ordering::SeqCst),
                failed: self.failed.load(Ordering::SeqCst),
                cancelled: self.cancelled.load(Ordering::SeqCst),
            }
        }
    }

    /// One release channel per blocked invocation, oldest first.
    ///
    /// A counting semaphore alone can retain a release that cancellation leaves unconsumed, letting a later `Block` invocation resume without a matching `release-blocked-call`.
    /// cancellation wins the `biased` race in `execute` stays in the semaphore,
    /// and the next `Block` run consumes it immediately and completes without a
    /// matching `release-blocked-call`. That silently rewrites the fault schedule
    /// an E2E scenario is asserting against. Addressing a specific invocation
    /// makes an unconsumed release impossible to mistake for a pending one.
    /// The queued sender delivers the acknowledgment channel the run uses to confirm it resumed.
    type BlockedQueue = Arc<Mutex<VecDeque<(u64, oneshot::Sender<oneshot::Sender<()>>)>>>;

    struct ControlledBackend {
        next: Mutex<NextBehavior>,
        blocked: BlockedQueue,
        next_blocked_id: Arc<AtomicU64>,
        shutdown: CancellationToken,
        counters: Arc<BackendCounters>,
    }

    impl ControlledBackend {
        fn new(shutdown: CancellationToken) -> Arc<Self> {
            Arc::new(Self {
                next: Mutex::new(NextBehavior::Success),
                blocked: Arc::new(Mutex::new(VecDeque::new())),
                next_blocked_id: Arc::new(AtomicU64::new(0)),
                shutdown,
                counters: Arc::new(BackendCounters::default()),
            })
        }

        fn set_next(&self, behavior: NextBehavior) {
            *self.next.lock().expect("fixture backend behavior mutex") = behavior;
        }

        /// Hands one release to a blocked invocation and waits for it to resume.
        ///
        /// `oneshot::Sender::send` only proves the receiver still existed, not
        /// that the run selected the release branch: the `biased` select prefers
        /// shutdown and cancellation, so a release handed over at that instant is
        /// never consumed. Counting it here would report one invocation as both
        /// for a run that never resumed.
        /// The run acknowledges release consumption.
        /// A release that lost the race moves to the next waiting invocation.
        async fn release_blocked(&self) -> bool {
            loop {
                let Some((_id, sender)) = self
                    .blocked
                    .lock()
                    .expect("fixture blocked queue mutex")
                    .pop_front()
                else {
                    return false;
                };
                let (ack, resumed) = oneshot::channel();
                if sender.send(ack).is_err() {
                    // The invocation ended before the handoff; it already
                    // withdrew itself and counted its own outcome.
                    continue;
                }
                if resumed.await.is_ok() {
                    return true;
                }
                // Handed over but not consumed: the run lost to cancellation and
                // counted itself cancelled. Offer this release to the next one.
            }
        }

        fn terminal_error(message: &str) -> BackendTerminal {
            BackendTerminal::Failed(BackendError {
                class: ErrorClass::Permanent,
                message: message.to_owned(),
                retry_after_secs: None,
                provider_code: Some("fixture_terminal".to_owned()),
            })
        }
    }

    fn take_blocked_slot(counters: &BackendCounters) -> bool {
        counters
            .active_blocked
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                active.checked_sub(1)
            })
            .is_ok()
    }

    fn register_blocked(
        queue: &BlockedQueue,
        next_id: &AtomicU64,
    ) -> (u64, oneshot::Receiver<oneshot::Sender<()>>) {
        let id = next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        queue
            .lock()
            .expect("fixture blocked queue mutex")
            .push_back((id, tx));
        (id, rx)
    }

    /// Withdraws a blocked invocation that ended without consuming a release, so
    /// its slot cannot be handed a release no run is waiting for.
    fn withdraw_blocked(queue: &BlockedQueue, id: u64) {
        queue
            .lock()
            .expect("fixture blocked queue mutex")
            .retain(|(queued, _)| *queued != id);
    }

    impl LlmExecutionBackend for ControlledBackend {
        fn execute(
            &self,
            _request: BackendRequest,
            events: EventSink,
            cancel: CancellationToken,
        ) -> BackendFuture {
            self.counters.started.fetch_add(1, Ordering::SeqCst);
            let behavior = std::mem::replace(
                &mut *self.next.lock().expect("fixture backend behavior mutex"),
                NextBehavior::Success,
            );
            let blocked = Arc::clone(&self.blocked);
            let next_blocked_id = Arc::clone(&self.next_blocked_id);
            let shutdown = self.shutdown.clone();
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                match behavior {
                    NextBehavior::Success => {
                        events.emit(BackendEvent::AssistantText {
                            text: "fixture-success".to_owned(),
                            finish_reason: None,
                        });
                        counters.completed.fetch_add(1, Ordering::SeqCst);
                        BackendTerminal::Completed {
                            finish_reason: FinishReason::Completed,
                        }
                    }
                    NextBehavior::Failure => {
                        counters.failed.fetch_add(1, Ordering::SeqCst);
                        ControlledBackend::terminal_error("fixture requested typed failure")
                    }
                    NextBehavior::Block => {
                        counters.blocked.fetch_add(1, Ordering::SeqCst);
                        counters.active_blocked.fetch_add(1, Ordering::SeqCst);
                        // Registered before the race so a release issued while
                        // this run is waiting reaches this run and no other.
                        let (id, release) = register_blocked(&blocked, &next_blocked_id);
                        tokio::select! {
                            biased;
                            () = shutdown.cancelled() => {
                                withdraw_blocked(&blocked, id);
                                take_blocked_slot(&counters);
                                counters.cancelled.fetch_add(1, Ordering::SeqCst);
                                ControlledBackend::terminal_error("fixture shutting down")
                            }
                            () = cancel.cancelled() => {
                                withdraw_blocked(&blocked, id);
                                take_blocked_slot(&counters);
                                counters.cancelled.fetch_add(1, Ordering::SeqCst);
                                ControlledBackend::terminal_error("fixture run cancelled")
                            }
                            granted = release => {
                                // This branch is the only place a release counts,
                                // so no invocation can be both released and
                                // cancelled. A dropped grant means the fixture is
                                // going away, which the cancellation branches own.
                                let Ok(ack) = granted else {
                                    withdraw_blocked(&blocked, id);
                                    take_blocked_slot(&counters);
                                    counters.cancelled.fetch_add(1, Ordering::SeqCst);
                                    return ControlledBackend::terminal_error("fixture release dropped");
                                };
                                take_blocked_slot(&counters);
                                counters.released.fetch_add(1, Ordering::SeqCst);
                                // The acknowledgment guarantees that the release waiter observes updated counters.
                                // the counters.
                                let _ = ack.send(());
                                events.emit(BackendEvent::AssistantText {
                                    text: "fixture-released".to_owned(),
                                    finish_reason: None,
                                });
                                counters.completed.fetch_add(1, Ordering::SeqCst);
                                BackendTerminal::Completed {
                                    finish_reason: FinishReason::Completed,
                                }
                            }
                        }
                    }
                }
            })
        }
    }

    struct DeterministicEngine;

    impl EmbeddingEngine for DeterministicEngine {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, InferenceError> {
            Ok(texts
                .iter()
                .map(|text| {
                    let digest = sha2::Sha256::digest(text.as_bytes());
                    let mut vector: Vec<f32> = digest[..8]
                        .iter()
                        .map(|byte| f32::from(*byte) + 1.0)
                        .collect();
                    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
                    for value in &mut vector {
                        *value /= norm;
                    }
                    vector
                })
                .collect())
        }
    }

    fn synapse_component() -> SynapseComponent {
        let limits = SynapseLimits::default();
        SynapseComponent::ready_with_engine(
            LaneInfo {
                model: "direct-host-fixture".to_owned(),
                fingerprint: "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234"
                    .to_owned(),
                table_epoch: 1,
                dims: 8,
                execution_provider: "cpu",
                max_tokens: 512,
                max_text_bytes: limits.max_text_bytes,
                provenance: serde_json::json!({"source": "direct host fixture"}),
                recommended_rows: 16,
                recommended_token_budget: 8_192,
            },
            Arc::new(DeterministicEngine),
            limits,
        )
        .expect("fixture Synapse limits are valid")
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ControlRequest {
        id: u64,
        command: ControlCommand,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ControlId {
        id: u64,
        command: serde::de::IgnoredAny,
    }

    #[derive(Deserialize)]
    #[serde(tag = "name", rename_all = "kebab-case", deny_unknown_fields)]
    enum ControlCommand {
        BackendSuccess,
        BlockNextCall,
        ReleaseBlockedCall,
        TypedFailure,
        Counters,
        GracefulShutdown,
    }

    #[derive(Serialize)]
    struct ControlResponse<T: Serialize> {
        id: Option<u64>,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<T>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ControlError>,
    }

    #[derive(Serialize)]
    struct ControlError {
        code: &'static str,
        message: &'static str,
    }

    #[derive(Serialize)]
    #[serde(untagged)]
    enum ControlResult {
        Ack { accepted: bool },
        Counters(CounterSnapshot),
    }

    /// One control frame after draining through its newline or end of stream.
    enum BoundedLine {
        Line(Vec<u8>),
        Oversized,
    }

    async fn read_bounded_line(stream: &mut UnixStream) -> io::Result<Option<BoundedLine>> {
        let mut line = Vec::with_capacity(256);
        let mut oversized = false;
        let mut byte = [0u8; 1];
        loop {
            let read = stream.read(&mut byte).await?;
            if read == 0 {
                return if line.is_empty() && !oversized {
                    Ok(None)
                } else if oversized {
                    Ok(Some(BoundedLine::Oversized))
                } else {
                    Ok(Some(BoundedLine::Line(line)))
                };
            }
            if byte[0] == b'\n' {
                return if oversized {
                    Ok(Some(BoundedLine::Oversized))
                } else {
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    Ok(Some(BoundedLine::Line(line)))
                };
            }
            if line.len() < MAX_CONTROL_LINE {
                line.push(byte[0]);
            } else {
                oversized = true;
            }
        }
    }

    async fn write_response<T: Serialize>(
        stream: &mut UnixStream,
        response: &ControlResponse<T>,
    ) -> io::Result<()> {
        let bytes = serde_json::to_vec(response).expect("fixture response serializes");
        if bytes.len() > MAX_CONTROL_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture response exceeded control cap",
            ));
        }
        stream.write_all(&bytes).await?;
        stream.write_all(b"\n").await
    }

    fn rejected(id: Option<u64>, code: &'static str) -> ControlResponse<ControlResult> {
        ControlResponse {
            id,
            ok: false,
            result: None,
            error: Some(ControlError {
                code,
                message: "control request rejected",
            }),
        }
    }

    async fn handle_control_connection(
        mut stream: UnixStream,
        backend: Arc<ControlledBackend>,
        shutdown: CancellationToken,
    ) -> io::Result<()> {
        loop {
            let line = tokio::select! {
                biased;
                line = read_bounded_line(&mut stream) => line?,
                () = shutdown.cancelled() => return Ok(()),
            };
            let Some(line) = line else {
                return Ok(());
            };
            let response = match line {
                BoundedLine::Oversized => rejected(None, "request_too_large"),
                BoundedLine::Line(line) => match serde_json::from_slice::<ControlRequest>(&line) {
                    Ok(request) => {
                        let (result, stop) = match request.command {
                            ControlCommand::BackendSuccess => {
                                backend.set_next(NextBehavior::Success);
                                (ControlResult::Ack { accepted: true }, false)
                            }
                            ControlCommand::BlockNextCall => {
                                backend.set_next(NextBehavior::Block);
                                (ControlResult::Ack { accepted: true }, false)
                            }
                            ControlCommand::ReleaseBlockedCall => (
                                ControlResult::Ack {
                                    accepted: backend.release_blocked().await,
                                },
                                false,
                            ),
                            ControlCommand::TypedFailure => {
                                backend.set_next(NextBehavior::Failure);
                                (ControlResult::Ack { accepted: true }, false)
                            }
                            ControlCommand::Counters => {
                                (ControlResult::Counters(backend.counters.snapshot()), false)
                            }
                            ControlCommand::GracefulShutdown => {
                                (ControlResult::Ack { accepted: true }, true)
                            }
                        };
                        let response = ControlResponse {
                            id: Some(request.id),
                            ok: true,
                            result: Some(result),
                            error: None,
                        };
                        write_response(&mut stream, &response).await?;
                        if stop {
                            shutdown.cancel();
                            return Ok(());
                        }
                        continue;
                    }
                    Err(error) => {
                        let code = if error.to_string().contains("unknown variant") {
                            "unknown_command"
                        } else {
                            "malformed_request"
                        };
                        let id = serde_json::from_slice::<ControlId>(&line)
                            .ok()
                            .map(|probe| {
                                let _ = probe.command;
                                probe.id
                            });
                        rejected(id, code)
                    }
                },
            };
            write_response(&mut stream, &response).await?;
        }
    }

    async fn run_control_server(
        listener: Arc<UnixListener>,
        backend: Arc<ControlledBackend>,
        shutdown: CancellationToken,
        accepting: tokio::sync::oneshot::Sender<()>,
    ) -> io::Result<()> {
        let _ = accepting.send(());
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let backend = Arc::clone(&backend);
                    let shutdown = shutdown.clone();
                    connections.spawn(async move {
                        let _ = handle_control_connection(stream, backend, shutdown).await;
                    });
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        while connections.join_next().await.is_some() {}
        Ok(())
    }

    fn prepare_state_root(path: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
        if path.as_os_str().is_empty() {
            return Err("state root must not be empty".into());
        }
        if path.exists() {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("state root must be a real directory".into());
            }
        } else {
            fs::create_dir_all(path)?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        if fs::symlink_metadata(path)?.permissions().mode() & 0o777 != 0o700 {
            return Err("state root is not owner-only".into());
        }
        Ok(())
    }

    /// Identity of the socket inode this fixture bound, so teardown unlinks only its own socket.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct SocketIdentity {
        dev: u64,
        ino: u64,
    }

    fn socket_identity(path: &Path) -> io::Result<SocketIdentity> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(SocketIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    /// Removes `path` only while it still names the socket this fixture bound; a successor that
    /// replaced a stale socket during a handover keeps its own.
    fn unlink_own_control_socket(path: &Path, own: SocketIdentity) -> io::Result<()> {
        match socket_identity(path) {
            Ok(current) if current == own => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn bind_control_socket(path: &Path) -> Result<UnixListener, Box<dyn Error + Send + Sync>> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if !metadata.file_type().is_socket() {
                return Err("control path exists and is not a socket".into());
            }
            // An accepting control socket must not be unlinked: its listener would stay live but unreachable.
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    return Err(
                        "another direct-host fixture is live at this state root; its control socket still accepts connections".into(),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(path)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        if fs::symlink_metadata(path)?.permissions().mode() & 0o777 != 0o600 {
            return Err("control socket is not owner-only".into());
        }
        Ok(listener)
    }

    fn storage_init(root: &Path) -> Result<HostInit, Box<dyn Error + Send + Sync>> {
        let descriptor = daemon::managed_store_descriptor(root)?;
        Ok(HostInit {
            host_capabilities: Vec::new(),
            storage: Some(serde_json::to_value(descriptor).expect("storage descriptor serializes")),
        })
    }

    /// Readiness is this process's own incarnation publishing: the lifecycle record at `root` must classify `Running` and carry this process's PID, so a live managed host's publication or a crashed host's stale file at the same path does not count.
    fn own_incarnation_is_running(root: &Path) -> bool {
        host_runtime::probe_lifecycle(Some(root), &host_runtime::ProbeFreshness::default())
            .ok()
            .is_some_and(|observed| {
                observed.state == host_runtime::LifecycleState::Running
                    && observed
                        .record
                        .as_ref()
                        .is_some_and(|record| record.pid == std::process::id())
            })
    }

    async fn wait_for_publication(
        root: &Path,
        publication: &Path,
        host: &mut tokio::task::JoinHandle<Result<(), host_runtime::HostError>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if own_incarnation_is_running(root)
                && let Ok(info) = host_runtime::read_connection_file(publication)
            {
                if info.wire_version != 2 {
                    return Err("fixture published an unsupported wire version".into());
                }
                return Ok(());
            }
            if host.is_finished() {
                // Only `run` may await `host`; polling its completed `JoinHandle` again panics and skips control-socket cleanup.
                return Err("host exited before readiness".into());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("host did not publish before readiness deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn state_root_arg() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        let mut args = std::env::args_os().skip(1);
        match (args.next(), args.next(), args.next()) {
            (Some(flag), Some(path), None) if flag == "--state-root" => Ok(path.into()),
            _ => Err("usage: direct_host_fixture --state-root <path>".into()),
        }
    }

    pub async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
        let root = state_root_arg()?;
        prepare_state_root(&root)?;
        let control_path = root.join(CONTROL_FILE);
        let listener = Arc::new(bind_control_socket(&control_path)?);
        let own_socket = socket_identity(&control_path)?;

        let shutdown = CancellationToken::new();
        let backend = ControlledBackend::new(shutdown.clone());
        let (accepting_tx, accepting_rx) = tokio::sync::oneshot::channel();
        let control_shutdown = shutdown.clone();
        let control_backend = Arc::clone(&backend);
        let control_listener = Arc::clone(&listener);
        let control_task = tokio::spawn(async move {
            run_control_server(
                control_listener,
                control_backend,
                control_shutdown,
                accepting_tx,
            )
            .await
        });
        accepting_rx
            .await
            .map_err(|_| "control server failed to start")?;

        let publication =
            host_runtime::runtime_dir_path(Some(&root))?.join(host_runtime::CONNECTION_FILE_NAME);
        let synapse = synapse_component();
        let composite = StaticComposite::new(
            daemon::Handler::new_with_connection_file(Some(publication.clone())),
            synapse,
            BrocaComponent::new(
                backend,
                host_runtime::broca::subprocess::group_registry::StateRoot::resolve(Some(&root))?,
            ),
        )?;
        let config = HostConfig {
            data_dir: Some(root.clone()),
            daemon_ver: "eidnara-host/direct-host-fixture".to_owned(),
            init: storage_init(&root)?,
            limits: host_runtime::HostLimits {
                // The runtime deducts every linked component's declared retention from the resident budget, so the budget grows by the composite's own declarations.
                max_resident_bytes: host_runtime::HostLimits::default().max_resident_bytes
                    + host_runtime::HostHandler::resource_declarations(&composite)
                        .iter()
                        .map(|declaration| declaration.retained_resident_bytes)
                        .sum::<u64>(),
                ..host_runtime::HostLimits::default()
            },
            ..Default::default()
        };
        let host_shutdown = shutdown.clone();
        let mut host =
            tokio::spawn(async move { host_runtime::run(composite, config, host_shutdown).await });

        let signal_shutdown = shutdown.clone();
        let signal_task = tokio::spawn(async move {
            let mut signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler installs");
            if signal.recv().await.is_some() {
                signal_shutdown.cancel();
            }
        });

        let ready = wait_for_publication(&root, &publication, &mut host).await;
        if ready.is_ok() {
            let record = serde_json::json!({
                "status": "ready",
                "wire_version": 2,
                "catalog": CATALOG,
            });
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &record)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        } else {
            shutdown.cancel();
        }

        let host_result = host.await?;
        shutdown.cancel();
        signal_task.abort();
        let _ = signal_task.await;
        // The socket is unlinked while this listener is still bound, so a successor that connects in this window is accepted into the backlog and refuses to start rather than replacing the socket between the inode check and the unlink.
        let unlinked = unlink_own_control_socket(&control_path, own_socket);
        control_task.await??;
        drop(listener);
        unlinked?;
        host_result?;
        ready?;
        Ok(())
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    unix::run().await
}

#[cfg(not(unix))]
fn main() {
    compile_error!("direct_host_fixture is Unix-only");
}
