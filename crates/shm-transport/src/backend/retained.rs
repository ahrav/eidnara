//! Backing that outlives an endpoint: the mapping, its completion cells, the capacity wake
//! handle, and the receiver's per-block records. An owned `PayloadLease` keeps one of these
//! alive, so a block a worker is still reading stays mapped after its `Ring` exits.

use std::cell::UnsafeCell;
use std::fmt;
use std::mem::size_of;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// The kernel page size, cached; falls back to `BLOCK_ALIGN` if `sysconf` reports none.
pub(crate) fn system_page_size() -> usize {
    static PAGE_SIZE_CACHE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE_CACHE.get_or_init(|| match sys::page_size() {
        0 => crate::pool::BLOCK_ALIGN,
        size => size,
    })
}

use crate::backend::sys;
use crate::descriptor::{Incarnation, WIRE_V3_HEADER_BYTES};
use crate::pool::{
    CACHELINE, CLASS_COUNT, DESCRIPTOR_SLOT_BYTES, LIFECYCLE_PAGE_BYTES, MappingLayout,
    PoolGeometry, RETURN_WORD_BITS,
};
use crate::profile::BackingAdmission;

/// Magic at the start of the lifecycle page; distinguishes a pool mapping from any other memfd.
pub(crate) const MAPPING_MAGIC: u64 = 0x4d43_5348_4d50_3034;
/// Mapping layout version; the sole version either peer accepts.
pub const LAYOUT_VERSION: u16 = 4;

/// Bytes of padding that keep a control page exactly one cacheline. Explicit padding lets
/// `shared_page` hand out `&Page` over peer-writable memory: a shared reference permits
/// foreign mutation only of bytes inside an atomic or an `UnsafeCell`, and implicit padding
/// is neither.
const CONTROL_PAGE_PADDING: usize = CACHELINE - size_of::<AtomicU64>();

#[repr(C, align(128))]
pub(crate) struct ProducerPage {
    /// Publication sequence of the most recent descriptor; Release-stored after the slot.
    pub(crate) published: AtomicU64,
    _padding: UnsafeCell<[u8; CONTROL_PAGE_PADDING]>,
}

#[repr(C, align(128))]
pub(crate) struct ConsumerPage {
    /// Publication sequence of the most recent descriptor the consumer captured; Release-stored
    /// after the slot's fields were copied out.
    pub(crate) consumed: AtomicU64,
    _padding: UnsafeCell<[u8; CONTROL_PAGE_PADDING]>,
}

#[repr(C, align(128))]
pub(crate) struct WakeEpoch {
    pub(crate) generation: AtomicU64,
    pub(crate) parked: AtomicU64,
    _padding: UnsafeCell<[u8; CACHELINE - 2 * size_of::<AtomicU64>()]>,
}

/// One published frame. Every field is a fixed-width atomic so the consumer copies them out
/// word by word under the Acquire it performed on `published`; there is no whole-struct read.
#[repr(C, align(64))]
pub(crate) struct DescriptorSlot {
    pub(crate) sequence: AtomicU64,
    pub(crate) block: AtomicU64,
    pub(crate) generation: AtomicU64,
    pub(crate) body_len: AtomicU64,
    _padding: UnsafeCell<[u8; DESCRIPTOR_SLOT_BYTES - 4 * size_of::<AtomicU64>()]>,
}

/// One block's return cell. The final lease owner publishes the captured generation with
/// Release; the producer observes it with Acquire before reusing the block.
#[repr(C, align(8))]
pub(crate) struct CompletionCell {
    pub(crate) generation: AtomicU64,
}

/// Plain fields written once by the creator; both peers read them volatile and compare against
/// the grant. `quarantined` is the one field written after creation.
#[repr(C, align(256))]
pub(crate) struct LifecyclePage {
    pub(crate) magic: u64,
    pub(crate) layout_version: u16,
    pub(crate) lane: u32,
    pub(crate) incarnation: [u8; 16],
    pub(crate) total_bytes: u64,
    pub(crate) ordinary_descriptors: u32,
    pub(crate) reserved_descriptors: u32,
    pub(crate) class_bytes: [u64; CLASS_COUNT],
    pub(crate) class_counts: [u32; CLASS_COUNT],
    pub(crate) quarantined: AtomicU8,
}

