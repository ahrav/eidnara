//! Single-producer, single-consumer payload pool over one sealed memfd.
//!
//! One `Ring` is one direction. The mapping holds control pages (producer, consumer, two wake
//! epochs, lifecycle), a ring of fixed-width descriptor slots, one completion cell per block,
//! and an arena of fixed blocks grouped in classes. The producer takes a free block from the
//! smallest fitting class, writes the complete frame (header first) into it, publishes a
//! descriptor naming the block and its reuse generation, and rings the data doorbell. The
//! consumer copies the descriptor out under Acquire, validates it against its own geometry and
//! per-block records, acknowledges consumption, and hands out an owned `PayloadLease`. The
//! lease's final owner publishes the generation into the block's completion cell with Release
//! and rings the capacity doorbell; the producer observes the cell with Acquire and returns
//! the block to its class free list.
//!
//! Descriptor consumption and payload return are independent: a consumed slot is reusable
//! while its payload stays held, and a returned block is reusable while older payloads stay
//! held. There is no oldest-release frontier and no page removal; blocks are reused in place.
//!
//! Both peers can write the mapping. Every value read from it is treated as untrusted:
//! descriptors are copied out then validated, and cursors are checked for monotonicity and
//! depth. Impossible shared-memory state quarantines the ring; the local latch keeps
//! quarantine terminal if a peer clears the shared flag.
//!
//! `Ring` and `ProducerReservation` are confined to one thread; `PayloadLease` is not:
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<shm_transport::backend::ring::Ring>();
//! ```
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<shm_transport::backend::ring::ProducerReservation<'static>>();
//! ```
//!
//! ```
//! fn assert_send<T: Send>() {}
//! assert_send::<shm_transport::lease::PayloadLease>();
//! ```
#[cfg(not(target_os = "linux"))]
compile_error!("shm-transport ring backend supports Linux only");

use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::backend::retained::{
    LAYOUT_VERSION, Mapping, MappingError, Retained, WakeEpoch, initialize_mapping, seal_object,
    system_page_size, validate_lifecycle, validate_object_len,
};
use crate::backend::sys;
use crate::descriptor::{
    DESCRIPTOR_SCHEMA_VERSION, DescriptorError, Incarnation, PayloadIdentity, PoolDescriptor,
    WIRE_V3_HEADER_BYTES, WIRE_V3_VERSION, check_wire_header,
};
use crate::lease::{LeaseError, LeaseSpan, PayloadLease, copy_in, copy_out};
use crate::pool::{
    BlockClass, CLASS_COUNT, ClassSpec, GeometryError, Inventory, MAX_FRAME_BYTES, MappingLayout,
    ORDINARY_CLASSES, PoolGeometry,
};
use crate::profile::{BackingAdmission, TargetProfile};

/// Encoded grant length: layout version, incarnation, lane, two descriptor depths, seven
/// class specs, total bytes, and a zero reserved tail.
pub const GRANT_BYTES: usize = 2 + 16 + 4 + 4 + 4 + CLASS_COUNT * (8 + 4) + 8 + 4;

/// Marks a wake epoch parked for one generation and clears the marker on drop, so every exit
/// from a park loop, including `?` and `continue`, unparks.
struct ParkGuard<'a>(&'a WakeEpoch);

impl<'a> ParkGuard<'a> {
    /// Records the current generation and stores a nonzero `parked` bound to it.
    fn arm(wake: &'a WakeEpoch) -> (u64, Self) {
        let generation = wake.generation.load(Ordering::SeqCst);
        wake.parked
            .store(generation.wrapping_add(1), Ordering::SeqCst);
        (generation, Self(wake))
    }
}

impl Drop for ParkGuard<'_> {
    fn drop(&mut self) {
        self.0.parked.store(0, Ordering::Release);
    }
}

/// Syscalls this process has issued through a ring handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyscallCounters {
    /// `send`, `recv`, and `poll` calls on either doorbell from this handle.
    pub doorbell: u64,
    /// Blocking doorbell `poll` calls issued after a zero-timeout probe found nothing ready;
    /// each is one park/wake transition.
    pub parks: u64,
    /// Capacity wake tokens sent from the retained backing, including tokens a lease's final
    /// drop sent from another thread.
    pub retained_wakes: u64,
}

impl SyscallCounters {
    /// Every counted syscall; `parks` is a subset of `doorbell`.
    pub const fn total(self) -> u64 {
        self.doorbell.wrapping_add(self.retained_wakes)
    }

    /// Field-wise sum.
    pub const fn add(self, other: Self) -> Self {
        Self {
            doorbell: self.doorbell.wrapping_add(other.doorbell),
            parks: self.parks.wrapping_add(other.parks),
            retained_wakes: self.retained_wakes.wrapping_add(other.retained_wakes),
        }
    }

    /// Field-wise difference, saturating at zero.
    pub const fn since(self, earlier: Self) -> Self {
        Self {
            doorbell: self.doorbell.saturating_sub(earlier.doorbell),
            parks: self.parks.saturating_sub(earlier.parks),
            retained_wakes: self.retained_wakes.saturating_sub(earlier.retained_wakes),
        }
    }
}

/// Which of the two wake channels a `Doorbell` drives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wake {
    Data,
    Capacity,
}

/// One wake channel between the peers, built on a `socketpair`. Each socketpair endpoint has a
/// separate open file description, so peer-set status flags such as `O_NONBLOCK` cannot
/// affect the local end. `MSG_DONTWAIT` prevents blocking regardless of status flags, and
/// `MSG_NOSIGNAL` keeps a closed peer end from raising `SIGPIPE`. The local end lives in the
/// retained backing so a lease's final drop can ring the capacity channel from any thread.
struct Doorbell {
    retained: Arc<Retained>,
    wake: Wake,
    /// The peer's end. `attachment` moves it out, so after the handoff only the peer holds
    /// that end and its exit is visible here as EOF or `EPIPE`.
    remote: Cell<Option<OwnedFd>>,
    counters: Cell<SyscallCounters>,
}

/// Each `drain` call consumes at most this many bytes; remaining bytes only cause a spurious
/// wake, so a flooding peer cannot keep `drain` spinning.
const DRAIN_BYTES: usize = 256;

/// Creates one connected stream socketpair with both ends nonblocking.
fn doorbell_pair() -> Result<(UnixStream, OwnedFd), RingError> {
    let (local, remote) = UnixStream::pair().map_err(|_| RingError::DoorbellFailed)?;
    local
        .set_nonblocking(true)
        .and_then(|()| remote.set_nonblocking(true))
        .map_err(|_| RingError::DoorbellFailed)?;
    Ok((local, remote.into()))
}

/// Accepts only a connected `AF_UNIX` stream socket. `UnixStream` itself admits any fd, and
/// `peer_addr` proves only that the peer is `AF_UNIX`, so the socket type is checked here:
/// `drain` reads a zero-length `recv` as peer close, which a datagram or seqpacket socket
/// makes ambiguous, and an eventfd is not a socket at all.
fn doorbell_from_fd(fd: OwnedFd) -> Result<UnixStream, RingError> {
    if sys::socket_type(fd.as_fd()).map_err(|_| RingError::DoorbellFailed)? != libc::SOCK_STREAM {
        return Err(RingError::DoorbellFailed);
    }
    let local = UnixStream::from(fd);
    local.peer_addr().map_err(|_| RingError::DoorbellFailed)?;
    Ok(local)
}

impl Doorbell {
    fn new(retained: Arc<Retained>, wake: Wake, remote: Option<OwnedFd>) -> Self {
        Self {
            retained,
            wake,
            remote: Cell::new(remote),
            counters: Cell::new(SyscallCounters::default()),
        }
    }

    fn local(&self) -> &UnixStream {
        match self.wake {
            Wake::Data => self.retained.data_signal(),
            Wake::Capacity => self.retained.capacity_signal(),
        }
    }

    fn counters(&self) -> SyscallCounters {
        self.counters.get()
    }

    fn record(&self, parked: bool) {
        let mut counters = self.counters.get();
        counters.doorbell = counters.doorbell.wrapping_add(1);
        if parked {
            counters.parks = counters.parks.wrapping_add(1);
        }
        self.counters.set(counters);
    }

    fn duplicate(&self) -> Result<OwnedFd, RingError> {
        self.local()
            .try_clone()
            .map(OwnedFd::from)
            .map_err(|_| RingError::DoorbellFailed)
    }

    fn take_peer_end(&self) -> Result<OwnedFd, RingError> {
        self.remote.take().ok_or(RingError::DoorbellFailed)
    }

    /// `EAGAIN` means the peer already has unread wake bytes, which is the same outcome.
    fn signal(&self) -> Result<(), RingError> {
        let token = [1u8];
        loop {
            self.record(false);
            let error = match sys::send_token(self.local().as_fd(), &token) {
                Ok(sent) if sent == token.len() => return Ok(()),
                Ok(_) => return Err(RingError::DoorbellFailed),
                Err(error) => error,
            };
            match error.kind() {
                std::io::ErrorKind::WouldBlock => return Ok(()),
                std::io::ErrorKind::Interrupted => continue,
                _ => return Err(RingError::DoorbellFailed),
            }
        }
    }

    /// A zero-length read means the peer closed its end, which ends the channel.
    fn drain(&self) -> Result<(), RingError> {
        let mut buffer = [0u8; DRAIN_BYTES];
        loop {
            self.record(false);
            let error = match sys::recv_tokens(self.local().as_fd(), &mut buffer) {
                Ok(0) => return Err(RingError::DoorbellFailed),
                Ok(_) => return Ok(()),
                Err(error) => error,
            };
            match error.kind() {
                std::io::ErrorKind::WouldBlock => return Ok(()),
                std::io::ErrorKind::Interrupted => continue,
                _ => return Err(RingError::DoorbellFailed),
            }
        }
    }

    fn wait_until(&self, deadline: Instant) -> Result<bool, RingError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let timeout = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        // A signal that landed between the caller's last recheck and this call makes the
        // blocking poll return at once; the probe keeps such a call out of `parks`.
        self.record(false);
        if sys::poll_readable(self.local().as_fd(), 0).unwrap_or(false) {
            return Ok(true);
        }
        self.record(true);
        match sys::poll_readable(self.local().as_fd(), timeout) {
            Ok(ready) => Ok(ready),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                Ok(Instant::now() < deadline)
            }
            Err(_) => Err(RingError::DoorbellFailed),
        }
    }
}

/// Everything a peer needs to attach: layout version, incarnation, lane, and geometry. Sent
/// over the authenticated setup channel alongside the file descriptors; `decode` refuses any
/// grant whose geometry does not map to a valid pool.
///
/// The hardware profile id is excluded from this encoding. The setup layer validates the
/// hardware profile id before decoding any grant.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PoolGrant {
    layout_version: u16,
    incarnation: Incarnation,
    lane: u32,
    geometry: PoolGeometry,
    total_bytes: u64,
}

impl PoolGrant {
    /// Serializes to `GRANT_BYTES` little-endian bytes with a zero reserved tail.
    pub fn encode(self) -> [u8; GRANT_BYTES] {
        let mut bytes = [0u8; GRANT_BYTES];
        bytes[0..2].copy_from_slice(&self.layout_version.to_le_bytes());
        bytes[2..18].copy_from_slice(&self.incarnation.into_bytes());
        bytes[18..22].copy_from_slice(&self.lane.to_le_bytes());
        bytes[22..26].copy_from_slice(&self.geometry.ordinary_descriptors().to_le_bytes());
        bytes[26..30].copy_from_slice(&self.geometry.reserved_descriptors().to_le_bytes());
        let mut cursor = 30;
        for class in self.geometry.classes() {
            bytes[cursor..cursor + 8].copy_from_slice(&class.block_bytes.to_le_bytes());
            bytes[cursor + 8..cursor + 12].copy_from_slice(&class.count.to_le_bytes());
            cursor += 12;
        }
        bytes[cursor..cursor + 8].copy_from_slice(&self.total_bytes.to_le_bytes());
        cursor += 8;
        bytes[cursor..cursor + 4].copy_from_slice(&0u32.to_le_bytes());
        bytes
    }

