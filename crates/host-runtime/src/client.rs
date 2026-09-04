//! This module manages one authenticated host-runtime generation.
//!
//! This module owns discovery, authentication, mandatory ring setup,
//! correlation allocation, framing, liveness, route epochs, bounded queues,
//! cancellation, and cleanup. Raw frame types never cross the public API.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fmt,
    io::Write as _,
    os::fd::OwnedFd,
    os::unix::net::UnixStream as StdUnixStream,
    path::Path,
    sync::{
        Arc, LazyLock, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant as StdInstant},
};

use serde_json::Value;
use tokio::{
    net::UnixStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, timeout_at},
};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::{
    auth::authenticate_client,
    connection_file::{ConnectionInfo, DAEMON_ID_LEN, read_for_client},
    control::{OP_HOST_SHUTDOWN, OP_HOST_STATUS, OP_ROUTE_OPEN},
    handler::{HealthStatus, RouteHandle, RouteIdentity, RouteTarget, TargetKind},
    ring_transport::SendFailure,
    wire::{
        AdmissionClass, EnvelopeHeader, Flags, FrameId, FrameType, HEADER_LEN, MAX_BODY_LEN,
        MAX_CONTROL_BODY_LEN, PROTOCOL_VERSION, Priority, frame_header, pure_header_flags,
    },
};

/// Total deadline for discovery, authentication, and mandatory ring setup.
pub const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// The client starts `CLIENT_FRAME_TIMEOUT` at the first header byte and leaves idle header waits unbounded.
pub const CLIENT_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
/// The client applies `CLIENT_ROUTE_OPEN_TIMEOUT` to a route-open operation and its retries.
pub const CLIENT_ROUTE_OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// The client uses `CLIENT_REQUEST_TIMEOUT` as the default deadline for a request.
pub const CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The owner must shut down within `CLIENT_SHUTDOWN_TIMEOUT`.
pub const CLIENT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
pub const CLIENT_MAX_PENDING_REQUESTS: usize = 1_024;
pub const CLIENT_MAX_LIVE_STREAMS: usize = 64;
/// Each stream queues at most `CLIENT_STREAM_QUEUE_ITEMS` items; saturation cancels only that stream.
pub const CLIENT_STREAM_QUEUE_ITEMS: usize = 16;
/// `CLIENT_DATA_QUEUE_FRAMES` limits ordinary writer slots; reserved controls consume none.
pub const CLIENT_DATA_QUEUE_FRAMES: usize = 256;
/// `CLIENT_CONTROL_QUEUE_FRAMES` reserves slots for pure-header Pong, Cancel, and Goodbye frames.
pub const CLIENT_CONTROL_QUEUE_FRAMES: usize = 32;
/// `CLIENT_QUEUED_BYTES` is the one queued-byte ceiling shared by data and reserved-control frames (§11).
///
/// `CLIENT_CONTROL_QUEUED_BYTES` of it is partitioned for reserved control frames so ordinary traffic cannot starve them; data frames draw from the remainder, `CLIENT_DATA_QUEUED_BYTES`.
/// A failed control-byte charge retires the generation.
/// A failed data-byte charge returns a local error to that caller.
pub const CLIENT_QUEUED_BYTES: usize = MAX_BODY_LEN as usize + 1_048_576;
///
/// `CLIENT_CONTROL_QUEUED_BYTES` covers exactly `CLIENT_CONTROL_QUEUE_FRAMES` header-only control frames.
/// A control-byte charge can fail only when the control channel is full; that condition retires the generation.
pub const CLIENT_CONTROL_QUEUED_BYTES: usize = CLIENT_CONTROL_QUEUE_FRAMES * HEADER_LEN;
/// The data partition of `CLIENT_QUEUED_BYTES`; it still admits one maximum-sized request frame.
pub const CLIENT_DATA_QUEUED_BYTES: usize = CLIENT_QUEUED_BYTES - CLIENT_CONTROL_QUEUED_BYTES;
const _: () = assert!(CLIENT_DATA_QUEUED_BYTES >= MAX_BODY_LEN as usize + HEADER_LEN);
/// `CLIENT_INBOUND_FRAME_BYTES` reserves space for the body the reader is decoding.
///
/// An admitted connection must accept every otherwise-valid frame, so this reservation covers the framing maximum separately from `CLIENT_RETAINED_RESPONSE_BYTES`.
/// `CLIENT_INBOUND_FRAME_BYTES` is a per-connection ceiling because one reader decodes one frame at a time.
pub const CLIENT_INBOUND_FRAME_BYTES: usize = MAX_BODY_LEN as usize;
/// `CLIENT_RETAINED_RESPONSE_BYTES` caps bytes retained in pending stream queues, and separately caps unary responses their callers have not yet polled.
///
/// Queueing charges each item before a consumer reads it.
/// The two pools are owner-wide, not per-request: exhausting one fails whichever frame charges it next, which need not be the frame holding the bytes.
/// A failed stream item cancels only that stream; a failed unary response fails only that request.
/// Each pool admits one maximum-sized item plus 1_048_576 bytes.
/// One connection can retain at most `CLIENT_RETAINED_RESPONSE_BYTES + CLIENT_INBOUND_FRAME_BYTES` bytes.
pub const CLIENT_RETAINED_RESPONSE_BYTES: usize = MAX_BODY_LEN as usize + 1_048_576;

/// `CLIENT_DISCOVERY_SLOTS` caps concurrent connection-file snapshots process-wide.
///
const CLIENT_DISCOVERY_SLOTS: usize = 64;

/// The blocking closure holds each `CLIENT_DISCOVERY_SLOTS` permit.
/// A detached worker still holds its `CLIENT_DISCOVERY_SLOTS` permit.
static DISCOVERY_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(CLIENT_DISCOVERY_SLOTS)));

const FIRST_APPLICATION_CORRELATION: u64 = 1;
const MAX_ERROR_CODE_BYTES: usize = 128;
const MAX_ERROR_MESSAGE_BYTES: usize = 512;
/// Exact send-outcome classifications used by recovery policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Request bytes provably never reached the writer.
    NotSent,
    /// Some request bytes may have reached the peer without a terminal.
    OutcomeUnknown,
    /// The reader observed a matching host terminal.
    Terminal,
}

impl SendOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotSent => "not_sent",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Terminal => "terminal",
        }
    }
}

impl fmt::Display for SendOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `CallError` formatting excludes raw host terminal messages.
#[derive(Clone, PartialEq, Eq)]
pub struct CallError {
    outcome: SendOutcome,
    code: String,
    message: String,
}

impl CallError {
    fn new(outcome: SendOutcome, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            outcome,
            code: bounded_code(&code.into()),
            message: bounded_text(&message.into(), MAX_ERROR_MESSAGE_BYTES),
        }
    }

    fn local(outcome: SendOutcome, code: &'static str, message: &'static str) -> Self {
        Self::new(outcome, code, message)
    }

    /// `None` means the body is not a canonical `ErrorBody` (§6.2): a JSON object
    /// with string `code` and `message`. Unknown members are permitted.
    fn host_terminal(body: &[u8]) -> Option<Self> {
        let value = serde_json::from_slice::<Value>(body).ok()?;
        let code = value.get("code")?.as_str()?;
        value.get("message")?.as_str()?;
        // Raw terminal messages may contain request, credential, or identity data.
        // `CallError` retains the bounded terminal code and discards the raw terminal message.
        // `host.` identifies host-supplied codes.
        Some(Self::new(
            SendOutcome::Terminal,
            format!("host.{}", bounded_code(code)),
            "host returned a terminal error (message redacted)",
        ))
    }

    /// Send classification.
    pub const fn outcome(&self) -> SendOutcome {
        self.outcome
    }

    /// `CallError::new` bounds the error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The error message is bounded.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallError")
            .field("outcome", &self.outcome)
            .field("code", &self.code)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.outcome, self.message, self.code)
    }
}

impl Error for CallError {}

/// Discovery, authentication, ring setup, or owner-lifecycle failure.
#[derive(Clone, PartialEq, Eq)]
pub struct ClientError {
    code: &'static str,
    message: &'static str,
}

impl ClientError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    /// The failure code is stable.
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Debug for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientError")
            .field("code", &self.code)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl Error for ClientError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Response bytes. The client does not interpret application payloads.
    pub body: Vec<u8>,
    /// The host sets `binary` when it marks the body as binary.
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostStatusSnapshot {
    pub health: HealthStatus,
    pub metrics: serde_json::Value,
    pub shared_memory: serde_json::Value,
}

/// The host emits each item in stream order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamItem {
    /// Item bytes.
    pub body: Vec<u8>,
    /// The host sets the item's binary flag.
    pub binary: bool,
}

#[derive(Debug, Clone)]
pub struct RequestOptions {
    /// Total operation budget. Queueing, publication, and terminal wait share it.
    pub timeout: Duration,
    pub cancellation: Option<CancellationToken>,
    /// Sets the frame's binary flag; the host exposes it as `RequestCtx::binary`.
    /// Routed bodies are opaque to transport, so either encoding is legal (§7.1).
    pub binary: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout: CLIENT_REQUEST_TIMEOUT,
            cancellation: None,
            binary: false,
        }
    }
}

/// The client manages one authenticated daemon generation through this connection.
pub struct Client {
    inner: Arc<Inner>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("closed", &self.inner.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Securely discovers, authenticates, and attaches one ring generation.
    ///
    /// Discovery validates one descriptor-anchored snapshot before any dial.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        // The deadline starts before discovery, not after it. §11.2 spends one
        // 2-second budget on discovery, setup-socket authentication, and ring attachment
        // together, so starting the clock after the snapshot would give a
        // stalled filesystem unbounded time and then hand the handshake a fresh
        // budget. The snapshot also runs on a blocking pool: it is synchronous
        // filesystem work, and on a wedged mount it would otherwise occupy an
        // async worker for as long as the mount takes.
        let deadline = Instant::now() + CLIENT_HANDSHAKE_TIMEOUT;
        let path = path.as_ref().to_path_buf();
        // `DISCOVERY_SLOTS` limits concurrent discovery snapshots.
        // `spawn_blocking` cannot cancel submitted work.
        // Dropping the join handle detaches the closure.
        // A filesystem syscall on a wedged mount retains its blocking worker until the call returns.
        // Each reconnect attempt can strand another blocking worker until its filesystem call returns.
        // Each timed-out attempt can strand a blocking worker until the filesystem call returns.
        // The permit limits blocking workers occupied by abandoned mounts.
        // A detached worker retains the permit, so the cap counts active workers rather than waiting callers.
        // Waiting for a permit spends the handshake deadline budget.
        // Permit exhaustion surfaces as `handshake_timeout`.
        let permit = timeout_at(deadline, Arc::clone(&DISCOVERY_SLOTS).acquire_owned())
            .await
            .map_err(|_| ClientError::new("handshake_timeout", "client handshake timed out"))?
            .expect("discovery semaphore is never closed");
        let info = timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                read_for_client(path)
            }),
        )
        .await
        .map_err(|_| ClientError::new("handshake_timeout", "client handshake timed out"))?
        .map_err(|_| ClientError::new("discovery_failed", "secure discovery failed"))?
        .map_err(|error| {
            use crate::connection_file::ConnectionFileError as E;
            let code = match error {
                E::Replaced { .. } => "discovery_replaced",
                E::Insecure { .. } => "discovery_insecure",
                E::UnsupportedSchema { .. } | E::WireVersionMismatch { .. } => {
                    "discovery_unsupported"
                }
                _ => "discovery_failed",
            };
            ClientError::new(code, "secure discovery failed")
        })?;
        Self::connect_info(info, deadline).await
    }

    async fn connect_info(info: ConnectionInfo, deadline: Instant) -> Result<Self, ClientError> {
        let mut stream = timeout_at(deadline, UnixStream::connect(&info.setup_socket))
            .await
            .map_err(|_| ClientError::new("handshake_timeout", "client handshake timed out"))?
            .map_err(|_| ClientError::new("dial_failed", "daemon dial failed"))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ClientError::new(
                "handshake_timeout",
                "client handshake timed out",
            ));
        }
        let authenticated =
            timeout_at(deadline, authenticate_client(&mut stream, &info, remaining))
                .await
                .map_err(|_| ClientError::new("handshake_timeout", "client handshake timed out"))?
                .map_err(|_| {
                    ClientError::new("authentication_failed", "daemon authentication failed")
                })?;
        let setup_failed = || ClientError::new("setup_failed", "shared-memory setup failed");
        let (descriptor, descriptors) = crate::setup_socket::activate_client(&mut stream, deadline)
            .await
            .map_err(|_| setup_failed())?;
        let cancel = CancellationToken::new();
        let read_budget = Arc::new(ByteCounter::new(CLIENT_INBOUND_FRAME_BYTES));
        // §11.2: the client attaches both directions, then commits activation.
        // The bridge thread owns the attached rings, so commit waits for its readiness report and only then hands it the setup socket.
        let RingBridge {
            write: ring_tx,
            read: ring_rx,
            setup: setup_tx,
            thread: bridge,
        } = start_ring_bridge(
            descriptor,
            descriptors,
            cancel.clone(),
            Arc::clone(&read_budget),
            deadline,
        )
        .await?;
        crate::setup_socket::commit_activation(&mut stream, deadline)
            .await
            .map_err(|_| setup_failed())?;
        let setup_stream = stream.into_std().map_err(|_| setup_failed())?;
        setup_stream
            .set_nonblocking(false)
            .map_err(|_| setup_failed())?;
        setup_tx.send(setup_stream).map_err(|_| setup_failed())?;
        let (data_tx, data_rx) = mpsc::channel(CLIENT_DATA_QUEUE_FRAMES);
        let (control_tx, control_rx) = mpsc::channel(CLIENT_CONTROL_QUEUE_FRAMES);
        let bridge_wake = Arc::downgrade(&ring_tx.wake);
        let inner = Arc::new(Inner {
            daemon_id: info.daemon_id,
            daemon_ver: authenticated.daemon_ver,
            closed: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            cancel,
            correlations: Mutex::new(Correlations::new(FIRST_APPLICATION_CORRELATION)),
            admission: Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            streams: Mutex::new(0),
            routes: Mutex::new(HashSet::new()),
            queue_budget: Arc::new(ByteCounter::new(CLIENT_DATA_QUEUED_BYTES)),
            control_budget: Arc::new(ByteCounter::new(CLIENT_CONTROL_QUEUED_BYTES)),
            _read_budget: read_budget,
            retained_budget: Arc::new(ByteCounter::new(CLIENT_RETAINED_RESPONSE_BYTES)),
            unary_budget: Arc::new(ByteCounter::new(CLIENT_RETAINED_RESPONSE_BYTES)),
            data_tx,
            control_tx,
            close_lock: tokio::sync::Mutex::new(()),
            reader: tokio::sync::Mutex::new(None),
            writer: tokio::sync::Mutex::new(None),
            bridge: Mutex::new(Some(bridge)),
            bridge_wake,
        });
        let writer_inner = Arc::clone(&inner);
        let writer = tokio::spawn(async move {
            writer_loop(writer_inner, ring_tx, data_rx, control_rx).await;
        });
        let reader_inner = Arc::clone(&inner);
        let reader = tokio::spawn(async move {
            ring_reader_loop(reader_inner, ring_rx).await;
        });
        *inner.writer.lock().await = Some(writer);
        *inner.reader.lock().await = Some(reader);
        // The reader runs on another worker and can retire this generation
        // before the constructor returns — a peer that closes or sends
        // connection `Goodbye` right after setup does exactly that.
        // Returning a "ready" client then defers the failure to the first
        // operation, which reports `connection_retired` as `NotSent`; the
        // historian does not reconnect on that path, so a daemon reload race
        // would abort the run instead of establishing a replacement.
        if inner.retired.load(Ordering::Acquire) {
            return Err(ClientError::new(
                "connection_retired",
                "connection retired during setup",
            ));
        }
        Ok(Self { inner })
    }

    /// Authentication verifies the daemon ID against secure discovery and the proof transcript.
    pub fn daemon_id(&self) -> [u8; DAEMON_ID_LEN] {
        self.inner.daemon_id
    }

    /// Returns the daemon version obtained during authentication.
    pub fn daemon_ver(&self) -> &str {
        &self.inner.daemon_ver
    }

    /// Opens a full `(channel, epoch)` route under one absolute 30-second deadline.
    pub async fn open_route(
        &self,
        target: RouteTarget,
        identity: RouteIdentity,
    ) -> Result<RouteHandle, CallError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "client_closed",
                "client is closed",
            ));
        }
        let body = route_open_body(&target, &identity)?;
        let deadline = Instant::now() + CLIENT_ROUTE_OPEN_TIMEOUT;
        let mut backoff = Duration::from_millis(25);
        loop {
            let response = self
                .inner
                .unary(
                    RouteHandle {
                        channel: 0,
                        epoch: 0,
                    },
                    body.clone(),
                    false,
                    deadline,
                    None,
                )
                .await;
            match response {
                Ok(response) => {
                    // `parse_route_open` must return a usable tag, channel, and epoch.
                    // Without a usable tag, channel, and epoch, the client cannot name the host-bound route.
                    // The client cannot send route `Goodbye` for a route it cannot name.
                    // Leaving the connection live after an unnameable route lets repeated opens strand host-side routes and channel permits.
                    // Retiring the connection obliges the host to settle every route on the generation.
                    // The host must settle the route for which `parse_route_open` produced no handle.
                    let handle = match parse_route_open(&response.body) {
                        Ok(handle) => handle,
                        Err(error) => {
                            self.inner.retire("invalid_route_response");
                            return Err(error);
                        }
                    };
                    // Holding `routes` while inserting and checking `closed` prevents `close` from missing a newly opened handle.
                    // A close between response receipt and handle insertion can leave the handle outside the drained set.
                    // Returning that handle would produce `Ok` even though its first use fails with `client_closed`.
                    // `close` takes precedence over a concurrent successful route open.
                    // `close` sends connection `Goodbye`, so this race needs no route `Goodbye`.
                    // A successful `parse_route_open` proves the request bytes reached the host.
                    // Returning `NotSent` would falsely mark the route open replay-safe.
                    {
                        let mut routes = lock_unpoisoned(&self.inner.routes);
                        if self.inner.closed.load(Ordering::Acquire) {
                            return Err(CallError::local(
                                SendOutcome::OutcomeUnknown,
                                "client_closed",
                                "client is closed",
                            ));
                        }
                        routes.insert(handle);
                    }
                    return Ok(handle);
                }
                Err(error)
                    if error.outcome == SendOutcome::Terminal
                        && matches!(
                            error.code.as_str(),
                            "host.unknown_module"
                                | "host.module_reloading"
                                | "host.target_unavailable"
                                | "host.module_timeout"
                        )
                        && Instant::now() < deadline =>
                {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    tokio::time::sleep(backoff.min(remaining)).await;
                    backoff = (backoff * 2).min(Duration::from_millis(500));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The request body is never replayed.
    pub async fn request(
        &self,
        route: RouteHandle,
        body: Vec<u8>,
        options: RequestOptions,
    ) -> Result<Response, CallError> {
        self.require_route(route)?;
        let deadline = request_deadline(options.timeout)?;
        self.inner
            .unary(route, body, options.binary, deadline, options.cancellation)
            .await
    }

    pub async fn request_stream(
        &self,
        route: RouteHandle,
        body: Vec<u8>,
        options: RequestOptions,
    ) -> Result<ResponseStream, CallError> {
        self.require_route(route)?;
        self.inner.start_stream(route, body, options)
    }

    /// Idempotently closes one exact route generation.
    ///
    /// `settle_route` removes `route` before `send_control_wait`; later calls return `Ok(())` without sending `Goodbye`.
    /// Retiring the generation on a failed wait makes setup-socket teardown release the host-side route.
    pub async fn close_route(&self, route: RouteHandle) -> Result<(), ClientError> {
        if !self.inner.settle_route(route) {
            return Ok(());
        }
        let deadline = Instant::now() + CLIENT_SHUTDOWN_TIMEOUT;
        let result = self
            .inner
            .send_control_wait(FrameType::Goodbye, FrameId::routed(route, 0), deadline)
            .await;
        if result.is_err() {
            self.inner.retire("route_close_timeout");
        }
        result
    }

    /// `Ok` means the complete `host.shutdown` response frame reached the socket; the connection remains open.
    pub async fn host_shutdown(&self) -> Result<(), CallError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "client_closed",
                "client is closed",
            ));
        }
        let body = serde_json::to_vec(&serde_json::json!({"op": OP_HOST_SHUTDOWN}))
            .expect("static host.shutdown request serializes");
        let deadline = Instant::now() + CLIENT_SHUTDOWN_TIMEOUT;
        let response = self
            .inner
            .unary(
                RouteHandle {
                    channel: 0,
                    epoch: 0,
                },
                body,
                false,
                deadline,
                None,
            )
            .await?;
        let acknowledged = serde_json::from_slice::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|value| {
                value
                    .get("op")
                    .and_then(serde_json::Value::as_str)
                    .map(|op| op == OP_HOST_SHUTDOWN)
            })
            .unwrap_or(false);
        if !acknowledged {
            // A channel-0 response that is not the tagged JSON object §7.1 requires is framing corruption, not an application result.
            self.inner.retire("invalid_shutdown_response");
            return Err(CallError::local(
                SendOutcome::Terminal,
                "invalid_shutdown_response",
                "host.shutdown response did not echo the operation",
            ));
        }
        Ok(())
    }

    /// Reads the host-owned readiness snapshot without opening a route or sending an application body.
    pub async fn host_status(&self) -> Result<HostStatusSnapshot, CallError> {
        // Unknown members are tolerated so a host that adds a field does not turn
        // every status read into a terminal error on older clients.
        #[derive(serde::Deserialize)]
        struct WireStatus {
            op: String,
            health: String,
            metrics: serde_json::Value,
            shared_memory: serde_json::Value,
        }

        if self.inner.closed.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "client_closed",
                "client is closed",
            ));
        }
        let deadline = Instant::now() + CLIENT_REQUEST_TIMEOUT;
        let response = self
            .inner
            .unary(
                RouteHandle {
                    channel: 0,
                    epoch: 0,
                },
                serde_json::to_vec(&serde_json::json!({"op": OP_HOST_STATUS}))
                    .expect("static host.status request serializes"),
                false,
                deadline,
                None,
            )
            .await?;
        // A channel-0 response that is not the tagged JSON object §7.1 requires is framing corruption, not an application result.
        let decoded = serde_json::from_slice::<WireStatus>(&response.body).map_err(|_| {
            self.inner.retire("invalid_host_status_response");
            CallError::local(
                SendOutcome::Terminal,
                "invalid_host_status_response",
                "host.status response is malformed",
            )
        })?;
        let invalid_identity = || {
            self.inner.retire("invalid_host_status_response");
            CallError::local(
                SendOutcome::Terminal,
                "invalid_host_status_response",
                "host.status response has an invalid identity",
            )
        };
        if decoded.op != OP_HOST_STATUS {
            return Err(invalid_identity());
        }
        let health = HealthStatus::parse(&decoded.health).ok_or_else(invalid_identity)?;
        Ok(HostStatusSnapshot {
            health,
            metrics: decoded.metrics,
            shared_memory: decoded.shared_memory,
        })
    }

    pub async fn close(&self) -> Result<(), ClientError> {
        let deadline = Instant::now() + CLIENT_SHUTDOWN_TIMEOUT;
        let _close = timeout_at(deadline, self.inner.close_lock.lock())
            .await
            .map_err(|_| {
                self.inner.retire("shutdown_timeout");
                ClientError::new("shutdown_timeout", "client shutdown timed out")
            })?;
        let mut guard = CloseGuard::new(&self.inner);
        let already_closed = self.inner.mark_closed(|_| ());
        let mut result = Ok(());
        if !already_closed {
            self.inner.settle_all("owner_close");
            let routes: Vec<_> = lock_unpoisoned(&self.inner.routes).drain().collect();
            let goodbyes = routes
                .into_iter()
                .map(|route| FrameId::routed(route, 0))
                .chain(std::iter::once(FrameId::control(0)));
            for id in goodbyes {
                if self
                    .inner
                    .send_control_wait(FrameType::Goodbye, id, deadline)
                    .await
                    .is_err()
                {
                    if !self.inner.retired.load(Ordering::Acquire) {
                        result = Err(ClientError::new(
                            "shutdown_timeout",
                            "client shutdown timed out",
                        ));
                    }
                    break;
                }
            }
            self.inner.cancel.cancel();
        }
        if !self.inner.join_tasks_until(deadline).await {
            result = Err(ClientError::new(
                "shutdown_timeout",
                "client shutdown timed out",
            ));
        }
        guard.disarm();
        result
    }

    fn require_route(&self, route: RouteHandle) -> Result<(), CallError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "client_closed",
                "client is closed",
            ));
        }
        if !lock_unpoisoned(&self.inner.routes).contains(&route) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "route_not_live",
                "route is not live on this generation",
            ));
        }
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.inner.retire("owner_drop");
    }
}

