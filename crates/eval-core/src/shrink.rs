//! Delta debugging over paired worlds. A candidate removes self-contained
//! scenario elements from both worlds; the pair compiler derives the fresh
//! arm and mapping from each candidate's own logs.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::EvaluatedSurface;
use crate::event::{EventId, EventLog, Payload};
use crate::failure_class::FailureClass;
use crate::fault::FaultEpisode;
use crate::manifest::Cut;
use crate::pairs::{PairError, PairSet, PairSetInput, Task, compile_pair_set};
use crate::reducer::reduce;

pub const SHRINK_REPORT_SCHEMA: &str = "eval-shrink/v1";
pub const SCENARIO_DIGEST_PROTOCOL: &str = "eval-scenario/v1";
/// Replay effects that may be unresolved at once. The effect issued at the
/// bound is refused, never silently dropped.
pub const MAX_OUTSTANDING_REPLAY_EFFECTS: usize = 4;

/// What a failure is, pinned before the first candidate is tried. A candidate
/// reproduces only when the replay reports this value field by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailurePredicate {
    pub oracle: String,
    pub checkpoint: Cut,
    pub profile_digest: String,
    pub witness_class: WitnessClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WitnessClass {
    /// A task failed at its cut; the class comes from the pinned truth table.
    Failure { class: FailureClass },
    /// An effect's outcome disagreed with its expectation after recovery.
    Recovery,
    /// A lane missed its bound while the healthy core was declared.
    Liveness,
    /// A never-restored resource grew past its bound.
    Sustainability,
}

/// What one replay of a candidate reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayOutcome {
    /// The replay reached the checkpoint and the oracle failed as described.
    Failed {
        predicate: FailurePredicate,
    },
    /// The replay reached the checkpoint and the oracle passed.
    Passed,
    Unknown {
        reason: UnknownReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    ReplayBudgetExhausted,
    EffectUnanswered,
    ChildExitedBeforeBarrier,
    ReadBackFailed,
    Cancelled,
}

impl UnknownReason {
    pub const ALL: [Self; 5] = [
        Self::ReplayBudgetExhausted,
        Self::EffectUnanswered,
        Self::ChildExitedBeforeBarrier,
        Self::ReadBackFailed,
        Self::Cancelled,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateVerdict {
    Reproduced,
    /// The replay completed and the oracle passed at the checkpoint.
    NotReproduced,
    /// The replay failed, but not the pinned failure.
    Slipped {
        observed: FailurePredicate,
    },
    Unknown {
        reason: UnknownReason,
    },
    /// The pair compiler refused the candidate; nothing was replayed.
    InvalidPair {
        reason: String,
    },
}

/// `Unknown` stays unknown for every reason; `NotReproduced` needs a completed
/// replay whose oracle passed.
pub fn classify_replay(expected: &FailurePredicate, outcome: &ReplayOutcome) -> CandidateVerdict {
    match outcome {
        ReplayOutcome::Failed { predicate } if predicate == expected => {
            CandidateVerdict::Reproduced
        }
        ReplayOutcome::Failed { predicate } => CandidateVerdict::Slipped {
            observed: predicate.clone(),
        },
        ReplayOutcome::Passed => CandidateVerdict::NotReproduced,
        ReplayOutcome::Unknown { reason } => CandidateVerdict::Unknown { reason: *reason },
    }
}

/// The parent's shrinking order, restricted to what a scenario holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transformation {
    FaultEpisodeRemoval,
    EventDeletion,
}

impl Transformation {
    pub const ORDER: [Self; 2] = [Self::FaultEpisodeRemoval, Self::EventDeletion];
}

/// One self-contained thing a candidate may delete.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Element {
    Episode { id: String },
    Event { id: EventId },
}

impl Element {
    fn transformation(&self) -> Transformation {
        match self {
            Self::Episode { .. } => Transformation::FaultEpisodeRemoval,
            Self::Event { .. } => Transformation::EventDeletion,
        }
    }
}