    /// Parses `encode` output. Rejects a nonzero reserved tail and any geometry that cannot
    /// map a valid pool: wrong layout version, an invalid class table, or a total size that
    /// disagrees with the computed layout.
    pub fn decode(bytes: [u8; GRANT_BYTES]) -> Result<Self, RingError> {
        let u32_at = |offset: usize| -> u32 {
            u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("grant u32 ranges have fixed width"),
            )
        };
        let u64_at = |offset: usize| -> u64 {
            u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .expect("grant u64 ranges have fixed width"),
            )
        };
        let layout_version = u16::from_le_bytes([bytes[0], bytes[1]]);
        let incarnation = Incarnation::from_bytes(
            bytes[2..18]
                .try_into()
                .expect("grant incarnation has fixed width"),
        );
        let lane = u32_at(18);
        let ordinary_descriptors = u32_at(22);
        let reserved_descriptors = u32_at(26);
        let mut classes = [ClassSpec::new(0, 0); CLASS_COUNT];
        let mut cursor = 30;
        for class in &mut classes {
            *class = ClassSpec::new(u64_at(cursor), u32_at(cursor + 8));
            cursor += 12;
        }
        let total_bytes = u64_at(cursor);
        cursor += 8;
        if bytes[cursor..cursor + 4] != [0; 4] {
            return Err(RingError::InvalidGrant);
        }
        if layout_version != LAYOUT_VERSION {
            return Err(RingError::InvalidGrant);
        }
        let ordinary: [ClassSpec; ORDINARY_CLASSES] = classes[..ORDINARY_CLASSES]
            .try_into()
            .expect("class table has fixed width");
        let geometry = PoolGeometry::new(
            ordinary_descriptors,
            reserved_descriptors,
            ordinary,
            classes[ORDINARY_CLASSES],
            classes[ORDINARY_CLASSES + 1],
        )
        .map_err(|_| RingError::InvalidGrant)?;
        let grant = Self {
            layout_version,
            incarnation,
            lane,
            geometry,
            total_bytes,
        };
        grant.checked_layout()?;
        Ok(grant)
    }

    /// `decode` for a slice; any length other than `GRANT_BYTES` is `InvalidGrant`.
    pub fn decode_slice(bytes: &[u8]) -> Result<Self, RingError> {
        let bytes: [u8; GRANT_BYTES] = bytes.try_into().map_err(|_| RingError::InvalidGrant)?;
        Self::decode(bytes)
    }

    fn checked_layout(&self) -> Result<MappingLayout, RingError> {
        let layout = MappingLayout::new(&self.geometry, system_page_size())
            .map_err(|_| RingError::InvalidGrant)?;
        if layout.total as u64 != self.total_bytes {
            return Err(RingError::InvalidGrant);
        }
        Ok(layout)
    }

    /// Length `encode` produces and `decode_slice` requires.
    pub const fn encoded_len() -> usize {
        GRANT_BYTES
    }

    /// Geometry of the pool this grant names.
    pub const fn geometry(&self) -> &PoolGeometry {
        &self.geometry
    }

    /// Complete mapping length, including control pages and alignment.
    pub const fn mapping_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Direction lane.
    pub const fn lane(&self) -> u32 {
        self.lane
    }
}

impl fmt::Debug for PoolGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PoolGrant(<redacted>)")
    }
}

/// Duplicated descriptors plus grant, ready to hand to another owner or to `attach`.
pub struct RingAttachment {
    descriptors: [OwnedFd; 3],
    grant: PoolGrant,
}

impl RingAttachment {
    /// Maps the pool these descriptors name.
    pub fn attach(self) -> Result<Ring, RingError> {
        Ring::attach(self.descriptors, self.grant)
    }

    /// Grant the descriptors were duplicated for.
    pub const fn grant(&self) -> PoolGrant {
        self.grant
    }

    /// Takes the descriptors and grant apart, for callers that send them separately.
    pub fn into_parts(self) -> ([OwnedFd; 3], PoolGrant) {
        (self.descriptors, self.grant)
    }
}

impl fmt::Debug for RingAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RingAttachment(<redacted>)")
    }
}

/// Producer-private state of one block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BlockState {
    Free,
    Reserved,
    Published,
}

/// The producer's private ledger: which block is in which state, each block's current reuse
/// generation, one free-index list per class, and the list of published blocks whose
/// completion cells are scanned before allocation. Everything is preallocated at creation or
/// attachment; allocation and return never grow the heap.
struct ProducerLedger {
    states: Vec<BlockState>,
    generations: Vec<u64>,
    free: [Vec<u32>; CLASS_COUNT],
    outstanding: Vec<u32>,
    /// Blocks returned since the owner last drained them, so a caller can settle whatever it
    /// tied to a published block at the physical return point.
    reclaimed: Vec<u32>,
    /// Whether this handle may produce: true for a created ring or one attached before any
    /// publication.
    allowed: bool,
    /// Set once a sequence or generation would wrap; no further reservation is granted while
    /// every live lease and charge stays valid.
    retired: bool,
}

impl ProducerLedger {
    fn new(geometry: &PoolGeometry, allowed: bool) -> Self {
        let blocks = geometry.block_count() as usize;
        let mut free: [Vec<u32>; CLASS_COUNT] = std::array::from_fn(|_| Vec::new());
        for (index, class) in geometry.classes().iter().enumerate() {
            let first = geometry
                .first_block(BlockClass::from_index(index).expect("index within class count"));
            let mut list = Vec::with_capacity(class.count as usize);
            // Highest id first so `pop` hands out the lowest id of a class first.
            for id in (first..first + class.count).rev() {
                list.push(id);
            }
            free[index] = list;
        }
        Self {
            states: vec![BlockState::Free; blocks],
            generations: vec![0; blocks],
            free,
            outstanding: Vec::with_capacity(blocks),
            reclaimed: Vec::with_capacity(blocks),
            allowed,
            retired: false,
        }
    }
}

/// Per-class occupancy of the producer's ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassInventory {
    /// Blocks on the class free list.
    pub free: u32,
    /// Blocks reserved by this producer and not yet published.
    pub reserved: u32,
    /// Blocks published and not yet returned.
    pub published: u32,
}

/// Occupancy of one direction as its producer and consumer handles see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolInventory {
    /// The seven classes, ordinary first.
    pub classes: [ClassInventory; CLASS_COUNT],
    /// Descriptors published and not yet consumed.
    pub descriptors_outstanding: u64,
    /// Total descriptor slots.
    pub descriptor_depth: u64,
    /// Slots ordinary traffic may hold at once.
    pub ordinary_descriptors: u64,
    /// Leases this handle's backing has live right now.
    pub outstanding_returns: u64,
    /// Whether the producer ledger has retired.
    pub retired: bool,
}

impl PoolInventory {
    /// Whether every block is accounted for: free plus reserved plus published equals the
    /// class count in every class.
    pub fn conserves(&self, geometry: &PoolGeometry) -> bool {
        self.classes
            .iter()
            .zip(geometry.classes())
            .all(|(inventory, class)| {
                inventory.free + inventory.reserved + inventory.published == class.count
            })
    }
}

