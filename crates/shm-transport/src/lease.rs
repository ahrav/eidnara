use std::fmt;
use std::marker::PhantomData;
use std::mem::size_of;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::backend::retained::Retained;
use crate::descriptor::PayloadIdentity;

/// Raw view of one arena span, readable for `'lease`. The peer can still write the mapping,
/// so there is no `&[u8]` accessor: reads go through `read_byte`, `copy_to`, or `checksum`,
/// each of which loads through relaxed atomics whose width `AccessShape` fixes per byte, so a
/// concurrent store of the same shape is a stale value, not a data race.
#[derive(Clone, Copy)]
pub struct LeaseSpan<'lease> {
    base: NonNull<u8>,
    len: usize,
    _lifetime: PhantomData<&'lease [u8]>,
    _not_send: PhantomData<Rc<()>>,
}

impl<'lease> LeaseSpan<'lease> {
    /// Wraps `len` bytes at `base`. Fails only on a null `base`.
    ///
    /// # Safety
    /// `base..base.add(len)` must remain mapped and valid for reads and writes for `'lease`
    /// (`AtomicU8::from_ptr` and `AtomicU64::from_ptr` require write validity even for a
    /// load), and this process must not form a `&[u8]` or `&mut [u8]` over any of those bytes
    /// while the span exists. Every access through the span is a relaxed atomic load. A
    /// concurrent writer must use the span's exact base and length, as `copy_in` does,
    /// because `AccessShape` derives access widths from range boundaries: a shifted
    /// overlapping range assigns another width to shared bytes, a mixed-size data race.
    /// Concurrent accesses must be disjoint or share identical boundaries.
    pub(crate) unsafe fn new(base: *mut u8, len: usize) -> Result<Self, LeaseError> {
        let base = NonNull::new(base).ok_or(LeaseError::InvalidSpan)?;
        Ok(Self {
            base,
            len,
            _lifetime: PhantomData,
            _not_send: PhantomData,
        })
    }

    /// Byte length.
    pub const fn len(self) -> usize {
        self.len
    }

    /// The returned pointer supports in-place writes to a producer span; non-atomic stores
    /// require exclusive ownership before `commit`, since a store racing an atomic lease
    /// load is a data race, and a shared span takes `AccessShape` atomics only. Do not form
    /// a long-lived slice from it; the peer may write the same bytes.
    pub const fn as_mut_ptr(self) -> *mut u8 {
        self.base.as_ptr()
    }

    /// Whether `len` is zero.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Reads the byte at `index` atomically, or `None` when `index >= len`.
    pub fn read_byte(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None;
        }
        let shape = AccessShape::of(self.base.as_ptr(), self.len);
        // SAFETY: `index < self.len`; the constructor's contract keeps `len` bytes valid for
        // reads and writes throughout `'lease`, and no Rust reference is formed over them. A
        // word returned by `word_containing` starts at an 8-aligned address and lies entirely
        // within the span, so the `AtomicU64` view is aligned and in bounds.
        Some(unsafe {
            match shape.word_containing(index) {
                Some(word_start) => {
                    let word = AtomicU64::from_ptr(self.base.as_ptr().add(word_start).cast())
                        .load(Ordering::Relaxed);
                    word.to_ne_bytes()[index - word_start]
                }
                None => AtomicU8::from_ptr(self.base.as_ptr().add(index)).load(Ordering::Relaxed),
            }
        })
    }

    /// Copies every byte into `destination`, which must be exactly `len` long.
    pub fn copy_to(self, destination: &mut [u8]) -> Result<(), LeaseError> {
        if destination.len() != self.len {
            return Err(LeaseError::LengthMismatch);
        }
        // SAFETY: the constructor's contract keeps `len` source bytes valid for reads and
        // writes throughout `'lease` with no Rust reference over them.
        unsafe { copy_out(self.base.as_ptr(), destination) };
        Ok(())
    }

    /// Wrapping sum of all bytes. Tests compare it before and after a lease to detect mutation.
    pub fn checksum(self) -> u64 {
        // The peer may write these bytes at any time, so no `&[u8]` is ever formed over them.
        let shape = AccessShape::of(self.base.as_ptr(), self.len);
        let mut sum = 0u64;
        // SAFETY: `shape` partitions exactly `[0, self.len)`, which the constructor's contract
        // keeps valid for reads and writes throughout `'lease` with no Rust reference over it;
        // each word starts at an 8-aligned address and lies entirely within the span.
        unsafe {
            for offset in 0..shape.head {
                let byte =
                    AtomicU8::from_ptr(self.base.as_ptr().add(offset)).load(Ordering::Relaxed);
                sum = sum.wrapping_add(u64::from(byte));
            }
            for word_start in shape.word_starts() {
                let word = AtomicU64::from_ptr(self.base.as_ptr().add(word_start).cast())
                    .load(Ordering::Relaxed);
                for byte in word.to_ne_bytes() {
                    sum = sum.wrapping_add(u64::from(byte));
                }
            }
            for offset in shape.tail_range() {
                let byte =
                    AtomicU8::from_ptr(self.base.as_ptr().add(offset)).load(Ordering::Relaxed);
                sum = sum.wrapping_add(u64::from(byte));
            }
        }
        sum
    }
}

