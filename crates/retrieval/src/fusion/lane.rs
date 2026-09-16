use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;

use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;

use super::{IdentityRefusal, OccurrenceId};

/// A closed set keeps a lane from being declared under two names; `ORDER` is the one summation order fusion uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    Exact,
    Lexical,
    Dense,
}

impl Lane {
    pub const ORDER: [Lane; 3] = [Lane::Exact, Lane::Lexical, Lane::Dense];

    pub fn code(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Lexical => "lexical",
            Self::Dense => "dense",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Exact => 0,
            Self::Lexical => 1,
            Self::Dense => 2,
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Each variant names its lane so a score can only be compared within that lane; there is no ordering across variants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawScore {
    /// The exact lane returns a set without a rank, so every member ties and the declared ranking is occurrence-identifier order.
    Exact,
    Lexical(f64),
    Dense(f64),
}

impl RawScore {
    pub fn lane(self) -> Lane {
        match self {
            Self::Exact => Lane::Exact,
            Self::Lexical(_) => Lane::Lexical,
            Self::Dense(_) => Lane::Dense,
        }
    }

    fn is_finite(self) -> bool {
        match self {
            Self::Exact => true,
            Self::Lexical(value) | Self::Dense(value) => value.is_finite(),
        }
    }

    /// Lower FTS5 rank and higher dense similarity come first; `consolidate` admits one score kind per ranking, so the mixed arm cannot run.
    fn better_first(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Exact, Self::Exact) => Ordering::Equal,
            (Self::Lexical(left), Self::Lexical(right)) => left.total_cmp(&right),
            (Self::Dense(left), Self::Dense(right)) => right.total_cmp(&left),
            _ => unreachable!("a lane ranking holds one score kind"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneHit {
    pub occurrence: OccurrenceId,
    pub raw_score: RawScore,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneEntry {
    occurrence: OccurrenceId,
    position: NonZeroUsize,
    raw_score: RawScore,
}

impl LaneEntry {
    pub fn occurrence(&self) -> &OccurrenceId {
        &self.occurrence
    }

    pub fn position(&self) -> NonZeroUsize {
        self.position
    }

    pub fn raw_score(&self) -> RawScore {
        self.raw_score
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneRanking {
    lane: Lane,
    encoding_version: u8,
    entries: Vec<LaneEntry>,
}

impl LaneRanking {
    /// An occurrence keeps its best lane score; score then occurrence-identifier byte order fixes every position, so probe or generation order and duplication cannot change the ranking.
    /// A stamp other than the version this build mints is refused, because identifiers minted under another encoding would not name the same occurrences.
    pub fn consolidate(
        lane: Lane,
        encoding_version: u8,
        hits: impl IntoIterator<Item = LaneHit>,
    ) -> Result<Self, IdentityRefusal> {
        if encoding_version != OCCURRENCE_ENCODING_VERSION {
            return Err(IdentityRefusal::EncodingVersion {
                lane,
                expected: OCCURRENCE_ENCODING_VERSION,
                found: encoding_version,
            });
        }
        let mut best: BTreeMap<OccurrenceId, RawScore> = BTreeMap::new();
        for hit in hits {
            if hit.raw_score.lane() != lane {
                return Err(IdentityRefusal::ScoreLaneMismatch {
                    declared: lane,
                    found: hit.raw_score.lane(),
                });
            }
            if !hit.raw_score.is_finite() {
                return Err(IdentityRefusal::NonFiniteScore);
            }
            best.entry(hit.occurrence)
                .and_modify(|incumbent| {
                    if hit.raw_score.better_first(*incumbent) == Ordering::Less {
                        *incumbent = hit.raw_score;
                    }
                })
                .or_insert(hit.raw_score);
        }
        let mut ordered: Vec<(OccurrenceId, RawScore)> = best.into_iter().collect();
        // The map yields identifier order; the stable sort preserves that order among equal scores.
        ordered.sort_by(|(_, left), (_, right)| left.better_first(*right));
        let entries = ordered
            .into_iter()
            .zip(1usize..)
            .map(|((occurrence, raw_score), position)| LaneEntry {
                occurrence,
                position: NonZeroUsize::new(position).expect("positions start at one"),
                raw_score,
            })
            .collect();
        Ok(Self {
            lane,
            encoding_version,
            entries,
        })
    }

    pub fn lane(&self) -> Lane {
        self.lane
    }

    pub fn encoding_version(&self) -> u8 {
        self.encoding_version
    }

    pub fn entries(&self) -> &[LaneEntry] {
        &self.entries
    }
}

/// One slot per lane in [`Lane::ORDER`], so a second ranking for a lane is refused rather than summed twice.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredLanes {
    slots: [Option<LaneRanking>; Lane::ORDER.len()],
}

impl DeclaredLanes {
    pub fn admit(rankings: impl IntoIterator<Item = LaneRanking>) -> Result<Self, IdentityRefusal> {
        let mut slots: [Option<LaneRanking>; Lane::ORDER.len()] = Default::default();
        for ranking in rankings {
            let slot = &mut slots[ranking.lane.index()];
            if slot.is_some() {
                return Err(IdentityRefusal::DuplicateLane(ranking.lane));
            }
            *slot = Some(ranking);
        }
        Ok(Self { slots })
    }

    pub fn rankings(&self) -> impl Iterator<Item = &LaneRanking> {
        self.slots.iter().flatten()
    }

    pub fn lane(&self, lane: Lane) -> Option<&LaneRanking> {
        self.slots[lane.index()].as_ref()
    }
}
