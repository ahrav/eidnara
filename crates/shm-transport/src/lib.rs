//! Fixed shared-memory ring transport between the host and one peer process.
//!
//! The ring carries wire-v2 frames through a memfd-backed arena. Every frame is described by
//! a descriptor the receiver validates against the identity it expects before any byte of the
//! body is read, so a peer that writes into shared memory cannot make the receiver decode a
//! frame it did not admit. The transport has no fallback: an unavailable or corrupt ring is
//! terminal for that connection.
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

/// Arena geometry: span arithmetic and the frame size bounds shared by both peers.
pub mod arena;
/// Frame decoders that run before any body byte is trusted; `sample` handles the fixed
/// prefix ahead of every complete-frame body.
pub mod backend;
/// Descriptor schema and validation for frames received from an untrusted peer.
pub mod descriptor;
/// Close-state machine shared by the native addon and the host.
pub mod lifecycle;

pub use arena::{MAX_FRAME_BYTES, MIN_ARENA_BYTES};
pub use descriptor::{Incarnation, ReleaseIdentity, WIRE_V2_HEADER_BYTES};