impl fmt::Debug for LeaseSpan<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseSpan(<redacted>)")
    }
}

const WORD: usize = size_of::<u64>();

/// Per-byte atomic access width for a shared-memory range.
///
/// `AccessShape` depends on the absolute address and length alone, so two parties touching the
/// same range agree on the width of every byte. Racing relaxed atomics of equal width read
/// stale data; unequal widths are a mixed-size data race.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AccessShape {
    head: usize,
    words: usize,
    len: usize,
}

impl AccessShape {
    fn of(base: *const u8, len: usize) -> Self {
        let misalignment = base.addr() % WORD;
        let head = if misalignment == 0 {
            0
        } else {
            (WORD - misalignment).min(len)
        };
        let words = (len - head) / WORD;
        Self { head, words, len }
    }

    fn word_starts(self) -> impl Iterator<Item = usize> {
        (0..self.words).map(move |word| self.head + word * WORD)
    }

    fn tail_range(self) -> std::ops::Range<usize> {
        self.head + self.words * WORD..self.len
    }

    /// Start offset of the aligned word containing `index`, or `None` when `index` is outside
    /// the word region.
    fn word_containing(self, index: usize) -> Option<usize> {
        if index < self.head || index >= self.head + self.words * WORD {
            return None;
        }
        Some(self.head + (index - self.head) / WORD * WORD)
    }
}

/// Copies `destination.len()` bytes out of shared memory at `source` through relaxed atomic
/// loads of the width `AccessShape` assigns to each byte, so a same-shape store during the copy
/// yields stale bytes rather than a data race. The destination is ordinary Rust memory and
/// takes plain stores.
///
/// # Safety
/// `source..source.add(destination.len())` must be valid for reads and writes for the duration
/// of the call (`AtomicU8::from_ptr` and `AtomicU64::from_ptr` require both) and must not
/// overlap `destination`, and no Rust reference may cover that source range while the call
/// runs.
pub(crate) unsafe fn copy_out(source: *mut u8, destination: &mut [u8]) {
    let shape = AccessShape::of(source, destination.len());
    let (head, rest) = destination.split_at_mut(shape.head);
    let (words, tail) = rest.split_at_mut(shape.words * WORD);
    // SAFETY: `shape` partitions exactly `[0, destination.len())`, which the caller keeps
    // valid for reads and writes with no Rust reference over it; each word starts at an
    // 8-aligned address and lies entirely within the range, so every `from_ptr` is aligned and
    // in bounds.
    unsafe {
        for (offset, byte) in head.iter_mut().enumerate() {
            *byte = AtomicU8::from_ptr(source.add(offset)).load(Ordering::Relaxed);
        }
        for (word_start, chunk) in shape.word_starts().zip(words.as_chunks_mut::<WORD>().0) {
            let word = AtomicU64::from_ptr(source.add(word_start).cast()).load(Ordering::Relaxed);
            *chunk = word.to_ne_bytes();
        }
        for (offset, byte) in shape.tail_range().zip(tail.iter_mut()) {
            *byte = AtomicU8::from_ptr(source.add(offset)).load(Ordering::Relaxed);
        }
    }
}