/// Volatile copy of the plain `LifecyclePage` fields; `validate_lifecycle` compares it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifecycleSnapshot {
    pub(crate) magic: u64,
    pub(crate) layout_version: u16,
    pub(crate) lane: u32,
    pub(crate) incarnation: [u8; 16],
    pub(crate) total_bytes: u64,
    pub(crate) ordinary_descriptors: u32,
    pub(crate) reserved_descriptors: u32,
    pub(crate) class_bytes: [u64; CLASS_COUNT],
    pub(crate) class_counts: [u32; CLASS_COUNT],
}

const _: () = {
    use std::mem::offset_of;
    assert!(size_of::<ProducerPage>() == CACHELINE);
    assert!(offset_of!(ProducerPage, _padding) == 8);
    assert!(size_of::<ConsumerPage>() == CACHELINE);
    assert!(offset_of!(ConsumerPage, _padding) == 8);
    assert!(size_of::<WakeEpoch>() == CACHELINE);
    assert!(offset_of!(WakeEpoch, _padding) == 16);
    assert!(size_of::<DescriptorSlot>() == DESCRIPTOR_SLOT_BYTES);
    assert!(offset_of!(DescriptorSlot, sequence) == 0);
    assert!(offset_of!(DescriptorSlot, block) == 8);
    assert!(offset_of!(DescriptorSlot, generation) == 16);
    assert!(offset_of!(DescriptorSlot, body_len) == 24);
    assert!(offset_of!(DescriptorSlot, _padding) == 32);
    assert!(size_of::<CompletionCell>() == 8);
    assert!(size_of::<LifecyclePage>() == LIFECYCLE_PAGE_BYTES);
    assert!(offset_of!(LifecyclePage, magic) == 0);
    assert!(offset_of!(LifecyclePage, layout_version) == 8);
    assert!(offset_of!(LifecyclePage, lane) == 12);
    assert!(offset_of!(LifecyclePage, incarnation) == 16);
    assert!(offset_of!(LifecyclePage, total_bytes) == 32);
    assert!(offset_of!(LifecyclePage, ordinary_descriptors) == 40);
    assert!(offset_of!(LifecyclePage, reserved_descriptors) == 44);
    assert!(offset_of!(LifecyclePage, class_bytes) == 48);
    assert!(offset_of!(LifecyclePage, class_counts) == 104);
    assert!(offset_of!(LifecyclePage, quarantined) == 132);
};

/// Why a mapping could not be created, attached, or addressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MappingError {
    ObjectSetupFailed,
    ObjectValidationFailed,
    InvalidLayout,
    ArithmeticOverflow,
}

pub(crate) struct Mapping {
    fd: OwnedFd,
    base: NonNull<u8>,
    len: usize,
}

impl Mapping {
    pub(crate) fn create(len: usize) -> Result<Self, MappingError> {
        let fd = create_linux_memfd(len)?;
        validate_object(&fd, len)?;
        let base =
            sys::mmap_shared(fd.as_fd(), len).map_err(|_| MappingError::ObjectSetupFailed)?;
        Ok(Self { fd, base, len })
    }

    /// Anonymous private mapping with a placeholder descriptor, for accessor and lease tests
    /// that must run under Miri, which cannot map a memfd.
    #[cfg(test)]
    pub(crate) fn anonymous(len: usize) -> Result<Self, MappingError> {
        let fd = sys::eventfd().map_err(|_| MappingError::ObjectSetupFailed)?;
        let base = sys::mmap_anonymous(len).map_err(|_| MappingError::ObjectSetupFailed)?;
        Ok(Self { fd, base, len })
    }

