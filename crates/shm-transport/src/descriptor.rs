use std::fmt;

use serde::{Deserialize, Serialize};

use crate::pool::{BlockPlacement, MAX_FRAME_BYTES, PoolGeometry};

/// Shared descriptor schema version; the sole version either peer accepts.
pub const DESCRIPTOR_SCHEMA_VERSION: u16 = 4;
/// Pool mappings a grant transfers, one per direction.
pub const SETUP_MAPPING_COUNT: usize = 2;
/// Doorbell descriptors a grant transfers, two per direction. `profile` charges this many
/// file descriptors on top of the mappings, so the admission budget and the setup transfer
/// count cannot drift apart.
pub const SETUP_DOORBELL_COUNT: usize = 4;
/// File descriptors a grant transfers over the setup socket: the pool mappings and the
/// doorbells. `setup_auth` re-exports this as `RING_DESCRIPTOR_COUNT`.
pub const SETUP_DESCRIPTOR_COUNT: usize = SETUP_MAPPING_COUNT + SETUP_DOORBELL_COUNT;
/// Frozen application header length.
pub const WIRE_V3_HEADER_BYTES: usize = 21;
/// Version byte at `wire_header[4]`.
pub const WIRE_V3_VERSION: u8 = 3;

/// Shared by the producer's commit and the consumer's receive so both paths agree on which
/// wire headers are admissible. Callers that must reject a header before consuming a
/// reservation use this ahead of `commit`, which runs the same check.
pub fn check_wire_header(
    wire_header: &[u8; WIRE_V3_HEADER_BYTES],
    body_len: u64,
) -> Result<(), DescriptorError> {
    let declared_len = u32::from_le_bytes([
        wire_header[0],
        wire_header[1],
        wire_header[2],
        wire_header[3],
    ]);
    if u64::from(declared_len) != body_len || wire_header[4] != WIRE_V3_VERSION {
        return Err(DescriptorError::WireHeaderMismatch);
    }
    Ok(())
}

/// Validated opaque hardware-profile identifier.
///
/// Deserialization calls [`HardwareProfileId::new`], so decoded values are validated;
/// serialization emits the contained string.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
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

impl TryFrom<String> for HardwareProfileId {
    type Error = DescriptorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for HardwareProfileId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

crate::redacted_debug!(HardwareProfileId);

/// Fixed pool profile identity carried by an authenticated grant.
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

crate::redacted_debug!(TransportDescriptor);

/// 128-bit identity drawn once per pool incarnation. A payload from an earlier incarnation
/// carries a different value, so its identity fails every check even if the block and
/// generation line up.
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

    /// Raw bytes for the setup channel. `Debug` redacts them; keep them out of logs.
    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }
}

crate::redacted_debug!(Incarnation);

/// The identity a payload return carries: pool incarnation, direction lane, block id, and
/// reuse generation. No pointer or peer-supplied offset appears here; reclamation authority is
/// this tuple checked against the producer's private ledger.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PayloadIdentity {
    incarnation: Incarnation,
    lane: u32,
    block: u32,
    generation: u64,
}

impl PayloadIdentity {
    /// Assembles the identity; `generation` zero is accepted here and rejected by validation.
    pub const fn new(incarnation: Incarnation, lane: u32, block: u32, generation: u64) -> Self {
        Self {
            incarnation,
            lane,
            block,
            generation,
        }
    }

    /// Pool incarnation this payload belongs to.
    pub const fn incarnation(self) -> Incarnation {
        self.incarnation
    }

    /// Direction lane.
    pub const fn lane(self) -> u32 {
        self.lane
    }

    /// Block id within the direction's arena.
    pub const fn block(self) -> u32 {
        self.block
    }

    /// Reuse generation of the block; increases with every reservation of the block.
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

crate::redacted_debug!(PayloadIdentity);

/// One descriptor slot as read from shared memory, field by field through fixed-width
/// atomics, before any check runs. Copying first means a peer that rewrites the slot
/// mid-validation cannot make one field pass and another fail against different values.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PoolDescriptor {
    sequence: u64,
    block: u64,
    generation: u64,
    body_len: u64,
}

impl PoolDescriptor {
    /// Captures descriptor fields exactly as read from shared memory. Nothing is checked
    /// here; `validate` decides whether the snapshot describes an admissible frame.
    pub const fn from_untrusted(sequence: u64, block: u64, generation: u64, body_len: u64) -> Self {
        Self {
            sequence,
            block,
            generation,
            body_len,
        }
    }