/// One direction of the transport. Not `Send` or `Sync`: the producer and consumer of one
/// direction live in different processes, and within a process one thread owns the handle.
/// The backing it maps is shared with every lease it hands out and survives the handle.
pub struct Ring {
    retained: Arc<Retained>,
    grant: PoolGrant,
    data_ready: Doorbell,
    capacity_ready: Doorbell,
    /// The peer can overwrite the shared flag; this latch keeps quarantine terminal for this
    /// handle.
    quarantined: Cell<bool>,
    ledger: RefCell<ProducerLedger>,
    /// Block of the sole outstanding reservation, if any.
    reserved_block: Cell<Option<u32>>,
    /// `published` as this handle last wrote it; the shared value is peer-writable.
    published_local: Cell<u64>,
    /// `consumed` as this handle last wrote it.
    consumed_local: Cell<u64>,
    /// Greatest `consumed` this producer handle has read from the peer.
    consumed_seen: Cell<u64>,
    /// Greatest `published` this consumer handle has read from the peer.
    published_seen: Cell<u64>,
    /// Set once this handle has produced or consumed; `probe` then checks the matching cursor
    /// against this handle's record.
    producer: Cell<bool>,
    consumer: Cell<bool>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl Ring {
    /// Creates a sealed sparse pool for `lane` from `profile`'s geometry. The mapping, the
    /// producer ledger, the receiver records, and both doorbells exist before this returns;
    /// nothing is allocated per frame afterwards.
    pub fn create(profile: &TargetProfile, lane: u32) -> Result<Self, RingError> {
        debug_assert_eq!(
            profile.descriptor().schema_version(),
            DESCRIPTOR_SCHEMA_VERSION
        );
        let geometry = *profile.geometry();
        let layout =
            MappingLayout::new(&geometry, system_page_size()).map_err(RingError::Geometry)?;
        let incarnation = Incarnation::random().map_err(RingError::Descriptor)?;
        let grant = PoolGrant {
            layout_version: LAYOUT_VERSION,
            incarnation,
            lane,
            geometry,
            total_bytes: layout.total as u64,
        };
        let mapping = Mapping::create(layout.total).map_err(RingError::from)?;
        initialize_mapping(&mapping, &layout, &geometry, incarnation, lane)
            .map_err(RingError::from)?;
        seal_object(mapping.fd()).map_err(RingError::from)?;
        validate_object_len(mapping.fd(), mapping.len()).map_err(RingError::from)?;
        let (data_local, data_remote) = doorbell_pair()?;
        let (capacity_local, capacity_remote) = doorbell_pair()?;
        let retained = Arc::new(Retained::new(
            mapping,
            layout,
            geometry,
            incarnation,
            lane,
            data_local,
            capacity_local,
        ));
        Ok(Self {
            data_ready: Doorbell::new(Arc::clone(&retained), Wake::Data, Some(data_remote)),
            capacity_ready: Doorbell::new(
                Arc::clone(&retained),
                Wake::Capacity,
                Some(capacity_remote),
            ),
            ledger: RefCell::new(ProducerLedger::new(&geometry, true)),
            retained,
            grant,
            quarantined: Cell::new(false),
            reserved_block: Cell::new(None),
            published_local: Cell::new(0),
            consumed_local: Cell::new(0),
            consumed_seen: Cell::new(0),
            published_seen: Cell::new(0),
            producer: Cell::new(false),
            consumer: Cell::new(false),
            _not_send_or_sync: PhantomData,
        })
    }

    /// Maps an existing pool from its three descriptors (mapping, data doorbell, capacity
    /// doorbell). The mapping's magic, layout, and geometry must match `grant` exactly, and
    /// the doorbells must be connected stream sockets. A handle attached after publication
    /// began may consume but never produce.
    pub fn attach(descriptors: [OwnedFd; 3], grant: PoolGrant) -> Result<Self, RingError> {
        // Descriptors received over `SCM_RIGHTS` without `MSG_CMSG_CLOEXEC` arrive inheritable;
        // a child this process later execs would hold the mapping and the peer's doorbell ends
        // open and hide this side's exit from the peer.
        for descriptor in &descriptors {
            sys::set_cloexec(descriptor.as_fd()).map_err(|_| RingError::ObjectValidationFailed)?;
        }
        let [mapping_fd, data_ready, capacity_ready] = descriptors;
        let layout = grant.checked_layout()?;
        let mapping = Mapping::attach(mapping_fd, layout.total).map_err(RingError::from)?;
        let data_local = doorbell_from_fd(data_ready)?;
        let capacity_local = doorbell_from_fd(capacity_ready)?;
        let retained = Arc::new(Retained::new(
            mapping,
            layout,
            grant.geometry,
            grant.incarnation,
            grant.lane,
            data_local,
            capacity_local,
        ));
        let snapshot = retained.lifecycle_snapshot().map_err(RingError::from)?;
        if !validate_lifecycle(
            snapshot,
            &grant.geometry,
            grant.incarnation,
            grant.lane,
            layout.total,
        ) {
            return Err(RingError::InvalidGrant);
        }
        let published = retained
            .producer()
            .map_err(RingError::from)?
            .published
            .load(Ordering::Acquire);
        let consumed = retained
            .consumer()
            .map_err(RingError::from)?
            .consumed
            .load(Ordering::Acquire);
        let depth = u64::from(grant.geometry.descriptor_depth());
        if consumed > published || published - consumed > depth {
            return Err(RingError::InvalidSharedState);
        }
        let fresh = published == 0 && consumed == 0;
        let ring = Self {
            data_ready: Doorbell::new(Arc::clone(&retained), Wake::Data, None),
            capacity_ready: Doorbell::new(Arc::clone(&retained), Wake::Capacity, None),
            ledger: RefCell::new(ProducerLedger::new(&grant.geometry, fresh)),
            retained,
            grant,
            quarantined: Cell::new(false),
            reserved_block: Cell::new(None),
            published_local: Cell::new(published),
            consumed_local: Cell::new(consumed),
            consumed_seen: Cell::new(consumed),
            published_seen: Cell::new(published),
            producer: Cell::new(false),
            consumer: Cell::new(false),
            _not_send_or_sync: PhantomData,
        };
        if ring.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        Ok(ring)
    }

    /// Whether no frame has been published or consumed. Setup paths attach only fresh pools;
    /// a pool with traffic already in flight was not created for this attachment.
    pub fn is_fresh(&self) -> bool {
        self.published_local.get() == 0 && self.consumed_local.get() == 0
    }

    /// Grant a peer needs to attach to this pool.
    pub const fn grant(&self) -> PoolGrant {
        self.grant
    }

    /// Geometry of this direction.
    pub fn geometry(&self) -> &PoolGeometry {
        self.retained.geometry()
    }

    /// The backing this handle shares with its leases. Callers attach the admission charge
    /// here so it is settled when the last lease returns, not when the handle drops.
    pub fn retained(&self) -> &Arc<Retained> {
        &self.retained
    }

    /// Attaches the backing charge; see `Retained::retain_charge`.
    pub fn retain_charge(&self, charge: Arc<BackingAdmission>) {
        self.retained.retain_charge(charge);
    }

    /// Descriptor of the memfd, for sending over the setup channel.
    pub fn raw_fd(&self) -> RawFd {
        self.retained.mapping().fd().as_raw_fd()
    }

    /// Duplicate of the data doorbell, for registering with an event loop that owns its fds.
    pub fn duplicate_data_ready(&self) -> Result<OwnedFd, RingError> {
        self.data_ready.duplicate()
    }

    /// Both doorbells plus retained wakes. The counts never reset, so a window is the
    /// difference of two samples.
    pub fn syscall_counters(&self) -> SyscallCounters {
        self.data_ready
            .counters()
            .add(self.capacity_ready.counters())
            .add(SyscallCounters {
                retained_wakes: self.retained.wake_signals(),
                ..SyscallCounters::default()
            })
    }

    /// Duplicates the mapping and moves the peer's two doorbell ends out, all with `CLOEXEC`
    /// set, paired with the grant. Callable once per created pool; an attached pool or a
    /// second call fails with `DoorbellFailed`.
    pub fn attachment(&self) -> Result<RingAttachment, RingError> {
        let fd = self
            .retained
            .mapping()
            .fd()
            .try_clone()
            .map_err(|_| RingError::ObjectSetupFailed)?;
        Ok(RingAttachment {
            descriptors: [
                fd,
                self.data_ready.take_peer_end()?,
                self.capacity_ready.take_peer_end()?,
            ],
            grant: self.grant,
        })
    }

    /// Mappings this pool holds; always one. Exists so callers charge admission uniformly.
    pub const fn mapping_count(&self) -> usize {
        1
    }

    /// Byte length of the memfd, equal to the grant's total.
    pub fn object_size(&self) -> usize {
        self.retained.mapping().len()
    }

    // ---- wake protocol -------------------------------------------------------------------

    /// Prepares to block on the data doorbell. Records the wake generation, re-checks for
    /// data, and drains a stale token, so a publish that raced this call is not missed.
    /// Returns `true` only when blocking is correct; `false` means data or a generation change
    /// is already visible and the caller should poll again instead.
    pub fn arm_data_wait(&self) -> Result<bool, RingError> {
        match self.arm_data_wait_guarded()? {
            Some(guard) => {
                // The caller owns `parked` until `complete_data_wait` clears it.
                std::mem::forget(guard);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn arm_data_wait_guarded(&self) -> Result<Option<ParkGuard<'_>>, RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        if self.data_available()? {
            return Ok(None);
        }
        let wake = self.retained.data_wake().map_err(RingError::from)?;
        let (generation, guard) = ParkGuard::arm(wake);
        if !self.armed_wait_holds(wake, generation)? {
            return Ok(None);
        }
        self.data_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))?;
        if !self.armed_wait_holds(wake, generation)? {
            return Ok(None);
        }
        Ok(Some(guard))
    }