    pub(crate) fn attach(fd: OwnedFd, len: usize) -> Result<Self, MappingError> {
        // Seals first: once `F_SEAL_SHRINK | F_SEAL_GROW` are observed the size read below
        // cannot change, so a peer cannot shrink the object between the size check and the
        // mapping and leave a page whose first touch is `SIGBUS`.
        validate_seals(&fd)?;
        validate_object(&fd, len)?;
        let base =
            sys::mmap_shared(fd.as_fd(), len).map_err(|_| MappingError::ObjectSetupFailed)?;
        Ok(Self { fd, base, len })
    }

    pub(crate) const fn fd(&self) -> &OwnedFd {
        &self.fd
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn ptr_at<T>(&self, offset: usize) -> Result<*mut T, MappingError> {
        let end = offset
            .checked_add(size_of::<T>())
            .ok_or(MappingError::ArithmeticOverflow)?;
        if end > self.len {
            return Err(MappingError::InvalidLayout);
        }
        // SAFETY: checked offset remains inside mapping.
        Ok(unsafe { self.base.as_ptr().add(offset).cast() })
    }

    /// # Safety
    ///
    /// Every byte of `T`, padding included, must lie inside an atomic or an `UnsafeCell`, so
    /// that concurrent peer writes anywhere in the page are permitted behind `&T`. Peer stores
    /// are assumed atomic-width; a peer that tears a store violates the protocol.
    pub(crate) unsafe fn shared_page<T>(&self, offset: usize) -> Result<&T, MappingError> {
        let ptr = self.ptr_at::<T>(offset)?;
        if ptr.addr() % std::mem::align_of::<T>() != 0 {
            return Err(MappingError::InvalidLayout);
        }
        // SAFETY: bounds: `ptr_at` checked `offset + size_of::<T>()` against `self.len`.
        // Lifetime: the mapping is unmapped only in `Drop`, after `&self` ends.
        // Alignment: checked above. Validity: atomics and `UnsafeCell<[u8; N]>` accept every
        // bit pattern. Aliasing: the caller ensures every byte of `T` is atomic or inside an
        // `UnsafeCell`.
        Ok(unsafe { &*ptr })
    }

    /// Writes a whole page. Only `initialize_mapping` calls this, on a mapping no peer has yet.
    pub(crate) fn initialize_page<T>(&self, offset: usize, value: T) -> Result<(), MappingError> {
        let ptr = self.ptr_at::<T>(offset)?;
        if ptr.addr() % std::mem::align_of::<T>() != 0 {
            return Err(MappingError::InvalidLayout);
        }
        // SAFETY: `ptr_at` bounds-checked the range and alignment was checked above; the
        // mapping is fresh and unshared until creation returns, so nothing else reads or
        // writes it during this store.
        unsafe { ptr.write(value) };
        Ok(())
    }

    /// Pointer into the arena for `[offset, offset + len)`, checked against the arena and the
    /// mapping length. Callers copy through it or wrap it in a `LeaseSpan`.
    pub(crate) fn arena_ptr(
        &self,
        layout: &MappingLayout,
        offset: usize,
        len: usize,
    ) -> Result<*mut u8, MappingError> {
        let end = offset
            .checked_add(len)
            .ok_or(MappingError::ArithmeticOverflow)?;
        if end > layout.arena_bytes {
            return Err(MappingError::InvalidLayout);
        }
        let start = layout
            .arena
            .checked_add(offset)
            .ok_or(MappingError::ArithmeticOverflow)?;
        if start
            .checked_add(len)
            .ok_or(MappingError::ArithmeticOverflow)?
            > self.len
        {
            return Err(MappingError::InvalidLayout);
        }
        // SAFETY: `start + len <= self.len`, so the pointer stays inside the mapping.
        Ok(unsafe { self.base.as_ptr().add(start) })
    }
}

impl fmt::Debug for Mapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Mapping(<redacted>)")
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: base and len came from one successful mmap and are unmapped once here, in
        // Drop, after every borrow of the mapping has ended.
        let _ = unsafe { sys::munmap(self.base, self.len) };
    }
}

