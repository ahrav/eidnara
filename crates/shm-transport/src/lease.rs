use std::fmt;
use std::marker::PhantomData;
use std::mem::size_of;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::descriptor::ReleaseIdentity;

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
    /// while the span exists. Every access through the span is a relaxed atomic load of the
    /// width `AccessShape` assigns to that byte's absolute address; a party storing into the
    /// same bytes while the span reads them must use the same shape (`copy_in` does), since a
    /// racing access of another width is a mixed-size data race.
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

    use super::{
        AccessShape, LeaseError, LeaseSpan, ReceiveLease, ReleaseSink, WORD, copy_in, copy_out,
    };
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
