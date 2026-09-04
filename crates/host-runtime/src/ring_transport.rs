//! Mandatory shared-memory ring transport.
//!
//! One dedicated OS thread creates and owns both `!Send` ring endpoints. Host
//! tasks exchange frame tickets and completion notifications with that thread.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};
use std::{fmt, io};

use crate::setup_socket::RING_DESCRIPTOR_COUNT;
use crate::wire::{EnvelopeHeader, FrameType, decode_header};
use shm_transport::backend::ring::RingGrant;
use shm_transport::backend::ring::{DuplexRing, ProducerReservation, Ring};
use shm_transport::profile::{
    AdmissionController, HostLimits as ShmHostLimits, ResourceCharges, TargetProfile,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::frame_channel::{
    DirectFrame, InboundEvent, InboundFrame, OutboundFrame, ReadClose, RejectedFrame, SenderQueue,
    frame_sender, validate_inbound_header,
};
use crate::wire::{ByteBudget, MAX_CONTROL_BODY_LEN};

/// Current ring profile accepted by every process in one release.
pub const RING_PROFILE: &str = shm_transport::profile::HOST_TEST_RING_PROFILE;

/// Test-only observer invoked after each successful frame publication with
/// the published frame's type and channel. It receives no descriptors,
/// payloads, or provider data.
#[doc(hidden)]
pub type PublishHook = Arc<dyn Fn(FrameType, u16) + Send + Sync>;

pub fn ring_profile() -> TargetProfile {
    shm_transport::profile::host_test_ring_profile().expect("static shared-memory profile is valid")
}

/// Admission limits sufficient for one connection.
pub fn per_connection_limits() -> ShmHostLimits {
    let charges = ring_profile().charges();
    ShmHostLimits {
        descriptors: charges.descriptors,
        arena_bytes: charges.arena_bytes,
        leases: charges.leases,
        mappings: charges.mappings,
        file_descriptors: charges.file_descriptors,
        workers: charges.workers,
        client_instances: charges.client_instances,
        pinned_workers: charges.pinned_workers,
    }
}

/// Ceiling on sparse ring virtual arena bytes this process admits at once.
pub const MAX_RING_RESIDENT_BYTES: u64 = 1 << 30;

pub fn affordable_connections() -> u64 {
    MAX_RING_RESIDENT_BYTES
        .checked_div(per_connection_limits().arena_bytes)
        .unwrap_or(1)
        .max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLimitsError {
    /// Requested connections exceed the aggregate arena limit. Callers must
    /// admit no more than `affordable` connections.
    ExceedsResidentBytes {
        requested: u64,
        affordable: u64,
    },
    ChargeOverflow,
}

impl fmt::Display for ProcessLimitsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExceedsResidentBytes {
                requested,
                affordable,
            } => write!(
                formatter,
                "{requested} shared-memory connections exceed the {affordable} affordable under \
                 {MAX_RING_RESIDENT_BYTES} resident arena bytes"
            ),
            Self::ChargeOverflow => formatter.write_str("shared-memory resource limits overflow"),
        }
    }
}

impl std::error::Error for ProcessLimitsError {}

/// Returns an error when `connections` exceeds [`affordable_connections`] so
/// the connection gate and ring admission use the same limit.
pub fn process_limits(connections: usize) -> Result<ShmHostLimits, ProcessLimitsError> {
    let one = per_connection_limits();
    let affordable = affordable_connections();
    let requested = u64::try_from(connections)
        .map_err(|_| ProcessLimitsError::ChargeOverflow)?
        .max(1);
    if requested > affordable {
        return Err(ProcessLimitsError::ExceedsResidentBytes {
            requested,
            affordable,
        });
    }
    let scale = |charge: u64| {
        charge
            .checked_mul(requested)
            .ok_or(ProcessLimitsError::ChargeOverflow)
    };
    Ok(ShmHostLimits {
        descriptors: scale(one.descriptors)?,
        arena_bytes: scale(one.arena_bytes)?,
        leases: scale(one.leases)?,
        mappings: scale(one.mappings)?,
        file_descriptors: scale(one.file_descriptors)?,
        workers: scale(one.workers)?,
        client_instances: scale(one.client_instances)?,
        pinned_workers: scale(one.pinned_workers)?,
    })
}

/// Process-wide owner of ring admission and endpoint creation.
pub struct RingTransport {
    profile: Arc<TargetProfile>,
    admission: Arc<AdmissionController>,
    limits: ShmHostLimits,
    activations: AtomicU64,
    peer_deaths: AtomicU64,
    reclamations: AtomicU64,
    exhaustions: AtomicU64,
    /// Shared with each endpoint thread so a caught panic is counted after
    /// the thread has already left `run_endpoint`.
    endpoint_panics: Arc<AtomicU64>,
    publish_hook: Mutex<Option<PublishHook>>,
}

pub(crate) struct PreparedRing {
    pub(crate) descriptor: serde_json::Value,
    pub(crate) descriptors: [OwnedFd; RING_DESCRIPTOR_COUNT],
    pub(crate) sender: crate::frame_channel::FrameSender,
    pub(crate) receiver: ShmReceiver,
    pub(crate) io: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    pub(crate) root: CancellationToken,
    pub(crate) read_cancel: CancellationToken,
}

#[derive(Debug, Clone, Copy)]
pub struct RingUnavailable;

impl std::fmt::Display for RingUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("shared-memory ring is unavailable")
    }
}

impl std::error::Error for RingUnavailable {}

impl RingTransport {
    /// Builds the process-wide transport with finite admission limits.
    pub fn for_ring_profile(limits: ShmHostLimits) -> Self {
        let profile = Arc::new(ring_profile());
        let admission = Arc::new(AdmissionController::new(limits));
        Self {
            profile,
            admission,
            limits,
            activations: AtomicU64::new(0),
            peer_deaths: AtomicU64::new(0),
            reclamations: AtomicU64::new(0),
            exhaustions: AtomicU64::new(0),
            endpoint_panics: Arc::new(AtomicU64::new(0)),
            publish_hook: Mutex::new(None),
        }
    }

    /// Returns redacted aggregate admission accounting.
    pub fn accounting(
        &self,
    ) -> Result<shm_transport::profile::AccountingSnapshot, shm_transport::profile::AdmissionError>
    {
        self.admission.snapshot()
    }