fn validate_object(fd: &OwnedFd, expected_len: usize) -> Result<(), MappingError> {
    let stat = sys::fstat(fd.as_fd()).map_err(|_| MappingError::ObjectValidationFailed)?;
    let current_uid = sys::geteuid();
    let type_valid = stat.mode & libc::S_IFMT == libc::S_IFREG;
    if stat.uid != current_uid
        || stat.size < 0
        || stat.size as usize != expected_len
        || !type_valid
        || stat.mode & 0o7777 != 0o600
    {
        return Err(MappingError::ObjectValidationFailed);
    }
    Ok(())
}

fn validate_seals(fd: &OwnedFd) -> Result<(), MappingError> {
    let seals = sys::get_seals(fd.as_fd()).map_err(|_| MappingError::ObjectValidationFailed)?;
    if seals & sys::RING_SEALS != sys::RING_SEALS {
        return Err(MappingError::ObjectValidationFailed);
    }
    Ok(())
}

fn create_linux_memfd(len: usize) -> Result<OwnedFd, MappingError> {
    let fd = sys::memfd_create(c"shm-transport").map_err(|_| MappingError::ObjectSetupFailed)?;
    let len = libc::off_t::try_from(len).map_err(|_| MappingError::ArithmeticOverflow)?;
    sys::ftruncate(fd.as_fd(), len)
        .and_then(|()| sys::fchmod(fd.as_fd(), 0o600))
        .map_err(|_| MappingError::ObjectSetupFailed)?;
    Ok(fd)
}

pub(crate) fn seal_object(fd: &OwnedFd) -> Result<(), MappingError> {
    sys::add_seals(fd.as_fd(), sys::RING_SEALS).map_err(|_| MappingError::ObjectSetupFailed)
}

pub(crate) fn validate_object_len(fd: &OwnedFd, len: usize) -> Result<(), MappingError> {
    validate_object(fd, len)
}

/// Retained backing of one direction, shared by the `Ring` and every owned lease.
///
/// The mapping is unmapped and the backing charge is settled only when the last holder drops,
/// so an endpoint exit never unmaps a block a reader still holds. Every access through the
/// mapping is a relaxed or ordered atomic, or a copy through `copy_in`/`copy_out`; no Rust
/// reference over arena bytes exists in this process.
pub struct Retained {
    mapping: Mapping,
    layout: MappingLayout,
    geometry: PoolGeometry,
    incarnation: Incarnation,
    lane: u32,
    /// Local end of the data doorbell.
    data_signal: UnixStream,
    /// Local end of the capacity doorbell. Each end is its own open file description and a
    /// `UnixStream` may be used from any thread, so a worker's final drop can ring it.
    capacity_signal: UnixStream,
    /// Wake tokens sent from this backing, for diagnostics.
    wake_signals: AtomicU64,
    /// Receiver record per block: the generation currently leased, or zero.
    live: Box<[AtomicU64]>,
    /// Receiver record per block: the greatest generation ever accepted.
    last_seen: Box<[AtomicU64]>,
    /// Live leases across every block, for diagnostics.
    outstanding_returns: AtomicU64,
    /// Admission charge for the backing, set once by the endpoint owner and settled on the
    /// last drop.
    charge: OnceLock<Arc<BackingAdmission>>,
}

// SAFETY: the raw mapping pointer is only ever dereferenced through fixed-width atomics or
// the `copy_in`/`copy_out` atomic byte copies, both of which tolerate concurrent access from
// any thread and from the peer process; `UnixStream` is `Sync`; every other field is an atomic
// or a `OnceLock`. No method hands out a `&mut` into the mapping.
unsafe impl Send for Retained {}
// SAFETY: see the `Send` justification; shared access performs only atomic operations.
unsafe impl Sync for Retained {}

