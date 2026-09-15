//! Mandatory shared-memory payload-pool transport.
//!
//! One dedicated OS thread creates and owns both `!Send` pool endpoints. Host
//! tasks exchange frame tickets and completion notifications with that thread.
//! The backing charge is shared with every owned lease the endpoint hands out,
//! so it settles when the last lease returns, while the worker charge settles
//! when the thread exits.

use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};
use std::{fmt, io};

use crate::setup_socket::RING_DESCRIPTOR_COUNT;
use crate::wire::{EnvelopeHeader, FrameType, decode_header};
use shm_transport::backend::ring::{DuplexRing, ProducerError, ProducerReservation, Ring};
use shm_transport::backend::ring::{HOST_TO_PEER_LANE, PEER_TO_HOST_LANE, PoolGrant};
use shm_transport::pool::Inventory;
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

/// Current pool profile accepted by every process in one release.
pub const RING_PROFILE: &str = shm_transport::profile::HOST_PAYLOAD_POOL_PROFILE;

/// Test-only observer invoked after each successful frame publication with
/// the published frame's type and channel. It receives no descriptors,
/// payloads, or provider data.
#[doc(hidden)]
pub type PublishHook = Arc<dyn Fn(FrameType, u16) + Send + Sync>;

pub fn ring_profile() -> TargetProfile {
    shm_transport::profile::host_payload_pool_profile()
        .expect("static shared-memory profile is valid")
}

/// Admission limits sufficient for one connection.
pub fn per_connection_limits() -> ShmHostLimits {
    let charges = ring_profile().charges();
    ShmHostLimits {
        descriptors: charges.descriptors,
        mapping_bytes: charges.mapping_bytes,
        ledger_bytes: charges.ledger_bytes,
        leases: charges.leases,
        mappings: charges.mappings,
        file_descriptors: charges.file_descriptors,
        wake_handles: charges.wake_handles,
        workers: charges.workers,
        client_instances: charges.client_instances,
        pinned_workers: charges.pinned_workers,
    }
}

/// Maximum committed transport bytes admitted at once, including every pool's mapped bytes
/// and private ledgers. Pools never return touched pages to the kernel, so the maximum bounds
/// resident bytes as well as the mapping commitment.
pub const MAX_RING_RESIDENT_BYTES: u64 = 1 << 30;

/// Connections whose complete checked transport charge fits under
/// [`MAX_RING_RESIDENT_BYTES`]; never below one.
pub fn affordable_connections() -> u64 {
    let committed = ring_profile()
        .charges()
        .committed_bytes()
        .unwrap_or(u64::MAX);
    MAX_RING_RESIDENT_BYTES
        .checked_div(committed)
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
        mapping_bytes: scale(one.mapping_bytes)?,
        ledger_bytes: scale(one.ledger_bytes)?,
        leases: scale(one.leases)?,
        mappings: scale(one.mappings)?,
        file_descriptors: scale(one.file_descriptors)?,
        wake_handles: scale(one.wake_handles)?,
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
    /// Connection generations that ended. This is not released backing: an ended generation
    /// may leave owned leases live, which `returns` below reports separately.
    reclamations: AtomicU64,
    exhaustions: AtomicU64,
    /// Admission refusals by the first exhausted resource, so an operator can tell which
    /// ceiling refused rather than reading one aggregate count.
    refusals: Mutex<std::collections::BTreeMap<&'static str, u64>>,
    backings: Arc<BackingRegistry>,
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

/// Return obligations and released backing at one instant; see `RingTransport::return_snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnSnapshot {
    /// Payload leases this host holds across every mapped backing: the host's own return
    /// obligations to the peer. Leases the peer holds live in the peer's process and are not
    /// counted here.
    pub outstanding: u64,
    /// Backings still mapped by an endpoint handle or a lease.
    pub live_backings: u64,
    /// Backings whose last holder dropped.
    pub released_backings: u64,
    /// `released_backings * mapping_bytes_per_direction`.
    pub released_backing_bytes: u64,
}

/// Every retained backing this transport created, weakly. A backing that no longer upgrades
/// has unmapped: its leases returned and its handles dropped. Registration and endpoint exit
/// prune dead entries, so the registry is bounded by live backings plus those whose leases are
/// still in flight, not by the number of connections ever prepared.
#[derive(Default)]
struct BackingRegistry {
    entries: Mutex<Vec<std::sync::Weak<shm_transport::backend::retained::Retained>>>,
    /// Backings observed unmapped and pruned from `entries`.
    released: AtomicU64,
}

impl BackingRegistry {
    /// Drops entries that no longer upgrade, counting each as a released backing, and folds
    /// `live` over the rest.
    fn prune(&self, mut live: impl FnMut(&shm_transport::backend::retained::Retained)) {
        let mut entries = self.entries.lock().expect("backing registry lock");
        entries.retain(|weak| match weak.upgrade() {
            Some(retained) => {
                live(&retained);
                true
            }
            None => {
                self.released.fetch_add(1, Ordering::Relaxed);
                false
            }
        });
    }

    fn register(&self, rings: &DuplexRing) {
        self.prune(|_| {});
        self.entries.lock().expect("backing registry lock").extend([
            Arc::downgrade(rings.first.retained()),
            Arc::downgrade(rings.second.retained()),
        ]);
    }

    /// Removes both of `rings`' backings without counting them released. A quarantined
    /// connection's peer still maps the pools, so the host-side handles unmapping later proves
    /// nothing about the storage.
    fn forget(&self, rings: &DuplexRing) {
        let quarantined = [
            Arc::downgrade(rings.first.retained()),
            Arc::downgrade(rings.second.retained()),
        ];
        self.entries
            .lock()
            .expect("backing registry lock")
            .retain(|weak| !quarantined.iter().any(|gone| weak.ptr_eq(gone)));
    }
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
            refusals: Mutex::new(std::collections::BTreeMap::new()),
            backings: Arc::new(BackingRegistry::default()),
            endpoint_panics: Arc::new(AtomicU64::new(0)),
            publish_hook: Mutex::new(None),
        }
    }

    /// Return obligations and released backing, read in one pass over the backing registry:
    /// `outstanding` counts the leases this host holds across every backing still mapped, not
    /// the leases the peer holds, which live in the peer's process;
    /// `released_backings` counts backings whose last holder has dropped, each of which
    /// returned `mapping_bytes_per_direction` bytes. Leases return on other threads while the
    /// pass runs, so the two counts are independent samples, not one atomic snapshot.
    pub fn return_snapshot(&self) -> ReturnSnapshot {
        let mut outstanding = 0u64;
        let mut live = 0u64;
        self.backings.prune(|retained| {
            live += 1;
            outstanding = outstanding.saturating_add(retained.outstanding_returns());
        });
        let released = self.backings.released.load(Ordering::Relaxed);
        ReturnSnapshot {
            outstanding,
            live_backings: live,
            released_backings: released,
            released_backing_bytes: released
                .saturating_mul(self.profile.mapping_bytes_per_direction()),
        }
    }

    fn record_refusal(&self, error: shm_transport::profile::AdmissionError) {
        use shm_transport::profile::AdmissionError as E;
        let resource = match error {
            E::DescriptorLimit => "descriptors",
            E::MappingByteLimit => "mapping_bytes",
            E::LedgerByteLimit => "ledger_bytes",
            E::LeaseLimit => "leases",
            E::MappingLimit => "mappings",
            E::FileDescriptorLimit => "file_descriptors",
            E::WakeHandleLimit => "wake_handles",
            E::WorkerLimit => "workers",
            E::ClientInstanceLimit => "client_instances",
            E::PhysicalCoreBudgetExceeded | E::PhysicalCoresUnverified => "pinned_workers",
            E::ChargeOverflow | E::ChargeUnderflow | E::AccountingUnavailable => "accounting",
        };
        *self
            .refusals
            .lock()
            .expect("refusal counter lock")
            .entry(resource)
            .or_insert(0) += 1;
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
                "mapping_bytes": value.mapping_bytes,
                "ledger_bytes": value.ledger_bytes,
                "leases": value.leases,
                "mappings": value.mappings,
                "file_descriptors": value.file_descriptors,
                "wake_handles": value.wake_handles,
                "workers": value.workers,
                "client_instances": value.client_instances,
                "pinned_workers": value.pinned_workers,
            })
        };
        let limits = serde_json::json!({
            "descriptors": self.limits.descriptors,
            "mapping_bytes": self.limits.mapping_bytes,
            "ledger_bytes": self.limits.ledger_bytes,
            "leases": self.limits.leases,
            "mappings": self.limits.mappings,
            "file_descriptors": self.limits.file_descriptors,
            "wake_handles": self.limits.wake_handles,
            "workers": self.limits.workers,
            "client_instances": self.limits.client_instances,
            "pinned_workers": self.limits.pinned_workers,
        });
        let returns = self.return_snapshot();
        let refusals: serde_json::Map<String, serde_json::Value> = self
            .refusals
            .lock()
            .expect("refusal counter lock")
            .iter()
            .map(|(resource, count)| ((*resource).to_owned(), serde_json::json!(count)))
            .collect();
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
                "layout_version": shm_transport::backend::retained::LAYOUT_VERSION,
            },
            "bounds": limits,
            "accounting": accounting,
            "activation": {"completed": self.activations.load(Ordering::Acquire)},
            "peer_death": {"observed": self.peer_deaths.load(Ordering::Acquire)},
            // `completed` counts connection generations that ended; released backing is the
            // separate `returns` quantity below. The key is a wire name and stays.
            "reclamation": {
                "completed": self.reclamations.load(Ordering::Acquire),
                "meaning": "connection generations ended",
            },
            "returns": {
                "outstanding": returns.outstanding,
                "live_backings": returns.live_backings,
                "released_backings": returns.released_backings,
                "released_backing_bytes": returns.released_backing_bytes,
            },
            "exhaustion": {
                "observed": self.exhaustions.load(Ordering::Acquire),
                "by_resource": refusals,
            },
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
        let admission = self.admission.admit(&self.profile, None).map_err(|error| {
            self.exhaustions.fetch_add(1, Ordering::Relaxed);
            self.record_refusal(error);
            RingUnavailable
        })?;
        // The worker charge ends with the endpoint thread; the backing charge ends with the
        // last owned lease of either direction, or moves to quarantine.
        let (worker_admission, backing_admission) = admission.split();
        let backing_admission = Arc::new(backing_admission);
        let root = CancellationToken::new();
        let read_cancel = root.child_token();
        let (sender, queue) = frame_sender(queue_frames, root.clone(), frame_deadline);
        // One slot beyond `queue_frames` is reserved for the terminal event, so a fault or cancellation is reported even when the receiver has stopped draining.
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
        let backing_registry = Arc::clone(&self.backings);
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
                        // Nothing was exposed to a peer: every charge refunds with the drops.
                        let _ = initialized_tx.send(Err(RingUnavailable));
                        drop(worker_admission);
                        return;
                    }
                };
                rings.retain_charge(Arc::clone(&backing_admission));
                backing_registry.register(&rings);
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
                        queue_frames,
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
                // The endpoint has exited, so `io` completes now; the charges settle below, after
                // the peer-release wait, without delaying the caller.
                let _ = done_tx.send(());
                // The backing refunds only once both doorbells read end-of-file, which proves the
                // peer dropped its rings and every lease. An orderly peer tears its rings down right
                // after Goodbye, so the wait is short; a peer that keeps a mapping past the grace
                // moves the charge to the quarantined bucket rather than refunding storage it holds.
                let quarantined = !peer_released_ring(&rings, PEER_RELEASE_GRACE);
                if quarantined {
                    backing_registry.forget(&rings);
                }
                drop(rings);
                // A released backing with no lease in flight unmaps here; one still leased is
                // pruned by a later registration or snapshot.
                backing_registry.prune(|_| {});
                if quarantined && backing_admission.quarantine().is_err() {
                    // Quarantine accounting failed: the charge stays counted rather than falling
                    // through to a refund of storage nobody proved released.
                    backing_admission.retain_uncertain();
                }
                // The backing charge refunds when the last lease returns and this clone drops;
                // the worker charge refunds with the thread.
                drop(backing_admission);
                drop(worker_admission);
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

