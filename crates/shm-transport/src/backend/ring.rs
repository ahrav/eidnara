//! Single-producer, single-consumer descriptor ring over one sealed memfd.
//!
//! One `Ring` is one direction. The mapping holds control pages (producer, consumer, reclaim,
//! two wake epochs, lifecycle), a ring of `DescriptorSlot`s, and the payload arena. The
//! producer reserves a slot and arena span, writes the body, then publishes; the consumer
//! validates the descriptor against the identity it expects, leases the body, and releases.
//! Released slots return to the producer in FIFO order, so a stalled oldest lease holds back
//! reclamation of everything behind it.
//!
//! Reclaimed arena pages go back to the kernel through `MADV_REMOVE`.
//! Reclamation punches once a quarter of the arena is dead; `trim` punches whatever is left.
//! Only pages with no live byte are ever punched.
//!
//! Both peers can write the mapping. Every value read from it is treated as untrusted:
//! descriptors are snapshotted then validated, and cursors are checked for wrap and overflow.
//! Impossible shared-memory state quarantines the ring; the local latch keeps quarantine
//! terminal if a peer clears the shared flag.
#[cfg(not(target_os = "linux"))]
compile_error!("shm-transport ring backend supports Linux only");

use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::fd::RawFd;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use crate::arena::{ArenaCounts, ArenaError, ArenaSpan, MAX_FRAME_BYTES, SpanPlan};
use crate::backend::sys;
use crate::descriptor::{
    DESCRIPTOR_SCHEMA_VERSION, DescriptorCounts, DescriptorError, FrameDescriptor, Incarnation,
    MAX_SPANS, ReleaseIdentity, WIRE_V2_HEADER_BYTES, WIRE_V2_VERSION, check_wire_header,
};
use crate::lease::{LeaseError, LeaseSpan, ReceiveLease, ReleaseSink, volatile_copy};
use crate::profile::TargetProfile;

const MAPPING_MAGIC: u64 = 0x4d43_5348_4d52_3031;
const LAYOUT_VERSION: u16 = 3;
const CACHELINE: usize = 128;
const PAGE_SIZE: usize = 4096;
const GRANT_BYTES: usize = 58;
const PUNCH_BATCH_DIVISOR: u64 = 4;
/// Upper bound on `descriptor_depth`. Each slot costs 256 bytes of mapping plus a heap entry in
/// the producer's allocation record, and the record is allocated before a grant's mapping is
/// validated, so an unbounded depth in a hostile grant would be an allocation attack.
pub const MAX_DESCRIPTOR_DEPTH: usize = 4096;

const SLOT_FREE: u8 = 0;
const SLOT_PRODUCER_RESERVED: u8 = 1;
const SLOT_PUBLISHED: u8 = 2;
const SLOT_RECEIVER_HELD: u8 = 3;
const SLOT_RECEIVER_LEASED: u8 = 4;
const SLOT_RELEASE_PENDING: u8 = 5;

#[repr(C, align(128))]
struct ProducerPage {
    published: AtomicU64,
    arena_write: AtomicU64,
}

#[repr(C, align(128))]
struct ConsumerPage {
    consumed: AtomicU64,
    active_leases: AtomicU64,
}

#[repr(C, align(128))]
struct ReclaimPage {
    completed: AtomicU64,
    arena_reclaimed: AtomicU64,
}

#[repr(C, align(128))]
struct WakeEpoch {
    generation: AtomicU64,
    parked: AtomicU64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SharedDescriptor {
    schema_version: u16,
    wire_header: [u8; WIRE_V2_HEADER_BYTES],
    incarnation: [u8; 16],
    lane: u32,
    sequence: u64,
    body_len: u64,
    allocation_start: u64,
    allocation_len: u64,
    span_count: u8,
    span_offsets: [u64; 2],
    span_lengths: [u64; 2],
}

impl SharedDescriptor {
    const ZERO: Self = Self {
        schema_version: 0,
        wire_header: [0; WIRE_V2_HEADER_BYTES],
        incarnation: [0; 16],
        lane: 0,
        sequence: 0,
        body_len: 0,
        allocation_start: 0,
        allocation_len: 0,
        span_count: 0,
        span_offsets: [0; 2],
        span_lengths: [0; 2],
    };

    fn snapshot(self) -> FrameDescriptor {
        FrameDescriptor::from_untrusted(
            self.schema_version,
            self.wire_header,
            ReleaseIdentity::new(
                Incarnation::from_bytes(self.incarnation),
                self.lane,
                self.sequence,
            ),
            self.body_len,
            self.allocation_start,
            self.allocation_len,
            self.span_count,
            [
                ArenaSpan::from_untrusted(self.span_offsets[0], self.span_lengths[0]),
                ArenaSpan::from_untrusted(self.span_offsets[1], self.span_lengths[1]),
            ],
        )
    }
}

#[repr(C, align(128))]
struct DescriptorSlot {
    state: AtomicU8,
    completion_sequence: AtomicU64,
    reservation_len: AtomicU64,
    descriptor: UnsafeCell<SharedDescriptor>,
}

// `DescriptorSlot` tail padding can hide `SharedDescriptor` layout changes from a slot-size
// assertion; these assertions reject them at compile time.
const _: () = {
    use std::mem::offset_of;
    assert!(size_of::<SharedDescriptor>() == 120);
    assert!(offset_of!(SharedDescriptor, schema_version) == 0);
    assert!(offset_of!(SharedDescriptor, wire_header) == 2);
    assert!(offset_of!(SharedDescriptor, incarnation) == 23);
    assert!(offset_of!(SharedDescriptor, lane) == 40);
    assert!(offset_of!(SharedDescriptor, sequence) == 48);
    assert!(offset_of!(SharedDescriptor, body_len) == 56);
    assert!(offset_of!(SharedDescriptor, allocation_start) == 64);
    assert!(offset_of!(SharedDescriptor, allocation_len) == 72);
    assert!(offset_of!(SharedDescriptor, span_count) == 80);
    assert!(offset_of!(SharedDescriptor, span_offsets) == 88);
    assert!(offset_of!(SharedDescriptor, span_lengths) == 104);
    assert!(size_of::<DescriptorSlot>() == 256);
    assert!(offset_of!(DescriptorSlot, state) == 0);
    assert!(offset_of!(DescriptorSlot, completion_sequence) == 8);
    assert!(offset_of!(DescriptorSlot, reservation_len) == 16);
    assert!(offset_of!(DescriptorSlot, descriptor) == 24);
    assert!(size_of::<ProducerPage>() == CACHELINE);
    assert!(size_of::<ConsumerPage>() == CACHELINE);
    assert!(size_of::<ReclaimPage>() == CACHELINE);
    assert!(size_of::<WakeEpoch>() == CACHELINE);
    assert!(size_of::<LifecyclePage>() == CACHELINE);
};

impl DescriptorSlot {
    /// One volatile read of the whole descriptor. The peer may write the cell at any time, so
    /// the result is an untrusted snapshot, never a reference into the cell.
    fn read_descriptor(&self) -> SharedDescriptor {
        // SAFETY: `get()` points at a live `SharedDescriptor` inside the mapping (initialized
        // by `initialize_mapping`), every field is a plain integer valid for all bit patterns,
        // and no `&SharedDescriptor` into the cell exists in this process.
        unsafe { std::ptr::read_volatile(self.descriptor.get()) }
    }

    fn write_descriptor(&self, descriptor: SharedDescriptor) {
        // SAFETY: as in `read_descriptor`; the slot state machine gives the writer exclusive
        // ownership of the cell while it is producer-reserved.
        unsafe { std::ptr::write_volatile(self.descriptor.get(), descriptor) }
    }
}

#[repr(C, align(128))]
struct LifecyclePage {
    magic: u64,
    layout_version: u16,
    descriptor_depth: u64,
    arena_bytes: u64,
    max_leases: u64,
    total_bytes: u64,
    incarnation: [u8; 16],
    lane: u32,
    quarantined: AtomicU8,
}

/// Volatile copy of the plain `LifecyclePage` fields. Those fields are peer-writable and not
/// atomic, so no `&LifecyclePage` is ever formed; `validate_lifecycle` compares this copy.
#[derive(Clone, Copy, PartialEq, Eq)]
struct LifecycleSnapshot {
    magic: u64,
    layout_version: u16,
    descriptor_depth: u64,
    arena_bytes: u64,
    max_leases: u64,
    total_bytes: u64,
    incarnation: [u8; 16],
    lane: u32,
}

/// The producer's own cursors as this handle last wrote them. Shared memory is peer-writable,
/// so before a producer operation trusts a cursor it checks the shared value against this
/// copy; a rewind or advance the handle did not perform is invalid shared state.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ProducerCursors {
    published: u64,
    arena_write: u64,
    completed: u64,
    arena_reclaimed: u64,
}

/// The consumer's own cursors as this handle last wrote them; same role as `ProducerCursors`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ConsumerCursors {
    consumed: u64,
    active_leases: u64,
}

/// Every shared cursor, read together for a health check.
#[derive(Clone, Copy, PartialEq, Eq)]
struct CursorSnapshot {
    published: u64,
    arena_write: u64,
    consumed: u64,
    active_leases: u64,
    completed: u64,
    arena_reclaimed: u64,
}

#[derive(Clone, Copy)]
struct Layout {
    producer: usize,
    consumer: usize,
    reclaim: usize,
    data_wake: usize,
    capacity_wake: usize,
    slots: usize,
    arena: usize,
    lifecycle: usize,
    total: usize,
}

impl Layout {
    fn new(depth: usize, arena_bytes: usize) -> Result<Self, RingError> {
        let page_size = system_page_size();
        // Page removal works in whole pages, so the arena must tile them exactly or the ring
        // would create, publish, and receive normally and then fail at its first reclaim.
        if arena_bytes == 0 || !arena_bytes.is_multiple_of(page_size) {
            return Err(RingError::InvalidLayout);
        }
        if depth == 0 || depth > MAX_DESCRIPTOR_DEPTH {
            return Err(RingError::InvalidLayout);
        }
        let producer = 0usize;
        let consumer = align_up(size_of::<ProducerPage>(), CACHELINE)?;
        let reclaim = align_up(
            consumer
                .checked_add(size_of::<ConsumerPage>())
                .ok_or(RingError::ArithmeticOverflow)?,
            CACHELINE,
        )?;
        let data_wake = align_up(
            reclaim
                .checked_add(size_of::<ReclaimPage>())
                .ok_or(RingError::ArithmeticOverflow)?,
            CACHELINE,
        )?;
        let capacity_wake = align_up(
            data_wake
                .checked_add(size_of::<WakeEpoch>())
                .ok_or(RingError::ArithmeticOverflow)?,
            CACHELINE,
        )?;
        let slots = align_up(
            capacity_wake
                .checked_add(size_of::<WakeEpoch>())
                .ok_or(RingError::ArithmeticOverflow)?,
            CACHELINE,
        )?;
        let slot_bytes = size_of::<DescriptorSlot>()
            .checked_mul(depth)
            .ok_or(RingError::ArithmeticOverflow)?;
        let arena = align_up(
            slots
                .checked_add(slot_bytes)
                .ok_or(RingError::ArithmeticOverflow)?,
            page_size,
        )?;
        let lifecycle = align_up(
            arena
                .checked_add(arena_bytes)
                .ok_or(RingError::ArithmeticOverflow)?,
            page_size,
        )?;
        let total = lifecycle
            .checked_add(page_size)
            .ok_or(RingError::ArithmeticOverflow)?;
        Ok(Self {
            producer,
            consumer,
            reclaim,
            data_wake,
            capacity_wake,
            slots,
            arena,
            lifecycle,
            total,
        })
    }

    fn slot_offset(&self, index: usize) -> Result<usize, RingError> {
        self.slots
            .checked_add(
                index
                    .checked_mul(size_of::<DescriptorSlot>())
                    .ok_or(RingError::ArithmeticOverflow)?,
            )
            .ok_or(RingError::ArithmeticOverflow)
    }
}

fn allocation_shadow(depth: usize) -> Vec<Cell<Option<(u64, u64)>>> {
    (0..depth).map(|_| Cell::new(None)).collect()
}

fn align_up(value: usize, alignment: usize) -> Result<usize, RingError> {
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|sum| sum & !mask)
        .ok_or(RingError::ArithmeticOverflow)
}

fn removal_ranges(
    arena_offset: usize,
    arena_bytes: usize,
    logical_start: u64,
    logical_len: u64,
    page_size: usize,
) -> Result<[(usize, usize); 2], RingError> {
    if arena_bytes == 0
        || page_size == 0
        || !page_size.is_power_of_two()
        || !arena_offset.is_multiple_of(page_size)
        || !arena_bytes.is_multiple_of(page_size)
    {
        return Err(RingError::InvalidLayout);
    }
    let logical_end = logical_start
        .checked_add(logical_len)
        .ok_or(RingError::ArithmeticOverflow)?;
    if logical_len > arena_bytes as u64 {
        return Err(RingError::InvalidSharedState);
    }
    let page_size = page_size as u64;
    let page_mask = !(page_size - 1);
    let removable_start = if logical_start.is_multiple_of(page_size) {
        logical_start
    } else {
        (logical_start & page_mask)
            .checked_add(page_size)
            .ok_or(RingError::ArithmeticOverflow)?
    };
    let removable_end = logical_end & page_mask;
    if removable_start >= removable_end {
        return Ok([(0, 0); 2]);
    }
    let len = usize::try_from(removable_end - removable_start)
        .map_err(|_| RingError::ArithmeticOverflow)?;
    let start = usize::try_from(removable_start % arena_bytes as u64)
        .map_err(|_| RingError::ArithmeticOverflow)?;
    let first_len = len.min(arena_bytes - start);
    let segments = [(start, first_len), (0, len - first_len)];
    let mut ranges = [(0, 0); 2];
    for (index, (offset, segment_len)) in segments.into_iter().enumerate() {
        if segment_len != 0 {
            ranges[index] = (
                arena_offset
                    .checked_add(offset)
                    .ok_or(RingError::ArithmeticOverflow)?,
                segment_len,
            );
        }
    }
    Ok(ranges)
}

#[cfg(test)]
static FAIL_NEXT_PAGE_REMOVAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn remove_pages(base: NonNull<u8>, offset: usize, len: usize) -> Result<(), RingError> {
    #[cfg(test)]
    if FAIL_NEXT_PAGE_REMOVAL.swap(false, Ordering::AcqRel) {
        return Err(RingError::PageRemovalFailed);
    }
    // SAFETY: caller supplies a page-aligned range inside the live shared mapping with no
    // live byte in it.
    unsafe { sys::madvise_remove(base, offset, len) }.map_err(|_| RingError::PageRemovalFailed)
}

fn system_page_size() -> usize {
    static PAGE_SIZE_CACHE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE_CACHE.get_or_init(|| {
        Some(sys::page_size())
            .filter(|size| *size > 0)
            .unwrap_or(PAGE_SIZE)
    })
}

fn residency_vector_len(mapping_len: usize, page_size: usize) -> usize {
    mapping_len.div_ceil(page_size.max(1))
}

struct Mapping {
    fd: OwnedFd,
    base: NonNull<u8>,
    len: usize,
}

impl Mapping {
    fn create(len: usize) -> Result<Self, RingError> {
        let fd = create_linux_memfd(len)?;

        validate_object(&fd, len)?;
        let base = sys::mmap_shared(fd.as_fd(), len).map_err(|_| RingError::ObjectSetupFailed)?;
        Ok(Self { fd, base, len })
    }

    fn attach(fd: OwnedFd, len: usize) -> Result<Self, RingError> {
        // Seals first: once `F_SEAL_SHRINK | F_SEAL_GROW` are observed the size read below
        // cannot change, so a peer cannot shrink the object between the size check and the
        // mapping and leave a page whose first touch is `SIGBUS`.
        validate_seals(&fd)?;
        validate_object(&fd, len)?;
        let base = sys::mmap_shared(fd.as_fd(), len).map_err(|_| RingError::ObjectSetupFailed)?;
        Ok(Self { fd, base, len })
    }

    const fn fd(&self) -> &OwnedFd {
        &self.fd
    }

    fn ptr_at<T>(&self, offset: usize) -> Result<*mut T, RingError> {
        let end = offset
            .checked_add(size_of::<T>())
            .ok_or(RingError::ArithmeticOverflow)?;
        if end > self.len {
            return Err(RingError::InvalidLayout);
        }
        // SAFETY: checked offset remains inside mapping.
        Ok(unsafe { self.base.as_ptr().add(offset).cast() })
    }

    /// # Safety
    ///
    /// Every field of `T` must be an atomic or an `UnsafeCell`, so that concurrent peer writes
    /// through the mapping are permitted behind `&T`. Peer stores are assumed atomic-width;
    /// a peer that tears a store violates the protocol.
    unsafe fn shared_page<T>(&self, offset: usize) -> Result<&T, RingError> {
        let ptr = self.ptr_at::<T>(offset)?;
        // SAFETY: bounds: `ptr_at` checked `offset + size_of::<T>()` against `self.len`.
        // Lifetime: the mapping is unmapped only in `Drop`, after `&self` ends.
        // Alignment: `Layout::new` places every page on a `CACHELINE` boundary.
        // Validity: atomics accept every bit pattern. Aliasing: the caller's contract above.
        Ok(unsafe { &*ptr })
    }

    pub(crate) fn producer(&self, layout: Layout) -> Result<&ProducerPage, RingError> {
        // SAFETY: `ProducerPage` is two `AtomicU64`s.
        unsafe { self.shared_page(layout.producer) }
    }

