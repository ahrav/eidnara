use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::descriptor::ReleaseIdentity;

/// Raw view of one arena span, readable for `'lease`. The peer can still write the mapping,
/// so there is no `&[u8]` accessor: reads go through `read_byte`, `copy_to`, or `checksum`,
/// each of which loads every byte atomically so a concurrent peer store is a stale value, not
/// a data race.
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
    /// `base..base.add(len)` must remain mapped and readable for `'lease`, and this process
    /// must not form a `&[u8]` or `&mut [u8]` over any of those bytes while the span exists.
    /// Every access through the span is an atomic byte load; a foreign process writing the
    /// same bytes must use whole-byte stores, which every store instruction does.
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

    /// Base pointer, for callers that write into a producer span in place. Do not form a
    /// long-lived slice from it; the peer may write the same bytes.
    pub const fn as_mut_ptr(self) -> *mut u8 {
        self.base.as_ptr()
    }

    /// Whether `len` is zero.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// One atomic byte read, or `None` past `len`.
    pub fn read_byte(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None;
        }
        // SAFETY: `index < self.len`, and the constructor's contract keeps `len` bytes mapped
        // for `'lease` with no Rust reference formed over them; `AtomicU8` has alignment 1.
        Some(unsafe { AtomicU8::from_ptr(self.base.as_ptr().add(index)) }.load(Ordering::Relaxed))
    }

    /// Copies every byte into `destination`, which must be exactly `len` long.
    pub fn copy_to(self, destination: &mut [u8]) -> Result<(), LeaseError> {
        if destination.len() != self.len {
            return Err(LeaseError::LengthMismatch);
        }
        // SAFETY: the constructor's contract keeps `len` source bytes readable for `'lease`
        // with no Rust reference over them, and `destination` is a live exclusive slice of the
        // same length that cannot overlap the mapping.
        unsafe { copy_out(self.base.as_ptr(), destination) };
        Ok(())
    }

    /// Wrapping sum of all bytes. Tests compare it before and after a lease to detect mutation.
    pub fn checksum(self) -> u64 {
        // The peer may write these bytes at any time, so no `&[u8]` is ever formed over them.
        (0..self.len).fold(0u64, |sum, index| {
            // SAFETY: `index < self.len`; same contract as `read_byte`.
            let byte = unsafe { AtomicU8::from_ptr(self.base.as_ptr().add(index)) }
                .load(Ordering::Relaxed);
            sum.wrapping_add(u64::from(byte))
        })
    }
}

impl fmt::Debug for LeaseSpan<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseSpan(<redacted>)")
    }
}

/// Copies `destination.len()` bytes out of shared memory at `source`, one relaxed atomic load
/// per byte, so a peer store during the copy yields a stale byte rather than a data race. The
/// destination is ordinary Rust memory and takes plain stores.
///
/// # Safety
/// `source..source.add(destination.len())` must be readable for the duration of the call and
/// must not overlap `destination`, and no Rust reference may cover that source range while the
/// call runs.
pub(crate) unsafe fn copy_out(source: *const u8, destination: &mut [u8]) {
    for (offset, slot) in destination.iter_mut().enumerate() {
        // SAFETY: `offset < destination.len()`, so the pointer stays inside the caller's range;
        // `AtomicU8` has alignment 1. The caller vouches that the range is shared memory with no
        // Rust reference over it, which is what `from_ptr` requires.
        *slot =
            unsafe { AtomicU8::from_ptr(source.add(offset).cast_mut()) }.load(Ordering::Relaxed);
    }
}

/// Copies `source` into shared memory at `destination`, one relaxed atomic store per byte, so a
/// peer reading during the copy observes whole bytes. The source is ordinary Rust memory and
/// takes plain loads.
///
/// # Safety
/// `destination..destination.add(source.len())` must be writable for the duration of the call
/// and must not overlap `source`, and no Rust reference may cover that destination range while
/// the call runs.
pub(crate) unsafe fn copy_in(source: &[u8], destination: *mut u8) {
    for (offset, byte) in source.iter().enumerate() {
        // SAFETY: `offset < source.len()`, so the pointer stays inside the caller's range;
        // `AtomicU8` has alignment 1. The caller vouches that the range is shared memory with no
        // Rust reference over it, which is what `from_ptr` requires.
        unsafe { AtomicU8::from_ptr(destination.add(offset)) }.store(*byte, Ordering::Relaxed);
    }
}

