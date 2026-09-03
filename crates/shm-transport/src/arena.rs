use std::fmt;

/// Largest wire-v2 body either peer will publish or admit.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// Smallest arena that can hold one maximum frame; `SpanPlan::reserve` refuses anything
/// smaller so a legal frame can never be unplaceable.
pub const MIN_ARENA_BYTES: usize = MAX_FRAME_BYTES;

/// Failure while planning a FIFO arena reservation.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArenaError {
    /// Arena cannot hold one legal maximum frame.
    #[error("arena capacity is below the protocol minimum")]
    BelowMinimumCapacity,
    /// Requested frame exceeds the wire limit.
    #[error("frame exceeds the protocol maximum")]
    FrameTooLarge,
    /// `prefix` was asked for more bytes than its spans cover; a narrowed plan cannot widen.
    #[error("committed length exceeds the bytes the plan covers")]
    ExceedsAllocation,
    /// Absolute cursors are malformed or wrapped.
    #[error("arena cursor is invalid")]
    InvalidCursor,
    /// Current FIFO hold leaves insufficient contiguous logical capacity.
    #[error("arena capacity is exhausted")]
    Exhausted,
    /// Offset or length arithmetic overflowed.
    #[error("arena arithmetic overflow")]
    ArithmeticOverflow,
}

impl fmt::Debug for ArenaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// One offset-and-length region within an arena.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ArenaSpan {
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

impl ArenaSpan {
    /// Wraps peer-supplied values; `FrameDescriptor::validate` checks them against the arena.
    pub const fn from_untrusted(offset: u64, len: u64) -> Self {
        Self { offset, len }
    }

    /// Byte offset from the arena base, already reduced modulo the arena size.
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Byte length of the span.
    pub const fn len(self) -> u64 {
        self.len
    }

    /// Whether `len` is zero.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

crate::redacted_debug!(ArenaSpan);

/// Where one frame body lives in the arena: one span, or two when the body wraps past the
/// arena end. `reserve` plans the allocation; `prefix` narrows it to the committed length.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SpanPlan {
    allocation_start: u64,
    allocation_len: u64,
    spans: [ArenaSpan; 2],
    span_count: u8,
}

impl SpanPlan {
    /// Plans a reservation of `len` bytes at cursor `write`. `write` and `reclaimed` are
    /// monotonic byte counts; `write - reclaimed` is the number of bytes still held, so the
    /// reservation fits only if `len <= capacity - held`. Wrapping is decided here: the second
    /// span is non-empty exactly when `write % capacity + len > capacity`.
    pub fn reserve(
        capacity: usize,
        write: u64,
        reclaimed: u64,
        len: usize,
    ) -> Result<Self, ArenaError> {
        if capacity < MIN_ARENA_BYTES {
            return Err(ArenaError::BelowMinimumCapacity);
        }
        if len > MAX_FRAME_BYTES {
            return Err(ArenaError::FrameTooLarge);
        }
        let capacity = u64::try_from(capacity).map_err(|_| ArenaError::ArithmeticOverflow)?;
        let len = u64::try_from(len).map_err(|_| ArenaError::ArithmeticOverflow)?;
        let used = write
            .checked_sub(reclaimed)
            .ok_or(ArenaError::InvalidCursor)?;
        if used > capacity {
            return Err(ArenaError::InvalidCursor);
        }
        if len > capacity - used {
            return Err(ArenaError::Exhausted);
        }
        write
            .checked_add(len)
            .ok_or(ArenaError::ArithmeticOverflow)?;

        let offset = write % capacity;
        let first_len = len.min(capacity - offset);
        let second_len = len - first_len;
        Ok(Self {
            allocation_start: write,
            allocation_len: len,
            spans: [
                ArenaSpan::from_untrusted(offset, first_len),
                ArenaSpan::from_untrusted(0, second_len),
            ],
            span_count: if second_len == 0 { 1 } else { 2 },
        })
    }

    /// Same allocation, spans shortened to `exact_len` committed bytes. The allocation length
    /// is kept so reclamation still frees the full reserved range.
    ///
    /// `exact_len` is bounded by the bytes the current spans cover, not by `allocation_len`.
    /// Growing past the covered spans writes at arena offset zero, outside the reservation.
    pub fn prefix(self, exact_len: usize) -> Result<Self, ArenaError> {
        let exact_len = u64::try_from(exact_len).map_err(|_| ArenaError::ArithmeticOverflow)?;
        let committed = self.spans[0]
            .len
            .checked_add(self.spans[1].len)
            .ok_or(ArenaError::ArithmeticOverflow)?;
        if exact_len > committed {
            return Err(ArenaError::ExceedsAllocation);
        }
        let first_len = exact_len.min(self.spans[0].len);
        let second_len = exact_len - first_len;
        Ok(Self {
            allocation_start: self.allocation_start,
            allocation_len: self.allocation_len,
            spans: [
                ArenaSpan::from_untrusted(self.spans[0].offset, first_len),
                ArenaSpan::from_untrusted(0, second_len),
            ],
            span_count: if second_len == 0 { 1 } else { 2 },
        })
    }

    /// Absolute monotonic allocation start.
    pub const fn allocation_start(self) -> u64 {
        self.allocation_start
    }

    /// Reserved bytes, including any uncommitted tail.
    pub const fn allocation_len(self) -> u64 {
        self.allocation_len
    }

    /// One or two.
    pub const fn span_count(self) -> u8 {
        self.span_count
    }

    /// The span at `index`, or `None` if `index >= span_count`.
    /// `reserve` and `prefix` set `span_count` to 1 or 2, so the slice bound is in range.
    pub fn span(self, index: usize) -> Option<ArenaSpan> {
        self.spans[..usize::from(self.span_count)]
            .get(index)
            .copied()
    }
}

crate::redacted_debug!(SpanPlan);

/// How many arena bytes sit in each ownership state. `conserves` checks that the states
/// partition the capacity, so no byte is lost or double-counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArenaCounts {
    /// Bytes no state has claimed.
    pub free: u64,
    /// Bytes held by an unfinished producer.
    pub producer_reserved: u64,
    /// Published bytes not acquired by receiver.
    pub published: u64,
    /// Bytes undergoing receiver validation.
    pub receiver_held: u64,
    /// Bytes visible through receive leases.
    pub receiver_leased: u64,
    /// Released bytes awaiting FIFO reclamation.
    pub release_pending: u64,
    /// Bytes reserved for alignment and not carrying frame data. The ring backend wraps
    /// frames instead of padding, so it reports zero here.
    pub pad: u64,
    /// Bytes permanently withheld after uncertain cleanup.
    pub quarantined: u64,
}

impl ArenaCounts {
    /// Whether the eight states sum to exactly `capacity` without overflow.
    pub fn conserves(self, capacity: u64) -> bool {
        [
            self.free,
            self.producer_reserved,
            self.published,
            self.receiver_held,
            self.receiver_leased,
            self.release_pending,
            self.pad,
            self.quarantined,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
            == Some(capacity)
    }
}