    pub(crate) fn consumer(&self, layout: Layout) -> Result<&ConsumerPage, RingError> {
        // SAFETY: `ConsumerPage` is two `AtomicU64`s.
        unsafe { self.shared_page(layout.consumer) }
    }

    pub(crate) fn reclaim(&self, layout: Layout) -> Result<&ReclaimPage, RingError> {
        // SAFETY: `ReclaimPage` is two `AtomicU64`s.
        unsafe { self.shared_page(layout.reclaim) }
    }

    pub(crate) fn data_wake(&self, layout: Layout) -> Result<&WakeEpoch, RingError> {
        // SAFETY: `WakeEpoch` is two `AtomicU64`s.
        unsafe { self.shared_page(layout.data_wake) }
    }

    pub(crate) fn capacity_wake(&self, layout: Layout) -> Result<&WakeEpoch, RingError> {
        // SAFETY: `WakeEpoch` is two `AtomicU64`s.
        unsafe { self.shared_page(layout.capacity_wake) }
    }

    /// Slot at ring index `index` (not sequence); the caller reduces modulo depth.
    pub(crate) fn slot(&self, layout: Layout, index: usize) -> Result<&DescriptorSlot, RingError> {
        // SAFETY: `DescriptorSlot` is an `AtomicU8`, two `AtomicU64`s, and an
        // `UnsafeCell<SharedDescriptor>` that is only ever accessed volatilely.
        unsafe { self.shared_page(layout.slot_offset(index)?) }
    }

    pub(crate) fn lifecycle_snapshot(
        &self,
        layout: Layout,
    ) -> Result<LifecycleSnapshot, RingError> {
        let page = self.ptr_at::<LifecyclePage>(layout.lifecycle)?;
        // SAFETY: `ptr_at` bounds-checked the page; `addr_of!` projects each field without
        // forming a reference, and every field read is a plain integer or byte array valid for
        // all bit patterns, so a concurrent peer write yields a wrong value, never UB.
        unsafe {
            use std::ptr::{addr_of, read_volatile};
            Ok(LifecycleSnapshot {
                magic: read_volatile(addr_of!((*page).magic)),
                layout_version: read_volatile(addr_of!((*page).layout_version)),
                descriptor_depth: read_volatile(addr_of!((*page).descriptor_depth)),
                arena_bytes: read_volatile(addr_of!((*page).arena_bytes)),
                max_leases: read_volatile(addr_of!((*page).max_leases)),
                total_bytes: read_volatile(addr_of!((*page).total_bytes)),
                incarnation: read_volatile(addr_of!((*page).incarnation)),
                lane: read_volatile(addr_of!((*page).lane)),
            })
        }
    }

    /// The one atomic field of the lifecycle page. Only that field is referenced; the plain
    /// fields around it stay behind the raw pointer.
    pub(crate) fn lifecycle_quarantined(&self, layout: Layout) -> Result<&AtomicU8, RingError> {
        let page = self.ptr_at::<LifecyclePage>(layout.lifecycle)?;
        // SAFETY: `ptr_at` bounds-checked the page and the mapping outlives `&self`;
        // `addr_of!` projects the field without touching its neighbors, and an `AtomicU8`
        // tolerates concurrent foreign stores through a shared reference.
        Ok(unsafe { &*std::ptr::addr_of!((*page).quarantined) })
    }

    /// Pages of `[offset, offset + len)` the kernel reports resident via `mincore`.
    fn resident_pages(&self, offset: usize, len: usize) -> Result<usize, RingError> {
        let end = offset
            .checked_add(len)
            .ok_or(RingError::ArithmeticOverflow)?;
        if end > self.len {
            return Err(RingError::InvalidLayout);
        }
        let mut residency = vec![0u8; residency_vector_len(len, system_page_size())];
        // SAFETY: `offset + len <= self.len` was checked above, and the mapping lives as long
        // as `&self`.
        unsafe { sys::mincore(self.base, offset, len, &mut residency) }
            .map_err(|_| RingError::ObjectValidationFailed)?;
        Ok(residency.into_iter().filter(|entry| entry & 1 == 1).count())
    }

    /// Writes a whole page. Only `initialize_mapping` calls this, on a mapping no peer has yet.
    fn initialize_page<T>(&self, offset: usize, value: T) -> Result<(), RingError> {
        let ptr = self.ptr_at::<T>(offset)?;
        // SAFETY: `ptr_at` bounds-checked the range and `Layout` aligns every page offset for
        // `T`; the mapping is fresh and unshared until `Ring::create` returns, so nothing else
        // reads or writes it during this store.
        unsafe { ptr.write(value) };
        Ok(())
    }

    /// Pointer into the arena for `[offset, offset + len)`, checked against `arena_bytes` and
    /// the mapping length. Callers copy through it or wrap it in a `LeaseSpan`.
    fn arena_ptr(
        &self,
        layout: Layout,
        arena_bytes: usize,
        offset: usize,
        len: usize,
    ) -> Result<*mut u8, RingError> {
        let end = offset
            .checked_add(len)
            .ok_or(RingError::ArithmeticOverflow)?;
        if end > arena_bytes {
            return Err(RingError::InvalidLayout);
        }
        let start = layout
            .arena
            .checked_add(offset)
            .ok_or(RingError::ArithmeticOverflow)?;
        if start
            .checked_add(len)
            .ok_or(RingError::ArithmeticOverflow)?
            > self.len
        {
            return Err(RingError::InvalidLayout);
        }
        // SAFETY: `start + len <= self.len`, so the pointer stays inside the mapping.
        Ok(unsafe { self.base.as_ptr().add(start) })
    }
}

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

impl fmt::Debug for Mapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Mapping(<redacted>)")
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: base and len came from one successful mmap_shared and are unmapped once
        // here, in Drop, after every borrow of the mapping has ended.
        let _ = unsafe { sys::munmap(self.base, self.len) };
    }
}

/// Syscalls this process has issued through a ring handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyscallCounters {
    /// `send`, `recv`, and `poll` calls on either doorbell.
    pub doorbell: u64,
    /// Blocking doorbell `poll` calls issued after a zero-timeout probe found nothing ready;
    /// each is one park/wake transition.
    pub parks: u64,
    /// `madvise(MADV_REMOVE)` calls that punched dead arena pages.
    pub page_removals: u64,
}

impl SyscallCounters {
    /// Every counted syscall; `parks` is a subset of `doorbell`.
    pub const fn total(self) -> u64 {
        self.doorbell.wrapping_add(self.page_removals)
    }

    /// Field-wise sum.
    pub const fn add(self, other: Self) -> Self {
        Self {
            doorbell: self.doorbell.wrapping_add(other.doorbell),
            parks: self.parks.wrapping_add(other.parks),
            page_removals: self.page_removals.wrapping_add(other.page_removals),
        }
    }

    /// Field-wise difference, saturating at zero.
    pub const fn since(self, earlier: Self) -> Self {
        Self {
            doorbell: self.doorbell.saturating_sub(earlier.doorbell),
            parks: self.parks.saturating_sub(earlier.parks),
            page_removals: self.page_removals.saturating_sub(earlier.page_removals),
        }
    }
}

/// One wake channel between the peers, built on a `socketpair`. Each socketpair endpoint has a
/// separate open file description, so peer-set status flags such as `O_NONBLOCK` cannot
/// affect `local`. `MSG_DONTWAIT` prevents blocking regardless of status flags, and
/// `MSG_NOSIGNAL` keeps a closed peer end from raising `SIGPIPE`.
struct Doorbell {
    local: UnixStream,
    /// The peer's end. `attachment` moves it out, so after the handoff only the peer holds
    /// that end and its exit is visible here as EOF or `EPIPE`.
    remote: Cell<Option<OwnedFd>>,
    counters: Cell<SyscallCounters>,
}

/// Each `drain` call consumes at most this many bytes; remaining bytes only cause a spurious
/// wake, so a flooding peer cannot keep `drain` spinning.
const DRAIN_BYTES: usize = 256;

impl Doorbell {
    fn create() -> Result<Self, RingError> {
        let (local, remote) = UnixStream::pair().map_err(|_| RingError::DoorbellFailed)?;
        local
            .set_nonblocking(true)
            .and_then(|()| remote.set_nonblocking(true))
            .map_err(|_| RingError::DoorbellFailed)?;
        Ok(Self {
            local,
            remote: Cell::new(Some(remote.into())),
            counters: Cell::new(SyscallCounters::default()),
        })
    }

    /// Accepts only a connected `AF_UNIX` stream socket. `UnixStream` itself admits any fd,
    /// and `peer_addr` proves only that the peer is `AF_UNIX`, so the socket type is checked
    /// here: `drain` reads a zero-length `recv` as peer close, which a datagram or seqpacket
    /// socket makes ambiguous.
    fn from_fd(fd: OwnedFd) -> Result<Self, RingError> {
        if sys::socket_type(fd.as_fd()).map_err(|_| RingError::DoorbellFailed)? != libc::SOCK_STREAM
        {
            return Err(RingError::DoorbellFailed);
        }
        let local = UnixStream::from(fd);
        local.peer_addr().map_err(|_| RingError::DoorbellFailed)?;
        Ok(Self {
            local,
            remote: Cell::new(None),
            counters: Cell::new(SyscallCounters::default()),
        })
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
        self.local
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
            let error = match sys::send_token(self.local.as_fd(), &token) {
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
            let error = match sys::recv_tokens(self.local.as_fd(), &mut buffer) {
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
        if sys::poll_readable(self.local.as_fd(), 0).unwrap_or(false) {
            return Ok(true);
        }
        self.record(true);
        match sys::poll_readable(self.local.as_fd(), timeout) {
            Ok(ready) => Ok(ready),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                Ok(Instant::now() < deadline)
            }
            Err(_) => Err(RingError::DoorbellFailed),
        }
    }
}

fn set_cloexec(fd: &OwnedFd) -> Result<(), RingError> {
    sys::set_cloexec(fd.as_fd()).map_err(|_| RingError::ObjectValidationFailed)
}

/// Everything a peer needs to attach: layout version, incarnation, lane, and geometry. Sent
/// over the authenticated setup channel alongside the file descriptors; `decode` refuses
/// any grant whose geometry does not map to a valid ring.
///
/// The hardware profile id is excluded from this encoding. The setup layer validates the
/// hardware profile id before decoding any grant.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RingGrant {
    layout_version: u16,
    incarnation: Incarnation,
    lane: u32,
    descriptor_depth: u64,
    arena_bytes: u64,
    max_leases: u64,
    total_bytes: u64,
}

/// Geometry a grant describes, for callers that size buffers or check limits before attaching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingGeometry {
    /// Descriptor slots in one direction.
    pub descriptor_depth: u64,
    /// Payload arena bytes in one direction.
    pub arena_bytes: u64,
    /// Concurrent receive leases in one direction.
    pub max_leases: u64,
    /// Complete mapping length, including control pages and alignment.
    pub mapping_bytes: u64,
}

impl RingGrant {
    /// Serializes to `GRANT_BYTES` little-endian bytes with a zero reserved tail.
    pub fn encode(self) -> [u8; GRANT_BYTES] {
        let mut bytes = [0u8; GRANT_BYTES];
        bytes[0..2].copy_from_slice(&self.layout_version.to_le_bytes());
        bytes[2..18].copy_from_slice(&self.incarnation.into_bytes());
        bytes[18..22].copy_from_slice(&self.lane.to_le_bytes());
        bytes[22..30].copy_from_slice(&self.descriptor_depth.to_le_bytes());
        bytes[30..38].copy_from_slice(&self.arena_bytes.to_le_bytes());
        bytes[38..46].copy_from_slice(&self.max_leases.to_le_bytes());
        bytes[46..54].copy_from_slice(&self.total_bytes.to_le_bytes());
        bytes[54..58].copy_from_slice(&0u32.to_le_bytes());
        bytes
    }

    /// Parses `encode` output. Rejects a nonzero reserved tail and any geometry that cannot
    /// map a valid ring: wrong layout version, zero depth, an arena below one maximum frame,
    /// lease bound outside `1..=depth`, or a total size that disagrees with the computed
    /// layout.
    pub fn decode(bytes: [u8; GRANT_BYTES]) -> Result<Self, RingError> {
        if bytes[54..58] != [0; 4] {
            return Err(RingError::InvalidGrant);
        }
        let array = |range: std::ops::Range<usize>| -> [u8; 8] {
            bytes[range]
                .try_into()
                .expect("grant ranges have fixed eight-byte width")
        };
        let grant = Self {
            layout_version: u16::from_le_bytes([bytes[0], bytes[1]]),
            incarnation: Incarnation::from_bytes(
                bytes[2..18]
                    .try_into()
                    .expect("grant incarnation has fixed width"),
            ),
            lane: u32::from_le_bytes(
                bytes[18..22]
                    .try_into()
                    .expect("grant lane has fixed width"),
            ),
            descriptor_depth: u64::from_le_bytes(array(22..30)),
            arena_bytes: u64::from_le_bytes(array(30..38)),
            max_leases: u64::from_le_bytes(array(38..46)),
            total_bytes: u64::from_le_bytes(array(46..54)),
        };
        grant.checked_layout()?;
        Ok(grant)
    }

    /// `decode` for a slice; any length other than `GRANT_BYTES` is `InvalidGrant`.
    pub fn decode_slice(bytes: &[u8]) -> Result<Self, RingError> {
        let bytes: [u8; GRANT_BYTES] = bytes.try_into().map_err(|_| RingError::InvalidGrant)?;
        Self::decode(bytes)
    }

    fn checked_layout(&self) -> Result<Layout, RingError> {
        if self.layout_version != LAYOUT_VERSION
            || self.descriptor_depth == 0
            || self.arena_bytes < MAX_FRAME_BYTES as u64
            || self.max_leases == 0
            || self.max_leases > self.descriptor_depth
        {
            return Err(RingError::InvalidGrant);
        }
        let depth = usize::try_from(self.descriptor_depth).map_err(|_| RingError::InvalidGrant)?;
        let arena = usize::try_from(self.arena_bytes).map_err(|_| RingError::InvalidGrant)?;
        let total = usize::try_from(self.total_bytes).map_err(|_| RingError::InvalidGrant)?;
        let layout = Layout::new(depth, arena).map_err(|_| RingError::InvalidGrant)?;
        if layout.total != total {
            return Err(RingError::InvalidGrant);
        }
        Ok(layout)
    }

    /// Length `encode` produces and `decode_slice` requires.
    pub const fn encoded_len() -> usize {
        GRANT_BYTES
    }

    /// Geometry fields of this grant.
    pub const fn geometry(self) -> RingGeometry {
        RingGeometry {
            descriptor_depth: self.descriptor_depth,
            arena_bytes: self.arena_bytes,
            max_leases: self.max_leases,
            mapping_bytes: self.total_bytes,
        }
    }
}

impl fmt::Debug for RingGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RingGrant(<redacted>)")
    }
}

/// Duplicated descriptors plus grant, ready to hand to another owner or to `attach`.
pub struct RingAttachment {
    descriptors: [OwnedFd; 3],
    grant: RingGrant,
}

impl RingAttachment {
    /// Maps the ring these descriptors name.
    pub fn attach(self) -> Result<Ring, RingError> {
        Ring::attach(self.descriptors, self.grant)
    }

    /// Grant the descriptors were duplicated for.
    pub const fn grant(&self) -> RingGrant {
        self.grant
    }

    /// Takes the descriptors and grant apart, for callers that send them separately.
    pub fn into_parts(self) -> ([OwnedFd; 3], RingGrant) {
        (self.descriptors, self.grant)
    }
}

impl fmt::Debug for RingAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RingAttachment(<redacted>)")
    }
}