struct CloseGuard<'a> {
    inner: &'a Inner,
    armed: bool,
}

impl<'a> CloseGuard<'a> {
    const fn new(inner: &'a Inner) -> Self {
        Self { inner, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CloseGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.inner.retire("owner_close_dropped");
        }
    }
}

/// `ResponseStream` consumes one bounded stream. Dropping `ResponseStream` emits a best-effort Cancel.
pub struct ResponseStream {
    inner: Weak<Inner>,
    key: PendingKey,
    correlation: u64,
    items: mpsc::Receiver<ChargedItem>,
    terminal: Option<oneshot::Receiver<Result<(), CallError>>>,
    finished: bool,
}

impl fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseStream")
            .field("correlation", &self.correlation)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl ResponseStream {
    pub const fn correlation(&self) -> u64 {
        self.correlation
    }

    /// Returns next ordered item, `None` after StreamEnd, or terminal error.
    pub async fn next(&mut self) -> Result<Option<StreamItem>, CallError> {
        if self.finished {
            return Ok(None);
        }
        if let Ok(item) = self.items.try_recv() {
            return Ok(Some(item.into_public()));
        }
        let Some(terminal) = self.terminal.as_mut() else {
            self.finished = true;
            return Ok(None);
        };
        enum Next {
            Item(ChargedItem),
            ItemsClosed,
            Terminal(Result<Result<(), CallError>, oneshot::error::RecvError>),
        }
        let next = tokio::select! {
            biased;
            item = self.items.recv() => match item {
                Some(item) => Next::Item(item),
                None => Next::ItemsClosed,
            },
            result = terminal => Next::Terminal(result),
        };
        match next {
            Next::Item(item) => Ok(Some(item.into_public())),
            Next::ItemsClosed => {
                let Some(terminal) = self.terminal.take() else {
                    self.finished = true;
                    return Err(retired_error(SendOutcome::OutcomeUnknown));
                };
                let result = terminal
                    .await
                    .unwrap_or_else(|_| Err(retired_error(SendOutcome::OutcomeUnknown)));
                self.finished = true;
                result.map(|()| None)
            }
            Next::Terminal(result) => {
                self.finished = true;
                self.terminal = None;
                result
                    .unwrap_or_else(|_| Err(retired_error(SendOutcome::OutcomeUnknown)))
                    .map(|()| None)
            }
        }
    }

    /// Cancels the stream once. Cleanup remains epoch- and correlation-scoped.
    pub fn cancel(&mut self) -> Result<(), CallError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        // Close the response channel before draining it so the reader task cannot refill it.
        self.items.close();
        while self.items.try_recv().is_ok() {}
        if let Some(inner) = self.inner.upgrade() {
            inner.cancel_key(self.key, "cancelled")?;
        }
        Ok(())
    }
}

impl Drop for ResponseStream {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PendingKey {
    channel: u16,
    epoch: u32,
    corr: u64,
}

impl PendingKey {
    fn new(route: RouteHandle, corr: u64) -> Self {
        Self {
            channel: route.channel,
            epoch: route.epoch,
            corr,
        }
    }

    fn route(self) -> RouteHandle {
        RouteHandle {
            channel: self.channel,
            epoch: self.epoch,
        }
    }
}

const QUEUED: u8 = 0;
const WRITING: u8 = 1;
const WRITTEN: u8 = 2;
const CANCELLED: u8 = 3;

/// Indicates whether `stop` removed the pending entry.
///
/// `Cancelled` means this stop settled the caller; `AlreadyTaken` means another owner may still send a terminal result.
/// flight.
#[derive(Debug)]
enum PendingRemoval {
    /// This stop removed the entry and settled the caller.
    Cancelled,
    /// The entry was gone, so another owner may still send a terminal result.
    AlreadyTaken,
}

struct PendingState {
    publish: Arc<AtomicU8>,
    kind: PendingKind,
}

/// A unary response held for its caller. `_charge` returns the body's bytes to
/// `retained_budget` when the caller consumes or abandons the response.
struct RetainedResponse {
    response: Response,
    _charge: ByteCharge,
}

impl fmt::Debug for RetainedResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.response.fmt(f)
    }
}

type UnaryTerminal = Result<RetainedResponse, CallError>;

enum PendingKind {
    Unary(oneshot::Sender<UnaryTerminal>),
    Stream {
        items: mpsc::Sender<ChargedItem>,
        terminal: oneshot::Sender<Result<(), CallError>>,
        /// Dropping the pending entry cancels its deadline watcher.
        /// The guard covers every settlement path because each path drops the pending entry.
        _settled: DropGuard,
    },
}

struct Inner {
    daemon_id: [u8; DAEMON_ID_LEN],
    daemon_ver: String,
    closed: AtomicBool,
    retired: AtomicBool,
    cancel: CancellationToken,
    correlations: Mutex<Correlations>,
    admission: Mutex<()>,
    pending: Mutex<HashMap<PendingKey, PendingState>>,
    streams: Mutex<usize>,
    routes: Mutex<HashSet<RouteHandle>>,
    queue_budget: Arc<ByteCounter>,
    /// The client reserves queue capacity for header-only control frames so data traffic cannot starve Pong, Cancel, or Goodbye.
    control_budget: Arc<ByteCounter>,
    /// Reserved for the body of the one frame the reader is decoding. Separate
    /// from `retained_budget` so queue retention can never deny an otherwise
    /// valid inbound frame; see `CLIENT_INBOUND_FRAME_BYTES`.
    _read_budget: Arc<ByteCounter>,
    retained_budget: Arc<ByteCounter>,
    /// Unpolled unary responses. Separate from `retained_budget` so a stream
    /// backlog cannot discard a successfully received unary response.
    unary_budget: Arc<ByteCounter>,
    data_tx: mpsc::Sender<QueuedFrame>,
    control_tx: mpsc::Sender<QueuedFrame>,
    close_lock: tokio::sync::Mutex<()>,
    reader: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    writer: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// The ring bridge thread, joined by `join_tasks_until` after the writer and reader.
    bridge: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The bridge's poll eventfd. `retire` signals it so a drop-only teardown
    /// does not depend on the writer task being polled again to wake the bridge.
    /// `Weak` so the fd still closes with the writer and bridge, not with `Inner`.
    bridge_wake: Weak<OwnedFd>,
}