/// Receives a frame release from a `ReceiveLease`. The ring that issued the lease implements
/// it; the lease borrows the sink for `'lease`, so a lease cannot outlive its ring.
pub(crate) trait ReleaseSink {
    /// Returns the frame named by `identity` to the producer.
    fn release(&self, identity: ReleaseIdentity) -> Result<(), LeaseError>;
}

/// A published frame the receiver holds. The body is visible through `segment` as one or two
/// raw spans; `to_vec` copies it out. Dropping the lease releases the frame to the producer.
/// `!Send`: the release callback belongs to the ring that issued the lease, and that ring is
/// owned by one thread.
pub struct ReceiveLease<'lease> {
    spans: [Option<LeaseSpan<'lease>>; 2],
    span_count: u8,
    body_len: usize,
    wire_header: [u8; crate::descriptor::WIRE_V2_HEADER_BYTES],
    identity: ReleaseIdentity,
    sink: &'lease dyn ReleaseSink,
    released: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl<'lease> ReceiveLease<'lease> {
    /// Checks that `span_count` agrees with which spans are present. `sink` receives exactly
    /// one `release` for `identity`, on explicit release or on drop.
    pub(crate) fn new(
        spans: [Option<LeaseSpan<'lease>>; 2],
        span_count: u8,
        body_len: usize,
        wire_header: [u8; crate::descriptor::WIRE_V2_HEADER_BYTES],
        identity: ReleaseIdentity,
        sink: &'lease dyn ReleaseSink,
    ) -> Result<Self, LeaseError> {
        if !(1..=2).contains(&span_count)
            || spans[0].is_none()
            || (span_count == 1 && spans[1].is_some())
            || (span_count == 2 && spans[1].is_none())
        {
            return Err(LeaseError::InvalidSpan);
        }
        Ok(Self {
            spans,
            span_count,
            body_len,
            wire_header,
            identity,
            sink,
            released: false,
            _not_send: PhantomData,
        })
    }

    /// Body length across all segments.
    pub const fn len(&self) -> usize {
        self.body_len
    }

    /// Whether `len` is zero.
    pub const fn is_empty(&self) -> bool {
        self.body_len == 0
    }

    /// One segment, or two when the body wraps around the arena end.
    pub const fn segment_count(&self) -> usize {
        self.span_count as usize
    }

    /// The segment at `index`, or `None` past `segment_count`.
    pub fn segment(&self, index: usize) -> Option<LeaseSpan<'_>> {
        if index >= usize::from(self.span_count) {
            return None;
        }
        self.spans[index]
    }

    /// Header the producer committed with the body.
    pub const fn wire_header(&self) -> [u8; crate::descriptor::WIRE_V2_HEADER_BYTES] {
        self.wire_header
    }

    /// Identity the release carries; the ring matches it against the slot.
    pub const fn identity(&self) -> ReleaseIdentity {
        self.identity
    }

    /// Releases the frame and returns the ring's verdict. Drop does the same but discards
    /// the error.
    pub fn release(mut self) -> Result<(), LeaseError> {
        self.release_once()
    }

    /// Copies every segment, in order, into one `Vec` of `len` bytes. Fails if the segment
    /// lengths do not sum to `len`.
    pub fn to_vec(&self) -> Result<Vec<u8>, LeaseError> {
        let mut bytes = vec![0u8; self.body_len];
        let mut cursor = 0usize;
        for index in 0..usize::from(self.span_count) {
            let span = self.spans[index].ok_or(LeaseError::InvalidSpan)?;
            let end = cursor
                .checked_add(span.len())
                .ok_or(LeaseError::LengthMismatch)?;
            let destination = bytes
                .get_mut(cursor..end)
                .ok_or(LeaseError::LengthMismatch)?;
            span.copy_to(destination)?;
            cursor = end;
        }
        if cursor != self.body_len {
            return Err(LeaseError::LengthMismatch);
        }
        Ok(bytes)
    }

    fn release_once(&mut self) -> Result<(), LeaseError> {
        if self.released {
            return Err(LeaseError::DuplicateRelease);
        }
        // `released` is set before the sink call so `Drop` cannot retry a failed release.
        self.released = true;
        self.sink.release(self.identity)
    }
}

impl fmt::Debug for ReceiveLease<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReceiveLease(<redacted>)")
    }
}

