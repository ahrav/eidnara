use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arena::{ArenaSpan, MAX_FRAME_BYTES};

/// Shared descriptor schema version.
pub const DESCRIPTOR_SCHEMA_VERSION: u16 = 3;
/// Setup descriptor count.
pub const SETUP_DESCRIPTOR_COUNT: usize = 6;
/// Frozen wire-v2 header length.
pub const WIRE_V2_HEADER_BYTES: usize = 21;
/// A complete-frame descriptor contains at most two shared spans.
pub const MAX_SPANS: usize = 2;

/// Validated opaque hardware-profile identifier.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HardwareProfileId(String);

impl HardwareProfileId {
    /// Accepts 1 to 64 ASCII alphanumeric, `-`, `_`, or `.` bytes; anything else is
    /// `DescriptorError::InvalidHardwareProfile`.
    pub fn new(value: impl Into<String>) -> Result<Self, DescriptorError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(DescriptorError::InvalidHardwareProfile);
        }
        Ok(Self(value))
    }

    /// Whether this identifier spells exactly `value`.
    pub fn matches(&self, value: &str) -> bool {
        self.0 == value
    }
}

impl fmt::Debug for HardwareProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HardwareProfileId(<redacted>)")
    }
}

/// Fixed ring profile identity carried by an authenticated grant.
#[derive(Clone, PartialEq, Eq)]
pub struct TransportDescriptor {
    schema_version: u16,
    hardware: HardwareProfileId,
}

impl TransportDescriptor {
    /// Constructs the transport descriptor.
    pub const fn new(hardware: HardwareProfileId) -> Self {
        Self {
            schema_version: DESCRIPTOR_SCHEMA_VERSION,
            hardware,
        }
    }

    /// Schema version.
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Tests equality with expected hardware-profile identifier.
    pub fn hardware_matches(&self, expected: &str) -> bool {
        self.hardware.matches(expected)
    }
}

impl fmt::Debug for TransportDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransportDescriptor(<redacted>)")
    }
}

/// Each candidate receives a fresh 128-bit identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Incarnation([u8; 16]);

impl Incarnation {
    /// Draws a fresh identity from the operating-system random source.
    pub fn random() -> Result<Self, DescriptorError> {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).map_err(|_| DescriptorError::RandomSourceUnavailable)?;
        Ok(Self(bytes))
    }

    /// Wraps bytes received over the setup channel without checking them; validation
    /// compares them against the expected incarnation.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Diagnostics must not include the setup-channel representation.
    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl fmt::Debug for Incarnation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Incarnation(<redacted>)")
    }
}

/// ReleaseIdentity qualifies a completion by incarnation, lane, and sequence.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReleaseIdentity {
    incarnation: Incarnation,
    lane: u32,
    sequence: u64,
}

impl ReleaseIdentity {
    /// Builds the identity a completion must echo back exactly.
    pub const fn new(incarnation: Incarnation, lane: u32, sequence: u64) -> Self {
        Self {
            incarnation,
            lane,
            sequence,
        }
    }

    /// ReleaseIdentity::incarnation returns the incarnation required for exact completion matching.
    pub const fn incarnation(self) -> Incarnation {
        self.incarnation
    }

    /// Lane the frame travels on.
    pub const fn lane(self) -> u32 {
        self.lane
    }

    /// Per-lane sequence number; zero is never valid.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

impl fmt::Debug for ReleaseIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReleaseIdentity(<redacted>)")
    }
}

/// FrameDescriptor stores a complete metadata snapshot received from an untrusted source.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrameDescriptor {
    schema_version: u16,
    wire_header: [u8; WIRE_V2_HEADER_BYTES],
    identity: ReleaseIdentity,
    body_len: u64,
    allocation_start: u64,
    allocation_len: u64,
    span_count: u8,
    spans: [ArenaSpan; MAX_SPANS],
}

