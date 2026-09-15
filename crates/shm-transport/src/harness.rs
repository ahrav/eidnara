//! Entry points the fuzz targets and the corpus-replay test call. Each takes arbitrary bytes,
//! runs one decoder, and asserts the invariants a successful decode promises. None touches a
//! file descriptor, mapping, or thread, so a fuzzer can call them millions of times.
//!
//! Every function also checks that a related identity change makes validation fail, so the
//! target cannot pass by accepting everything, and each has a corpus entry that decodes, so it
//! cannot pass by rejecting everything.

use crate::backend::ring::PoolGrant;
use crate::descriptor::{CompletionRecord, PoolDescriptor};
use crate::pool::PoolGeometry;

/// Byte length of the fixed descriptor encoding the fuzz targets decode: sequence, block,
/// generation, and body length, each little-endian `u64`.
pub const POOL_DESCRIPTOR_BYTES: usize = 4 * 8;

/// Byte length of the fixed completion encoding: block and generation, each little-endian
/// `u64`, followed by the live generation the producer ledger holds for the block.
pub const COMPLETION_BYTES: usize = 3 * 8;

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(buffer)
}

/// Decodes a `POOL_DESCRIPTOR_BYTES` descriptor and validates it against the production
/// geometry with its own sequence as the expected one. On success, asserts the body fits the
/// named block and stays within the protocol maximum. Returns whether the descriptor
/// validated.
pub fn pool_descriptor(bytes: &[u8]) -> bool {
    if bytes.len() != POOL_DESCRIPTOR_BYTES {
        return false;
    }
    let geometry = PoolGeometry::host_payload_pool();
    let sequence = read_u64(bytes, 0);
    let block = read_u64(bytes, 8);
    let generation = read_u64(bytes, 16);
    let body_len = read_u64(bytes, 24);
    let descriptor = PoolDescriptor::from_untrusted(sequence, block, generation, body_len);
    let accepted = if let Ok(validated) = descriptor.validate(sequence, &geometry) {
        assert!(validated.body_len() <= crate::pool::MAX_FRAME_BYTES as u64);
        assert!(validated.body_len() <= validated.placement().body_capacity());
        assert!(geometry.contains(validated.block()));
        assert_ne!(validated.generation(), 0);
        true
    } else {
        false
    };
    // The same bytes under the next expected sequence must fail: identity is checked before
    // geometry.
    assert!(
        descriptor
            .validate(sequence.wrapping_add(1), &geometry)
            .is_err(),
        "a mismatched sequence must be rejected"
    );
    accepted
}

/// Decodes `bytes` as a pool grant and asserts the decoder round-trips; returns whether the
/// bytes decoded.
pub fn provider_grant(bytes: &[u8]) -> bool {
    if let Ok(grant) = PoolGrant::decode_slice(bytes) {
        assert_eq!(
            grant.encode().as_slice(),
            bytes,
            "accepted grant must round-trip byte-exactly"
        );
        true
    } else {
        false
    }
}

/// Decodes a `COMPLETION_BYTES` completion and validates it against the production geometry
/// and the encoded live generation. On success, asserts the completion names exactly the live
/// generation. Returns whether the completion validated.
pub fn payload_completion(bytes: &[u8]) -> bool {
    if bytes.len() != COMPLETION_BYTES {
        return false;
    }
    let geometry = PoolGeometry::host_payload_pool();
    let block = read_u64(bytes, 0);
    let generation = read_u64(bytes, 8);
    let live = read_u64(bytes, 16);
    let record = CompletionRecord::from_untrusted(block, generation);
    let accepted = match record.validate(&geometry, (live != 0).then_some(live)) {
        Ok(freed) => {
            assert_eq!(u64::from(freed), block);
            assert_eq!(generation, live);
            true
        }
        Err(_) => false,
    };
    // A completion for a block the ledger has not published can never free anything.
    assert!(
        record.validate(&geometry, None).is_err(),
        "an unpublished block must reject every completion"
    );
    accepted
}