/// One direction of the transport. Not `Send` or `Sync`: the producer and consumer of one
/// ring live in different processes, and within a process one thread owns the handle.
pub struct Ring {
    mapping: Mapping,
    layout: Layout,
    grant: RingGrant,
    data_ready: Doorbell,
    capacity_ready: Doorbell,
    /// The peer can overwrite the shared flag; this latch keeps quarantine terminal for this
    /// handle.
    quarantined: Cell<bool>,
    /// End of the sole outstanding `try_reserve` reservation; `arena_write` advances only when
    /// it commits.
    reserved_end: Cell<Option<u64>>,
    /// Logical arena position below which every reclaimed page has been removed.
    punched: Cell<u64>,
    /// `madvise(MADV_REMOVE)` calls this handle has issued.
    page_removals: Cell<u64>,
    /// Set once this handle has reserved. Only the producer knows its live reservation, so
    /// page removal is refused on any other handle.
    producer: Cell<bool>,
    /// Allocation this handle published for each slot, indexed like the slot ring. Reclaim
    /// checks the peer-writable descriptor against it, so a lengthened descriptor cannot
    /// reclaim into a later live frame.
    published_allocations: Vec<Cell<Option<(u64, u64)>>>,
    producer_cursors: Cell<ProducerCursors>,
    consumer_cursors: Cell<ConsumerCursors>,
    /// Set once this handle has received; `conservation` then checks the consumer cursors
    /// against this handle's record.
    consumer: Cell<bool>,
    /// Greatest `published` this handle has read. The producer's cursor is peer-writable from
    /// the consumer's side, and a rewind to `consumed` would look like an empty ring while a
    /// frame stays queued.
    published_seen: Cell<u64>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl Ring {
    /// Creates a sealed sparse ring. `TargetProfile::new` already checked the profile; only
    /// `max_spans` is re-checked here, because this backend wraps frames into two spans and
    /// cannot honor a one-span profile.
    pub fn create(profile: &TargetProfile, lane: u32) -> Result<Self, RingError> {
        debug_assert_eq!(
            profile.descriptor().schema_version(),
            DESCRIPTOR_SCHEMA_VERSION
        );
        // Reservations crossing the arena end wrap into two spans, so a
        // profile advertising fewer spans per frame cannot be honored.
        if profile.max_spans() < MAX_SPANS {
            return Err(RingError::ProfileMismatch);
        }
        let layout = Layout::new(profile.descriptor_depth(), profile.arena_bytes())?;
        let incarnation = Incarnation::random().map_err(RingError::Descriptor)?;
        let grant = RingGrant {
            layout_version: LAYOUT_VERSION,
            incarnation,
            lane,
            descriptor_depth: profile.descriptor_depth() as u64,
            arena_bytes: profile.arena_bytes() as u64,
            max_leases: profile.max_leases() as u64,
            total_bytes: layout.total as u64,
        };
        let mapping = Mapping::create(layout.total)?;
        initialize_mapping(&mapping, layout, grant)?;
        seal_object(mapping.fd())?;
        validate_object(mapping.fd(), mapping.len)?;
        Ok(Self {
            mapping,
            layout,
            grant,
            data_ready: Doorbell::create()?,
            capacity_ready: Doorbell::create()?,
            quarantined: Cell::new(false),
            reserved_end: Cell::new(None),
            punched: Cell::new(0),
            page_removals: Cell::new(0),
            producer: Cell::new(false),
            published_allocations: allocation_shadow(profile.descriptor_depth()),
            producer_cursors: Cell::new(ProducerCursors {
                published: 0,
                arena_write: 0,
                completed: 0,
                arena_reclaimed: 0,
            }),
            consumer_cursors: Cell::new(ConsumerCursors {
                consumed: 0,
                active_leases: 0,
            }),
            consumer: Cell::new(false),
            published_seen: Cell::new(0),
            _not_send_or_sync: PhantomData,
        })
    }

    /// Maps an existing ring from its three descriptors (mapping, data doorbell, capacity
    /// doorbell). The mapping's magic, layout, and grant fields must match `grant` exactly.
    pub fn attach(descriptors: [OwnedFd; 3], grant: RingGrant) -> Result<Self, RingError> {
        // Descriptors received over `SCM_RIGHTS` without `MSG_CMSG_CLOEXEC` arrive inheritable;
        // a child this process later execs would hold the mapping and the peer's doorbell ends
        // open and hide this side's exit from the peer.
        for descriptor in &descriptors {
            set_cloexec(descriptor)?;
        }
        let [mapping_fd, data_ready, capacity_ready] = descriptors;
        let layout = grant.checked_layout()?;
        let depth = usize::try_from(grant.descriptor_depth).map_err(|_| RingError::InvalidGrant)?;
        let total = usize::try_from(grant.total_bytes).map_err(|_| RingError::InvalidGrant)?;
        let mapping = Mapping::attach(mapping_fd, total)?;
        validate_lifecycle(&mapping, layout, grant)?;
        // Nothing this handle will own has been written yet, so the cursors as attached are
        // the baseline its own writes advance from.
        let producer = mapping.producer(layout)?;
        let consumer = mapping.consumer(layout)?;
        let reclaim = mapping.reclaim(layout)?;
        let producer_cursors = ProducerCursors {
            published: producer.published.load(Ordering::Acquire),
            arena_write: producer.arena_write.load(Ordering::Acquire),
            completed: reclaim.completed.load(Ordering::Acquire),
            arena_reclaimed: reclaim.arena_reclaimed.load(Ordering::Acquire),
        };
        let consumer_cursors = ConsumerCursors {
            consumed: consumer.consumed.load(Ordering::Acquire),
            active_leases: consumer.active_leases.load(Ordering::Acquire),
        };
        let ring = Self {
            mapping,
            layout,
            grant,
            data_ready: Doorbell::from_fd(data_ready)?,
            capacity_ready: Doorbell::from_fd(capacity_ready)?,
            quarantined: Cell::new(false),
            reserved_end: Cell::new(None),
            punched: Cell::new(0),
            page_removals: Cell::new(0),
            producer: Cell::new(false),
            published_allocations: allocation_shadow(depth),
            producer_cursors: Cell::new(producer_cursors),
            consumer_cursors: Cell::new(consumer_cursors),
            consumer: Cell::new(false),
            published_seen: Cell::new(producer_cursors.published),
            _not_send_or_sync: PhantomData,
        };
        if ring.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        // The baseline becomes this handle's record, so a mapping the peer already broke is
        // refused here rather than adopted as truth. Nothing this handle owns is in flight, so
        // the cursors must agree with the slots exactly once the peer's own transition, if
        // any, has settled.
        ring.conservation_inner(true)?;
        Ok(ring)
    }

    /// Grant a peer needs to attach to this ring.
    pub const fn grant(&self) -> RingGrant {
        self.grant
    }

    /// Descriptor of the memfd, for sending over the setup channel.
    pub fn raw_fd(&self) -> RawFd {
        self.mapping.fd.as_raw_fd()
    }

    /// Duplicate of the data doorbell, for registering with an event loop that owns its fds.
    pub fn duplicate_data_ready(&self) -> Result<OwnedFd, RingError> {
        self.data_ready.duplicate()
    }

    /// Both doorbells plus page removals. The counts never reset, so a window is the
    /// difference of two samples.
    pub fn syscall_counters(&self) -> SyscallCounters {
        self.data_ready
            .counters()
            .add(self.capacity_ready.counters())
            .add(SyscallCounters {
                page_removals: self.page_removals.get(),
                ..SyscallCounters::default()
            })
    }

    /// Prepares to block on the data doorbell. Records the wake generation, re-checks for
    /// data, and drains a stale token, so a publish that raced this call is not missed.
    /// Returns `true` only when blocking is correct; `false` means data or a generation change
    /// is already visible and the caller should poll again instead.
    ///
    /// A wait armed while `max_leases` leases are outstanding is woken by the next publish,
    /// not by this handle's own releases: release runs on the thread that would block here.
    /// Release leases, then poll again, before blocking.
    pub fn arm_data_wait(&self) -> Result<bool, RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        if self.data_available()? {
            return Ok(false);
        }
        let wake = self.data_wake()?;
        let (generation, guard) = ParkGuard::arm(wake);
        if !self.armed_wait_holds(wake, generation)? {
            return Ok(false);
        }
        self.data_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))?;
        let holds = self.armed_wait_holds(wake, generation)?;
        if holds {
            // On `Ok(true)`, the caller owns `parked` until `complete_data_wait` clears it.
            std::mem::forget(guard);
        }
        Ok(holds)
    }

    /// Re-checks, after `parked` is set, that blocking is still correct: no quarantine, no
    /// data, and no wake generation change. `enter_quarantine` rings the doorbell only for a
    /// handle it sees parked, so a quarantine that lands between the first check and the
    /// `parked` store sends no token; this re-check covers that window. Clears `parked` on
    /// every path that does not return `Ok(true)`.
    fn armed_wait_holds(&self, wake: &WakeEpoch, generation: u64) -> Result<bool, RingError> {
        let unpark = || wake.parked.store(0, Ordering::Release);
        if self.is_quarantined() {
            unpark();
            return Err(RingError::Quarantined);
        }
        let available = self.data_available().inspect_err(|_| unpark())?;
        if available || wake.generation.load(Ordering::SeqCst) != generation {
            unpark();
            return Ok(false);
        }
        Ok(true)
    }

    /// Clears the parked marker set by `arm_data_wait` and drains the doorbell token. A
    /// doorbell failure means the peer closed its end, which quarantines the ring.
    pub fn complete_data_wait(&self) -> Result<(), RingError> {
        self.data_wake()?.parked.store(0, Ordering::Release);
        self.data_ready
            .drain()
            .map_err(|error| self.quarantine_with(error))
    }

    /// Duplicates the mapping and moves the peer's two doorbell ends out, all with `CLOEXEC`
    /// set, paired with the grant. Callable once per created ring; an attached ring or a
    /// second call fails with `DoorbellFailed`.
    pub fn attachment(&self) -> Result<RingAttachment, RingError> {
        let fd = self
            .mapping
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

    /// Reserves a slot and up to `bound` arena bytes without blocking. `Exhausted` means no
    /// slot or not enough contiguous-or-wrapped arena; `ReservationOutstanding` means this
    /// handle has not committed or aborted its previous reservation. Nothing is charged on any
    /// error.
    pub fn try_reserve(
        &self,
        bound: usize,
        wire_header: [u8; WIRE_V2_HEADER_BYTES],
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        if bound > MAX_FRAME_BYTES {
            return Err(ProducerError::BoundExceedsSpans);
        }
        if self.is_quarantined() {
            return Err(ProducerError::Quarantined);
        }
        if self.reserved_end.get().is_some() {
            return Err(ProducerError::ReservationOutstanding);
        }
        self.reclaim_completed().map_err(ProducerError::Ring)?;
        let ProducerCursors {
            published,
            arena_write: write,
            completed,
            arena_reclaimed: reclaimed,
        } = self
            .verified_producer_cursors()
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        let outstanding = published.checked_sub(completed).ok_or_else(|| {
            ProducerError::Ring(self.quarantine_with(RingError::InvalidSharedState))
        })?;
        if outstanding >= self.grant.descriptor_depth {
            return Err(ProducerError::Exhausted);
        }
        let sequence = published
            .checked_add(1)
            .ok_or(ProducerError::SequenceExhausted)?;
        let slot = self.slot(sequence).map_err(ProducerError::Ring)?;
        // `outstanding < depth` means this slot's previous occupant was reclaimed and stored
        // `SLOT_FREE`, so any other state is corruption rather than backpressure.
        slot.state
            .compare_exchange(
                SLOT_FREE,
                SLOT_PRODUCER_RESERVED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| {
                ProducerError::Ring(self.quarantine_with(RingError::InvalidSharedState))
            })?;
        let plan = match SpanPlan::reserve(self.arena_bytes(), write, reclaimed, bound) {
            Ok(plan) => plan,
            Err(ArenaError::Exhausted) => {
                slot.state.store(SLOT_FREE, Ordering::Release);
                return Err(ProducerError::Exhausted);
            }
            Err(error) => {
                slot.state.store(SLOT_FREE, Ordering::Release);
                // Cursors the protocol cannot produce are a fault, not backpressure.
                self.enter_quarantine();
                return Err(ProducerError::Arena(error));
            }
        };
        slot.reservation_len
            .store(plan.allocation_len(), Ordering::Relaxed);
        // `SpanPlan::reserve` checked this sum.
        self.reserved_end
            .set(Some(plan.allocation_start() + plan.allocation_len()));
        self.producer.set(true);
        Ok(ProducerReservation {
            ring: self,
            plan,
            sequence,
            cursor: 0,
            wire_header,
            finished: false,
            _not_send: PhantomData,
        })
    }

    /// `try_reserve` that parks on the capacity doorbell until a release frees room or
    /// `deadline` passes. Each park is bound to a wake generation so a release between the
    /// check and the park cannot be missed.
    pub fn reserve_until(
        &self,
        bound: usize,
        wire_header: [u8; WIRE_V2_HEADER_BYTES],
        deadline: Instant,
    ) -> Result<ProducerReservation<'_>, ProducerError> {
        loop {
            match self.try_reserve(bound, wire_header) {
                Err(ProducerError::Exhausted) if Instant::now() < deadline => {}
                Err(ProducerError::Exhausted) => return Err(ProducerError::Deadline),
                result => return result,
            }
            let wake = self.capacity_wake().map_err(ProducerError::Ring)?;
            // The guard clears `parked` on every exit from this iteration, including `?`.
            let (generation, _guard) = ParkGuard::arm(wake);
            match self.try_reserve(bound, wire_header) {
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
            match self.try_reserve(bound, wire_header) {
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

    /// Leases the next published frame. `Ok(None)` means nothing is deliverable: the ring is
    /// empty or `max_leases` leases are outstanding. `Err` means the channel is dead; the
    /// descriptor failed validation or shared state is impossible, and the ring is quarantined.
    pub fn try_receive(&self) -> Result<Option<ReceiveLease<'_>>, RingError> {
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

    fn try_receive_inner(&self) -> Result<Option<ReceiveLease<'_>>, RingError> {
        let consumer = self.consumer()?;
        let ConsumerCursors {
            consumed,
            active_leases: active,
        } = self.verified_consumer_cursors()?;
        if active >= self.grant.max_leases {
            // A full lease set is backpressure, not a fault: published
            // frames stay queued until a lease is released and the caller
            // polls again.
            return Ok(None);
        }
        let published = self.verified_published(consumed)?;
        if consumed == published {
            return Ok(None);
        }
        let sequence = consumed
            .checked_add(1)
            .ok_or(RingError::SequenceExhausted)?;
        let slot = self.slot(sequence)?;
        slot.state
            .compare_exchange(
                SLOT_PUBLISHED,
                SLOT_RECEIVER_HELD,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| RingError::InvalidSharedState)?;
        // The acquire exchange above pairs with the producer's release of `published`.
        let shared = slot.read_descriptor();
        let expected = ReleaseIdentity::new(self.grant.incarnation, self.grant.lane, sequence);
        let validated = shared
            .snapshot()
            .validate(expected, self.arena_bytes())
            .map_err(RingError::Descriptor)?;
        let first = self.lease_span(validated.span(0).ok_or(RingError::InvalidSharedState)?)?;
        let second = if validated.span_count() == 2 {
            Some(self.lease_span(validated.span(1).ok_or(RingError::InvalidSharedState)?)?)
        } else {
            None
        };
        slot.state.store(SLOT_RECEIVER_LEASED, Ordering::Release);
        Self::advance_cursor(&consumer.consumed, consumed, sequence)?;
        Self::advance_cursor(&consumer.active_leases, active, active + 1)?;
        self.consumer_cursors.set(ConsumerCursors {
            consumed: sequence,
            active_leases: active + 1,
        });
        self.consumer.set(true);
        let body_len =
            usize::try_from(validated.body_len()).map_err(|_| RingError::InvalidLayout)?;
        let lease = ReceiveLease::new(
            [Some(first), second],
            validated.span_count(),
            body_len,
            validated.wire_header(),
            validated.identity(),
            self,
        )
        .map_err(RingError::Lease)?;
        Ok(Some(lease))
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
            if !self.arm_data_wait()? {
                continue;
            }
            // `arm_data_wait` left `parked` set; the guard clears it on every exit below.
            let _guard = ParkGuard(self.data_wake()?);
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
        let ConsumerCursors {
            consumed,
            active_leases: active,
        } = self
            .verified_consumer_cursors()
            .map_err(|error| self.quarantine_with(error))?;
        let published = self
            .verified_published(consumed)
            .map_err(|error| self.quarantine_with(error))?;
        Ok(published != consumed && active < self.grant.max_leases)
    }

    /// Returns a leased frame to the producer. Checks incarnation, lane, and sequence against
    /// the grant and the slot, then moves the slot to release-pending and rings the capacity
    /// doorbell. Only `ReceiveLease` reaches this: an identity is `Copy`, so a public entry
    /// point would let a caller release a frame while still holding the lease that reads it.
    ///
    /// The data doorbell is left alone. This handle is the consumer, its doorbell end only
    /// reaches the producer, and the thread releasing a lease is the thread that would poll
    /// for data, so it is not blocked; a caller that parked on the lease limit must poll
    /// again after releasing. Touching the data wake epoch here would clear this handle's own
    /// parked marker and silence the next publish.
    ///
    /// The identity was validated when the lease was built, so every mismatch here means the
    /// peer rewrote the slot under a live lease. Each one quarantines; the variant names what
    /// changed.
    pub(crate) fn release(&self, identity: ReleaseIdentity) -> Result<(), LeaseError> {
        if self.is_quarantined() {
            return Err(LeaseError::Quarantined);
        }
        self.release_inner(identity)
            .inspect_err(|_| self.enter_quarantine())
    }

    fn release_inner(&self, identity: ReleaseIdentity) -> Result<(), LeaseError> {
        if identity.incarnation() != self.grant.incarnation {
            return Err(LeaseError::WrongIncarnation);
        }
        if identity.lane() != self.grant.lane {
            return Err(LeaseError::WrongLane);
        }
        let sequence = identity.sequence();
        if sequence == 0 {
            return Err(LeaseError::InvalidSequence);
        }
        let consumer = self.consumer().map_err(|_| LeaseError::InvalidSequence)?;
        // A peer-rewritten count would wrap or undercount on decrement and turn every later
        // receive into permanent backpressure.
        let ConsumerCursors {
            consumed,
            active_leases: active,
        } = self
            .verified_consumer_cursors()
            .map_err(|_| LeaseError::Quarantined)?;
        if sequence > consumed {
            return Err(LeaseError::InvalidSequence);
        }
        if active == 0 {
            return Err(LeaseError::Quarantined);
        }
        let slot = self
            .slot(sequence)
            .map_err(|_| LeaseError::InvalidSequence)?;
        let descriptor = slot.read_descriptor();
        if descriptor.incarnation != identity.incarnation().into_bytes() {
            return Err(LeaseError::WrongIncarnation);
        }
        if descriptor.lane != identity.lane() {
            return Err(LeaseError::WrongLane);
        }
        if descriptor.sequence != sequence {
            return Err(LeaseError::InvalidSequence);
        }
        let changed = slot.state.compare_exchange(
            SLOT_RECEIVER_LEASED,
            SLOT_RELEASE_PENDING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if let Err(observed) = changed {
            return Err(
                if observed == SLOT_RELEASE_PENDING || observed == SLOT_FREE {
                    LeaseError::DuplicateRelease
                } else {
                    LeaseError::InvalidSequence
                },
            );
        }
        // The release store publishes completion after every receiver read.
        slot.completion_sequence.store(sequence, Ordering::Release);
        Self::advance_cursor(&consumer.active_leases, active, active - 1)
            .map_err(|_| LeaseError::Quarantined)?;
        self.consumer_cursors.set(ConsumerCursors {
            consumed,
            active_leases: active - 1,
        });
        self.signal_wake(self.capacity_wake(), &self.capacity_ready)
            .map_err(|_| LeaseError::Quarantined)
    }

    /// Counts descriptors and arena bytes by ownership state. A quarantined ring reports
    /// everything as quarantined; a live one must partition depth and capacity exactly and
    /// its cursors must satisfy the protocol's ordering bounds, or `InvalidSharedState` is
    /// returned and the ring is quarantined. `Busy` means the peer kept the cursors moving for
    /// the whole check; nothing was judged and the ring stays live.
    pub fn conservation(&self) -> Result<(DescriptorCounts, ArenaCounts), RingError> {
        if self.is_quarantined() {
            return Ok((
                DescriptorCounts {
                    quarantined: self.grant.descriptor_depth,
                    ..DescriptorCounts::default()
                },
                ArenaCounts {
                    quarantined: self.grant.arena_bytes,
                    ..ArenaCounts::default()
                },
            ));
        }
        self.conservation_inner(false).map_err(|error| match error {
            RingError::Busy => error,
            error => self.quarantine_with(error),
        })
    }

    /// Walks the slots until one walk completes with every cursor unchanged around it. The
    /// cursors only advance, so an unchanged set means no transition finished during the walk,
    /// and the counts then differ from the cursors by at most the one transition whose slot
    /// store landed before its cursor store. With `exact`, that one transition is also waited
    /// out, since it completes within a few instructions on an honest peer while a forged
    /// cursor never converges. Sustained traffic can keep a cursor moving through every walk;
    /// that is `Busy`, not a fault, and the caller retries later.
    fn conservation_inner(
        &self,
        exact: bool,
    ) -> Result<(DescriptorCounts, ArenaCounts), RingError> {
        const STABLE_SNAPSHOT_ATTEMPTS: usize = 64;
        for attempt in 1..=STABLE_SNAPSHOT_ATTEMPTS {
            let before = self.cursor_snapshot()?;
            let (descriptors, bytes) = self.walk_slots()?;
            if self.cursor_snapshot()? != before {
                continue;
            }
            match self.check_cursor_invariants(&descriptors, before, exact) {
                Ok(()) => return Ok((descriptors, bytes)),
                Err(RingError::InvalidSharedState)
                    if exact
                        && attempt < STABLE_SNAPSHOT_ATTEMPTS
                        && self
                            .check_cursor_invariants(&descriptors, before, false)
                            .is_ok() =>
                {
                    std::thread::yield_now();
                }
                Err(error) => return Err(error),
            }
        }
        Err(RingError::Busy)
    }

    fn cursor_snapshot(&self) -> Result<CursorSnapshot, RingError> {
        let producer = self.producer()?;
        let consumer = self.consumer()?;
        let reclaim = self.reclaim()?;
        Ok(CursorSnapshot {
            published: producer.published.load(Ordering::Acquire),
            arena_write: producer.arena_write.load(Ordering::Acquire),
            consumed: consumer.consumed.load(Ordering::Acquire),
            active_leases: consumer.active_leases.load(Ordering::Acquire),
            completed: reclaim.completed.load(Ordering::Acquire),
            arena_reclaimed: reclaim.arena_reclaimed.load(Ordering::Acquire),
        })
    }

    fn walk_slots(&self) -> Result<(DescriptorCounts, ArenaCounts), RingError> {
        let mut descriptors = DescriptorCounts::default();
        let mut bytes = ArenaCounts::default();
        let mut charged = 0u64;
        for index in 0..self.grant.descriptor_depth {
            let slot = self.slot(index + 1)?;
            let state = slot.state.load(Ordering::Acquire);
            // The reservation length is assigned before any non-free state becomes visible.
            let len = slot.reservation_len.load(Ordering::Relaxed);
            match state {
                SLOT_FREE => descriptors.free += 1,
                SLOT_PRODUCER_RESERVED => {
                    descriptors.producer_reserved += 1;
                    bytes.producer_reserved = bytes
                        .producer_reserved
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                    charged = charged
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                }
                SLOT_PUBLISHED => {
                    descriptors.published += 1;
                    bytes.published = bytes
                        .published
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                    charged = charged
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                }
                SLOT_RECEIVER_HELD => {
                    descriptors.receiver_held += 1;
                    bytes.receiver_held = bytes
                        .receiver_held
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                    charged = charged
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                }
                SLOT_RECEIVER_LEASED => {
                    descriptors.receiver_leased += 1;
                    bytes.receiver_leased = bytes
                        .receiver_leased
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                    charged = charged
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                }
                SLOT_RELEASE_PENDING => {
                    descriptors.release_pending += 1;
                    bytes.release_pending = bytes
                        .release_pending
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                    charged = charged
                        .checked_add(len)
                        .ok_or(RingError::ArithmeticOverflow)?;
                }
                _ => return Err(RingError::InvalidSharedState),
            }
        }
        bytes.free = self
            .grant
            .arena_bytes
            .checked_sub(charged)
            .ok_or(RingError::InvalidSharedState)?;
        Ok((descriptors, bytes))
    }

    /// Checks a cursor snapshot that was stable across a slot walk. Cursor-versus-slot
    /// comparisons allow one transition in flight: a receive stores the slot before `consumed`
    /// and `active_leases`, a release stores the slot before `active_leases`, and a commit
    /// stores the slot before `published`. One endpoint performs one transition at a time, so
    /// each compared count differs from its cursor by at most one. `exact` requires zero
    /// difference. Receiver-owned slots (held, leased, release-pending) may exceed
    /// `consumed - completed` by one for the receive in flight but never by more; they may fall
    /// short by a whole run, because reclaim frees the run before it stores `completed`. A
    /// handle that has acted in a role also requires that role's cursors to match its own
    /// record, which only that handle writes.
    fn check_cursor_invariants(
        &self,
        descriptors: &DescriptorCounts,
        cursors: CursorSnapshot,
        exact: bool,
    ) -> Result<(), RingError> {
        if exact {
            self.validate_idle_window(cursors)?;
        }
        let CursorSnapshot {
            published,
            arena_write,
            consumed,
            active_leases,
            completed,
            arena_reclaimed,
        } = cursors;
        let tolerance = if exact { 0 } else { 1 };
        let matches = |cursor: u64, count: u64| cursor.abs_diff(count) <= tolerance;
        let in_flight = published
            .checked_sub(consumed)
            .ok_or(RingError::InvalidSharedState)?;
        let held = consumed
            .checked_sub(completed)
            .ok_or(RingError::InvalidSharedState)?;
        let live_bytes = arena_write
            .checked_sub(arena_reclaimed)
            .ok_or(RingError::InvalidSharedState)?;
        let receiver_owned =
            descriptors.receiver_held + descriptors.receiver_leased + descriptors.release_pending;
        let receiver_owned_ok = if exact {
            receiver_owned == held
        } else {
            receiver_owned.saturating_sub(1) <= held
        };
        let outstanding = in_flight
            .checked_add(held)
            .ok_or(RingError::InvalidSharedState)?;
        // One producer holds at most one reservation.
        if descriptors.producer_reserved > 1
            || active_leases > self.grant.max_leases
            || !matches(active_leases, descriptors.receiver_leased)
            || !matches(in_flight, descriptors.published)
            || !receiver_owned_ok
            || outstanding > self.grant.descriptor_depth
            || live_bytes > self.grant.arena_bytes
        {
            return Err(RingError::InvalidSharedState);
        }
        if self.producer.get() {
            self.verified_producer_cursors()?;
        }
        if self.consumer.get() {
            self.verified_consumer_cursors()?;
        }
        Ok(())
    }

    /// Structural check of an idle mapping: every sequence in `(completed, published]` holds a
    /// live descriptor that validates against its own identity, the allocations chain from
    /// `arena_reclaimed` to `arena_write` without gaps, each allocation matches the slot's
    /// `reservation_len`, and every other slot is free except for one open reservation at
    /// `published + 1`. A running probe cannot require this, since the peer's transitions
    /// change several of these fields one store at a time; an attaching handle can, because
    /// the stable-snapshot loop waits those transitions out.
    fn validate_idle_window(&self, cursors: CursorSnapshot) -> Result<(), RingError> {
        let CursorSnapshot {
            published,
            arena_write,
            completed,
            arena_reclaimed,
            ..
        } = cursors;
        let depth = self.grant.descriptor_depth;
        let mut expected_start = arena_reclaimed;
        let mut sequence = completed;
        while sequence < published {
            sequence += 1;
            let slot = self.slot(sequence)?;
            let state = slot.state.load(Ordering::Acquire);
            let reservation_len = slot.reservation_len.load(Ordering::Acquire);
            let descriptor = slot.read_descriptor();
            if !matches!(
                state,
                SLOT_PUBLISHED | SLOT_RECEIVER_LEASED | SLOT_RELEASE_PENDING
            ) {
                return Err(RingError::InvalidSharedState);
            }
            let expected = ReleaseIdentity::new(self.grant.incarnation, self.grant.lane, sequence);
            let validated = descriptor
                .snapshot()
                .validate(expected, self.arena_bytes())
                .map_err(RingError::Descriptor)?;
            if validated.allocation_start() != expected_start
                || validated.allocation_len() != reservation_len
            {
                return Err(RingError::InvalidSharedState);
            }
            expected_start = expected_start
                .checked_add(reservation_len)
                .ok_or(RingError::ArithmeticOverflow)?;
        }
        if expected_start != arena_write {
            return Err(RingError::InvalidSharedState);
        }
        // Slots outside the live window: free, or the one open reservation right after it.
        let outside = depth.saturating_sub(published.saturating_sub(completed));
        for offset in 1..=outside {
            let sequence = published
                .checked_add(offset)
                .ok_or(RingError::SequenceExhausted)?;
            let slot = self.slot(sequence)?;
            let state = slot.state.load(Ordering::Acquire);
            let allowed = state == SLOT_FREE || (offset == 1 && state == SLOT_PRODUCER_RESERVED);
            if !allowed {
                return Err(RingError::InvalidSharedState);
            }
        }
        Ok(())
    }

    /// `conservation` without the counts: `Ok` if shared state is consistent and not
    /// quarantined. An inconsistency quarantines the ring, as any other operation would;
    /// `Busy` does not.
    ///
    /// The peer's cursors are checked against the bounds that hold while one of its
    /// transitions is in flight, not for exact agreement with the slot states; the peer's own
    /// handle enforces exact agreement against its private record on every operation. A probe
    /// therefore never quarantines a healthy ring under traffic, and a forged peer cursor is
    /// caught by the peer before it can act on it.
    pub fn probe(&self) -> Result<(), RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        self.conservation().map(|_| ())
    }

    /// Arena pages the kernel reports resident via `mincore`. Tests use it to check that a
    /// sparse ring stays sparse.
    pub fn resident_arena_pages(&self) -> Result<usize, RingError> {
        self.mapping
            .resident_pages(self.layout.arena, self.arena_bytes())
    }

    /// Mappings this ring holds; always one. Exists so callers charge admission uniformly.
    pub const fn mapping_count(&self) -> usize {
        1
    }

    /// Byte length of the memfd, equal to the grant's total.
    pub const fn object_size(&self) -> usize {
        self.mapping.len
    }

    /// The private latch never clears, so rewriting the shared flag cannot revive the ring.
    /// Both wake channels are rung so a peer parked in `wait_for_data` or `reserve_until`
    /// re-checks and sees the flag instead of sleeping to its deadline. Wake failures are
    /// ignored here: the ring is already terminal and the wake is best effort.
    pub fn enter_quarantine(&self) {
        self.quarantined.set(true);
        if let Ok(flag) = self.mapping.lifecycle_quarantined(self.layout) {
            flag.store(1, Ordering::Release);
        }
        let _ = self.signal_wake(self.data_wake(), &self.data_ready);
        let _ = self.signal_wake(self.capacity_wake(), &self.capacity_ready);
    }

    /// True if this handle latched quarantine or the shared flag is set. Observing the shared
    /// flag latches too, so a peer that sets and then clears it cannot revive this handle.
    /// An unreadable lifecycle page counts as quarantined.
    pub fn is_quarantined(&self) -> bool {
        if self.quarantined.get() {
            return true;
        }
        let observed = self
            .mapping
            .lifecycle_quarantined(self.layout)
            .map(|flag| flag.load(Ordering::Acquire) != 0)
            .unwrap_or(true);
        if observed {
            self.quarantined.set(true);
        }
        observed
    }

    /// Quarantines and returns `error`, for impossible shared state observed mid-operation.
    fn quarantine_with(&self, error: RingError) -> RingError {
        self.enter_quarantine();
        error
    }

    /// Moves an owned cursor from the value this handle's record holds to `to`. The compare
    /// makes the verify-then-store pair atomic: a peer rewrite between them fails the exchange
    /// instead of being overwritten or, for a counter, wrapped.
    fn advance_cursor(cursor: &AtomicU64, from: u64, to: u64) -> Result<(), RingError> {
        cursor
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| RingError::InvalidSharedState)
    }

    /// Loads the producer-owned cursors and checks them against this handle's own record.
    fn verified_producer_cursors(&self) -> Result<ProducerCursors, RingError> {
        let producer = self.producer()?;
        let reclaim = self.reclaim()?;
        let shared = ProducerCursors {
            published: producer.published.load(Ordering::Acquire),
            arena_write: producer.arena_write.load(Ordering::Acquire),
            completed: reclaim.completed.load(Ordering::Acquire),
            arena_reclaimed: reclaim.arena_reclaimed.load(Ordering::Acquire),
        };
        if shared != self.producer_cursors.get() {
            return Err(RingError::InvalidSharedState);
        }
        Ok(shared)
    }

    /// Loads `published` and rejects a value below the greatest one this handle has seen or
    /// more than `descriptor_depth` ahead of `consumed`, which no producer can reach.
    fn verified_published(&self, consumed: u64) -> Result<u64, RingError> {
        // Acquire pairs with the producer's release store in `publish_commit`.
        let published = self.producer()?.published.load(Ordering::Acquire);
        let queued = published
            .checked_sub(consumed)
            .ok_or(RingError::InvalidSharedState)?;
        if published < self.published_seen.get() || queued > self.grant.descriptor_depth {
            return Err(RingError::InvalidSharedState);
        }
        self.published_seen.set(published);
        Ok(published)
    }

    /// Loads the consumer-owned cursors and checks them against this handle's own record.
    fn verified_consumer_cursors(&self) -> Result<ConsumerCursors, RingError> {
        let consumer = self.consumer()?;
        let shared = ConsumerCursors {
            consumed: consumer.consumed.load(Ordering::Acquire),
            active_leases: consumer.active_leases.load(Ordering::Acquire),
        };
        if shared != self.consumer_cursors.get() {
            return Err(RingError::InvalidSharedState);
        }
        Ok(shared)
    }

    fn arena_bytes(&self) -> usize {
        self.grant.arena_bytes as usize
    }

    fn producer(&self) -> Result<&ProducerPage, RingError> {
        self.mapping.producer(self.layout)
    }

    fn consumer(&self) -> Result<&ConsumerPage, RingError> {
        self.mapping.consumer(self.layout)
    }

    fn reclaim(&self) -> Result<&ReclaimPage, RingError> {
        self.mapping.reclaim(self.layout)
    }

    fn data_wake(&self) -> Result<&WakeEpoch, RingError> {
        self.mapping.data_wake(self.layout)
    }

    fn capacity_wake(&self) -> Result<&WakeEpoch, RingError> {
        self.mapping.capacity_wake(self.layout)
    }

    fn signal_wake(
        &self,
        wake: Result<&WakeEpoch, RingError>,
        doorbell: &Doorbell,
    ) -> Result<(), RingError> {
        let wake = wake?;
        wake.generation.fetch_add(1, Ordering::SeqCst);
        if wake.parked.swap(0, Ordering::SeqCst) != 0 {
            doorbell.signal()?;
        }
        Ok(())
    }

    fn slot(&self, sequence: u64) -> Result<&DescriptorSlot, RingError> {
        if sequence == 0 || self.grant.descriptor_depth == 0 {
            return Err(RingError::InvalidSharedState);
        }
        let index = (sequence - 1) % self.grant.descriptor_depth;
        let index = usize::try_from(index).map_err(|_| RingError::ArithmeticOverflow)?;
        self.mapping.slot(self.layout, index)
    }

    /// Shadow entry for `sequence`; `sequence` is nonzero because callers derive it from a
    /// published or pending slot, and `descriptor_depth` is nonzero by grant validation.
    fn allocation_shadow(&self, sequence: u64) -> &Cell<Option<(u64, u64)>> {
        let index = ((sequence - 1) % self.grant.descriptor_depth) as usize;
        &self.published_allocations[index]
    }

    /// Wraps a validated span of the arena. `span` came from `FrameDescriptor::validate` or
    /// `SpanPlan`, both of which bound it to the arena; `arena_ptr` re-checks anyway.
    fn lease_span<'lease>(&'lease self, span: ArenaSpan) -> Result<LeaseSpan<'lease>, RingError> {
        let offset = usize::try_from(span.offset()).map_err(|_| RingError::InvalidLayout)?;
        let len = usize::try_from(span.len()).map_err(|_| RingError::InvalidLayout)?;
        let ptr = self
            .mapping
            .arena_ptr(self.layout, self.arena_bytes(), offset, len)?;
        // SAFETY: `arena_ptr` checked `[offset, offset + len)` against both the arena and the
        // mapping length, and the mapping (owned by `self`) stays mapped for `'lease`. No
        // `&[u8]` over the arena is ever formed in this crate; access goes through volatile
        // or atomic reads.
        unsafe { LeaseSpan::new(ptr, len) }.map_err(RingError::Lease)
    }

    fn reclaim_completed(&self) -> Result<(), RingError> {
        self.reclaim_completed_inner()
            .map_err(|error| self.quarantine_with(error))
    }

    fn reclaim_completed_inner(&self) -> Result<(), RingError> {
        let reclaim = self.reclaim()?;
        let cursors = self.verified_producer_cursors()?;
        let ProducerCursors {
            completed,
            arena_reclaimed: reclaimed,
            arena_write,
            ..
        } = cursors;
        let mut last = completed;
        let mut run_len = 0u64;
        loop {
            let next = last.checked_add(1).ok_or(RingError::SequenceExhausted)?;
            let slot = self.slot(next)?;
            // Acquire pairs with the receiver's release store of `completion_sequence`.
            if slot.completion_sequence.load(Ordering::Acquire) != next {
                break;
            }
            if slot.state.load(Ordering::Acquire) != SLOT_RELEASE_PENDING {
                return Err(RingError::InvalidSharedState);
            }
            let descriptor = slot.read_descriptor();
            let expected = ReleaseIdentity::new(self.grant.incarnation, self.grant.lane, next);
            let validated = descriptor
                .snapshot()
                .validate(expected, self.arena_bytes())
                .map_err(RingError::Descriptor)?;
            // The descriptor is peer-writable, so its allocation is checked against what this
            // handle published; a lengthened descriptor would otherwise reclaim into the next
            // live frame.
            let published = (validated.allocation_start(), validated.allocation_len());
            if self.allocation_shadow(next).get() != Some(published) {
                return Err(RingError::InvalidSharedState);
            }
            let expected_start = reclaimed
                .checked_add(run_len)
                .ok_or(RingError::ArithmeticOverflow)?;
            if validated.allocation_start() != expected_start {
                return Err(RingError::InvalidSharedState);
            }
            run_len = run_len
                .checked_add(validated.allocation_len())
                .ok_or(RingError::ArithmeticOverflow)?;
            last = next;
        }
        if last == completed {
            return Ok(());
        }
        let new_reclaimed = reclaimed
            .checked_add(run_len)
            .ok_or(RingError::ArithmeticOverflow)?;
        if new_reclaimed > self.live_end(arena_write) {
            return Err(RingError::InvalidSharedState);
        }
        let unpunched = new_reclaimed
            .checked_sub(self.punched.get())
            .ok_or(RingError::InvalidSharedState)?;
        if unpunched >= self.punch_batch_bytes() {
            self.punch_dead_pages(new_reclaimed, arena_write, false)?;
        }
        for sequence in completed + 1..=last {
            let slot = self.slot(sequence)?;
            self.allocation_shadow(sequence).set(None);
            slot.reservation_len.store(0, Ordering::Relaxed);
            slot.completion_sequence.store(0, Ordering::Relaxed);
            slot.state.store(SLOT_FREE, Ordering::Release);
        }
        // Capacity becomes visible only after every removal succeeded.
        Self::advance_cursor(&reclaim.arena_reclaimed, reclaimed, new_reclaimed)?;
        Self::advance_cursor(&reclaim.completed, completed, last)?;
        self.producer_cursors.set(ProducerCursors {
            completed: last,
            arena_reclaimed: new_reclaimed,
            ..cursors
        });
        Ok(())
    }

    /// The arena's logical end bounds bytes that readers or writers may access.
    fn live_end(&self, arena_write: u64) -> u64 {
        self.reserved_end.get().unwrap_or(arena_write)
    }

    /// `PUNCH_BATCH_DIVISOR` amortizes page-punch overhead.
    fn punch_batch_bytes(&self) -> u64 {
        (self.grant.arena_bytes / PUNCH_BATCH_DIVISOR).max(system_page_size() as u64)
    }

    /// Dead pages in `[punched, reclaimed)` do not overlap `[reclaimed, live_end)`.
    /// `everything` also counts partially covered pages as dead once no live bytes remain.
    ///
    /// `punched` is left page-aligned. Advancing it to an unaligned `reclaimed` would drop the
    /// dead prefix of the boundary page: the next batch would start past that prefix and
    /// round the page up, leaving it resident until the ring drained completely.
    fn punch_dead_pages(
        &self,
        reclaimed: u64,
        arena_write: u64,
        everything: bool,
    ) -> Result<(), RingError> {
        let punched = self.punched.get();
        if punched == reclaimed {
            return Ok(());
        }
        // Both cursors live in peer-writable memory; a backwards cursor is a fault.
        if punched > reclaimed {
            return Err(RingError::InvalidSharedState);
        }
        let arena_bytes = self.arena_bytes();
        let arena_bytes_u64 = arena_bytes as u64;
        let page_size = system_page_size();
        let page_size_u64 = page_size as u64;
        let live_end = self.live_end(arena_write);
        let live_len = live_end
            .checked_sub(reclaimed)
            .ok_or(RingError::InvalidSharedState)?;
        if live_len > arena_bytes_u64 {
            return Err(RingError::InvalidSharedState);
        }
        let remove = |offset, len| {
            self.page_removals
                .set(self.page_removals.get().wrapping_add(1));
            remove_pages(self.mapping.base, offset, len)
        };
        if everything && live_end == reclaimed {
            let start = punched & !(page_size_u64 - 1);
            let end = reclaimed
                .checked_add(page_size_u64 - 1)
                .ok_or(RingError::ArithmeticOverflow)?
                & !(page_size_u64 - 1);
            if end - start >= arena_bytes_u64 {
                remove(self.layout.arena, arena_bytes)?;
            } else {
                for (offset, len) in removal_ranges(
                    self.layout.arena,
                    arena_bytes,
                    start,
                    end - start,
                    page_size,
                )?
                .into_iter()
                .filter(|(_, len)| *len != 0)
                {
                    remove(offset, len)?;
                }
            }
        } else {
            // `live_len <= arena_bytes` puts `live_end - arena_bytes` at or below `reclaimed`.
            let dead_start = punched.max(live_end.saturating_sub(arena_bytes_u64));
            let dead_len = reclaimed
                .checked_sub(dead_start)
                .ok_or(RingError::InvalidSharedState)?;
            for (offset, len) in removal_ranges(
                self.layout.arena,
                arena_bytes,
                dead_start,
                dead_len,
                page_size,
            )?
            .into_iter()
            .filter(|(_, len)| *len != 0)
            {
                remove(offset, len)?;
            }
        }
        self.punched.set(reclaimed & !(page_size_u64 - 1));
        Ok(())
    }

    /// Punches every dead arena page, including partial ones. Producer handles only: the
    /// shared `arena_write` cursor excludes an uncommitted reservation, so any other handle
    /// could remove the page that reservation is being written into.
    pub fn trim(&self) -> Result<(), RingError> {
        if self.is_quarantined() {
            return Err(RingError::Quarantined);
        }
        if !self.producer.get() {
            return Err(RingError::RoleMismatch);
        }
        // Releases only become reclaimed capacity through this pass, which otherwise runs
        // inside `try_reserve`; an idle ring would keep newly dead pages resident without it.
        self.reclaim_completed()?;
        let ProducerCursors {
            arena_write,
            arena_reclaimed,
            ..
        } = self
            .verified_producer_cursors()
            .map_err(|error| self.quarantine_with(error))?;
        self.punch_dead_pages(arena_reclaimed, arena_write, true)
            .map_err(|error| self.quarantine_with(error))
    }

    /// Returns the slot and arena range without publishing. Pages the reservation dirtied lie
    /// above `arena_write`, where no reclaim pass will ever reach them, so they are removed
    /// here; a removal failure quarantines because `Drop` cannot report it.
    fn abort_reservation(&self, sequence: u64) {
        if let Some(reserved_end) = self.reserved_end.take() {
            let arena_write = self.producer_cursors.get().arena_write;
            if let Err(error) = self.punch_range(arena_write, reserved_end) {
                self.quarantine_with(error);
            }
        }
        if let Ok(slot) = self.slot(sequence) {
            slot.reservation_len.store(0, Ordering::Relaxed);
            slot.state.store(SLOT_FREE, Ordering::Release);
        }
    }

    /// Removes every page that lies wholly inside the logical range `[start, end)`.
    fn punch_range(&self, start: u64, end: u64) -> Result<(), RingError> {
        let len = end
            .checked_sub(start)
            .ok_or(RingError::InvalidSharedState)?;
        for (offset, len) in removal_ranges(
            self.layout.arena,
            self.arena_bytes(),
            start,
            len,
            system_page_size(),
        )?
        .into_iter()
        .filter(|(_, len)| *len != 0)
        {
            self.page_removals
                .set(self.page_removals.get().wrapping_add(1));
            remove_pages(self.mapping.base, offset, len)?;
        }
        Ok(())
    }

    /// Everything `commit` checks before it writes shared state. Any error here leaves the
    /// reservation abortable.
    fn prepare_commit(
        &self,
        sequence: u64,
        plan: SpanPlan,
        exact_len: usize,
        wire_header: [u8; WIRE_V2_HEADER_BYTES],
    ) -> Result<PreparedCommit, ProducerError> {
        let exact = plan.prefix(exact_len).map_err(ProducerError::Arena)?;
        check_wire_header(&wire_header, exact_len as u64)
            .map_err(|_| ProducerError::WireHeaderMismatch)?;
        let identity = ReleaseIdentity::new(self.grant.incarnation, self.grant.lane, sequence);
        let spans = exact.spans();
        let descriptor = SharedDescriptor {
            schema_version: DESCRIPTOR_SCHEMA_VERSION,
            wire_header,
            incarnation: identity.incarnation().into_bytes(),
            lane: identity.lane(),
            sequence: identity.sequence(),
            body_len: exact_len as u64,
            allocation_start: plan.allocation_start(),
            allocation_len: plan.allocation_len(),
            span_count: exact.span_count(),
            span_offsets: [spans[0].offset(), spans[1].offset()],
            span_lengths: [spans[0].len(), spans[1].len()],
        };
        // Both pages are re-fetched in `publish_commit`; checking them here keeps every
        // failure before the first shared-state write.
        self.slot(sequence).map_err(ProducerError::Ring)?;
        self.producer().map_err(ProducerError::Ring)?;
        Ok(PreparedCommit {
            identity,
            descriptor,
            // `SpanPlan::reserve` checked this sum.
            next_write: plan.allocation_start() + plan.allocation_len(),
        })
    }

    /// Publishes a prepared commit. Once the slot and cursors are written the peer may hold
    /// the frame, so a failed wake quarantines the ring but never rolls the slot back.
    fn publish_commit(&self, prepared: PreparedCommit) -> Result<ReleaseIdentity, ProducerError> {
        let PreparedCommit {
            identity,
            descriptor,
            next_write,
        } = prepared;
        self.allocation_shadow(identity.sequence()).set(Some((
            descriptor.allocation_start,
            descriptor.allocation_len,
        )));
        let cursors = self.producer_cursors.get();
        let slot = self
            .slot(identity.sequence())
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        let producer = self
            .producer()
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        slot.write_descriptor(descriptor);
        slot.state.store(SLOT_PUBLISHED, Ordering::Relaxed);
        Self::advance_cursor(&producer.arena_write, cursors.arena_write, next_write)
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        Self::advance_cursor(&producer.published, cursors.published, identity.sequence())
            .map_err(|error| ProducerError::Ring(self.quarantine_with(error)))?;
        self.producer_cursors.set(ProducerCursors {
            published: identity.sequence(),
            arena_write: next_write,
            ..cursors
        });
        self.reserved_end.set(None);
        if let Err(error) = self.signal_wake(self.data_wake(), &self.data_ready) {
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

    fn write_reservation(
        &self,
        plan: SpanPlan,
        cursor: usize,
        bytes: &[u8],
    ) -> Result<(), ProducerError> {
        let end = cursor
            .checked_add(bytes.len())
            .ok_or(ProducerError::Overflow)?;
        if end > plan.allocation_len() as usize {
            return Err(ProducerError::Overflow);
        }
        let mut copied = 0usize;
        while copied < bytes.len() {
            let absolute = plan
                .allocation_start()
                .checked_add((cursor + copied) as u64)
                .ok_or(ProducerError::Overflow)?;
            let offset = (absolute % self.grant.arena_bytes) as usize;
            let available = self.arena_bytes() - offset;
            let take = available.min(bytes.len() - copied);
            let destination = self
                .mapping
                .arena_ptr(self.layout, self.arena_bytes(), offset, take)
                .map_err(ProducerError::Ring)?;
            // SAFETY: `arena_ptr` checked `[offset, offset + take)` against the arena and the
            // mapping; `copied + take <= bytes.len()` keeps the source in the slice; the
            // reservation owns this arena range until commit or abort, so no reader has a
            // lease over it.
            unsafe { volatile_copy(bytes.as_ptr().add(copied), destination, take) };
            copied += take;
        }
        Ok(())
    }
}

impl fmt::Debug for Ring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ring(<redacted>)")
    }
}