/// The semantic scenario a witness replays: both authored histories, the
/// tasks over them, and the fault episodes armed during the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub surface: EvaluatedSurface,
    pub recency_bound: Option<NonZeroU32>,
    pub aged: EventLog,
    pub natural_fresh: EventLog,
    pub tasks: Vec<Task>,
    pub episodes: Vec<FaultEpisode>,
}

impl Scenario {
    /// Every deletable element, in the parent's transformation order.
    pub fn elements(&self) -> Vec<Element> {
        let episodes = self.episodes.iter().map(|episode| Element::Episode {
            id: episode.id.clone(),
        });
        let events = self
            .aged
            .events
            .iter()
            .chain(&self.natural_fresh.events)
            .map(|event| Element::Event {
                id: event.id.clone(),
            });
        episodes.chain(events).collect()
    }

    pub fn without(&self, deleted: &BTreeSet<Element>) -> Self {
        let mut candidate = self.clone();
        candidate.episodes.retain(|episode| {
            !deleted.contains(&Element::Episode {
                id: episode.id.clone(),
            })
        });
        for element in deleted {
            if let Element::Event { id } = element {
                candidate.aged = candidate.aged.without(id);
                candidate.natural_fresh = candidate.natural_fresh.without(id);
            }
        }
        candidate
    }

    /// Recompiles the pair set from this scenario's own logs.
    pub fn compile(&self, fixture: &Value) -> Result<PairSet, PairError> {
        compile_pair_set(PairSetInput {
            surface: self.surface,
            declared_bound: self.recency_bound,
            aged: &self.aged,
            natural_fresh: &self.natural_fresh,
            fixture,
            tasks: &self.tasks,
        })
    }

    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("scenario serializes");
        protocol_digest(SCENARIO_DIGEST_PROTOCOL, &value).expect("scenario is canonical")
    }

    /// How many aged events each payload kind contributes; a kind still
    /// counted above one in a 1-minimal scenario is a count that triggers the
    /// failure.
    pub fn multiplicities(&self) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for event in &self.aged.events {
            let payload = serde_json::to_value(&event.payload).expect("payload serializes");
            let kind = payload["kind"]
                .as_str()
                .expect("payload is tagged")
                .to_string();
            *counts.entry(kind).or_insert(0) += 1;
        }
        counts
    }
}

/// An oracle a replay evaluates over the compiled pair set at the pinned cut.
/// `RequiredCommits` is the evaluator's own planted defect: it fails from
/// `failing_at` required commits and changes class from `slipping_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Oracle {
    RequiredCommits { failing_at: u32, slipping_at: u32 },
}

impl Oracle {
    pub fn name(&self) -> String {
        match self {
            Self::RequiredCommits { .. } => "planted:required-commits".to_string(),
        }
    }

    /// The truth is reduced from the aged log at the first task's cut, which
    /// the compiler has already made every task's cut.
    pub fn evaluate(
        &self,
        set: &PairSet,
        fixture: &Value,
        checkpoint: Cut,
        profile_digest: &str,
    ) -> ReplayOutcome {
        let Some(pair) = set.pairs.first() else {
            return ReplayOutcome::Passed;
        };
        let Ok(truth) = reduce(&set.aged, fixture, &pair.task.query) else {
            return ReplayOutcome::Unknown {
                reason: UnknownReason::ReadBackFailed,
            };
        };
        let Self::RequiredCommits {
            failing_at,
            slipping_at,
        } = self;
        let commits = set
            .aged
            .events
            .iter()
            .filter(|event| {
                truth.required.contains(&event.id)
                    && matches!(event.payload, Payload::Commit { .. })
            })
            .count();
        let class = if commits >= *slipping_at as usize {
            FailureClass::Interference
        } else if commits >= *failing_at as usize {
            FailureClass::DurableState
        } else {
            return ReplayOutcome::Passed;
        };
        ReplayOutcome::Failed {
            predicate: FailurePredicate {
                oracle: self.name(),
                checkpoint,
                profile_digest: profile_digest.to_string(),
                witness_class: WitnessClass::Failure { class },
            },
        }
    }
}