impl Retained {
    pub(crate) fn new(
        mapping: Mapping,
        layout: MappingLayout,
        geometry: PoolGeometry,
        incarnation: Incarnation,
        lane: u32,
        data_signal: UnixStream,
        capacity_signal: UnixStream,
    ) -> Self {
        let blocks = geometry.block_count() as usize;
        Self {
            mapping,
            layout,
            geometry,
            incarnation,
            lane,
            data_signal,
            capacity_signal,
            wake_signals: AtomicU64::new(0),
            live: (0..blocks).map(|_| AtomicU64::new(0)).collect(),
            last_seen: (0..blocks).map(|_| AtomicU64::new(0)).collect(),
            outstanding_returns: AtomicU64::new(0),
            charge: OnceLock::new(),
        }
    }

    pub(crate) const fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// Region offsets of the mapping.
    pub fn layout(&self) -> &MappingLayout {
        &self.layout
    }

    /// Geometry both peers agreed on.
    pub const fn geometry(&self) -> &PoolGeometry {
        &self.geometry
    }

    /// Pool incarnation.
    pub const fn incarnation(&self) -> Incarnation {
        self.incarnation
    }

    /// Direction lane.
    pub const fn lane(&self) -> u32 {
        self.lane
    }

    pub(crate) const fn data_signal(&self) -> &UnixStream {
        &self.data_signal
    }

    pub(crate) const fn capacity_signal(&self) -> &UnixStream {
        &self.capacity_signal
    }

    /// Attaches the admission charge for this backing. Only the first call takes effect.
    pub fn retain_charge(&self, charge: Arc<BackingAdmission>) {
        let _ = self.charge.set(charge);
    }

    /// Wake tokens this backing has sent.
    pub fn wake_signals(&self) -> u64 {
        self.wake_signals.load(Ordering::Relaxed)
    }

    /// Leases live right now across every block.
    pub fn outstanding_returns(&self) -> u64 {
        self.outstanding_returns.load(Ordering::Relaxed)
    }

    pub(crate) fn producer(&self) -> Result<&ProducerPage, MappingError> {
        // SAFETY: `ProducerPage` has no implicit padding; every byte is atomic or inside an
        // `UnsafeCell`.
        unsafe { self.mapping.shared_page(self.layout.producer) }
    }

    pub(crate) fn consumer(&self) -> Result<&ConsumerPage, MappingError> {
        // SAFETY: `ConsumerPage` has no implicit padding; every byte is atomic or inside an
        // `UnsafeCell`.
        unsafe { self.mapping.shared_page(self.layout.consumer) }
    }

    pub(crate) fn data_wake(&self) -> Result<&WakeEpoch, MappingError> {
        // SAFETY: `WakeEpoch` has no implicit padding; every byte is atomic or inside an
        // `UnsafeCell`.
        unsafe { self.mapping.shared_page(self.layout.data_wake) }
    }

    pub(crate) fn capacity_wake(&self) -> Result<&WakeEpoch, MappingError> {
        // SAFETY: `WakeEpoch` has no implicit padding; every byte is atomic or inside an
        // `UnsafeCell`.
        unsafe { self.mapping.shared_page(self.layout.capacity_wake) }
    }

    /// Slot at ring index `index` (not sequence); the caller reduces modulo depth.
    pub(crate) fn slot(&self, index: usize) -> Result<&DescriptorSlot, MappingError> {
        let offset = self
            .layout
            .descriptor_offset(index)
            .ok_or(MappingError::InvalidLayout)?;
        // SAFETY: `DescriptorSlot` has no implicit padding; every byte is atomic or inside an
        // `UnsafeCell`.
        unsafe { self.mapping.shared_page(offset) }
    }

    /// Completion cell of block `block`.
    pub(crate) fn completion(&self, block: u32) -> Result<&CompletionCell, MappingError> {
        let offset = self
            .layout
            .completion_offset(block as usize)
            .ok_or(MappingError::InvalidLayout)?;
        // SAFETY: `CompletionCell` is one atomic with no padding.
        unsafe { self.mapping.shared_page(offset) }
    }