/// Copies `source` into shared memory at `destination` through relaxed atomic stores of the
/// width `AccessShape` assigns to each byte, so a same-shape load during the copy observes
/// whole bytes. The source is ordinary Rust memory and takes plain loads.
///
/// # Safety
/// `destination..destination.add(source.len())` must be valid for reads and writes for the
/// duration of the call and must not overlap `source`, and no Rust reference may cover that
/// destination range while the call runs.
pub(crate) unsafe fn copy_in(source: &[u8], destination: *mut u8) {
    let shape = AccessShape::of(destination, source.len());
    let (head, rest) = source.split_at(shape.head);
    let (words, tail) = rest.split_at(shape.words * WORD);
    // SAFETY: `shape` partitions exactly `[0, source.len())`, which the caller keeps valid for
    // reads and writes with no Rust reference over it; each word starts at an 8-aligned address
    // and lies entirely within the range, so every `from_ptr` is aligned and in bounds.
    unsafe {
        for (offset, byte) in head.iter().enumerate() {
            AtomicU8::from_ptr(destination.add(offset)).store(*byte, Ordering::Relaxed);
        }
        for (word_start, chunk) in shape.word_starts().zip(words.as_chunks::<WORD>().0) {
            AtomicU64::from_ptr(destination.add(word_start).cast())
                .store(u64::from_ne_bytes(*chunk), Ordering::Relaxed);
        }
        for (offset, byte) in shape.tail_range().zip(tail.iter()) {
            AtomicU8::from_ptr(destination.add(offset)).store(*byte, Ordering::Relaxed);
        }
    }
}

/// One received payload, owned by whoever holds it. The lease keeps its backing mapped, exposes
/// the body as a lexical raw view, and returns the block to the producer exactly once: on
/// `release` or on drop, whichever comes first.
///
/// `PayloadLease` is `Send`: it holds only an `Arc<Retained>` and plain integers, so a
/// runtime worker may take it across the thread boundary and drop it there. The `Ring` that
/// received it is not reachable from the lease, and the body view it hands out is `!Send`, so
/// no Rust slice or endpoint state crosses with it.
pub struct PayloadLease {
    retained: Arc<Retained>,
    block: u32,
    generation: u64,
    body_len: usize,
    wire_header: [u8; crate::descriptor::WIRE_V3_HEADER_BYTES],
    returned: bool,
}

impl PayloadLease {
    /// Wraps a validated receive. The receiver has already marked `block` live at `generation`
    /// in `retained`'s records; this lease owns the single return of that mark.
    pub(crate) fn new(
        retained: Arc<Retained>,
        block: u32,
        generation: u64,
        body_len: usize,
        wire_header: [u8; crate::descriptor::WIRE_V3_HEADER_BYTES],
    ) -> Self {
        Self {
            retained,
            block,
            generation,
            body_len,
            wire_header,
            returned: false,
        }
    }

    /// Body length.
    pub const fn len(&self) -> usize {
        self.body_len
    }

    /// Whether the body is empty.
    pub const fn is_empty(&self) -> bool {
        self.body_len == 0
    }

    /// Header the producer committed with the body, captured and validated at receive time.
    pub const fn wire_header(&self) -> [u8; crate::descriptor::WIRE_V3_HEADER_BYTES] {
        self.wire_header
    }

    /// Identity the return carries.
    pub fn identity(&self) -> PayloadIdentity {
        PayloadIdentity::new(
            self.retained.incarnation(),
            self.retained.lane(),
            self.block,
            self.generation,
        )
    }

    /// The backing this lease keeps alive.
    pub fn retained(&self) -> &Arc<Retained> {
        &self.retained
    }