impl Inner {
    async fn unary(
        self: &Arc<Self>,
        route: RouteHandle,
        body: Vec<u8>,
        binary: bool,
        deadline: Instant,
        cancellation: Option<CancellationToken>,
    ) -> Result<Response, CallError> {
        // A token cancelled before the call must not enqueue anything.
        // Admission must reject pre-cancelled tokens because `select!` runs only after admission.
        // Once writer admission succeeds, the writer may claim the frame despite the biased cancellation `select!`.
        if cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "cancelled",
                "request was cancelled",
            ));
        }
        let (tx, rx) = oneshot::channel();
        let (key, publish) = self.admit(route, body, binary, PendingKind::Unary(tx), deadline)?;
        let mut guard = UnaryAdmissionGuard::new(Arc::clone(self), key, rx);
        let cancelled = cancellation.unwrap_or_default();
        // The stop branches borrow `rx` after `select!` because `dispatch` removes the pending entry before sending a terminal; a stop in that window must await the authoritative result.
        enum Stopped {
            Terminal(UnaryTerminal),
            Cancelled,
            DeadlineExpired,
        }
        let stopped = tokio::select! {
            biased;
            result = &mut guard.rx => Stopped::Terminal(
                result.unwrap_or_else(|_| Err(retired_error(classify(&publish)))),
            ),
            () = cancelled.cancelled() => Stopped::Cancelled,
            () = tokio::time::sleep_until(deadline) => Stopped::DeadlineExpired,
        };
        let result = match stopped {
            Stopped::Terminal(result) => result,
            Stopped::Cancelled => {
                self.stop_or_take_terminal(
                    key,
                    &mut guard.rx,
                    &publish,
                    "cancelled",
                    "request was cancelled",
                )
                .await
            }
            Stopped::DeadlineExpired => {
                self.stop_or_take_terminal(
                    key,
                    &mut guard.rx,
                    &publish,
                    "deadline_expired",
                    "request deadline expired",
                )
                .await
            }
        };
        guard.disarm();
        // Consuming the response releases its retained charge with `_charge`.
        result.map(|retained| retained.response)
    }

    fn start_stream(
        self: &Arc<Self>,
        route: RouteHandle,
        body: Vec<u8>,
        options: RequestOptions,
    ) -> Result<ResponseStream, CallError> {
        let deadline = request_deadline(options.timeout)?;
        // The client rejects pre-cancelled tokens before admission because the writer can transmit a request before its cancellation watcher starts.
        if options
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "cancelled",
                "request was cancelled",
            ));
        }
        {
            let mut streams = lock_unpoisoned(&self.streams);
            if *streams >= CLIENT_MAX_LIVE_STREAMS {
                return Err(CallError::local(
                    SendOutcome::NotSent,
                    "stream_capacity",
                    "live stream capacity exhausted",
                ));
            }
            *streams += 1;
        }
        let (item_tx, item_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let settled = CancellationToken::new();
        let admitted = self.admit(
            route,
            body,
            options.binary,
            PendingKind::Stream {
                items: item_tx,
                terminal: terminal_tx,
                _settled: settled.clone().drop_guard(),
            },
            deadline,
        );
        let (key, _publish) = match admitted {
            Ok(value) => value,
            Err(error) => {
                *lock_unpoisoned(&self.streams) -= 1;
                return Err(error);
            }
        };
        // A default token keeps the cancellation branch available when no cancellation token is supplied.
        let cancel = options.cancellation.unwrap_or_default();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::select! {
                biased;
                // The client must not cancel a correlation the host may have reused.
                () = settled.cancelled() => {}
                () = cancel.cancelled() => {
                    if let Some(inner) = weak.upgrade() {
                        let _ = inner.cancel_key(key, "cancelled");
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    if let Some(inner) = weak.upgrade() {
                        let _ = inner.cancel_key(key, "deadline_expired");
                    }
                }
            }
        });
        Ok(ResponseStream {
            inner: Arc::downgrade(self),
            key,
            correlation: key.corr,
            items: item_rx,
            terminal: Some(terminal_rx),
            finished: false,
        })
    }

    fn admit(
        &self,
        route: RouteHandle,
        body: Vec<u8>,
        binary: bool,
        kind: PendingKind,
        deadline: Instant,
    ) -> Result<(PendingKey, Arc<AtomicU8>), CallError> {
        if self.closed.load(Ordering::Acquire) || self.retired.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "connection_retired",
                "connection generation is retired",
            ));
        }
        if Instant::now() >= deadline {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "deadline_expired",
                "request deadline expired before admission",
            ));
        }
        let _admission = lock_unpoisoned(&self.admission);
        let mut pending = lock_unpoisoned(&self.pending);
        if self.closed.load(Ordering::Acquire) || self.retired.load(Ordering::Acquire) {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "connection_retired",
                "connection generation is retired",
            ));
        }
        if route
            != (RouteHandle {
                channel: 0,
                epoch: 0,
            })
            && !lock_unpoisoned(&self.routes).contains(&route)
        {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "route_not_live",
                "route is not live on this generation",
            ));
        }
        if Instant::now() >= deadline {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "deadline_expired",
                "request deadline expired before admission",
            ));
        }
        if pending.len() >= CLIENT_MAX_PENDING_REQUESTS {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "pending_capacity",
                "pending request capacity exhausted",
            ));
        }
        let mut correlations = lock_unpoisoned(&self.correlations);
        let Some(corr) = correlations.allocate() else {
            // §8.3: after `u64::MAX` the sender MUST retire the generation and reconnect before another request.
            // `retire` re-enters `admission`, so the guards are released before the call.
            drop(correlations);
            drop(pending);
            drop(_admission);
            self.retire("correlations_exhausted");
            return Err(CallError::local(
                SendOutcome::NotSent,
                "correlations_exhausted",
                "correlation space exhausted after u64::MAX",
            ));
        };
        let key = PendingKey::new(route, corr);
        let publish = Arc::new(AtomicU8::new(QUEUED));
        let frame = match encode_data_frame(
            route,
            corr,
            body,
            binary,
            Arc::clone(&publish),
            &self.queue_budget,
        ) {
            Ok(frame) => frame,
            Err(error) => {
                correlations.restore(corr);
                return Err(error);
            }
        };
        // Reserve inside the locks so a full queue rolls back the insert; the send itself
        // runs the writer's waker and stays outside the locks.
        let Ok(permit) = self.data_tx.try_reserve() else {
            correlations.restore(corr);
            return Err(CallError::local(
                SendOutcome::NotSent,
                "writer_queue_full",
                "writer data queue is full",
            ));
        };
        pending.insert(
            key,
            PendingState {
                publish: Arc::clone(&publish),
                kind,
            },
        );
        drop(correlations);
        drop(pending);
        drop(_admission);
        permit.send(frame);
        Ok((key, publish))
    }

    /// `stop_or_take_terminal` stops a pending unary request and prefers a terminal that beat the stop.
    ///
    /// `dispatch` removes the pending entry before publishing the terminal, so cancellation or deadline in that window finds nothing to cancel.
    /// When `remove` finds no entry, the caller awaits the terminal instead of returning a local error.
    async fn stop_or_take_terminal(
        &self,
        key: PendingKey,
        rx: &mut oneshot::Receiver<UnaryTerminal>,
        publish: &AtomicU8,
        code: &'static str,
        message: &'static str,
    ) -> UnaryTerminal {
        let stopped = match self.cancel_key(key, code) {
            Ok(PendingRemoval::AlreadyTaken) => {
                return rx
                    .await
                    .unwrap_or_else(|_| Err(retired_error(classify(publish))));
            }
            // `cancel_key` has already settled the channel, so `try_recv` observes its own result.
            Ok(PendingRemoval::Cancelled) => {
                if let Ok(result) = rx.try_recv() {
                    return result;
                }
                None
            }
            Err(error) => Some(error.outcome),
        };
        let outcome = stopped.unwrap_or_else(|| classify(publish));
        Err(CallError::local(outcome, code, message))
    }

    fn cancel_key(&self, key: PendingKey, code: &'static str) -> Result<PendingRemoval, CallError> {
        let state = lock_unpoisoned(&self.pending).remove(&key);
        let Some(state) = state else {
            return Ok(PendingRemoval::AlreadyTaken);
        };
        let outcome = if state
            .publish
            .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            SendOutcome::NotSent
        } else {
            SendOutcome::OutcomeUnknown
        };
        self.finish_pending(state, CallError::local(outcome, code, "request stopped"));
        if outcome == SendOutcome::OutcomeUnknown {
            // Control requests use identity 0/0, but §6.2 permits `Cancel` only for a pending nonzero correlation on a current nonzero route.
            // `cancel_key` preserves `OutcomeUnknown` because the request may already have reached the host.
            // OutcomeUnknown prevents the caller from treating the request as replay-safe.
            if key.channel == 0 {
                return Ok(PendingRemoval::Cancelled);
            }
            // `Cancel` is best-effort cleanup.
            // A failed Cancel enqueue returns an error without changing the request's OutcomeUnknown.
            // A concurrently retired generation can make the Cancel outcome NotSent.
            // Replacing OutcomeUnknown with the Cancel's NotSent outcome would incorrectly mark a possibly delivered request as replay-safe.
            // is replay-safe.
            if let Err(error) = self.send_control(
                FrameType::Cancel,
                pure_header_flags(),
                FrameId {
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                None,
            ) {
                return Err(CallError::new(outcome, error.code, error.message));
            }
        }
        Ok(PendingRemoval::Cancelled)
    }

    /// `flags` is explicit because `Pong` must echo `Ping` flags exactly, while §6.1 permits any valid priority.
    fn send_control(
        &self,
        ty: FrameType,
        flags: Flags,
        id: FrameId,
        ack: Option<oneshot::Sender<()>>,
    ) -> Result<(), CallError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(retired_error(SendOutcome::NotSent));
        }
        let header = frame_header(ty, flags, id, 0).map_err(|_| {
            CallError::local(
                SendOutcome::NotSent,
                "encode_failed",
                "control encode failed",
            )
        })?;
        // `send_control` uses the reserved pool so ordinary requests cannot prevent control-frame admission.
        // Non-`Cancel` control-capacity exhaustion retires the generation.
        // Cancel is best-effort cleanup; its exhaustion does not retire the generation.
        let exhausted = || {
            if ty != FrameType::Cancel {
                self.retire("control_capacity_exhausted");
            }
            CallError::local(
                SendOutcome::Terminal,
                "control_capacity_exhausted",
                "reserved control admission exhausted",
            )
        };
        let charge = self
            .control_budget
            .charge(HEADER_LEN)
            .ok_or_else(exhausted)?;
        let frame = QueuedFrame {
            header,
            body: Vec::new(),
            charge,
            publish: None,
            ack,
            deadline: Instant::now() + CLIENT_FRAME_TIMEOUT,
        };
        if self.control_tx.try_send(frame).is_err() {
            return Err(exhausted());
        }
        Ok(())
    }

    async fn send_control_wait(
        &self,
        ty: FrameType,
        id: FrameId,
        deadline: Instant,
    ) -> Result<(), ClientError> {
        let (tx, rx) = oneshot::channel();
        self.send_control(ty, pure_header_flags(), id, Some(tx))
            .map_err(|_| {
                ClientError::new(
                    "control_capacity_exhausted",
                    "client control admission failed",
                )
            })?;
        timeout_at(deadline, rx)
            .await
            .map_err(|_| ClientError::new("shutdown_timeout", "client shutdown timed out"))?
            .map_err(|_| ClientError::new("connection_retired", "connection retired"))
    }

    fn dispatch(self: &Arc<Self>, header: EnvelopeHeader, body: Vec<u8>, charge: ByteCharge) {
        match header.ty {
            FrameType::Ping => {
                // `Pong` echoes `Ping` flags exactly.
                let _ = self.send_control(
                    FrameType::Pong,
                    header.flags,
                    FrameId::control(header.corr),
                    None,
                );
            }
            FrameType::Goodbye if header.channel == 0 => self.retire("connection_goodbye"),
            FrameType::Goodbye => {
                let route = RouteHandle {
                    channel: header.channel,
                    epoch: header.epoch,
                };
                self.settle_route(route);
            }
            FrameType::Push => {}
            FrameType::Response | FrameType::Error | FrameType::StreamEnd => {
                // A malformed `Error` body is structurally illegal (§6.2) and closes the generation whether or not a correlation matches.
                let host_error = if header.ty == FrameType::Error {
                    match CallError::host_terminal(&body) {
                        Some(error) => Some(error),
                        None => {
                            drop(charge);
                            let key = PendingKey {
                                channel: header.channel,
                                epoch: header.epoch,
                                corr: header.corr,
                            };
                            if let Some(state) = lock_unpoisoned(&self.pending).remove(&key) {
                                self.finish_pending(
                                    state,
                                    CallError::local(
                                        SendOutcome::OutcomeUnknown,
                                        "protocol_violation",
                                        "host sent a malformed error body",
                                    ),
                                );
                            }
                            self.retire("protocol_violation");
                            return;
                        }
                    }
                } else {
                    None
                };
                let key = PendingKey {
                    channel: header.channel,
                    epoch: header.epoch,
                    corr: header.corr,
                };
                let state = lock_unpoisoned(&self.pending).remove(&key);
                let Some(state) = state else {
                    // A `Response` on identity 0/0 can carry a route bound for an `open_route` caller that dropped or timed out.
                    // An abandoned `open_route` on identity 0/0 cannot withdraw its bind because §6.2 permits no `Cancel`.
                    if header.ty == FrameType::Response && header.channel == 0 {
                        self.release_stranded_route(&body);
                    }
                    return;
                };
                match state.kind {
                    PendingKind::Unary(tx) => {
                        let result = match header.ty {
                            FrameType::Response => {
                                // The response is retained until the caller polls it, so its bytes
                                // move from the read reservation to the retained budget; a caller that
                                // never polls cannot hold more than `CLIENT_RETAINED_RESPONSE_BYTES`.
                                match self.unary_budget.charge(body.len()) {
                                    Some(retained) => Ok(RetainedResponse {
                                        response: Response {
                                            body,
                                            binary: header.flags.is_binary(),
                                        },
                                        _charge: retained,
                                    }),
                                    None => {
                                        // The body is discarded, so a route the host bound for this caller can only be released here.
                                        if header.channel == 0 {
                                            self.release_stranded_route(&body);
                                        }
                                        Err(CallError::local(
                                            // The reader observed the host's terminal; only the local copy is lost, so a retry would duplicate a completed operation.
                                            SendOutcome::Terminal,
                                            "response_retention_exhausted",
                                            "retained response capacity exhausted",
                                        ))
                                    }
                                }
                            }
                            FrameType::Error => Err(host_error.expect("validated above")),
                            FrameType::StreamEnd => Err(CallError::local(
                                SendOutcome::Terminal,
                                "unexpected_stream",
                                "unary request received stream terminal",
                            )),
                            _ => unreachable!(),
                        };
                        drop(charge);
                        // A receiver dropped between `pending.remove(&key)` and `tx.send(result)` strands a successful bind exactly like an absent pending entry.
                        if let Err(Ok(retained)) = tx.send(result)
                            && header.channel == 0
                        {
                            self.release_stranded_route(&retained.response.body);
                        }
                    }
                    PendingKind::Stream { terminal, .. } => {
                        drop(charge);
                        // Direct settlement retires the deadline watcher without calling `finish_pending`.
                        // `PendingKind::Stream::_settled`.
                        let terminal_result = match header.ty {
                            FrameType::StreamEnd => Ok(()),
                            FrameType::Error => Err(host_error.expect("validated above")),
                            FrameType::Response => Err(CallError::local(
                                SendOutcome::Terminal,
                                "unexpected_response",
                                "stream received unary response terminal",
                            )),
                            _ => unreachable!(),
                        };
                        let _ = terminal.send(terminal_result);
                        self.release_stream();
                    }
                }
            }
            FrameType::StreamData => {
                let key = PendingKey {
                    channel: header.channel,
                    epoch: header.epoch,
                    corr: header.corr,
                };
                let mut pending = lock_unpoisoned(&self.pending);
                let Some(state) = pending.get_mut(&key) else {
                    return;
                };
                match &mut state.kind {
                    PendingKind::Unary(_) => {
                        let state = pending.remove(&key).expect("entry exists");
                        drop(pending);
                        self.finish_pending(
                            state,
                            CallError::local(
                                // The best-effort `Cancel` may leave the run executing or committing.
                                // `Terminal` would suppress OutcomeUnknown recovery.
                                // The frame handler does not drain stream frames for a unary correlation.
                                // A unary correlation has no legal stream frames.
                                // Stream frames are invalid for unary correlations.
                                SendOutcome::OutcomeUnknown,
                                "unexpected_stream",
                                "unary request received stream data",
                            ),
                        );
                        let _ = self.send_control(
                            FrameType::Cancel,
                            pure_header_flags(),
                            FrameId {
                                channel: key.channel,
                                epoch: key.epoch,
                                corr: key.corr,
                            },
                            None,
                        );
                    }
                    PendingKind::Stream { items, .. } => {
                        // The stream queue charges retained bytes against the queue budget so held items cannot exhaust the reader's frame reservation.
                        // The stream handler treats retention-budget exhaustion as stream saturation.
                        // The stream handler cancels the saturated stream without advancing its generation.
                        // the generation.
                        let retained = self.retained_budget.charge(body.len());
                        let item = retained.map(|retained| ChargedItem {
                            body,
                            binary: header.flags.is_binary(),
                            _charge: retained,
                        });
                        // The reader releases the read reservation when bytes are retained or discarded.
                        drop(charge);
                        // Reserve under the lock, send after releasing it: `send` runs the consumer's waker.
                        let items = items.clone();
                        let permit =
                            item.and_then(|item| items.try_reserve().ok().map(|p| (p, item)));
                        if let Some((permit, item)) = permit {
                            drop(pending);
                            permit.send(item);
                        } else {
                            let state = pending.remove(&key).expect("entry exists");
                            drop(pending);
                            self.finish_pending(
                                state,
                                CallError::local(
                                    // The handler uses `OutcomeUnknown` because local overflow occurs after sending the request.
                                    // The handler observed no terminal frame.
                                    // The best-effort `Cancel` may not reach the host.
                                    // The run may still be committing.
                                    // `Terminal` would falsely claim authoritative settlement.
                                    SendOutcome::OutcomeUnknown,
                                    "stream_saturated",
                                    "stream consumer queue saturated",
                                ),
                            );
                            let _ = self.send_control(
                                FrameType::Cancel,
                                pure_header_flags(),
                                FrameId {
                                    channel: key.channel,
                                    epoch: key.epoch,
                                    corr: key.corr,
                                },
                                None,
                            );
                        }
                    }
                }
            }
            _ => self.retire("protocol_violation"),
        }
    }

    /// `release_stranded_route` releases a late route bind that no caller can own.
    ///
    /// The handler sends a best-effort route `Goodbye` for a successful bind that no caller can cache.
    /// The handler closes the connection only when it cannot queue the route `Goodbye`.
    /// The handler does not send `Goodbye` when the body names no route.
    ///
    /// A cached bind belongs to the caller that received it.
    /// The handler does not treat a duplicate terminal for a cached bind as stranded.
    /// Sending `Goodbye` would close a route still in use.
    fn release_stranded_route(&self, body: &[u8]) {
        let Ok(route) = parse_route_open(body) else {
            return;
        };
        if lock_unpoisoned(&self.routes).contains(&route) {
            return;
        }
        if self
            .send_control(
                FrameType::Goodbye,
                pure_header_flags(),
                FrameId::routed(route, 0),
                None,
            )
            .is_err()
        {
            self.retire("stranded_route_cleanup_failed");
        }
    }

    fn finish_pending(&self, state: PendingState, error: CallError) {
        match state.kind {
            PendingKind::Unary(tx) => {
                let _ = tx.send(Err(error));
            }
            PendingKind::Stream { terminal, .. } => {
                let _ = terminal.send(Err(error));
                // Dropping `state` retires the deadline watcher.
                // see `PendingKind::Stream::_settled`.
                self.release_stream();
            }
        }
    }

    fn release_stream(&self) {
        let mut streams = lock_unpoisoned(&self.streams);
        *streams = streams.saturating_sub(1);
    }

    ///
    /// The route settlement sends no per-correlation `Cancel`.
    /// `settle_all` also sends no `Cancel` frames.
    /// reason.
    fn settle_route(&self, route: RouteHandle) -> bool {
        let pending = {
            let _admission = lock_unpoisoned(&self.admission);
            let mut pending = lock_unpoisoned(&self.pending);
            if !lock_unpoisoned(&self.routes).remove(&route) {
                return false;
            }
            let keys: Vec<_> = pending
                .keys()
                .copied()
                .filter(|key| key.route() == route)
                .collect();
            keys.into_iter()
                .filter_map(|key| pending.remove(&key).map(|state| (key, state)))
                .collect::<Vec<_>>()
        };
        for (_key, state) in pending {
            let outcome = cancel_classification(&state.publish);
            self.finish_pending(
                state,
                CallError::local(outcome, "route_gone", "request stopped"),
            );
        }
        true
    }

    fn settle_all(&self, code: &'static str) {
        let pending = {
            let _admission = lock_unpoisoned(&self.admission);
            std::mem::take(&mut *lock_unpoisoned(&self.pending))
        };
        for (_, state) in pending {
            let outcome = cancel_classification(&state.publish);
            self.finish_pending(
                state,
                CallError::local(outcome, code, "connection generation closed"),
            );
        }
    }

    /// Flips `closed` inside the admission and route critical sections and runs
    /// `transition` there. `admit` checks `closed` under `admission` before it
    /// enqueues, and `open_route` checks it under `routes` before it inserts, so
    /// no request is queued and no handle is published after this returns.
    /// Returns the previous `closed` value.
    fn mark_closed(&self, transition: impl FnOnce(&mut HashSet<RouteHandle>)) -> bool {
        let _admission = lock_unpoisoned(&self.admission);
        let mut routes = lock_unpoisoned(&self.routes);
        let already = self.closed.swap(true, Ordering::AcqRel);
        transition(&mut routes);
        already
    }

    fn retire(&self, code: &'static str) {
        // The retirement latch and the route cache clear share the closed transition.
        let mut first = false;
        self.mark_closed(|routes| {
            first = !self.retired.swap(true, Ordering::AcqRel);
            if first {
                routes.clear();
            }
        });
        if !first {
            return;
        }
        self.settle_all(code);
        self.cancel.cancel();
        if let Some(wake) = self.bridge_wake.upgrade() {
            signal_eventfd(&wake);
        }
    }

    async fn join_tasks_until(&self, deadline: Instant) -> bool {
        let mut within_deadline = true;
        // The shared deadline bounds total shutdown time across all tasks.
        // A `yield_now` loop re-queues itself every iteration.
        // A `yield_now` loop spins the worker for the whole shutdown budget.
        // The handle stays in its slot while awaited so a cancelled `close()`
        // leaves it for the next `close()` to join instead of detaching the task.
        for slot in [&self.writer, &self.reader] {
            let mut slot = slot.lock().await;
            let Some(task) = slot.as_mut() else {
                continue;
            };
            if tokio::time::timeout_at(deadline, &mut *task).await.is_err() {
                within_deadline = false;
                // `JoinHandle` must not be awaited again after it completes.
                task.abort();
                let _ = task.await;
            }
            *slot = None;
        }
        // The bridge exits on its own once cancelled: the writer's dropped
        // `RingWriteSender` wakes its poll, the reader's dropped receiver fails
        // its `blocking_send`, and a capacity wait re-checks cancellation every
        // `BRIDGE_RESERVE_SLICE`. Joining it here keeps the setup socket and
        // mappings from outliving a successful `close`.
        let bridge = lock_unpoisoned(&self.bridge).take();
        if let Some(bridge) = bridge
            && tokio::time::timeout_at(
                deadline,
                tokio::task::spawn_blocking(move || {
                    let _ = bridge.join();
                }),
            )
            .await
            .is_err()
        {
            within_deadline = false;
        }
        within_deadline
    }
}

/// Owns the terminal receiver so a caller dropped after delivery still releases what it never claimed.
struct UnaryAdmissionGuard {
    inner: Arc<Inner>,
    key: PendingKey,
    rx: oneshot::Receiver<UnaryTerminal>,
    armed: bool,
}