/// Output of `prepare_commit`, consumed by `publish_commit`.
struct PreparedCommit {
    identity: ReleaseIdentity,
    descriptor: SharedDescriptor,
    next_write: u64,
}

impl ReleaseSink for Ring {
    fn release(&self, identity: ReleaseIdentity) -> Result<(), LeaseError> {
        Ring::release(self, identity)
    }
}

/// A reserved slot and arena range the producer fills then commits. Dropping without
/// `commit` aborts, returning the slot and bytes to the ring.
#[must_use = "producer reservation must be committed or aborted"]
pub struct ProducerReservation<'ring> {
    ring: &'ring Ring,
    plan: SpanPlan,
    sequence: u64,
    cursor: usize,
    wire_header: [u8; WIRE_V2_HEADER_BYTES],
    finished: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl ProducerReservation<'_> {
    /// Bytes reserved; `commit` may publish fewer.
    pub const fn capacity(&self) -> usize {
        self.plan.allocation_len() as usize
    }

    /// Bytes written through `write` or `advance`.
    pub const fn written(&self) -> usize {
        self.cursor
    }

    /// `capacity() - written()`.
    pub const fn remaining(&self) -> usize {
        self.capacity() - self.cursor
    }

    /// One, or two when the reservation wraps the arena end.
    pub const fn segment_count(&self) -> usize {
        self.plan.span_count() as usize
    }

    /// Raw view of reserved span `index`, for callers that write in place instead of via
    /// `write`. Follow with `advance`.
    pub fn segment(&self, index: usize) -> Result<Option<LeaseSpan<'_>>, ProducerError> {
        let Some(span) = self.plan.span(index) else {
            return Ok(None);
        };
        self.ring
            .lease_span(span)
            .map(Some)
            .map_err(ProducerError::Ring)
    }

    /// Records `bytes` written in place through `segment`. Aborts the reservation on overflow.
    pub fn advance(&mut self, bytes: usize) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        let Some(cursor) = self.cursor.checked_add(bytes) else {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(ProducerError::Overflow);
        };
        if cursor > self.capacity() {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(ProducerError::Overflow);
        }
        self.cursor = cursor;
        Ok(())
    }

    /// Replaces the header given to `try_reserve`. `commit` checks its declared length.
    pub fn set_wire_header(
        &mut self,
        wire_header: [u8; WIRE_V2_HEADER_BYTES],
    ) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        self.wire_header = wire_header;
        Ok(())
    }

    /// Copies `bytes` at the cursor, spanning the wrap if needed. Aborts on overflow.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        if let Err(error) = self.ring.write_reservation(self.plan, self.cursor, bytes) {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(error);
        }
        self.cursor += bytes.len();
        Ok(())
    }

    /// Publishes `body_len` bytes. `body_len` must equal `written()`; the header's declared
    /// length must equal `body_len`. A failure before publication aborts the reservation; a
    /// failed wake after publication quarantines the ring and leaves the frame published.
    pub fn commit(mut self, body_len: usize) -> Result<ReleaseIdentity, ProducerError> {
        if self.finished {
            return Err(ProducerError::Aborted);
        }
        // Quarantine may have been entered, locally or by the peer, since `try_reserve`.
        if self.ring.is_quarantined() {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(ProducerError::Quarantined);
        }
        if body_len > self.capacity() {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(ProducerError::CommitOutsideReservation);
        }
        if self.cursor != body_len {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
            return Err(ProducerError::Underfill);
        }
        let prepared =
            match self
                .ring
                .prepare_commit(self.sequence, self.plan, body_len, self.wire_header)
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.ring.abort_reservation(self.sequence);
                    self.finished = true;
                    return Err(error);
                }
            };
        self.finished = true;
        self.ring.publish_commit(prepared)
    }

    /// Gives the slot and arena bytes back without publishing. Same as drop, but explicit.
    pub fn abort(mut self) {
        if !self.finished {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
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
        if !self.finished {
            self.ring.abort_reservation(self.sequence);
            self.finished = true;
        }
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
}

impl fmt::Debug for DuplexRing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DuplexRing(<redacted>)")
    }
}