impl FrameDescriptor {
    #[allow(
        clippy::too_many_arguments,
        reason = "models fixed shared descriptor fields"
    )]
    /// Captures descriptor fields exactly as read from shared memory. Nothing is checked
    /// here; `validate` decides whether the snapshot describes an admissible frame.
    pub const fn from_untrusted(
        schema_version: u16,
        wire_header: [u8; WIRE_V2_HEADER_BYTES],
        identity: ReleaseIdentity,
        body_len: u64,
        allocation_start: u64,
        allocation_len: u64,
        span_count: u8,
        spans: [ArenaSpan; MAX_SPANS],
    ) -> Self {
        Self {
            schema_version,
            wire_header,
            identity,
            body_len,
            allocation_start,
            allocation_len,
            span_count,
            spans,
        }
    }

    /// Checks the snapshot against the identity the receiver expects and the arena size.
    ///
    /// Every field is validated before the body is trusted: schema version, exact identity
    /// match, body and allocation bounds, span count and wrap layout, and the wire header's
    /// declared length and version. The first failing check is returned.
    pub fn validate(
        self,
        expected: ReleaseIdentity,
        arena_bytes: usize,
    ) -> Result<ValidatedFrame, DescriptorError> {
        let Self {
            schema_version,
            wire_header,
            identity,
            body_len,
            allocation_start,
            allocation_len,
            span_count,
            spans,
        } = self;

        if schema_version != DESCRIPTOR_SCHEMA_VERSION {
            return Err(DescriptorError::UnsupportedSchema);
        }
        if identity.sequence == 0 {
            return Err(DescriptorError::InvalidSequence);
        }
        if identity.incarnation != expected.incarnation {
            return Err(DescriptorError::WrongIncarnation);
        }
        if identity.lane != expected.lane {
            return Err(DescriptorError::WrongLane);
        }
        if identity.sequence != expected.sequence {
            return Err(DescriptorError::InvalidSequence);
        }
        if body_len > MAX_FRAME_BYTES as u64 {
            return Err(DescriptorError::FrameTooLarge);
        }
        let arena_bytes = u64::try_from(arena_bytes).map_err(|_| DescriptorError::Overflow)?;
        if arena_bytes == 0 || allocation_len > arena_bytes || allocation_len < body_len {
            return Err(DescriptorError::InvalidAllocation);
        }
        allocation_start
            .checked_add(allocation_len)
            .ok_or(DescriptorError::Overflow)?;
        if !(1..=MAX_SPANS as u8).contains(&span_count) {
            return Err(DescriptorError::InvalidSpanCount);
        }
        if spans[0].offset != allocation_start % arena_bytes {
            return Err(DescriptorError::InvalidWrapMetadata);
        }

        let first_end = spans[0]
            .offset
            .checked_add(spans[0].len)
            .ok_or(DescriptorError::Overflow)?;
        if first_end > arena_bytes {
            return Err(DescriptorError::OutOfBounds);
        }
        let summed = spans[0]
            .len
            .checked_add(spans[1].len)
            .ok_or(DescriptorError::Overflow)?;
        if summed != body_len {
            return Err(DescriptorError::LengthMismatch);
        }

        match span_count {
            1 => {
                if spans[1] != ArenaSpan::default() {
                    return Err(DescriptorError::InvalidWrapMetadata);
                }
            }
            2 => {
                if spans[0].is_empty()
                    || spans[1].is_empty()
                    || first_end != arena_bytes
                    || spans[1].offset != 0
                    || spans[1].len > arena_bytes
                {
                    return Err(DescriptorError::InvalidWrapMetadata);
                }
            }
            _ => return Err(DescriptorError::InvalidSpanCount),
        }

        let declared_len = u32::from_le_bytes([
            wire_header[0],
            wire_header[1],
            wire_header[2],
            wire_header[3],
        ]);
        if u64::from(declared_len) != body_len || wire_header[4] != 2 {
            return Err(DescriptorError::WireHeaderMismatch);
        }

        Ok(ValidatedFrame {
            wire_header,
            identity,
            body_len,
            allocation_start,
            allocation_len,
            span_count,
            spans,
        })
    }
}

impl fmt::Debug for FrameDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FrameDescriptor(<redacted>)")
    }
}

/// A frame descriptor that passed `FrameDescriptor::validate`; its spans are in bounds and
/// its lengths agree, so the receiver may build a lease over them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ValidatedFrame {
    wire_header: [u8; WIRE_V2_HEADER_BYTES],
    identity: ReleaseIdentity,
    body_len: u64,
    allocation_start: u64,
    allocation_len: u64,
    span_count: u8,
    spans: [ArenaSpan; MAX_SPANS],
}

