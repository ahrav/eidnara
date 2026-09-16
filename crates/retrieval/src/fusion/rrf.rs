use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::num::NonZeroUsize;

use super::{DeclaredLanes, Lane, OccurrenceId, RawScore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParameterRefusal {
    #[error("lane {0} has a weight that is negative or not finite")]
    Weight(Lane),
    #[error("k is not positive and finite")]
    K,
    /// Finite weights and a finite k can still sum to infinity at rank one, so the maximum score is checked before any occurrence is scored.
    #[error("the maximum fused score is not finite under these parameters")]
    SumOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the fused union exceeds the bound of {bound} occurrences")]
pub struct UnionExceeded {
    pub bound: usize,
}

/// One weight per lane, named so a call site cannot swap two lanes' weights without the compiler noticing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneWeights {
    pub exact: f64,
    pub lexical: f64,
    pub dense: f64,
}

impl LaneWeights {
    fn by_lane(self, lane: Lane) -> f64 {
        match lane {
            Lane::Exact => self.exact,
            Lane::Lexical => self.lexical,
            Lane::Dense => self.dense,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionParameters {
    weights: [f64; Lane::ORDER.len()],
    k: f64,
}

impl FusionParameters {
    /// A negative-zero weight is admitted as zero, so an all-zero parameter set has one spelling and every score it produces is `+0.0`.
    pub fn new(weights: LaneWeights, k: f64) -> Result<Self, ParameterRefusal> {
        let mut admitted = Lane::ORDER.map(|lane| weights.by_lane(lane));
        for (lane, weight) in Lane::ORDER.into_iter().zip(&mut admitted) {
            if !weight.is_finite() || *weight < 0.0 {
                return Err(ParameterRefusal::Weight(lane));
            }
            *weight += 0.0;
        }
        if !k.is_finite() || k <= 0.0 {
            return Err(ParameterRefusal::K);
        }
        let parameters = Self {
            weights: admitted,
            k,
        };
        let maximum = Lane::ORDER
            .into_iter()
            .fold(0.0f64, |sum, lane| sum + parameters.term(lane, 1));
        if !maximum.is_finite() {
            return Err(ParameterRefusal::SumOverflow);
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
    position: NonZeroUsize,
    raw_score: RawScore,
}

impl LaneContribution {
    /// The position consolidation assigned in the lane, unchanged by fusion.
    pub fn position(&self) -> NonZeroUsize {
        self.position
    }

    pub fn raw_score(&self) -> RawScore {
        self.raw_score
    }
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

    /// `None` when the lane did not rank the occurrence.
    pub fn lane(&self, lane: Lane) -> Option<LaneContribution> {
        self.contributions[lane.index()]
    }
}

/// A `Fused` exposes no path back to lane rankings, so no consumer can rescore it; [`Fused::filter`] removes entries without touching a survivor's position or score.
#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    entries: Vec<FusedEntry>,
    undeclared: [bool; Lane::ORDER.len()],
}

impl Fused {
    pub fn entries(&self) -> &[FusedEntry] {
        &self.entries
    }

    /// A lane that was not declared contributed zero to every score; whether it was unavailable, busy, or never requested is the route's knowledge, not fusion's.
    pub fn undeclared_lanes(&self) -> impl Iterator<Item = Lane> + '_ {
        Lane::ORDER
            .into_iter()
            .filter(move |lane| self.undeclared[lane.index()])
    }

    #[must_use = "filter consumes the ranking and returns the survivors"]
    pub fn filter(mut self, keep: impl FnMut(&FusedEntry) -> bool) -> Self {
        self.entries.retain(keep);
        self
    }
}

/// Terms are summed in [`Lane::ORDER`] in `f64`; an undeclared lane or one that did not rank an occurrence adds nothing; order is descending score, then occurrence-identifier bytes.
/// The union is refused as soon as it would hold more than `bound` occurrences, before the excess is materialized.
pub fn fuse(
    lanes: DeclaredLanes,
    parameters: &FusionParameters,
    bound: NonZeroUsize,
) -> Result<Fused, UnionExceeded> {
    let undeclared = Lane::ORDER.map(|lane| lanes.lane(lane).is_none());
    let mut union: BTreeMap<OccurrenceId, [Option<LaneContribution>; Lane::ORDER.len()]> =
        BTreeMap::new();
    for ranking in lanes.rankings() {
        let lane = ranking.lane();
        for entry in ranking.entries() {
            let size = union.len();
            let slot = match union.entry(*entry.occurrence()) {
                Entry::Vacant(_) if size >= bound.get() => {
                    return Err(UnionExceeded { bound: bound.get() });
                }
                Entry::Vacant(vacant) => vacant.insert([None; Lane::ORDER.len()]),
                Entry::Occupied(occupied) => occupied.into_mut(),
            };
            slot[lane.index()] = Some(LaneContribution {
                position: entry.position(),
                raw_score: entry.raw_score(),
            });
        }
    }
    let mut entries: Vec<FusedEntry> = union
        .into_iter()
        .map(|(occurrence, contributions)| {
            let score = Lane::ORDER.into_iter().fold(0.0f64, |sum, lane| {
                match contributions[lane.index()] {
                    Some(contribution) => sum + parameters.term(lane, contribution.position.get()),
                    None => sum,
                }
            });
            FusedEntry {
                occurrence,
                position: NonZeroUsize::MIN,
                score,
                contributions,
            }
        })
        .collect();
    entries.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.occurrence.cmp(&right.occurrence))
    });
    for (entry, position) in entries.iter_mut().zip(1usize..) {
        entry.position = NonZeroUsize::new(position).expect("positions start at one");
    }
    Ok(Fused {
        entries,
        undeclared,
    })
}