/// Replay effects issued and not yet resolved, keyed by a receipt key. A
/// retry keeps its key; a cancellation resolves to `Unknown`; reading an
/// outstanding effect is refused.
#[derive(Debug, Clone)]
pub struct ReplayEffects {
    bound: usize,
    outstanding: BTreeMap<String, u32>,
    resolved: BTreeMap<String, ReplayOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayRefused {
    OutstandingBound {
        bound: usize,
    },
    UnknownKey {
        key: String,
    },
    /// The effect has not answered; classifying it now would be premature.
    Outstanding {
        key: String,
    },
    AlreadyResolved {
        key: String,
    },
}

debug_display!(ReplayRefused);

impl ReplayEffects {
    pub fn new(bound: usize) -> Self {
        Self {
            bound,
            outstanding: BTreeMap::new(),
            resolved: BTreeMap::new(),
        }
    }

    pub fn issue(&mut self, key: &str) -> Result<(), ReplayRefused> {
        if self.resolved.contains_key(key) || self.outstanding.contains_key(key) {
            return Err(ReplayRefused::AlreadyResolved {
                key: key.to_string(),
            });
        }
        if self.outstanding.len() >= self.bound {
            return Err(ReplayRefused::OutstandingBound { bound: self.bound });
        }
        self.outstanding.insert(key.to_string(), 1);
        Ok(())
    }

    /// Another attempt under the same key; returns the attempt count.
    pub fn retry(&mut self, key: &str) -> Result<u32, ReplayRefused> {
        let refused = self.absent(key);
        let attempts = self.outstanding.get_mut(key).ok_or(refused)?;
        *attempts += 1;
        Ok(*attempts)
    }

    pub fn cancel(&mut self, key: &str) -> Result<(), ReplayRefused> {
        self.resolve(
            key,
            ReplayOutcome::Unknown {
                reason: UnknownReason::Cancelled,
            },
        )
    }

    pub fn resolve(&mut self, key: &str, outcome: ReplayOutcome) -> Result<(), ReplayRefused> {
        self.outstanding
            .remove(key)
            .ok_or_else(|| self.absent(key))?;
        self.resolved.insert(key.to_string(), outcome);
        Ok(())
    }

    pub fn outcome(&self, key: &str) -> Result<&ReplayOutcome, ReplayRefused> {
        if self.outstanding.contains_key(key) {
            return Err(ReplayRefused::Outstanding {
                key: key.to_string(),
            });
        }
        self.resolved
            .get(key)
            .ok_or_else(|| ReplayRefused::UnknownKey {
                key: key.to_string(),
            })
    }