    /// Raw view of the body, valid while the lease is borrowed. The peer can still write the
    /// mapping, so reads go through atomics; copy before decoding.
    pub fn body(&self) -> Result<LeaseSpan<'_>, LeaseError> {
        let ptr = self
            .retained
            .body_ptr(self.block, self.body_len)
            .map_err(|_| LeaseError::InvalidSpan)?;
        if self.body_len == 0 {
            // A zero-length view still needs a non-null base; the header pointer is inside the
            // block and never dereferenced for a zero length.
            let header = self
                .retained
                .header_ptr(self.block)
                .map_err(|_| LeaseError::InvalidSpan)?;
            // SAFETY: zero bytes are read through the span; the pointer is inside the retained
            // mapping, which lives as long as `self`.
            return unsafe { LeaseSpan::new(header, 0) };
        }
        // SAFETY: `body_ptr` checked `[offset, offset + body_len)` against the block's capacity
        // and the mapping length; the mapping (owned by `retained`) stays mapped for the borrow
        // of `self`. No `&[u8]` over the arena is ever formed in this crate; access goes through
        // atomic reads.
        unsafe { LeaseSpan::new(ptr, self.body_len) }
    }

    /// Copies the body into one `Vec` of `len` bytes.
    pub fn to_vec(&self) -> Result<Vec<u8>, LeaseError> {
        let mut bytes = vec![0u8; self.body_len];
        if self.body_len != 0 {
            self.body()?.copy_to(&mut bytes)?;
        }
        Ok(bytes)
    }

    /// Returns the payload and reports whether the producer could be woken. Drop does the same
    /// but discards the error; either way the block is returned exactly once.
    pub fn release(mut self) -> Result<(), LeaseError> {
        self.return_once()
    }

    fn return_once(&mut self) -> Result<(), LeaseError> {
        if self.returned {
            return Err(LeaseError::DuplicateRelease);
        }
        // `returned` is set before the completion so `Drop` cannot publish twice.
        self.returned = true;
        self.retained
            .complete(self.block, self.generation)
            .map_err(|_| LeaseError::WakeFailed)
    }
}

impl fmt::Debug for PayloadLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PayloadLease(<redacted>)")
    }
}

impl Drop for PayloadLease {
    fn drop(&mut self) {
        if !self.returned {
            let _ = self.return_once();
        }
    }
}

/// Test-only observers for the three operations a final drop must never perform: a `Ring`
/// call, a queue-slot wait, and a free-list mutation. `enter_final_drop` marks the window;
/// each forbidden entry point calls its observer, which records a violation when the window
/// is open. Completion-node allocation and the N-API boundary have no code point in this
/// crate: the pool publishes into a fixed cell, and the boundary lives in the native addon.
#[cfg(test)]
pub mod observers {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    thread_local! {
        static IN_FINAL_DROP: Cell<u32> = const { Cell::new(0) };
    }

    /// `Ring` entry points reached inside a final drop.
    pub static RING_CALL: AtomicU64 = AtomicU64::new(0);
    /// Descriptor-slot parks reached inside a final drop.
    pub static SLOT_WAIT: AtomicU64 = AtomicU64::new(0);
    /// Free-list pushes or pops reached inside a final drop.
    pub static FREE_LIST_MUTATION: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn enter_final_drop() {
        IN_FINAL_DROP.with(|depth| depth.set(depth.get() + 1));
    }

    pub(crate) fn exit_final_drop() {
        IN_FINAL_DROP.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }

    /// Whether the calling thread is inside a final drop.
    pub fn in_final_drop() -> bool {
        IN_FINAL_DROP.with(|depth| depth.get() != 0)
    }

    fn observe(counter: &AtomicU64) {
        if in_final_drop() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Called at every `Ring` entry point.
    pub fn ring_call() {
        observe(&RING_CALL);
    }

    /// Called before parking for a descriptor slot.
    pub fn slot_wait() {
        observe(&SLOT_WAIT);
    }

    /// Called at every free-list push or pop.
    pub fn free_list_mutation() {
        observe(&FREE_LIST_MUTATION);
    }

    /// Every counter, in the order above.
    pub fn snapshot() -> [u64; 3] {
        [
            RING_CALL.load(Ordering::Relaxed),
            SLOT_WAIT.load(Ordering::Relaxed),
            FREE_LIST_MUTATION.load(Ordering::Relaxed),
        ]
    }
}

/// Why a span, lease, or return was refused.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// Null span pointer, or a body that does not fit its block.
    #[error("receive span is invalid")]
    InvalidSpan,
    /// A destination length disagrees with the span length.
    #[error("receive span lengths disagree")]
    LengthMismatch,
    /// The same payload was returned twice.
    #[error("release is duplicated")]
    DuplicateRelease,
    /// The completion was published but the capacity doorbell could not be rung. The block is
    /// returned regardless; only the wake is lost.
    #[error("payload completion could not wake the producer")]
    WakeFailed,
}