impl ValidatedFrame {
    /// The 21-byte wire-v2 header carried alongside the body.
    pub const fn wire_header(self) -> [u8; WIRE_V2_HEADER_BYTES] {
        self.wire_header
    }

    /// Identity the completion must carry.
    pub const fn identity(self) -> ReleaseIdentity {
        self.identity
    }

    /// Declared body length, at most `MAX_FRAME_BYTES`.
    pub const fn body_len(self) -> u64 {
        self.body_len
    }

    /// Unwrapped arena offset where the allocation begins.
    pub const fn allocation_start(self) -> u64 {
        self.allocation_start
    }

    /// Allocation length, at least `body_len` and at most the arena size.
    pub const fn allocation_len(self) -> u64 {
        self.allocation_len
    }

    /// One span, or two when the body wraps around the arena end.
    pub const fn span_count(self) -> u8 {
        self.span_count
    }

    /// The span at `index`, or `None` past `span_count`.
    pub fn span(self, index: usize) -> Option<ArenaSpan> {
        (index < usize::from(self.span_count)).then_some(self.spans[index])
    }
}

impl fmt::Debug for ValidatedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedFrame(<redacted>)")
    }
}

/// Descriptor-state snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptorCounts {
    /// Reusable descriptors.
    pub free: u64,
    /// Reserved by the producer, not yet published.
    pub producer_reserved: u64,
    /// Published and waiting for the receiver.
    pub published: u64,
    /// Taken by the receiver, not yet leased to a caller.
    pub receiver_held: u64,
    /// Leased to a caller through a `ReceiveLease`.
    pub receiver_leased: u64,
    /// Released by the caller, not yet returned to the producer.
    pub release_pending: u64,
    /// Withdrawn after a protocol violation; never reused.
    pub quarantined: u64,
}

impl DescriptorCounts {
    /// Whether the seven states sum to exactly `depth` without overflow, so no descriptor
    /// was lost or counted twice.
    pub fn conserves(self, depth: u64) -> bool {
        [
            self.free,
            self.producer_reserved,
            self.published,
            self.receiver_held,
            self.receiver_leased,
            self.release_pending,
            self.quarantined,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
            == Some(depth)
    }
}

/// Why a descriptor, grant, or sample was rejected. Each variant is one failed check.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorError {
    /// The operating-system random source failed.
    #[error("operating-system random source unavailable")]
    RandomSourceUnavailable,
    /// The profile id is empty, too long, or has a byte outside the allowed set.
    #[error("hardware profile identifier is invalid")]
    InvalidHardwareProfile,
    /// The byte buffer is shorter than the fixed structure it should hold.
    #[error("fixed structure is truncated")]
    Truncated,
    /// The schema version is not `DESCRIPTOR_SCHEMA_VERSION`.
    #[error("descriptor schema is unsupported")]
    UnsupportedSchema,
    /// The incarnation differs from the expected one.
    #[error("release identity does not match incarnation")]
    WrongIncarnation,
    /// The lane differs from the expected one.
    #[error("release identity does not match lane")]
    WrongLane,
    /// Sequence is zero or does not match the expected sequence.
    #[error("release sequence is invalid")]
    InvalidSequence,
    /// body_len exceeds MAX_FRAME_BYTES.
    #[error("frame exceeds protocol maximum")]
    FrameTooLarge,
    /// The allocation is empty, exceeds the arena, or is shorter than the body.
    #[error("arena allocation is invalid")]
    InvalidAllocation,
    /// The span count is not 1 or 2.
    #[error("descriptor span count is invalid")]
    InvalidSpanCount,
    /// A span ends past the arena.
    #[error("descriptor span is outside arena")]
    OutOfBounds,
    /// An offset or length sum overflowed.
    #[error("descriptor arithmetic overflow")]
    Overflow,
    /// The span lengths do not sum to the body length.
    #[error("descriptor lengths disagree")]
    LengthMismatch,
    /// The spans do not describe a valid single or wrapped layout.
    #[error("descriptor wrap metadata is invalid")]
    InvalidWrapMetadata,
    /// The wire header's declared length or version disagrees with the descriptor.
    #[error("wire header disagrees with descriptor")]
    WireHeaderMismatch,
}

impl fmt::Debug for DescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
