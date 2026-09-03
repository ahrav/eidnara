use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::descriptor::ReleaseIdentity;

/// Raw view of one arena span, readable for `'lease`. The peer can still write the mapping,
/// so there is no `&[u8]` accessor: reads go through `read_byte`, `copy_to`, or `checksum`,
/// each of which tolerates concurrent mutation without undefined behavior.
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
    /// `base..base.add(len)` must remain mapped and readable for `'lease`.
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

    /// One volatile byte read, or `None` past `len`.
    pub fn read_byte(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None;
        }
        // SAFETY: constructor bound covers len and index was checked.
        Some(unsafe { self.base.as_ptr().add(index).read_volatile() })
    }

    /// Copies every byte into `destination`, which must be exactly `len` long.
    pub fn copy_to(self, destination: &mut [u8]) -> Result<(), LeaseError> {
        if destination.len() != self.len {
            return Err(LeaseError::LengthMismatch);
        }
        // SAFETY: `LeaseSpan::new` guarantees that the source range is readable.
        unsafe {
            std::ptr::copy_nonoverlapping(self.base.as_ptr(), destination.as_mut_ptr(), self.len)
        };
        Ok(())
    }

    /// Wrapping sum of all bytes. Tests compare it before and after a lease to detect mutation.
    pub fn checksum(self) -> u64 {
        // SAFETY: `LeaseSpan::new` guarantees that the slice range is readable for the lease.
        let bytes = unsafe { std::slice::from_raw_parts(self.base.as_ptr(), self.len) };
        bytes
            .iter()
            .fold(0u64, |sum, byte| sum.wrapping_add(u64::from(*byte)))
    }
}

impl fmt::Debug for LeaseSpan<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseSpan(<redacted>)")
    }
}

pub(crate) type ReleaseFn = unsafe fn(*const (), ReleaseIdentity) -> Result<(), LeaseError>;

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
    release_context: *const (),
    release_fn: ReleaseFn,
    released: bool,
    _owner: PhantomData<&'lease ()>,
    _not_send: PhantomData<Rc<()>>,
}

impl<'lease> ReceiveLease<'lease> {
    /// Checks that `span_count` agrees with which spans are present.
    ///
    /// # Safety
    /// Spans and `release_context` must remain valid for `'lease`, and `release_fn` must
    /// accept `identity` exactly once.
    pub(crate) unsafe fn new(
        spans: [Option<LeaseSpan<'lease>>; 2],
        span_count: u8,
        body_len: usize,
        wire_header: [u8; crate::descriptor::WIRE_V2_HEADER_BYTES],
        identity: ReleaseIdentity,
        release_context: *const (),
        release_fn: ReleaseFn,
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
            release_context,
            release_fn,
            released: false,
            _owner: PhantomData,
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
        // SAFETY: constructor requires a live callback context for lease lifetime.
        unsafe { (self.release_fn)(self.release_context, self.identity)? };
        self.released = true;
        Ok(())
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
