//! Fixed shared-memory payload-pool transport between the host and one peer process.
//!
//! Each direction carries application frames through fixed blocks in a memfd-backed arena.
//! Every frame is described by a descriptor the receiver validates against its own geometry
//! and per-block records before any byte of the body is read, so a peer that writes into
//! shared memory cannot make the receiver decode a frame it did not admit. Received payloads
//! are owned leases that return their block exactly once from any thread. The transport has
//! no fallback: an unavailable or corrupt pool is terminal for that connection.
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

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

/// Ring backend and the retained backing it shares with owned leases.
pub mod backend;
/// Descriptor schema and validation for frames received from an untrusted peer.
pub mod descriptor;
/// Fuzz and corpus-replay entry points for the strict byte decoders.
pub mod harness;
/// Receive leases: owned payloads with bounded raw views that return exactly once.
pub mod lease;
/// Close-state machine shared by the native addon and the host.
pub mod lifecycle;
/// Pool geometry: block classes, ids, and the mapping layout both peers derive from them.
pub mod pool;
/// Hardware profiles: the pool geometry a profile id names, and host-wide admission of them.
pub mod profile;
/// Setup-handshake proof transcript shared by both peers.
pub mod setup_auth;

pub use descriptor::{Incarnation, PayloadIdentity, WIRE_V3_HEADER_BYTES};
pub use lease::{LeaseSpan, PayloadLease};
pub use pool::MAX_FRAME_BYTES;
