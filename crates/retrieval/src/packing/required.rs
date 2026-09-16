//! This module reads neither a store nor a clock.

use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};

use kernel::EligibilityVerdict;

use super::SelectedOccurrence;
use crate::eligibility::Disposition;
use crate::fusion::OccurrenceId;

/// Accounting-token count.
pub trait TokenCount: Copy + Ord + fmt::Debug {
    const ZERO: Self;
    const MAX: Self;
    fn checked_add(self, other: Self) -> Option<Self>;
    fn checked_sub(self, other: Self) -> Option<Self>;
}

impl TokenCount for u64 {
    const ZERO: Self = 0;
    const MAX: Self = u64::MAX;

    fn checked_add(self, other: Self) -> Option<Self> {
        u64::checked_add(self, other)
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        u64::checked_sub(self, other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredRequest {
    pub occurrence: OccurrenceId,
    /// The revision the selection was made against.
    pub revision: i64,
}

/// Every bound is caller-supplied; there is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredBounds<C> {
    pub max_payload_loads: NonZeroUsize,
    pub max_payload_bytes: NonZeroU64,
    pub max_item_bytes: NonZeroU64,
    pub token_limit: C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredBound {
    PayloadLoads,
    PayloadBytes,
    ItemBytes,
}

/// The phase returns the first fault in request order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredContextFailure<C> {
    Missing(OccurrenceId),
    Stale(OccurrenceId),
    Hidden(OccurrenceId),
    Corrupt(OccurrenceId),
    Ineligible {
        occurrence: OccurrenceId,
        verdict: EligibilityVerdict,
    },
    Oversized {
        occurrence: OccurrenceId,
        bound: RequiredBound,
    },
    OverBudget {
        limit: C,
        /// The sum through the item that crossed the limit; `C::MAX` when
        /// the sum itself is unrepresentable.
        charged: C,
    },
}

impl<C> RequiredContextFailure<C> {
    pub fn class(&self) -> &'static str {
        match self {
            Self::Missing(_) => "missing",
            Self::Stale(_) => "stale",
            Self::Hidden(_) => "hidden",
            Self::Corrupt(_) => "corrupt",
            Self::Ineligible { .. } => "ineligible",
            Self::Oversized { .. } => "oversized",
            Self::OverBudget { .. } => "over_budget",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RequiredFact<'a> {
    pub request: RequiredRequest,
    pub row: &'a SelectedOccurrence,
    pub disposition: Disposition,
}

/// Only [`admit_required`] constructs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedRequired<'a> {
    row: &'a SelectedOccurrence,
}

impl<'a> AdmittedRequired<'a> {
    pub fn row(&self) -> &'a SelectedOccurrence {
        self.row
    }
}

/// Runs before any payload byte is loaded.
pub fn admit_required<'a, C>(
    facts: &[RequiredFact<'a>],
    bounds: &RequiredBounds<C>,
) -> Result<Vec<AdmittedRequired<'a>>, RequiredContextFailure<C>> {
    let mut admitted = Vec::with_capacity(facts.len());
    let mut total_bytes = 0u64;
    for (index, fact) in facts.iter().enumerate() {
        let occurrence = fact.request.occurrence;
        let row = fact.row;
        if row.occurrence != occurrence {
            return Err(RequiredContextFailure::Corrupt(occurrence));
        }
        if row.tombstone.is_some() || row.revision != fact.request.revision {
            return Err(RequiredContextFailure::Stale(occurrence));
        }
        match fact.disposition {
            Disposition::Eligible => {}
            Disposition::PolicyExcluded(EligibilityVerdict::Hidden) => {
                return Err(RequiredContextFailure::Hidden(occurrence));
            }
            Disposition::PolicyExcluded(
                EligibilityVerdict::Stale | EligibilityVerdict::Superseded,
            ) => {
                return Err(RequiredContextFailure::Stale(occurrence));
            }
            Disposition::PolicyExcluded(verdict) => {
                return Err(RequiredContextFailure::Ineligible {
                    occurrence,
                    verdict,
                });
            }
        }
        if index >= bounds.max_payload_loads.get() {
            return Err(RequiredContextFailure::Oversized {
                occurrence,
                bound: RequiredBound::PayloadLoads,
            });
        }
        let bytes = row.payload.byte_length;
        if bytes > bounds.max_item_bytes.get() {
            return Err(RequiredContextFailure::Oversized {
                occurrence,
                bound: RequiredBound::ItemBytes,
            });
        }
        total_bytes = total_bytes
            .checked_add(bytes)
            .filter(|total| *total <= bounds.max_payload_bytes.get())
            .ok_or(RequiredContextFailure::Oversized {
                occurrence,
                bound: RequiredBound::PayloadBytes,
            })?;
        admitted.push(AdmittedRequired { row });
    }
    Ok(admitted)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredReservation<C> {
    /// One cost per admitted item, in admission order.
    pub costs: Vec<C>,
    pub charged: C,
}

pub fn reserve_required<C: TokenCount>(
    admitted: &[AdmittedRequired<'_>],
    bytes: &[&[u8]],
    token_limit: C,
    cost: impl Fn(&[u8]) -> C,
) -> Result<RequiredReservation<C>, RequiredContextFailure<C>> {
    if let Some(unloaded) = admitted.get(bytes.len()) {
        return Err(RequiredContextFailure::Corrupt(unloaded.row.occurrence));
    }
    let mut charged = C::ZERO;
    let mut costs = Vec::with_capacity(admitted.len());
    for (item, bytes) in admitted.iter().zip(bytes) {
        if bytes.len() as u64 != item.row.payload.byte_length {
            return Err(RequiredContextFailure::Corrupt(item.row.occurrence));
        }
        let cost = cost(bytes);
        let sum = charged.checked_add(cost);
        charged = match sum {
            Some(sum) if sum <= token_limit => sum,
            _ => {
                return Err(RequiredContextFailure::OverBudget {
                    limit: token_limit,
                    charged: sum.unwrap_or(C::MAX),
                });
            }
        };
        costs.push(cost);
    }
    Ok(RequiredReservation { costs, charged })
}
