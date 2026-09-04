//! Fixed shared-memory ring transport between the host and one peer process.
//!
//! The ring carries wire-v2 frames through a memfd-backed arena. Every frame is described by
//! a descriptor the receiver validates against the identity it expects before any byte of the
//! body is read, so a peer that writes into shared memory cannot make the receiver decode a
//! frame it did not admit. The transport has no fallback: an unavailable or corrupt ring is
//! terminal for that connection.
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

/// `Debug` that prints only the type name. Values here are sentinels a peer must echo back,
/// so they stay out of logs.
macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => {$(
        impl ::core::fmt::Debug for $ty {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(concat!(stringify!($ty), "(<redacted>)"))
            }
        }
    )+};
}
pub(crate) use redacted_debug;

/// Arena geometry: span arithmetic and the frame size bounds shared by both peers.
pub mod arena;
/// Ring and sample backends that publish frames into the arena.
pub mod backend;
/// Descriptor schema and validation for frames received from an untrusted peer.
pub mod descriptor;
/// Machine-readable evidence records emitted by the hardware-envelope bench.
pub mod evidence;
/// Fuzz and corpus-replay entry points for the strict byte decoders.
pub mod harness;
/// Receive leases: bounded raw views over the arena that release exactly once.
pub mod lease;
/// Close-state machine shared by the native addon and the host.
pub mod lifecycle;
/// Hardware profiles: the ring geometry a profile id names, and host-wide admission of them.
pub mod profile;
/// Setup-handshake proof transcript shared by both peers.
pub mod setup_auth;

pub use arena::{MAX_FRAME_BYTES, MIN_ARENA_BYTES};
pub use descriptor::{Incarnation, ReleaseIdentity, WIRE_V2_HEADER_BYTES};
pub use lease::{LeaseSpan, ReceiveLease};