    /// Return-summary word `word`; bit `b` names block `word * RETURN_WORD_BITS + b`.
    pub(crate) fn return_word(&self, word: usize) -> Result<&AtomicU64, MappingError> {
        let offset = self
            .layout
            .return_offset(word)
            .ok_or(MappingError::InvalidLayout)?;
        // SAFETY: `AtomicU64` is one atomic with no padding.
        unsafe { self.mapping.shared_page(offset) }
    }

    pub(crate) fn lifecycle_snapshot(&self) -> Result<LifecycleSnapshot, MappingError> {
        let page = self
            .mapping
            .ptr_at::<LifecyclePage>(self.layout.lifecycle)?;
        // SAFETY: `ptr_at` bounds-checked the page; `addr_of!` projects each field without
        // forming a reference, and every field read is a plain integer or array valid for all
        // bit patterns, so a concurrent peer write yields a wrong value, never UB.
        unsafe {
            use std::ptr::{addr_of, read_volatile};
            Ok(LifecycleSnapshot {
                magic: read_volatile(addr_of!((*page).magic)),
                layout_version: read_volatile(addr_of!((*page).layout_version)),
                lane: read_volatile(addr_of!((*page).lane)),
                incarnation: read_volatile(addr_of!((*page).incarnation)),
                total_bytes: read_volatile(addr_of!((*page).total_bytes)),
                ordinary_descriptors: read_volatile(addr_of!((*page).ordinary_descriptors)),
                reserved_descriptors: read_volatile(addr_of!((*page).reserved_descriptors)),
                class_bytes: read_volatile(addr_of!((*page).class_bytes)),
                class_counts: read_volatile(addr_of!((*page).class_counts)),
            })
        }
    }

    /// The one atomic field of the lifecycle page. Only that field is referenced; the plain
    /// fields around it stay behind the raw pointer.
    pub(crate) fn lifecycle_quarantined(&self) -> Result<&AtomicU8, MappingError> {
        let page = self
            .mapping
            .ptr_at::<LifecyclePage>(self.layout.lifecycle)?;
        // SAFETY: `ptr_at` bounds-checked the page and the mapping outlives `&self`;
        // `addr_of!` projects the field without touching its neighbors, and an `AtomicU8`
        // tolerates concurrent foreign stores through a shared reference.
        Ok(unsafe { &*std::ptr::addr_of!((*page).quarantined) })
    }

    /// Raw pointer to `[offset, offset + len)` of the arena, bounds-checked against the
    /// geometry and mapping.
    pub(crate) fn arena_ptr(&self, offset: usize, len: usize) -> Result<*mut u8, MappingError> {
        self.mapping.arena_ptr(&self.layout, offset, len)
    }

    /// Pointer to the body of block `block`, which starts one header past the block start.
    pub(crate) fn body_ptr(&self, block: u32, body_len: usize) -> Result<*mut u8, MappingError> {
        let placement = self
            .geometry
            .placement(block)
            .ok_or(MappingError::InvalidLayout)?;
        let start = usize::try_from(placement.offset)
            .ok()
            .and_then(|offset| offset.checked_add(WIRE_V3_HEADER_BYTES))
            .ok_or(MappingError::ArithmeticOverflow)?;
        if body_len as u64 > placement.body_capacity() {
            return Err(MappingError::InvalidLayout);
        }
        self.arena_ptr(start, body_len)
    }

    /// Pointer to the header of block `block`.
    pub(crate) fn header_ptr(&self, block: u32) -> Result<*mut u8, MappingError> {
        let placement = self
            .geometry
            .placement(block)
            .ok_or(MappingError::InvalidLayout)?;
        let start =
            usize::try_from(placement.offset).map_err(|_| MappingError::ArithmeticOverflow)?;
        self.arena_ptr(start, WIRE_V3_HEADER_BYTES)
    }

    /// Receiver record for `block`: the generation live now, or zero.
    pub(crate) fn live_generation(&self, block: u32) -> Option<u64> {
        self.live
            .get(block as usize)
            .map(|cell| cell.load(Ordering::Acquire))
    }