    /// Bounded, aggregate-only state for authenticated doctor output.
    pub fn diagnostics(&self) -> serde_json::Value {
        let charges = |value: ResourceCharges| {
            serde_json::json!({
                "descriptors": value.descriptors,
                "arena_bytes": value.arena_bytes,
                "leases": value.leases,
                "mappings": value.mappings,
                "file_descriptors": value.file_descriptors,
                "workers": value.workers,
                "client_instances": value.client_instances,
                "pinned_workers": value.pinned_workers,
            })
        };
        let limits = serde_json::json!({
            "descriptors": self.limits.descriptors,
            "arena_bytes": self.limits.arena_bytes,
            "leases": self.limits.leases,
            "mappings": self.limits.mappings,
            "file_descriptors": self.limits.file_descriptors,
            "workers": self.limits.workers,
            "client_instances": self.limits.client_instances,
            "pinned_workers": self.limits.pinned_workers,
        });
        let (state, error_class, accounting) = match self.accounting() {
            Ok(accounting) => (
                "healthy",
                serde_json::Value::Null,
                serde_json::json!({
                    "active": charges(accounting.active),
                    "quarantined": charges(accounting.quarantined),
                }),
            ),
            Err(_) => (
                "terminal",
                serde_json::Value::String("setup_failure".to_owned()),
                serde_json::Value::Null,
            ),
        };
        serde_json::json!({
            "state": state,
            "error_class": error_class,
            "artifact": {
                "profile": RING_PROFILE,
                "wire_version": crate::wire::PROTOCOL_VERSION,
                "descriptor_schema": shm_transport::descriptor::DESCRIPTOR_SCHEMA_VERSION,
            },
            "bounds": limits,
            "accounting": accounting,
            "activation": {"completed": self.activations.load(Ordering::Acquire)},
            "peer_death": {"observed": self.peer_deaths.load(Ordering::Acquire)},
            "reclamation": {"completed": self.reclamations.load(Ordering::Acquire)},
            "exhaustion": {"observed": self.exhaustions.load(Ordering::Acquire)},
            "endpoint_panic": {"observed": self.endpoint_panics.load(Ordering::Acquire)},
        })
    }