impl Drop for ReceiveLease<'_> {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release_once();
        }
    }
}

/// Why a span, lease, or release was refused.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// Null span pointer, or a span count that disagrees with the spans present.
    #[error("receive span is invalid")]
    InvalidSpan,
    /// A destination or segment length disagrees with the span length.
    #[error("receive span lengths disagree")]
    LengthMismatch,
    /// The release identity names a different incarnation.
    #[error("release identity does not match incarnation")]
    WrongIncarnation,
    /// The release identity names a different lane.
    #[error("release identity does not match lane")]
    WrongLane,
    /// The release sequence is zero or not the one leased.
    #[error("release sequence is invalid")]
    InvalidSequence,
    /// The same identity was released twice.
    #[error("release is duplicated")]
    DuplicateRelease,
    /// The transport storage was quarantined; no release can complete.
    #[error("transport storage is quarantined")]
    Quarantined,
}

impl fmt::Debug for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{LeaseError, LeaseSpan, ReceiveLease, ReleaseSink, copy_in, copy_out};
    use crate::descriptor::{Incarnation, ReleaseIdentity};

    struct CallLog {
        calls: Cell<usize>,
        verdict: Result<(), LeaseError>,
    }

    impl ReleaseSink for CallLog {
        fn release(&self, _: ReleaseIdentity) -> Result<(), LeaseError> {
            self.calls.set(self.calls.get() + 1);
            self.verdict
        }
    }

    fn lease<'a>(bytes: &'a mut [u8], log: &'a CallLog) -> ReceiveLease<'a> {
        // SAFETY: `bytes` outlives the returned lease.
        let span = unsafe { LeaseSpan::new(bytes.as_mut_ptr(), bytes.len()) }.unwrap();
        let identity = ReleaseIdentity::new(Incarnation::from_bytes([7; 16]), 0, 1);
        ReceiveLease::new(
            [Some(span), None],
            1,
            bytes.len(),
            [0; crate::descriptor::WIRE_V2_HEADER_BYTES],
            identity,
            log,
        )
        .unwrap()
    }

    #[test]
    fn failed_explicit_release_is_not_retried_by_drop() {
        let mut bytes = [1u8; 4];
        let log = CallLog {
            calls: Cell::new(0),
            verdict: Err(LeaseError::Quarantined),
        };
        let held = lease(&mut bytes, &log);
        assert_eq!(held.release(), Err(LeaseError::Quarantined));
        assert_eq!(
            log.calls.get(),
            1,
            "drop must not call the release callback again"
        );
    }

    #[test]
    fn drop_releases_exactly_once() {
        let mut bytes = [1u8; 4];
        let log = CallLog {
            calls: Cell::new(0),
            verdict: Ok(()),
        };
        drop(lease(&mut bytes, &log));
        assert_eq!(log.calls.get(), 1);
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
    fn span_null_base_is_refused() {
        // SAFETY: a null pointer with zero length names no memory.
        let refused = unsafe { LeaseSpan::new(std::ptr::null_mut(), 0) };
        assert_eq!(refused.err(), Some(LeaseError::InvalidSpan));
    }

    /// A peer may store into the bytes a span is reading at any time. The span's reads are
    /// atomic, so under Miri this is a race-free program and every observed byte is one the
    /// writer stored.
    #[test]
    fn span_reads_tolerate_a_concurrent_writer() {
        use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
        const LEN: usize = 64;
        let rounds = if cfg!(miri) { 4 } else { 64 };
        let shared: [AtomicU8; LEN] = std::array::from_fn(|_| AtomicU8::new(0x11));
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut value = 0x11u8;
                while !stop.load(Ordering::Relaxed) {
                    value = if value == 0x11 { 0x22 } else { 0x11 };
                    for cell in shared.iter() {
                        cell.store(value, Ordering::Relaxed);
                    }
                }
            });
            // SAFETY: `shared` outlives the scope, so the bytes stay mapped for the span's
            // lifetime, and the only accesses are the writer's atomic stores and the span's
            // atomic loads.
            let span =
                unsafe { LeaseSpan::new(shared.as_ptr().cast_mut().cast::<u8>(), LEN) }.unwrap();
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