    /// Receiver record for `block`: the greatest generation accepted.
    pub(crate) fn last_seen_generation(&self, block: u32) -> Option<u64> {
        self.last_seen
            .get(block as usize)
            .map(|cell| cell.load(Ordering::Relaxed))
    }

    /// Marks `block` live at `generation`. Only the receiving thread calls this, after the
    /// duplicate-live and non-increasing checks passed.
    pub(crate) fn mark_live(&self, block: u32, generation: u64) {
        if let (Some(live), Some(seen)) = (
            self.live.get(block as usize),
            self.last_seen.get(block as usize),
        ) {
            seen.store(generation, Ordering::Relaxed);
            live.store(generation, Ordering::Release);
            self.outstanding_returns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Final return of one payload. Runs on whichever thread drops the last owner, so it
    /// touches only this backing: the local record is finished first, the completion cell is
    /// published with Release, and then the retained capacity doorbell is rung. It never
    /// reaches a `Ring`, allocates, waits for a slot, or mutates a free list.
    pub(crate) fn complete(&self, block: u32, generation: u64) -> Result<(), WakeError> {
        #[cfg(test)]
        crate::lease::observers::enter_final_drop();
        if let Some(live) = self.live.get(block as usize) {
            // The record is retired before publication so a producer that reuses the block
            // and republishes cannot find the old lease still marked live. Only the lease that
            // owns the live mark decrements the outstanding count; a stale lease owns nothing.
            if live
                .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.outstanding_returns.fetch_sub(1, Ordering::Relaxed);
            }
        }
        let published = match self.completion(block) {
            Ok(cell) => {
                // `fetch_max` keeps publication monotonic: a stale return can never lower a
                // newer completion.
                cell.generation.fetch_max(generation, Ordering::Release);
                let word = block as usize / RETURN_WORD_BITS;
                let bit = 1u64 << (block as usize % RETURN_WORD_BITS);
                match self.return_word(word) {
                    Ok(summary) => {
                        summary.fetch_or(bit, Ordering::Release);
                        true
                    }
                    Err(_) => false,
                }
            }
            Err(_) => false,
        };
        let outcome = if published {
            self.signal_capacity()
        } else {
            Err(WakeError::Mapping)
        };
        #[cfg(test)]
        crate::lease::observers::exit_final_drop();
        outcome
    }

    /// Rings the capacity doorbell if the peer is parked on it: bump the wake generation,
    /// clear `parked`, and send one token. `WouldBlock` means a token already waits, which is
    /// the same outcome. Any other error is reported to the caller; publication already
    /// happened and is never rolled back.
    pub(crate) fn signal_capacity(&self) -> Result<(), WakeError> {
        let wake = self.capacity_wake().map_err(|_| WakeError::Mapping)?;
        wake.generation.fetch_add(1, Ordering::SeqCst);
        if wake.parked.swap(0, Ordering::SeqCst) == 0 {
            return Ok(());
        }
        self.send_capacity_token()
    }

    pub(crate) fn send_capacity_token(&self) -> Result<(), WakeError> {
        let token = [1u8];
        loop {
            self.wake_signals.fetch_add(1, Ordering::Relaxed);
            let error = match sys::send_token(self.capacity_signal.as_fd(), &token) {
                Ok(sent) if sent == token.len() => return Ok(()),
                Ok(_) => return Err(WakeError::Doorbell),
                Err(error) => error,
            };
            match error.kind() {
                std::io::ErrorKind::WouldBlock => return Ok(()),
                std::io::ErrorKind::Interrupted => continue,
                _ => return Err(WakeError::Doorbell),
            }
        }
    }
}

impl fmt::Debug for Retained {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Retained(<redacted>)")
    }
}

/// Why a completion could not wake the producer. Publication stands regardless.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WakeError {
    /// The wake epoch or completion cell could not be addressed.
    #[error("retained mapping could not be addressed")]
    Mapping,
    /// The doorbell send failed for a reason other than a pending token.
    #[error("capacity doorbell failed")]
    Doorbell,
}

