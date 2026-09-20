use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// The kernel's eligibility batch size; a stage observation naming more
/// candidates than one batch is refused at construction.
pub const MAX_CANDIDATES_PER_STAGE_OBSERVATION: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    /// Produces candidates in parallel with the other sources; a required
    /// occurrence enters the chain through exactly one of them.
    Source,
    /// Receives every candidate the stages before it kept and may drop some.
    Filter,
}

/// One ordered stage list. `ALL` is production order and a stage's position in
/// it is the only ordinal the ledger uses.
pub trait Stage: Copy + Eq + fmt::Debug + 'static {
    const ALL: &'static [Self];
    fn kind(self) -> StageKind;

    fn ordinal(self) -> usize {
        Self::ALL
            .iter()
            .position(|stage| *stage == self)
            .expect("every stage is listed in ALL")
    }
}

/// The activated query route and packer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStage {
    Exact,
    Lexical,
    Dense,
    /// The kernel's admission verdicts over the lanes' hits.
    Eligibility,
    Fusion,
    /// Revalidation and the response cap.
    Selection,
    Packing,
}

pub const CHAIN_STAGES: [ChainStage; 7] = [
    ChainStage::Exact,
    ChainStage::Lexical,
    ChainStage::Dense,
    ChainStage::Eligibility,
    ChainStage::Fusion,
    ChainStage::Selection,
    ChainStage::Packing,
];

impl Stage for ChainStage {
    const ALL: &'static [Self] = &CHAIN_STAGES;

    fn kind(self) -> StageKind {
        match self {
            Self::Exact | Self::Lexical | Self::Dense => StageKind::Source,
            Self::Eligibility | Self::Fusion | Self::Selection | Self::Packing => StageKind::Filter,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// No observation of the stage exists.
    NotReached,
    /// The stage ran and its output does not name the occurrence.
    ReachedEvidenceAbsent,
    Reached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// The occurrences the stage kept.
    Candidates(BTreeSet<String>),
    /// The stage ran but its return describes no reusable state, so nothing
    /// about presence or absence can be read from it.
    Unjoinable,
}

/// One production return, as values. `sequence` orders repeated observations
/// of one stage; the highest sequence is the stage's output. `incarnation` is
/// the shell's token for the `CommitReadIncarnation` the return was judged
/// under, or `None` when the return carries none. The fields stay private so
/// the candidate bound holds for every observation a ledger sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation<S> {
    stage: S,
    sequence: u32,
    incarnation: Option<u64>,
    evidence: Evidence,
}

impl<S: Stage> Observation<S> {
    /// # Errors
    ///
    /// `CandidatesOverBound` when `candidates` exceeds
    /// [`MAX_CANDIDATES_PER_STAGE_OBSERVATION`].
    pub fn new(
        stage: S,
        sequence: u32,
        incarnation: Option<u64>,
        candidates: BTreeSet<String>,
    ) -> Result<Self, LedgerError> {
        if candidates.len() > MAX_CANDIDATES_PER_STAGE_OBSERVATION {
            return Err(LedgerError::CandidatesOverBound {
                count: candidates.len(),
                bound: MAX_CANDIDATES_PER_STAGE_OBSERVATION,
            });
        }
        Ok(Self {
            stage,
            sequence,
            incarnation,
            evidence: Evidence::Candidates(candidates),
        })
    }

    pub fn unjoinable(stage: S, sequence: u32, incarnation: Option<u64>) -> Self {
        Self {
            stage,
            sequence,
            incarnation,
            evidence: Evidence::Unjoinable,
        }
    }

    pub fn stage(&self) -> S {
        self.stage
    }