    /// Re-checks, after `parked` is set, that blocking is still correct: no quarantine, no
    /// data, and no wake generation change. `enter_quarantine` rings the doorbell only for a
    /// handle it sees parked, so a quarantine that lands between the first check and the
    /// `parked` store sends no token; this re-check covers that window.
    fn armed_wait_holds(&self, wake: &WakeEpoch, generation: u64) -> Result<bool, RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        let available = self.data_available()?;
        Ok(!available && wake.generation.load(Ordering::SeqCst) == generation)
    }

    /// Clears the parked marker set by `arm_data_wait` and drains the doorbell token. A
    /// doorbell failure means the peer closed its end, which quarantines the ring.
    pub fn complete_data_wait(&self) -> Result<(), RingError> {
        self.retained
            .data_wake()
            .map_err(RingError::from)?
            .parked
            .store(0, Ordering::Release);
        self.data_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))
    }

    /// Blocks on the data doorbell until `try_receive` would return a frame, or `deadline`.
    /// Returns whether data is available; a quarantined ring returns `Quarantined` at once.
    pub fn wait_for_data(&self, deadline: Instant) -> Result<bool, RingError> {
        loop {
            if self.is_quarantined() {
                return Err(RingError::Quarantined);
            }
            if self.data_available()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            let Some(_guard) = self.arm_data_wait_guarded()? else {
                continue;
            };
            let ready = self
                .data_ready
                .wait_until(deadline)
                .map_err(|error| self.quarantine_with(error))?;
            if !ready && Instant::now() >= deadline {
                return Ok(false);
            }
            self.complete_data_wait()?;
        }
    }

    fn data_available(&self) -> Result<bool, RingError> {
        let consumed = self.consumed_local.get();
        let published = self
            .verified_published(consumed)
            .map_err(|error| self.quarantine_with(error))?;
        Ok(published != consumed)
    }

    /// Prepares to block on the capacity doorbell; the caller retries `try_reserve` first and
    /// polls the returned readiness descriptor only when this returns `true`. Mirrors
    /// `arm_data_wait` for the producer side so an owner that multiplexes several sources can
    /// park without an uninterruptible wait.
    pub fn arm_capacity_wait(&self) -> Result<bool, RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        let wake = self.retained.capacity_wake().map_err(RingError::from)?;
        let (generation, guard) = ParkGuard::arm(wake);
        self.capacity_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))?;
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        if wake.generation.load(Ordering::SeqCst) != generation {
            return Ok(false);
        }
        std::mem::forget(guard);
        Ok(true)
    }

    /// Clears the parked marker set by `arm_capacity_wait` and drains the doorbell token.
    pub fn complete_capacity_wait(&self) -> Result<(), RingError> {
        self.retained
            .capacity_wake()
            .map_err(RingError::from)?
            .parked
            .store(0, Ordering::Release);
        self.capacity_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))
    }

    /// Duplicate of the capacity doorbell, for a producer that multiplexes capacity readiness
    /// with other sources.
    pub fn duplicate_capacity_ready(&self) -> Result<OwnedFd, RingError> {
        self.capacity_ready.duplicate()
    }

    // ---- producer -------------------------------------------------------------------------

    /// `try_reserve_in(Inventory::Ordinary, ..)`.
    pub fn try_reserve(
        &self,
        bound: usize,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        self.try_reserve_in(Inventory::Ordinary, bound, wire_header)
    }

    /// Reserves a block from `inventory` for a body of at most `bound` bytes and a descriptor
    /// slot, without blocking. `Exhausted` means the class or the inventory's descriptor
    /// headroom is full; `ReservationOutstanding` means this handle has not committed or
    /// aborted its previous reservation. Nothing is charged on any error. Completion cells are
    /// scanned first, so a block returned by any thread since the last call is reusable here.
    pub fn try_reserve_in(
        &self,
        inventory: Inventory,
        bound: usize,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        #[cfg(test)]
        crate::lease::observers::ring_call();
        if bound > MAX_FRAME_BYTES {
            return Err(ProducerError::BoundExceedsClass);
        }
        if self.is_quarantined() {
            return Err(ProducerError::Quarantined);
        }
        if self.reserved_block.get().is_some() {
            return Err(ProducerError::ReservationOutstanding);
        }
        {
            let ledger = self.ledger.borrow();
            if !ledger.allowed {
                return Err(ProducerError::Ring(RingError::RoleMismatch));
            }
            if ledger.retired {
                return Err(ProducerError::Retired);
            }
        }
        self.reclaim_completions().map_err(ProducerError::Ring)?;
        let geometry = *self.geometry();
        let class = geometry
            .class_for(inventory, bound as u64)
            .ok_or(ProducerError::BoundExceedsClass)?;
        // Descriptor headroom: ordinary traffic leaves the reserved slots alone.
        let outstanding = self
            .descriptors_outstanding()
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        let limit = match inventory {
            Inventory::Ordinary => u64::from(geometry.ordinary_descriptors()),
            Inventory::Control | Inventory::Terminal => u64::from(geometry.descriptor_depth()),
        };
        if outstanding >= limit {
            return Err(ProducerError::Exhausted);
        }
        if self.published_local.get() == u64::MAX {
            self.ledger.borrow_mut().retired = true;
            return Err(ProducerError::Retired);
        }
        let mut ledger = self.ledger.borrow_mut();
        #[cfg(test)]
        crate::lease::observers::free_list_mutation();
        let Some(block) = ledger.free[class.index()].pop() else {
            return Err(ProducerError::Exhausted);
        };
        let Some(generation) = ledger.generations[block as usize].checked_add(1) else {
            ledger.free[class.index()].push(block);
            ledger.retired = true;
            return Err(ProducerError::Retired);
        };
        ledger.generations[block as usize] = generation;
        ledger.states[block as usize] = BlockState::Reserved;
        drop(ledger);
        self.reserved_block.set(Some(block));
        self.producer.set(true);
        Ok(ProducerReservation {
            ring: self,
            block,
            generation,
            class,
            capacity: bound,
            cursor: 0,
            wire_header,
            finished: false,
            _not_send: PhantomData,
        })
    }

    /// `try_reserve` that parks on the capacity doorbell until a return or consumption frees
    /// room or `deadline` passes. Each park is bound to a wake generation so a wake between the
    /// check and the park cannot be missed.
    pub fn reserve_until(
        &self,
        bound: usize,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
        deadline: Instant,
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        self.reserve_until_in(Inventory::Ordinary, bound, wire_header, deadline)
    }

    /// `reserve_until` for an explicit inventory.
    pub fn reserve_until_in(
        &self,
        inventory: Inventory,
        bound: usize,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
        deadline: Instant,
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        loop {
            match self.try_reserve_in(inventory, bound, wire_header) {
                Err(ProducerError::Exhausted) if Instant::now() < deadline => {}
                Err(ProducerError::Exhausted) => return Err(ProducerError::Deadline),
                result => return result,
            }
            #[cfg(test)]
            crate::lease::observers::slot_wait();
            let wake = self
                .retained
                .capacity_wake()
                .map_err(|error| ProducerError::Ring(error.into()))?;
            // The guard clears `parked` on every exit from this iteration, including `?`.
            let (generation, _guard) = ParkGuard::arm(wake);
            match self.try_reserve_in(inventory, bound, wire_header) {
                Err(ProducerError::Exhausted) if Instant::now() < deadline => {}
                Err(ProducerError::Exhausted) => return Err(ProducerError::Deadline),
                result => return result,
            }
            if wake.generation.load(Ordering::SeqCst) != generation {
                continue;
            }
            self.capacity_ready
                .drain()
                .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
            match self.try_reserve_in(inventory, bound, wire_header) {
                Err(ProducerError::Exhausted) if Instant::now() < deadline => {}
                Err(ProducerError::Exhausted) => return Err(ProducerError::Deadline),
                result => return result,
            }
            if wake.generation.load(Ordering::SeqCst) != generation {
                continue;
            }
            let ready = self
                .capacity_ready
                .wait_until(deadline)
                .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
            if !ready && Instant::now() >= deadline {
                return Err(ProducerError::Deadline);
            }
            self.capacity_ready
                .drain()
                .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        }
    }

    /// Scans the completion cell of every published block and returns each block whose cell
    /// holds its current generation to its class free list. A cell ahead of the generation the
    /// producer issued names a payload this pool never published and quarantines the ring.
    fn reclaim_completions(&self) -> Result<(), RingError> {
        let mut ledger = self.ledger.borrow_mut();
        let geometry = *self.geometry();
        let mut index = 0;
        while index < ledger.outstanding.len() {
            let block = ledger.outstanding[index];
            let generation = ledger.generations[block as usize];
            let cell = self.retained.completion(block).map_err(RingError::from)?;
            // Acquire pairs with the final owner's Release publication of the generation.
            let completed = cell.generation.load(Ordering::Acquire);
            if completed > generation {
                drop(ledger);
                return Err(
                    self.quarantine_with(RingError::Descriptor(DescriptorError::FutureCompletion))
                );
            }
            if completed == generation {
                let class = geometry
                    .placement(block)
                    .ok_or(RingError::InvalidLayout)?
                    .class;
                ledger.outstanding.swap_remove(index);
                ledger.states[block as usize] = BlockState::Free;
                #[cfg(test)]
                crate::lease::observers::free_list_mutation();
                ledger.free[class.index()].push(block);
                if ledger.reclaimed.len() < ledger.reclaimed.capacity() {
                    ledger.reclaimed.push(block);
                }
                continue;
            }
            index += 1;
        }
        Ok(())
    }

    /// `published - consumed`, with the peer-writable `consumed` checked for monotonicity and
    /// depth.
    fn descriptors_outstanding(&self) -> Result<u64, RingError> {
        let published = self.published_local.get();
        // Acquire pairs with the consumer's Release store after it copied the slot out.
        let consumed = self
            .retained
            .consumer()
            .map_err(RingError::from)?
            .consumed
            .load(Ordering::Acquire);
        if consumed < self.consumed_seen.get() || consumed > published {
            return Err(RingError::InvalidSharedState);
        }
        self.consumed_seen.set(consumed);
        Ok(published - consumed)
    }

    /// Returns the block of an unpublished reservation to its free list. The generation stays
    /// burned so a later reservation of the block publishes a strictly newer one.
    fn abort_reservation(&self, block: u32) {
        #[cfg(test)]
        crate::lease::observers::ring_call();
        if self.reserved_block.get() != Some(block) {
            return;
        }
        self.reserved_block.set(None);
        let mut ledger = self.ledger.borrow_mut();
        if ledger.states[block as usize] != BlockState::Reserved {
            return;
        }
        if let Some(placement) = self.geometry().placement(block) {
            ledger.states[block as usize] = BlockState::Free;
            #[cfg(test)]
            crate::lease::observers::free_list_mutation();
            ledger.free[placement.class.index()].push(block);
        }
    }

    /// Writes the header into the block, publishes the descriptor, and rings the data
    /// doorbell. Once the slot and cursor are written the peer may hold the frame, so a failed
    /// wake quarantines the ring but never rolls the publication back.
    fn publish_commit(
        &self,
        block: u32,
        generation: u64,
        body_len: usize,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
    ) -> Result<PayloadIdentity, ProducerError> {
        #[cfg(test)]
        crate::lease::observers::ring_call();
        check_wire_header(&wire_header, body_len as u64)
            .map_err(|_| ProducerError::WireHeaderMismatch)?;
        let published = self.published_local.get();
        let Some(sequence) = published.checked_add(1) else {
            self.ledger.borrow_mut().retired = true;
            return Err(ProducerError::Retired);
        };
        let depth = self.geometry().descriptor_depth() as u64;
        let index = usize::try_from((sequence - 1) % depth).map_err(|_| ProducerError::Overflow)?;
        let slot = self
            .retained
            .slot(index)
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error.into())))?;
        let producer = self
            .retained
            .producer()
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error.into())))?;
        let header_ptr = self
            .retained
            .header_ptr(block)
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error.into())))?;
        // SAFETY: `header_ptr` checked the header range against the block and the mapping; the
        // reservation owns the block until publication, so no reader has a lease over it, and
        // no Rust reference covers arena bytes.
        unsafe { copy_in(&wire_header, header_ptr) };
        slot.block.store(u64::from(block), Ordering::Relaxed);
        slot.generation.store(generation, Ordering::Relaxed);
        slot.body_len.store(body_len as u64, Ordering::Relaxed);
        slot.sequence.store(sequence, Ordering::Relaxed);
        // The compare makes the verify-then-store pair atomic: a peer rewrite of `published`
        // fails the exchange instead of being overwritten.
        producer
            .published
            .compare_exchange(published, sequence, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ProducerError::Ring(self.quarantine_with(RingError::InvalidSharedState))
            })?;
        self.published_local.set(sequence);
        {
            let mut ledger = self.ledger.borrow_mut();
            ledger.states[block as usize] = BlockState::Published;
            ledger.outstanding.push(block);
        }
        self.reserved_block.set(None);
        let identity =
            PayloadIdentity::new(self.grant.incarnation, self.grant.lane, block, generation);
        if let Err(error) = self.signal_data_wake() {
            self.enter_quarantine();
            return Err(ProducerError::Ring(error));
        }
        // A peer quarantine that landed between `commit`'s check and the stores above leaves
        // the frame published on a terminal ring; the caller must not take it as delivered.
        if self.is_quarantined() {
            return Err(ProducerError::Quarantined);
        }
        Ok(identity)
    }

    fn signal_data_wake(&self) -> Result<(), RingError> {
        let wake = self.retained.data_wake().map_err(RingError::from)?;
        wake.generation.fetch_add(1, Ordering::SeqCst);
        if wake.parked.swap(0, Ordering::SeqCst) != 0 {
            self.data_ready.signal()?;
        }
        Ok(())
    }

    fn write_reservation(
        &self,
        block: u32,
        capacity: usize,
        cursor: usize,
        bytes: &[u8],
    ) -> Result<(), ProducerError> {
        let end = cursor
            .checked_add(bytes.len())
            .ok_or(ProducerError::Overflow)?;
        if end > capacity {
            return Err(ProducerError::Overflow);
        }
        let body = self
            .retained
            .body_ptr(block, capacity)
            .map_err(|error| ProducerError::Ring(error.into()))?;
        // SAFETY: `body_ptr` checked `[body, body + capacity)` against the block and the
        // mapping, `cursor + bytes.len() <= capacity`, no Rust reference covers arena bytes, and
        // the reservation owns this block until commit or abort, so no reader has a lease over
        // it.
        unsafe { copy_in(bytes, body.add(cursor)) };
        Ok(())
    }

    // ---- consumer -------------------------------------------------------------------------

    /// Leases the next published frame. `Ok(None)` means the pool is empty. `Err` means the
    /// channel is dead: the descriptor failed validation, its block is already live, its
    /// generation did not increase, or shared state is impossible, and the ring is
    /// quarantined. Consumption is acknowledged before this returns, so the slot is reusable
    /// while the payload stays held.
    pub fn try_receive(&self) -> Result<Option<PayloadLease>, RingError> {
        #[cfg(test)]
        crate::lease::observers::ring_call();
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        let lease = self
            .try_receive_inner()
            .map_err(|error| self.quarantine_with(error))?;
        // A peer quarantine that landed while the slot was being taken leaves the frame leased
        // on a terminal ring; the caller must not read it as delivered.
        if self.is_quarantined() {
            drop(lease);
            return Err(RingError::Quarantined);
        }
        Ok(lease)
    }

    fn try_receive_inner(&self) -> Result<Option<PayloadLease>, RingError> {
        let consumed = self.consumed_local.get();
        let published = self.verified_published(consumed)?;
        if consumed == published {
            return Ok(None);
        }
        let sequence = consumed
            .checked_add(1)
            .ok_or(RingError::SequenceExhausted)?;
        let geometry = *self.geometry();
        let depth = geometry.descriptor_depth() as u64;
        let index =
            usize::try_from((sequence - 1) % depth).map_err(|_| RingError::ArithmeticOverflow)?;
        let slot = self.retained.slot(index).map_err(RingError::from)?;
        // The Acquire load of `published` in `verified_published` pairs with the producer's
        // Release publication, so these Relaxed loads see the fields written before it. Each is
        // a fixed-width atomic copied out once; nothing re-reads the slot afterwards.
        let snapshot = PoolDescriptor::from_untrusted(
            slot.sequence.load(Ordering::Relaxed),
            slot.block.load(Ordering::Relaxed),
            slot.generation.load(Ordering::Relaxed),
            slot.body_len.load(Ordering::Relaxed),
        );
        let validated = snapshot
            .validate(sequence, &geometry)
            .map_err(RingError::Descriptor)?;
        let block = validated.block();
        let generation = validated.generation();
        // Receiver-local records: a block with a live lease, or a generation that did not
        // increase, is a protocol error before any lease exists.
        match self.retained.live_generation(block) {
            Some(0) => {}
            _ => return Err(RingError::Descriptor(DescriptorError::DuplicateLiveBlock)),
        }
        if self
            .retained
            .last_seen_generation(block)
            .is_none_or(|seen| generation <= seen)
        {
            return Err(RingError::Descriptor(
                DescriptorError::NonIncreasingGeneration,
            ));
        }
        let body_len =
            usize::try_from(validated.body_len()).map_err(|_| RingError::InvalidLayout)?;
        let header_ptr = self.retained.header_ptr(block).map_err(RingError::from)?;
        let mut wire_header = [0u8; WIRE_V3_HEADER_BYTES];
        // SAFETY: `header_ptr` checked the header range against the block and the mapping, and
        // no Rust reference covers arena bytes; the atomic copy tolerates a peer write, which
        // the check below then rejects.
        unsafe { copy_out(header_ptr, &mut wire_header) };
        check_wire_header(&wire_header, validated.body_len()).map_err(RingError::Descriptor)?;
        let consumer = self.retained.consumer().map_err(RingError::from)?;
        // Release publishes the slot copy-out before the producer may reuse the slot.
        consumer
            .consumed
            .compare_exchange(consumed, sequence, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| RingError::InvalidSharedState)?;
        self.consumed_local.set(sequence);
        self.consumer.set(true);
        self.retained.mark_live(block, generation);
        // Consumption alone frees a descriptor slot, so the producer is woken even though the
        // payload stays held. A failed wake latches on the backing; the consumption stands and
        // the frame is still delivered.
        let _ = self.retained.signal_capacity();
        Ok(Some(PayloadLease::new(
            Arc::clone(&self.retained),
            block,
            generation,
            body_len,
            wire_header,
        )))
    }

    /// Loads `published` and rejects a value below the greatest one this handle has seen or
    /// more than `descriptor_depth` ahead of `consumed`, which no producer can reach.
    fn verified_published(&self, consumed: u64) -> Result<u64, RingError> {
        // Acquire pairs with the producer's release store in `publish_commit`.
        let published = self
            .retained
            .producer()
            .map_err(RingError::from)?
            .published
            .load(Ordering::Acquire);
        let queued = published
            .checked_sub(consumed)
            .ok_or(RingError::InvalidSharedState)?;
        if published < self.published_seen.get()
            || queued > u64::from(self.geometry().descriptor_depth())
        {
            return Err(RingError::InvalidSharedState);
        }
        self.published_seen.set(published);
        Ok(published)
    }

    // ---- health ---------------------------------------------------------------------------

    /// Occupancy from this handle's ledger and backing. A quarantined ring reports every block
    /// as published so nothing looks reusable.
    pub fn inventory(&self) -> PoolInventory {
        let geometry = *self.geometry();
        if self.ledger.borrow().allowed && !self.is_quarantined() {
            // Returns since the last reservation are folded in so the counts reflect what the
            // next reservation would see; a failure here quarantines, which the counts show.
            let _ = self.reclaim_completions();
        }
        let ledger = self.ledger.borrow();
        let mut classes = [ClassInventory::default(); CLASS_COUNT];
        if self.is_quarantined() {
            for (inventory, class) in classes.iter_mut().zip(geometry.classes()) {
                inventory.published = class.count;
            }
        } else {
            for (block, state) in ledger.states.iter().enumerate() {
                let Some(placement) = geometry.placement(block as u32) else {
                    continue;
                };
                let inventory = &mut classes[placement.class.index()];
                match state {
                    BlockState::Free => inventory.free += 1,
                    BlockState::Reserved => inventory.reserved += 1,
                    BlockState::Published => inventory.published += 1,
                }
            }
        }
        let consumed = self
            .retained
            .consumer()
            .map(|page| page.consumed.load(Ordering::Acquire))
            .unwrap_or(0);
        PoolInventory {
            classes,
            descriptors_outstanding: self.published_local.get().saturating_sub(consumed),
            descriptor_depth: u64::from(geometry.descriptor_depth()),
            ordinary_descriptors: u64::from(geometry.ordinary_descriptors()),
            outstanding_returns: self.retained.outstanding_returns(),
            retired: ledger.retired,
        }
    }

    /// `Ok` if the shared cursors are consistent and the ring is not quarantined: `consumed`
    /// never exceeds `published`, their gap never exceeds the depth, and each cursor this handle
    /// wrote still holds its value. An inconsistency quarantines the ring.
    pub fn probe(&self) -> Result<(), RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        let published = self
            .retained
            .producer()
            .map_err(RingError::from)?
            .published
            .load(Ordering::Acquire);
        let consumed = self
            .retained
            .consumer()
            .map_err(RingError::from)?
            .consumed
            .load(Ordering::Acquire);
        let depth = u64::from(self.geometry().descriptor_depth());
        let consistent = consumed <= published
            && published - consumed <= depth
            && (!self.producer.get() || published == self.published_local.get())
            && (!self.consumer.get() || consumed == self.consumed_local.get());
        if !consistent {
            return Err(self.quarantine_with(RingError::InvalidSharedState));
        }
        // Completion cells ahead of any issued generation are caught here too.
        if self.producer.get() {
            self.reclaim_completions()?;
        }
        Ok(())
    }

    /// The private latch never clears, so rewriting the shared flag cannot revive the ring.
    /// Both wake channels are rung so a peer parked in `wait_for_data` or `reserve_until`
    /// re-checks and sees the flag instead of sleeping to its deadline. Wake failures are
    /// ignored here: the ring is already terminal and the wake is best effort.
    pub fn enter_quarantine(&self) {
        self.quarantined.set(true);
        if let Ok(flag) = self.retained.lifecycle_quarantined() {
            flag.store(1, Ordering::Release);
        }
        let _ = self.signal_data_wake();
        let _ = self.retained.signal_capacity();
    }

    /// True if this handle latched quarantine or the shared flag is set. Observing the shared
    /// flag latches too, so a peer that sets and then clears it cannot revive this handle.
    /// An unreadable lifecycle page counts as quarantined.
    pub fn is_quarantined(&self) -> bool {
        if self.quarantined.get() {
            return true;
        }
        let observed = self
            .retained
            .lifecycle_quarantined()
            .map(|flag| flag.load(Ordering::Acquire) != 0)
            .unwrap_or(true);
        if observed {
            self.quarantined.set(true);
        }
        observed
    }

    /// Whether the producer ledger retired because a sequence or generation would wrap.
    pub fn is_retired(&self) -> bool {
        self.ledger.borrow().retired
    }

    /// Scans completion cells now and hands every block returned since the previous call to
    /// `settle`, in return order. A caller that ties a credit or record to a published block
    /// releases it here, at the physical return, not when a callback finishes. The list is
    /// bounded by the block count, so a caller that never drains loses only the oldest
    /// notifications, never a return.
    pub fn take_reclaimed(&self, mut settle: impl FnMut(u32)) -> Result<(), RingError> {
        if self.ledger.borrow().allowed && !self.is_quarantined() {
            self.reclaim_completions()?;
        }
        let reclaimed: Vec<u32> = std::mem::take(&mut self.ledger.borrow_mut().reclaimed);
        for block in &reclaimed {
            settle(*block);
        }
        // The buffer keeps its capacity so steady-state returns allocate nothing.
        let mut ledger = self.ledger.borrow_mut();
        let mut buffer = reclaimed;
        buffer.clear();
        ledger.reclaimed = buffer;
        Ok(())
    }

    /// Quarantines and returns `error`, for impossible shared state observed mid-operation.
    fn quarantine_with(&self, error: RingError) -> RingError {
        self.enter_quarantine();
        error
    }

    fn lease_span<'ring>(
        &'ring self,
        block: u32,
        capacity: usize,
    ) -> Result<LeaseSpan<'ring>, RingError> {
        if capacity == 0 {
            let header = self.retained.header_ptr(block).map_err(RingError::from)?;
            // SAFETY: a zero-length span reads nothing; the pointer is inside the retained
            // mapping, which `self` keeps alive.
            return unsafe { LeaseSpan::new(header, 0) }.map_err(RingError::Lease);
        }
        let ptr = self
            .retained
            .body_ptr(block, capacity)
            .map_err(RingError::from)?;
        // SAFETY: `body_ptr` checked `[ptr, ptr + capacity)` against the block and the
        // mapping, and the mapping (kept alive by `self`) stays mapped for `'ring`. No `&[u8]`
        // over the arena is ever formed in this crate; access goes through atomic reads and
        // writes.
        unsafe { LeaseSpan::new(ptr, capacity) }.map_err(RingError::Lease)
    }

    /// Advances the producer ledger's generation for `block` to `generation`, so a test can
    /// reach the wrap boundary without publishing `u64::MAX` frames.
    #[doc(hidden)]
    pub fn set_block_generation_for_test(&self, block: u32, generation: u64) {
        if let Some(slot) = self.ledger.borrow_mut().generations.get_mut(block as usize) {
            *slot = generation;
        }
    }

    /// Moves both shared cursors and this handle's records to `sequence`, so a test can reach
    /// the sequence wrap boundary on an idle ring. Both peers must share the process.
    #[doc(hidden)]
    pub fn set_sequence_for_test(&self, sequence: u64) -> Result<(), RingError> {
        let producer = self.retained.producer().map_err(RingError::from)?;
        let consumer = self.retained.consumer().map_err(RingError::from)?;
        producer.published.store(sequence, Ordering::Release);
        consumer.consumed.store(sequence, Ordering::Release);
        self.published_local.set(sequence);
        self.consumed_local.set(sequence);
        self.consumed_seen.set(sequence);
        self.published_seen.set(sequence);
        Ok(())
    }
}

