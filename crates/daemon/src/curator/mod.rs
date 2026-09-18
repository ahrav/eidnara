//! The Curator investigation coordinator's daemon-owned pieces: the evidence broker, run-local aliases, accounting, provenance tags, the disclosure ledger, bounded related-memory discovery, confined project-text inspection, the verified one-use model sender with its bounded response decoding, the authorized, durably charged disclosure of one prepared body, and the settlement that publishes a run's proposal only through its completed receipt.

use std::ops::Range;

pub mod broker;
pub mod disclosure;
pub mod model_request;
pub mod model_response;
pub mod project_text;
pub mod related_memories;
pub mod settlement;

/// Per-hit excerpt bound (Q19), in bytes of the referenced artifact.
pub const MAX_EXCERPT_BYTES: usize = 512;
/// Bytes kept before a matching position so an excerpt carries its lead-in.
pub(crate) const EXCERPT_LEAD_BYTES: usize = 64;

/// Why a bounded traversal stopped where it did. Only `Complete` means the inventory was exhausted; every other code is explicit incompleteness, never absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    /// Every candidate was examined.
    Complete,
    /// The bound on candidates or entries examined in one call was reached.
    CandidateBound,
    /// The bound on bytes probed or scanned in one call was reached.
    ProbeBound,
    /// The bound on hits delivered in one call was reached.
    PageFull,
    /// The model batch has no operation left, or a broker capacity bound stopped the call after at least one hit.
    CapacityBound,
    /// The caller's budget expired or was cancelled after the call passed at least one candidate.
    BudgetBound,
}

/// The excerpt window around `position` in `text`: up to [`EXCERPT_LEAD_BYTES`] before it and [`MAX_EXCERPT_BYTES`] long, clamped to char boundaries. Rounding the lead up keeps at least `MAX_EXCERPT_BYTES - EXCERPT_LEAD_BYTES` bytes after the match, which bounds every matcher.
pub(crate) fn excerpt_window(text: &str, position: usize) -> Range<usize> {
    let start = text.ceil_char_boundary(position.saturating_sub(EXCERPT_LEAD_BYTES));
    let end = text.floor_char_boundary(start.saturating_add(MAX_EXCERPT_BYTES));
    start..end
}