    fn absent(&self, key: &str) -> ReplayRefused {
        if self.resolved.contains_key(key) {
            ReplayRefused::AlreadyResolved {
                key: key.to_string(),
            }
        } else {
            ReplayRefused::UnknownKey {
                key: key.to_string(),
            }
        }
    }
}

/// One candidate to replay.
#[derive(Debug, Clone, Copy)]
pub struct ReplayRequest<'a> {
    /// The receipt key: the candidate scenario's digest.
    pub key: &'a str,
    pub scenario: &'a Scenario,
    pub set: &'a PairSet,
    pub predicate: &'a FailurePredicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecord {
    pub scenario_digest: String,
    pub deleted: BTreeSet<Element>,
    pub verdict: CandidateVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Minimality {
    /// Every single-element deletion under each named transformation was
    /// tried against the minimized scenario and rejected.
    OneMinimal {
        transformations: Vec<Transformation>,
    },
    NotEstablished {
        reason: NotEstablishedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotEstablishedReason {
    ReplayBudgetExhausted,
    /// A single deletion answered `Unknown`; the scenario may not be minimal.
    UnknownCandidates {
        count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShrinkReport {
    pub schema: String,
    pub predicate: FailurePredicate,
    pub original_digest: String,
    pub minimized_digest: String,
    pub deleted: BTreeSet<Element>,
    pub candidates: Vec<CandidateRecord>,
    pub replays: u64,
    pub unknown_candidates: u64,
    pub minimality: Minimality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShrinkRefused {
    /// The original scenario itself did not reproduce the pinned predicate.
    OriginalNotReproduced {
        verdict: CandidateVerdict,
    },
    Replay(ReplayRefused),
}

debug_display!(ShrinkRefused);

struct Driver<'a> {
    fixture: &'a Value,
    predicate: &'a FailurePredicate,
    max_replays: u64,
    replay: &'a mut dyn FnMut(ReplayRequest<'_>) -> ReplayOutcome,
    effects: ReplayEffects,
    verdicts: BTreeMap<String, CandidateVerdict>,
    candidates: Vec<CandidateRecord>,
    replays: u64,
}

impl Driver<'_> {
    /// Tries the candidate; a digest already answered is not replayed twice.
    fn test(
        &mut self,
        candidate: &Scenario,
        deleted: &BTreeSet<Element>,
    ) -> Result<CandidateVerdict, ShrinkRefused> {
        let digest = candidate.digest();
        let verdict = match self.verdicts.get(&digest) {
            Some(verdict) => verdict.clone(),
            None => {
                let verdict = self.replay(candidate, &digest)?;
                self.verdicts.insert(digest.clone(), verdict.clone());
                verdict
            }
        };
        self.candidates.push(CandidateRecord {
            scenario_digest: digest,
            deleted: deleted.clone(),
            verdict: verdict.clone(),
        });
        Ok(verdict)
    }

    fn replay(
        &mut self,
        candidate: &Scenario,
        key: &str,
    ) -> Result<CandidateVerdict, ShrinkRefused> {
        let set = match candidate.compile(self.fixture) {
            Ok(set) => set,
            Err(error) => {
                return Ok(CandidateVerdict::InvalidPair {
                    reason: format!("{error:?}"),
                });
            }
        };
        if self.replays >= self.max_replays {
            return Ok(CandidateVerdict::Unknown {
                reason: UnknownReason::ReplayBudgetExhausted,
            });
        }
        self.replays += 1;
        self.effects.issue(key).map_err(ShrinkRefused::Replay)?;
        let outcome = (self.replay)(ReplayRequest {
            key,
            scenario: candidate,
            set: &set,
            predicate: self.predicate,
        });
        self.effects
            .resolve(key, outcome)
            .map_err(ShrinkRefused::Replay)?;
        let outcome = self.effects.outcome(key).map_err(ShrinkRefused::Replay)?;
        Ok(classify_replay(self.predicate, outcome))
    }
}

/// Shrinks `original` until no single deletion under any tried transformation
/// still reproduces `predicate`, or the replay budget runs out. The returned
/// scenario reproduced the predicate on its last replay.
pub fn shrink(
    original: &Scenario,
    fixture: &Value,
    predicate: &FailurePredicate,
    max_replays: u64,
    replay: &mut dyn FnMut(ReplayRequest<'_>) -> ReplayOutcome,
) -> Result<(Scenario, ShrinkReport), ShrinkRefused> {
    let mut driver = Driver {
        fixture,
        predicate,
        max_replays,
        replay,
        effects: ReplayEffects::new(MAX_OUTSTANDING_REPLAY_EFFECTS),
        verdicts: BTreeMap::new(),
        candidates: Vec::new(),
        replays: 0,
    };
    let none = BTreeSet::new();
    let verdict = driver.test(original, &none)?;
    if verdict != CandidateVerdict::Reproduced {
        return Err(ShrinkRefused::OriginalNotReproduced { verdict });
    }
    let mut deleted = BTreeSet::new();
    let mut tried = Vec::new();
    for transformation in Transformation::ORDER {
        let kept: Vec<Element> = original
            .without(&deleted)
            .elements()
            .into_iter()
            .filter(|element| element.transformation() == transformation)
            .collect();
        if kept.is_empty() {
            continue;
        }
        tried.push(transformation);
        deleted = ddmin(&mut driver, original, deleted, kept)?;
    }
    let minimality = one_minimality(&mut driver, original, &mut deleted, &tried)?;
    let minimized = original.without(&deleted);
    let unknown_candidates = driver
        .candidates
        .iter()
        .filter(|record| matches!(record.verdict, CandidateVerdict::Unknown { .. }))
        .count() as u64;
    let report = ShrinkReport {
        schema: SHRINK_REPORT_SCHEMA.to_string(),
        predicate: predicate.clone(),
        original_digest: original.digest(),
        minimized_digest: minimized.digest(),
        deleted,
        candidates: driver.candidates,
        replays: driver.replays,
        unknown_candidates,
        minimality,
    };
    Ok((minimized, report))
}

/// Zeller's ddmin over `kept`, holding `deleted` from earlier transformations
/// fixed. Only `Reproduced` shrinks; `Unknown` stays in the set.
fn ddmin(
    driver: &mut Driver<'_>,
    original: &Scenario,
    deleted: BTreeSet<Element>,
    mut kept: Vec<Element>,
) -> Result<BTreeSet<Element>, ShrinkRefused> {
    let all: BTreeSet<Element> = kept.iter().cloned().collect();
    let mut n = 2;
    while !kept.is_empty() {
        let chunk = kept.len().div_ceil(n);
        let subsets: Vec<Vec<Element>> = kept.chunks(chunk).map(<[Element]>::to_vec).collect();
        let mut reduced = None;
        'subsets: for (index, subset) in subsets.iter().enumerate() {
            let complement: Vec<Element> = subsets
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .flat_map(|(_, s)| s.iter().cloned())
                .collect();
            for (candidate, next_n) in [(subset.clone(), 2), (complement, n.max(3) - 1)] {
                if candidate.len() == kept.len() {
                    continue;
                }
                let drop: BTreeSet<Element> = deleted
                    .iter()
                    .cloned()
                    .chain(all.iter().filter(|e| !candidate.contains(e)).cloned())
                    .collect();
                let verdict = driver.test(&original.without(&drop), &drop)?;
                if verdict == CandidateVerdict::Reproduced {
                    reduced = Some((candidate, next_n));
                    break 'subsets;
                }
            }
        }
        match reduced {
            Some((candidate, next_n)) => {
                kept = candidate;
                n = next_n.min(kept.len().max(2));
            }
            None if n >= kept.len() => break,
            None => n = (2 * n).min(kept.len()),
        }
    }
    Ok(deleted
        .into_iter()
        .chain(all.into_iter().filter(|e| !kept.contains(e)))
        .collect())
}

/// Tries every single deletion against the minimized scenario until a full
/// pass rejects them all. A rejection under every tried transformation is
/// 1-minimality; an `Unknown` is not.
fn one_minimality(
    driver: &mut Driver<'_>,
    original: &Scenario,
    deleted: &mut BTreeSet<Element>,
    tried: &[Transformation],
) -> Result<Minimality, ShrinkRefused> {
    loop {
        let mut unknown = 0;
        let mut reduced = false;
        for element in original.without(deleted).elements() {
            let mut drop = deleted.clone();
            drop.insert(element.clone());
            let verdict = driver.test(&original.without(&drop), &drop)?;
            match verdict {
                CandidateVerdict::Reproduced => {
                    *deleted = drop;
                    reduced = true;
                    break;
                }
                CandidateVerdict::Unknown {
                    reason: UnknownReason::ReplayBudgetExhausted,
                } => {
                    return Ok(Minimality::NotEstablished {
                        reason: NotEstablishedReason::ReplayBudgetExhausted,
                    });
                }
                CandidateVerdict::Unknown { .. } => unknown += 1,
                _ => {}
            }
        }
        if reduced {
            continue;
        }
        return Ok(if unknown == 0 {
            Minimality::OneMinimal {
                transformations: tried.to_vec(),
            }
        } else {
            Minimality::NotEstablished {
                reason: NotEstablishedReason::UnknownCandidates { count: unknown },
            }
        });
    }
}