    pub(crate) fn record_activation(&self) {
        self.activations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_peer_death(&self) {
        self.peer_deaths.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reclamation(&self) {
        self.reclamations.fetch_add(1, Ordering::Relaxed);
    }

    /// Test hook: install a publication observer for connections prepared
    /// after this call. The hook runs on the endpoint thread after the ring
    /// commit.
    #[doc(hidden)]
    pub fn set_publish_hook(&self, hook: PublishHook) {
        *self.publish_hook.lock().expect("publish hook lock") = Some(hook);
    }

    pub(crate) fn prepare(
        &self,
        ingress: ByteBudget,
        queue_frames: usize,
        frame_deadline: Duration,
    ) -> Result<PreparedRing, RingUnavailable> {
        let admission = self.admission.admit(&self.profile, None).map_err(|_| {
            self.exhaustions.fetch_add(1, Ordering::Relaxed);
            RingUnavailable
        })?;
        let root = CancellationToken::new();
        let read_cancel = root.child_token();
        let (sender, queue) = frame_sender(queue_frames, root.clone(), frame_deadline);
        // One slot beyond `queue_frames` is reserved for the terminal event, so a fault or cancellation is reported even when the receiver has stopped draining. commentlint: allow(JUDGE)
        let inbound_capacity = queue_frames
            .saturating_add(1)
            .min(tokio::sync::Semaphore::MAX_PERMITS);
        let (inbound_tx, inbound_rx) = mpsc::channel(inbound_capacity);
        let terminal = inbound_tx
            .clone()
            .try_reserve_owned()
            .expect("a fresh inbound channel has a free slot");
        let inbound = Inbound {
            sender: inbound_tx.clone(),
            terminal,
        };
        let (initialized_tx, initialized_rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let profile = Arc::clone(&self.profile);
        let worker_root = root.clone();
        let worker_read_cancel = read_cancel.clone();
        let publish_hook = self.publish_hook.lock().expect("publish hook lock").clone();
        let endpoint_panics = Arc::clone(&self.endpoint_panics);
        let panic_root = root.clone();
        let panic_retired = queue.retired.clone();
        // Held outside `run_endpoint` so a panic there can still deliver an
        // explicit non-clean close instead of a bare channel drop.
        let panic_inbound = inbound_tx;

        let spawned = std::thread::Builder::new()
            .name("host-shm-endpoint".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build();
                let rings = runtime
                    .as_ref()
                    .map_err(|_| RingUnavailable)
                    .and_then(|_| DuplexRing::create(&profile).map_err(|_| RingUnavailable));
                let (runtime, rings) = match (runtime, rings) {
                    (Ok(runtime), Ok(rings)) => (runtime, rings),
                    _ => {
                        let _ = initialized_tx.send(Err(RingUnavailable));
                        return;
                    }
                };
                let transfer = worker_descriptor(&rings);
                let Ok((descriptor, descriptors)) = transfer else {
                    let _ = initialized_tx.send(Err(RingUnavailable));
                    return;
                };
                if initialized_tx.send(Ok((descriptor, descriptors))).is_err() {
                    return;
                }
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(run_endpoint(
                        &rings,
                        queue,
                        inbound,
                        ingress,
                        frame_deadline,
                        worker_root,
                        worker_read_cancel,
                        publish_hook,
                    ))
                }));
                if outcome.is_err() {
                    endpoint_panics.fetch_add(1, Ordering::Relaxed);
                    panic_retired.cancel();
                    panic_root.cancel();
                    let _ = panic_inbound
                        .try_send(Err(ReadClose::Corrupt("shared-memory endpoint panicked")));
                }
                drop(panic_inbound);
                // A quarantined ring may remain mapped by its peer, so its charges move to the quarantined bucket rather than being refunded.
                // A peer that closed its doorbell ends has dropped its attachment, so its ring is reclaimable even though the backend latched quarantine on the closed doorbell. commentlint: allow(JUDGE)
                let quarantined = (rings.first.is_quarantined() || rings.second.is_quarantined())
                    && !peer_released_ring(&rings);
                drop(rings);
                if quarantined {
                    // A failed quarantine drops the consumed `Admission`, which refunds the charges.
                    let _ = admission.quarantine();
                } else {
                    admission.release();
                }
                let _ = done_tx.send(());
            });
        if spawned.is_err() {
            return Err(RingUnavailable);
        }
        let (descriptor, descriptors) = initialized_rx.recv().map_err(|_| RingUnavailable)??;
        let receiver = ShmReceiver {
            inbound: inbound_rx,
        };
        let io = Box::pin(async move {
            let _ = done_rx.await;
        });
        Ok(PreparedRing {
            descriptor,
            descriptors,
            sender,
            receiver,
            io,
            root,
            read_cancel,
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct WireDescriptor {
    profile: String,
    host_to_peer_grant: String,
    peer_to_host_grant: String,
}

pub(crate) fn worker_descriptor(
    rings: &DuplexRing,
) -> Result<(serde_json::Value, [OwnedFd; RING_DESCRIPTOR_COUNT]), ()> {
    let descriptor = WireDescriptor {
        profile: RING_PROFILE.to_owned(),
        host_to_peer_grant: encode_hex(&rings.first.grant().encode()),
        peer_to_host_grant: encode_hex(&rings.second.grant().encode()),
    };
    let [first_mapping, first_data, first_capacity] =
        rings.first.attachment().map_err(|_| ())?.into_parts().0;
    let [second_mapping, second_data, second_capacity] =
        rings.second.attachment().map_err(|_| ())?.into_parts().0;
    let descriptors = [
        first_mapping,
        first_data,
        first_capacity,
        second_mapping,
        second_data,
        second_capacity,
    ];
    Ok((
        serde_json::to_value(descriptor).map_err(|_| ())?,
        descriptors,
    ))
}

/// A stream doorbell reads end-of-file only after its peer end is closed, which is how a peer that exited or dropped its attachment appears to the host. commentlint: allow(JUDGE)
fn peer_released_ring(rings: &DuplexRing) -> bool {
    use std::io::Read;
    let Ok(doorbell) = rings.second.duplicate_data_ready() else {
        return false;
    };
    let mut doorbell = std::os::unix::net::UnixStream::from(doorbell);
    if doorbell.set_nonblocking(true).is_err() {
        return false;
    }
    // The ring is already retired, so consuming a pending wake token here has no reader to starve.
    matches!(doorbell.read(&mut [0u8; 1]), Ok(0))
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

pub(crate) struct ShmReceiver {
    inbound: mpsc::Receiver<Result<InboundEvent, ReadClose>>,
}

type InboundSender = mpsc::Sender<Result<InboundEvent, ReadClose>>;

/// The endpoint's handle on the receiver: ordinary events wait for capacity, the terminal event never does.
struct Inbound {
    sender: InboundSender,
    terminal: mpsc::OwnedPermit<Result<InboundEvent, ReadClose>>,
}

impl Inbound {
    fn close(self, close: ReadClose) {
        self.terminal.send(Err(close));
    }
}

impl ShmReceiver {
    pub(crate) async fn recv(&mut self) -> Result<InboundEvent, ReadClose> {
        self.inbound
            .recv()
            .await
            .unwrap_or(Err(ReadClose::CleanEof))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_endpoint(
    rings: &DuplexRing,
    mut queue: SenderQueue,
    inbound: Inbound,
    ingress: ByteBudget,
    frame_deadline: Duration,
    root: CancellationToken,
    read_cancel: CancellationToken,
    publish_hook: Option<PublishHook>,
) {
    let discard = queue.discard.clone();
    let finish = queue.finish.clone();
    let mut inbound = Some(inbound);
    let readiness = match rings.second.duplicate_data_ready().and_then(|fd| {
        tokio::io::unix::AsyncFd::new(fd)
            .map_err(|_| shm_transport::backend::ring::RingError::ObjectSetupFailed)
    }) {
        Ok(readiness) => readiness,
        Err(_) => {
            fail(
                &mut inbound,
                &mut queue,
                &root,
                ReadClose::Corrupt("shared-memory readiness setup failed"),
            );
            return;
        }
    };
    // One ring depth of receives after `read_cancel` covers every frame committed before it. commentlint: allow(JUDGE)
    let post_cancel_depth =
        usize::try_from(rings.second.grant().geometry().descriptor_depth).unwrap_or(usize::MAX);
    let mut post_cancel_frames: Option<usize> = None;
    let mut finishing = false;
    loop {
        // The loop checks lifecycle tokens before receiving frames so sustained inbound traffic cannot bypass the `select!` below. commentlint: allow(JUDGE)
        if discard.is_cancelled() || root.is_cancelled() {
            return;
        }
        if finish.is_cancelled() {
            finishing = true;
        }
        let mut received = false;
        if let Some(inbound_sender) = inbound.as_ref().map(|inbound| &inbound.sender) {
            let cancelled = read_cancel.is_cancelled();
            let drain_exhausted =
                cancelled && *post_cancel_frames.get_or_insert(post_cancel_depth) == 0;
            let outcome = if drain_exhausted {
                Ok(false)
            } else {
                receive_one(
                    rings,
                    &mut queue,
                    inbound_sender,
                    &ingress,
                    frame_deadline,
                    &root,
                    &read_cancel,
                    publish_hook.as_ref(),
                )
                .await
            };
            match outcome {
                Ok(true) => {
                    received = true;
                    if let Some(remaining) = post_cancel_frames.as_mut() {
                        *remaining = remaining.saturating_sub(1);
                    }
                }
                Ok(false) => {
                    if cancelled && let Some(inbound) = inbound.take() {
                        inbound.close(ReadClose::Cancelled);
                    }
                }
                Err(close) => {
                    fail(&mut inbound, &mut queue, &root, close);
                    return;
                }
            }
        }

        let queued = if received {
            // Directions alternate under sustained inbound traffic: each
            // received frame is followed by at most one queued outbound
            // frame, taken without waiting, so a peer that refills the
            // inbound ring as slots release cannot starve responses, Pings,
            // and close frames while host-to-peer capacity is free.
            queue.try_recv().ok()
        } else if finishing {
            match queue.drain_finished() {
                Some(frame) => Some(frame),
                None => return,
            }
        } else {
            let data_armed = if inbound.is_some() {
                match rings.second.arm_data_wait() {
                    Ok(false) => continue,
                    Ok(true) => true,
                    Err(_) => {
                        fail(
                            &mut inbound,
                            &mut queue,
                            &root,
                            ReadClose::Corrupt("shared-memory data wait failed"),
                        );
                        return;
                    }
                }
            } else {
                false
            };
            tokio::select! {
                biased;
                () = discard.cancelled() => return,
                () = finish.cancelled() => {
                    finishing = true;
                    None
                }
                () = read_cancel.cancelled(), if inbound.is_some() => None,
                frame = queue.recv() => match frame {
                    Some(frame) => Some(frame),
                    None => return,
                },
                ready = readiness.readable(), if data_armed => {
                    let Ok(mut guard) = ready else {
                        fail(
                            &mut inbound,
                            &mut queue,
                            &root,
                            ReadClose::Corrupt("shared-memory readiness failed"),
                        );
                        return;
                    };
                    guard.clear_ready();
                    if rings.second.complete_data_wait().is_err() {
                        fail(
                            &mut inbound,
                            &mut queue,
                            &root,
                            ReadClose::Corrupt("shared-memory data wait failed"),
                        );
                        return;
                    }
                    None
                },
                () = root.cancelled() => return,
            }
        };
        let Some(queued) = queued else {
            continue;
        };
        if publish_one(&rings.first, queued, frame_deadline, publish_hook.as_ref()).is_err() {
            fail(
                &mut inbound,
                &mut queue,
                &root,
                ReadClose::Corrupt("shared-memory publish failed"),
            );
            return;
        }
    }
}

// `ShmReceiver::recv` maps a closed channel to `CleanEof`, so a fault must be sent explicitly before `inbound` drops. commentlint: allow(JUDGE)
fn fail(
    inbound: &mut Option<Inbound>,
    queue: &mut SenderQueue,
    root: &CancellationToken,
    close: ReadClose,
) {
    if let Some(inbound) = inbound.take() {
        inbound.close(close);
    }
    queue.retired.cancel();
    root.cancel();
}

// Teardown must not depend on the receiver draining: a full channel under `discard` or `root` cancellation yields instead of blocking the endpoint. commentlint: allow(JUDGE)
async fn deliver(
    inbound: &InboundSender,
    queue: &SenderQueue,
    root: &CancellationToken,
    event: Result<InboundEvent, ReadClose>,
) -> Result<(), ReadClose> {
    tokio::select! {
        biased;
        sent = inbound.send(event) => sent.map_err(|_| ReadClose::Cancelled),
        () = queue.discard.cancelled() => Err(ReadClose::Cancelled),
        () = root.cancelled() => Err(ReadClose::Cancelled),
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_one(
    rings: &DuplexRing,
    queue: &mut SenderQueue,
    inbound: &InboundSender,
    ingress: &ByteBudget,
    frame_deadline: Duration,
    root: &CancellationToken,
    read_cancel: &CancellationToken,
    publish_hook: Option<&PublishHook>,
) -> Result<bool, ReadClose> {
    let Some(lease) = rings
        .second
        .try_receive()
        .map_err(|_| ReadClose::Corrupt("shared-memory receive failed"))?
    else {
        return Ok(false);
    };
    let header = decode_header(&lease.wire_header())
        .map_err(|_| ReadClose::Corrupt("invalid shared-memory header"))?;
    validate_inbound_header(header)?;
    if header.ty == FrameType::Request && header.channel == 0 && header.len > MAX_CONTROL_BODY_LEN {
        lease
            .release()
            .map_err(|_| ReadClose::Corrupt("shared-memory completion failed"))?;
        deliver(
            inbound,
            queue,
            root,
            Ok(InboundEvent::Rejected(RejectedFrame { corr: header.corr })),
        )
        .await?;
        return Ok(true);
    }

    let deadline = Instant::now() + frame_deadline;
    let discard = queue.discard.clone();
    let charge = ingress.charge(header.len);
    tokio::pin!(charge);
    let charge = loop {
        tokio::select! {
            biased;
            // An available budget charges before the lifecycle arms are polled, so a frame committed before read cancellation still drains; only a frame that must wait for budget yields to cancellation. commentlint: allow(JUDGE)
            charge = &mut charge => break charge,
            // Read cancellation stops only the read side: dropping `lease` discards the frame and `Ok(false)` lets the writer keep draining. commentlint: allow(JUDGE)
            () = read_cancel.cancelled() => return Ok(false),
            // The endpoint loop observes `discard` and exits; the dropped lease discards the frame. commentlint: allow(JUDGE)
            () = discard.cancelled() => return Ok(false),
            () = tokio::time::sleep_until(deadline) => {
                // The peer and transport are healthy; only the ingress budget is
                // saturated. Overloaded retires the generation without branding
                // it corrupt, so the admission charge releases cleanly.
                return Err(ReadClose::Overloaded);
            }
            queued = queue.recv() => match queued {
                Some(queued) => {
                    if publish_one(&rings.first, queued, frame_deadline, publish_hook).is_err() {
                        return Err(ReadClose::Corrupt("shared-memory publish failed"));
                    }
                }
                None => return Err(ReadClose::Cancelled),
            }
        }
    };
    let body = lease
        .to_vec()
        .map_err(|_| ReadClose::Corrupt("shared-memory lease failed"))?;
    lease
        .release()
        .map_err(|_| ReadClose::Corrupt("shared-memory completion failed"))?;
    deliver(
        inbound,
        queue,
        root,
        Ok(InboundEvent::Frame(InboundFrame::owned(
            header, body, charge,
        ))),
    )
    .await?;
    Ok(true)
}

fn publish_one(
    ring: &Ring,
    queued: OutboundFrame,
    frame_deadline: Duration,
    publish_hook: Option<&PublishHook>,
) -> Result<(), ()> {
    let OutboundFrame {
        bytes,
        tail,
        direct,
        charge,
        written,
    } = queued;
    let wire_header: Option<[u8; crate::wire::HEADER_LEN]> = match &direct {
        Some(direct) => Some(direct.header()),
        None => bytes
            .get(..crate::wire::HEADER_LEN)
            .and_then(|header| header.try_into().ok()),
    };
    let deadline = StdInstant::now() + frame_deadline;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match direct {
        Some(direct) => publish_direct(ring, direct, deadline),
        None => publish_owned(ring, &bytes, &tail, deadline),
    }));
    if !matches!(result, Ok(Ok(()))) {
        return Err(());
    }
    if let Some(hook) = publish_hook
        && let Some(header) = wire_header.and_then(|header| decode_header(&header).ok())
    {
        hook(header.ty, header.channel);
    }
    if let Some(written) = written {
        written(Instant::now());
    }
    drop(charge);
    Ok(())
}

fn publish_direct(ring: &Ring, direct: DirectFrame, deadline: StdInstant) -> Result<(), ()> {
    let header = direct.header();
    let body_len = direct.body_len();
    let mut reservation = ring
        .reserve_until(body_len, header, deadline)
        .map_err(|_| ())?;
    let result = crate::panic_boundary::redact_sync(|| {
        let mut writer = ReservationWriter(&mut reservation);
        direct.serialize(&mut writer)
    });
    result.map_err(|_| ())?;
    commit_before(reservation, body_len, deadline)
}

fn publish_owned(ring: &Ring, bytes: &[u8], tail: &[u8], deadline: StdInstant) -> Result<(), ()> {
    let (header, first_body) = bytes.split_at_checked(crate::wire::HEADER_LEN).ok_or(())?;
    let header: [u8; crate::wire::HEADER_LEN] = header.try_into().map_err(|_| ())?;
    let body_len = first_body.len().checked_add(tail.len()).ok_or(())?;
    let mut reservation = ring
        .reserve_until(body_len, header, deadline)
        .map_err(|_| ())?;
    reservation.write(first_body).map_err(|_| ())?;
    reservation.write(tail).map_err(|_| ())?;
    commit_before(reservation, body_len, deadline)
}

// Serialization runs after `reserve_until` returns, so the deadline is re-checked at commit; dropping an uncommitted reservation aborts it. commentlint: allow(JUDGE)
fn commit_before(
    reservation: ProducerReservation<'_>,
    body_len: usize,
    deadline: StdInstant,
) -> Result<(), ()> {
    if StdInstant::now() >= deadline {
        return Err(());
    }
    reservation.commit(body_len).map_err(|_| ())?;
    Ok(())
}

struct ReservationWriter<'reservation, 'ring>(&'reservation mut ProducerReservation<'ring>);

impl io::Write for ReservationWriter<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WriteZero,
                "shared-memory reservation exhausted",
            )
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Thread-confined peer endpoint for integration tests.
pub struct RingClientEndpoint {
    /// Peer-to-host producer direction.
    pub to_host: Ring,
    /// Host-to-peer consumer direction.
    pub from_host: Ring,
}

impl RingClientEndpoint {
    /// Attaches a descriptor and its setup-socket file descriptors.
    pub fn attach_with_descriptors(
        descriptor: &serde_json::Value,
        descriptors: [OwnedFd; RING_DESCRIPTOR_COUNT],
    ) -> Result<Self, RingClientError> {
        let descriptor: WireDescriptor =
            serde_json::from_value(descriptor.clone()).map_err(|_| RingClientError)?;
        if descriptor.profile != RING_PROFILE {
            return Err(RingClientError);
        }
        let [
            from_mapping,
            from_data,
            from_capacity,
            to_mapping,
            to_data,
            to_capacity,
        ] = descriptors;
        let from_host_grant = decode_grant(&descriptor.host_to_peer_grant)?;
        let to_host_grant = decode_grant(&descriptor.peer_to_host_grant)?;
        if from_host_grant.geometry() != to_host_grant.geometry() {
            return Err(RingClientError);
        }
        let from_host = Ring::attach([from_mapping, from_data, from_capacity], from_host_grant)
            .map_err(|_| RingClientError)?;
        let to_host = Ring::attach([to_mapping, to_data, to_capacity], to_host_grant)
            .map_err(|_| RingClientError)?;
        Ok(Self { to_host, from_host })
    }

    /// Publishes one complete consumer frame under the caller's deadline.
    pub fn send(
        &self,
        header: EnvelopeHeader,
        body: &[u8],
        deadline: StdInstant,
    ) -> Result<(), RingClientError> {
        let mut reservation = self
            .to_host
            .reserve_until(body.len(), header.encode(), deadline)
            .map_err(|_| RingClientError)?;
        reservation.write(body).map_err(|_| RingClientError)?;
        reservation
            .commit(body.len())
            .map_err(|_| RingClientError)?;
        Ok(())
    }

    pub fn try_recv(&self) -> Result<Option<(EnvelopeHeader, Vec<u8>)>, RingClientError> {
        self.try_recv_with(|_| Some(()))
            .map(|frame| frame.map(|(header, body, ())| (header, body)))
    }

    pub(crate) fn try_recv_with<T>(
        &self,
        charge: impl FnOnce(usize) -> Option<T>,
    ) -> Result<Option<(EnvelopeHeader, Vec<u8>, T)>, RingClientError> {
        let Some(lease) = self.from_host.try_receive().map_err(|_| RingClientError)? else {
            return Ok(None);
        };
        let header = decode_header(&lease.wire_header()).map_err(|_| RingClientError)?;
        let Some(charge) = charge(lease.len()) else {
            lease.release().map_err(|_| RingClientError)?;
            return Err(RingClientError);
        };
        let body = lease.to_vec().map_err(|_| RingClientError)?;
        lease.release().map_err(|_| RingClientError)?;
        Ok(Some((header, body, charge)))
    }
}

fn decode_grant(grant: &str) -> Result<RingGrant, RingClientError> {
    RingGrant::decode(decode_hex(grant)?).map_err(|_| RingClientError)
}

fn decode_hex<const N: usize>(text: &str) -> Result<[u8; N], RingClientError> {
    let text = text.as_bytes();
    if text.len() != N * 2 {
        return Err(RingClientError);
    }
    fn nibble(byte: u8) -> Result<u8, RingClientError> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(RingClientError),
        }
    }
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = nibble(text[index * 2])? << 4 | nibble(text[index * 2 + 1])?;
    }
    Ok(bytes)
}

/// Redacted test-peer attachment or I/O failure.
#[derive(Clone, Copy)]
pub struct RingClientError;

impl fmt::Debug for RingClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RingClientError(<redacted>)")
    }
}