impl fmt::Debug for Ring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ring(<redacted>)")
    }
}

/// A reserved block the producer fills then commits. Dropping without `commit` aborts,
/// returning the block to its class free list; the burned generation is never reused.
#[must_use = "producer reservation must be committed or aborted"]
pub struct ProducerReservation<'ring> {
    ring: &'ring Ring,
    block: u32,
    generation: u64,
    class: BlockClass,
    /// The caller's body bound, not the block's class slack.
    capacity: usize,
    cursor: usize,
    wire_header: [u8; WIRE_V3_HEADER_BYTES],
    finished: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl ProducerReservation<'_> {
    /// Body bytes the caller may write: the bound given to `try_reserve`, never the class
    /// slack behind it. `commit` may publish fewer.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes written through `write` or `advance`.
    pub const fn written(&self) -> usize {
        self.cursor
    }

    /// `capacity() - written()`.
    pub const fn remaining(&self) -> usize {
        self.capacity - self.cursor
    }

    /// Block this reservation owns.
    pub const fn block(&self) -> u32 {
        self.block
    }

    /// Class the block was drawn from.
    pub const fn class(&self) -> BlockClass {
        self.class
    }

    /// Identity the published payload will carry.
    pub fn identity(&self) -> PayloadIdentity {
        PayloadIdentity::new(
            self.ring.grant.incarnation,
            self.ring.grant.lane,
            self.block,
            self.generation,
        )
    }

    /// Always one: a block is contiguous. Kept so callers that iterate segments need no
    /// special case.
    pub const fn segment_count(&self) -> usize {
        1
    }

    /// Raw view of the body bound, for callers that write in place instead of via `write`.
    /// Follow with `advance`. `None` past the single segment.
    pub fn segment(&self, index: usize) -> Result<Option<LeaseSpan<'_>>, ProducerError> {
        if index != 0 {
            return Ok(None);
        }
        self.ring
            .lease_span(self.block, self.capacity)
            .map(Some)
            .map_err(ProducerError::Ring)
    }

    /// Records `bytes` written in place through `segment`. Aborts the reservation on overflow.
    pub fn advance(&mut self, bytes: usize) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        let Some(cursor) = self.cursor.checked_add(bytes) else {
            self.abort_now();
            return Err(ProducerError::Overflow);
        };
        if cursor > self.capacity {
            self.abort_now();
            return Err(ProducerError::Overflow);
        }
        self.cursor = cursor;
        Ok(())
    }

    /// Replaces the header given to `try_reserve`. `commit` checks its declared length.
    pub fn set_wire_header(
        &mut self,
        wire_header: [u8; WIRE_V3_HEADER_BYTES],
    ) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        self.wire_header = wire_header;
        Ok(())
    }

    /// Copies `bytes` at the cursor. Aborts on overflow.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        if let Err(error) =
            self.ring
                .write_reservation(self.block, self.capacity, self.cursor, bytes)
        {
            self.abort_now();
            return Err(error);
        }
        self.cursor += bytes.len();
        Ok(())
    }

    /// Publishes `body_len` bytes. `body_len` must equal `written()`; the header's declared
    /// length must equal `body_len`. A shorter body than the bound keeps its block: there is
    /// no relocation after serialization. A failure before publication aborts the
    /// reservation; a failed wake after publication quarantines the ring and leaves the frame
    /// published.
    pub fn commit(mut self, body_len: usize) -> Result<PayloadIdentity, ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        // Quarantine may have been entered, locally or by the peer, since `try_reserve`.
        if self.ring.is_quarantined() {
            self.abort_now();
            return Err(ProducerError::Quarantined);
        }
        if body_len > self.capacity {
            self.abort_now();
            return Err(ProducerError::CommitOutsideReservation);
        }
        if self.cursor != body_len {
            self.abort_now();
            return Err(ProducerError::Underfill);
        }
        if check_wire_header(&self.wire_header, body_len as u64).is_err() {
            self.abort_now();
            return Err(ProducerError::WireHeaderMismatch);
        }
        self.finished = true;
        self.ring
            .publish_commit(self.block, self.generation, body_len, self.wire_header)
    }

    /// Gives the block back without publishing. Same as drop, but explicit.
    pub fn abort(mut self) {
        self.abort_now();
    }

    fn abort_now(&mut self) {
        if !self.finished {
            self.finished = true;
            self.ring.abort_reservation(self.block);
        }
    }
}

