//! A sample is a complete frame delivered as one contiguous allocation: a fixed prefix
//! (`SAMPLE_PREFIX_BYTES`) followed by the body. The allocation may be longer than the body;
//! the extra capacity is slack, not payload. `SamplePrefix::validate` yields the exact body
//! range so the wire decoder never reads slack.
//!
//! The mapping stays peer-writable while a decoder runs. Nothing here prevents the peer from
//! rewriting the body after publication; callers that need stability must copy first.

use std::ops::Range;

use crate::arena::MAX_FRAME_BYTES;
use crate::descriptor::{
    DESCRIPTOR_SCHEMA_VERSION, DescriptorError, Incarnation, ReleaseIdentity, WIRE_V2_HEADER_BYTES,
};

/// Bytes ahead of every sample body: schema (2), wire header (21), incarnation (16),
/// lane (4), sequence (8), body length (8). All integers are little-endian.
pub const SAMPLE_PREFIX_BYTES: usize = 2 + WIRE_V2_HEADER_BYTES + 16 + 4 + 8 + 8;

/// The fixed prefix of a sample as read from the peer, before validation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SamplePrefix {
    schema: u16,
    wire_header: [u8; WIRE_V2_HEADER_BYTES],
    identity: ReleaseIdentity,
    body_len: u64,
}

impl SamplePrefix {
    /// Copies the prefix fields out of `payload`. Fails only when `payload` is shorter than
    /// `SAMPLE_PREFIX_BYTES`; field values are checked by `validate`.
    pub fn snapshot(payload: &[u8]) -> Result<Self, DescriptorError> {
        let prefix: &[u8; SAMPLE_PREFIX_BYTES] = payload
            .get(..SAMPLE_PREFIX_BYTES)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(DescriptorError::Truncated)?;
        let schema = u16::from_le_bytes([prefix[0], prefix[1]]);
        let mut wire_header = [0u8; WIRE_V2_HEADER_BYTES];
        wire_header.copy_from_slice(&prefix[2..2 + WIRE_V2_HEADER_BYTES]);
        let identity_offset = 2 + WIRE_V2_HEADER_BYTES;
        let mut incarnation = [0u8; 16];
        incarnation.copy_from_slice(&prefix[identity_offset..identity_offset + 16]);
        let lane_offset = identity_offset + 16;
        let mut lane = [0u8; 4];
        lane.copy_from_slice(&prefix[lane_offset..lane_offset + 4]);
        let sequence_offset = lane_offset + 4;
        let mut sequence = [0u8; 8];
        sequence.copy_from_slice(&prefix[sequence_offset..sequence_offset + 8]);
        let body_len_offset = sequence_offset + 8;
        let mut body_len = [0u8; 8];
        body_len.copy_from_slice(&prefix[body_len_offset..body_len_offset + 8]);
        Ok(Self {
            schema,
            wire_header,
            identity: ReleaseIdentity::new(
                Incarnation::from_bytes(incarnation),
                u32::from_le_bytes(lane),
                u64::from_le_bytes(sequence),
            ),
            body_len: u64::from_le_bytes(body_len),
        })
    }

    /// Identity as declared by the peer; trusted only after `validate`.
    pub const fn identity(&self) -> ReleaseIdentity {
        self.identity
    }

    /// Checks the prefix against the identity the receiver expects. `allocation_len` is the
    /// full allocation, body plus slack; the declared body must end within it. Checks run in
    /// the same order as `FrameDescriptor::validate` and return the first failure.
    pub fn validate(
        &self,
        allocation_len: usize,
        expected: ReleaseIdentity,
    ) -> Result<ValidatedSample, DescriptorError> {
        if self.schema != DESCRIPTOR_SCHEMA_VERSION {
            return Err(DescriptorError::UnsupportedSchema);
        }
        if self.identity.sequence() == 0 {
            return Err(DescriptorError::InvalidSequence);
        }
        if self.identity.incarnation() != expected.incarnation() {
            return Err(DescriptorError::WrongIncarnation);
        }
        if self.identity.lane() != expected.lane() {
            return Err(DescriptorError::WrongLane);
        }
        if self.identity.sequence() != expected.sequence() {
            return Err(DescriptorError::InvalidSequence);
        }
        if self.body_len > MAX_FRAME_BYTES as u64 {
            return Err(DescriptorError::FrameTooLarge);
        }
        let body_len = usize::try_from(self.body_len).map_err(|_| DescriptorError::Overflow)?;
        let body_end = SAMPLE_PREFIX_BYTES
            .checked_add(body_len)
            .ok_or(DescriptorError::Overflow)?;
        if body_end > allocation_len {
            return Err(DescriptorError::InvalidAllocation);
        }
        let declared = u32::from_le_bytes([
            self.wire_header[0],
            self.wire_header[1],
            self.wire_header[2],
            self.wire_header[3],
        ]);
        if u64::from(declared) != self.body_len || self.wire_header[4] != 2 {
            return Err(DescriptorError::WireHeaderMismatch);
        }
        Ok(ValidatedSample {
            identity: self.identity,
            body_len,
        })
    }
}

impl std::fmt::Debug for SamplePrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SamplePrefix(<redacted>)")
    }
}

/// A sample prefix that passed `validate`. `body_range` is the only range a decoder may read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ValidatedSample {
    identity: ReleaseIdentity,
    body_len: usize,
}

impl ValidatedSample {
    /// Identity the release must carry.
    pub const fn identity(&self) -> ReleaseIdentity {
        self.identity
    }

    /// Declared body length, at most `MAX_FRAME_BYTES`.
    pub const fn body_len(&self) -> usize {
        self.body_len
    }

    /// Body bytes within the allocation, starting after the prefix. Bytes past `end` are slack.
    pub const fn body_range(&self) -> Range<usize> {
        SAMPLE_PREFIX_BYTES..SAMPLE_PREFIX_BYTES + self.body_len
    }
}

impl std::fmt::Debug for ValidatedSample {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ValidatedSample(<redacted>)")
    }
}