    pub fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    /// The same return attributed to `stage`; the misattribution control in
    /// the self-test needs exactly this.
    pub fn at(&self, stage: S) -> Self {
        Self {
            stage,
            ..self.clone()
        }
    }
}

/// One occurrence the task needs and the source stage it must enter through.
/// Stages before `entry`, and every other source stage, are not this
/// occurrence's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Required<S> {
    pub occurrence: String,
    pub entry: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageVerdict<S> {
    /// The earliest stage on a required occurrence's path whose output lacks it.
    FirstLoss(S),
    /// A stale occurrence reached the terminal stage; this is the stage where
    /// it first appeared.
    StaleIngress(S),
    /// Every required occurrence is present at the terminal stage and no stale
    /// one is.
    Clean,
    /// A contradiction, two incarnations, an unreached entry or terminal, or a
    /// loss behind an unjoinable stage: nothing can be attributed.
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    CandidatesOverBound {
        count: usize,
        bound: usize,
    },
    /// Two completed folds persisted under different stores; their verdicts
    /// describe different histories.
    CrossStore {
        left: String,
        right: String,
    },
}

debug_display!(LedgerError);

const UNJOINABLE: Evidence = Evidence::Unjoinable;

/// The observations of one request, keyed by (stage ordinal, sequence) so the
/// fold is the same in every arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger<S: Stage> {
    observations: BTreeMap<(usize, u32), Observation<S>>,
    /// Keys that received two different observations; each reads as unjoinable.
    contradicted: BTreeSet<(usize, u32)>,
}

impl<S: Stage> Default for Ledger<S> {
    fn default() -> Self {
        Self {
            observations: BTreeMap::new(),
            contradicted: BTreeSet::new(),
        }
    }
}

struct Indeterminate;

impl<S: Stage> Ledger<S> {
    /// Records `observation`. An identical repeat is a no-op; a different
    /// observation at the same stage and sequence contradicts the first, the
    /// key reads as unjoinable, and the fold is `Indeterminate`.
    pub fn observe(&mut self, observation: Observation<S>) {
        let key = (observation.stage.ordinal(), observation.sequence);
        match self.observations.get(&key) {
            Some(existing) if *existing == observation => {}
            Some(_) => {
                self.contradicted.insert(key);
            }
            None => {
                self.observations.insert(key, observation);
            }
        }
    }

    pub fn observations(&self) -> impl Iterator<Item = &Observation<S>> {
        self.observations.values()
    }

    /// Every incarnation token the observations carry.
    pub fn incarnations(&self) -> BTreeSet<u64> {
        self.observations
            .values()
            .filter_map(|observation| observation.incarnation)
            .collect()
    }

    /// The stage's output: its highest-sequence observation.
    fn output(&self, stage: S) -> Option<&Evidence> {
        let ordinal = stage.ordinal();
        self.observations
            .range((ordinal, 0)..=(ordinal, u32::MAX))
            .next_back()
            .map(|(key, observation)| {
                if self.contradicted.contains(key) {
                    &UNJOINABLE
                } else {
                    &observation.evidence
                }
            })
    }

    /// `None` when the stage's output is unjoinable.
    pub fn presence(&self, stage: S, occurrence: &str) -> Option<Presence> {
        match self.output(stage) {
            None => Some(Presence::NotReached),
            Some(Evidence::Unjoinable) => None,
            Some(Evidence::Candidates(candidates)) if candidates.contains(occurrence) => {
                Some(Presence::Reached)
            }
            Some(Evidence::Candidates(_)) => Some(Presence::ReachedEvidenceAbsent),
        }
    }

    /// The stages on `required`'s path up to `through`: its entry, then every
    /// later filter.
    fn path(required: &Required<S>, through: S) -> impl Iterator<Item = S> {
        let entry = required.entry;
        S::ALL
            .iter()
            .copied()
            .filter(move |stage| {
                *stage == entry
                    || (stage.ordinal() > entry.ordinal() && stage.kind() == StageKind::Filter)
            })
            .take_while(move |stage| stage.ordinal() <= through.ordinal())
    }