impl fmt::Debug for ProducerReservation<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProducerReservation(<redacted>)")
    }
}

impl Drop for ProducerReservation<'_> {
    fn drop(&mut self) {
        self.abort_now();
    }
}

/// Two rings: lane 0 and lane 1.
pub struct DuplexRing {
    /// Caller-to-peer direction.
    pub first: Ring,
    /// Peer-to-caller direction.
    pub second: Ring,
}

impl DuplexRing {
    /// Creates both rings from the same profile.
    pub fn create(profile: &TargetProfile) -> Result<Self, RingError> {
        Ok(Self {
            first: Ring::create(profile, 0)?,
            second: Ring::create(profile, 1)?,
        })
    }

    /// Attaches one backing charge to both directions. The charge settles when the last
    /// lease of either direction returns and both handles have dropped.
    pub fn retain_charge(&self, charge: Arc<BackingAdmission>) {
        self.first.retain_charge(Arc::clone(&charge));
        self.second.retain_charge(charge);
    }
}

impl fmt::Debug for DuplexRing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DuplexRing(<redacted>)")
    }
}

/// Header with `body_len` in the first four bytes and the application version in the fifth,
/// zeros elsewhere.
pub fn wire_v3_header(body_len: usize) -> Result<[u8; WIRE_V3_HEADER_BYTES], ProducerError> {
    let body_len = u32::try_from(body_len).map_err(|_| ProducerError::BoundExceedsClass)?;
    if body_len as usize > MAX_FRAME_BYTES {
        return Err(ProducerError::BoundExceedsClass);
    }
    let mut header = [0u8; WIRE_V3_HEADER_BYTES];
    header[0..4].copy_from_slice(&body_len.to_le_bytes());
    header[4] = WIRE_V3_VERSION;
    Ok(header)
}

/// Why a reservation, write, or commit failed. Failures other than `Exhausted`, `Deadline`,
/// `Retired`, and `ReservationOutstanding` abort the reservation.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProducerError {
    /// `bound` exceeds `MAX_FRAME_BYTES` or the inventory's largest class.
    #[error("producer bound exceeds every class of its inventory")]
    BoundExceedsClass,
    /// A write or advance would pass `capacity()`.
    #[error("producer cursor overflow")]
    Overflow,
    /// `commit(body_len)` with `body_len > capacity()`.
    #[error("commit exceeds reservation")]
    CommitOutsideReservation,
    /// `commit(body_len)` with `body_len != written()`.
    #[error("producer reservation is underfilled")]
    Underfill,
    /// The reservation was already committed or aborted.
    #[error("producer reservation is aborted")]
    Aborted,
    /// No free block in the class, or no descriptor headroom for the inventory. Retry after a
    /// return or consumption.
    #[error("bounded pool capacity is exhausted")]
    Exhausted,
    /// This handle already holds an uncommitted reservation. No peer return can clear it;
    /// commit or abort that reservation first.
    #[error("producer reservation is already outstanding")]
    ReservationOutstanding,
    /// `reserve_until` hit its deadline while still `Exhausted`.
    #[error("bounded backpressure deadline elapsed")]
    Deadline,
    /// A sequence or generation would wrap; the producer retired and grants no further
    /// reservation, while live leases and charges stay valid.
    #[error("pool producer retired before a counter could wrap")]
    Retired,
    /// The header's version is wrong or its declared length is not `body_len`.
    #[error("wire header disagrees with committed body")]
    WireHeaderMismatch,
    /// The ring is quarantined.
    #[error("transport storage is quarantined")]
    Quarantined,
    /// Shared state was unreadable or inconsistent.
    #[error("ring operation failed")]
    Ring(#[source] RingError),
}