impl UnaryAdmissionGuard {
    const fn new(inner: Arc<Inner>, key: PendingKey, rx: oneshot::Receiver<UnaryTerminal>) -> Self {
        Self {
            inner,
            key,
            rx,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for UnaryAdmissionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // `AlreadyTaken` means `dispatch` delivered a terminal this caller never polled.
        // A channel-0 success sitting in `rx` is a route bind nobody owns, so it is released like an unmatched one.
        if matches!(
            self.inner.cancel_key(self.key, "caller_dropped"),
            Ok(PendingRemoval::AlreadyTaken)
        ) && self.key.channel == 0
        {
            // `close` makes a concurrent `tx.send` fail and hand the value back to `dispatch`, which releases it; a value already sent is still readable here.
            self.rx.close();
            if let Ok(Ok(retained)) = self.rx.try_recv() {
                self.inner.release_stranded_route(&retained.response.body);
            }
        }
    }
}

struct Correlations {
    next: Option<u64>,
}

impl Correlations {
    const fn new(first: u64) -> Self {
        Self { next: Some(first) }
    }

    fn allocate(&mut self) -> Option<u64> {
        let current = self.next?;
        self.next = current.checked_add(1);
        Some(current)
    }

    fn restore(&mut self, correlation: u64) {
        if self.next == correlation.checked_add(1)
            || (correlation == u64::MAX && self.next.is_none())
        {
            self.next = Some(correlation);
        }
    }
}

struct ByteCounter {
    cap: usize,
    used: Mutex<usize>,
    wake: Mutex<Option<Weak<OwnedFd>>>,
    /// Set while the bridge thread is parked waiting for this budget.
    /// A release signals the wake fd only then, avoiding a syscall when no
    /// thread waits.
    parked: AtomicBool,
}

impl ByteCounter {
    const fn new(cap: usize) -> Self {
        Self {
            cap,
            used: Mutex::new(0),
            wake: Mutex::new(None),
            parked: AtomicBool::new(false),
        }
    }

    fn charge(self: &Arc<Self>, bytes: usize) -> Option<ByteCharge> {
        let mut used = lock_unpoisoned(&self.used);
        let next = used.checked_add(bytes)?;
        if next > self.cap {
            return None;
        }
        *used = next;
        Some(ByteCharge {
            owner: Arc::downgrade(self),
            bytes,
        })
    }

    const fn capacity(&self) -> usize {
        self.cap
    }

    fn set_wake(&self, wake: &Arc<OwnedFd>) {
        *lock_unpoisoned(&self.wake) = Some(Arc::downgrade(wake));
    }

    #[cfg(test)]
    fn used(&self) -> usize {
        *lock_unpoisoned(&self.used)
    }
}

struct ByteCharge {
    owner: Weak<ByteCounter>,
    bytes: usize,
}

impl ByteCharge {
    /// A zero-byte charge for bodiless frames. Holding one keeps every
    /// inbound frame's accounting uniform: an absent charge never reaches
    /// `dispatch`, so "no charge" cannot be misread as an exhausted budget.
    #[cfg(test)]
    const fn none() -> Self {
        Self {
            owner: Weak::new(),
            bytes: 0,
        }
    }
}

impl Drop for ByteCharge {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            {
                let mut used = lock_unpoisoned(&owner.used);
                *used = used.saturating_sub(self.bytes);
            }
            // Read `parked` only after releasing `used`; the waiter sets it before its
            // final `charge` under the same lock, which rules out a lost wake. commentlint: allow(JUDGE)
            if owner.parked.load(Ordering::SeqCst)
                && let Some(wake) = lock_unpoisoned(&owner.wake)
                    .as_ref()
                    .and_then(Weak::upgrade)
            {
                signal_eventfd(&wake);
            }
        }
    }
}

struct ChargedItem {
    body: Vec<u8>,
    binary: bool,
    /// account for.
    _charge: ByteCharge,
}

impl ChargedItem {
    fn into_public(self) -> StreamItem {
        StreamItem {
            body: self.body,
            binary: self.binary,
        }
    }
}

struct QueuedFrame {
    header: EnvelopeHeader,
    body: Vec<u8>,
    charge: ByteCharge,
    publish: Option<Arc<AtomicU8>>,
    ack: Option<oneshot::Sender<()>>,
    /// The ring write uses a connection-scoped deadline, not the caller's
    /// request deadline. Expiry here retires the whole generation, so a
    /// request-scoped value would let one short request fail every other
    /// in-flight request; the request's own deadline watcher settles that
    /// single caller.
    deadline: Instant,
}

struct RingWrite {
    header: EnvelopeHeader,
    body: Vec<u8>,
    completed: oneshot::Sender<Result<(), SendFailure>>,
    deadline: StdInstant,
}

struct RingWriteSender {
    tx: std::sync::mpsc::SyncSender<RingWrite>,
    wake: Arc<OwnedFd>,
}

impl RingWriteSender {
    fn try_send(&self, write: RingWrite) -> Result<(), std::sync::mpsc::TrySendError<RingWrite>> {
        self.tx.try_send(write)?;
        signal_eventfd(&self.wake);
        Ok(())
    }
}

impl Drop for RingWriteSender {
    fn drop(&mut self) {
        signal_eventfd(&self.wake);
    }
}

type RingFrameReceiver = mpsc::Receiver<(EnvelopeHeader, Vec<u8>, ByteCharge)>;

/// The mapped ring cannot express host death: a host that exits without a
/// Goodbye leaves its rings looking merely idle, so the setup socket is the
/// only liveness signal. `MSG_PEEK` keeps the probe side-effect free.
fn setup_peer_closed(stream: &StdUnixStream) -> bool {
    use std::os::fd::AsFd;
    let mut probe = [0u8; 1];
    match rustix::net::recv(
        stream.as_fd(),
        &mut probe,
        rustix::net::RecvFlags::PEEK | rustix::net::RecvFlags::DONTWAIT,
    ) {
        Ok(_) => true,
        Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => false,
        Err(_) => true,
    }
}

fn signal_eventfd(fd: &OwnedFd) {
    let _ = rustix::io::write(fd, &1u64.to_ne_bytes());
}

fn drain_eventfd(fd: &OwnedFd) {
    let mut value = [0u8; size_of::<u64>()];
    let _ = rustix::io::read(fd, &mut value);
}

/// One attached ring bridge. The thread is parked until `setup` delivers the
/// committed setup socket, so attachment completes before activation commits.
struct RingBridge {
    write: RingWriteSender,
    read: RingFrameReceiver,
    setup: oneshot::Sender<StdUnixStream>,
    thread: std::thread::JoinHandle<()>,
}

/// Upper bound on one uninterruptible capacity wait inside the bridge. A full
/// outbound ring otherwise parks `reserve_until` until the frame deadline,
/// which no cancellation can reach.
const BRIDGE_RESERVE_SLICE: Duration = Duration::from_millis(50);