    /// A filter passes only what it received, so presence at a later filter
    /// proves presence at an unjoinable one before it; an unjoinable stage
    /// with no later sighting leaves any later absence unplaced.
    fn first_loss(&self, required: &Required<S>, through: S) -> Result<Option<S>, Indeterminate> {
        let mut opaque = false;
        let mut last = None;
        for stage in Self::path(required, through) {
            last = self.presence(stage, &required.occurrence);
            match last {
                None => opaque = true,
                Some(Presence::NotReached) if stage == required.entry => {
                    return Err(Indeterminate);
                }
                Some(Presence::NotReached) => {}
                Some(Presence::Reached) => opaque = false,
                Some(Presence::ReachedEvidenceAbsent) if opaque => return Err(Indeterminate),
                Some(Presence::ReachedEvidenceAbsent) => return Ok(Some(stage)),
            }
        }
        match last {
            Some(Presence::Reached) => Ok(None),
            _ => Err(Indeterminate),
        }
    }

    fn stale_ingress(&self, stale: &str, through: S) -> Result<Option<S>, Indeterminate> {
        let entered = S::ALL
            .iter()
            .copied()
            .take_while(|stage| stage.ordinal() <= through.ordinal())
            .find(|stage| self.presence(*stage, stale) == Some(Presence::Reached));
        match self.presence(through, stale) {
            Some(Presence::Reached) => Ok(entered),
            None if entered.is_some() => Err(Indeterminate),
            _ => Ok(None),
        }
    }

    /// The earliest event by ordinal, or `Indeterminate` if any item is.
    fn earliest(
        events: impl Iterator<Item = Result<Option<S>, Indeterminate>>,
    ) -> Result<Option<S>, Indeterminate> {
        let mut earliest: Option<S> = None;
        for event in events {
            if let Some(stage) = event? {
                earliest = Some(match earliest {
                    Some(seen) if seen.ordinal() <= stage.ordinal() => seen,
                    _ => stage,
                });
            }
        }
        Ok(earliest)
    }

    /// Judges the observations against `required` and `stale` with `through`
    /// as the terminal stage the run was meant to reach: the first loss of a
    /// required occurrence, the first ingress of a stale one delivered at
    /// `through`, or, at equal ordinals, the loss.
    pub fn verdict(
        &self,
        required: &[Required<S>],
        stale: &BTreeSet<String>,
        through: S,
    ) -> StageVerdict<S> {
        if !self.contradicted.is_empty() || self.incarnations().len() > 1 {
            return StageVerdict::Indeterminate;
        }
        let loss = Self::earliest(required.iter().map(|item| self.first_loss(item, through)));
        let ingress = Self::earliest(stale.iter().map(|item| self.stale_ingress(item, through)));
        match (loss, ingress) {
            (Ok(Some(loss)), Ok(Some(ingress))) if ingress.ordinal() < loss.ordinal() => {
                StageVerdict::StaleIngress(ingress)
            }
            (Ok(Some(loss)), Ok(_)) => StageVerdict::FirstLoss(loss),
            (Ok(None), Ok(Some(ingress))) => StageVerdict::StaleIngress(ingress),
            (Ok(None), Ok(None)) => StageVerdict::Clean,
            (Err(Indeterminate), _) | (_, Err(Indeterminate)) => StageVerdict::Indeterminate,
        }
    }
}

/// A closed fold. Folds compare only within one persisted store; a restart
/// that keeps `database_incarnation_id` keeps comparability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completed<S> {
    pub verdict: StageVerdict<S>,
    pub database_incarnation_id: String,
}

impl<S: Stage> Completed<S> {
    /// Whether the two folds reached the same verdict.
    ///
    /// # Errors
    ///
    /// `CrossStore` when the folds were persisted under different stores.
    pub fn agrees_with(&self, other: &Self) -> Result<bool, LedgerError> {
        if self.database_incarnation_id != other.database_incarnation_id {
            return Err(LedgerError::CrossStore {
                left: self.database_incarnation_id.clone(),
                right: other.database_incarnation_id.clone(),
            });
        }
        Ok(self.verdict == other.verdict)
    }
}