impl fmt::Debug for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// A tiny pool over an anonymous mapping, so Miri witnesses run without a memfd. Nothing is
/// parked on the capacity doorbell, so no token is ever sent.
#[cfg(test)]
pub(crate) fn retained_fixture() -> Arc<Retained> {
    use crate::backend::retained::{Mapping, initialize_mapping};
    use crate::descriptor::Incarnation;
    use crate::pool::{ClassSpec, MappingLayout, PoolGeometry};

    let geometry = PoolGeometry::new(
        2,
        1,
        [
            ClassSpec::new(4096, 2),
            ClassSpec::new(8192, 1),
            ClassSpec::new(12288, 1),
            ClassSpec::new(16384, 1),
            ClassSpec::new(20480, 1),
        ],
        ClassSpec::new(4096, 1),
        ClassSpec::new(8192, 1),
    )
    .unwrap();
    let layout = MappingLayout::new(&geometry, 4096).unwrap();
    let mapping = Mapping::anonymous(layout.total).unwrap();
    let incarnation = Incarnation::from_bytes([7; 16]);
    initialize_mapping(&mapping, &layout, &geometry, incarnation, 3).unwrap();
    let (data, _data_peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let (capacity, _capacity_peer) = std::os::unix::net::UnixStream::pair().unwrap();
    // The peer ends are dropped so no reader exists; `send` is never reached because `parked`
    // stays zero in these tests.
    Arc::new(Retained::new(
        mapping,
        layout,
        geometry,
        incarnation,
        3,
        data,
        capacity,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        AccessShape, LeaseError, LeaseSpan, PayloadLease, WORD, copy_in, copy_out, observers,
    };
    use crate::backend::retained::Retained;

    pub(crate) use super::retained_fixture;

    fn lease_over(
        retained: &Arc<Retained>,
        block: u32,
        generation: u64,
        body: &[u8],
    ) -> PayloadLease {
        let ptr = retained.body_ptr(block, body.len()).unwrap();
        // SAFETY: the pointer is inside the retained anonymous mapping, which no reference
        // covers; the atomic copy is the only access.
        unsafe { copy_in(body, ptr) };
        retained.mark_live(block, generation);
        let mut header = [0u8; crate::descriptor::WIRE_V3_HEADER_BYTES];
        header[..4].copy_from_slice(&(body.len() as u32).to_le_bytes());
        header[4] = crate::descriptor::WIRE_V3_VERSION;
        PayloadLease::new(Arc::clone(retained), block, generation, body.len(), header)
    }

    fn completion(retained: &Retained, block: u32) -> u64 {
        retained
            .completion(block)
            .unwrap()
            .generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    const _: () = {
        fn assert_send<T: Send>() {}
        let _ = assert_send::<PayloadLease>;
    };

    #[test]
    fn owned_lease_exposes_exact_bytes_and_returns_exactly_once() {
        let retained = retained_fixture();
        let body = [0xabu8, 0xcd, 0xef, 0x01, 0x23];
        let lease = lease_over(&retained, 1, 5, &body);
        assert_eq!(lease.len(), 5);
        assert_eq!(lease.to_vec().unwrap(), body);
        assert_eq!(lease.body().unwrap().read_byte(4), Some(0x23));
        assert_eq!(lease.body().unwrap().read_byte(5), None);
        assert_eq!(lease.identity().block(), 1);
        assert_eq!(lease.identity().generation(), 5);
        assert_eq!(retained.live_generation(1), Some(5));
        assert_eq!(retained.outstanding_returns(), 1);
        assert_eq!(completion(&retained, 1), 0);
        lease.release().unwrap();
        assert_eq!(retained.live_generation(1), Some(0));
        assert_eq!(retained.outstanding_returns(), 0);
        assert_eq!(
            completion(&retained, 1),
            5,
            "the captured generation is published once"
        );
        assert_eq!(
            Arc::strong_count(&retained),
            1,
            "the lease released its backing"
        );
    }

    #[test]
    fn owned_lease_drop_returns_once_after_moving_to_another_thread() {
        let retained = retained_fixture();
        let lease = lease_over(&retained, 2, 9, b"moved");
        let worker = std::thread::spawn(move || {
            let copied = lease.to_vec().unwrap();
            drop(lease);
            copied
        });
        assert_eq!(worker.join().unwrap(), b"moved");
        assert_eq!(completion(&retained, 2), 9);
        assert_eq!(retained.live_generation(2), Some(0));
        assert_eq!(retained.outstanding_returns(), 0);
    }

    #[test]
    fn stale_completion_cannot_lower_a_newer_one() {
        let retained = retained_fixture();
        let newer = lease_over(&retained, 0, 12, b"new");
        newer.release().unwrap();
        assert_eq!(completion(&retained, 0), 12);
        // A late return from an earlier generation of the same block publishes nothing lower.
        let stale = PayloadLease::new(
            Arc::clone(&retained),
            0,
            11,
            0,
            [0; crate::descriptor::WIRE_V3_HEADER_BYTES],
        );
        drop(stale);
        assert_eq!(completion(&retained, 0), 12);
    }

    #[test]
    fn backing_outlives_the_last_endpoint_handle_until_the_lease_returns() {
        let retained = retained_fixture();
        let lease = lease_over(&retained, 3, 1, b"held");
        let weak = Arc::downgrade(&retained);
        drop(retained);
        assert!(
            weak.upgrade().is_some(),
            "the lease keeps the mapping alive"
        );
        assert_eq!(lease.to_vec().unwrap(), b"held");
        drop(lease);
        assert!(weak.upgrade().is_none(), "the last owner unmaps");
    }

    #[test]
    fn final_drop_reaches_no_forbidden_operation() {
        let retained = retained_fixture();
        let before = observers::snapshot();
        let lease = lease_over(&retained, 4, 2, b"quiet");
        drop(lease);
        assert_eq!(observers::snapshot(), before);
        assert!(!observers::in_final_drop());
    }

    #[test]
    fn copy_in_then_copy_out_round_trips_at_every_alignment_and_length() {
        use std::sync::atomic::AtomicU8;
        let source: Vec<u8> = (0..64u8).collect();
        for start in 0..16 {
            for shared_shift in 0..16 {
                for len in 0..40 {
                    let shared: Vec<AtomicU8> = (0..len + shared_shift)
                        .map(|_| AtomicU8::new(0xff))
                        .collect();
                    let shared_ptr = shared.as_ptr().cast_mut().cast::<u8>();
                    let mut back = vec![0u8; len];
                    // SAFETY: `shared` is live for both calls and no `&[u8]` or `&mut [u8]` is
                    // formed over it; the atomics are the only accesses.
                    unsafe {
                        copy_in(&source[start..start + len], shared_ptr.add(shared_shift));
                        copy_out(shared_ptr.add(shared_shift), &mut back);
                    }
                    assert_eq!(
                        back,
                        source[start..start + len],
                        "start {start} shift {shared_shift} len {len}"
                    );
                    assert!(
                        shared[..shared_shift]
                            .iter()
                            .all(|cell| cell.load(std::sync::atomic::Ordering::Relaxed) == 0xff),
                        "bytes before the shift are untouched"
                    );
                }
            }
        }
    }

    #[test]
    fn access_shape_partitions_the_range_on_aligned_words() {
        let buffer = [0u64; 8];
        let base = buffer.as_ptr().cast::<u8>();
        for shift in 0..8 {
            for len in 0..48 {
                let range = base.wrapping_add(shift);
                let shape = AccessShape::of(range, len);
                assert_eq!(
                    shape.head + shape.words * WORD + shape.tail_range().len(),
                    len,
                    "shift {shift} len {len}"
                );
                assert!(shape.head <= len && shape.head < WORD);
                for word_start in shape.word_starts() {
                    assert_eq!(range.wrapping_add(word_start).addr() % WORD, 0);
                    assert!(word_start + WORD <= len);
                }
                for index in 0..len {
                    let word = shape.word_containing(index);
                    let in_words = index >= shape.head && index < shape.head + shape.words * WORD;
                    assert_eq!(
                        word.is_some(),
                        in_words,
                        "shift {shift} len {len} index {index}"
                    );
                    if let Some(word_start) = word {
                        assert!(word_start <= index && index < word_start + WORD);
                    }
                }
            }
        }
    }

    #[test]
    fn read_byte_agrees_with_copy_to_at_every_alignment() {
        use std::sync::atomic::AtomicU64;
        let shared: [AtomicU64; 8] = std::array::from_fn(|_| AtomicU64::new(0));
        let base = shared.as_ptr().cast_mut().cast::<u8>();
        let pattern: Vec<u8> = (0..64u8).map(|byte| byte.wrapping_mul(37)).collect();
        for shift in 0..8 {
            let len = 64 - shift;
            // SAFETY: `shared` is live for both calls and no `&[u8]` or `&mut [u8]` is formed
            // over it; the atomics are the only accesses.
            unsafe {
                copy_in(&pattern[..len], base.add(shift));
                let span = LeaseSpan::new(base.add(shift), len).unwrap();
                let mut copied = vec![0u8; len];
                span.copy_to(&mut copied).unwrap();
                assert_eq!(copied, pattern[..len]);
                for (index, expected) in pattern[..len].iter().enumerate() {
                    assert_eq!(span.read_byte(index), Some(*expected), "shift {shift}");
                }
                assert_eq!(
                    span.checksum(),
                    pattern[..len]
                        .iter()
                        .map(|byte| u64::from(*byte))
                        .sum::<u64>()
                );
                assert_eq!(span.read_byte(len), None);
            }
        }
    }

    #[test]
    fn span_null_base_is_refused() {
        // SAFETY: a null pointer with zero length names no memory.
        let refused = unsafe { LeaseSpan::new(std::ptr::null_mut(), 0) };
        assert_eq!(refused.err(), Some(LeaseError::InvalidSpan));
    }

    #[test]
    fn span_reads_tolerate_a_concurrent_writer() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        const SHIFT: usize = 3;
        const LEN: usize = 62;
        let rounds = if cfg!(miri) { 4 } else { 64 };
        let shared: [AtomicU64; 9] = std::array::from_fn(|_| AtomicU64::new(0x1111_1111_1111_1111));
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let base = shared.as_ptr().cast_mut().cast::<u8>();
                let mut value = 0x11u8;
                while !stop.load(Ordering::Relaxed) {
                    value = if value == 0x11 { 0x22 } else { 0x11 };
                    // SAFETY: `shared` is live for both calls and no `&[u8]` or `&mut [u8]` is
                    // formed over it; the atomics are the only accesses.
                    unsafe { copy_in(&[value; LEN], base.add(SHIFT)) };
                }
            });
            let base = shared.as_ptr().cast_mut().cast::<u8>();
            // SAFETY: `shared` is live for both calls and no `&[u8]` or `&mut [u8]` is formed
            // over it; the atomics are the only accesses.
            let span = unsafe { LeaseSpan::new(base.add(SHIFT), LEN) }.unwrap();
            let mut copy = [0u8; LEN];
            for _ in 0..rounds {
                for index in 0..LEN {
                    let byte = span.read_byte(index).unwrap();
                    assert!(byte == 0x11 || byte == 0x22, "torn byte {byte:#x}");
                }
                span.copy_to(&mut copy).unwrap();
                assert!(copy.iter().all(|byte| *byte == 0x11 || *byte == 0x22));
                let sum = span.checksum();
                assert!(
                    sum >= 0x11 * LEN as u64 && sum <= 0x22 * LEN as u64,
                    "{sum}"
                );
            }
            stop.store(true, Ordering::Relaxed);
        });
    }
}