impl fmt::Display for RingClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("shared-memory peer operation failed")
    }
}

impl std::error::Error for RingClientError {}

#[cfg(test)]
impl RingClientEndpoint {
    /// Returns an error if no frame arrives before `timeout`.
    pub(crate) fn recv(
        &self,
        timeout: Duration,
    ) -> Result<(EnvelopeHeader, Vec<u8>), RingClientError> {
        let deadline = StdInstant::now() + timeout;
        loop {
            if let Some(frame) = self.try_recv()? {
                return Ok(frame);
            }
            if !self
                .from_host
                .wait_for_data(deadline)
                .map_err(|_| RingClientError)?
            {
                return Err(RingClientError);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Flags, PROTOCOL_VERSION, Priority};

    struct TestCharge {
        used: Arc<std::sync::atomic::AtomicUsize>,
        bytes: usize,
    }

    impl Drop for TestCharge {
        fn drop(&mut self) {
            self.used.fetch_sub(self.bytes, Ordering::SeqCst);
        }
    }

    #[test]
    fn process_limits_reject_counts_above_the_resident_byte_ceiling() {
        let affordable = affordable_connections();
        let one = per_connection_limits();
        assert_eq!(
            affordable,
            MAX_RING_RESIDENT_BYTES / one.arena_bytes,
            "affordable count is the arena ceiling divided by one ring's charge"
        );
        let exact = process_limits(usize::try_from(affordable).unwrap()).expect("affordable");
        assert_eq!(exact.arena_bytes, one.arena_bytes * affordable);
        assert_eq!(
            process_limits(usize::try_from(affordable + 1).unwrap()),
            Err(ProcessLimitsError::ExceedsResidentBytes {
                requested: affordable + 1,
                affordable,
            }),
            "one connection above the ceiling is rejected, not clamped"
        );
        assert_eq!(
            process_limits(0).expect("zero rounds up to one ring"),
            process_limits(1).expect("one ring")
        );
    }

    #[test]
    fn shared_memory_workers_have_no_periodic_polling() {
        let endpoint = include_str!("ring_transport.rs");
        let micro_poll = concat!("Duration::from_micros(", "50)");
        assert!(!endpoint.contains(micro_poll));
        assert!(!endpoint.contains(concat!("POLL_", "INTERVAL")));
    }

    #[tokio::test]
    async fn finish_wakes_after_read_cancellation_with_unread_peer_data() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            read_cancel,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);

        read_cancel.cancel();
        assert!(matches!(receiver.recv().await, Err(ReadClose::Cancelled)));
        peer.send(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Goodbye,
                flags: crate::wire::pure_header_flags(),
                channel: 0,
                epoch: 0,
                corr: 0,
            },
            &[],
            StdInstant::now() + Duration::from_secs(1),
        )
        .expect("peer publishes late Goodbye");
        sender.finish();

        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("finished endpoint wakes despite unread peer data")
            .expect("endpoint task joins");
    }

    #[test]
    fn construction_has_no_ring_side_effects() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let accounting = transport.accounting().unwrap();
        assert_eq!(accounting.active, ResourceCharges::ZERO);
        assert_eq!(accounting.quarantined, ResourceCharges::ZERO);
    }

    #[test]
    fn diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts() {
        let limits = per_connection_limits();
        let transport = RingTransport::for_ring_profile(limits);
        transport.record_activation();
        transport.record_peer_death();
        transport.record_reclamation();

        let diagnostics = transport.diagnostics();
        assert_eq!(diagnostics["state"], "healthy");
        assert_eq!(diagnostics["error_class"], serde_json::Value::Null);
        assert_eq!(diagnostics["artifact"]["profile"], RING_PROFILE);
        assert_eq!(
            diagnostics["artifact"]["wire_version"],
            crate::wire::PROTOCOL_VERSION
        );
        assert_eq!(
            diagnostics["artifact"]["descriptor_schema"],
            shm_transport::descriptor::DESCRIPTOR_SCHEMA_VERSION
        );
        assert_eq!(diagnostics["bounds"]["arena_bytes"], limits.arena_bytes);
        assert_eq!(diagnostics["accounting"]["active"]["arena_bytes"], 0);
        assert_eq!(diagnostics["accounting"]["quarantined"]["arena_bytes"], 0);
        assert_eq!(diagnostics["activation"]["completed"], 1);
        assert_eq!(diagnostics["peer_death"]["observed"], 1);
        assert_eq!(diagnostics["reclamation"]["completed"], 1);
        assert_eq!(diagnostics["exhaustion"]["observed"], 0);

        let encoded = diagnostics.to_string();
        for secret_field in [
            "socket_path",
            "native_handle",
            "mapping_descriptor",
            "activation_token",
            "authentication_key",
            "payload",
            "mapped_address",
        ] {
            assert!(!encoded.contains(secret_field));
        }
    }

    #[test]
    fn ring_profile_pins_per_connection_grant_geometry() {
        let profile = ring_profile();
        assert_eq!(profile.descriptor_depth(), 8);
        assert_eq!(profile.max_leases(), 8);
        assert_eq!(profile.arena_bytes(), shm_transport::MIN_ARENA_BYTES);
    }

    #[test]
    fn grant_hex_is_strict_lowercase_ascii_without_panics() {
        assert_eq!(decode_hex::<2>("00af").unwrap(), [0x00, 0xaf]);
        assert!(decode_hex::<2>("00AF").is_err());
        assert!(decode_hex::<1>("+0").is_err());
        let non_ascii = std::panic::catch_unwind(|| decode_hex::<2>("0é0"));
        assert!(matches!(non_ascii, Ok(Err(_))));
    }

    #[test]
    fn inbound_materialization_cannot_exceed_its_byte_budget() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let from_host = rings.first.attachment().unwrap().attach().unwrap();
        let to_host = rings.second.attachment().unwrap().attach().unwrap();
        let endpoint = RingClientEndpoint { to_host, from_host };
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Response,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        for byte in [1, 2] {
            let mut reservation = rings.first.try_reserve(1, header.encode()).unwrap();
            reservation.write(&[byte]).unwrap();
            reservation.commit(1).unwrap();
        }
        let used = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let charge = |bytes| {
            let previous = used.fetch_add(bytes, Ordering::SeqCst);
            if previous + bytes > 1 {
                used.fetch_sub(bytes, Ordering::SeqCst);
                None
            } else {
                Some(TestCharge {
                    used: Arc::clone(&used),
                    bytes,
                })
            }
        };

        let first = endpoint.try_recv_with(charge).unwrap().unwrap();
        assert_eq!(first.1, [1]);
        assert_eq!(used.load(Ordering::SeqCst), 1);
        assert!(endpoint.try_recv_with(charge).is_err());
        assert_eq!(used.load(Ordering::SeqCst), 1);
        drop(first);
        assert_eq!(used.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn control_frame_body_is_copied_out_of_the_ring() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let geometry = rings.first.grant().geometry();
        assert_eq!(geometry.descriptor_depth, 8);
        assert_eq!(geometry.max_leases, 8);
        assert_eq!(
            geometry.mapping_bytes,
            (shm_transport::MIN_ARENA_BYTES + 8_192) as u64
        );
        let body = b"copy";
        let header = EnvelopeHeader {
            len: body.len() as u32,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        let mut reservation = rings
            .second
            .try_reserve(body.len(), header.encode())
            .unwrap();
        reservation.write(body).unwrap();
        reservation.commit(body.len()).unwrap();

        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let (inbound, mut received) = mpsc::channel(1);
        assert!(
            receive_one(
                &rings,
                &mut queue,
                &inbound,
                &ByteBudget::new(1024),
                Duration::from_secs(1),
                &CancellationToken::new(),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap()
        );
        let InboundEvent::Frame(frame) = received.recv().await.unwrap().unwrap() else {
            panic!("expected copied frame");
        };
        assert_eq!(frame.with_lease(|lease| lease.to_owned()), body);
        assert!(
            rings.second.try_receive().unwrap().is_none(),
            "the ring slot is released once the body is copied out"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn budget_wait_observes_read_cancellation_without_retiring() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let body = [7u8];
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        let mut reservation = rings.second.try_reserve(1, header.encode()).unwrap();
        reservation.write(&body).unwrap();
        reservation.commit(1).unwrap();
        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let (inbound, _received) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let cancellation = cancel.clone();
        let root = CancellationToken::new();
        let budget = ByteBudget::new(0);
        let receive = receive_one(
            &rings,
            &mut queue,
            &inbound,
            &budget,
            Duration::from_secs(1),
            &root,
            &cancellation,
            None,
        );
        let cancel_after_poll = async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(receive, cancel_after_poll);
        assert!(
            matches!(result, Ok(false)),
            "read cancellation is not a transport fault"
        );
        assert!(
            !queue.retired.is_cancelled(),
            "read cancellation must leave the writer draining"
        );
        assert!(
            rings.second.try_receive().unwrap().is_none(),
            "the parked frame is discarded with its lease"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn budget_wait_observes_discard_without_retiring() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        let mut reservation = rings.second.try_reserve(1, header.encode()).unwrap();
        reservation.write(&[7]).unwrap();
        reservation.commit(1).unwrap();
        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let discard = queue.discard.clone();
        let (inbound, _received) = mpsc::channel(1);
        let budget = ByteBudget::new(0);
        let root = CancellationToken::new();
        let read_cancel = CancellationToken::new();
        let receive = receive_one(
            &rings,
            &mut queue,
            &inbound,
            &budget,
            Duration::from_secs(1),
            &root,
            &read_cancel,
            None,
        );
        let discard_after_poll = async move {
            tokio::task::yield_now().await;
            discard.cancel();
        };
        let started = Instant::now();
        let (result, ()) = tokio::join!(receive, discard_after_poll);
        assert!(
            matches!(result, Ok(false)),
            "discard ends the budget wait without a transport fault"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "discard must not wait for the frame deadline"
        );
        assert!(!queue.retired.is_cancelled());
    }

    #[tokio::test]
    async fn read_cancellation_drains_frames_committed_before_it() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            read_cancel,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 1, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        let depth = ring_profile().descriptor_depth();
        for corr in 1..=depth as u64 {
            peer.send(
                EnvelopeHeader {
                    len: 1,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::Request,
                    flags: Flags::new(false, Priority::Interactive, false),
                    channel: 7,
                    epoch: 1,
                    corr,
                },
                &[1],
                StdInstant::now() + Duration::from_secs(1),
            )
            .expect("peer fills the ring");
        }
        // With a one-frame inbound queue, most of the ring is still uncommitted to the receiver when the read side is cancelled.
        let first = receiver.recv().await.expect("first frame");
        drop(first);
        read_cancel.cancel();

        let mut forwarded = 1usize;
        loop {
            match receiver.recv().await {
                Ok(InboundEvent::Frame(frame)) => {
                    assert_eq!(frame.header.corr, forwarded as u64 + 1);
                    forwarded += 1;
                }
                Ok(InboundEvent::Rejected(_)) => panic!("unexpected rejection"),
                Err(ReadClose::Cancelled) => break,
                Err(other) => panic!("unexpected close {other:?}"),
            }
        }
        assert_eq!(
            forwarded, depth,
            "every frame the peer committed before read cancellation is delivered before Cancelled"
        );
        assert!(!sender.is_retired());
        sender.finish();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits after finish")
            .expect("endpoint task joins");
    }

    #[tokio::test]
    async fn cancellation_reports_after_one_ring_depth_under_sustained_inbound() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            read_cancel,
            root,
        } = transport
            .prepare(ByteBudget::new(1 << 20), 64, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        let depth = ring_profile().descriptor_depth();
        let request = |corr: u64| EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr,
        };
        let deadline = || StdInstant::now() + Duration::from_secs(1);
        for corr in 1..=depth as u64 {
            peer.send(request(corr), &[1], deadline())
                .expect("peer fills the ring");
        }
        read_cancel.cancel();

        let mut forwarded = 0usize;
        let mut next_corr = depth as u64 + 1;
        loop {
            match receiver.recv().await {
                Ok(InboundEvent::Frame(frame)) => {
                    forwarded += 1;
                    drop(frame);
                    peer.send(request(next_corr), &[1], deadline())
                        .expect("peer refills the released slot");
                    next_corr += 1;
                }
                Ok(InboundEvent::Rejected(_)) => panic!("unexpected rejection"),
                Err(ReadClose::Cancelled) => break,
                Err(other) => panic!("unexpected close {other:?}"),
            }
            assert!(
                forwarded <= depth + 1,
                "a peer that refills every released slot must not postpone Cancelled past one ring depth"
            );
        }
        assert!(
            !sender.is_retired(),
            "read cancellation must leave the writer draining"
        );
        assert!(!root.is_cancelled());
        sender.finish();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits after finish")
            .expect("endpoint task joins");
    }

    #[tokio::test]
    async fn root_cancellation_is_observed_under_sustained_inbound() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender: _sender,
            mut receiver,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 64, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let mut io = tokio::spawn(io);
        let depth = ring_profile().descriptor_depth();
        let request = |corr: u64| EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr,
        };
        for corr in 1..=depth as u64 {
            peer.send(
                request(corr),
                &[1],
                StdInstant::now() + Duration::from_secs(1),
            )
            .expect("peer fills the ring");
        }
        let first = receiver.recv().await.expect("first frame");
        drop(first);
        root.cancel();
        let mut next_corr = depth as u64 + 1;
        let exited = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    joined = &mut io => {
                        joined.expect("endpoint task joins");
                        return;
                    }
                    event = receiver.recv() => {
                        if let Ok(InboundEvent::Frame(frame)) = event {
                            drop(frame);
                            let _ = peer.send(
                                request(next_corr),
                                &[1],
                                StdInstant::now() + Duration::from_millis(100),
                            );
                            next_corr += 1;
                        }
                    }
                }
            }
        })
        .await;
        assert!(
            exited.is_ok(),
            "root cancellation must stop the endpoint while the peer keeps the ring full"
        );
    }

    #[tokio::test]
    async fn root_cancellation_is_observed_while_the_inbound_queue_is_full() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender: _sender,
            receiver,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 1, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        for corr in 1..=4u64 {
            peer.send(
                EnvelopeHeader {
                    len: 1,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType::Request,
                    flags: Flags::new(false, Priority::Interactive, false),
                    channel: 7,
                    epoch: 1,
                    corr,
                },
                &[1],
                StdInstant::now() + Duration::from_secs(1),
            )
            .expect("peer fills the ring");
        }
        // Nothing drains `receiver`, so the endpoint parks on a full inbound queue.
        tokio::time::sleep(Duration::from_millis(100)).await;
        root.cancel();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("root cancellation must stop an endpoint blocked on a full inbound queue")
            .expect("endpoint task joins");
        drop(receiver);
        assert_eq!(
            transport.accounting().unwrap().active,
            ResourceCharges::ZERO,
            "a cancelled endpoint over a healthy ring refunds its admission"
        );
    }

    #[tokio::test]
    async fn transport_fault_is_reported_while_the_inbound_queue_is_full() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 1, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        peer.send(
            EnvelopeHeader {
                len: 1,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Request,
                flags: Flags::new(false, Priority::Interactive, false),
                channel: 7,
                epoch: 1,
                corr: 1,
            },
            &[1],
            StdInstant::now() + Duration::from_secs(1),
        )
        .expect("peer publishes one frame");
        // Nothing drains `receiver`, so the one-frame queue is full when the fault lands.
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.to_host.enter_quarantine();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("a fault must retire the endpoint without waiting for the receiver to drain")
            .expect("endpoint task joins");
        assert!(sender.is_retired());
        assert!(matches!(receiver.recv().await, Ok(InboundEvent::Frame(_))));
        assert!(
            matches!(receiver.recv().await, Err(ReadClose::Corrupt(_))),
            "the terminal event follows the queued frame instead of a bare channel drop"
        );
    }

    #[tokio::test]
    async fn endpoint_panic_is_reported_while_the_inbound_queue_is_full() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        transport.set_publish_hook(Arc::new(|_, _| panic!("completion hook panics")));
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 1, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        peer.send(
            EnvelopeHeader {
                len: 1,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Request,
                flags: Flags::new(false, Priority::Interactive, false),
                channel: 7,
                epoch: 1,
                corr: 1,
            },
            &[1],
            StdInstant::now() + Duration::from_secs(1),
        )
        .expect("peer publishes one frame");
        // Nothing drains `receiver`, so the one-frame queue is full when the hook panics.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let goodbye = OutboundFrame {
            bytes: crate::wire::encode_frame(
                FrameType::Goodbye,
                crate::wire::pure_header_flags(),
                crate::wire::FrameId::control(0),
                &[],
            )
            .expect("header-only frame encodes"),
            tail: Vec::new(),
            direct: None,
            charge: crate::wire::ByteCharge::none(),
            written: None,
        };
        sender.send(goodbye).await.expect("frame admits");
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("a panicking endpoint exits without the receiver draining")
            .expect("endpoint task joins");
        assert!(sender.is_retired());
        assert!(root.is_cancelled());
        assert_eq!(transport.diagnostics()["endpoint_panic"]["observed"], 1);
        assert!(matches!(receiver.recv().await, Ok(InboundEvent::Frame(_))));
        assert!(
            matches!(
                receiver.recv().await,
                Err(ReadClose::Corrupt("shared-memory endpoint panicked"))
            ),
            "the panic reason follows the queued frame instead of a bare channel drop"
        );
    }

    #[tokio::test]
    async fn peer_close_refunds_admission_although_the_backend_quarantines_the_ring() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        peer.send(
            EnvelopeHeader {
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Goodbye,
                flags: crate::wire::pure_header_flags(),
                channel: 0,
                epoch: 0,
                corr: 0,
            },
            &[],
            StdInstant::now() + Duration::from_secs(1),
        )
        .expect("peer publishes Goodbye");
        // An orderly peer closes its attachment right after Goodbye, before the host cancels the generation.
        drop(peer);
        assert!(matches!(
            receiver.recv().await,
            Ok(InboundEvent::Frame(frame)) if frame.header.ty == FrameType::Goodbye
        ));
        assert!(
            matches!(receiver.recv().await, Err(ReadClose::Corrupt(_))),
            "the closed doorbell still ends the read side as a transport fault"
        );
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits after the peer closes")
            .expect("endpoint task joins");
        assert!(sender.is_retired());

        let accounting = transport.accounting().unwrap();
        assert_eq!(accounting.active, ResourceCharges::ZERO);
        assert_eq!(
            accounting.quarantined,
            ResourceCharges::ZERO,
            "a peer that released its attachment does not consume host capacity"
        );
        assert!(
            transport
                .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
                .is_ok(),
            "the next connection admits after an orderly peer close"
        );
    }

    #[tokio::test]
    async fn root_cancellation_ends_a_budget_wait() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender: _sender,
            receiver: _receiver,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(0), 8, Duration::from_secs(30))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        peer.send(
            EnvelopeHeader {
                len: 1,
                ver: PROTOCOL_VERSION,
                ty: FrameType::Request,
                flags: Flags::new(false, Priority::Interactive, false),
                channel: 7,
                epoch: 1,
                corr: 1,
            },
            &[1],
            StdInstant::now() + Duration::from_secs(1),
        )
        .expect("peer publishes one frame");
        // A zero budget parks the endpoint in the ingress wait for this frame.
        tokio::time::sleep(Duration::from_millis(100)).await;
        root.cancel();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("root cancellation must end a budget wait well before the frame deadline")
            .expect("endpoint task joins");
        assert_eq!(
            transport.accounting().unwrap().active,
            ResourceCharges::ZERO
        );
    }

    #[test]
    fn a_commit_past_the_write_deadline_is_refused() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Response,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr: 1,
        };
        let deadline = StdInstant::now() + Duration::from_millis(20);
        let direct = DirectFrame::new(
            header,
            1,
            Box::new(|writer| {
                std::thread::sleep(Duration::from_millis(60));
                writer.write_all(&[1])
            }),
        );
        assert!(
            publish_direct(&rings.first, direct, deadline).is_err(),
            "a serializer that finishes after the deadline must not publish"
        );
        let attached = rings.first.attachment().unwrap().attach().unwrap();
        assert!(
            attached.try_receive().unwrap().is_none(),
            "the aborted reservation leaves no frame in the ring"
        );
    }

    #[tokio::test]
    async fn quarantined_ring_moves_its_charges_to_the_quarantined_bucket() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            mut receiver,
            io,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        let charges = ring_profile().charges();
        assert_eq!(transport.accounting().unwrap().active, charges);

        peer.to_host.enter_quarantine();
        assert!(
            matches!(receiver.recv().await, Err(ReadClose::Corrupt(_))),
            "a quarantined ring is a transport fault, not a clean close"
        );
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits after quarantine")
            .expect("endpoint task joins");
        assert!(sender.is_retired());

        let accounting = transport.accounting().unwrap();
        assert_eq!(accounting.active, ResourceCharges::ZERO);
        assert_eq!(
            accounting.quarantined,
            ResourceCharges {
                workers: 0,
                pinned_workers: 0,
                ..charges
            },
            "a quarantined ring keeps its memory charges and refunds only its workers"
        );
        assert!(
            transport
                .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
                .is_err(),
            "quarantined charges still count against the admission limit"
        );
    }
}