impl fmt::Debug for ProducerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Why creating, attaching, or reading a pool failed. Any variant from `try_receive` means
/// the ring is now quarantined.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RingError {
    /// Offset or size arithmetic overflowed.
    #[error("ring arithmetic overflow")]
    ArithmeticOverflow,
    /// `memfd_create`, `ftruncate`, `mmap`, or `fcntl` failed.
    #[error("shared object setup failed")]
    ObjectSetupFailed,
    /// The memfd failed an owner, size, type, mode, or seal check.
    #[error("shared object validation failed")]
    ObjectValidationFailed,
    /// The grant failed `decode`, or disagrees with the mapping it was presented with.
    #[error("attachment grant is invalid")]
    InvalidGrant,
    /// The mapping's magic, version, or geometry fields disagree with the grant, or an offset
    /// left the mapping.
    #[error("shared memory layout is invalid")]
    InvalidLayout,
    /// A cursor, completion cell, or count is one the protocol cannot produce.
    #[error("shared ring state is invalid")]
    InvalidSharedState,
    /// A doorbell `socketpair`, `poll`, `recv`, or `send` failed, the peer closed its doorbell
    /// end, or an attachment descriptor is not a connected `AF_UNIX` stream socket.
    #[error("ring doorbell failed")]
    DoorbellFailed,
    /// A sequence number would overflow `u64`.
    #[error("publication sequence exhausted")]
    SequenceExhausted,
    /// The ring is quarantined.
    #[error("transport storage is quarantined")]
    Quarantined,
    /// A producer operation on a handle that attached after publication began.
    #[error("operation belongs to the producer handle")]
    RoleMismatch,
    /// The geometry could not be laid out on this page size.
    #[error("pool geometry is invalid")]
    Geometry(#[source] GeometryError),
    /// A published descriptor or completion failed validation.
    #[error("shared descriptor validation failed")]
    Descriptor(#[source] DescriptorError),
    /// A validated frame could not be turned into a lease view.
    #[error("receive lease construction failed")]
    Lease(#[source] LeaseError),
}

impl From<MappingError> for RingError {
    fn from(error: MappingError) -> Self {
        match error {
            MappingError::ObjectSetupFailed => Self::ObjectSetupFailed,
            MappingError::ObjectValidationFailed => Self::ObjectValidationFailed,
            MappingError::InvalidLayout => Self::InvalidLayout,
            MappingError::ArithmeticOverflow => Self::ArithmeticOverflow,
        }
    }
}

impl fmt::Debug for RingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Accessor tests over an anonymous mapping. Miri cannot map a memfd or open a doorbell that
/// sends, so this module exercises the layer of `ring.rs` that touches raw shared memory
/// without constructing a `Ring`: slot copy-out, completion-cell monotonicity, and lifecycle
/// validation.
#[cfg(test)]
mod miri {
    use std::sync::atomic::Ordering;

    use crate::backend::retained::validate_lifecycle;
    use crate::descriptor::{DescriptorError, Incarnation, PoolDescriptor};
    use crate::lease::retained_fixture;

    #[test]
    fn every_page_accessor_reads_the_initialized_zero_state() {
        let retained = retained_fixture();
        assert_eq!(
            retained
                .producer()
                .unwrap()
                .published
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            retained
                .consumer()
                .unwrap()
                .consumed
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            retained.data_wake().unwrap().parked.load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            retained
                .capacity_wake()
                .unwrap()
                .generation
                .load(Ordering::Relaxed),
            0
        );
        for index in 0..retained.layout().descriptor_depth {
            let slot = retained.slot(index).unwrap();
            assert_eq!(slot.sequence.load(Ordering::Relaxed), 0);
            assert_eq!(slot.generation.load(Ordering::Relaxed), 0);
        }
        for block in 0..retained.geometry().block_count() {
            assert_eq!(
                retained
                    .completion(block)
                    .unwrap()
                    .generation
                    .load(Ordering::Relaxed),
                0
            );
        }
        assert_eq!(
            retained
                .lifecycle_quarantined()
                .unwrap()
                .load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn slot_and_cell_indexes_past_their_regions_are_refused_before_any_dereference() {
        let retained = retained_fixture();
        assert!(retained.slot(retained.layout().descriptor_depth).is_err());
        assert!(
            retained
                .completion(retained.geometry().block_count())
                .is_err()
        );
        assert!(
            retained
                .body_ptr(retained.geometry().block_count(), 0)
                .is_err()
        );
        assert!(
            retained.body_ptr(0, 4096).is_err(),
            "body past the block's capacity"
        );
    }

    #[test]
    fn descriptor_snapshot_is_copied_out_field_by_field_and_validated() {
        let retained = retained_fixture();
        let slot = retained.slot(1).unwrap();
        slot.block.store(2, Ordering::Relaxed);
        slot.generation.store(7, Ordering::Relaxed);
        slot.body_len.store(40, Ordering::Relaxed);
        slot.sequence.store(2, Ordering::Release);
        let snapshot = PoolDescriptor::from_untrusted(
            slot.sequence.load(Ordering::Acquire),
            slot.block.load(Ordering::Relaxed),
            slot.generation.load(Ordering::Relaxed),
            slot.body_len.load(Ordering::Relaxed),
        );
        let validated = snapshot.validate(2, retained.geometry()).unwrap();
        assert_eq!(validated.block(), 2);
        assert_eq!(validated.generation(), 7);
        assert_eq!(validated.body_len(), 40);
        assert_eq!(
            snapshot.validate(3, retained.geometry()).err(),
            Some(DescriptorError::InvalidSequence)
        );
        // A peer that rewrites the slot after the copy changes nothing the receiver holds.
        slot.body_len.store(1 << 40, Ordering::Relaxed);
        assert_eq!(validated.body_len(), 40);
    }

    #[test]
    fn completion_cell_publication_is_monotonic() {
        let retained = retained_fixture();
        let cell = retained.completion(3).unwrap();
        cell.generation.fetch_max(5, Ordering::Release);
        cell.generation.fetch_max(3, Ordering::Release);
        assert_eq!(cell.generation.load(Ordering::Acquire), 5);
        cell.generation.fetch_max(6, Ordering::Release);
        assert_eq!(cell.generation.load(Ordering::Acquire), 6);
    }

    #[test]
    fn lifecycle_snapshot_sees_a_write_made_through_the_raw_page() {
        let retained = retained_fixture();
        let snapshot = retained.lifecycle_snapshot().unwrap();
        let incarnation = Incarnation::from_bytes([7; 16]);
        assert!(validate_lifecycle(
            snapshot,
            retained.geometry(),
            incarnation,
            3,
            retained.layout().total
        ));
        assert!(!validate_lifecycle(
            snapshot,
            retained.geometry(),
            incarnation,
            4,
            retained.layout().total
        ));
        let page = retained
            .mapping()
            .ptr_at::<crate::backend::retained::LifecyclePage>(retained.layout().lifecycle)
            .unwrap();
        // SAFETY: `ptr_at` bounds-checked the page; this is the peer's view, a raw write to a
        // plain field with no reference outstanding.
        unsafe { std::ptr::write_volatile(std::ptr::addr_of_mut!((*page).lane), 4) };
        assert_eq!(retained.lifecycle_snapshot().unwrap().lane, 4);
        assert!(!validate_lifecycle(
            retained.lifecycle_snapshot().unwrap(),
            retained.geometry(),
            incarnation,
            3,
            retained.layout().total
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use crate::descriptor::{
        DescriptorError, HardwareProfileId, SETUP_MAPPING_COUNT, TransportDescriptor,
    };
    use crate::lease::PayloadLease;
    use crate::pool::{BlockClass, ClassSpec, Inventory, PoolGeometry};
    use crate::profile::{ProfileConfig, TargetProfile, WorkerTopology};

    use super::{
        LAYOUT_VERSION, PoolGrant, ProducerError, Ring, RingAttachment, RingError, sys,
        wire_v3_header,
    };

    pub(crate) fn tiny_geometry() -> PoolGeometry {
        PoolGeometry::new(
            2,
            1,
            [
                ClassSpec::new(4096, 3),
                ClassSpec::new(64 * 1024, 2),
                ClassSpec::new(1024 * 1024, 1),
                ClassSpec::new(8 * 1024 * 1024, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 2),
            ClassSpec::new(32 * 1024, 2),
        )
        .unwrap()
    }

    pub(crate) fn profile_for(geometry: PoolGeometry) -> TargetProfile {
        TargetProfile::new(ProfileConfig {
            descriptor: TransportDescriptor::new(HardwareProfileId::new("pool-test-v1").unwrap()),
            geometry,
            mappings: SETUP_MAPPING_COUNT,
            pinned_workers: 0,
            worker_topology: WorkerTopology::CallerThread,
        })
        .unwrap()
    }

    /// Producer and consumer handles over one direction, both in this process.
    pub(crate) fn pair(geometry: PoolGeometry) -> (Ring, Ring) {
        let producer = Ring::create(&profile_for(geometry), 5).unwrap();
        let consumer = producer.attachment().unwrap().attach().unwrap();
        (producer, consumer)
    }

    pub(crate) fn publish(ring: &Ring, bytes: &[u8]) -> u32 {
        let mut reservation = ring
            .try_reserve(bytes.len(), wire_v3_header(bytes.len()).unwrap())
            .unwrap();
        reservation.write(bytes).unwrap();
        reservation.commit(bytes.len()).unwrap().block()
    }

    fn receive(ring: &Ring) -> PayloadLease {
        ring.try_receive().unwrap().expect("a frame is published")
    }

    #[test]
    fn held_payload_stays_intact_while_another_block_is_reused_beyond_descriptor_laps() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        let depth = geometry.descriptor_depth() as usize;
        let a_bytes = vec![0xa5u8; 1000];
        let a_block = publish(&producer, &a_bytes);
        let a = receive(&consumer);
        assert_eq!(a.identity().block(), a_block);
        let mut b_blocks = Vec::new();
        for cycle in 0..(2 * depth + 1) {
            let b_bytes = vec![cycle as u8; 900];
            let block = publish(&producer, &b_bytes);
            let b = receive(&consumer);
            assert_eq!(b.to_vec().unwrap(), b_bytes);
            b_blocks.push(block);
            b.release().unwrap();
            assert_eq!(a.to_vec().unwrap(), a_bytes, "cycle {cycle} disturbed A");
        }
        assert!(
            b_blocks.iter().all(|block| *block != a_block),
            "B never took A's block"
        );
        assert!(
            b_blocks.iter().skip(1).any(|block| *block == b_blocks[0]),
            "B's block was reused while A stayed live"
        );
        let inventory = producer.inventory();
        assert!(inventory.conserves(&geometry));
        assert_eq!(inventory.classes[0].published, 1, "only A is outstanding");
        drop(a);
        producer.probe().unwrap();
        assert_eq!(producer.inventory().classes[0].published, 0);
    }

    #[test]
    fn descriptor_consumption_frees_a_slot_while_the_payload_stays_held() {
        let geometry = PoolGeometry::new(
            1,
            1,
            [
                ClassSpec::new(4096, 4),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"first");
        assert_eq!(
            producer
                .try_reserve(5, wire_v3_header(5).unwrap())
                .err()
                .map(|error| error.to_string()),
            Some(ProducerError::Exhausted.to_string()),
            "one ordinary descriptor is outstanding"
        );
        let held = receive(&consumer);
        // The slot is free although the payload is held; a new block carries the next frame.
        let second = publish(&producer, b"second");
        assert_ne!(second, held.identity().block());
        assert_eq!(receive(&consumer).to_vec().unwrap(), b"second");
        assert_eq!(held.to_vec().unwrap(), b"first");
    }

    #[test]
    fn every_class_backpressures_without_spill_and_returns_wake_capacity() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        let count = geometry.class(BlockClass::Ordinary(0)).count as usize;
        let mut held = Vec::new();
        for _ in 0..count {
            publish(&producer, &[1u8; 100]);
            held.push(receive(&consumer));
        }
        assert!(matches!(
            producer.try_reserve(100, wire_v3_header(100).unwrap()),
            Err(ProducerError::Exhausted)
        ));
        // A larger class still serves its own bound: no spill in either direction.
        let large = producer
            .try_reserve(5000, wire_v3_header(5000).unwrap())
            .unwrap();
        assert_eq!(large.class(), BlockClass::Ordinary(1));
        large.abort();
        held.pop().unwrap().release().unwrap();
        let reservation = producer
            .try_reserve(100, wire_v3_header(100).unwrap())
            .unwrap();
        assert_eq!(reservation.class(), BlockClass::Ordinary(0));
        reservation.abort();
        assert!(producer.inventory().conserves(&geometry));
    }

    #[test]
    fn abort_underfill_and_short_commit_conserve_blocks_and_records() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        let free_before = producer.inventory().classes[0].free;
        producer
            .try_reserve(10, wire_v3_header(10).unwrap())
            .unwrap()
            .abort();
        assert_eq!(producer.inventory().classes[0].free, free_before);
        let mut underfilled = producer
            .try_reserve(100, wire_v3_header(100).unwrap())
            .unwrap();
        underfilled.write(&[9; 10]).unwrap();
        assert!(matches!(
            underfilled.commit(100),
            Err(ProducerError::Underfill)
        ));
        assert_eq!(producer.inventory().classes[0].free, free_before);
        assert_eq!(producer.inventory().descriptors_outstanding, 0);
        let mut short = producer
            .try_reserve(100, wire_v3_header(10).unwrap())
            .unwrap();
        short.write(&[8; 10]).unwrap();
        let identity = short.commit(10).unwrap();
        assert_eq!(producer.inventory().classes[0].free, free_before - 1);
        let lease = receive(&consumer);
        assert_eq!(lease.identity(), identity);
        assert_eq!(lease.to_vec().unwrap(), [8; 10]);
        lease.release().unwrap();
        producer.probe().unwrap();
        assert_eq!(producer.inventory().classes[0].free, free_before);
        assert!(producer.inventory().conserves(&geometry));
    }

    #[test]
    fn zero_body_class_boundaries_maximum_and_maximum_plus_one_have_explicit_outcomes() {
        let geometry = PoolGeometry::host_payload_pool();
        let (producer, consumer) = pair(geometry);
        let mut cases: Vec<(usize, BlockClass)> = vec![(0, BlockClass::Ordinary(0))];
        for (index, class) in geometry.classes()[..crate::pool::ORDINARY_CLASSES]
            .iter()
            .enumerate()
        {
            let capacity = class.body_capacity() as usize;
            let this = BlockClass::Ordinary(index as u8);
            if capacity <= crate::pool::MAX_FRAME_BYTES {
                cases.push((capacity - 1, this));
                cases.push((capacity, this));
            }
            if index + 1 < crate::pool::ORDINARY_CLASSES {
                cases.push((capacity + 1, BlockClass::Ordinary(index as u8 + 1)));
            }
        }
        cases.push((crate::pool::MAX_FRAME_BYTES, BlockClass::Ordinary(4)));
        for (len, expected_class) in cases {
            let body: Vec<u8> = (0..len).map(|index| (index % 251) as u8).collect();
            let mut reservation = producer
                .try_reserve(len, wire_v3_header(len).unwrap())
                .unwrap();
            assert_eq!(reservation.class(), expected_class, "body of {len} bytes");
            assert_eq!(reservation.capacity(), len, "the caller sees its bound");
            reservation.write(&body).unwrap();
            reservation.commit(len).unwrap();
            let lease = receive(&consumer);
            assert_eq!(lease.len(), len);
            assert_eq!(lease.to_vec().unwrap(), body, "body of {len} bytes");
            lease.release().unwrap();
        }
        assert!(matches!(
            producer.try_reserve(crate::pool::MAX_FRAME_BYTES + 1, wire_v3_header(1).unwrap()),
            Err(ProducerError::BoundExceedsClass)
        ));
        producer.probe().unwrap();
        assert!(producer.inventory().conserves(&geometry));
    }

    #[test]
    fn reserved_inventories_progress_when_ordinary_descriptors_are_exhausted() {
        let geometry = PoolGeometry::new(
            1,
            2,
            [
                ClassSpec::new(4096, 4),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"data");
        assert!(matches!(
            producer.try_reserve(4, wire_v3_header(4).unwrap()),
            Err(ProducerError::Exhausted)
        ));
        let mut control = producer
            .try_reserve_in(Inventory::Control, 0, wire_v3_header(0).unwrap())
            .unwrap();
        assert_eq!(control.class(), BlockClass::Control);
        control.write(&[]).unwrap();
        control.commit(0).unwrap();
        let mut terminal = producer
            .try_reserve_in(Inventory::Terminal, 3, wire_v3_header(3).unwrap())
            .unwrap();
        assert_eq!(terminal.class(), BlockClass::Terminal);
        terminal.write(b"end").unwrap();
        terminal.commit(3).unwrap();
        // Every reserved slot is now taken too.
        assert!(matches!(
            producer.try_reserve_in(Inventory::Control, 0, wire_v3_header(0).unwrap()),
            Err(ProducerError::Exhausted)
        ));
        assert_eq!(receive(&consumer).to_vec().unwrap(), b"data");
        let control = receive(&consumer);
        assert!(control.is_empty());
        assert_eq!(receive(&consumer).to_vec().unwrap(), b"end");
        // A control block never serves ordinary data even when ordinary classes are empty.
        assert!(matches!(
            producer.try_reserve_in(Inventory::Control, 4096, wire_v3_header(4096).unwrap()),
            Err(ProducerError::BoundExceedsClass)
        ));
    }

    #[test]
    fn forged_descriptors_quarantine_before_exposing_bytes() {
        fn forged(mutate: impl Fn(&super::Retained, &Ring, &Ring)) -> Result<(), RingError> {
            let (producer, consumer) = pair(tiny_geometry());
            publish(&producer, b"honest");
            mutate(consumer.retained(), &producer, &consumer);
            consumer.try_receive().map(|lease| assert!(lease.is_some()))
        }
        fn slot(retained: &super::Retained) -> &super::super::retained::DescriptorSlot {
            retained.slot(0).unwrap()
        }
        assert_eq!(
            forged(|retained, _, _| slot(retained)
                .block
                .store(u64::from(tiny_geometry().block_count()), Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::InvalidBlock))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).block.store(u64::MAX, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::InvalidBlock))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).body_len.store(4096, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::BodyExceedsBlock))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).body_len.store(u64::MAX, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::FrameTooLarge))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).generation.store(0, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::InvalidGeneration))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).sequence.store(2, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::InvalidSequence))
        );
        assert_eq!(
            forged(|retained, _, _| slot(retained).body_len.store(5, Ordering::Relaxed)),
            Err(RingError::Descriptor(DescriptorError::WireHeaderMismatch)),
            "the header in the block declares six bytes"
        );
        // A republished descriptor for a block whose lease is still live.
        let (producer, consumer) = pair(tiny_geometry());
        publish(&producer, b"one");
        let live = receive(&consumer);
        publish(&producer, b"two");
        let second = consumer.retained().slot(1).unwrap();
        second
            .block
            .store(u64::from(live.identity().block()), Ordering::Relaxed);
        second.generation.store(2, Ordering::Relaxed);
        assert_eq!(
            consumer.try_receive().err(),
            Some(RingError::Descriptor(DescriptorError::DuplicateLiveBlock))
        );
        drop(live);
        // A generation that does not increase on a returned block.
        let (producer, consumer) = pair(tiny_geometry());
        let block = publish(&producer, b"one");
        receive(&consumer).release().unwrap();
        publish(&producer, b"two");
        let second = consumer.retained().slot(1).unwrap();
        second.block.store(u64::from(block), Ordering::Relaxed);
        second.generation.store(1, Ordering::Relaxed);
        assert_eq!(
            consumer.try_receive().err(),
            Some(RingError::Descriptor(
                DescriptorError::NonIncreasingGeneration
            ))
        );
        assert!(consumer.is_quarantined());
        assert!(
            producer.is_quarantined(),
            "the shared flag reached the peer"
        );
    }

    #[test]
    fn stale_returns_free_nothing_and_future_completions_quarantine() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        let block = publish(&producer, b"first");
        receive(&consumer).release().unwrap();
        assert_eq!(publish(&producer, b"second"), block, "the block was reused");
        let second = receive(&consumer);
        // A late return of the first generation lands in the same cell and is ignored.
        let cell = consumer.retained().completion(block).unwrap();
        cell.generation.fetch_max(1, Ordering::Release);
        assert!(matches!(
            producer.try_reserve(4000, wire_v3_header(4000).unwrap()).map(|r| r.block()),
            Ok(other) if other != block
        ));
        assert_eq!(producer.inventory().classes[0].published, 1);
        assert_eq!(second.to_vec().unwrap(), b"second");
        // A completion ahead of anything issued is a protocol error.
        cell.generation.store(99, Ordering::Release);
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Ring(RingError::Descriptor(
                DescriptorError::FutureCompletion
            )))
        ));
        assert!(producer.is_quarantined());
    }

    #[test]
    fn retirement_at_a_counter_boundary_preserves_live_leases() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"live");
        let live = receive(&consumer);
        // The next 4 KiB reservation pops block 1, so its generation is placed at the wrap.
        producer.set_block_generation_for_test(1, u64::MAX);
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Retired)
        ));
        assert!(producer.is_retired());
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Retired)
        ));
        assert_eq!(live.to_vec().unwrap(), b"live");
        assert!(producer.inventory().conserves(&geometry));
        live.release().unwrap();
        assert!(!producer.is_quarantined(), "retirement is not quarantine");

        let (producer, consumer) = pair(geometry);
        producer.set_sequence_for_test(u64::MAX).unwrap();
        consumer.set_sequence_for_test(u64::MAX).unwrap();
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Retired)
        ));
        assert_eq!(consumer.try_receive().unwrap().map(|_| ()), None);
    }

    #[test]
    fn owned_lease_outlives_both_endpoint_handles_and_returns_once() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"retained");
        let lease = receive(&consumer);
        let weak = std::sync::Arc::downgrade(lease.retained());
        drop(consumer);
        drop(producer);
        assert!(
            weak.upgrade().is_some(),
            "the lease keeps the consumer backing mapped"
        );
        assert_eq!(lease.to_vec().unwrap(), b"retained");
        let worker = std::thread::spawn(move || lease.release());
        // The producer's doorbell end is gone, but the wake is best effort and the completion
        // still publishes.
        let _ = worker.join().unwrap();
        assert!(
            weak.upgrade().is_none(),
            "the final owner unmapped the backing"
        );
    }

    #[test]
    fn worker_final_drop_wakes_a_capacity_parked_producer_without_incoming_data() {
        let geometry = PoolGeometry::new(
            4,
            1,
            [
                ClassSpec::new(4096, 1),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"only");
        let lease = receive(&consumer);
        let before = producer.syscall_counters();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(lease);
        });
        let started = Instant::now();
        let reservation = producer
            .reserve_until(
                4,
                wire_v3_header(4).unwrap(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_secs(4));
        reservation.abort();
        worker.join().unwrap();
        let counters = producer.syscall_counters().since(before);
        assert!(counters.parks >= 1, "the producer parked: {counters:?}");
    }

    #[test]
    fn descriptor_consumption_alone_wakes_a_descriptor_parked_producer() {
        let geometry = PoolGeometry::new(
            1,
            1,
            [
                ClassSpec::new(4096, 4),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap();
        let producer = Ring::create(&profile_for(geometry), 1).unwrap();
        let attachment = producer.attachment().unwrap();
        publish(&producer, b"parked");
        let (held_tx, held_rx) = std::sync::mpsc::channel::<PayloadLease>();
        let consumer = std::thread::spawn(move || {
            let consumer = attachment.attach().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let lease = receive(&consumer);
            // The payload stays held on this thread; only the descriptor was consumed.
            held_tx.send(lease).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        let reservation = producer
            .reserve_until(
                6,
                wire_v3_header(6).unwrap(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        let held = held_rx.recv().unwrap();
        assert_eq!(held.to_vec().unwrap(), b"parked");
        reservation.abort();
        consumer.join().unwrap();
        drop(held);
    }

    #[test]
    fn wake_failure_after_publication_quarantines_but_leaves_the_frame_published() {
        let geometry = tiny_geometry();
        let producer = Ring::create(&profile_for(geometry), 2).unwrap();
        let (descriptors, _grant) = producer.attachment().unwrap().into_parts();
        // The peer's data doorbell end is closed: it will park nowhere, but a parked marker
        // forces the producer to send, and the send fails with `EPIPE`.
        drop(descriptors);
        producer
            .retained()
            .data_wake()
            .unwrap()
            .parked
            .store(1, Ordering::SeqCst);
        let mut reservation = producer.try_reserve(3, wire_v3_header(3).unwrap()).unwrap();
        reservation.write(b"pub").unwrap();
        assert!(matches!(
            reservation.commit(3),
            Err(ProducerError::Ring(RingError::DoorbellFailed))
        ));
        assert!(producer.is_quarantined());
        assert_eq!(
            producer
                .retained()
                .producer()
                .unwrap()
                .published
                .load(Ordering::Acquire),
            1,
            "publication stands"
        );
    }

    #[test]
    fn attach_rejects_eventfd_and_datagram_doorbells_and_a_second_producer() {
        let geometry = tiny_geometry();
        let producer = Ring::create(&profile_for(geometry), 3).unwrap();
        let (descriptors, grant) = producer.attachment().unwrap().into_parts();
        let [mapping, data, capacity] = descriptors;
        let eventfd = sys::eventfd().unwrap();
        assert!(matches!(
            Ring::attach(
                [
                    mapping.try_clone().unwrap(),
                    eventfd,
                    capacity.try_clone().unwrap()
                ],
                grant
            ),
            Err(RingError::DoorbellFailed)
        ));
        let (datagram, _other) = std::os::unix::net::UnixDatagram::pair().unwrap();
        assert!(matches!(
            Ring::attach(
                [
                    mapping.try_clone().unwrap(),
                    data.try_clone().unwrap(),
                    OwnedFd::from(datagram)
                ],
                grant
            ),
            Err(RingError::DoorbellFailed)
        ));
        let unconnected = sys::unix_stream_socket().unwrap();
        assert!(matches!(
            Ring::attach(
                [
                    mapping.try_clone().unwrap(),
                    unconnected,
                    capacity.try_clone().unwrap()
                ],
                grant
            ),
            Err(RingError::DoorbellFailed)
        ));
        let not_a_socket: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(matches!(
            Ring::attach(
                [
                    not_a_socket,
                    data.try_clone().unwrap(),
                    capacity.try_clone().unwrap()
                ],
                grant
            ),
            Err(RingError::ObjectValidationFailed)
        ));
        publish(&producer, b"traffic");
        let late = Ring::attach([mapping, data, capacity], grant).unwrap();
        assert!(!late.is_fresh());
        assert!(matches!(
            late.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Ring(RingError::RoleMismatch))
        ));
        assert_eq!(
            late.try_receive().unwrap().unwrap().to_vec().unwrap(),
            b"traffic"
        );
    }

    #[test]
    fn attach_sets_close_on_exec_on_every_descriptor() {
        use std::os::fd::AsFd;
        let producer = Ring::create(&profile_for(tiny_geometry()), 6).unwrap();
        let (descriptors, grant) = producer.attachment().unwrap().into_parts();
        for descriptor in &descriptors {
            sys::clear_cloexec(descriptor.as_fd()).unwrap();
            assert!(!sys::is_cloexec(descriptor.as_fd()).unwrap());
        }
        let raw: Vec<i32> = descriptors.iter().map(|fd| fd.as_raw_fd()).collect();
        let consumer = Ring::attach(descriptors, grant).unwrap();
        for fd in raw {
            // SAFETY: the descriptor is owned by `consumer`, which is alive; the borrow is used
            // only for one `fcntl` query.
            let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
            assert!(sys::is_cloexec(borrowed).unwrap());
        }
        drop(consumer);
    }

    #[test]
    fn grant_round_trips_and_rejects_every_malformation() {
        let geometry = tiny_geometry();
        let producer = Ring::create(&profile_for(geometry), 9).unwrap();
        let grant = producer.grant();
        let bytes = grant.encode();
        assert_eq!(PoolGrant::decode(bytes).unwrap(), grant);
        assert_eq!(grant.lane(), 9);
        assert_eq!(*grant.geometry(), geometry);
        assert_eq!(grant.mapping_bytes() as usize, producer.object_size());
        let mut wrong_version = bytes;
        wrong_version[0..2].copy_from_slice(&(LAYOUT_VERSION + 1).to_le_bytes());
        assert_eq!(
            PoolGrant::decode(wrong_version),
            Err(RingError::InvalidGrant)
        );
        let mut tail = bytes;
        tail[super::GRANT_BYTES - 1] = 1;
        assert_eq!(PoolGrant::decode(tail), Err(RingError::InvalidGrant));
        let mut total = bytes;
        total[super::GRANT_BYTES - 12] ^= 1;
        assert_eq!(PoolGrant::decode(total), Err(RingError::InvalidGrant));
        let mut empty_class = bytes;
        empty_class[30 + 8..30 + 12].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(PoolGrant::decode(empty_class), Err(RingError::InvalidGrant));
        for cut in 0..super::GRANT_BYTES {
            assert_eq!(
                PoolGrant::decode_slice(&bytes[..cut]),
                Err(RingError::InvalidGrant)
            );
        }
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert_eq!(
            PoolGrant::decode_slice(&longer),
            Err(RingError::InvalidGrant)
        );
    }

    #[test]
    fn attachment_can_be_handed_to_another_thread_and_the_ring_cannot() {
        let geometry = tiny_geometry();
        let producer = Ring::create(&profile_for(geometry), 4).unwrap();
        let attachment: RingAttachment = producer.attachment().unwrap();
        publish(&producer, b"cross");
        let consumer = std::thread::spawn(move || {
            let consumer = attachment.attach().unwrap();
            receive(&consumer)
        });
        let lease = consumer.join().unwrap();
        assert_eq!(lease.to_vec().unwrap(), b"cross");
        drop(lease);
        producer.probe().unwrap();
        assert_eq!(producer.inventory().classes[0].published, 0);
    }

    #[test]
    fn quarantine_rejects_operations_and_survives_the_peer_clearing_the_flag() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        publish(&producer, b"before");
        consumer.enter_quarantine();
        assert!(producer.is_quarantined());
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Quarantined)
        ));
        assert_eq!(consumer.try_receive().err(), Some(RingError::Quarantined));
        producer
            .retained()
            .lifecycle_quarantined()
            .unwrap()
            .store(0, Ordering::Release);
        assert!(producer.is_quarantined(), "the local latch never clears");
        assert!(consumer.is_quarantined());
        let inventory = producer.inventory();
        assert!(
            inventory.conserves(&geometry),
            "everything reads as published"
        );
        assert!(inventory.classes.iter().all(|class| class.free == 0));
    }

    #[test]
    fn peer_closing_its_doorbell_quarantines_the_waiting_side() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        drop(producer);
        assert!(matches!(
            consumer.wait_for_data(Instant::now() + Duration::from_millis(200)),
            Err(RingError::DoorbellFailed | RingError::Quarantined)
        ));
        assert!(consumer.is_quarantined());
    }

    #[test]
    fn forbidden_operation_observers_stay_unreached_across_a_saturated_drop_storm() {
        let geometry = tiny_geometry();
        let (producer, consumer) = pair(geometry);
        let before = crate::lease::observers::snapshot();
        let count = geometry.class(BlockClass::Ordinary(0)).count as usize;
        let mut leases = Vec::new();
        for _ in 0..count {
            publish(&producer, &[3u8; 50]);
            leases.push(receive(&consumer));
        }
        assert!(matches!(
            producer.try_reserve(1, wire_v3_header(1).unwrap()),
            Err(ProducerError::Exhausted)
        ));
        let workers: Vec<_> = leases
            .into_iter()
            .map(|lease| std::thread::spawn(move || drop(lease)))
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        drop(consumer);
        drop(producer);
        assert_eq!(
            crate::lease::observers::snapshot(),
            before,
            "no final drop reached a forbidden operation"
        );
    }
}