/// Header with `body_len` in the first four bytes and version 2 in the fifth, zeros elsewhere.
pub fn wire_v2_header(body_len: usize) -> Result<[u8; WIRE_V2_HEADER_BYTES], ProducerError> {
    let body_len = u32::try_from(body_len).map_err(|_| ProducerError::BoundExceedsSpans)?;
    if body_len as usize > MAX_FRAME_BYTES {
        return Err(ProducerError::BoundExceedsSpans);
    }
    let mut header = [0u8; WIRE_V2_HEADER_BYTES];
    header[0..4].copy_from_slice(&body_len.to_le_bytes());
    header[4] = WIRE_V2_VERSION;
    Ok(header)
}

/// Why a reservation, write, or commit failed. Failures other than `Exhausted`, `Deadline`,
/// and `ReservationOutstanding` abort the reservation.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProducerError {
    /// `bound` exceeds `MAX_FRAME_BYTES`.
    #[error("producer bound exceeds legal spans")]
    BoundExceedsSpans,
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
    /// No free slot, or not enough arena. Retry after a release.
    #[error("bounded ring capacity is exhausted")]
    Exhausted,
    /// This handle already holds an uncommitted reservation. No peer release can clear it;
    /// commit or abort that reservation first.
    #[error("producer reservation is already outstanding")]
    ReservationOutstanding,
    /// `reserve_until` hit its deadline while still `Exhausted`.
    #[error("bounded backpressure deadline elapsed")]
    Deadline,
    /// The next sequence number would overflow `u64`.
    #[error("release sequence exhausted")]
    SequenceExhausted,
    /// The header's version is not 2 or its declared length is not `body_len`.
    #[error("wire header disagrees with committed body")]
    WireHeaderMismatch,
    /// The ring is quarantined.
    #[error("transport storage is quarantined")]
    Quarantined,
    /// `SpanPlan::reserve` or `prefix` failed for a reason other than exhaustion.
    #[error("arena reservation failed")]
    Arena(#[source] ArenaError),
    /// Shared state was unreadable or inconsistent.
    #[error("ring operation failed")]
    Ring(#[source] RingError),
}

impl fmt::Debug for ProducerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Why creating, attaching, or reading a ring failed. Any variant from `try_receive` means
/// the ring is now quarantined.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RingError {
    /// Offset or size arithmetic overflowed.
    #[error("ring arithmetic overflow")]
    ArithmeticOverflow,
    /// The profile's `max_spans` is 1; this backend needs 2.
    #[error("target profile does not match ring backend")]
    ProfileMismatch,
    /// `memfd_create`, `ftruncate`, `mmap`, or `fcntl` failed.
    #[error("shared object setup failed")]
    ObjectSetupFailed,
    /// The memfd failed an owner, size, type, mode, or seal check.
    #[error("shared object validation failed")]
    ObjectValidationFailed,
    /// The grant failed `decode`, or disagrees with the mapping it was presented with.
    #[error("attachment grant is invalid")]
    InvalidGrant,
    /// The mapping's magic, version, or geometry fields disagree with the grant.
    #[error("shared memory layout is invalid")]
    InvalidLayout,
    /// A cursor, slot state, or count is one the protocol cannot produce.
    #[error("shared ring state is invalid")]
    InvalidSharedState,
    /// A doorbell `socketpair`, `poll`, `recv`, or `send` failed, the peer closed its doorbell
    /// end, or an attachment descriptor is not a connected `AF_UNIX` stream socket.
    #[error("ring doorbell failed")]
    DoorbellFailed,
    /// `madvise(MADV_REMOVE)` on a reclaimed arena range failed; the ring is quarantined.
    #[error("shared arena page removal failed")]
    PageRemovalFailed,
    /// A sequence number would overflow `u64`.
    #[error("release sequence exhausted")]
    SequenceExhausted,
    /// The ring is quarantined.
    #[error("transport storage is quarantined")]
    Quarantined,
    /// `trim` was called on a handle that has never reserved; only the producer knows the
    /// live reservation range page removal must avoid.
    #[error("operation belongs to the producer handle")]
    RoleMismatch,
    /// A health check could not observe a stable snapshot because the peer kept the cursors
    /// moving. Nothing was judged; retry when traffic is lighter.
    #[error("shared state was busy for the whole health check")]
    Busy,
    /// A published descriptor failed `FrameDescriptor::validate`.
    #[error("shared descriptor validation failed")]
    Descriptor(#[source] DescriptorError),
    /// A validated frame could not be turned into a `ReceiveLease`.
    #[error("receive lease construction failed")]
    Lease(#[source] LeaseError),
}

impl fmt::Debug for RingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

fn initialize_mapping(
    mapping: &Mapping,
    layout: Layout,
    grant: RingGrant,
) -> Result<(), RingError> {
    mapping.initialize_page(
        layout.producer,
        ProducerPage {
            published: AtomicU64::new(0),
            arena_write: AtomicU64::new(0),
        },
    )?;
    mapping.initialize_page(
        layout.consumer,
        ConsumerPage {
            consumed: AtomicU64::new(0),
            active_leases: AtomicU64::new(0),
        },
    )?;
    mapping.initialize_page(
        layout.reclaim,
        ReclaimPage {
            completed: AtomicU64::new(0),
            arena_reclaimed: AtomicU64::new(0),
        },
    )?;
    for offset in [layout.data_wake, layout.capacity_wake] {
        mapping.initialize_page(
            offset,
            WakeEpoch {
                generation: AtomicU64::new(0),
                parked: AtomicU64::new(0),
            },
        )?;
    }
    for index in 0..grant.descriptor_depth {
        let index = usize::try_from(index).map_err(|_| RingError::ArithmeticOverflow)?;
        mapping.initialize_page(
            layout.slot_offset(index)?,
            DescriptorSlot {
                state: AtomicU8::new(SLOT_FREE),
                completion_sequence: AtomicU64::new(0),
                reservation_len: AtomicU64::new(0),
                descriptor: UnsafeCell::new(SharedDescriptor::ZERO),
            },
        )?;
    }
    mapping.initialize_page(
        layout.lifecycle,
        LifecyclePage {
            magic: MAPPING_MAGIC,
            layout_version: LAYOUT_VERSION,
            descriptor_depth: grant.descriptor_depth,
            arena_bytes: grant.arena_bytes,
            max_leases: grant.max_leases,
            total_bytes: grant.total_bytes,
            incarnation: grant.incarnation.into_bytes(),
            lane: grant.lane,
            quarantined: AtomicU8::new(0),
        },
    )
}

fn validate_lifecycle(
    mapping: &Mapping,
    layout: Layout,
    expected: RingGrant,
) -> Result<(), RingError> {
    let snapshot = mapping.lifecycle_snapshot(layout)?;
    if snapshot.magic != MAPPING_MAGIC
        || snapshot.layout_version != expected.layout_version
        || snapshot.descriptor_depth != expected.descriptor_depth
        || snapshot.arena_bytes != expected.arena_bytes
        || snapshot.max_leases != expected.max_leases
        || snapshot.total_bytes != expected.total_bytes
        || snapshot.incarnation != expected.incarnation.into_bytes()
        || snapshot.lane != expected.lane
    {
        return Err(RingError::InvalidGrant);
    }
    Ok(())
}

fn validate_object(fd: &OwnedFd, expected_len: usize) -> Result<(), RingError> {
    let stat = sys::fstat(fd.as_fd()).map_err(|_| RingError::ObjectValidationFailed)?;
    let current_uid = sys::geteuid();
    let type_valid = stat.mode & libc::S_IFMT == libc::S_IFREG;
    if stat.uid != current_uid
        || stat.size < 0
        || stat.size as usize != expected_len
        || !type_valid
        || stat.mode & 0o077 != 0
    {
        return Err(RingError::ObjectValidationFailed);
    }
    Ok(())
}

fn validate_seals(fd: &OwnedFd) -> Result<(), RingError> {
    let seals = sys::get_seals(fd.as_fd()).map_err(|_| RingError::ObjectValidationFailed)?;
    if seals & sys::RING_SEALS != sys::RING_SEALS {
        return Err(RingError::ObjectValidationFailed);
    }
    Ok(())
}

fn create_linux_memfd(len: usize) -> Result<OwnedFd, RingError> {
    let fd = sys::memfd_create(c"shm-transport").map_err(|_| RingError::ObjectSetupFailed)?;
    let len = libc::off_t::try_from(len).map_err(|_| RingError::ArithmeticOverflow)?;
    sys::ftruncate(fd.as_fd(), len)
        .and_then(|()| sys::fchmod(fd.as_fd(), 0o600))
        .map_err(|_| RingError::ObjectSetupFailed)?;
    Ok(fd)
}

fn seal_object(fd: &OwnedFd) -> Result<(), RingError> {
    sys::add_seals(fd.as_fd(), sys::RING_SEALS).map_err(|_| RingError::ObjectSetupFailed)
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsFd, AsRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use crate::arena::{ArenaError, MIN_ARENA_BYTES};
    use crate::descriptor::{
        DescriptorError, HardwareProfileId, Incarnation, ReleaseIdentity, SETUP_MAPPING_COUNT,
        TransportDescriptor,
    };
    use crate::lease::LeaseError;
    use crate::profile::{ProfileConfig, TargetProfile, WorkerTopology, ring_profile};

    use super::{
        Doorbell, FAIL_NEXT_PAGE_REMOVAL, ProducerError, Ring, RingError, RingGrant,
        SyscallCounters, removal_ranges, residency_vector_len, sys, wire_v2_header,
    };

    fn ring() -> Ring {
        let profile = ring_profile(HardwareProfileId::new("ring-reclaim-test").unwrap()).unwrap();
        Ring::create(&profile, 99).unwrap()
    }

    fn publish(ring: &Ring, bytes: &[u8]) {
        let mut reservation = ring
            .try_reserve(bytes.len(), wire_v2_header(bytes.len()).unwrap())
            .unwrap();
        reservation.write(bytes).unwrap();
        reservation.commit(bytes.len()).unwrap();
    }

    #[test]
    fn syscall_counters_track_only_actual_ring_syscalls() {
        let ring = ring();
        let before = ring.syscall_counters();
        assert_eq!(before, SyscallCounters::default());

        // No peer is parked and no page is dead, so publishing issues no syscall.
        publish(&ring, b"quiet");
        assert_eq!(ring.syscall_counters(), SyscallCounters::default());

        // A wait on a ring with data returns before touching the doorbell.
        assert!(ring.wait_for_data(Instant::now()).unwrap());
        assert_eq!(ring.syscall_counters(), SyscallCounters::default());
        ring.try_receive().unwrap().unwrap().release().unwrap();

        // An empty ring parks once: one drain `recv` while arming, one probe `poll`, one
        // blocking `poll`.
        assert!(
            !ring
                .wait_for_data(Instant::now() + Duration::from_millis(5))
                .unwrap()
        );
        assert_eq!(
            ring.syscall_counters().since(before),
            SyscallCounters {
                doorbell: 3,
                parks: 1,
                page_removals: 0,
            }
        );

        // The released frame's page is dead; `trim` punches it with one `madvise`.
        ring.trim().unwrap();
        assert_eq!(ring.syscall_counters().since(before).page_removals, 1);
    }

    fn clear_nonblock(fd: impl AsFd) {
        let stream = UnixStream::from(fd.as_fd().try_clone_to_owned().unwrap());
        stream.set_nonblocking(false).unwrap();
    }

    #[test]
    fn doorbell_attachment_requires_connected_unix_stream_socket() {
        let not_a_socket: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(matches!(
            Doorbell::from_fd(not_a_socket),
            Err(RingError::DoorbellFailed)
        ));

        let eventfd = sys::eventfd().unwrap();
        assert!(matches!(
            Doorbell::from_fd(eventfd),
            Err(RingError::DoorbellFailed)
        ));

        let unconnected = sys::unix_stream_socket().unwrap();
        assert!(matches!(
            Doorbell::from_fd(unconnected),
            Err(RingError::DoorbellFailed)
        ));

        // `drain` reads a zero-length `recv` as peer close, which a datagram socket makes
        // ambiguous, so `SO_TYPE` must reject it even though it is connected and `AF_UNIX`.
        let (datagram, _datagram_peer) = std::os::unix::net::UnixDatagram::pair().unwrap();
        assert!(matches!(
            Doorbell::from_fd(OwnedFd::from(datagram)),
            Err(RingError::DoorbellFailed)
        ));

        let created = Doorbell::create().unwrap();
        let peer = created.take_peer_end().unwrap();
        assert!(
            matches!(created.take_peer_end(), Err(RingError::DoorbellFailed)),
            "the peer end moves out exactly once"
        );
        let attached = Doorbell::from_fd(peer).unwrap();
        assert!(matches!(
            attached.take_peer_end(),
            Err(RingError::DoorbellFailed)
        ));
    }

    #[test]
    fn doorbell_never_blocks_after_either_end_clears_nonblock() {
        let created = Doorbell::create().unwrap();
        let peer = created.take_peer_end().unwrap();
        clear_nonblock(&created.local);
        clear_nonblock(&peer);
        let attached = Doorbell::from_fd(peer).unwrap();

        let started = std::time::Instant::now();
        created.drain().unwrap();
        attached.drain().unwrap();
        created.signal().unwrap();
        assert!(
            attached
                .wait_until(started + std::time::Duration::from_secs(5))
                .unwrap()
        );
        attached.drain().unwrap();
        attached.drain().unwrap();
        // A peer that fills its own receive queue only makes later signals report EAGAIN.
        for _ in 0..1_000_000 {
            attached.signal().unwrap();
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn closed_peer_doorbell_fails_instead_of_blocking() {
        let created = Doorbell::create().unwrap();
        let attached = Doorbell::from_fd(created.take_peer_end().unwrap()).unwrap();
        drop(created);
        assert!(matches!(attached.signal(), Err(RingError::DoorbellFailed)));
        assert!(matches!(attached.drain(), Err(RingError::DoorbellFailed)));
    }

    #[test]
    fn creator_observes_peer_exit_once_the_attachment_is_handed_over() {
        let ring = ring();
        let attached = ring.attachment().unwrap().attach().unwrap();
        assert!(
            matches!(ring.attachment(), Err(RingError::DoorbellFailed)),
            "peer ends move out once"
        );
        assert!(matches!(
            attached.attachment(),
            Err(RingError::DoorbellFailed)
        ));
        drop(attached);
        assert!(
            matches!(ring.data_ready.drain(), Err(RingError::DoorbellFailed)),
            "a retained peer end would hide the peer's exit"
        );
    }

    #[test]
    fn quarantine_wakes_a_parked_peer() {
        let ring = ring();
        let attached = ring.attachment().unwrap().attach().unwrap();
        let wake = attached.data_wake().unwrap();
        wake.parked.store(1, Ordering::SeqCst);
        ring.enter_quarantine();
        assert!(
            attached
                .data_ready
                .wait_until(std::time::Instant::now() + std::time::Duration::from_secs(5))
                .unwrap(),
            "quarantine must ring the doorbell a parked peer waits on"
        );
        assert!(attached.is_quarantined());
    }

    #[test]
    fn commit_after_quarantine_is_refused_and_aborts() {
        let ring = ring();
        let mut reservation = ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap();
        reservation.write(&[1]).unwrap();
        ring.enter_quarantine();
        assert_eq!(reservation.commit(1), Err(ProducerError::Quarantined));
        let slot = ring.slot(1).unwrap();
        let producer = ring.producer().unwrap();
        assert_eq!(slot.state.load(Ordering::Acquire), super::SLOT_FREE);
        assert_eq!(producer.published.load(Ordering::Acquire), 0);
    }

    #[test]
    fn only_a_producer_handle_may_trim() {
        let ring = ring();
        let consumer = ring.attachment().unwrap().attach().unwrap();
        publish(&ring, &[1; 100]);
        consumer.try_receive().unwrap().unwrap().release().unwrap();
        assert!(matches!(consumer.trim(), Err(RingError::RoleMismatch)));
        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        ring.trim().unwrap();
    }

    #[test]
    fn lengthened_released_descriptor_cannot_reclaim_a_live_frame() {
        let ring = ring();
        publish(&ring, &[1; 4096]);
        publish(&ring, &[2; 4096]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        let live = ring.try_receive().unwrap().unwrap();
        let slot = ring.slot(1).unwrap();
        let mut descriptor = slot.read_descriptor();
        descriptor.allocation_len = 8192;
        slot.write_descriptor(descriptor);
        assert!(matches!(
            ring.try_reserve(0, wire_v2_header(0).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
        let reclaim = ring.reclaim().unwrap();
        assert_eq!(reclaim.arena_reclaimed.load(Ordering::Acquire), 0);
        assert_eq!(live.segment(0).unwrap().read_byte(0), Some(2));
    }

    #[test]
    fn forged_active_lease_count_quarantines_on_release() {
        let ring = ring();
        publish(&ring, &[1]);
        let lease = ring.try_receive().unwrap().unwrap();
        let consumer = ring.consumer().unwrap();
        consumer.active_leases.store(0, Ordering::Release);
        assert_eq!(lease.release(), Err(LeaseError::Quarantined));
        assert!(ring.is_quarantined());
        assert_eq!(
            consumer.active_leases.load(Ordering::Acquire),
            0,
            "the count must not wrap"
        );
    }

    #[test]
    fn rewound_arena_write_quarantines_instead_of_overlapping_a_live_frame() {
        let ring = ring();
        publish(&ring, &[1; 4096]);
        let live = ring.try_receive().unwrap().unwrap();
        let producer = ring.producer().unwrap();
        // In range for `SpanPlan::reserve` (`write >= reclaimed`, used bytes fit), so only the
        // handle's own record can tell it apart from a legitimate cursor.
        producer.arena_write.store(0, Ordering::Release);
        assert!(matches!(
            ring.try_reserve(4096, wire_v2_header(4096).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
        assert_eq!(live.segment(0).unwrap().read_byte(0), Some(1));
    }

    #[test]
    fn rewound_published_cursor_quarantines_even_with_a_freed_slot() {
        let ring = ring();
        publish(&ring, &[1]);
        publish(&ring, &[2]);
        let producer = ring.producer().unwrap();
        let slot = ring.slot(2).unwrap();
        producer.published.store(1, Ordering::Release);
        slot.state.store(super::SLOT_FREE, Ordering::Release);
        assert!(matches!(
            ring.try_reserve(1, wire_v2_header(1).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn forged_consumer_cursors_fail_waits_instead_of_parking() {
        let ring = ring();
        let consumer = ring.consumer().unwrap();
        consumer
            .active_leases
            .store(ring.grant().max_leases + 1, Ordering::Release);
        let started = std::time::Instant::now();
        assert!(matches!(
            ring.wait_for_data(started + std::time::Duration::from_secs(5)),
            Err(RingError::InvalidSharedState)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(ring.is_quarantined());

        let fresh = self::ring();
        let consumer = fresh.consumer().unwrap();
        consumer.consumed.store(7, Ordering::Release);
        assert!(matches!(
            fresh.arm_data_wait(),
            Err(RingError::InvalidSharedState)
        ));
        assert!(fresh.is_quarantined());
    }

    #[test]
    fn trim_reclaims_pending_releases_before_punching() {
        let ring = ring();
        let page = super::system_page_size();
        publish(&ring, &vec![1; page * 2]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        assert_eq!(ring.resident_arena_pages().unwrap(), 2);
        ring.trim().unwrap();
        assert_eq!(
            ring.resident_arena_pages().unwrap(),
            0,
            "an idle trim must reclaim the released frame before punching"
        );
        let (descriptors, bytes) = ring.conservation().unwrap();
        assert_eq!(descriptors.free, ring.grant().descriptor_depth);
        assert_eq!(bytes.free, ring.grant().arena_bytes);
    }

    #[test]
    fn armed_wait_recheck_sees_a_quarantine_that_sent_no_token() {
        let ring = ring();
        assert!(ring.arm_data_wait().unwrap());
        let wake = ring.data_wake().unwrap();
        let lifecycle = ring.mapping.lifecycle_quarantined(ring.layout).unwrap();
        // A peer that quarantined before observing `parked` writes only the flag.
        lifecycle.store(1, Ordering::Release);
        let generation = wake.generation.load(Ordering::SeqCst);
        assert!(matches!(
            ring.armed_wait_holds(wake, generation),
            Err(RingError::Quarantined)
        ));
        assert_eq!(wake.parked.load(Ordering::Acquire), 0);
    }

    #[test]
    fn peer_closing_its_doorbell_quarantines_the_waiting_side() {
        let ring = ring();
        let attached = ring.attachment().unwrap().attach().unwrap();
        drop(attached);
        assert!(matches!(
            ring.wait_for_data(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            Err(RingError::DoorbellFailed)
        ));
        assert!(ring.is_quarantined());
        let wake = ring.data_wake().unwrap();
        assert_eq!(wake.parked.load(Ordering::Acquire), 0);

        let ring = self::ring();
        let attached = ring.attachment().unwrap().attach().unwrap();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![1; arena_len]);
        drop(attached);
        assert!(matches!(
            ring.reserve_until(
                1,
                wire_v2_header(1).unwrap(),
                std::time::Instant::now() + std::time::Duration::from_secs(5)
            ),
            Err(ProducerError::Ring(RingError::DoorbellFailed))
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn sealed_object_of_the_wrong_size_is_refused_before_mapping() {
        let ring = ring();
        let (descriptors, grant) = ring.attachment().unwrap().into_parts();
        let [_, data_ready, capacity_ready] = descriptors;
        let short = sys::memfd_create(c"shm-short-test").unwrap();
        let short_len = (ring.object_size() - super::system_page_size()) as libc::off_t;
        sys::ftruncate(short.as_fd(), short_len).unwrap();
        sys::fchmod(short.as_fd(), 0o600).unwrap();
        super::seal_object(&short).unwrap();
        assert!(matches!(
            Ring::attach([short, data_ready, capacity_ready], grant),
            Err(RingError::ObjectValidationFailed)
        ));
    }

    #[test]
    fn probe_checks_cursors_against_slot_states() {
        let ring = ring();
        publish(&ring, &[1; 100]);
        let lease = ring.try_receive().unwrap().unwrap();
        let held = ring.try_reserve(50, wire_v2_header(50).unwrap()).unwrap();
        ring.probe().unwrap();
        let (descriptors, bytes) = ring.conservation().unwrap();
        assert_eq!(descriptors.receiver_leased, 1);
        assert_eq!(descriptors.producer_reserved, 1);
        assert_eq!(bytes.producer_reserved, 50);
        held.abort();
        lease.release().unwrap();

        for forge in [
            |ring: &Ring| {
                let consumer = ring.consumer().unwrap();
                consumer
                    .active_leases
                    .store(ring.grant().max_leases + 1, Ordering::Release)
            },
            |ring: &Ring| {
                let consumer = ring.consumer().unwrap();
                consumer.consumed.store(5, Ordering::Release);
            },
            |ring: &Ring| {
                let producer = ring.producer().unwrap();
                producer.arena_write.store(4096, Ordering::Release);
            },
        ] {
            let ring = self::ring();
            publish(&ring, &[1]);
            ring.probe().unwrap();
            forge(&ring);
            assert!(matches!(ring.probe(), Err(RingError::InvalidSharedState)));
            assert!(
                ring.is_quarantined(),
                "probe must quarantine, not just report"
            );
        }
    }

    #[test]
    fn rewound_published_cursor_does_not_hide_a_queued_frame() {
        let ring = ring();
        publish(&ring, &[1]);
        publish(&ring, &[2]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        let producer = ring.producer().unwrap();
        // `published == consumed` now, which an unguarded check reads as empty.
        producer.published.store(1, Ordering::Release);
        let started = std::time::Instant::now();
        assert!(matches!(
            ring.wait_for_data(started + std::time::Duration::from_secs(5)),
            Err(RingError::InvalidSharedState)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn attach_sets_close_on_exec_on_every_descriptor() {
        let ring = ring();
        let (descriptors, grant) = ring.attachment().unwrap().into_parts();
        for descriptor in &descriptors {
            sys::clear_cloexec(descriptor.as_fd()).unwrap();
        }
        let raw = descriptors.each_ref().map(AsRawFd::as_raw_fd);
        let attached = Ring::attach(descriptors, grant).unwrap();
        for fd in raw {
            // SAFETY: the attached ring keeps these descriptors open until `drop(attached)`.
            let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
            assert!(
                sys::is_cloexec(borrowed).unwrap(),
                "descriptor {fd} stayed inheritable"
            );
        }
        drop(attached);
    }

    #[test]
    fn attach_refuses_a_mapping_whose_cursors_already_break_the_protocol() {
        for forge in [
            |ring: &Ring| {
                let consumer = ring.consumer().unwrap();
                consumer
                    .active_leases
                    .store(ring.grant().max_leases + 1, Ordering::Release)
            },
            |ring: &Ring| {
                let consumer = ring.consumer().unwrap();
                consumer.consumed.store(3, Ordering::Release);
            },
        ] {
            let ring = ring();
            publish(&ring, &[1]);
            forge(&ring);
            assert!(matches!(
                ring.attachment().unwrap().attach(),
                Err(RingError::InvalidSharedState)
            ));
        }
        // A live ring with traffic in flight attaches.
        let ring = ring();
        publish(&ring, &[1; 4096]);
        let _lease = ring.try_receive().unwrap().unwrap();
        ring.attachment().unwrap().attach().unwrap();
    }

    #[test]
    fn probe_tolerates_every_intermediate_state_of_honest_transitions() {
        // `publish_commit` stores the slot before `published`, and `release` moves the slot
        // before decrementing `active_leases`; a probe between those stores must pass.
        let ring = ring();
        let mut reservation = ring.try_reserve(4, wire_v2_header(4).unwrap()).unwrap();
        reservation.write(&[1; 4]).unwrap();
        let slot = ring.slot(1).unwrap();
        slot.state.store(super::SLOT_PUBLISHED, Ordering::Release);
        ring.probe().unwrap();
        slot.state
            .store(super::SLOT_PRODUCER_RESERVED, Ordering::Release);
        reservation.abort();

        publish(&ring, &[2; 4]);
        let lease = ring.try_receive().unwrap().unwrap();
        slot.state
            .store(super::SLOT_RELEASE_PENDING, Ordering::Release);
        ring.probe().unwrap();
        slot.state
            .store(super::SLOT_RECEIVER_LEASED, Ordering::Release);
        lease.release().unwrap();
        ring.probe().unwrap();
    }

    #[test]
    fn probe_rejects_a_lease_count_more_than_one_transition_from_the_slots() {
        let ring = ring();
        publish(&ring, &[1]);
        publish(&ring, &[2]);
        publish(&ring, &[3]);
        let first = ring.try_receive().unwrap().unwrap();
        let second = ring.try_receive().unwrap().unwrap();
        let third = ring.try_receive().unwrap().unwrap();
        let consumer = ring.consumer().unwrap();
        // The consumer flag is cleared so only the cross-field bound, not this handle's own
        // record, can catch the forgery; that is the producer-side probe's view.
        ring.consumer.set(false);
        consumer.active_leases.store(0, Ordering::Release);
        assert!(matches!(ring.probe(), Err(RingError::InvalidSharedState)));
        assert!(ring.is_quarantined());
        drop((first, second, third));
    }

    #[test]
    fn attach_refuses_a_phantom_lease_count_that_a_probe_would_tolerate() {
        let ring = ring();
        let consumer = ring.consumer().unwrap();
        // One transition's worth of skew is legal mid-operation but not on an idle mapping.
        consumer.active_leases.store(1, Ordering::Release);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::InvalidSharedState)
        ));
    }

    #[test]
    fn published_running_ahead_of_depth_quarantines_before_any_delivery() {
        let ring = ring();
        publish(&ring, &[1]);
        let producer = ring.producer().unwrap();
        let depth = ring.grant().descriptor_depth;
        producer.published.store(depth + 1, Ordering::Release);
        assert!(matches!(
            ring.try_receive(),
            Err(RingError::InvalidSharedState)
        ));
        assert!(ring.is_quarantined());

        let ring = self::ring();
        publish(&ring, &[1]);
        let producer = ring.producer().unwrap();
        producer.published.store(depth + 1, Ordering::Release);
        assert!(matches!(
            ring.wait_for_data(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            Err(RingError::InvalidSharedState)
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn attach_refuses_an_orphaned_receiver_slot() {
        let ring = ring();
        let slot = ring.slot(1).unwrap();
        // Cursors all zero, one slot receiver-owned: no honest history produces this.
        slot.state
            .store(super::SLOT_RELEASE_PENDING, Ordering::Release);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::InvalidSharedState)
        ));
    }

    #[test]
    fn probe_treats_receiver_slots_beyond_the_cursor_gap_as_a_fault() {
        let ring = ring();
        publish(&ring, &[1]);
        publish(&ring, &[2]);
        let first = ring.try_receive().unwrap().unwrap();
        let second = ring.try_receive().unwrap().unwrap();
        ring.probe().unwrap();
        // Two receiver-owned slots with `consumed` rewound to zero is two transitions of skew.
        ring.consumer.set(false);
        let consumer = ring.consumer().unwrap();
        consumer.consumed.store(0, Ordering::Release);
        assert!(matches!(ring.probe(), Err(RingError::InvalidSharedState)));
        assert!(ring.is_quarantined());
        drop((first, second));
    }

    #[test]
    fn owned_cursor_advance_fails_closed_when_the_shared_value_moved() {
        let cursor = AtomicU64::new(5);
        Ring::advance_cursor(&cursor, 5, 4).unwrap();
        assert!(matches!(
            Ring::advance_cursor(&cursor, 5, 3),
            Err(RingError::InvalidSharedState)
        ));
        assert_eq!(
            cursor.load(Ordering::Acquire),
            4,
            "a failed exchange writes nothing"
        );
    }

    #[test]
    fn publication_that_raced_a_quarantine_is_not_reported_as_delivered() {
        let ring = ring();
        let mut reservation = ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap();
        reservation.write(&[1]).unwrap();
        let prepared = ring
            .prepare_commit(1, reservation.plan, 1, reservation.wire_header)
            .unwrap();
        let lifecycle = ring.mapping.lifecycle_quarantined(ring.layout).unwrap();
        // The peer quarantines after `commit`'s check would have passed.
        lifecycle.store(1, Ordering::Release);
        assert_eq!(
            ring.publish_commit(prepared),
            Err(ProducerError::Quarantined)
        );
        reservation.finished = true;
        assert!(ring.is_quarantined());
    }

    #[test]
    fn health_check_bounds_do_not_overflow_on_forged_cursors() {
        let ring = ring();
        let producer = ring.producer().unwrap();
        let consumer = ring.consumer().unwrap();
        producer.published.store(u64::MAX, Ordering::Release);
        consumer.consumed.store(u64::MAX, Ordering::Release);
        assert!(matches!(ring.probe(), Err(RingError::InvalidSharedState)));
    }

    #[test]
    fn aborted_reservation_leaves_no_resident_pages() {
        let ring = ring();
        let page = super::system_page_size();
        let mut reservation = ring
            .try_reserve(page * 2, wire_v2_header(page * 2).unwrap())
            .unwrap();
        reservation.write(&vec![7; page * 2]).unwrap();
        assert_eq!(ring.resident_arena_pages().unwrap(), 2);
        reservation.abort();
        assert_eq!(
            ring.resident_arena_pages().unwrap(),
            0,
            "an aborted reservation's pages sit above every reclaim cursor and must be punched on abort"
        );
        let (descriptors, bytes) = ring.conservation().unwrap();
        assert_eq!(descriptors.free, ring.grant().descriptor_depth);
        assert_eq!(bytes.free, ring.grant().arena_bytes);
    }

    #[test]
    fn attach_refuses_a_quarantined_ring() {
        let ring = ring();
        let attachment = ring.attachment().unwrap();
        ring.enter_quarantine();
        assert!(matches!(attachment.attach(), Err(RingError::Quarantined)));
    }

    #[test]
    fn receive_that_raced_a_quarantine_is_not_reported_as_delivered() {
        let ring = ring();
        publish(&ring, &[1]);
        let lifecycle = ring.mapping.lifecycle_quarantined(ring.layout).unwrap();
        lifecycle.store(1, Ordering::Release);
        // The wrapper's first check catches this; the inner path's own success is what the
        // post-check guards, so drive it directly.
        let lease = ring.try_receive_inner().unwrap();
        assert!(lease.is_some());
        drop(lease);
        assert!(matches!(ring.try_receive(), Err(RingError::Quarantined)));
    }

    #[test]
    fn two_producer_reserved_slots_are_impossible() {
        let ring = ring();
        let first = ring.slot(1).unwrap();
        let second = ring.slot(2).unwrap();
        first
            .state
            .store(super::SLOT_PRODUCER_RESERVED, Ordering::Release);
        second
            .state
            .store(super::SLOT_PRODUCER_RESERVED, Ordering::Release);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::InvalidSharedState)
        ));
        assert!(matches!(ring.probe(), Err(RingError::InvalidSharedState)));
    }

    #[test]
    fn release_leaves_the_consumers_data_wait_armed_for_the_next_publish() {
        let producer = ring();
        let consumer = producer.attachment().unwrap().attach().unwrap();
        publish(&producer, &[1]);
        let lease = consumer.try_receive().unwrap().unwrap();
        assert!(
            consumer.arm_data_wait().unwrap(),
            "empty ring: blocking is correct"
        );
        lease.release().unwrap();
        let wake = consumer.data_wake().unwrap();
        assert_ne!(
            wake.parked.load(Ordering::Acquire),
            0,
            "a release must not clear the consumer's own parked marker"
        );
        publish(&producer, &[2]);
        assert!(
            consumer
                .data_ready
                .wait_until(std::time::Instant::now() + std::time::Duration::from_secs(5))
                .unwrap(),
            "the publish after a release must still wake the parked consumer"
        );
        consumer.complete_data_wait().unwrap();
        consumer.try_receive().unwrap().unwrap().release().unwrap();
    }

    #[test]
    fn attach_refuses_a_write_cursor_beyond_the_committed_frames() {
        let ring = ring();
        let producer = ring.producer().unwrap();
        // Every slot free, yet the arena reads as full: nothing could ever be released.
        producer
            .arena_write
            .store(ring.grant().arena_bytes, Ordering::Release);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::InvalidSharedState)
        ));
    }

    #[test]
    fn attach_refuses_a_live_slot_whose_descriptor_does_not_validate() {
        let ring = ring();
        publish(&ring, &[1]);
        let slot = ring.slot(1).unwrap();
        slot.write_descriptor(super::SharedDescriptor::ZERO);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::Descriptor(_))
        ));

        let ring = self::ring();
        publish(&ring, &[1; 100]);
        let slot = ring.slot(1).unwrap();
        slot.reservation_len.store(99, Ordering::Release);
        assert!(matches!(
            ring.attachment().unwrap().attach(),
            Err(RingError::InvalidSharedState)
        ));
    }

    #[test]
    fn descriptor_depth_above_the_cap_is_rejected_before_any_allocation() {
        let mut bytes = ring().grant().encode();
        bytes[22..30].copy_from_slice(&((super::MAX_DESCRIPTOR_DEPTH as u64) + 1).to_le_bytes());
        assert!(matches!(
            RingGrant::decode(bytes),
            Err(RingError::InvalidGrant)
        ));

        let profile = TargetProfile::new(ProfileConfig {
            descriptor: TransportDescriptor::new(HardwareProfileId::new("ring-deep").unwrap()),
            descriptor_depth: super::MAX_DESCRIPTOR_DEPTH + 1,
            arena_bytes: MIN_ARENA_BYTES,
            max_spans: 2,
            max_leases: 1,
            mappings: SETUP_MAPPING_COUNT,
            pinned_workers: 0,
            worker_topology: WorkerTopology::CallerThread,
        })
        .unwrap();
        assert!(matches!(
            Ring::create(&profile, 0),
            Err(RingError::InvalidLayout)
        ));
    }

    #[test]
    fn oversized_active_lease_count_quarantines_on_receive() {
        let ring = ring();
        publish(&ring, &[1]);
        let consumer = ring.consumer().unwrap();
        consumer.active_leases.store(u64::MAX, Ordering::Release);
        assert!(matches!(
            ring.try_receive(),
            Err(RingError::InvalidSharedState)
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn unaligned_arena_is_rejected_before_any_frame_flows() {
        let profile = TargetProfile::new(ProfileConfig {
            descriptor: TransportDescriptor::new(
                HardwareProfileId::new("ring-unaligned-arena").unwrap(),
            ),
            descriptor_depth: 4,
            arena_bytes: MIN_ARENA_BYTES + 1,
            max_spans: 2,
            max_leases: 4,
            mappings: SETUP_MAPPING_COUNT,
            pinned_workers: 0,
            worker_topology: WorkerTopology::CallerThread,
        })
        .unwrap();
        assert!(matches!(
            Ring::create(&profile, 0),
            Err(RingError::InvalidLayout)
        ));

        let mut bytes = ring().grant().encode();
        let arena = u64::from_le_bytes(bytes[30..38].try_into().unwrap()) + 1;
        bytes[30..38].copy_from_slice(&arena.to_le_bytes());
        assert!(matches!(
            RingGrant::decode(bytes),
            Err(RingError::InvalidGrant)
        ));
    }

    /// Every identity mismatch at release time names what changed and quarantines, since the
    /// identity was validated when the lease was built.
    #[test]
    fn mismatched_release_identity_names_the_field_and_quarantines() {
        type Forge = fn(ReleaseIdentity) -> ReleaseIdentity;
        let cases: [(Forge, LeaseError); 4] = [
            (
                |id| {
                    ReleaseIdentity::new(
                        Incarnation::from_bytes([99; 16]),
                        id.lane(),
                        id.sequence(),
                    )
                },
                LeaseError::WrongIncarnation,
            ),
            (
                |id| ReleaseIdentity::new(id.incarnation(), id.lane() + 1, id.sequence()),
                LeaseError::WrongLane,
            ),
            (
                |id| ReleaseIdentity::new(id.incarnation(), id.lane(), id.sequence() + 99),
                LeaseError::InvalidSequence,
            ),
            (
                |id| ReleaseIdentity::new(id.incarnation(), id.lane(), id.sequence() + 1),
                LeaseError::DuplicateRelease,
            ),
        ];
        for (forge, expected) in cases {
            let ring = ring();
            publish(&ring, &[1]);
            publish(&ring, &[2]);
            let first = ring.try_receive().unwrap().unwrap();
            ring.try_receive().unwrap().unwrap().release().unwrap();
            assert_eq!(ring.release(forge(first.identity())), Err(expected));
            assert!(ring.is_quarantined(), "{expected:?} must quarantine");
            assert_eq!(first.release(), Err(LeaseError::Quarantined));
        }
    }

    #[test]
    fn stale_lap_release_cannot_complete_recycled_slot() {
        let ring = ring();
        let depth = ring.grant().descriptor_depth;

        publish(&ring, &[1]);
        let stale = ring.try_receive().unwrap().unwrap();
        let stale_id = stale.identity();
        stale.release().unwrap();
        for value in 2..=depth {
            publish(&ring, &[value as u8]);
            ring.try_receive().unwrap().unwrap().release().unwrap();
        }

        publish(&ring, &[0xa5]);
        let fresh = ring.try_receive().unwrap().unwrap();
        assert_eq!(
            ring.release(stale_id),
            Err(LeaseError::InvalidSequence),
            "stale identity must not complete recycled slot"
        );
        assert!(ring.is_quarantined());
        let slot = ring.slot(stale_id.sequence()).unwrap();
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            super::SLOT_RECEIVER_LEASED,
            "the recycled slot stays leased to the fresh frame"
        );
        assert_eq!(fresh.segment(0).unwrap().read_byte(0), Some(0xa5));
        assert_eq!(fresh.release(), Err(LeaseError::Quarantined));
    }

    #[test]
    fn shared_quarantine_flag_latches_locally_when_observed() {
        let ring = ring();
        let lifecycle = ring.mapping.lifecycle_quarantined(ring.layout).unwrap();
        lifecycle.store(1, Ordering::Release);
        assert!(ring.is_quarantined());
        lifecycle.store(0, Ordering::Release);
        assert!(
            ring.is_quarantined(),
            "a cleared shared flag must not revive the ring"
        );
        assert!(matches!(
            ring.wait_for_data(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            Err(RingError::Quarantined)
        ));
        assert!(matches!(ring.arm_data_wait(), Err(RingError::Quarantined)));
    }

    #[test]
    fn foreign_slot_state_on_reserve_is_a_fault_not_backpressure() {
        let ring = ring();
        let slot = ring.slot(1).unwrap();
        slot.state
            .store(super::SLOT_PRODUCER_RESERVED, Ordering::Release);
        assert!(matches!(
            ring.try_reserve(1, wire_v2_header(1).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn failed_publication_wake_leaves_the_slot_published() {
        let ring = ring();
        // Dropping the peer end makes the next wake signal fail with EPIPE.
        ring.data_ready.remote.take();
        let wake = ring.data_wake().unwrap();
        wake.parked.store(1, Ordering::Release);

        let mut reservation = ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap();
        reservation.write(&[9]).unwrap();
        assert!(matches!(
            reservation.commit(1),
            Err(ProducerError::Ring(RingError::DoorbellFailed))
        ));
        assert!(ring.is_quarantined());
        let slot = ring.slot(1).unwrap();
        let producer = ring.producer().unwrap();
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            super::SLOT_PUBLISHED,
            "a published slot must not be rolled back to free"
        );
        assert_eq!(slot.reservation_len.load(Ordering::Acquire), 1);
        assert_eq!(producer.published.load(Ordering::Acquire), 1);
    }

    #[test]
    fn forged_arena_write_quarantines_instead_of_underflowing() {
        let ring = ring();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![1; arena_len / 2]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        let producer = ring.producer().unwrap();
        producer
            .arena_write
            .store(3 * arena_len as u64, Ordering::Release);
        assert!(matches!(
            ring.try_reserve(0, wire_v2_header(0).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn unaligned_batch_boundaries_do_not_strand_pages() {
        let ring = ring();
        let page = super::system_page_size();
        let batch = ring.punch_batch_bytes() as usize;
        assert!(batch > page);

        publish(&ring, &vec![1; batch + 100]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        assert_eq!(
            ring.resident_arena_pages().unwrap(),
            1,
            "only the boundary page keeps its dead prefix resident"
        );

        publish(&ring, &vec![2; batch]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        assert_eq!(
            ring.resident_arena_pages().unwrap(),
            1,
            "the earlier boundary page must be removed once its tail is dead"
        );
    }

    #[test]
    fn residency_vector_tracks_runtime_page_size() {
        let mapping_len = 128 * 1024 + 1;
        assert_eq!(residency_vector_len(mapping_len, 16 * 1024), 9);
        assert_eq!(residency_vector_len(mapping_len, 64 * 1024), 3);
    }

    #[test]
    fn removal_ranges_exclude_partial_pages_and_split_once_at_wrap() {
        for page in [4 * 1024, 16 * 1024, 64 * 1024] {
            let arena = page * 4;
            assert_eq!(
                removal_ranges(page, arena, 1, (page - 1) as u64, page).unwrap(),
                [(0, 0), (0, 0)]
            );
            assert_eq!(
                removal_ranges(page, arena, 1, (page * 3 - 2) as u64, page).unwrap(),
                [(page * 2, page), (0, 0)]
            );
            assert_eq!(
                removal_ranges(page, arena, (arena - page) as u64, (page * 2) as u64, page)
                    .unwrap(),
                [(page * 4, page), (page, page)]
            );
        }
    }

    #[test]
    fn reclaimed_pages_leave_residency_and_reuse_as_zeroes() {
        let ring = ring();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![0xa5; arena_len]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        assert!(ring.resident_arena_pages().unwrap() > 0);

        let reservation = ring
            .try_reserve(arena_len, wire_v2_header(arena_len).unwrap())
            .unwrap();
        assert_eq!(ring.resident_arena_pages().unwrap(), 0);
        let segment = reservation.segment(0).unwrap().unwrap();
        assert_eq!(segment.read_byte(0), Some(0));
        assert_eq!(segment.read_byte(segment.len() - 1), Some(0));
        reservation.abort();
    }

    #[test]
    fn subpage_releases_stay_resident_until_trim() {
        let ring = ring();
        let page = super::system_page_size();
        assert!(page >= 256 && page.is_multiple_of(256));

        for index in 0..page / 256 {
            publish(&ring, &[index as u8; 256]);
            ring.try_receive().unwrap().unwrap().release().unwrap();
            ring.try_reserve(0, wire_v2_header(0).unwrap())
                .unwrap()
                .abort();
            assert_eq!(ring.resident_arena_pages().unwrap(), 1);
        }
        ring.trim().unwrap();
        assert_eq!(ring.resident_arena_pages().unwrap(), 0);

        publish(&ring, &[0x5a; 256]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        assert_eq!(ring.resident_arena_pages().unwrap(), 1);
        ring.trim().unwrap();
        assert_eq!(ring.resident_arena_pages().unwrap(), 0);
    }

    #[test]
    fn partial_page_reclaim_preserves_live_neighbor() {
        let ring = ring();
        publish(&ring, &[0x11; 256]);
        publish(&ring, &[0x22; 256]);
        let first = ring.try_receive().unwrap().unwrap();
        let second = ring.try_receive().unwrap().unwrap();
        first.release().unwrap();

        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        ring.trim().unwrap();
        assert_eq!(second.segment(0).unwrap().read_byte(0), Some(0x22));
        assert_eq!(second.segment(0).unwrap().read_byte(255), Some(0x22));
        second.release().unwrap();
    }

    #[test]
    fn trim_preserves_bytes_of_an_uncommitted_reservation() {
        let ring = ring();
        publish(&ring, &[0x11; 100]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        ring.try_reserve(0, wire_v2_header(0).unwrap())
            .unwrap()
            .abort();
        // Drained: `arena_reclaimed == arena_write == 100`, mid-page. The reservation now
        // starts inside the page that `trim` would otherwise treat as fully dead.
        let mut held = ring.try_reserve(50, wire_v2_header(50).unwrap()).unwrap();
        held.write(&[0x33; 50]).unwrap();

        assert_eq!(
            ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap_err(),
            ProducerError::ReservationOutstanding
        );
        ring.trim().unwrap();
        let span = held.segment(0).unwrap().unwrap();
        assert_eq!(span.read_byte(0), Some(0x33));
        assert_eq!(span.read_byte(49), Some(0x33));

        held.commit(50).unwrap();
        let lease = ring.try_receive().unwrap().unwrap();
        assert_eq!(lease.to_vec().unwrap(), vec![0x33; 50]);
        lease.release().unwrap();
    }

    #[test]
    fn outstanding_reservation_is_refused_without_parking() {
        let ring = ring();
        let held = ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap();
        assert_eq!(
            ring.try_reserve(1, wire_v2_header(1).unwrap()).unwrap_err(),
            ProducerError::ReservationOutstanding
        );
        let started = std::time::Instant::now();
        assert_eq!(
            ring.reserve_until(
                1,
                wire_v2_header(1).unwrap(),
                started + std::time::Duration::from_secs(5),
            )
            .unwrap_err(),
            ProducerError::ReservationOutstanding
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        held.abort();
        ring.try_reserve(1, wire_v2_header(1).unwrap())
            .unwrap()
            .abort();
    }

    #[test]
    fn reserve_until_deadline_leaves_the_capacity_wake_unparked() {
        let ring = ring();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![1; arena_len]);
        let started = Instant::now();
        assert_eq!(
            ring.reserve_until(
                1,
                wire_v2_header(1).unwrap(),
                started + Duration::from_millis(30),
            )
            .unwrap_err(),
            ProducerError::Deadline
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let wake = ring.capacity_wake().unwrap();
        assert_eq!(
            wake.parked.load(Ordering::Acquire),
            0,
            "a timed-out park must not leave the producer marked parked"
        );
    }

    #[test]
    fn stale_capacity_token_after_a_drain_does_not_deadlock_the_next_park() {
        let ring = ring();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![1; arena_len]);
        let attachment = ring.attachment().unwrap();
        let (done, wait_for_done) = std::sync::mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            let consumer = attachment.attach().unwrap();
            consumer.capacity_ready.signal().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            consumer.try_receive().unwrap().unwrap().release().unwrap();
            let _ = wait_for_done.recv();
        });
        let started = Instant::now();
        ring.reserve_until(
            1,
            wire_v2_header(1).unwrap(),
            started + Duration::from_secs(10),
        )
        .unwrap()
        .abort();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the stale token must cause at most a spurious wake, never a missed one"
        );
        done.send(()).unwrap();
        peer.join().unwrap();
    }

    #[test]
    fn page_removal_failure_quarantines_before_capacity_publication() {
        let ring = ring();
        let arena_len = ring.arena_bytes();
        publish(&ring, &vec![1; arena_len]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        FAIL_NEXT_PAGE_REMOVAL.store(true, Ordering::Release);

        assert!(matches!(
            ring.try_reserve(0, wire_v2_header(0).unwrap()),
            Err(ProducerError::Ring(RingError::PageRemovalFailed))
        ));
        assert!(ring.is_quarantined());
        let reclaim = ring.reclaim().unwrap();
        assert_eq!(reclaim.completed.load(Ordering::Acquire), 0);
        assert_eq!(reclaim.arena_reclaimed.load(Ordering::Acquire), 0);
    }

    #[test]
    fn quarantine_survives_peer_clearing_shared_flag() {
        let ring = ring();
        publish(&ring, &[1]);
        ring.enter_quarantine();
        let lifecycle = ring.mapping.lifecycle_quarantined(ring.layout).unwrap();
        lifecycle.store(0, Ordering::Release);
        assert!(ring.is_quarantined());
        assert!(matches!(ring.try_receive(), Err(RingError::Quarantined)));
        assert_eq!(
            ring.try_reserve(0, wire_v2_header(0).unwrap()).unwrap_err(),
            ProducerError::Quarantined
        );
        assert!(matches!(ring.trim(), Err(RingError::Quarantined)));
    }

    #[test]
    fn impossible_slot_state_quarantines_the_receiver() {
        let ring = ring();
        publish(&ring, &[1]);
        let slot = ring.slot(1).unwrap();
        slot.state
            .store(super::SLOT_RELEASE_PENDING, Ordering::Release);
        assert!(matches!(
            ring.try_receive(),
            Err(RingError::InvalidSharedState)
        ));
        assert!(ring.is_quarantined());
    }

    #[test]
    fn forged_reclaim_length_quarantines_the_producer() {
        let ring = ring();
        let arena_len = ring.arena_bytes() as u64;
        publish(&ring, &[1; 16]);
        ring.try_receive().unwrap().unwrap().release().unwrap();
        let slot = ring.slot(1).unwrap();
        let mut descriptor = slot.read_descriptor();
        descriptor.allocation_len = arena_len;
        slot.write_descriptor(descriptor);
        assert!(matches!(
            ring.try_reserve(0, wire_v2_header(0).unwrap()),
            Err(ProducerError::Ring(RingError::InvalidSharedState))
        ));
        assert!(ring.is_quarantined());
        assert!(ring.resident_arena_pages().unwrap() > 0);
    }

    #[test]
    fn wrapped_errors_preserve_sources() {
        use std::error::Error;

        let producer = ProducerError::Arena(ArenaError::Exhausted);
        assert!(producer.source().unwrap().is::<ArenaError>());
        let producer = ProducerError::Ring(RingError::InvalidGrant);
        assert!(producer.source().unwrap().is::<RingError>());
        assert!(ProducerError::Exhausted.source().is_none());

        let ring = RingError::Descriptor(DescriptorError::Truncated);
        assert!(ring.source().unwrap().is::<DescriptorError>());
        let ring = RingError::Lease(LeaseError::InvalidSpan);
        assert!(ring.source().unwrap().is::<LeaseError>());
        assert!(RingError::InvalidGrant.source().is_none());
    }
}