async fn start_ring_bridge(
    descriptor: serde_json::Value,
    descriptors: [OwnedFd; crate::setup_socket::RING_DESCRIPTOR_COUNT],
    cancel: CancellationToken,
    read_budget: Arc<ByteCounter>,
    ready_deadline: Instant,
) -> Result<RingBridge, ClientError> {
    let (write_tx, write_rx) = std::sync::mpsc::sync_channel::<RingWrite>(CLIENT_DATA_QUEUE_FRAMES);
    let wake_fd = Arc::new(
        rustix::event::eventfd(
            0,
            rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
        )
        .map_err(|_| ClientError::new("setup_failed", "shared-memory setup failed"))?,
    );
    let worker_wake = Arc::clone(&wake_fd);
    read_budget.set_wake(&wake_fd);
    let (read_tx, read_rx) = mpsc::channel(CLIENT_DATA_QUEUE_FRAMES);
    let (ready_tx, ready_rx) = oneshot::channel::<Result<(), ()>>();
    let (setup_tx, setup_rx) = oneshot::channel::<StdUnixStream>();
    let thread = std::thread::Builder::new()
        .name("host-ring-client".to_owned())
        .spawn(move || {
            let endpoint = crate::ring_transport::RingClientEndpoint::attach_with_descriptors(
                &descriptor,
                descriptors,
            );
            let Ok(endpoint) = endpoint else {
                let _ = ready_tx.send(Err(()));
                return;
            };
            let Ok(data_ready) = endpoint.from_host.duplicate_data_ready() else {
                let _ = ready_tx.send(Err(()));
                return;
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            // A dropped sender means activation never committed; the rings are released with the thread.
            let Ok(mut setup) = setup_rx.blocking_recv() else {
                return;
            };
            // A half-close or a post-setup write makes the setup socket readable
            // with no `HUP`; `setup_peer_closed` treats both as teardown, so the
            // idle poll watches `IN` alongside `HUP | ERR`.
            let setup_events = rustix::event::PollFlags::IN
                | rustix::event::PollFlags::HUP
                | rustix::event::PollFlags::ERR;
            while !cancel.is_cancelled() {
                let mut wrote = false;
                match write_rx.try_recv() {
                    Ok(write) => {
                        // `reserve_until` parks on the peer's capacity doorbell, which
                        // cancellation cannot ring, so the wait is taken in slices.
                        let result = loop {
                            let slice = StdInstant::now() + BRIDGE_RESERVE_SLICE;
                            match endpoint.send(
                                write.header,
                                &write.body,
                                write.deadline.min(slice),
                            ) {
                                Err(SendFailure::Deadline)
                                    if StdInstant::now() < write.deadline
                                        && !cancel.is_cancelled() => {}
                                result => break result,
                            }
                        };
                        let failed = result.is_err();
                        let _ = write.completed.send(result);
                        if failed {
                            break;
                        }
                        wrote = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
                // `endpoint.try_recv_with` advances the ring's consumed cursor,
                // so refusing a charge would discard a valid response. Waiting
                // is backpressure against `ring_reader_loop`, which releases
                // each queued charge as it drains; cancellation ends the wait.
                // Frames wider than `read_budget.capacity()` cannot be admitted
                // by any drain, so they refuse without waiting.
                let charge = |bytes: usize| {
                    if bytes > read_budget.capacity() {
                        return None;
                    }
                    if let Some(charge) = read_budget.charge(bytes) {
                        return Some(charge);
                    }
                    // `parked` must be visible before the next `charge` attempt so a
                    // release between that attempt and `poll` still signals the fd.
                    read_budget.parked.store(true, Ordering::SeqCst);
                    let admitted = loop {
                        if let Some(charge) = read_budget.charge(bytes) {
                            break Some(charge);
                        }
                        if cancel.is_cancelled() {
                            break None;
                        }
                        let mut fds = [
                            rustix::event::PollFd::new(&*worker_wake, rustix::event::PollFlags::IN),
                            rustix::event::PollFd::new(&setup, setup_events),
                        ];
                        let polled = loop {
                            match rustix::event::poll(&mut fds, None) {
                                Ok(_) => break true,
                                Err(rustix::io::Errno::INTR) if !cancel.is_cancelled() => {
                                    continue;
                                }
                                Err(_) => break false,
                            }
                        };
                        if !polled || fds[1].revents().intersects(setup_events) {
                            break None;
                        }
                        if fds[0].revents().contains(rustix::event::PollFlags::IN) {
                            drain_eventfd(&worker_wake);
                        }
                    };
                    read_budget.parked.store(false, Ordering::SeqCst);
                    admitted
                };
                match endpoint.try_recv_with(charge) {
                    Ok(Some(frame)) => {
                        if read_tx.blocking_send(frame).is_err() {
                            break;
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
                if wrote {
                    continue;
                }
                if setup_peer_closed(&setup) {
                    break;
                }
                match endpoint.from_host.arm_data_wait() {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(_) => break,
                }
                let mut fds = [
                    rustix::event::PollFd::new(&*worker_wake, rustix::event::PollFlags::IN),
                    rustix::event::PollFd::new(&data_ready, rustix::event::PollFlags::IN),
                    rustix::event::PollFd::new(&setup, setup_events),
                ];
                let poll_ready = loop {
                    match rustix::event::poll(&mut fds, None) {
                        Ok(_) => break true,
                        Err(rustix::io::Errno::INTR) if !cancel.is_cancelled() => continue,
                        Err(_) => break false,
                    }
                };
                if !poll_ready {
                    break;
                }
                if fds[0].revents().contains(rustix::event::PollFlags::IN) {
                    drain_eventfd(&worker_wake);
                }
                if fds[1].revents().contains(rustix::event::PollFlags::IN)
                    && endpoint.from_host.complete_data_wait().is_err()
                {
                    break;
                }
                if fds[2].revents().intersects(setup_events) {
                    break;
                }
            }
            if let Ok(goodbye) = crate::setup_socket::encoded_goodbye() {
                let _ = setup.write_all(&goodbye);
            }
            let _ = setup.shutdown(std::net::Shutdown::Both);
        })
        .map_err(|_| ClientError::new("setup_failed", "shared-memory setup failed"))?;
    // The attachment wait expires at the handshake deadline and yields while it waits.
    // A synchronous wait would hold this worker for the whole remaining budget on a stalled attach.
    // A thread that reports readiness after the timeout sees a dropped receiver and returns without joining.
    timeout_at(ready_deadline, ready_rx)
        .await
        .map_err(|_| ClientError::new("handshake_timeout", "client handshake timed out"))?
        .map_err(|_| ClientError::new("setup_failed", "shared-memory setup failed"))?
        .map_err(|_| ClientError::new("setup_failed", "shared-memory setup failed"))?;
    Ok(RingBridge {
        write: RingWriteSender {
            tx: write_tx,
            wake: wake_fd,
        },
        read: read_rx,
        setup: setup_tx,
        thread,
    })
}

/// The bridge writes and completes one frame per iteration in receive order, so awaiting the head is FIFO-safe. commentlint: allow(JUDGE)
const WRITER_WINDOW: usize = 32;

struct InFlight {
    publish: Option<Arc<AtomicU8>>,
    ack: Option<oneshot::Sender<()>>,
    charge: ByteCharge,
    deadline: Instant,
    completed: oneshot::Receiver<Result<(), SendFailure>>,
}

async fn writer_loop(
    inner: Arc<Inner>,
    write: RingWriteSender,
    mut data_rx: mpsc::Receiver<QueuedFrame>,
    mut control_rx: mpsc::Receiver<QueuedFrame>,
) {
    let mut window: VecDeque<InFlight> = VecDeque::with_capacity(WRITER_WINDOW);
    // Frames behind a failed head never reached the bridge's `try_recv`; they return to `QUEUED`.
    let fail =
        |inner: &Inner, window: &mut VecDeque<InFlight>, head_not_sent: Option<&InFlight>| {
            if let Some(state) = head_not_sent.and_then(|h| h.publish.as_ref()) {
                state.store(QUEUED, Ordering::Release);
            }
            for rest in window.drain(..) {
                if let Some(state) = &rest.publish {
                    state.store(QUEUED, Ordering::Release);
                }
            }
            inner.retire("write_failed");
        };
    loop {
        let next = if window.len() < WRITER_WINDOW {
            if let Ok(frame) = control_rx.try_recv() {
                Some(frame)
            } else if let Ok(frame) = data_rx.try_recv() {
                Some(frame)
            } else if window.is_empty() {
                tokio::select! {
                    biased;
                    () = inner.cancel.cancelled() => break,
                    // `Inner` holds `control_tx` and `data_tx`, so a closed channel is unreachable while `inner` is held. commentlint: allow(JUDGE)
                    // Break on channel closure so every wait remains inside the cancellation `select!`.
                    frame = control_rx.recv() => match frame {
                        Some(frame) => Some(frame),
                        None => break,
                    },
                    frame = data_rx.recv() => match frame {
                        Some(frame) => Some(frame),
                        None => break,
                    },
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(frame) = next {
            if frame
                .publish
                .as_ref()
                .is_some_and(|state| !claim_for_write(state))
            {
                continue;
            }
            let (completed_tx, completed_rx) = oneshot::channel();
            if write
                .try_send(RingWrite {
                    header: frame.header,
                    body: frame.body,
                    completed: completed_tx,
                    deadline: frame.deadline.into_std(),
                })
                .is_err()
            {
                // The bridge never received the frame, so nothing reached the ring.
                if let Some(state) = &frame.publish {
                    state.store(QUEUED, Ordering::Release);
                }
                fail(&inner, &mut window, None);
                break;
            }
            window.push_back(InFlight {
                publish: frame.publish,
                ack: frame.ack,
                charge: frame.charge,
                deadline: frame.deadline,
                completed: completed_rx,
            });
            continue;
        }
        let Some(mut head) = window.pop_front() else {
            continue;
        };
        let written = tokio::select! {
            biased;
            () = inner.cancel.cancelled() => break,
            result = timeout_at(head.deadline, &mut head.completed) => result,
        };
        match written {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(SendFailure::Deadline | SendFailure::Unreserved))) => {
                // Zero bytes reached the ring, so the frame returns to `QUEUED` and retirement classifies it `NotSent`.
                fail(&inner, &mut window, Some(&head));
                break;
            }
            _ => {
                fail(&inner, &mut window, None);
                break;
            }
        }
        if let Some(state) = &head.publish {
            state.store(WRITTEN, Ordering::Release);
        }
        if let Some(ack) = head.ack {
            let _ = ack.send(());
        }
        drop(head.charge);
    }
}

async fn ring_reader_loop(inner: Arc<Inner>, mut read: RingFrameReceiver) {
    while let Some((header, body, charge)) = read.recv().await {
        if validate_inbound(&header).is_err() || body.len() != header.len as usize {
            inner.retire("protocol_violation");
            return;
        }
        inner.dispatch(header, body, charge);
        if inner.retired.load(Ordering::Acquire) {
            return;
        }
    }
    inner.retire("eof");
}

/// The instant a request expires, or a local `invalid_timeout` error when the timeout does
/// not fit in the monotonic clock.
fn request_deadline(timeout: Duration) -> Result<Instant, CallError> {
    Instant::now().checked_add(timeout).ok_or_else(|| {
        CallError::local(
            SendOutcome::NotSent,
            "invalid_timeout",
            "request timeout is out of range",
        )
    })
}

fn validate_inbound(header: &EnvelopeHeader) -> Result<(), ()> {
    if header.ver != PROTOCOL_VERSION || header.len > MAX_BODY_LEN {
        return Err(());
    }
    // §7.1 caps channel-0 bodies at 65,536 bytes.
    if header.channel == 0 && header.len > MAX_CONTROL_BODY_LEN {
        return Err(());
    }
    match header.ty {
        // A terminal answers either a control request (0/0) or a routed one.
        // `decode_header` rejects mixed zero/nonzero channel/epoch pairs.
        FrameType::Response | FrameType::Error => {
            if header.corr == 0 {
                return Err(());
            }
            // §7.1 permits UTF-8 JSON only on channel 0.
            if header.channel == 0 && header.flags.is_binary() {
                return Err(());
            }
        }
        FrameType::StreamData | FrameType::StreamEnd => {
            if header.corr == 0 || header.channel == 0 || header.epoch == 0 {
                return Err(());
            }
            // The direct profile encodes stream termination in the header.
            // A `StreamEnd` body is structural corruption in the direct profile.
            // The framing layer does not classify `StreamEnd` as pure-header.
            if matches!(header.ty, FrameType::StreamEnd) && header.len != 0 {
                return Err(());
            }
        }
        FrameType::Push => {
            // `Push` frames must use correlation 0 because they are unsolicited.
            // A `Push` correlation would claim a pending request that the frame cannot answer.
            if header.channel == 0 || header.epoch == 0 || header.corr != 0 {
                return Err(());
            }
        }
        FrameType::Ping => {
            if header.channel != 0 || header.epoch != 0 || header.corr == 0 {
                return Err(());
            }
        }
        FrameType::Goodbye => {
            if header.corr != 0 || (header.channel != 0 && header.epoch == 0) {
                return Err(());
            }
        }
        _ => return Err(()),
    }
    // Pure-header frames must set binary 0, last 0, and admission Normal, but
    // §6.1 permits any valid priority — matching the framing layer's own check
    // in the host frame reader. Comparing the whole flag byte would retire the
    // generation over a conforming Ping that merely chose Interactive.
    if header.ty.is_pure_header()
        && (header.len != 0
            || header.flags.is_binary()
            || header.flags.is_last()
            || header.flags.admission_class() != Some(AdmissionClass::Normal))
    {
        return Err(());
    }
    Ok(())
}

fn encode_data_frame(
    route: RouteHandle,
    corr: u64,
    body: Vec<u8>,
    binary: bool,
    publish: Arc<AtomicU8>,
    budget: &Arc<ByteCounter>,
) -> Result<QueuedFrame, CallError> {
    let header = frame_header(
        FrameType::Request,
        Flags::new(binary, Priority::Interactive, false),
        FrameId::routed(route, corr),
        body.len(),
    )
    .map_err(|_| {
        CallError::local(
            SendOutcome::NotSent,
            "body_too_large",
            "request body exceeds wire limit",
        )
    })?;
    let charge = budget.charge(HEADER_LEN + body.len()).ok_or_else(|| {
        CallError::local(
            SendOutcome::NotSent,
            "queued_byte_capacity",
            "shared queued-byte capacity exhausted",
        )
    })?;
    Ok(QueuedFrame {
        header,
        body,
        charge,
        publish: Some(publish),
        ack: None,
        deadline: Instant::now() + CLIENT_FRAME_TIMEOUT,
    })
}

fn route_open_body(target: &RouteTarget, identity: &RouteIdentity) -> Result<Vec<u8>, CallError> {
    let project_root = identity.project_root.to_str().ok_or_else(|| {
        CallError::local(
            SendOutcome::NotSent,
            "invalid_identity",
            "route identity path is not UTF-8",
        )
    })?;
    let kind = match target.kind {
        TargetKind::ToolProvider => "tool_provider",
        TargetKind::ManagementSurface => "management_surface",
    };
    let mut request = serde_json::json!({
        "op": OP_ROUTE_OPEN,
        "target": {"kind": kind, "module_id": target.module_id},
        "identity": {
            "project_root": project_root,
            "harness": identity.harness,
            "session": identity.session
        },
        "consumer_capabilities": identity.consumer_capabilities
    });
    // A present JSON `null` makes `bind` observe `Some(..)`, unlike an absent member.
    if let Some(facts) = identity.admission_facts.as_ref() {
        request["admission_facts"] = facts.clone();
    }
    // `bind` decodes an absent `credential_fingerprints` member as an empty map.
    if !identity.credential_fingerprints.is_empty() {
        request["identity"]["credential_fingerprints"] =
            serde_json::json!(identity.credential_fingerprints);
    }
    // `bind` requires both members when `consumer_identity` is present, so a
    // half-specified consumer identity is rejected before sending.
    match (
        identity.consumer_module_id.as_ref(),
        identity.consumer_launch_nonce.as_ref(),
    ) {
        (Some(module_id), Some(launch_nonce)) => {
            request["consumer_identity"] = serde_json::json!({
                "module_id": module_id,
                "launch_nonce": launch_nonce
            });
        }
        (None, None) => {}
        _ => {
            return Err(CallError::local(
                SendOutcome::NotSent,
                "invalid_identity",
                "consumer identity requires both module_id and launch_nonce",
            ));
        }
    }
    serde_json::to_vec(&request).map_err(|_| {
        CallError::local(
            SendOutcome::NotSent,
            "invalid_identity",
            "route-open request could not be encoded",
        )
    })
}

fn parse_route_open(body: &[u8]) -> Result<RouteHandle, CallError> {
    let value = serde_json::from_slice::<Value>(body).map_err(|_| {
        CallError::local(
            SendOutcome::Terminal,
            "invalid_route_response",
            "host returned an invalid route-open response",
        )
    })?;
    if value.get("op").and_then(Value::as_str) != Some(OP_ROUTE_OPEN) {
        return Err(CallError::local(
            SendOutcome::Terminal,
            "invalid_route_response",
            "host returned an invalid route-open response",
        ));
    }
    let channel = value
        .get("route_channel")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            CallError::local(
                SendOutcome::Terminal,
                "invalid_route_response",
                "host returned an invalid route-open response",
            )
        })?;
    let epoch = value
        .get("route_epoch")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            CallError::local(
                SendOutcome::Terminal,
                "invalid_route_response",
                "host returned an invalid route-open response",
            )
        })?;
    Ok(RouteHandle { channel, epoch })
}

fn claim_for_write(state: &AtomicU8) -> bool {
    state
        .compare_exchange(QUEUED, WRITING, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn classify(state: &AtomicU8) -> SendOutcome {
    match state.load(Ordering::Acquire) {
        QUEUED | CANCELLED => SendOutcome::NotSent,
        WRITING | WRITTEN => SendOutcome::OutcomeUnknown,
        _ => SendOutcome::OutcomeUnknown,
    }
}

fn cancel_classification(state: &AtomicU8) -> SendOutcome {
    if state
        .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        SendOutcome::NotSent
    } else {
        classify(state)
    }
}

fn retired_error(outcome: SendOutcome) -> CallError {
    CallError::local(
        outcome,
        "generation_retired",
        "connection generation retired",
    )
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A code is kept verbatim only when every character is in the allowed set and
/// it fits `MAX_ERROR_CODE_BYTES`. Filtering characters out could alias a
/// nonconforming value onto a reserved code (`unknown_module!` → `unknown_module`)
/// and trigger that code's recovery rule, so the whole value falls back instead.
fn bounded_code(code: &str) -> String {
    let conforming = !code.is_empty()
        && code.len() <= MAX_ERROR_CODE_BYTES
        && code
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if conforming {
        code.to_owned()
    } else {
        "remote_error".to_owned()
    }
}

fn bounded_text(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::response_flags;

    fn test_inner(
        queued_bytes: usize,
    ) -> (
        Arc<Inner>,
        mpsc::Receiver<QueuedFrame>,
        mpsc::Receiver<QueuedFrame>,
    ) {
        let (data_tx, data_rx) = mpsc::channel(CLIENT_DATA_QUEUE_FRAMES);
        let (control_tx, control_rx) = mpsc::channel(CLIENT_CONTROL_QUEUE_FRAMES);
        (
            Arc::new(Inner {
                daemon_id: [0; DAEMON_ID_LEN],
                daemon_ver: "eidnara-host/0.0.0-test".to_owned(),
                closed: AtomicBool::new(false),
                retired: AtomicBool::new(false),
                cancel: CancellationToken::new(),
                correlations: Mutex::new(Correlations::new(FIRST_APPLICATION_CORRELATION)),
                admission: Mutex::new(()),
                pending: Mutex::new(HashMap::new()),
                streams: Mutex::new(0),
                routes: Mutex::new(HashSet::from([route(1), route(2)])),
                queue_budget: Arc::new(ByteCounter::new(queued_bytes)),
                control_budget: Arc::new(ByteCounter::new(CLIENT_CONTROL_QUEUED_BYTES)),
                _read_budget: Arc::new(ByteCounter::new(CLIENT_INBOUND_FRAME_BYTES)),
                retained_budget: Arc::new(ByteCounter::new(CLIENT_RETAINED_RESPONSE_BYTES)),
                unary_budget: Arc::new(ByteCounter::new(CLIENT_RETAINED_RESPONSE_BYTES)),
                data_tx,
                control_tx,
                close_lock: tokio::sync::Mutex::new(()),
                reader: tokio::sync::Mutex::new(None),
                writer: tokio::sync::Mutex::new(None),
                bridge: Mutex::new(None),
                bridge_wake: Weak::new(),
            }),
            data_rx,
            control_rx,
        )
    }

    fn route(epoch: u32) -> RouteHandle {
        RouteHandle { channel: 7, epoch }
    }

    fn retained_fixture(body: &[u8]) -> RetainedResponse {
        RetainedResponse {
            response: Response {
                body: body.to_vec(),
                binary: false,
            },
            _charge: ByteCharge::none(),
        }
    }

    fn unary_sender() -> (PendingKind, oneshot::Receiver<UnaryTerminal>) {
        let (tx, rx) = oneshot::channel();
        (PendingKind::Unary(tx), rx)
    }

    async fn ack_controls(mut rx: mpsc::Receiver<QueuedFrame>, count: usize) {
        for _ in 0..count {
            let mut frame = rx.recv().await.expect("control frame");
            frame
                .ack
                .take()
                .expect("close control has ack")
                .send(())
                .ok();
        }
    }

    #[test]
    fn max_correlation_is_used_once_then_exhausted() {
        let mut correlations = Correlations::new(u64::MAX);
        assert_eq!(correlations.allocate(), Some(u64::MAX));
        assert_eq!(correlations.allocate(), None);
        correlations.restore(u64::MAX);
        assert_eq!(correlations.allocate(), Some(u64::MAX));
        assert_eq!(correlations.allocate(), None);
    }

    #[tokio::test]
    async fn real_admission_exhausts_after_max_without_second_charge_or_frame() {
        let (inner, data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.correlations).next = Some(u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(1);
        let (first_kind, _first_rx) = unary_sender();
        let (key, _) = inner
            .admit(route(1), Vec::new(), false, first_kind, deadline)
            .expect("u64::MAX is admitted once");
        assert_eq!(key.corr, u64::MAX);
        let charged = inner.queue_budget.used();
        assert_eq!(data_rx.len(), 1);

        let (second_kind, _second_rx) = unary_sender();
        let error = inner
            .admit(route(1), Vec::new(), false, second_kind, deadline)
            .expect_err("correlation space is exhausted");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "correlations_exhausted");
        assert_eq!(data_rx.len(), 1);
        assert_eq!(inner.queue_budget.used(), charged);
        assert!(
            inner.retired.load(Ordering::Acquire),
            "§8.3: a generation past u64::MAX must retire before another request"
        );
        assert!(
            inner.cancel.is_cancelled(),
            "retirement stops the writer and reader tasks"
        );

        drop(data_rx);
        assert_eq!(inner.queue_budget.used(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_wins_against_admission_blocked_on_pending() {
        let (inner, data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let client = Arc::new(Client {
            inner: Arc::clone(&inner),
        });
        let pending = lock_unpoisoned(&inner.pending);
        let closer = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.close().await })
        };
        while !inner.closed.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        loop {
            match inner.admission.try_lock() {
                Err(std::sync::TryLockError::WouldBlock) => break,
                Err(std::sync::TryLockError::Poisoned(_)) => panic!("admission lock poisoned"),
                Ok(guard) => drop(guard),
            }
            std::thread::yield_now();
        }
        let admission = {
            let inner = Arc::clone(&inner);
            tokio::task::spawn_blocking(move || {
                let (kind, _rx) = unary_sender();
                inner.admit(
                    route(1),
                    b"must-not-write".to_vec(),
                    false,
                    kind,
                    Instant::now() + Duration::from_secs(1),
                )
            })
        };
        drop(pending);
        let acknowledger = tokio::spawn(ack_controls(control_rx, 3));

        closer.await.unwrap().expect("close completes");
        acknowledger.await.unwrap();
        let error = admission
            .await
            .unwrap()
            .expect_err("close wins admission ordering");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "connection_retired");
        assert!(lock_unpoisoned(&inner.pending).is_empty());
        assert!(data_rx.is_empty(), "losing admission queues no write");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exact_route_close_wins_against_admission_blocked_on_pending() {
        let (inner, data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let pending = lock_unpoisoned(&inner.pending);
        let closer = {
            let inner = Arc::clone(&inner);
            tokio::task::spawn_blocking(move || inner.settle_route(route(1)))
        };
        loop {
            match inner.admission.try_lock() {
                Err(std::sync::TryLockError::WouldBlock) => break,
                Err(std::sync::TryLockError::Poisoned(_)) => panic!("admission lock poisoned"),
                Ok(guard) => drop(guard),
            }
            std::thread::yield_now();
        }
        let admission = {
            let inner = Arc::clone(&inner);
            tokio::task::spawn_blocking(move || {
                let (kind, _rx) = unary_sender();
                inner.admit(
                    route(1),
                    b"must-not-write".to_vec(),
                    false,
                    kind,
                    Instant::now() + Duration::from_secs(1),
                )
            })
        };
        drop(pending);

        assert!(closer.await.unwrap(), "exact route was live");
        let error = admission
            .await
            .unwrap()
            .expect_err("closed route rejects admission");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "route_not_live");
        assert!(lock_unpoisoned(&inner.pending).is_empty());
        assert!(data_rx.is_empty(), "losing admission queues no write");
        assert!(lock_unpoisoned(&inner.routes).contains(&route(2)));
    }

    #[tokio::test]
    async fn admission_winning_is_settled_by_close() {
        let (inner, data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(1);
        let (kind, rx) = unary_sender();
        let (_key, publish) = inner
            .admit(route(1), b"admitted".to_vec(), false, kind, deadline)
            .expect("admission wins");
        let client = Client {
            inner: Arc::clone(&inner),
        };
        let acknowledger = tokio::spawn(ack_controls(control_rx, 3));

        client.close().await.expect("close completes");
        acknowledger.await.unwrap();
        let error = rx.await.unwrap().expect_err("close settles admitted work");
        assert_eq!(error.code(), "owner_close");
        assert_eq!(classify(&publish), SendOutcome::NotSent);
        assert!(lock_unpoisoned(&inner.pending).is_empty());
        assert_eq!(data_rx.len(), 1, "admission queued exactly one frame");
    }

    #[tokio::test]
    async fn cancel_winning_queued_prevents_writer_claim_and_frame() {
        let (inner, data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(1);
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(route(1), b"must-not-send".to_vec(), false, kind, deadline)
            .expect("admitted");
        inner.cancel_key(key, "cancelled").expect("cancel queued");
        let error = rx.await.expect("settled").expect_err("cancelled");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(publish.load(Ordering::Acquire), CANCELLED);
        assert!(!claim_for_write(&publish));
        assert!(control_rx.try_recv().is_err(), "not-sent needs no Cancel");

        let (write, writes) = fake_ring_writer();
        let writer_inner = Arc::clone(&inner);
        let writer = tokio::spawn(async move {
            writer_loop(writer_inner, write, data_rx, control_rx).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            matches!(writes.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "cancel-winning queued request must publish no ring frame"
        );
        assert_eq!(inner.queue_budget.used(), 0);
        inner.cancel.cancel();
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn writer_winning_cancel_is_outcome_unknown_and_queues_cancel() {
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                b"possibly-sent".to_vec(),
                false,
                kind,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish), "writer wins QUEUED CAS");
        inner.cancel_key(key, "cancelled").expect("cancel writing");
        let error = rx.await.expect("settled").expect_err("cancelled");
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
        let cancel = control_rx.recv().await.expect("Cancel queued");
        assert_eq!(cancel.header.ty, FrameType::Cancel);
        assert_eq!(publish.load(Ordering::Acquire), WRITING);
        drop(data_rx.recv().await);
        drop(cancel);
        assert_eq!(inner.queue_budget.used(), 0);
    }

    fn fake_ring_writer() -> (RingWriteSender, std::sync::mpsc::Receiver<RingWrite>) {
        let (tx, writes) = std::sync::mpsc::sync_channel(1);
        let wake = Arc::new(
            rustix::event::eventfd(
                0,
                rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
            )
            .unwrap(),
        );
        (RingWriteSender { tx, wake }, writes)
    }

    async fn settle_one_write_with(failure: SendFailure) -> CallError {
        let (inner, data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, rx) = unary_sender();
        let (_key, publish) = inner
            .admit(
                route(1),
                b"never-reserved".to_vec(),
                false,
                kind,
                Instant::now() + Duration::from_secs(5),
            )
            .expect("admitted");
        let (write, writes) = fake_ring_writer();
        let writer_inner = Arc::clone(&inner);
        let writer = tokio::spawn(async move {
            writer_loop(writer_inner, write, data_rx, control_rx).await;
        });
        let ring_write = tokio::task::spawn_blocking(move || {
            writes
                .recv_timeout(Duration::from_secs(2))
                .expect("writer hands the frame to the bridge")
        })
        .await
        .expect("bridge receive task");
        assert_eq!(publish.load(Ordering::Acquire), WRITING);
        ring_write
            .completed
            .send(Err(failure))
            .expect("writer awaits completion");
        writer.await.expect("writer exits after retiring");
        assert!(inner.retired.load(Ordering::Acquire));
        rx.await.expect("settled").expect_err("retired")
    }

    #[tokio::test]
    async fn a_frame_that_never_reserved_ring_space_settles_not_sent() {
        // `reserve_until` expiry proves zero bytes reached the host, so the
        // caller may retry on a fresh generation.
        let error = settle_one_write_with(SendFailure::Deadline).await;
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "write_failed");
        let error = settle_one_write_with(SendFailure::Unreserved).await;
        assert_eq!(error.outcome(), SendOutcome::NotSent);
    }

    #[tokio::test]
    async fn a_frame_that_failed_after_reservation_stays_outcome_unknown() {
        let error = settle_one_write_with(SendFailure::Reserved).await;
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
        assert_eq!(error.code(), "write_failed");
    }

    #[tokio::test]
    async fn a_binary_request_sets_the_frame_binary_flag() {
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(1);
        let (kind, _rx) = unary_sender();
        inner
            .admit(route(1), vec![0xff, 0x00], true, kind, deadline)
            .expect("binary request admitted");
        let frame = data_rx.recv().await.expect("queued frame");
        assert!(frame.header.flags.is_binary());

        let (kind, _rx) = unary_sender();
        inner
            .admit(route(1), b"{}".to_vec(), false, kind, deadline)
            .expect("json request admitted");
        let frame = data_rx.recv().await.expect("queued frame");
        assert!(!frame.header.flags.is_binary());
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn failed_cancel_enqueue_keeps_outcome_unknown() {
        let (inner, mut data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                b"possibly-sent".to_vec(),
                false,
                kind,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish), "writer claims the request");
        // Retiring without draining pending makes the best-effort `Cancel` enqueue fail with `NotSent`.
        inner.retired.store(true, Ordering::Release);

        let error = inner
            .cancel_key(key, "cancelled")
            .expect_err("Cancel cannot be queued on a retired generation");
        assert_eq!(
            error.outcome(),
            SendOutcome::OutcomeUnknown,
            "a claimed request stays possibly-sent when its Cancel cannot be queued"
        );
        assert_eq!(error.code(), "generation_retired");
        let settled = rx.await.expect("settled").expect_err("cancelled");
        assert_eq!(settled.outcome(), SendOutcome::OutcomeUnknown);
        drop(data_rx.recv().await);
        drop(control_rx);
    }

    #[tokio::test]
    async fn settled_stream_retires_its_deadline_watcher() {
        // `cancel_key` and terminal-frame dispatch settle through different paths; removing the watcher entry is the only cleanup common to both.
        for terminal in [None, Some(FrameType::StreamEnd)] {
            let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
            let (items_tx, _items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
            let (terminal_tx, terminal_rx) = oneshot::channel();
            let settled = CancellationToken::new();
            let (key, _publish) = inner
                .admit(
                    route(1),
                    Vec::new(),
                    false,
                    PendingKind::Stream {
                        items: items_tx,
                        terminal: terminal_tx,
                        _settled: settled.clone().drop_guard(),
                    },
                    Instant::now() + Duration::from_secs(600),
                )
                .expect("stream admitted");
            assert!(
                !settled.is_cancelled(),
                "the watcher must stay armed while the stream is live"
            );
            drop(data_rx.recv().await);

            match terminal {
                None => {
                    inner.cancel_key(key, "cancelled").expect("stream settled");
                }
                Some(ty) => inner.dispatch(
                    EnvelopeHeader {
                        len: 0,
                        ver: PROTOCOL_VERSION,
                        ty,
                        flags: response_flags(false, true),
                        channel: key.channel,
                        epoch: key.epoch,
                        corr: key.corr,
                    },
                    Vec::new(),
                    ByteCharge::none(),
                ),
            }

            assert!(
                settled.is_cancelled(),
                "settling via {terminal:?} must retire the watcher instead of \
                 leaving it asleep until the deadline"
            );
            assert!(lock_unpoisoned(&inner.pending).is_empty());
            let _ = terminal_rx.await;
        }
    }

    #[tokio::test]
    async fn route_settlement_never_floods_the_reserved_control_queue() {
        // Sending one `Cancel` per claimed request can exhaust the 32 reserved control slots; `send_control` then retires the generation and disconnects unrelated routes.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let route = route(1);
        lock_unpoisoned(&inner.routes).insert(route);
        for _ in 0..CLIENT_CONTROL_QUEUE_FRAMES + 1 {
            let (kind, _rx) = unary_sender();
            let (_key, publish) = inner
                .admit(
                    route,
                    Vec::new(),
                    false,
                    kind,
                    Instant::now() + Duration::from_secs(60),
                )
                .expect("admitted");
            // Claiming a request classifies its settlement as possibly sent, which requires a `Cancel`.
            assert!(claim_for_write(&publish));
            drop(data_rx.recv().await);
        }

        assert!(inner.settle_route(route));

        assert!(
            !inner.retired.load(Ordering::Acquire),
            "route settlement must not retire the generation"
        );
        assert!(
            control_rx.try_recv().is_err(),
            "route Goodbye already settles the host side; per-correlation Cancel adds only overflow risk"
        );
        assert!(lock_unpoisoned(&inner.pending).is_empty());
    }

    #[test]
    fn inbound_validation_enforces_the_direct_profile_table() {
        let header =
            |ty: FrameType, channel: u16, epoch: u32, corr: u64, len: u32| EnvelopeHeader {
                len,
                ver: PROTOCOL_VERSION,
                ty,
                flags: if ty.is_pure_header() {
                    pure_header_flags()
                } else {
                    Flags::new(false, Priority::Interactive, false)
                },
                channel,
                epoch,
                corr,
            };

        assert!(validate_inbound(&header(FrameType::Response, 0, 0, 7, 4)).is_ok());
        assert!(validate_inbound(&header(FrameType::Response, 3, 9, 7, 4)).is_ok());
        assert!(validate_inbound(&header(FrameType::StreamData, 3, 9, 7, 4)).is_ok());
        assert!(validate_inbound(&header(FrameType::StreamEnd, 3, 9, 7, 0)).is_ok());
        assert!(validate_inbound(&header(FrameType::Push, 3, 9, 0, 4)).is_ok());

        assert!(validate_inbound(&header(FrameType::Response, 3, 0, 7, 4)).is_ok());

        assert!(validate_inbound(&header(FrameType::StreamEnd, 3, 9, 7, 1)).is_err());

        assert!(validate_inbound(&header(FrameType::Push, 3, 9, 5, 4)).is_err());

        // §7.1 caps channel-0 bodies at 65,536 bytes.
        assert!(
            validate_inbound(&header(FrameType::Response, 0, 0, 7, MAX_CONTROL_BODY_LEN)).is_ok()
        );
        assert!(
            validate_inbound(&header(
                FrameType::Response,
                0,
                0,
                7,
                MAX_CONTROL_BODY_LEN + 1
            ))
            .is_err()
        );
        // A routed body is opaque and keeps the framing cap.
        assert!(
            validate_inbound(&header(
                FrameType::Response,
                3,
                9,
                7,
                MAX_CONTROL_BODY_LEN + 1
            ))
            .is_ok()
        );

        // §6.2 requires stream frames to use routed identities.
        // A control identity is structurally illegal, not merely unmatched.
        assert!(validate_inbound(&header(FrameType::StreamData, 0, 0, 7, 4)).is_err());
        assert!(validate_inbound(&header(FrameType::StreamEnd, 0, 0, 7, 0)).is_err());
        // A terminal may still answer a control request.
        assert!(validate_inbound(&header(FrameType::Response, 0, 0, 7, 4)).is_ok());
        assert!(validate_inbound(&header(FrameType::Error, 0, 0, 7, 4)).is_ok());

        // Channel-0 bodies must be UTF-8 JSON.
        // A binary terminal on channel 0 is malformed.
        let binary_control = EnvelopeHeader {
            len: 4,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Response,
            flags: response_flags(true, true),
            channel: 0,
            epoch: 0,
            corr: 7,
        };
        assert!(validate_inbound(&binary_control).is_err());
        // A routed body stays opaque and may be binary.
        let binary_routed = EnvelopeHeader {
            channel: 3,
            epoch: 9,
            ..binary_control
        };
        assert!(validate_inbound(&binary_routed).is_ok());

        assert!(validate_inbound(&header(FrameType::Response, 3, 9, 0, 4)).is_err());
        assert!(validate_inbound(&header(FrameType::Ping, 0, 0, 7, 0)).is_ok());
        assert!(validate_inbound(&header(FrameType::Ping, 1, 0, 7, 0)).is_err());
        assert!(validate_inbound(&header(FrameType::Goodbye, 0, 0, 0, 0)).is_ok());
        assert!(validate_inbound(&header(FrameType::Goodbye, 3, 9, 0, 1)).is_err());
        assert!(validate_inbound(&header(FrameType::Request, 3, 9, 7, 4)).is_err());
    }

    #[tokio::test]
    async fn a_ping_at_any_valid_priority_is_answered_with_an_exact_flag_echo() {
        // `Ping` fixes binary, last, and admission flags; priority may use any valid value.
        // `Pong` must echo `Ping`'s flags exactly.
        for priority in [
            Priority::Passive,
            Priority::Interactive,
            Priority::Background,
        ] {
            let (inner, _data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
            let flags = Flags::new(false, priority, false);
            let ping = EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Ping,
                flags,
                channel: 0,
                epoch: 0,
                corr: 41,
            };
            assert!(
                validate_inbound(&ping).is_ok(),
                "{priority:?} is a valid Ping priority, not a reason to retire"
            );

            inner.dispatch(ping, Vec::new(), ByteCharge::none());

            let pong = control_rx.recv().await.expect("Pong queued");
            assert_eq!(pong.header.ty, FrameType::Pong);
            assert_eq!(
                pong.header.flags, flags,
                "the Pong must echo the Ping's flag byte, not the client's default"
            );
            assert!(!inner.retired.load(Ordering::Acquire));
            drop(pong);
        }

        // `Ping` fixes binary, last, and admission flags.
        for flags in [
            Flags::new(true, Priority::Passive, false),
            Flags::new(false, Priority::Passive, true),
        ] {
            let ping = EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Ping,
                flags,
                channel: 0,
                epoch: 0,
                corr: 41,
            };
            assert!(validate_inbound(&ping).is_err());
        }
    }

    #[tokio::test]
    async fn a_zero_length_stream_item_is_delivered_without_retiring() {
        // Only `StreamEnd` must be empty; zero-length `StreamData` is valid.
        // A zero-length `StreamData` incurs no charge because it carries no bytes.
        // A zero-length `StreamData` must reach the stream instead of retiring it.
        // generation.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (items_tx, mut items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, _terminal_rx) = oneshot::channel();
        let (key, _publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        drop(data_rx.recv().await);

        inner.dispatch(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::StreamData,
                flags: response_flags(false, false),
                channel: key.channel,
                epoch: key.epoch,
                corr: key.corr,
            },
            Vec::new(),
            ByteCharge::none(),
        );

        assert!(
            !inner.retired.load(Ordering::Acquire),
            "an empty item carries a no-op charge, not an exhausted budget"
        );
        let item = items_rx.try_recv().expect("the empty item is delivered");
        assert!(item.body.is_empty());
        assert!(lock_unpoisoned(&inner.pending).contains_key(&key));
    }

    #[tokio::test]
    async fn an_out_of_range_timeout_is_rejected_instead_of_panicking() {
        // `Duration::MAX` means no timeout; reject an unrepresentable deadline with a typed error instead of panicking.
        let error = request_deadline(Duration::MAX).expect_err("unrepresentable");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "invalid_timeout");
        assert!(request_deadline(Duration::from_secs(30)).is_ok());
    }

    #[tokio::test]
    async fn a_pre_cancelled_stream_never_enqueues_a_frame() {
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let cancelled = CancellationToken::new();
        cancelled.cancel();

        let error = inner
            .start_stream(
                route(1),
                b"must-not-send".to_vec(),
                RequestOptions {
                    timeout: Duration::from_secs(30),
                    cancellation: Some(cancelled),
                    binary: false,
                },
            )
            .expect_err("an already-cancelled token admits nothing");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "cancelled");
        assert!(
            data_rx.try_recv().is_err(),
            "a cancelled stream must not reach the writer"
        );
        assert!(lock_unpoisoned(&inner.pending).is_empty());
        assert_eq!(
            *lock_unpoisoned(&inner.streams),
            0,
            "no live stream charged"
        );
    }

    #[tokio::test]
    async fn a_pre_cancelled_unary_never_enqueues_a_frame() {
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let cancelled = CancellationToken::new();
        cancelled.cancel();

        let error = inner
            .unary(
                route(1),
                b"must-not-send".to_vec(),
                false,
                Instant::now() + Duration::from_secs(30),
                Some(cancelled),
            )
            .await
            .expect_err("an already-cancelled token admits nothing");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "cancelled");
        assert!(
            data_rx.try_recv().is_err(),
            "a cancelled request must not reach the writer"
        );
        assert!(lock_unpoisoned(&inner.pending).is_empty());
    }

    #[tokio::test]
    async fn a_terminal_that_wins_the_cancellation_race_is_not_discarded() {
        // Remove the pending entry before publishing the terminal so a concurrent stop cannot cancel a completed request.
        // A stop after `dispatch` removes the pending entry finds nothing to cancel.
        // Reporting a local error after the host answers would discard that answer and force outcome-unknown recovery for a settled operation.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(60),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);

        // A concurrent stop cannot enqueue `Cancel` after the terminal becomes observable.
        let state = lock_unpoisoned(&inner.pending)
            .remove(&key)
            .expect("entry exists");
        match state.kind {
            PendingKind::Unary(tx) => tx
                .send(Ok(retained_fixture(b"authoritative")))
                .expect("terminal published"),
            PendingKind::Stream { .. } => unreachable!("admitted a unary request"),
        }

        let mut rx = rx;
        let response = inner
            .stop_or_take_terminal(key, &mut rx, &publish, "cancelled", "request was cancelled")
            .await
            .expect("the observed terminal wins over the local stop");
        assert_eq!(response.response.body, b"authoritative");
    }

    #[tokio::test]
    async fn a_terminal_still_in_flight_wins_over_the_local_stop() {
        // Removing the pending entry before sending the terminal prevents a concurrent stop from cancelling a completed request.
        // A single `try_recv` can report `OutcomeUnknown` after the host answered if `dispatch` holds the sender before publishing the terminal.
        // stop must wait for the owner that holds the sender.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(60),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);

        let state = lock_unpoisoned(&inner.pending)
            .remove(&key)
            .expect("entry exists");
        let PendingKind::Unary(tx) = state.kind else {
            unreachable!("admitted a unary request")
        };

        let mut rx = rx;
        let stop = async {
            inner
                .stop_or_take_terminal(key, &mut rx, &publish, "cancelled", "request was cancelled")
                .await
        };
        let publish_terminal = async {
            // `publish_terminal` waits after `stop` observes the absent entry and before the owner publishes.
            tokio::task::yield_now().await;
            tx.send(Ok(retained_fixture(b"authoritative")))
                .expect("terminal published");
        };
        let (result, ()) = tokio::join!(stop, publish_terminal);
        let response = result.expect("the in-flight terminal wins over the local stop");
        assert_eq!(response.response.body, b"authoritative");
    }

    #[tokio::test]
    async fn a_dropped_sender_after_an_absent_entry_reports_the_send_outcome() {
        // When generation retirement removes the pending entry, it drops the sender.
        // Generation retirement can drop the sender after taking the pending entry.
        // `stop` must classify generation-retirement removal from publish state.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(60),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);
        drop(
            lock_unpoisoned(&inner.pending)
                .remove(&key)
                .expect("entry exists"),
        );

        let mut rx = rx;
        let error = inner
            .stop_or_take_terminal(key, &mut rx, &publish, "cancelled", "request was cancelled")
            .await
            .expect_err("a dropped sender publishes no terminal");
        assert_eq!(error.code, "generation_retired");
        assert_eq!(
            error.outcome,
            SendOutcome::OutcomeUnknown,
            "a claimed request whose sender vanished may still have been delivered"
        );
    }

    #[test]
    fn absent_admission_facts_are_omitted_rather_than_sent_as_null() {
        // The host reads every present member as `Some(..)`.
        // `bind` observes caller-omitted facts when a present member is null.
        let target = RouteTarget {
            kind: TargetKind::ToolProvider,
            module_id: "context".to_owned(),
        };
        let mut identity = identity_fixture();
        let body = route_open_body(&target, &identity).expect("body encodes");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert!(
            value.get("admission_facts").is_none(),
            "absent facts must not appear as an explicit null"
        );

        identity.admission_facts = Some(serde_json::json!({"tier": "gold"}));
        let body = route_open_body(&target, &identity).expect("body encodes");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(
            value["admission_facts"],
            serde_json::json!({"tier": "gold"})
        );
    }

    #[test]
    fn credential_fingerprints_ride_inside_identity_only_when_present() {
        // `bind` reads `identity.credential_fingerprints`.
        let target = RouteTarget {
            kind: TargetKind::ToolProvider,
            module_id: "context".to_owned(),
        };
        let mut identity = identity_fixture();
        let body = route_open_body(&target, &identity).expect("body encodes");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert!(
            value["identity"].get("credential_fingerprints").is_none(),
            "an empty map is omitted rather than sent as {{}}"
        );

        let fingerprint = "a".repeat(64);
        identity
            .credential_fingerprints
            .insert("anthropic".to_owned(), fingerprint.clone());
        let body = route_open_body(&target, &identity).expect("body encodes");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(
            value["identity"]["credential_fingerprints"],
            serde_json::json!({"anthropic": fingerprint}),
            "fingerprints are nested under identity where bind reads them"
        );
    }

    #[test]
    fn a_half_specified_consumer_identity_is_rejected_before_sending() {
        // `bind` requires both members of a present `consumer_identity`.
        let target = RouteTarget {
            kind: TargetKind::ToolProvider,
            module_id: "context".to_owned(),
        };
        let mut identity = identity_fixture();
        identity.consumer_module_id = Some("synapse".to_owned());
        let error = route_open_body(&target, &identity).expect_err("nonce is missing");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "invalid_identity");

        identity.consumer_module_id = None;
        identity.consumer_launch_nonce = Some("nonce".to_owned());
        let error = route_open_body(&target, &identity).expect_err("module id is missing");
        assert_eq!(error.code(), "invalid_identity");

        identity.consumer_module_id = Some("synapse".to_owned());
        let body = route_open_body(&target, &identity).expect("both members encode");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(
            value["consumer_identity"],
            serde_json::json!({"module_id": "synapse", "launch_nonce": "nonce"})
        );
    }

    fn identity_fixture() -> RouteIdentity {
        RouteIdentity {
            project_root: std::path::PathBuf::from("/tmp/project"),
            harness: "opencode".to_owned(),
            session: "session".to_owned(),
            consumer_module_id: None,
            consumer_launch_nonce: None,
            consumer_capabilities: Vec::new(),
            admission_facts: None,
            credential_fingerprints: std::collections::BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn dropped_unary_future_cleans_pending_and_possibly_sent_request() {
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let task_inner = Arc::clone(&inner);
        let request = tokio::spawn(async move {
            task_inner
                .unary(
                    route(1),
                    b"stalled-peer".to_vec(),
                    false,
                    Instant::now() + Duration::from_secs(60),
                    None,
                )
                .await
        });
        let frame = data_rx.recv().await.expect("request admitted");
        let publish = frame.publish.as_ref().expect("data publication state");
        assert!(
            claim_for_write(publish),
            "simulate stalled writer after claim"
        );
        request.abort();
        assert!(request.await.expect_err("request aborted").is_cancelled());

        assert!(lock_unpoisoned(&inner.pending).is_empty());
        let cancel = control_rx.recv().await.expect("possibly-sent Cancel");
        assert_eq!(cancel.header.ty, FrameType::Cancel);
        drop(frame);
        drop(cancel);
        assert_eq!(inner.queue_budget.used(), 0);
    }

    #[tokio::test]
    async fn dropped_close_retires_and_repeated_close_joins_tasks() {
        let (inner, _data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let writer_cancel = inner.cancel.clone();
        *inner.writer.lock().await = Some(tokio::spawn(async move {
            writer_cancel.cancelled().await;
        }));
        let reader_cancel = inner.cancel.clone();
        *inner.reader.lock().await = Some(tokio::spawn(async move {
            reader_cancel.cancelled().await;
        }));
        let client = Arc::new(Client {
            inner: Arc::clone(&inner),
        });
        let closing = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.close().await })
        };
        while !inner.closed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        closing.abort();
        assert!(closing.await.expect_err("close aborted").is_cancelled());
        assert!(inner.retired.load(Ordering::Acquire));
        assert!(inner.cancel.is_cancelled());

        timeout_at(Instant::now() + Duration::from_secs(1), client.close())
            .await
            .expect("second close bounded")
            .expect("second close succeeds");
        assert!(inner.writer.lock().await.is_none());
        assert!(inner.reader.lock().await.is_none());
    }

    #[tokio::test]
    async fn data_capacity_spares_control_reserve_and_does_not_burn_correlation() {
        let (inner, data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut receivers = Vec::new();
        for _ in 0..CLIENT_DATA_QUEUE_FRAMES {
            let (kind, rx) = unary_sender();
            inner
                .admit(route(1), Vec::new(), false, kind, deadline)
                .expect("data slot");
            receivers.push(rx);
        }
        let next_before = lock_unpoisoned(&inner.correlations).next;
        let (kind, _rx) = unary_sender();
        let error = inner
            .admit(route(1), Vec::new(), false, kind, deadline)
            .expect_err("257th data frame is rejected");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "writer_queue_full");
        assert_eq!(lock_unpoisoned(&inner.correlations).next, next_before);

        inner
            .send_control(
                FrameType::Pong,
                pure_header_flags(),
                FrameId::control(99),
                None,
            )
            .expect("reserved control remains available");
        let pong = control_rx.recv().await.expect("queued Pong");
        assert_eq!(pong.header.ty, FrameType::Pong);
        drop(pong);
        assert_eq!(data_rx.len(), CLIENT_DATA_QUEUE_FRAMES);

        inner.retire("test_done");
        drop(data_rx);
        drop(control_rx);
        drop(receivers);
        assert_eq!(inner.queue_budget.used(), 0);
    }

    #[tokio::test]
    async fn control_exhaustion_retires_and_releases_all_queued_bytes() {
        let (inner, data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        for corr in 1..=CLIENT_CONTROL_QUEUE_FRAMES as u64 {
            inner
                .send_control(
                    FrameType::Pong,
                    pure_header_flags(),
                    FrameId::control(corr),
                    None,
                )
                .expect("reserved slot");
        }
        let error = inner
            .send_control(
                FrameType::Pong,
                pure_header_flags(),
                FrameId::control(99),
                None,
            )
            .expect_err("33rd control retires generation");
        assert_eq!(error.code(), "control_capacity_exhausted");
        assert!(inner.retired.load(Ordering::Acquire));
        drop(data_rx);
        drop(control_rx);
        assert_eq!(inner.queue_budget.used(), 0);
        assert!(lock_unpoisoned(&inner.pending).is_empty());
    }

    #[tokio::test]
    async fn data_saturation_never_starves_a_control_frame() {
        // Control frames must not charge the request-body pool: failed data charges reject one caller, but failed control charges retire the generation.
        // Control frames use a separate budget because queued data can otherwise retire the generation when Pong or Cancel admission fails.
        let (inner, data_rx, mut control_rx) = test_inner(HEADER_LEN);
        let (kind, _rx) = unary_sender();
        inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("data header fills the whole data budget");
        assert_eq!(
            inner.queue_budget.used(),
            HEADER_LEN,
            "the data pool is now saturated"
        );

        inner
            .send_control(
                FrameType::Pong,
                pure_header_flags(),
                FrameId::control(1),
                None,
            )
            .expect("a reserved control frame does not compete with request bytes");
        assert!(
            !inner.retired.load(Ordering::Acquire),
            "ordinary traffic must never retire the generation through a starved control frame"
        );
        assert_eq!(inner.control_budget.used(), HEADER_LEN);
        let queued = control_rx.try_recv().expect("Pong queued");
        assert_eq!(queued.header.ty, FrameType::Pong);

        drop(data_rx);
        drop(control_rx);
        assert_eq!(inner.queue_budget.used(), 0);
    }

    #[tokio::test]
    async fn stale_epoch_terminal_cannot_settle_reused_channel() {
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, mut rx) = unary_sender();
        let (key, _) = inner
            .admit(
                route(2),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("admit current epoch");
        drop(data_rx.recv().await);
        inner.dispatch(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, true),
                channel: key.channel,
                epoch: 1,
                corr: key.corr,
            },
            Vec::new(),
            ByteCharge::none(),
        );
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(lock_unpoisoned(&inner.pending).contains_key(&key));
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn saturated_stream_fails_alone_and_queues_cancel() {
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (items_tx, _items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (stream_key, _) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(1),
            )
            .expect("stream admitted");
        drop(data_rx.recv().await);
        let (unary_kind, mut unary_rx) = unary_sender();
        let (unary_key, _) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                unary_kind,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("unrelated unary admitted");
        drop(data_rx.recv().await);

        for _ in 0..=CLIENT_STREAM_QUEUE_ITEMS {
            let charge = inner.retained_budget.charge(1).expect("retained byte");
            inner.dispatch(
                EnvelopeHeader {
                    len: 1,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::StreamData,
                    flags: response_flags(false, false),
                    channel: stream_key.channel,
                    epoch: stream_key.epoch,
                    corr: stream_key.corr,
                },
                vec![1],
                charge,
            );
        }
        let error = terminal_rx
            .await
            .expect("terminal sender")
            .expect_err("saturated stream fails");
        assert_eq!(error.code(), "stream_saturated");
        // Saturation occurs after publication without a terminal frame; because Cancel is best-effort, report `OutcomeUnknown` rather than `Terminal`.
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
        let cancel = control_rx.recv().await.expect("stream Cancel");
        assert_eq!(cancel.header.ty, FrameType::Cancel);

        inner.dispatch(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, true),
                channel: unary_key.channel,
                epoch: unary_key.epoch,
                corr: unary_key.corr,
            },
            Vec::new(),
            ByteCharge::none(),
        );
        assert!(unary_rx.try_recv().expect("unary settled").is_ok());
        assert!(lock_unpoisoned(&inner.pending).is_empty());
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn unary_stream_data_is_unknown_not_terminal() {
        // `StreamData` is nonterminal and Cancel is best-effort, so report `OutcomeUnknown` rather than `Terminal`.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (kind, mut rx) = unary_sender();
        let (key, _publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(60),
            )
            .expect("admitted");
        drop(data_rx.recv().await);

        inner.dispatch(
            EnvelopeHeader {
                len: 1,
                ver: PROTOCOL_VERSION,
                ty: FrameType::StreamData,
                flags: response_flags(false, false),
                channel: key.channel,
                epoch: key.epoch,
                corr: key.corr,
            },
            vec![1],
            inner.retained_budget.charge(1).expect("retained byte"),
        );

        let error = rx
            .try_recv()
            .expect("unary settled")
            .expect_err("stream data on a unary is a violation");
        assert_eq!(error.code(), "unexpected_stream");
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
        let cancel = control_rx.recv().await.expect("scoped Cancel");
        assert_eq!(cancel.header.ty, FrameType::Cancel);
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn cancelling_a_stream_releases_its_queued_item_charges() {
        // Cancellation releases queued-item charges because `next` stops at `finished`; otherwise a retained stream pins the owner budget.
        // generation.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (items_tx, items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        // The host streams items only after receiving the request.
        // The writer has claimed the request before the host streams items.
        // `OutcomeUnknown` emits the `Cancel` frame.
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);

        let mut stream = ResponseStream {
            inner: Arc::downgrade(&inner),
            key,
            correlation: key.corr,
            items: items_rx,
            terminal: Some(terminal_rx),
            finished: false,
        };

        // Queued items simulate a slow consumer.
        // them.
        const ITEMS: usize = 4;
        for _ in 0..ITEMS {
            inner.dispatch(
                EnvelopeHeader {
                    len: 8,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::StreamData,
                    flags: response_flags(false, false),
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                vec![7; 8],
                inner.retained_budget.charge(8).expect("retained bytes"),
            );
        }
        assert_eq!(inner.retained_budget.used(), ITEMS * 8);

        stream.cancel().expect("cancel succeeds");
        assert_eq!(
            inner.retained_budget.used(),
            0,
            "cancel released every queued item's charge while the stream is still alive"
        );
        // `stream` remains alive so cancellation releases queued charges before `ResponseStream::drop`.
        assert!(
            stream
                .next()
                .await
                .expect("cancelled stream ends")
                .is_none()
        );
        let cancel = control_rx.recv().await.expect("scoped Cancel");
        assert_eq!(cancel.header.ty, FrameType::Cancel);
        inner.retire("test_done");
        drop(stream);
    }

    #[tokio::test]
    async fn an_abandoned_control_open_releases_a_late_bound_route() {
        // `open_route` can write the request before its caller drops or times out.
        // `open_route` responses can arrive after the pending entry is removed.
        // A late route-bind response without a pending entry strands the binding unless the client sends `Goodbye`.
        // The client never learns the route handle, so it cannot send a route `Goodbye`.
        // Each repeated abandonment consumes a host-side route and channel permit.
        // The client must send a best-effort route `Goodbye` for late binds it cannot own.
        // The client closes the connection only when route `Goodbye` cannot be queued.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let control = RouteHandle {
            channel: 0,
            epoch: 0,
        };
        let (tx, _rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                control,
                Vec::new(),
                false,
                PendingKind::Unary(tx),
                Instant::now() + Duration::from_secs(60),
            )
            .expect("control request admitted");
        // The host answers only a request it received, so the writer claimed it.
        // The writer's claim makes the abandonment `OutcomeUnknown`.
        // Identity 0/0 has no legal `Cancel`.
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);

        let removal = inner
            .cancel_key(key, "caller_dropped")
            .expect("abandoning a control request never fails");
        assert!(matches!(removal, PendingRemoval::Cancelled));
        assert!(
            control_rx.try_recv().is_err(),
            "identity 0/0 has no legal Cancel, so abandoning emits no control frame"
        );

        let bound = RouteHandle {
            channel: 9,
            epoch: 3,
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "op": "route.open",
            "route_channel": bound.channel,
            "route_epoch": bound.epoch,
        }))
        .expect("body encodes");
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits a frame length"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, false),
                channel: control.channel,
                epoch: control.epoch,
                corr: key.corr,
            },
            body,
            ByteCharge::none(),
        );

        let goodbye = control_rx
            .try_recv()
            .expect("a stranded bind is released with a route Goodbye");
        assert_eq!(goodbye.header.ty, FrameType::Goodbye);
        let header = goodbye.header;
        assert_eq!(header.channel, bound.channel, "the exact stranded channel");
        assert_eq!(header.epoch, bound.epoch, "the exact stranded epoch");
        assert_eq!(header.corr, 0, "a route Goodbye carries correlation 0");
        assert!(
            !inner.retired.load(Ordering::Acquire),
            "reclaiming one route must not take unrelated routes with it"
        );
        assert!(
            !lock_unpoisoned(&inner.routes).contains(&bound),
            "a late bind never enters the client cache"
        );
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn a_bind_delivered_to_a_caller_dropped_before_polling_is_released() {
        // `tx.send` succeeds while the receiver is alive; the caller can still be dropped before it polls the value.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let control = RouteHandle {
            channel: 0,
            epoch: 0,
        };
        let (tx, rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                control,
                Vec::new(),
                false,
                PendingKind::Unary(tx),
                Instant::now() + Duration::from_secs(60),
            )
            .expect("control request admitted");
        let guard = UnaryAdmissionGuard::new(Arc::clone(&inner), key, rx);
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);

        let bound = RouteHandle {
            channel: 9,
            epoch: 3,
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "op": "route.open",
            "route_channel": bound.channel,
            "route_epoch": bound.epoch,
        }))
        .expect("body encodes");
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits a frame length"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, false),
                channel: control.channel,
                epoch: control.epoch,
                corr: key.corr,
            },
            body,
            ByteCharge::none(),
        );
        assert!(
            control_rx.try_recv().is_err(),
            "a delivered bind is not released while its receiver is alive"
        );

        drop(guard);
        let goodbye = control_rx
            .try_recv()
            .expect("the dropped caller releases the bind it never claimed");
        assert_eq!(goodbye.header.ty, FrameType::Goodbye);
        assert_eq!(goodbye.header.channel, bound.channel);
        assert_eq!(goodbye.header.epoch, bound.epoch);
        assert!(!inner.retired.load(Ordering::Acquire));
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn a_bind_answered_to_a_dropped_receiver_is_released() {
        // `dispatch` removes the pending entry before it sends the terminal, so a receiver dropped in that window bypasses the absent-entry path.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let control = RouteHandle {
            channel: 0,
            epoch: 0,
        };
        let (tx, rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                control,
                Vec::new(),
                false,
                PendingKind::Unary(tx),
                Instant::now() + Duration::from_secs(60),
            )
            .expect("control request admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        drop(rx);

        let bound = RouteHandle {
            channel: 9,
            epoch: 3,
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "op": "route.open",
            "route_channel": bound.channel,
            "route_epoch": bound.epoch,
        }))
        .expect("body encodes");
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits a frame length"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, false),
                channel: control.channel,
                epoch: control.epoch,
                corr: key.corr,
            },
            body,
            ByteCharge::none(),
        );

        let goodbye = control_rx
            .try_recv()
            .expect("a bind nobody can receive is released with a route Goodbye");
        assert_eq!(goodbye.header.ty, FrameType::Goodbye);
        assert_eq!(goodbye.header.channel, bound.channel);
        assert_eq!(goodbye.header.epoch, bound.epoch);
        assert!(
            lock_unpoisoned(&inner.pending).is_empty(),
            "the pending entry was consumed by the terminal"
        );
        assert!(!inner.retired.load(Ordering::Acquire));
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn a_duplicate_bind_terminal_never_closes_an_owned_route() {
        // An unmatched control `Response` can represent a stranded route binding.
        // Only an unmatched route-bind response can indicate a stranded binding; duplicate terminals for delivered routes must not trigger cleanup.
        // `inner.routes` distinguishes stranded binds from routes the caller owns; only stranded binds receive `Goodbye`.
        let (inner, _data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let owned = route(1);
        assert!(
            lock_unpoisoned(&inner.routes).contains(&owned),
            "the fixture owns this route"
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "op": "route.open",
            "route_channel": owned.channel,
            "route_epoch": owned.epoch,
        }))
        .expect("body encodes");
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits a frame length"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, false),
                channel: 0,
                epoch: 0,
                corr: FIRST_APPLICATION_CORRELATION,
            },
            body,
            ByteCharge::none(),
        );

        assert!(
            control_rx.try_recv().is_err(),
            "an owned route is never released by a duplicate bind terminal"
        );
        assert!(
            lock_unpoisoned(&inner.routes).contains(&owned),
            "the owned route stays live"
        );
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn token_cancelling_a_stream_leaves_its_queued_items_reachable() {
        // `cancel_key` cannot reach the caller-held receiver, so queued items remain charged to the owner-wide retained budget.
        // `cancel_key` leaves `finished` false, so `next` can drain queued items.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (items_tx, items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);
        let mut stream = ResponseStream {
            inner: Arc::downgrade(&inner),
            key,
            correlation: key.corr,
            items: items_rx,
            terminal: Some(terminal_rx),
            finished: false,
        };

        const ITEMS: usize = 4;
        for _ in 0..ITEMS {
            inner.dispatch(
                EnvelopeHeader {
                    len: 8,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::StreamData,
                    flags: response_flags(false, false),
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                vec![7; 8],
                inner.retained_budget.charge(8).expect("retained bytes"),
            );
        }
        assert_eq!(inner.retained_budget.used(), ITEMS * 8);

        let _ = inner.cancel_key(key, "cancelled");
        assert!(
            !stream.finished,
            "a watcher cancellation does not short-circuit the consumer"
        );

        // Every queued item is still delivered, in order, before the terminal.
        let mut drained = 0;
        loop {
            match stream.next().await {
                Ok(Some(item)) => {
                    assert_eq!(item.body, vec![7; 8]);
                    drained += 1;
                }
                Ok(None) => panic!("a cancelled stream reports its cancellation"),
                Err(error) => {
                    assert_eq!(error.code(), "cancelled");
                    break;
                }
            }
        }
        assert_eq!(drained, ITEMS, "the queued items survive the cancellation");
        assert_eq!(
            inner.retained_budget.used(),
            0,
            "draining the cancelled stream releases every charge"
        );
        inner.retire("test_done");
        drop(stream);
    }

    #[tokio::test]
    async fn dropping_a_token_cancelled_stream_releases_its_queued_charges() {
        // Dropping an unpolled watcher-cancelled stream releases its retained bytes.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        lock_unpoisoned(&inner.routes).insert(route(1));
        let (items_tx, items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        assert!(claim_for_write(&publish), "the writer claimed the request");
        drop(data_rx.recv().await);
        let stream = ResponseStream {
            inner: Arc::downgrade(&inner),
            key,
            correlation: key.corr,
            items: items_rx,
            terminal: Some(terminal_rx),
            finished: false,
        };

        const ITEMS: usize = 4;
        for _ in 0..ITEMS {
            inner.dispatch(
                EnvelopeHeader {
                    len: 8,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::StreamData,
                    flags: response_flags(false, false),
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                vec![7; 8],
                inner.retained_budget.charge(8).expect("retained bytes"),
            );
        }
        let _ = inner.cancel_key(key, "cancelled");
        assert_eq!(inner.retained_budget.used(), ITEMS * 8);

        drop(stream);
        assert_eq!(
            inner.retained_budget.used(),
            0,
            "dropping the consumer releases every queued charge"
        );
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn retained_stream_bytes_never_deny_a_maximum_sized_frame() {
        // Admitted connections must accept every otherwise-valid frame under the wire contract.
        // A shared pool can make an unrelated maximum-sized terminal unreadable when another consumer retains queued bytes.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (items_tx, _items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, _terminal_rx) = oneshot::channel();
        let (key, _publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        drop(data_rx.recv().await);

        let queued = 2 * 1024 * 1024;
        let charge = inner._read_budget.charge(queued).expect("read reservation");
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(queued).expect("fits a frame length"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::StreamData,
                flags: response_flags(true, false),
                channel: key.channel,
                epoch: key.epoch,
                corr: key.corr,
            },
            vec![0; queued],
            charge,
        );

        assert_eq!(
            inner.retained_budget.used(),
            queued,
            "a queued item is accounted against retention, not the read reservation"
        );
        assert_eq!(
            inner._read_budget.used(),
            0,
            "the read reservation is released once the bytes are retained"
        );
        assert!(
            inner._read_budget.charge(MAX_BODY_LEN as usize).is_some(),
            "queued bytes must not deny the reader a maximum-sized frame"
        );
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn exhausted_retention_cancels_only_the_saturating_stream() {
        // Retention exhaustion is local to the affected stream, like item-queue overflow.
        // When retention is exhausted, `dispatch` cancels only the affected stream.
        // The stream cancellation preserves the generation and its unrelated routes.
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (items_tx, _items_rx) = mpsc::channel(CLIENT_STREAM_QUEUE_ITEMS);
        let (terminal_tx, terminal_rx) = oneshot::channel();
        let (key, _publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                PendingKind::Stream {
                    items: items_tx,
                    terminal: terminal_tx,
                    _settled: CancellationToken::new().drop_guard(),
                },
                Instant::now() + Duration::from_secs(60),
            )
            .expect("stream admitted");
        drop(data_rx.recv().await);

        let hold = inner
            .retained_budget
            .charge(CLIENT_RETAINED_RESPONSE_BYTES)
            .expect("retention fully held by an existing consumer");
        let charge = inner._read_budget.charge(1).expect("read reservation");
        inner.dispatch(
            EnvelopeHeader {
                len: 1,
                ver: PROTOCOL_VERSION,
                ty: FrameType::StreamData,
                flags: response_flags(false, false),
                channel: key.channel,
                epoch: key.epoch,
                corr: key.corr,
            },
            vec![7],
            charge,
        );

        let error = terminal_rx
            .await
            .expect("terminal sender")
            .expect_err("the stream that could not retain its item fails");
        assert_eq!(error.code(), "stream_saturated");
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
        let cancel = control_rx.recv().await.expect("stream Cancel");
        assert_eq!(cancel.header.ty, FrameType::Cancel);
        assert!(
            !inner.retired.load(Ordering::Acquire),
            "a saturated consumer must not retire the generation"
        );
        assert_eq!(
            inner._read_budget.used(),
            0,
            "the discarded item releases the read reservation"
        );
        drop(hold);
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn a_malformed_route_open_success_retires_the_generation() {
        // The host binds routes whose success bodies omit the channel and epoch, so clients cannot send those routes `Goodbye`.
        // Keeping the connection live lets repeated opens strand host-side routes and channel permits.
        // Retiring the connection obliges the host to settle stranded routes and channel permits.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let client = Client {
            inner: Arc::clone(&inner),
        };
        let open = tokio::spawn(async move {
            client
                .open_route(
                    RouteTarget {
                        kind: TargetKind::ManagementSurface,
                        module_id: "context".to_owned(),
                    },
                    identity_fixture(),
                )
                .await
        });

        let frame = data_rx.recv().await.expect("route.open request");
        let header = frame.header;
        inner.dispatch(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Response,
                flags: response_flags(false, true),
                channel: 0,
                epoch: 0,
                corr: header.corr,
            },
            br#"{"ok":true}"#.to_vec(),
            ByteCharge::none(),
        );

        let error = open
            .await
            .expect("open task")
            .expect_err("a success body without a route is not a route");
        assert_eq!(error.code(), "invalid_route_response");
        assert!(
            inner.retired.load(Ordering::Acquire),
            "an unnameable binding must not be left on a live generation"
        );
        assert!(
            lock_unpoisoned(&inner.routes).is_empty(),
            "retirement drops the generation's routes"
        );
    }

    #[test]
    fn queue_and_retained_charges_release_exactly() {
        let budget = Arc::new(ByteCounter::new(10));
        let first = budget.charge(7).expect("first charge");
        assert!(budget.charge(4).is_none());
        assert_eq!(budget.used(), 7);
        drop(first);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn epoch_is_part_of_pending_key() {
        let old = PendingKey::new(
            RouteHandle {
                channel: 7,
                epoch: 1,
            },
            9,
        );
        let current = PendingKey::new(
            RouteHandle {
                channel: 7,
                epoch: 2,
            },
            9,
        );
        assert_ne!(old, current);
    }

    #[test]
    fn terminal_formatting_redacts_peer_message_and_body() {
        let sentinel = "CANARY-CREDENTIAL-PAYLOAD-93ff";
        let body = serde_json::to_vec(&serde_json::json!({
            "code": "stable_code",
            "message": sentinel
        }))
        .expect("serialize");
        let error = CallError::host_terminal(&body).expect("canonical error body");
        let rendered = format!("{error:?} {error}");
        assert_eq!(error.outcome(), SendOutcome::Terminal);
        assert_eq!(error.code(), "host.stable_code");
        assert!(!rendered.contains(sentinel));
    }

    #[test]
    fn a_nonconforming_remote_code_never_aliases_a_reserved_one() {
        assert_eq!(bounded_code("unknown_module"), "unknown_module");
        assert_eq!(bounded_code("a.b-c_1"), "a.b-c_1");
        assert_eq!(bounded_code("unknown_module!"), "remote_error");
        assert_eq!(bounded_code("unknown module"), "remote_error");
        assert_eq!(bounded_code(""), "remote_error");
        assert_eq!(
            bounded_code(&"x".repeat(MAX_ERROR_CODE_BYTES + 1)),
            "remote_error"
        );
        assert_eq!(
            bounded_code(&"x".repeat(MAX_ERROR_CODE_BYTES)).len(),
            MAX_ERROR_CODE_BYTES
        );
    }

    #[tokio::test]
    async fn a_failed_bridge_handoff_settles_not_sent() {
        // The bridge receiver is gone before the writer hands over a claimed frame, so nothing reached the ring.
        let (inner, data_rx, control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, rx) = unary_sender();
        inner
            .admit(
                route(1),
                b"never-handed-over".to_vec(),
                false,
                kind,
                Instant::now() + Duration::from_secs(5),
            )
            .expect("admitted");
        let (write, writes) = fake_ring_writer();
        drop(writes);
        let writer_inner = Arc::clone(&inner);
        tokio::spawn(async move {
            writer_loop(writer_inner, write, data_rx, control_rx).await;
        })
        .await
        .expect("writer exits after retiring");
        let error = rx.await.expect("settled").expect_err("retired");
        assert_eq!(error.outcome(), SendOutcome::NotSent);
        assert_eq!(error.code(), "write_failed");
    }

    #[tokio::test]
    async fn closing_under_admission_blocks_a_late_enqueue() {
        // `admit` checks `closed` while holding `admission`; `mark_closed` takes the same lock, so no frame is queued after `closed` flips.
        let (inner, data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let admission = lock_unpoisoned(&inner.admission);
        let closer = std::thread::spawn({
            let inner = Arc::clone(&inner);
            move || inner.mark_closed(|_| ())
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !inner.closed.load(Ordering::Acquire),
            "closed waits for admission"
        );
        drop(admission);
        assert!(!closer.join().expect("closer completes"), "first close");
        let (kind, _rx) = unary_sender();
        let error = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(5),
            )
            .expect_err("closed generation admits nothing");
        assert_eq!(error.code(), "connection_retired");
        assert_eq!(data_rx.len(), 0);
    }

    #[test]
    fn only_a_canonical_error_body_parses() {
        for body in [
            &b"not json"[..],
            br#"{"message":"m"}"#,
            br#"{"code":"c"}"#,
            br#"{"code":1,"message":"m"}"#,
            br#"{"code":"c","message":null}"#,
            br#"["code","message"]"#,
        ] {
            assert!(
                CallError::host_terminal(body).is_none(),
                "{} must be rejected",
                String::from_utf8_lossy(body)
            );
        }
        let error = CallError::host_terminal(
            br#"{"code":"c","message":"m","retry_after_ms":5,"extra":true}"#,
        )
        .expect("unknown members are permitted");
        assert_eq!(error.code(), "host.c");
    }

    #[tokio::test]
    async fn a_malformed_error_body_retires_the_generation() {
        // §6.2: a structurally illegal body closes the generation.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(
                route(1),
                Vec::new(),
                false,
                kind,
                Instant::now() + Duration::from_secs(5),
            )
            .expect("admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        let body = b"{\"message\":\"no code\"}".to_vec();
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Error,
                flags: response_flags(false, false),
                channel: key.channel,
                epoch: key.epoch,
                corr: key.corr,
            },
            body,
            ByteCharge::none(),
        );
        assert!(inner.retired.load(Ordering::Acquire));
        let error = rx.await.expect("settled").expect_err("retired");
        assert_eq!(error.code(), "protocol_violation");
        assert_eq!(error.outcome(), SendOutcome::OutcomeUnknown);
    }

    #[tokio::test]
    async fn a_malformed_error_for_an_unmatched_correlation_still_retires() {
        // Structural validation precedes the stale-terminal drop (§6.2).
        let (inner, _data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let body = b"garbage".to_vec();
        inner.dispatch(
            EnvelopeHeader {
                len: u32::try_from(body.len()).expect("fits"),
                ver: PROTOCOL_VERSION,
                ty: FrameType::Error,
                flags: response_flags(false, false),
                channel: 7,
                epoch: 1,
                corr: 99,
            },
            body,
            ByteCharge::none(),
        );
        assert!(inner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn an_unconsumed_unary_response_holds_a_retained_charge() {
        // A caller that never polls cannot hold bytes past `CLIENT_RETAINED_RESPONSE_BYTES`.
        let (inner, mut data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(5);
        let deliver = |key: PendingKey, body: Vec<u8>| {
            inner.dispatch(
                EnvelopeHeader {
                    len: u32::try_from(body.len()).expect("fits"),
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::Response,
                    flags: response_flags(false, false),
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                body,
                ByteCharge::none(),
            );
        };
        let (kind, first_rx) = unary_sender();
        let (first, publish) = inner
            .admit(route(1), Vec::new(), false, kind, deadline)
            .expect("admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        deliver(first, vec![0u8; MAX_BODY_LEN as usize]);
        assert_eq!(inner.unary_budget.used(), MAX_BODY_LEN as usize);

        // The remaining 1 MiB admits a smaller response but not a second maximum one.
        let (kind, second_rx) = unary_sender();
        let (second, publish) = inner
            .admit(route(1), Vec::new(), false, kind, deadline)
            .expect("admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        deliver(second, vec![0u8; MAX_BODY_LEN as usize]);
        let error = second_rx
            .await
            .expect("settled")
            .expect_err("retention is exhausted");
        assert_eq!(error.code(), "response_retention_exhausted");
        assert_eq!(
            error.outcome(),
            SendOutcome::Terminal,
            "the reader observed the host terminal"
        );
        assert!(
            !inner.retired.load(Ordering::Acquire),
            "only the saturating request fails"
        );

        // Consuming the first response returns its bytes.
        let retained = first_rx.await.expect("settled").expect("delivered");
        assert_eq!(retained.response.body.len(), MAX_BODY_LEN as usize);
        drop(retained);
        assert_eq!(inner.unary_budget.used(), 0);
        inner.retire("test_done");
    }

    #[tokio::test]
    async fn a_bind_that_cannot_be_retained_is_released() {
        let (inner, mut data_rx, mut control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let deadline = Instant::now() + Duration::from_secs(5);
        let deliver = |key: PendingKey, body: Vec<u8>| {
            inner.dispatch(
                EnvelopeHeader {
                    len: u32::try_from(body.len()).expect("fits"),
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::Response,
                    flags: response_flags(false, false),
                    channel: key.channel,
                    epoch: key.epoch,
                    corr: key.corr,
                },
                body,
                ByteCharge::none(),
            );
        };
        // One unpolled maximum-sized response fills retention.
        let (kind, _filler_rx) = unary_sender();
        let (filler, publish) = inner
            .admit(route(1), Vec::new(), false, kind, deadline)
            .expect("admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        deliver(filler, vec![0u8; CLIENT_RETAINED_RESPONSE_BYTES]);
        assert_eq!(inner.unary_budget.used(), CLIENT_RETAINED_RESPONSE_BYTES);

        let control = RouteHandle {
            channel: 0,
            epoch: 0,
        };
        let (kind, rx) = unary_sender();
        let (key, publish) = inner
            .admit(control, Vec::new(), false, kind, deadline)
            .expect("control request admitted");
        assert!(claim_for_write(&publish));
        drop(data_rx.recv().await);
        let bound = RouteHandle {
            channel: 9,
            epoch: 3,
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "op": "route.open",
            "route_channel": bound.channel,
            "route_epoch": bound.epoch,
        }))
        .expect("body encodes");
        deliver(key, body);

        let error = rx
            .await
            .expect("settled")
            .expect_err("retention is exhausted");
        assert_eq!(error.code(), "response_retention_exhausted");
        assert_eq!(error.outcome(), SendOutcome::Terminal);
        let goodbye = control_rx
            .try_recv()
            .expect("the bind nobody can receive is released");
        assert_eq!(goodbye.header.ty, FrameType::Goodbye);
        assert_eq!(goodbye.header.channel, bound.channel);
        assert_eq!(goodbye.header.epoch, bound.epoch);
        inner.retire("test_done");
    }

    #[test]
    fn data_and_control_queues_share_one_queued_byte_ceiling() {
        assert_eq!(
            CLIENT_DATA_QUEUED_BYTES + CLIENT_CONTROL_QUEUED_BYTES,
            CLIENT_QUEUED_BYTES
        );
    }

    #[test]
    fn retire_sets_closed_under_the_routes_lock() {
        // `open_route` checks `closed` and inserts while holding `routes`.
        // Holding `routes` here must block `retire` from advancing past `closed` until the insert is visible to it.
        let (inner, _data_rx, _control_rx) = test_inner(CLIENT_QUEUED_BYTES);
        let routes = lock_unpoisoned(&inner.routes);
        let retiring = std::thread::spawn({
            let inner = Arc::clone(&inner);
            move || inner.retire("test_race")
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !inner.closed.load(Ordering::Acquire),
            "closed must not flip while an opener holds the route lock"
        );
        drop(routes);
        retiring.join().expect("retire completes");
        assert!(inner.closed.load(Ordering::Acquire));
        assert!(lock_unpoisoned(&inner.routes).is_empty());
    }

    #[test]
    fn outcome_spellings_are_exact() {
        assert_eq!(SendOutcome::NotSent.as_str(), "not_sent");
        assert_eq!(SendOutcome::OutcomeUnknown.as_str(), "outcome_unknown");
        assert_eq!(SendOutcome::Terminal.as_str(), "terminal");
    }

    #[tokio::test]
    async fn ring_bridge_drains_inbound_and_queued_writes() {
        let rings = shm_transport::backend::ring::DuplexRing::create(
            &crate::ring_transport::ring_profile(),
        )
        .expect("duplex ring");
        let (descriptor, descriptors) =
            crate::ring_transport::worker_descriptor(&rings).expect("descriptor");
        let (client_end, _host_end) = StdUnixStream::pair().expect("socket pair");
        let RingBridge {
            write,
            read: mut read_rx,
            setup,
            thread: _,
        } = start_ring_bridge(
            descriptor,
            descriptors,
            CancellationToken::new(),
            Arc::new(ByteCounter::new(CLIENT_INBOUND_FRAME_BYTES)),
            Instant::now() + CLIENT_HANDSHAKE_TIMEOUT,
        )
        .await
        .expect("bridge");
        setup
            .send(client_end)
            .expect("bridge awaits the setup socket");

        let outbound = EnvelopeHeader {
            len: 0,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: pure_header_flags(),
            channel: 1,
            epoch: 1,
            corr: 1,
        };
        let mut completions = Vec::new();
        for _ in 0..8 {
            let (completed, rx) = oneshot::channel();
            write
                .tx
                .try_send(RingWrite {
                    header: outbound,
                    body: Vec::new(),
                    completed,
                    deadline: StdInstant::now() + Duration::from_secs(1),
                })
                .expect("queue write without waking worker");
            completions.push(rx);
        }
        rings
            .first
            .try_reserve(
                0,
                EnvelopeHeader {
                    len: 0,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::Response,
                    flags: response_flags(false, true),
                    channel: 1,
                    epoch: 1,
                    corr: 1,
                }
                .encode(),
            )
            .expect("reserve inbound")
            .commit(0)
            .expect("publish inbound");
        signal_eventfd(&write.wake);

        let frame = tokio::time::timeout(Duration::from_millis(250), read_rx.recv())
            .await
            .expect("inbound frame starved behind queued writes")
            .expect("bridge closed");
        assert_eq!(frame.0.ty, FrameType::Response);
        for completion in completions {
            tokio::time::timeout(Duration::from_millis(250), completion)
                .await
                .expect("queued write stranded after eventfd drain")
                .expect("bridge dropped write completion")
                .expect("queued write failed");
        }
    }

    #[tokio::test]
    async fn ring_bridge_retires_when_host_drops_setup_socket() {
        let rings = shm_transport::backend::ring::DuplexRing::create(
            &crate::ring_transport::ring_profile(),
        )
        .expect("duplex ring");
        let (descriptor, descriptors) =
            crate::ring_transport::worker_descriptor(&rings).expect("descriptor");
        let (client_end, host_end) = StdUnixStream::pair().expect("socket pair");
        let RingBridge {
            write: _write_tx,
            read: mut read_rx,
            setup,
            thread: _,
        } = start_ring_bridge(
            descriptor,
            descriptors,
            CancellationToken::new(),
            Arc::new(ByteCounter::new(CLIENT_INBOUND_FRAME_BYTES)),
            Instant::now() + CLIENT_HANDSHAKE_TIMEOUT,
        )
        .await
        .expect("bridge");
        setup
            .send(client_end)
            .expect("bridge awaits the setup socket");
        drop(host_end);
        // A hang here means the bridge never observed the dead setup socket.
        assert!(read_rx.recv().await.is_none());
    }
}