    /// Publication sequence the slot claims.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Checks the snapshot against the sequence the receiver expects and the receiver's own
    /// geometry. Checks run in this order and the first failure is returned: sequence
    /// nonzero and expected, block id inside the geometry, generation nonzero, body length
    /// within the protocol maximum and the block's capacity.
    pub fn validate(
        self,
        expected_sequence: u64,
        geometry: &PoolGeometry,
    ) -> Result<ValidatedDescriptor, DescriptorError> {
        let Self {
            sequence,
            block,
            generation,
            body_len,
        } = self;
        if sequence == 0 || sequence != expected_sequence {
            return Err(DescriptorError::InvalidSequence);
        }
        let block = u32::try_from(block).map_err(|_| DescriptorError::InvalidBlock)?;
        let placement = geometry
            .placement(block)
            .ok_or(DescriptorError::InvalidBlock)?;
        if generation == 0 {
            return Err(DescriptorError::InvalidGeneration);
        }
        if body_len > MAX_FRAME_BYTES as u64 {
            return Err(DescriptorError::FrameTooLarge);
        }
        if body_len > placement.body_capacity() {
            return Err(DescriptorError::BodyExceedsBlock);
        }
        placement
            .offset
            .checked_add(WIRE_V3_HEADER_BYTES as u64)
            .and_then(|start| start.checked_add(body_len))
            .ok_or(DescriptorError::Overflow)?;
        Ok(ValidatedDescriptor {
            sequence,
            block,
            generation,
            body_len,
            placement,
        })
    }
}

crate::redacted_debug!(PoolDescriptor);

/// A descriptor that passed `PoolDescriptor::validate`: its block exists in the receiver's
/// geometry and its body fits that block, so the receiver may build a lease over it once
/// the local records also accept the identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ValidatedDescriptor {
    sequence: u64,
    block: u32,
    generation: u64,
    body_len: u64,
    placement: BlockPlacement,
}

impl ValidatedDescriptor {
    /// Publication sequence, equal to the expected one.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Block id.
    pub const fn block(self) -> u32 {
        self.block
    }

    /// Reuse generation, nonzero.
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Declared body length, at most `MAX_FRAME_BYTES` and the block's capacity.
    pub const fn body_len(self) -> u64 {
        self.body_len
    }

    /// Where the block lives, from the receiver's geometry.
    pub const fn placement(self) -> BlockPlacement {
        self.placement
    }
}

crate::redacted_debug!(ValidatedDescriptor);

/// A payload return as the producer ledger sees it: block id and the generation the returner
/// captured at receive time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CompletionRecord {
    block: u64,
    generation: u64,
}

impl CompletionRecord {
    /// Captures a completion without checking it.
    pub const fn from_untrusted(block: u64, generation: u64) -> Self {
        Self { block, generation }
    }

    /// Checks a completion against the producer's ledger. `live` is the generation the
    /// producer has outstanding on the block, or `None` when the block is not published.
    /// A generation below the live one is a stale return and frees nothing; a generation
    /// above it, or a completion for a block that is not published, names a payload this
    /// pool never issued and is a protocol error, never free-list input.
    pub fn validate(
        self,
        geometry: &PoolGeometry,
        live: Option<u64>,
    ) -> Result<u32, DescriptorError> {
        let block = u32::try_from(self.block).map_err(|_| DescriptorError::InvalidBlock)?;
        if !geometry.contains(block) {
            return Err(DescriptorError::InvalidBlock);
        }
        if self.generation == 0 {
            return Err(DescriptorError::InvalidGeneration);
        }
        match live {
            None => Err(DescriptorError::FutureCompletion),
            Some(live) if self.generation > live => Err(DescriptorError::FutureCompletion),
            Some(live) if self.generation < live => Err(DescriptorError::StaleCompletion),
            Some(_) => Ok(block),
        }
    }
}

crate::redacted_debug!(CompletionRecord);

/// Why a descriptor, grant, or completion was rejected. Each variant is one failed check.
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
    #[error("payload identity does not match incarnation")]
    WrongIncarnation,
    /// The lane differs from the expected one.
    #[error("payload identity does not match lane")]
    WrongLane,
    /// Sequence is zero or does not match the expected sequence.
    #[error("publication sequence is invalid")]
    InvalidSequence,
    /// The block id is outside the geometry.
    #[error("payload block id is invalid")]
    InvalidBlock,
    /// The generation is zero.
    #[error("payload generation is invalid")]
    InvalidGeneration,
    /// `body_len` exceeds `MAX_FRAME_BYTES`.
    #[error("frame exceeds protocol maximum")]
    FrameTooLarge,
    /// `body_len` exceeds the named block's body capacity.
    #[error("body exceeds its block capacity")]
    BodyExceedsBlock,
    /// An offset or length sum overflowed.
    #[error("descriptor arithmetic overflow")]
    Overflow,
    /// The wire header's declared length or version disagrees with the descriptor.
    #[error("wire header disagrees with descriptor")]
    WireHeaderMismatch,
    /// A completion names a generation older than the live occupant; it frees nothing.
    #[error("payload completion is stale")]
    StaleCompletion,
    /// A completion names a generation newer than any this pool issued, or a block with no
    /// published occupant.
    #[error("payload completion names an unissued payload")]
    FutureCompletion,
    /// A published descriptor names a block whose previous lease is still live locally.
    #[error("payload block is already live")]
    DuplicateLiveBlock,
    /// A published descriptor's generation does not exceed the last one seen for its block.
    #[error("payload generation did not increase")]
    NonIncreasingGeneration,
}

impl fmt::Debug for DescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