impl fmt::Debug for WakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

pub(crate) fn initialize_mapping(
    mapping: &Mapping,
    layout: &MappingLayout,
    geometry: &PoolGeometry,
    incarnation: Incarnation,
    lane: u32,
) -> Result<(), MappingError> {
    mapping.initialize_page(
        layout.producer,
        ProducerPage {
            published: AtomicU64::new(0),
            _padding: UnsafeCell::new([0; CONTROL_PAGE_PADDING]),
        },
    )?;
    mapping.initialize_page(
        layout.consumer,
        ConsumerPage {
            consumed: AtomicU64::new(0),
            _padding: UnsafeCell::new([0; CONTROL_PAGE_PADDING]),
        },
    )?;
    for offset in [layout.data_wake, layout.capacity_wake] {
        mapping.initialize_page(
            offset,
            WakeEpoch {
                generation: AtomicU64::new(0),
                parked: AtomicU64::new(0),
                _padding: UnsafeCell::new([0; CACHELINE - 2 * size_of::<AtomicU64>()]),
            },
        )?;
    }
    for index in 0..layout.descriptor_depth {
        mapping.initialize_page(
            layout
                .descriptor_offset(index)
                .ok_or(MappingError::InvalidLayout)?,
            DescriptorSlot {
                sequence: AtomicU64::new(0),
                block: AtomicU64::new(0),
                generation: AtomicU64::new(0),
                body_len: AtomicU64::new(0),
                _padding: UnsafeCell::new([0; DESCRIPTOR_SLOT_BYTES - 4 * size_of::<AtomicU64>()]),
            },
        )?;
    }
    for block in 0..layout.block_count {
        mapping.initialize_page(
            layout
                .completion_offset(block)
                .ok_or(MappingError::InvalidLayout)?,
            CompletionCell {
                generation: AtomicU64::new(0),
            },
        )?;
    }
    for word in 0..layout.return_words {
        mapping.initialize_page(
            layout
                .return_offset(word)
                .ok_or(MappingError::InvalidLayout)?,
            AtomicU64::new(0),
        )?;
    }
    let mut class_bytes = [0u64; CLASS_COUNT];
    let mut class_counts = [0u32; CLASS_COUNT];
    for (index, class) in geometry.classes().iter().enumerate() {
        class_bytes[index] = class.block_bytes;
        class_counts[index] = class.count;
    }
    mapping.initialize_page(
        layout.lifecycle,
        LifecyclePage {
            magic: MAPPING_MAGIC,
            layout_version: LAYOUT_VERSION,
            lane,
            incarnation: incarnation.into_bytes(),
            total_bytes: layout.total as u64,
            ordinary_descriptors: geometry.ordinary_descriptors(),
            reserved_descriptors: geometry.reserved_descriptors(),
            class_bytes,
            class_counts,
            quarantined: AtomicU8::new(0),
        },
    )
}

/// Compares the lifecycle page against the grant; any disagreement refuses attachment.
pub(crate) fn validate_lifecycle(
    snapshot: LifecycleSnapshot,
    geometry: &PoolGeometry,
    incarnation: Incarnation,
    lane: u32,
    total_bytes: usize,
) -> bool {
    let mut class_bytes = [0u64; CLASS_COUNT];
    let mut class_counts = [0u32; CLASS_COUNT];
    for (index, class) in geometry.classes().iter().enumerate() {
        class_bytes[index] = class.block_bytes;
        class_counts[index] = class.count;
    }
    snapshot.magic == MAPPING_MAGIC
        && snapshot.layout_version == LAYOUT_VERSION
        && snapshot.lane == lane
        && snapshot.incarnation == incarnation.into_bytes()
        && snapshot.total_bytes == total_bytes as u64
        && snapshot.ordinary_descriptors == geometry.ordinary_descriptors()
        && snapshot.reserved_descriptors == geometry.reserved_descriptors()
        && snapshot.class_bytes == class_bytes
        && snapshot.class_counts == class_counts
}