/// How long a retiring endpoint waits for the peer to drop its rings and leases before the
/// backing charge is treated as retained by the peer.
const PEER_RELEASE_GRACE: Duration = Duration::from_secs(2);

/// A stream doorbell reads end-of-file only after its peer end is closed, which is how a peer that exited or dropped its attachment appears to the host.
fn peer_released_ring(rings: &DuplexRing, grace: Duration) -> bool {
    // A lease the peer keeps from the host-to-peer pool holds that pool's doorbell end open, so
    // both directions must read end-of-file before the backing counts as released.
    let deadline = StdInstant::now() + grace;
    loop {
        if doorbell_at_eof(&rings.first) && doorbell_at_eof(&rings.second) {
            return true;
        }
        if StdInstant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn doorbell_at_eof(ring: &Ring) -> bool {
    use std::io::{ErrorKind, Read};
    let Ok(doorbell) = ring.duplicate_data_ready() else {
        return false;
    };
    let mut doorbell = std::os::unix::net::UnixStream::from(doorbell);
    if doorbell.set_nonblocking(true).is_err() {
        return false;
    }
    // The ring is already retired, so consuming pending wake tokens here has no reader to
    // starve. A peer that exited with a token unread surfaces as `ECONNRESET` once, then EOF.
    let mut buffer = [0u8; 64];
    for _ in 0..1024 {
        match doorbell.read(&mut buffer) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(error) => match error.kind() {
                ErrorKind::ConnectionReset | ErrorKind::Interrupted => {}
                _ => return false,
            },
        }
    }
    false
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
    queue_capacity: usize,
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
    let async_fd = |fd: OwnedFd| {
        tokio::io::unix::AsyncFd::new(fd)
            .map_err(|_| shm_transport::backend::ring::RingError::ObjectSetupFailed)
    };
    let readiness = match rings.second.duplicate_data_ready().and_then(async_fd) {
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
    // Capacity readiness of the outbound direction: descriptor acknowledgements and payload
    // returns from the peer ring it, so a blocked ticket waits here instead of parking the
    // owner inside an uninterruptible reserve.
    let capacity = match rings.first.duplicate_capacity_ready().and_then(async_fd) {
        Ok(capacity) => capacity,
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
    let mut publisher = Publisher::new(&rings.first, queue_capacity, frame_deadline, publish_hook);
    // One descriptor depth of receives after `read_cancel` covers every frame committed before it.
    let post_cancel_depth =
        usize::try_from(rings.second.grant().geometry().descriptor_depth()).unwrap_or(usize::MAX);
    let mut post_cancel_frames: Option<usize> = None;
    let mut finishing = false;
    loop {
        // The loop checks lifecycle tokens before receiving frames so sustained inbound traffic cannot bypass the `select!` below.
        if discard.is_cancelled() || root.is_cancelled() {
            return;
        }
        if finish.is_cancelled() {
            finishing = true;
        }
        // Returns and consumptions since the last pass settle credits and may unblock a ticket.
        if publisher.pump(&rings.first).is_err() {
            fail(
                &mut inbound,
                &mut queue,
                &root,
                ReadClose::Corrupt("shared-memory publish failed"),
            );
            return;
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
                    &mut publisher,
                    &capacity,
                    inbound_sender,
                    &ingress,
                    frame_deadline,
                    &root,
                    &read_cancel,
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

        let drained = if received {
            // Directions alternate under sustained inbound traffic: each
            // received frame is followed by at most one queued outbound
            // frame, taken without waiting, so a peer that refills the
            // inbound ring as slots release cannot starve responses, Pings,
            // and close frames while host-to-peer capacity is free.
            if publisher.can_accept() {
                queue.try_recv().ok()
            } else {
                None
            }
        } else if finishing && publisher.can_accept() {
            match queue.drain_finished() {
                Some(frame) => Some(frame),
                None if publisher.has_pending() => None,
                None => return,
            }
        } else {
            None
        };
        let queued = if received || drained.is_some() {
            drained
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
            // A blocked ticket parks on capacity readiness; an unblocked publisher never arms
            // it, so an idle owner does not wake on every peer return.
            let capacity_armed = match publisher.arm_capacity_wait(&rings.first) {
                Ok(armed) => armed,
                Err(()) => {
                    fail(
                        &mut inbound,
                        &mut queue,
                        &root,
                        ReadClose::Corrupt("shared-memory capacity wait failed"),
                    );
                    return;
                }
            };
            if publisher.has_pending() && !capacity_armed {
                // Capacity moved between the attempt and the arm; retry without blocking.
                if data_armed {
                    let _ = rings.second.complete_data_wait();
                }
                continue;
            }
            tokio::select! {
                biased;
                () = discard.cancelled() => return,
                () = finish.cancelled(), if !finishing => {
                    finishing = true;
                    None
                }
                () = read_cancel.cancelled(), if inbound.is_some() => None,
                frame = queue.recv(), if !finishing && publisher.can_accept() => match frame {
                    Some(frame) => Some(frame),
                    None if publisher.has_pending() => None,
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
                ready = capacity.readable(), if capacity_armed => {
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
                    if rings.first.complete_capacity_wait().is_err() {
                        fail(
                            &mut inbound,
                            &mut queue,
                            &root,
                            ReadClose::Corrupt("shared-memory capacity wait failed"),
                        );
                        return;
                    }
                    None
                },
                () = tokio::time::sleep_until(publisher.earliest_deadline()), if publisher.has_pending() => {
                    // A ticket outlived the frame deadline while parked: the peer is not
                    // draining, and the generation retires instead of waiting forever.
                    fail(
                        &mut inbound,
                        &mut queue,
                        &root,
                        ReadClose::Corrupt("shared-memory publish failed"),
                    );
                    return;
                }
                () = root.cancelled() => return,
            }
        };
        if let Some(queued) = queued {
            publisher.push(queued);
        }
        if publisher.pump(&rings.first).is_err() {
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

// `ShmReceiver::recv` maps a closed channel to `CleanEof`, so a fault must be sent explicitly before `inbound` drops.
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

// Teardown must not depend on the receiver draining: a full channel under `discard` or `root` cancellation yields instead of blocking the endpoint.
/// Hands `event` to the receiver. `pending_deadline` is the earliest deadline of a ticket the
/// publisher still holds, if any: the delivery wait can outlast it while the receiver is not
/// draining, and a ticket past its deadline retires the generation from here as well.
async fn deliver(
    inbound: &InboundSender,
    queue: &SenderQueue,
    root: &CancellationToken,
    pending_deadline: Option<Instant>,
    event: Result<InboundEvent, ReadClose>,
) -> Result<(), ReadClose> {
    tokio::select! {
        biased;
        sent = inbound.send(event) => sent.map_err(|_| ReadClose::Cancelled),
        () = queue.discard.cancelled() => Err(ReadClose::Cancelled),
        () = root.cancelled() => Err(ReadClose::Cancelled),
        () = tokio::time::sleep_until(pending_deadline.unwrap_or_else(Instant::now)), if pending_deadline.is_some() => {
            Err(ReadClose::Corrupt("shared-memory publish failed"))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_one(
    rings: &DuplexRing,
    queue: &mut SenderQueue,
    publisher: &mut Publisher,
    capacity: &tokio::io::unix::AsyncFd<OwnedFd>,
    inbound: &InboundSender,
    ingress: &ByteBudget,
    frame_deadline: Duration,
    root: &CancellationToken,
    read_cancel: &CancellationToken,
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
            publisher
                .has_pending()
                .then(|| publisher.earliest_deadline()),
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
        // A blocked ticket parks on capacity readiness during the budget wait too; otherwise a
        // peer return while handlers hold the budget would publish nothing until the deadline.
        let capacity_armed = if publisher.has_pending() {
            match publisher.arm_capacity_wait(&rings.first) {
                Ok(true) => true,
                Ok(false) => {
                    if publisher.pump(&rings.first).is_err() {
                        return Err(ReadClose::Corrupt("shared-memory publish failed"));
                    }
                    continue;
                }
                Err(()) => return Err(ReadClose::Corrupt("shared-memory capacity wait failed")),
            }
        } else {
            false
        };
        tokio::select! {
            biased;
            // An available budget charges before the lifecycle arms are polled, so a frame committed before read cancellation still drains; only a frame that must wait for budget yields to cancellation.
            charge = &mut charge => match charge {
                Some(charge) => break charge,
                // The declared body exceeds the ingress budget's capacity outright; no release can admit it.
                None => return Err(ReadClose::Overloaded),
            },
            // Read cancellation stops only the read side: dropping `lease` discards the frame and `Ok(false)` lets the writer keep draining.
            () = read_cancel.cancelled() => return Ok(false),
            // The endpoint loop observes `discard` and exits; the dropped lease discards the frame.
            () = discard.cancelled() => return Ok(false),
            () = tokio::time::sleep_until(deadline) => {
                // The peer and transport are healthy; only the ingress budget is
                // saturated. Overloaded retires the generation without branding
                // it corrupt, so the admission charge releases cleanly.
                return Err(ReadClose::Overloaded);
            }
            queued = queue.recv(), if publisher.can_accept() => match queued {
                Some(queued) => {
                    publisher.push(queued);
                    if publisher.pump(&rings.first).is_err() {
                        return Err(ReadClose::Corrupt("shared-memory publish failed"));
                    }
                }
                None => return Err(ReadClose::Cancelled),
            },
            ready = capacity.readable(), if capacity_armed => {
                let Ok(mut guard) = ready else {
                    return Err(ReadClose::Corrupt("shared-memory readiness failed"));
                };
                guard.clear_ready();
                if rings.first.complete_capacity_wait().is_err() {
                    return Err(ReadClose::Corrupt("shared-memory capacity wait failed"));
                }
                if publisher.pump(&rings.first).is_err() {
                    return Err(ReadClose::Corrupt("shared-memory publish failed"));
                }
            },
            () = tokio::time::sleep_until(publisher.earliest_deadline()), if publisher.has_pending() => {
                return Err(ReadClose::Corrupt("shared-memory publish failed"));
            }
        }
    };
    // The lease travels with the frame; the private copy happens where the request's work
    // ledgers can join it, and the block returns from there.
    deliver(
        inbound,
        queue,
        root,
        publisher
            .has_pending()
            .then(|| publisher.earliest_deadline()),
        Ok(InboundEvent::Frame(InboundFrame::new(
            header, lease, charge,
        ))),
    )
    .await?;
    Ok(true)
}

/// Which pool inventory a queued frame draws from. Pure-header controls use the reserved
/// control class; `Error` and `StreamEnd` terminals that fit use the reserved terminal class;
/// everything else is ordinary data. A channel-0 `Request` is never a control here: the host
/// does not send one, and the classifier treats any `Request` as ordinary.
fn inventory_for(header: &EnvelopeHeader, body_len: usize, terminal_capacity: u64) -> Inventory {
    match header.ty {
        ty if ty.is_pure_header() && body_len == 0 => Inventory::Control,
        FrameType::Error | FrameType::StreamEnd if body_len as u64 <= terminal_capacity => {
            Inventory::Terminal
        }
        _ => Inventory::Ordinary,
    }
}

/// One admitted frame the endpoint has not yet published.
struct PendingFrame {
    frame: OutboundFrame,
    header: Option<EnvelopeHeader>,
    inventory: Inventory,
    /// Body length the reservation is sized for; with `inventory`, the bound a capacity wait
    /// re-checks before parking.
    body_len: usize,
    /// Publication must complete before this instant or the generation retires.
    deadline: StdInstant,
}

impl PendingFrame {
    /// The request stream this frame belongs to, for the data-before-terminal rule. Pure-header
    /// controls carry host correlations from their own namespace and are never a stream
    /// prefix, so a `Ping` sharing a number with a consumer request does not hold its
    /// terminal back.
    fn stream_key(&self) -> Option<(u16, u64)> {
        self.header
            .filter(|header| !header.ty.is_pure_header())
            .map(|header| (header.channel, header.corr))
    }
}

/// The endpoint's single publication owner. Frames enter in admission order; ordinary frames
/// publish in that order, and while the head is blocked on ordinary capacity, eligible reserved
/// frames behind it publish from their own inventories: pure-header controls always, and a
/// terminal only when no earlier pending frame shares its `(channel, corr)`, so a terminal never
/// skips its own stream prefix. `Goodbye` is not eligible: it follows every admitted frame.
/// Every attempt is nonblocking; the owner parks on capacity readiness between attempts.
struct Publisher {
    pending: VecDeque<PendingFrame>,
    /// Frames the owner may hold unpublished at once. Together with the admission queue this
    /// bounds every unpublished ticket to twice the configured queue depth.
    capacity: usize,
    frame_deadline: Duration,
    terminal_capacity: u64,
    hook: Option<PublishHook>,
    /// Terminal credit held per block until the block physically returns.
    credits: Vec<Option<tokio::sync::OwnedSemaphorePermit>>,
}

impl Publisher {
    fn new(
        ring: &Ring,
        capacity: usize,
        frame_deadline: Duration,
        hook: Option<PublishHook>,
    ) -> Self {
        let geometry = ring.geometry();
        let mut credits = Vec::with_capacity(geometry.block_count() as usize);
        credits.resize_with(geometry.block_count() as usize, || None);
        Self {
            pending: VecDeque::new(),
            capacity: capacity.max(1),
            frame_deadline,
            terminal_capacity: geometry
                .class(shm_transport::pool::BlockClass::Terminal)
                .body_capacity(),
            hook,
            credits,
        }
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Inventory and bound of the head frame, which `pump` left pending only because its
    /// reservation was exhausted; a capacity wait arms against this reservation.
    fn blocked_head(&self) -> Option<(Inventory, usize)> {
        self.pending
            .front()
            .map(|pending| (pending.inventory, pending.body_len))
    }

    /// Arms the capacity doorbell for the blocked head. `Ok(true)` means the caller should park
    /// on capacity readiness; `Ok(false)` means nothing is blocked or capacity moved, so the
    /// caller pumps again instead. `Err` is a ring failure the caller retires on.
    ///
    /// The ring re-checks only the head's reservation when it arms. A reserved frame behind the
    /// head draws from its own inventory, so a return of that inventory before the arm bumps
    /// the generation without a token and the head's check cannot see it. One pump after the
    /// arm publishes such a frame; when it does, the arm is undone and the caller loops.
    fn arm_capacity_wait(&mut self, ring: &Ring) -> Result<bool, ()> {
        let Some((inventory, bound)) = self.blocked_head() else {
            return Ok(false);
        };
        if !ring.arm_capacity_wait(inventory, bound).map_err(|_| ())? {
            return Ok(false);
        }
        let before = self.pending.len();
        self.pump(ring)?;
        if self.pending.len() == before {
            return Ok(true);
        }
        ring.complete_capacity_wait().map_err(|_| ())?;
        Ok(false)
    }

    /// Whether another admitted frame may move from the queue into the owner's pending set.
    fn can_accept(&self) -> bool {
        self.pending.len() < self.capacity
    }

    fn earliest_deadline(&self) -> Instant {
        let earliest = self
            .pending
            .iter()
            .map(|pending| pending.deadline)
            .min()
            .unwrap_or_else(StdInstant::now);
        Instant::from_std(earliest)
    }

    fn push(&mut self, frame: OutboundFrame) {
        let header = match &frame.direct {
            Some(direct) => decode_header(&direct.header()).ok(),
            None => frame
                .bytes
                .get(..crate::wire::HEADER_LEN)
                .and_then(|header| header.try_into().ok())
                .and_then(|header: [u8; crate::wire::HEADER_LEN]| decode_header(&header).ok()),
        };
        let body_len = match &frame.direct {
            Some(direct) => direct.body_len(),
            None => frame
                .bytes
                .len()
                .saturating_sub(crate::wire::HEADER_LEN)
                .saturating_add(frame.tail.len()),
        };
        let inventory = header.as_ref().map_or(Inventory::Ordinary, |header| {
            inventory_for(header, body_len, self.terminal_capacity)
        });
        self.pending.push_back(PendingFrame {
            frame,
            header,
            inventory,
            body_len,
            deadline: StdInstant::now() + self.frame_deadline,
        });
    }

    /// Settles returned blocks, then publishes everything eligible. `Err` means a frame failed
    /// for a reason a retry cannot clear or outlived its deadline; the caller retires the
    /// generation. Publication failure after commit is reported the same way.
    fn pump(&mut self, ring: &Ring) -> Result<(), ()> {
        let credits = &mut self.credits;
        ring.take_reclaimed(|block| {
            if let Some(slot) = credits.get_mut(block as usize) {
                // The credit follows the block: it is released here, at the physical return.
                *slot = None;
            }
        })
        .map_err(|_| ())?;
        let now = StdInstant::now();
        if self.pending.iter().any(|pending| pending.deadline <= now) {
            return Err(());
        }
        let mut index = 0;
        let mut head_blocked = false;
        while index < self.pending.len() {
            let pending = &self.pending[index];
            let eligible = if index == 0 {
                true
            } else if head_blocked {
                match pending.inventory {
                    Inventory::Control => pending
                        .header
                        .is_some_and(|header| header.ty != FrameType::Goodbye),
                    Inventory::Terminal => {
                        let key = pending.stream_key();
                        !self
                            .pending
                            .iter()
                            .take(index)
                            .any(|earlier| earlier.stream_key() == key)
                    }
                    Inventory::Ordinary => false,
                }
            } else {
                // The head published; the next frame is the new head.
                true
            };
            if !eligible {
                index += 1;
                continue;
            }
            match self.try_publish(ring, index)? {
                true => {
                    // Removed at `index`; the same index now names the next frame.
                    if index == 0 {
                        head_blocked = false;
                    }
                }
                false => {
                    if index == 0 {
                        head_blocked = true;
                    }
                    index += 1;
                }
            }
        }
        Ok(())
    }

    /// Publishes the pending frame at `index` if its inventory has capacity. `Ok(false)` leaves
    /// the frame pending.
    fn try_publish(&mut self, ring: &Ring, index: usize) -> Result<bool, ()> {
        let (inventory, deadline) = {
            let pending = &self.pending[index];
            (pending.inventory, pending.deadline)
        };
        let frame = &self.pending[index].frame;
        let (header, body_len): ([u8; crate::wire::HEADER_LEN], usize) = match &frame.direct {
            Some(direct) => (direct.header(), direct.body_len()),
            None => {
                let (header, first_body) = frame
                    .bytes
                    .split_at_checked(crate::wire::HEADER_LEN)
                    .ok_or(())?;
                (
                    header.try_into().map_err(|_| ())?,
                    first_body.len().checked_add(frame.tail.len()).ok_or(())?,
                )
            }
        };
        let reservation = match ring.try_reserve_in(inventory, body_len, header) {
            Ok(reservation) => reservation,
            Err(ProducerError::Exhausted) => return Ok(false),
            Err(_) => return Err(()),
        };
        let block = reservation.block();
        let PendingFrame {
            frame,
            header: decoded,
            ..
        } = self.pending.remove(index).ok_or(())?;
        let OutboundFrame {
            bytes,
            tail,
            direct,
            charge,
            written,
            credit,
        } = frame;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match direct {
            Some(direct) => publish_direct(reservation, direct, body_len, deadline),
            None => publish_owned(reservation, &bytes, &tail, body_len, deadline),
        }));
        if !matches!(result, Ok(Ok(()))) {
            return Err(());
        }
        if let Some(slot) = self.credits.get_mut(block as usize) {
            *slot = credit;
        }
        if let (Some(hook), Some(header)) = (self.hook.as_ref(), decoded) {
            hook(header.ty, header.channel);
        }
        if let Some(written) = written {
            written(Instant::now());
        }
        drop(charge);
        Ok(true)
    }
}

fn publish_direct(
    mut reservation: ProducerReservation<'_>,
    direct: DirectFrame,
    body_len: usize,
    deadline: StdInstant,
) -> Result<(), ()> {
    let result = crate::panic_boundary::redact_sync(|| {
        let mut writer = ReservationWriter(&mut reservation);
        direct.serialize(&mut writer)
    });
    result.map_err(|_| ())?;
    commit_before(reservation, body_len, deadline)
}

fn publish_owned(
    mut reservation: ProducerReservation<'_>,
    bytes: &[u8],
    tail: &[u8],
    body_len: usize,
    deadline: StdInstant,
) -> Result<(), ()> {
    let (_, first_body) = bytes.split_at_checked(crate::wire::HEADER_LEN).ok_or(())?;
    reservation.write(first_body).map_err(|_| ())?;
    reservation.write(tail).map_err(|_| ())?;
    commit_before(reservation, body_len, deadline)
}

// Serialization runs after the reservation is taken, so the deadline is re-checked at commit; dropping an uncommitted reservation aborts it.
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

/// Thread-confined peer endpoint: the managed Rust client's bridge and the integration tests
/// attach through it.
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
        let expected = ring_profile();
        if from_host_grant.geometry() != to_host_grant.geometry()
            || from_host_grant.geometry() != expected.geometry()
            || from_host_grant.lane() != HOST_TO_PEER_LANE
            || to_host_grant.lane() != PEER_TO_HOST_LANE
        {
            return Err(RingClientError);
        }
        let from_host = Ring::attach([from_mapping, from_data, from_capacity], from_host_grant)
            .map_err(|_| RingClientError)?;
        let to_host = Ring::attach([to_mapping, to_data, to_capacity], to_host_grant)
            .map_err(|_| RingClientError)?;
        // Setup attaches only fresh pools: a pool with traffic already in flight was not created
        // for this activation.
        if !from_host.is_fresh() || !to_host.is_fresh() {
            return Err(RingClientError);
        }
        Ok(Self { to_host, from_host })
    }

    /// Parks on the capacity doorbell until the frame's inventory has room or `deadline`
    /// passes, then publishes. The inventory follows the frame exactly as in
    /// [`Self::try_send_bounded`].
    pub fn send(
        &self,
        header: EnvelopeHeader,
        body: &[u8],
        deadline: StdInstant,
    ) -> Result<(), SendFailure> {
        let inventory = self.inventory_for_frame(&header, body.len());
        let reservation = self
            .to_host
            .reserve_until_in(inventory, body.len(), header.encode(), deadline)
            .map_err(|error| match error {
                ProducerError::Deadline => SendFailure::Deadline,
                _ => SendFailure::Unreserved,
            })?;
        self.publish(reservation, body, deadline)
    }

    /// Publishes one frame without waiting for capacity. The inventory follows the frame:
    /// pure-header controls take the control reserve, terminals that fit take the terminal
    /// reserve, everything else is ordinary. `Ok(TrySend::Exhausted)` means the inventory has
    /// no block or descriptor headroom right now and nothing was charged; the caller parks on
    /// the capacity doorbell (`to_host.arm_capacity_wait`) and retries. A frame whose deadline
    /// has passed is refused before commit and publishes nothing.
    pub fn try_send_bounded(
        &self,
        header: EnvelopeHeader,
        body: &[u8],
        frame_deadline: StdInstant,
    ) -> Result<TrySend, SendFailure> {
        let inventory = self.inventory_for_frame(&header, body.len());
        if StdInstant::now() >= frame_deadline {
            return Err(SendFailure::Deadline);
        }
        let reservation = match self
            .to_host
            .try_reserve_in(inventory, body.len(), header.encode())
        {
            Ok(reservation) => reservation,
            Err(ProducerError::Exhausted) => return Ok(TrySend::Exhausted),
            Err(_) => return Err(SendFailure::Unreserved),
        };
        self.publish(reservation, body, frame_deadline)
            .map(|()| TrySend::Published)
    }

    pub(crate) fn inventory_for_frame(
        &self,
        header: &EnvelopeHeader,
        body_len: usize,
    ) -> Inventory {
        let terminal_capacity = self
            .to_host
            .geometry()
            .class(shm_transport::pool::BlockClass::Terminal)
            .body_capacity();
        inventory_for(header, body_len, terminal_capacity)
    }

    /// The body copy runs after reservation, so `publish` re-checks `frame_deadline` before
    /// `commit`; dropping an uncommitted reservation publishes nothing.
    fn publish(
        &self,
        mut reservation: ProducerReservation<'_>,
        body: &[u8],
        frame_deadline: StdInstant,
    ) -> Result<(), SendFailure> {
        // A failed `write` aborts the reservation, so nothing was published.
        reservation
            .write(body)
            .map_err(|_| SendFailure::Unreserved)?;
        if StdInstant::now() >= frame_deadline {
            return Err(SendFailure::Deadline);
        }
        // `commit` aborts on a quarantined ring without publishing, but its error does not say
        // whether the quarantine was seen before or after publication. Checking here first
        // classifies the common pre-commit case as zero-byte; only a quarantine that lands
        // between this check and `commit` stays ambiguous.
        if self.to_host.is_quarantined() {
            return Err(SendFailure::Unreserved);
        }
        reservation
            .commit(body.len())
            .map_err(|_| SendFailure::Reserved)?;
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

fn decode_grant(grant: &str) -> Result<PoolGrant, RingClientError> {
    PoolGrant::decode(decode_hex(grant)?).map_err(|_| RingClientError)
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

/// Outcome of a nonblocking publication attempt that did not fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrySend {
    /// The frame is committed and visible to the host.
    Published,
    /// The frame's inventory had no free block or descriptor headroom; nothing was charged.
    Exhausted,
}

/// The stage at which [`RingClientEndpoint::send`] failed.
///
/// A frame that never obtained a reservation wrote zero bytes; after reservation, the host's view is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendFailure {
    /// The ring stayed full until the reservation deadline, or the frame deadline passed before commit; no bytes were published and a later attempt can succeed.
    Deadline,
    /// The frame was aborted before publication for a reason a retry cannot clear, such as a quarantined ring or a body that does not fit its reservation; no bytes were published.
    Unreserved,
    /// `commit` failed and may have published the frame.
    Reserved,
}

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

    /// The backing charge settles after `io` completes, once the peer-release wait ends.
    /// Blocks until the endpoint thread has settled its backing after `io` completed: the
    /// thread waits for doorbell end-of-file before refunding or quarantining, so accounting
    /// and the backing registry lag `io` by up to `PEER_RELEASE_GRACE`.
    fn wait_until(settled: impl Fn() -> bool) {
        let deadline = StdInstant::now() + Duration::from_secs(5);
        while !settled() && StdInstant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    async fn settled_accounting(
        transport: &RingTransport,
        settled: impl Fn(&shm_transport::profile::AccountingSnapshot) -> bool,
    ) -> shm_transport::profile::AccountingSnapshot {
        let deadline = StdInstant::now() + Duration::from_secs(5);
        loop {
            let snapshot = transport.accounting().unwrap();
            if settled(&snapshot) || StdInstant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

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
    fn production_profile_affords_five_connections_under_the_byte_ceiling() {
        let one = per_connection_limits();
        assert_eq!(one.mapping_bytes, 2 * 95_825_920);
        assert_eq!(one.ledger_bytes, 2 * 187 * 44);
        assert_eq!(MAX_RING_RESIDENT_BYTES, 1 << 30);
        assert_eq!(affordable_connections(), 5);
        assert_eq!(crate::config::HostLimits::default().max_connections, 5);
    }

    #[test]
    fn process_limits_reject_counts_above_the_resident_byte_ceiling() {
        let affordable = affordable_connections();
        let one = per_connection_limits();
        assert_eq!(
            affordable,
            MAX_RING_RESIDENT_BYTES / (one.mapping_bytes + one.ledger_bytes),
            "affordable count is the byte ceiling divided by one connection's complete charge"
        );
        assert!(affordable >= 1);
        let exact = process_limits(usize::try_from(affordable).unwrap()).expect("affordable");
        assert_eq!(exact.mapping_bytes, one.mapping_bytes * affordable);
        assert_eq!(exact.ledger_bytes, one.ledger_bytes * affordable);
        assert_eq!(exact.wake_handles, one.wake_handles * affordable);
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
        let client = include_str!("client.rs");
        let micro_poll = concat!("Duration::from_micros(", "50)");
        assert!(!endpoint.contains(micro_poll));
        assert!(!client.contains(micro_poll));
        assert!(!endpoint.contains(concat!("POLL_", "INTERVAL")));
        // The client bridge parks on the capacity doorbell; it neither slices a blocking
        // reservation nor retries on a timer.
        assert!(!client.contains(concat!("BRIDGE_RESERVE", "_SLICE")));
        assert!(!client.contains(concat!("reserve_", "until")));
        assert!(!client.contains(concat!(".send_", "bounded(")));
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

    /// Kernel thread ids of live endpoint threads. `comm` truncates names to 15 bytes.
    fn endpoint_thread_ids() -> std::collections::BTreeSet<u32> {
        std::fs::read_dir("/proc/self/task")
            .expect("task directory")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let tid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
                comm.trim()
                    .starts_with(&"host-shm-endpoint"[..15])
                    .then_some(tid)
            })
            .collect()
    }

    /// User plus system clock ticks consumed by `threads`, from `/proc/self/task/<tid>/stat`
    /// fields 14 and 15.
    fn cpu_ticks(threads: &std::collections::BTreeSet<u32>) -> u64 {
        threads
            .iter()
            .map(|tid| {
                let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat"))
                    .unwrap_or_default();
                let after_comm = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
                let fields: Vec<&str> = after_comm.split_whitespace().collect();
                let tick = |index: usize| -> u64 {
                    fields.get(index).and_then(|v| v.parse().ok()).unwrap_or(0)
                };
                tick(11) + tick(12)
            })
            .sum()
    }

    /// A finishing endpoint whose head is blocked on peer capacity parks on capacity
    /// readiness instead of re-running the publisher until the peer drains or the frame
    /// deadline passes.
    #[tokio::test]
    async fn a_finishing_endpoint_with_a_blocked_head_parks_instead_of_spinning() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let before = endpoint_thread_ids();
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            receiver: _receiver,
            io,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(5))
            .expect("ring prepares");
        let endpoint_threads: std::collections::BTreeSet<u32> =
            endpoint_thread_ids().difference(&before).copied().collect();
        assert!(
            !endpoint_threads.is_empty(),
            "the endpoint thread is visible"
        );
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);

        // The peer holds every block of the smallest ordinary class. Receiving as frames land
        // keeps the sender queue and the ordinary descriptors from filling first.
        let count = ring_profile()
            .geometry()
            .class(shm_transport::pool::BlockClass::Ordinary(0))
            .count as usize;
        let mut held = Vec::with_capacity(count);
        for corr in 0..count as u64 {
            sender
                .send(frame(FrameType::StreamData, 7, corr, b"fill"))
                .await
                .expect("fill frame admits");
            while let Some(lease) = peer.from_host.try_receive().expect("peer receives") {
                held.push(lease);
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while held.len() < count {
            assert!(
                tokio::time::Instant::now() < deadline,
                "fill frames publish"
            );
            match peer.from_host.try_receive().expect("peer receives") {
                Some(lease) => held.push(lease),
                None => tokio::time::sleep(Duration::from_millis(1)).await,
            }
        }
        // One more ordinary frame blocks at the head; Goodbye waits behind it.
        sender
            .send(frame(FrameType::StreamData, 7, 99, b"blocked"))
            .await
            .expect("blocked frame admits");
        sender
            .send(frame(FrameType::Goodbye, 0, 0, &[]))
            .await
            .expect("goodbye admits");
        sender.finish();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let ticks_before = cpu_ticks(&endpoint_threads);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let ticks = cpu_ticks(&endpoint_threads) - ticks_before;
        assert!(
            ticks < 10,
            "a finishing endpoint blocked on peer capacity used {ticks} clock ticks in 500ms"
        );

        // Returning the held blocks lets the head and then Goodbye publish; the endpoint exits.
        drop(held);
        tokio::time::timeout(Duration::from_secs(2), io)
            .await
            .expect("endpoint exits once the peer returns capacity")
            .expect("endpoint task joins");
        let (head, _) = peer.try_recv().unwrap().expect("blocked head published");
        assert_eq!((head.ty, head.corr), (FrameType::StreamData, 99));
        let (goodbye, _) = peer.try_recv().unwrap().expect("goodbye published");
        assert_eq!(goodbye.ty, FrameType::Goodbye);
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
        assert_eq!(
            diagnostics["artifact"]["layout_version"],
            shm_transport::backend::retained::LAYOUT_VERSION
        );
        assert_eq!(diagnostics["bounds"]["mapping_bytes"], limits.mapping_bytes);
        assert_eq!(diagnostics["bounds"]["ledger_bytes"], limits.ledger_bytes);
        assert_eq!(diagnostics["bounds"]["wake_handles"], limits.wake_handles);
        assert_eq!(diagnostics["accounting"]["active"]["mapping_bytes"], 0);
        assert_eq!(diagnostics["accounting"]["quarantined"]["mapping_bytes"], 0);
        assert_eq!(diagnostics["activation"]["completed"], 1);
        assert_eq!(diagnostics["peer_death"]["observed"], 1);
        assert_eq!(diagnostics["reclamation"]["completed"], 1);
        assert_eq!(diagnostics["exhaustion"]["observed"], 0);

        // The profile id spells "payload"; only the literal it names is allowed to.
        let encoded = diagnostics.to_string().replace(RING_PROFILE, "");
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
    fn grant_hex_is_strict_lowercase_ascii_without_panics() {
        assert_eq!(decode_hex::<2>("00af").unwrap(), [0x00, 0xaf]);
        assert!(decode_hex::<2>("00AF").is_err());
        assert!(decode_hex::<1>("+0").is_err());
        let non_ascii = std::panic::catch_unwind(|| decode_hex::<2>("0é0"));
        assert!(matches!(non_ascii, Ok(Err(_))));
    }

    #[test]
    fn setup_rejects_grants_whose_lanes_do_not_match_their_direction() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            mut descriptor,
            descriptors,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let fields = descriptor.as_object_mut().unwrap();
        let host_to_peer = fields.remove("host_to_peer_grant").unwrap();
        let peer_to_host = fields.remove("peer_to_host_grant").unwrap();
        fields.insert("host_to_peer_grant".to_owned(), peer_to_host);
        fields.insert("peer_to_host_grant".to_owned(), host_to_peer);
        let [
            from_mapping,
            from_data,
            from_capacity,
            to_mapping,
            to_data,
            to_capacity,
        ] = descriptors;
        let swapped = [
            to_mapping,
            to_data,
            to_capacity,
            from_mapping,
            from_data,
            from_capacity,
        ];
        assert!(
            RingClientEndpoint::attach_with_descriptors(&descriptor, swapped).is_err(),
            "each grant attaches to its own mapping, but lane 1 is not host-to-peer"
        );
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

    fn capacity_fd(rings: &DuplexRing) -> tokio::io::unix::AsyncFd<OwnedFd> {
        tokio::io::unix::AsyncFd::new(rings.first.duplicate_capacity_ready().unwrap()).unwrap()
    }

    /// Holds every terminal-class block on the consumer side so an eligible terminal frame
    /// blocks on its own inventory.
    fn exhaust_terminal_class(
        producer: &Ring,
        consumer: &Ring,
    ) -> Vec<shm_transport::lease::PayloadLease> {
        let count = producer
            .geometry()
            .class(shm_transport::pool::BlockClass::Terminal)
            .count;
        (0..count)
            .map(|_| {
                let mut reservation = producer
                    .try_reserve_in(
                        Inventory::Terminal,
                        1,
                        shm_transport::backend::ring::wire_v3_header(1).unwrap(),
                    )
                    .expect("fill terminal reservation");
                reservation.write(&[1]).unwrap();
                reservation.commit(1).unwrap();
                consumer.try_receive().unwrap().expect("filled terminal")
            })
            .collect()
    }

    /// Holds every control-class block on the consumer side so a pure-header control blocks
    /// on its own inventory.
    fn exhaust_control_class(
        producer: &Ring,
        consumer: &Ring,
    ) -> Vec<shm_transport::lease::PayloadLease> {
        let count = producer
            .geometry()
            .class(shm_transport::pool::BlockClass::Control)
            .count;
        (0..count)
            .map(|_| {
                let reservation = producer
                    .try_reserve_in(
                        Inventory::Control,
                        0,
                        shm_transport::backend::ring::wire_v3_header(0).unwrap(),
                    )
                    .expect("fill control reservation");
                reservation.commit(0).unwrap();
                consumer.try_receive().unwrap().expect("filled control")
            })
            .collect()
    }

    /// A blocked host `Ping` whose correlation equals a channel-0 `Error`'s consumer
    /// correlation is not that error's stream prefix; the error still bypasses.
    #[test]
    fn a_blocked_ping_sharing_a_correlation_does_not_hold_back_a_channel_zero_error() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let ordinary = exhaust_smallest_class(&rings.first, &consumer);
        let controls = exhaust_control_class(&rings.first, &consumer);
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_secs(5), None);
        publisher.push(frame(FrameType::StreamData, 7, 1, b"blocked"));
        publisher.push(frame(FrameType::Ping, 0, 5, b""));
        publisher.push(frame(FrameType::Error, 0, 5, b"{}"));
        publisher.pump(&rings.first).expect("pump");
        assert_eq!(
            publisher.pending.len(),
            2,
            "the ordinary head and the Ping stay blocked; the error publishes"
        );
        assert!(
            publisher
                .pending
                .iter()
                .all(|pending| pending.inventory != Inventory::Terminal),
            "no terminal remains pending"
        );
        drop(ordinary);
        drop(controls);
    }

    /// `Publisher::arm_capacity_wait` must return `Ok(false)` and publish an eligible terminal
    /// when a terminal block returns after a blocked pump and before arming, even though the
    /// ordinary head stays exhausted.
    #[test]
    fn arming_publishes_a_bypassable_terminal_whose_block_returned_before_arming() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let ordinary = exhaust_smallest_class(&rings.first, &consumer);
        let mut terminals = exhaust_terminal_class(&rings.first, &consumer);
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_secs(5), None);
        publisher.push(frame(FrameType::StreamData, 7, 1, b"blocked"));
        publisher.push(frame(FrameType::Error, 9, 2, b"{}"));
        publisher
            .pump(&rings.first)
            .expect("both frames stay blocked");
        assert_eq!(publisher.pending.len(), 2);

        terminals.pop().unwrap().release().unwrap();
        assert_eq!(
            publisher.arm_capacity_wait(&rings.first),
            Ok(false),
            "the returned terminal block makes the eligible terminal publishable"
        );
        assert_eq!(
            publisher.pending.len(),
            1,
            "the terminal published past the still-blocked ordinary head"
        );
        drop(ordinary);
        drop(terminals);
    }

    /// `arm_capacity_wait` must return `Ok(false)` when capacity returns after a blocked pump
    /// and before arming.
    #[test]
    fn arming_against_the_blocked_head_refuses_to_park_over_a_return_before_arming() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let mut held = exhaust_smallest_class(&rings.first, &consumer);
        let mut publisher = Publisher::new(&rings.first, 1, Duration::from_secs(5), None);
        publisher.push(frame(FrameType::StreamData, 7, 1, b"blocked"));
        publisher.pump(&rings.first).expect("blocked pump");
        let (inventory, bound) = publisher.blocked_head().expect("the head is blocked");
        assert_eq!(inventory, Inventory::Ordinary);

        held.pop().unwrap().release().unwrap();
        assert_eq!(
            rings.first.arm_capacity_wait(inventory, bound),
            Ok(false),
            "capacity is already back; parking would wait for an unrelated return"
        );
        publisher
            .pump(&rings.first)
            .expect("pump publishes the head");
        assert!(!publisher.has_pending());
        drop(held);
    }

    /// A blocked ticket's frame deadline must still retire the generation while `receive_one`
    /// waits for inbound delivery space.
    #[tokio::test]
    async fn a_blocked_ticket_deadline_retires_the_generation_while_delivery_is_blocked() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let held = exhaust_smallest_class(&rings.first, &consumer);
        // One blocked ordinary ticket with a short deadline.
        let mut publisher = Publisher::new(&rings.first, 1, Duration::from_millis(300), None);
        publisher.push(frame(FrameType::StreamData, 7, 1, b"blocked"));
        publisher.pump(&rings.first).expect("blocked pump");
        assert!(publisher.has_pending());

        // The peer commits a request the budget grants at once; delivery is what blocks,
        // because the inbound channel already holds an undrained event.
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr: 1,
        };
        let mut reservation = rings.second.try_reserve(1, header.encode()).unwrap();
        reservation.write(&[7]).unwrap();
        reservation.commit(1).unwrap();
        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let (inbound, _received) = mpsc::channel(1);
        inbound
            .try_send(Err(ReadClose::CleanEof))
            .expect("fill the inbound channel");
        let budget = ByteBudget::new(1024);
        let capacity = capacity_fd(&rings);
        let root = CancellationToken::new();
        let read_cancel = CancellationToken::new();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            receive_one(
                &rings,
                &mut queue,
                &mut publisher,
                &capacity,
                &inbound,
                &budget,
                Duration::from_secs(5),
                &root,
                &read_cancel,
            ),
        )
        .await
        .expect("the blocked ticket's deadline ends the delivery wait");
        assert!(
            matches!(
                result,
                Err(ReadClose::Corrupt("shared-memory publish failed"))
            ),
            "a ticket past its deadline retires the generation: {result:?}"
        );
        drop(held);
    }

    /// A peer return during the ingress-budget wait publishes the blocked ticket instead of
    /// waiting for the budget or the frame deadline.
    #[tokio::test]
    async fn a_budget_wait_publishes_a_blocked_ticket_when_the_peer_returns_capacity() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let mut held = exhaust_smallest_class(&rings.first, &consumer);
        let published = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&published);
        let hook: PublishHook = Arc::new(move |ty, channel| {
            observed.lock().unwrap().push((ty, channel));
        });
        // The publisher is full with one blocked ordinary ticket, so no queued frame can
        // move it.
        let mut publisher = Publisher::new(&rings.first, 1, Duration::from_secs(5), Some(hook));
        publisher.push(frame(FrameType::StreamData, 7, 1, b"blocked"));
        publisher.pump(&rings.first).expect("blocked pump");
        assert!(publisher.has_pending() && !publisher.can_accept());

        // The peer commits a request whose one byte the ingress budget cannot grant.
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr: 1,
        };
        let mut reservation = rings.second.try_reserve(1, header.encode()).unwrap();
        reservation.write(&[7]).unwrap();
        reservation.commit(1).unwrap();
        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let (inbound, _received) = mpsc::channel(1);
        let budget = ByteBudget::new(1);
        let _held_byte = budget.try_charge(1).expect("hold the only byte");
        let capacity = capacity_fd(&rings);
        let root = CancellationToken::new();
        let read_cancel = CancellationToken::new();
        let receive = receive_one(
            &rings,
            &mut queue,
            &mut publisher,
            &capacity,
            &inbound,
            &budget,
            Duration::from_millis(400),
            &root,
            &read_cancel,
        );
        let returned = held.pop().unwrap();
        let return_after_poll = async move {
            tokio::task::yield_now().await;
            std::thread::spawn(move || drop(returned)).join().unwrap();
        };
        let (result, ()) = tokio::join!(receive, return_after_poll);
        assert!(
            matches!(result, Err(ReadClose::Overloaded)),
            "the budget never frees, so the wait still ends at the frame deadline"
        );
        assert_eq!(
            *published.lock().unwrap(),
            vec![(FrameType::StreamData, 7)],
            "the peer's return published the blocked ticket during the budget wait"
        );
        assert!(!publisher.has_pending());
        drop(held);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn control_frame_body_is_copied_out_of_the_ring() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let grant = rings.first.grant();
        let geometry = grant.geometry();
        assert_eq!(geometry.descriptor_depth(), 48);
        assert_eq!(geometry.ordinary_descriptors(), 32);
        assert_eq!(geometry.block_count(), 187);
        assert_eq!(
            grant.mapping_bytes(),
            ring_profile().mapping_bytes_per_direction()
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
                &mut Publisher::new(&rings.first, 1, Duration::from_secs(1), None),
                &capacity_fd(&rings),
                &inbound,
                &ByteBudget::new(1024),
                Duration::from_secs(1),
                &CancellationToken::new(),
                &CancellationToken::new(),
            )
            .await
            .unwrap()
        );
        let InboundEvent::Frame(frame) = received.recv().await.unwrap().unwrap() else {
            panic!("expected copied frame");
        };
        assert_eq!(frame.into_private().unwrap().body, body);
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
        // A held charge leaves the budget genuinely waiting; a zero-capacity budget would refuse the frame outright.
        let budget = ByteBudget::new(1);
        let _held = budget.try_charge(1).expect("hold the only byte");
        let mut publisher = Publisher::new(&rings.first, 1, Duration::from_secs(1), None);
        let capacity = capacity_fd(&rings);
        let receive = receive_one(
            &rings,
            &mut queue,
            &mut publisher,
            &capacity,
            &inbound,
            &budget,
            Duration::from_secs(1),
            &root,
            &cancellation,
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
        let budget = ByteBudget::new(1);
        let _held = budget.try_charge(1).expect("hold the only byte");
        let root = CancellationToken::new();
        let read_cancel = CancellationToken::new();
        let mut publisher = Publisher::new(&rings.first, 1, Duration::from_secs(1), None);
        let capacity = capacity_fd(&rings);
        let receive = receive_one(
            &rings,
            &mut queue,
            &mut publisher,
            &capacity,
            &inbound,
            &budget,
            Duration::from_secs(1),
            &root,
            &read_cancel,
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
        let depth = ring_profile().geometry().ordinary_descriptors() as usize;
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
        let depth = ring_profile().geometry().ordinary_descriptors() as usize;
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
            // Frames the peer filled before cancellation were already forwarded; after it, the
            // endpoint receives at most one descriptor depth more before reporting Cancelled.
            assert!(
                forwarded <= depth + ring_profile().descriptor_depth() as usize + 1,
                "a peer that refills every released slot must not postpone Cancelled past one descriptor depth"
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
        let depth = ring_profile().geometry().ordinary_descriptors() as usize;
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
        drop(peer);
        let accounting = settled_accounting(&transport, |accounting| {
            accounting.active == ResourceCharges::ZERO
        })
        .await;
        assert_eq!(
            accounting.active,
            ResourceCharges::ZERO,
            "a cancelled endpoint over a healthy ring refunds its admission once the peer releases"
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
            credit: None,
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
    async fn a_peer_still_attached_after_an_orderly_close_keeps_the_backing_charge_in_quarantine() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        // The host retires the generation, as a channel-0 Goodbye does, while the peer keeps
        // both rings attached.
        root.cancel();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits")
            .expect("endpoint task joins");
        assert_eq!(
            transport.accounting().unwrap().active.workers,
            1,
            "the worker thread is still alive while it waits for the peer to release"
        );
        let accounting = settled_accounting(&transport, |accounting| {
            accounting.active == ResourceCharges::ZERO
        })
        .await;
        assert_eq!(accounting.active, ResourceCharges::ZERO);
        assert_ne!(
            accounting.quarantined,
            ResourceCharges::ZERO,
            "the peer still maps both pools, so nothing is proved released"
        );
        drop(peer);
    }

    /// A backing whose charge moved to quarantine was not proved released, so the released
    /// counters must not count it when the host-side handles unmap.
    #[tokio::test]
    async fn a_quarantined_backing_is_not_counted_as_released() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let PreparedRing {
            descriptor,
            descriptors,
            io,
            root,
            ..
        } = transport
            .prepare(ByteBudget::new(1 << 20), 8, Duration::from_secs(1))
            .expect("ring prepares");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let io = tokio::spawn(io);
        root.cancel();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits")
            .expect("endpoint task joins");
        let accounting = settled_accounting(&transport, |accounting| {
            accounting.quarantined != ResourceCharges::ZERO
        })
        .await;
        assert_ne!(accounting.quarantined, ResourceCharges::ZERO);
        let snapshot = transport.return_snapshot();
        assert_eq!(
            snapshot.released_backings, 0,
            "a quarantined backing is still mapped by the peer and was never proved released"
        );
        assert_eq!(
            snapshot.live_backings, 0,
            "the registry no longer tracks it either"
        );
        assert_eq!(transport.diagnostics()["returns"]["released_backings"], 0);
        drop(peer);
    }

    #[test]
    fn a_doorbell_with_a_queued_token_ahead_of_end_of_file_still_reads_as_released() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let peer = rings.first.attachment().unwrap().attach().unwrap();
        assert_eq!(peer.arm_data_wait(), Ok(true), "the peer parks on data");
        let mut reservation = rings
            .first
            .try_reserve(1, shm_transport::backend::ring::wire_v3_header(1).unwrap())
            .unwrap();
        reservation.write(&[1]).unwrap();
        reservation.commit(1).unwrap();
        // The peer exits with the publication's token unread, which Linux reports to the host's
        // end as `ECONNRESET` on the first read and end-of-file on the next.
        drop(peer);
        assert!(doorbell_at_eof(&rings.first));
    }

    #[tokio::test]
    async fn a_lease_the_peer_keeps_after_closing_holds_the_backing_charge_in_quarantine() {
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
        let frame = OutboundFrame {
            bytes: crate::wire::encode_frame(
                FrameType::Response,
                Flags::new(false, Priority::Interactive, false),
                crate::wire::FrameId {
                    channel: 7,
                    epoch: 1,
                    corr: 1,
                },
                &[9],
            )
            .expect("frame encodes"),
            tail: Vec::new(),
            direct: None,
            charge: crate::wire::ByteCharge::none(),
            written: None,
            credit: None,
        };
        sender.send(frame).await.expect("frame admits");
        let deadline = StdInstant::now() + Duration::from_secs(1);
        let held = loop {
            if let Some(lease) = peer.from_host.try_receive().expect("receive") {
                break lease;
            }
            assert!(
                peer.from_host.wait_for_data(deadline).expect("wait"),
                "frame arrives"
            );
        };
        let RingClientEndpoint { to_host, from_host } = peer;
        drop(to_host);
        drop(from_host);
        assert!(matches!(receiver.recv().await, Err(ReadClose::Corrupt(_))));
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("endpoint exits after the peer closes")
            .expect("endpoint task joins");
        let accounting = settled_accounting(&transport, |accounting| {
            accounting.quarantined != ResourceCharges::ZERO
        })
        .await;
        assert_eq!(held.to_vec().expect("copy"), [9]);
        assert_ne!(
            accounting.quarantined,
            ResourceCharges::ZERO,
            "the peer still maps the host-to-peer pool through its lease"
        );
        held.release().expect("release");
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

        let accounting = settled_accounting(&transport, |accounting| {
            accounting.active == ResourceCharges::ZERO
        })
        .await;
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
        let budget = ByteBudget::new(1);
        let _held = budget.try_charge(1).expect("hold the only byte");
        let PreparedRing {
            descriptor,
            descriptors,
            sender: _sender,
            receiver: _receiver,
            io,
            root,
            ..
        } = transport
            .prepare(budget.clone(), 8, Duration::from_secs(30))
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
        // The held byte parks the endpoint in the ingress wait for this frame.
        tokio::time::sleep(Duration::from_millis(100)).await;
        root.cancel();
        tokio::time::timeout(Duration::from_secs(1), io)
            .await
            .expect("root cancellation must end a budget wait well before the frame deadline")
            .expect("endpoint task joins");
        drop(peer);
        let accounting = settled_accounting(&transport, |accounting| {
            accounting.active == ResourceCharges::ZERO
        })
        .await;
        assert_eq!(accounting.active, ResourceCharges::ZERO);
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
        let reservation = rings
            .first
            .try_reserve(1, direct.header())
            .expect("reservation");
        assert!(
            publish_direct(reservation, direct, 1, deadline).is_err(),
            "a serializer that finishes after the deadline must not publish"
        );
        let attached = rings.first.attachment().unwrap().attach().unwrap();
        assert!(
            attached.try_receive().unwrap().is_none(),
            "the aborted reservation leaves no frame in the ring"
        );
    }

    #[test]
    fn a_client_send_past_its_frame_deadline_publishes_nothing() {
        // Capacity is free, so the reservation succeeds although the frame deadline has passed.
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let (descriptor, descriptors) = worker_descriptor(&rings).expect("descriptor");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let header = EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr: 1,
        };
        let now = StdInstant::now();
        assert_eq!(peer.send(header, &[1], now), Err(SendFailure::Deadline));
        assert!(
            rings.second.try_receive().unwrap().is_none(),
            "the aborted reservation leaves no frame in the ring"
        );
        let live_deadline = StdInstant::now() + Duration::from_secs(1);
        peer.send(header, &[1], live_deadline)
            .expect("a live frame deadline publishes");
        assert!(rings.second.try_receive().unwrap().is_some());
    }

    /// Both client publication paths classify a frame's inventory the same way: with ordinary
    /// descriptor headroom exhausted, a pure-header `Pong` publishes from the control reserve
    /// through `send` as it does through `try_send_bounded`, while a data frame stays refused.
    #[test]
    fn client_send_and_try_send_share_the_frame_inventory() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let (descriptor, descriptors) = worker_descriptor(&rings).expect("descriptor");
        let peer = RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors)
            .expect("peer attaches");
        let request = |corr| EnvelopeHeader {
            len: 1,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr,
        };
        let deadline = StdInstant::now() + Duration::from_millis(200);
        let ordinary = u64::from(rings.second.geometry().ordinary_descriptors());
        for corr in 1..=ordinary {
            assert_eq!(
                peer.try_send_bounded(request(corr), &[1], deadline),
                Ok(TrySend::Published)
            );
        }
        assert_eq!(
            peer.try_send_bounded(request(ordinary + 1), &[1], deadline),
            Ok(TrySend::Exhausted),
            "ordinary descriptor headroom is exhausted"
        );
        let pong = EnvelopeHeader {
            len: 0,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Pong,
            flags: crate::wire::pure_header_flags(),
            channel: 0,
            epoch: 1,
            corr: 99,
        };
        peer.send(pong, &[], deadline)
            .expect("a control publishes from its reserve past exhausted ordinary headroom");
        assert_eq!(
            peer.send(request(ordinary + 1), &[1], deadline),
            Err(SendFailure::Deadline),
            "a data frame still waits on ordinary headroom until its deadline"
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

        let accounting = settled_accounting(&transport, |accounting| {
            accounting.active == ResourceCharges::ZERO
        })
        .await;
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

    fn frame(ty: FrameType, channel: u16, corr: u64, body: &[u8]) -> OutboundFrame {
        let flags = if body.is_empty() && ty.is_pure_header() {
            crate::wire::pure_header_flags()
        } else {
            Flags::new(false, Priority::Interactive, ty != FrameType::StreamData)
        };
        let id = if channel == 0 {
            crate::wire::FrameId::control(corr)
        } else {
            crate::wire::FrameId::routed(crate::handler::RouteHandle { channel, epoch: 1 }, corr)
        };
        let (bytes, tail) =
            crate::wire::encode_split_frame(ty, flags, id, body.to_vec()).expect("frame encodes");
        OutboundFrame {
            bytes,
            tail,
            direct: None,
            charge: crate::wire::ByteCharge::none(),
            written: None,
            credit: None,
        }
    }

    /// Holds every block of the smallest ordinary class on the consumer side so an ordinary
    /// frame of that class blocks while descriptors and reserves stay free.
    fn exhaust_smallest_class(
        producer: &Ring,
        consumer: &Ring,
    ) -> Vec<shm_transport::lease::PayloadLease> {
        let count = producer
            .geometry()
            .class(shm_transport::pool::BlockClass::Ordinary(0))
            .count;
        (0..count)
            .map(|_| {
                let mut reservation = producer
                    .try_reserve(1, shm_transport::backend::ring::wire_v3_header(1).unwrap())
                    .expect("fill reservation");
                reservation.write(&[1]).unwrap();
                reservation.commit(1).unwrap();
                consumer.try_receive().unwrap().expect("filled frame")
            })
            .collect()
    }

    /// A valid `writer_queue_frames` may be as large as `Semaphore::MAX_PERMITS`; the pending
    /// set must not allocate that depth up front.
    #[test]
    fn a_publisher_does_not_preallocate_its_configured_depth() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let publisher = Publisher::new(
            &rings.first,
            tokio::sync::Semaphore::MAX_PERMITS,
            Duration::from_secs(1),
            None,
        );
        assert!(publisher.can_accept());
        assert_eq!(publisher.pending.capacity(), 0);
    }

    /// A failed return doorbell makes `into_private` return `PrivateCopyError::Transport`.
    #[test]
    fn into_private_reports_a_failed_return_wake_as_a_transport_error() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let mut held = exhaust_smallest_class(&rings.first, &consumer);
        let lease = held.pop().unwrap();
        assert_eq!(
            rings.first.arm_capacity_wait(Inventory::Ordinary, 1),
            Ok(true),
            "the producer parks, so the return must ring the doorbell"
        );
        // Dropping the producer closes its doorbell ends; the return's wake now fails.
        drop(rings);
        let header = EnvelopeHeader {
            len: lease.len() as u32,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        let budget = ByteBudget::new(16);
        let charge = budget.try_charge(lease.len()).unwrap();
        let frame = InboundFrame::new(header, lease, charge);
        assert_eq!(
            frame.into_private().err(),
            Some(crate::frame_channel::PrivateCopyError::Transport),
            "a failed return wake is a transport failure, not a private frame"
        );
        drop(held);
    }

    #[test]
    fn eligible_controls_and_unrelated_terminals_publish_past_a_blocked_ordinary_ticket() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let held = exhaust_smallest_class(&rings.first, &consumer);
        let published = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&published);
        let hook: PublishHook = Arc::new(move |ty, channel| {
            observed.lock().unwrap().push((ty, channel));
        });
        let mut publisher = Publisher::new(&rings.first, 16, Duration::from_secs(5), Some(hook));
        // Ordinary data of the exhausted class blocks at the head.
        publisher.push(frame(FrameType::StreamData, 7, 1, b"data"));
        // Behind it: a Ping, an unrelated error terminal, this stream's own terminal, a second
        // unrelated stream's prefix and terminal, and Goodbye.
        publisher.push(frame(FrameType::Ping, 0, 9, &[]));
        publisher.push(frame(
            FrameType::Error,
            7,
            2,
            br#"{"code":"x","message":"y"}"#,
        ));
        publisher.push(frame(FrameType::StreamEnd, 7, 1, &[]));
        publisher.push(frame(FrameType::StreamData, 7, 3, b"more"));
        publisher.push(frame(FrameType::StreamEnd, 7, 3, &[]));
        publisher.push(frame(FrameType::Goodbye, 0, 0, &[]));
        publisher.pump(&rings.first).expect("pump");
        assert_eq!(
            *published.lock().unwrap(),
            vec![(FrameType::Ping, 0), (FrameType::Error, 7)],
            "only the Ping and the unrelated terminal bypass; a terminal never skips its own \
             prefix and Goodbye follows every admitted frame"
        );
        assert!(publisher.has_pending());
        // Returning one block from another thread frees the head; everything drains in order.
        let mut held = held;
        let returned = held.pop().unwrap();
        std::thread::spawn(move || drop(returned)).join().unwrap();
        publisher.pump(&rings.first).expect("pump after return");
        assert_eq!(
            published.lock().unwrap()[2..],
            [(FrameType::StreamData, 7), (FrameType::StreamEnd, 7)],
            "one returned block admits exactly the blocked head; its terminal then follows              from the terminal reserve, while the next stream's data blocks again"
        );
        assert!(publisher.has_pending());
        drop(held.pop());
        publisher.pump(&rings.first).expect("pump");
        assert_eq!(
            published.lock().unwrap()[4..],
            [
                (FrameType::StreamData, 7),
                (FrameType::StreamEnd, 7),
                (FrameType::Goodbye, 0)
            ],
            "the remaining frames keep admission order once capacity returns"
        );
        assert!(!publisher.has_pending());
        drop(held);
    }

    #[test]
    fn an_unreserved_direct_serializer_never_runs_and_a_reserved_one_runs_once() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let held = exhaust_smallest_class(&rings.first, &consumer);
        let runs = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&runs);
        let header = EnvelopeHeader {
            len: 3,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Response,
            flags: Flags::new(false, Priority::Interactive, true),
            channel: 7,
            epoch: 1,
            corr: 4,
        };
        let direct = DirectFrame::new(
            header,
            3,
            Box::new(move |writer| {
                counted.fetch_add(1, Ordering::SeqCst);
                writer.write_all(b"abc")
            }),
        );
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_secs(5), None);
        publisher.push(OutboundFrame {
            bytes: Vec::new(),
            tail: Vec::new(),
            direct: Some(direct),
            charge: crate::wire::ByteCharge::none(),
            written: None,
            credit: None,
        });
        for _ in 0..3 {
            publisher.pump(&rings.first).expect("blocked pump");
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "no reservation, no serialization"
        );
        drop(held);
        publisher.pump(&rings.first).expect("pump after returns");
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "one reservation, one serialization"
        );
        let lease = consumer.try_receive().unwrap().expect("published");
        assert_eq!(lease.to_vec().unwrap(), b"abc");
    }

    #[test]
    fn a_terminal_credit_returns_with_its_block_not_with_settlement() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let credits = Arc::new(tokio::sync::Semaphore::new(1));
        let credit = credits.clone().try_acquire_owned().unwrap();
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_secs(5), None);
        let mut terminal = frame(FrameType::Error, 7, 1, br#"{"code":"x","message":"y"}"#);
        terminal.credit = Some(credit);
        publisher.push(terminal);
        publisher.pump(&rings.first).expect("publish");
        assert_eq!(
            credits.available_permits(),
            0,
            "published, still held by the peer"
        );
        let lease = consumer.try_receive().unwrap().expect("terminal");
        publisher.pump(&rings.first).expect("pump while held");
        assert_eq!(
            credits.available_permits(),
            0,
            "consumed, still not returned"
        );
        lease.release().unwrap();
        publisher.pump(&rings.first).expect("pump after return");
        assert_eq!(
            credits.available_permits(),
            1,
            "the block returned, so the credit did"
        );
    }

    /// A second terminal reuses the block the first one returned; the credit on the second
    /// frame stays held until the peer releases that publication.
    #[test]
    fn a_credit_on_a_reused_block_waits_for_the_new_publication_to_return() {
        enum Peer {
            ReceiveAndRelease,
            ReceiveAndHold,
            ReleaseHeld,
        }
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let attachment = rings.first.attachment().unwrap();
        let (command_tx, command_rx) = std::sync::mpsc::channel::<Peer>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            let consumer = attachment.attach().unwrap();
            let mut held = None;
            while let Ok(command) = command_rx.recv() {
                match command {
                    Peer::ReceiveAndRelease => {
                        consumer.try_receive().unwrap().unwrap().release().unwrap();
                    }
                    Peer::ReceiveAndHold => {
                        held = Some(consumer.try_receive().unwrap().unwrap());
                    }
                    Peer::ReleaseHeld => {
                        held.take().unwrap().release().unwrap();
                    }
                }
                done_tx.send(()).unwrap();
            }
        });
        let publications = Arc::new(AtomicU64::new(0));
        let hook_publications = Arc::clone(&publications);
        let hook_commands = command_tx.clone();
        let hook_done = Arc::new(Mutex::new(done_rx));
        let hook_wait = Arc::clone(&hook_done);
        let hook: PublishHook = Arc::new(move |_, _| {
            let command = if hook_publications.fetch_add(1, Ordering::SeqCst) == 0 {
                Peer::ReceiveAndRelease
            } else {
                Peer::ReceiveAndHold
            };
            hook_commands.send(command).unwrap();
            hook_wait.lock().unwrap().recv().unwrap();
        });
        let credits = Arc::new(tokio::sync::Semaphore::new(2));
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_secs(5), Some(hook));
        for corr in 1..=2 {
            let mut terminal = frame(FrameType::Error, 7, corr, br#"{"code":"x","message":"y"}"#);
            terminal.credit = Some(credits.clone().try_acquire_owned().unwrap());
            publisher.push(terminal);
        }
        publisher.pump(&rings.first).expect("publish both");
        assert_eq!(publications.load(Ordering::SeqCst), 2);
        assert_eq!(
            credits.available_permits(),
            1,
            "the first block returned; the second is held by the peer"
        );
        publisher
            .pump(&rings.first)
            .expect("pump while the reused block is held");
        assert_eq!(
            credits.available_permits(),
            1,
            "the reused block's earlier return must not release the credit on its new frame"
        );
        command_tx.send(Peer::ReleaseHeld).unwrap();
        hook_done.lock().unwrap().recv().unwrap();
        publisher.pump(&rings.first).expect("pump after the return");
        assert_eq!(credits.available_permits(), 2);
        // The hook holds the last command sender; dropping the publisher ends the peer loop.
        drop(publisher);
        drop(command_tx);
        peer.join().unwrap();
    }

    #[test]
    fn a_pending_ticket_past_its_deadline_retires_instead_of_waiting() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let consumer = rings.first.attachment().unwrap().attach().unwrap();
        let held = exhaust_smallest_class(&rings.first, &consumer);
        let mut publisher = Publisher::new(&rings.first, 4, Duration::from_millis(10), None);
        publisher.push(frame(FrameType::StreamData, 7, 1, b"late"));
        publisher.pump(&rings.first).expect("blocked pump");
        std::thread::sleep(Duration::from_millis(30));
        assert!(publisher.pump(&rings.first).is_err());
        drop(held);
    }

    #[test]
    fn inventory_classification_reserves_controls_and_small_terminals_only() {
        let capacity = 32 * 1024 - 21;
        let header = |ty, len| EnvelopeHeader {
            len,
            ver: PROTOCOL_VERSION,
            ty,
            flags: Flags::new(false, Priority::Interactive, true),
            channel: 0,
            epoch: 0,
            corr: 1,
        };
        assert_eq!(
            inventory_for(&header(FrameType::Ping, 0), 0, capacity),
            Inventory::Control
        );
        assert_eq!(
            inventory_for(&header(FrameType::Goodbye, 0), 0, capacity),
            Inventory::Control
        );
        assert_eq!(
            inventory_for(&header(FrameType::Error, 100), 100, capacity),
            Inventory::Terminal
        );
        assert_eq!(
            inventory_for(&header(FrameType::StreamEnd, 0), 0, capacity),
            Inventory::Terminal
        );
        assert_eq!(
            inventory_for(&header(FrameType::Error, 40_000), 40_000, capacity),
            Inventory::Ordinary,
            "a terminal past the reserve falls back to ordinary capacity"
        );
        assert_eq!(
            inventory_for(&header(FrameType::Request, 0), 0, capacity),
            Inventory::Ordinary,
            "a channel-0 Request is never a bypass control"
        );
        assert_eq!(
            inventory_for(&header(FrameType::Response, 5), 5, capacity),
            Inventory::Ordinary
        );
    }

    /// A private copy running on the blocking barrier keeps the transport block and its ingress
    /// charge held through Cancel, route close, and host shutdown; both return exactly once, after
    /// the copy physically completes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn barrier_held_copy_returns_block_and_charge_once_after_physical_completion() {
        let rings = DuplexRing::create(&ring_profile()).unwrap();
        let body = vec![0xA5u8; 3_000];
        let header = EnvelopeHeader {
            len: body.len() as u32,
            ver: PROTOCOL_VERSION,
            ty: FrameType::Request,
            flags: Flags::new(false, Priority::Interactive, false),
            channel: 7,
            epoch: 1,
            corr: 1,
        };
        let mut reservation = rings
            .second
            .try_reserve(body.len(), header.encode())
            .unwrap();
        reservation.write(&body).unwrap();
        reservation.commit(body.len()).unwrap();

        let budget = ByteBudget::new(8 * 1024);
        let (_sender, mut queue) =
            frame_sender(1, CancellationToken::new(), Duration::from_secs(1));
        let (inbound, mut received) = mpsc::channel(1);
        assert!(
            receive_one(
                &rings,
                &mut queue,
                &mut Publisher::new(&rings.first, 1, Duration::from_secs(1), None),
                &capacity_fd(&rings),
                &inbound,
                &budget,
                Duration::from_secs(1),
                &CancellationToken::new(),
                &CancellationToken::new(),
            )
            .await
            .unwrap()
        );
        let InboundEvent::Frame(frame) = received.recv().await.unwrap().unwrap() else {
            panic!("expected a leased frame");
        };
        let retained = Arc::clone(rings.second.retained());
        assert_eq!(retained.outstanding_returns(), 1);
        let charged = budget.capacity() - budget.available();
        assert_eq!(
            charged,
            body.len(),
            "the ingress charge covers the private copy"
        );

        let ledgers = crate::handler::WorkLedgers {
            request: tokio_util::task::TaskTracker::new(),
            route: tokio_util::task::TaskTracker::new(),
            host: tokio_util::task::TaskTracker::new(),
        };
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let copy = ledgers.run_blocking(move || {
            let _ = started_tx.send(());
            let _ = gate_rx.recv();
            frame.into_private()
        });
        started_rx.await.unwrap();

        // Cancel, route close, and shutdown all arrive while the copy is still reading.
        ledgers.request.close();
        ledgers.route.close();
        ledgers.host.close();
        for tracker in [&ledgers.request, &ledgers.route, &ledgers.host] {
            assert!(
                tokio::time::timeout(Duration::from_millis(100), tracker.wait())
                    .await
                    .is_err(),
                "every ledger waits for the held copy"
            );
        }
        assert_eq!(
            retained.outstanding_returns(),
            1,
            "the block stays held while the copy is in flight"
        );
        assert_eq!(
            budget.capacity() - budget.available(),
            charged,
            "the charge is not refunded early"
        );

        gate_tx.send(()).unwrap();
        let private = copy.await.expect("copy joined").expect("copy succeeded");
        assert_eq!(private.body, body);
        for tracker in [&ledgers.request, &ledgers.route, &ledgers.host] {
            tokio::time::timeout(Duration::from_secs(5), tracker.wait())
                .await
                .expect("ledgers drain once the copy completes");
        }
        assert_eq!(
            retained.outstanding_returns(),
            0,
            "the block returns with the completed copy"
        );
        assert!(
            rings.second.try_receive().unwrap().is_none(),
            "no second frame appears from the returned block"
        );
        assert_eq!(
            budget.capacity() - budget.available(),
            charged,
            "the charge follows the private bytes, not the transport block"
        );
        drop(private);
        assert_eq!(
            budget.available(),
            budget.capacity(),
            "the charge returns exactly once, with the private bytes"
        );
    }

    /// Refusals name the resource that ran out. Limits for one connection admit the first
    /// `prepare` and refuse the second on the first field checked, and the refusal charges
    /// nothing: the live connection's accounting is unchanged and a third attempt after release
    /// succeeds.
    #[test]
    fn refusals_are_counted_by_exhausted_resource_and_charge_nothing() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let first = transport
            .prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(1))
            .expect("first connection admits");
        let active_before = transport.diagnostics()["accounting"]["active"].clone();
        assert!(
            transport
                .prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(1))
                .is_err(),
            "a second connection exceeds one connection's limits"
        );
        let diagnostics = transport.diagnostics();
        assert_eq!(diagnostics["exhaustion"]["observed"], 1);
        assert_eq!(
            diagnostics["exhaustion"]["by_resource"],
            serde_json::json!({"descriptors": 1}),
            "descriptors are the first limit checked, so the refusal is attributed there"
        );
        assert_eq!(
            diagnostics["accounting"]["active"], active_before,
            "a refusal charges nothing"
        );

        let PreparedRing {
            sender,
            io,
            root,
            descriptors,
            ..
        } = first;
        // No peer ever attached; closing its doorbell ends is what proves the release.
        drop(descriptors);
        root.cancel();
        drop(sender);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(io);
        // Backing charges end with the last owned lease; the endpoint held none, so the
        // returned charge admits the next connection once the thread proves the peer released.
        wait_until(|| transport.accounting().unwrap().active == ResourceCharges::ZERO);
        let recovered = transport.prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(1));
        assert!(
            recovered.is_ok(),
            "recovery follows the exhausted resource's own release"
        );
        assert_eq!(transport.diagnostics()["exhaustion"]["observed"], 1);
    }

    #[test]
    fn ended_connections_leave_no_dead_backing_entries_without_a_status_request() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for _ in 0..3 {
            let PreparedRing {
                sender, io, root, ..
            } = transport
                .prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(1))
                .expect("connection admits");
            root.cancel();
            drop(sender);
            runtime.block_on(io);
            wait_until(|| transport.accounting().unwrap().active == ResourceCharges::ZERO);
        }
        assert_eq!(
            transport.backings.entries.lock().unwrap().len(),
            0,
            "ended connections must not accumulate dead backing entries"
        );
        let snapshot = transport.return_snapshot();
        assert_eq!(snapshot.live_backings, 0);
        assert_eq!(snapshot.released_backings, 6);
    }

    #[test]
    fn return_snapshot_separates_outstanding_leases_from_released_backing() {
        let transport = RingTransport::for_ring_profile(per_connection_limits());
        // The outbound sender keeps the endpoint thread, and with it both backings, alive.
        let PreparedRing {
            descriptor,
            descriptors,
            sender,
            receiver,
            io,
            root,
            read_cancel: _,
        } = transport
            .prepare(ByteBudget::new(1 << 20), 4, Duration::from_secs(1))
            .expect("ring prepares");
        let peer =
            RingClientEndpoint::attach_with_descriptors(&descriptor, descriptors).expect("peer");
        let before = transport.return_snapshot();
        assert_eq!(before.live_backings, 2);
        assert_eq!(before.released_backings, 0);
        assert_eq!(before.outstanding, 0);
        // The host's rings and this peer's rings share backing; one held lease on the peer side
        // is not counted here because the registry tracks host backings only.
        transport.record_reclamation();
        assert_eq!(
            transport.diagnostics()["reclamation"]["completed"],
            1,
            "generation ends are counted separately"
        );
        assert_eq!(transport.diagnostics()["returns"]["outstanding"], 0);
        root.cancel();
        drop(sender);
        drop(peer);
        drop(receiver);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(io);
        wait_until(|| transport.return_snapshot().live_backings == 0);
        let after = transport.return_snapshot();
        assert_eq!(
            after.live_backings, 0,
            "both backings unmapped with the endpoint"
        );
        assert_eq!(after.released_backings, 2);
        assert_eq!(
            after.released_backing_bytes,
            2 * ring_profile().mapping_bytes_per_direction()
        );
    }
}
