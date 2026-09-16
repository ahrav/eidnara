use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};

use std::collections::HashMap;

use super::grouping::GroupIdentity;
use super::{SelectedOccurrence, TokenCount};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan<C> {
    pub admitted: Vec<usize>,
    pub skipped: Vec<usize>,
    pub remaining: C,
}

/// `marginal` sees the indices admitted so far, so a wrapper shared by
/// several items is charged once.
pub fn skip_and_continue<C: TokenCount>(
    budget: C,
    count: usize,
    mut marginal: impl FnMut(&[usize], usize) -> C,
) -> Scan<C> {
    let mut remaining = budget;
    let mut admitted: Vec<usize> = Vec::new();
    let mut skipped = Vec::new();
    for index in 0..count {
        let cost = marginal(&admitted, index);
        match remaining.checked_sub(cost) {
            Some(left) => {
                remaining = left;
                admitted.push(index);
            }
            None => skipped.push(index),
        }
    }
    Scan {
        admitted,
        skipped,
        remaining,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundExceeded {
    pub bound: OptionalBound,
    /// The zero-based position that crossed the bound.
    pub at: usize,
}

impl fmt::Display for BoundExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} exceeded at position {}", self.bound, self.at)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalBound {
    FusedCandidates,
    Parents,
    SpansPerParent,
    PayloadLoads,
    PayloadBytes,
    ItemBytes,
}

/// Every bound is caller-supplied; there is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionalBounds {
    pub max_fused_candidates: NonZeroUsize,
    pub max_parents: NonZeroUsize,
    pub max_spans_per_parent: NonZeroUsize,
    pub max_payload_loads: NonZeroUsize,
    pub max_payload_bytes: NonZeroU64,
    pub max_item_bytes: NonZeroU64,
}

/// Checks the fused candidate count before any row is read.
pub fn admit_fused_candidates(count: usize, bounds: &OptionalBounds) -> Result<(), BoundExceeded> {
    if count > bounds.max_fused_candidates.get() {
        return Err(BoundExceeded {
            bound: OptionalBound::FusedCandidates,
            at: bounds.max_fused_candidates.get(),
        });
    }
    Ok(())
}

/// Checks the rows that survived exclusion against every remaining bound
/// before any byte is loaded and names the first position that crosses one.
pub fn admit_optional_set<'a>(
    rows: impl IntoIterator<Item = &'a SelectedOccurrence>,
    bounds: &OptionalBounds,
) -> Result<(), BoundExceeded> {
    let mut total_bytes = 0u64;
    let mut spans_per_parent: HashMap<GroupIdentity, usize> = HashMap::new();
    for (at, row) in rows.into_iter().enumerate() {
        let exceeded = |bound| BoundExceeded { bound, at };
        if at >= bounds.max_payload_loads.get() {
            return Err(exceeded(OptionalBound::PayloadLoads));
        }
        if row.payload.byte_length > bounds.max_item_bytes.get() {
            return Err(exceeded(OptionalBound::ItemBytes));
        }
        total_bytes = total_bytes
            .checked_add(row.payload.byte_length)
            .filter(|total| *total <= bounds.max_payload_bytes.get())
            .ok_or(exceeded(OptionalBound::PayloadBytes))?;
        let identity = GroupIdentity::of(row);
        if !spans_per_parent.contains_key(&identity)
            && spans_per_parent.len() >= bounds.max_parents.get()
        {
            return Err(exceeded(OptionalBound::Parents));
        }
        let spans = spans_per_parent.entry(identity).or_insert(0);
        *spans += 1;
        if *spans > bounds.max_spans_per_parent.get() {
            return Err(exceeded(OptionalBound::SpansPerParent));
        }
    }
    Ok(())
}
