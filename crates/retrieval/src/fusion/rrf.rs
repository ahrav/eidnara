use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use super::{DeclaredLanes, Lane, OccurrenceId, RawScore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FusionRefusal {
    #[error("lane {0} has a weight that is negative or not finite")]
    Weight(Lane),
    #[error("k is not positive and finite")]
    K,
    /// Finite weights and a finite k can still sum to infinity at rank one, so the maximum score is checked before any occurrence is scored.
    #[error("the maximum fused score is not finite under these parameters")]
    SumOverflow,
    #[error("the fused union exceeds the bound of {bound} occurrences")]
    UnionExceeds { bound: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionParameters {
    weights: [f64; Lane::ORDER.len()],
    k: f64,
}

impl FusionParameters {
    /// `weights` are given in [`Lane::ORDER`]. A negative-zero weight is admitted as zero, so an all-zero parameter set has one spelling and every score it produces is `+0.0`.
    pub fn new(weights: [f64; Lane::ORDER.len()], k: f64) -> Result<Self, FusionRefusal> {
        let mut admitted = [0.0; Lane::ORDER.len()];
        for (slot, (lane, weight)) in admitted
            .iter_mut()
            .zip(Lane::ORDER.into_iter().zip(weights))
        {
            if !weight.is_finite() || weight < 0.0 {
                return Err(FusionRefusal::Weight(lane));
            }
            *slot = weight + 0.0;
        }
        if !k.is_finite() || k <= 0.0 {
            return Err(FusionRefusal::K);
        }
        let parameters = Self {
            weights: admitted,
            k,
        };
        let maximum = Lane::ORDER
            .into_iter()
            .fold(0.0f64, |sum, lane| sum + parameters.term(lane, 1));
        if !maximum.is_finite() {
            return Err(FusionRefusal::SumOverflow);
        }
        Ok(parameters)
    }

    pub fn weight(&self, lane: Lane) -> f64 {
        self.weights[lane.index()]
    }

    pub fn k(&self) -> f64 {
        self.k
    }

    fn term(&self, lane: Lane, position: usize) -> f64 {
        self.weight(lane) / (self.k + position as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneContribution {
    pub position: NonZeroUsize,
    pub raw_score: RawScore,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusedEntry {
    occurrence: OccurrenceId,
    position: NonZeroUsize,
    score: f64,
    contributions: [Option<LaneContribution>; Lane::ORDER.len()],
}

impl FusedEntry {
    pub fn occurrence(&self) -> &OccurrenceId {
        &self.occurrence
    }

    /// The position assigned by fusion; a later filter keeps it, so survivors can show gaps.
    pub fn position(&self) -> NonZeroUsize {
        self.position
    }

    pub fn score(&self) -> f64 {
        self.score
    }

    /// The lane's own position and raw score, unchanged by fusion; `None` when the lane did not rank the occurrence.
    pub fn lane(&self, lane: Lane) -> Option<&LaneContribution> {
        self.contributions[lane.index()].as_ref()
    }
}

/// Fusion runs once: the only constructor is [`fuse`], and [`Fused::filter`] removes entries without touching a survivor's position or score.
#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    entries: Vec<FusedEntry>,
    absent: [bool; Lane::ORDER.len()],
}

impl Fused {
    pub fn entries(&self) -> &[FusedEntry] {
        &self.entries
    }

    /// A lane that was not declared contributed zero to every score; the route reports it beside the ranking rather than treating it as a fusion error.
    pub fn absent_lanes(&self) -> impl Iterator<Item = Lane> + '_ {
        Lane::ORDER
            .into_iter()
            .filter(move |lane| self.absent[lane.index()])
    }

    pub fn filter(mut self, mut keep: impl FnMut(&FusedEntry) -> bool) -> Self {
        self.entries.retain(|entry| keep(entry));
        self
    }
}

/// Consumes the declared lanes so a second fusion of the same input is not expressible.
/// Terms are summed in [`Lane::ORDER`] in `f64`; an undeclared lane or one that did not rank an occurrence adds nothing; order is descending score, then occurrence-identifier bytes.
/// The union is refused as soon as it would hold more than `bound` occurrences, before the excess is materialized.
pub fn fuse(
    lanes: DeclaredLanes,
    parameters: &FusionParameters,
    bound: NonZeroUsize,
) -> Result<Fused, FusionRefusal> {
    let mut union: BTreeMap<OccurrenceId, [Option<LaneContribution>; Lane::ORDER.len()]> =
        BTreeMap::new();
    let mut absent = [true; Lane::ORDER.len()];
    for ranking in lanes.rankings() {
        let lane = ranking.lane();
        absent[lane.index()] = false;
        for entry in ranking.entries() {
            let occurrence = *entry.occurrence();
            if !union.contains_key(&occurrence) && union.len() >= bound.get() {
                return Err(FusionRefusal::UnionExceeds { bound: bound.get() });
            }
            union.entry(occurrence).or_default()[lane.index()] = Some(LaneContribution {
                position: entry.position(),
                raw_score: entry.raw_score(),
            });
        }
    }
    let mut scored: Vec<(
        OccurrenceId,
        f64,
        [Option<LaneContribution>; Lane::ORDER.len()],
    )> = union
        .into_iter()
        .map(|(occurrence, contributions)| {
            let score = Lane::ORDER.into_iter().fold(0.0f64, |sum, lane| {
                match contributions[lane.index()] {
                    Some(contribution) => sum + parameters.term(lane, contribution.position.get()),
                    None => sum,
                }
            });
            (occurrence, score, contributions)
        })
        .collect();
    // The map yields identifier order; the stable sort keeps it among equal scores.
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    let entries = scored
        .into_iter()
        .zip(1usize..)
        .map(
            |((occurrence, score, contributions), position)| FusedEntry {
                occurrence,
                position: NonZeroUsize::new(position).expect("positions start at one"),
                score,
                contributions,
            },
        )
        .collect();
    Ok(Fused { entries, absent })
}
