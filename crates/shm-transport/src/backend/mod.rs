//! Backends that publish frames into the arena.

/// Sealed descriptor ring over a memfd arena; the production transport.
pub mod ring;
/// Sample prefix decoding for complete frames received from an untrusted peer.
pub mod sample;
/// Every libc call the ring makes, one safe wrapper each.
pub(crate) mod sys;
