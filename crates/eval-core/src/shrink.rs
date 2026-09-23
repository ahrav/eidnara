//! Delta debugging over paired worlds. A candidate removes self-contained
//! scenario elements from a named world; the pair compiler derives the fresh
//! arm and mapping from each candidate's own logs.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use context_core::canonical_json::{
    ContractError, canonical_json_encode, is_lower_hex, protocol_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::EvaluatedSurface;
use crate::event::{EventId, EventLog, Payload};
use crate::failure_class::FailureClass;
use crate::fault::{EpisodeRefused, FaultEpisode, Lane, validate_episodes};
use crate::manifest::Cut;
use crate::pairs::{PairError, PairSet, PairSetInput, Task, compile_pair_set};
use crate::reducer::Truth;

pub const SHRINK_REPORT_SCHEMA: &str = "eval-shrink/v1";
pub const SCENARIO_DIGEST_PROTOCOL: &str = "eval-scenario/v1";
/// Replay effects a shell may leave unresolved at once. The effect issued at
/// the bound is refused, never silently dropped.
pub const MAX_OUTSTANDING_REPLAY_EFFECTS: usize = 4;
/// Fresh processes one budgeted replay may launch, the first attempt
/// included; the retry past it is refused. A shell therefore launches at
/// most `max_replays * MAX_REPLAY_ATTEMPTS` processes.
pub const MAX_REPLAY_ATTEMPTS: u32 = 3;

/// What a failure is, pinned before the first candidate is tried. A candidate
/// reproduces only when the replay reports this value field by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailurePredicate {
    pub oracle: Oracle,
    pub checkpoint: Cut,
    /// `RunProfile::digest`: 64 lowercase hex characters.
    pub profile_digest: String,
    pub witness_class: WitnessClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateRefused {
    Oracle(OracleRefused),
    /// The named field cannot be what it claims to pin.
    Malformed {
        field: &'static str,
    },
}

debug_display!(PredicateRefused);

impl FailurePredicate {
    /// A valid oracle and a profile digest that could name a run profile.
    pub fn validate(&self) -> Result<(), PredicateRefused> {
        self.oracle.validate().map_err(PredicateRefused::Oracle)?;
        if !is_lower_hex(&self.profile_digest, 64) {
            return Err(PredicateRefused::Malformed {
                field: "profile_digest",
            });
        }
        Ok(())
    }
}

/// What failed, and to what. Each variant names its subject, so a candidate
/// under which the original subject passes and another fails the same way
/// is a different predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WitnessClass {
    /// The named task failed at its cut; the class comes from the pinned
    /// truth table.
    Failure { task: String, class: FailureClass },
    /// The keyed effect's outcome disagreed with its expectation after
    /// recovery.
    Recovery { effect: String },
    /// The lane missed its bound while the healthy core was declared.
    Liveness { lane: Lane },
    /// The never-restored resource grew past its bound.
    Sustainability { resource: String },
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
    /// The pair compiler refused the candidate with the named refusal;
    /// nothing was replayed.
    InvalidPair {
        refusal: String,
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

/// The two authored histories. Their raw event ids overlap, so an event is
/// named by its history as well as its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum History {
    Aged,
    NaturalFresh,
}

/// One self-contained thing a candidate may delete.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Element {
    Episode { id: String },
    Event { history: History, id: EventId },
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
        let events = |history, log: &EventLog| {
            log.events
                .iter()
                .map(|event| Element::Event {
                    history,
                    id: event.id.clone(),
                })
                .collect::<Vec<_>>()
        };
        episodes
            .chain(events(History::Aged, &self.aged))
            .chain(events(History::NaturalFresh, &self.natural_fresh))
            .collect()
    }

    /// Applies the whole deletion set with one pass over each list, so a
    /// candidate costs the same whether it deletes one element or most.
    pub fn without(&self, deleted: &BTreeSet<Element>) -> Self {
        let mut episodes = BTreeSet::new();
        let mut aged = BTreeSet::new();
        let mut natural_fresh = BTreeSet::new();
        for element in deleted {
            match element {
                Element::Episode { id } => episodes.insert(id),
                Element::Event {
                    history: History::Aged,
                    id,
                } => aged.insert(id),
                Element::Event {
                    history: History::NaturalFresh,
                    id,
                } => natural_fresh.insert(id),
            };
        }
        let mut candidate = self.clone();
        if !episodes.is_empty() {
            candidate
                .episodes
                .retain(|episode| !episodes.contains(&episode.id));
        }
        if !aged.is_empty() {
            candidate.aged.remove_where(|id| aged.contains(id));
        }
        if !natural_fresh.is_empty() {
            candidate
                .natural_fresh
                .remove_where(|id| natural_fresh.contains(id));
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
}

/// An oracle a replay evaluates over the compiled pair set and the aged truth
/// at the pinned cut. The predicate pins the whole value, parameters included,
/// so a replay under other parameters is a different predicate.
/// `RequiredCommits` is the evaluator's own planted defect: it fails from
/// `failing_at` required commits and changes class from `slipping_at`, which
/// must not be below `failing_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Oracle {
    RequiredCommits { failing_at: u32, slipping_at: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleRefused {
    InvertedThresholds { failing_at: u32, slipping_at: u32 },
}

debug_display!(OracleRefused);

impl Oracle {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RequiredCommits { .. } => "planted:required-commits",
        }
    }

    pub fn validate(&self) -> Result<(), OracleRefused> {
        match *self {
            Self::RequiredCommits {
                failing_at,
                slipping_at,
            } if slipping_at < failing_at => Err(OracleRefused::InvertedThresholds {
                failing_at,
                slipping_at,
            }),
            Self::RequiredCommits { .. } => Ok(()),
        }
    }

    /// Evaluates over the aged arm and the truth reduced for `task`; a
    /// failure names that task.
    pub fn evaluate(
        &self,
        set: &PairSet,
        task: &str,
        truth: &Truth,
        checkpoint: Cut,
        profile_digest: &str,
    ) -> ReplayOutcome {
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
                oracle: self.clone(),
                checkpoint,
                profile_digest: profile_digest.to_string(),
                witness_class: WitnessClass::Failure {
                    task: task.to_string(),
                    class,
                },
            },
        }
    }
}

/// Replay effects a shell has issued and not yet resolved, keyed by a receipt
/// key. A retry keeps its key and advances its attempt; only the current
/// attempt may resolve, so a superseded process's late answer is refused; a
/// cancellation resolves to `Unknown`; reading an outstanding effect is
/// refused. The outstanding set is bounded at `MAX_OUTSTANDING_REPLAY_EFFECTS`
/// and attempts per key at `MAX_REPLAY_ATTEMPTS`; neither is configurable.
#[derive(Debug, Clone, Default)]
pub struct ReplayEffects {
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
    /// The key has used every attempt `MAX_REPLAY_ATTEMPTS` allows.
    AttemptsExhausted {
        key: String,
        attempts: u32,
    },
    /// The answer came from an attempt a retry superseded.
    StaleAttempt {
        key: String,
        attempt: u32,
        current: u32,
    },
}

debug_display!(ReplayRefused);

impl ReplayEffects {
    /// Issues the first attempt and returns its number, `1`.
    pub fn issue(&mut self, key: &str) -> Result<u32, ReplayRefused> {
        self.absent(key)?;
        if self.outstanding.len() >= MAX_OUTSTANDING_REPLAY_EFFECTS {
            return Err(ReplayRefused::OutstandingBound {
                bound: MAX_OUTSTANDING_REPLAY_EFFECTS,
            });
        }
        self.outstanding.insert(key.to_string(), 1);
        Ok(1)
    }

    /// Another attempt under the same key; returns its number. The attempt
    /// it supersedes can no longer resolve the key.
    pub fn retry(&mut self, key: &str) -> Result<u32, ReplayRefused> {
        self.present(key)?;
        let attempts = self.outstanding.get_mut(key).expect("checked present");
        if *attempts >= MAX_REPLAY_ATTEMPTS {
            return Err(ReplayRefused::AttemptsExhausted {
                key: key.to_string(),
                attempts: *attempts,
            });
        }
        *attempts += 1;
        Ok(*attempts)
    }

    /// Resolves `attempt` to `Unknown { cancelled }`; a superseded attempt's
    /// cancellation is refused like its answer.
    pub fn cancel(&mut self, key: &str, attempt: u32) -> Result<(), ReplayRefused> {
        self.resolve(
            key,
            attempt,
            ReplayOutcome::Unknown {
                reason: UnknownReason::Cancelled,
            },
        )
    }

    /// Resolves the key with `attempt`'s answer; an attempt a retry
    /// superseded is refused so the newest process's answer is the one read.
    pub fn resolve(
        &mut self,
        key: &str,
        attempt: u32,
        outcome: ReplayOutcome,
    ) -> Result<(), ReplayRefused> {
        self.present(key)?;
        let current = self.outstanding[key];
        if attempt != current {
            return Err(ReplayRefused::StaleAttempt {
                key: key.to_string(),
                attempt,
                current,
            });
        }
        self.outstanding.remove(key);
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

    /// `Ok` when the key names an outstanding effect.
    fn present(&self, key: &str) -> Result<(), ReplayRefused> {
        if self.outstanding.contains_key(key) {
            Ok(())
        } else if self.resolved.contains_key(key) {
            Err(ReplayRefused::AlreadyResolved {
                key: key.to_string(),
            })
        } else {
            Err(ReplayRefused::UnknownKey {
                key: key.to_string(),
            })
        }
    }

    /// `Ok` when the key names no effect at all.
    fn absent(&self, key: &str) -> Result<(), ReplayRefused> {
        match self.present(key) {
            Ok(()) => Err(ReplayRefused::Outstanding {
                key: key.to_string(),
            }),
            Err(ReplayRefused::UnknownKey { .. }) => Ok(()),
            Err(refused) => Err(refused),
        }
    }
}

/// One candidate to replay: the oracle to run, where, and under which
/// profile. The expected witness class is withheld so a replay cannot echo it.
#[derive(Debug, Clone, Copy)]
pub struct ReplayRequest<'a> {
    /// The receipt key: the candidate scenario's digest.
    pub key: &'a str,
    pub scenario: &'a Scenario,
    pub set: &'a PairSet,
    pub oracle: &'a Oracle,
    pub checkpoint: Cut,
    pub profile_digest: &'a str,
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
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
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
    /// Elements the minimized scenario still holds: the single deletions a
    /// completed 1-minimality pass tried.
    pub remaining: u64,
    /// Every attempt in order; a digest answered earlier is recorded again
    /// with its cached verdict.
    pub candidates: Vec<CandidateRecord>,
    /// Replays issued, the original's included; every distinct candidate the
    /// compiler accepted took exactly one.
    pub replays: u64,
    pub max_replays: u64,
    /// Distinct candidates whose replay answered `Unknown`.
    pub unknown_candidates: u64,
    pub minimality: Minimality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShrinkReportError {
    SchemaMismatch {
        found: String,
    },
    Oracle(OracleRefused),
    /// The named field disagrees with the candidate ledger or cannot be what
    /// it claims to be.
    Inconsistent {
        field: &'static str,
    },
    /// An integer left the range both runtimes represent exactly.
    NotCanonical(ContractError),
    Shape(String),
    Lossy,
}

impl ShrinkReport {
    /// The schema, the pinned oracle, and the report's accounting against its
    /// own candidate ledger: the first candidate is the reproduced original,
    /// the last reproduced candidate is the minimized scenario, the counters
    /// agree with the distinct verdicts, and the minimality claim agrees with
    /// the single deletions tried after the last reproduction.
    pub fn validate(&self) -> Result<(), ShrinkReportError> {
        if self.schema != SHRINK_REPORT_SCHEMA {
            return Err(ShrinkReportError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        let inconsistent = |field| Err(ShrinkReportError::Inconsistent { field });
        match self.predicate.validate() {
            Ok(()) => {}
            Err(PredicateRefused::Oracle(refused)) => {
                return Err(ShrinkReportError::Oracle(refused));
            }
            Err(PredicateRefused::Malformed { field }) => return inconsistent(field),
        }
        // Digests are what `Scenario::digest` produces.
        let digest = |text: &str| is_lower_hex(text, 64);
        if !digest(&self.original_digest) {
            return inconsistent("original_digest");
        }
        if !digest(&self.minimized_digest) {
            return inconsistent("minimized_digest");
        }
        for record in &self.candidates {
            // `classify_replay` calls an observed predicate equal to the pinned
            // one `Reproduced`; a slip cannot have observed it.
            let echoed = matches!(&record.verdict, CandidateVerdict::Slipped { observed } if *observed == self.predicate);
            if !digest(&record.scenario_digest) || echoed {
                return inconsistent("candidates");
            }
        }
        let Some(first) = self.candidates.first() else {
            return inconsistent("candidates");
        };
        if first.scenario_digest != self.original_digest
            || !first.deleted.is_empty()
            || first.verdict != CandidateVerdict::Reproduced
        {
            return inconsistent("candidates");
        }
        let last = self
            .candidates
            .iter()
            .rposition(|record| record.verdict == CandidateVerdict::Reproduced)
            .unwrap_or(0);
        let last_reproduced = &self.candidates[last];
        if last_reproduced.scenario_digest != self.minimized_digest {
            return inconsistent("minimized_digest");
        }
        if last_reproduced.deleted != self.deleted {
            return inconsistent("deleted");
        }
        // A digest is answered once; a later record repeats its cached verdict.
        let mut distinct: BTreeMap<&str, &CandidateVerdict> = BTreeMap::new();
        for record in &self.candidates {
            let verdict = distinct
                .entry(record.scenario_digest.as_str())
                .or_insert(&record.verdict);
            if **verdict != record.verdict {
                return inconsistent("candidates");
            }
        }
        let count = |keep: fn(&CandidateVerdict) -> bool| {
            distinct.values().filter(|verdict| keep(verdict)).count() as u64
        };
        if self.unknown_candidates
            != count(|verdict| matches!(verdict, CandidateVerdict::Unknown { .. }))
        {
            return inconsistent("unknown_candidates");
        }
        // Every distinct candidate the compiler accepted took one replay: the
        // driver checks the budget before each test, so the budget refusal
        // never reaches a returned report.
        if self.replays != count(|verdict| !matches!(verdict, CandidateVerdict::InvalidPair { .. }))
            || self.replays > self.max_replays
        {
            return inconsistent("replays");
        }
        // Everything after the last reproduction deletes strictly more than the
        // minimized scenario; the single deletions among it are the final
        // 1-minimality pass, the only evidence the minimality claim rests on.
        let mut singles: BTreeMap<&str, (&CandidateVerdict, Transformation)> = BTreeMap::new();
        for record in &self.candidates[last + 1..] {
            if !record.deleted.is_superset(&self.deleted)
                || record.deleted.len() == self.deleted.len()
            {
                return inconsistent("candidates");
            }
            if record.deleted.len() == self.deleted.len() + 1 {
                let Some(extra) = record.deleted.difference(&self.deleted).next() else {
                    return inconsistent("candidates");
                };
                singles.insert(
                    record.scenario_digest.as_str(),
                    (&record.verdict, extra.transformation()),
                );
            }
        }
        let unknown_singles = singles
            .values()
            .filter(|(verdict, _)| matches!(verdict, CandidateVerdict::Unknown { .. }))
            .count() as u64;
        let full_pass = singles.len() as u64 == self.remaining;
        let claim_holds = match &self.minimality {
            Minimality::OneMinimal { transformations } => {
                full_pass
                    && unknown_singles == 0
                    && transformations.windows(2).all(|pair| pair[0] < pair[1])
                    && singles
                        .values()
                        .all(|(_, transformation)| transformations.contains(transformation))
            }
            Minimality::NotEstablished {
                reason: NotEstablishedReason::UnknownCandidates { count },
            } => full_pass && *count == unknown_singles && *count > 0,
            Minimality::NotEstablished {
                reason: NotEstablishedReason::ReplayBudgetExhausted,
            } => self.replays == self.max_replays,
        };
        if !claim_holds {
            return inconsistent("minimality");
        }
        Ok(())
    }
}

impl ShrinkReport {
    /// `validate`, then bind the report to the scenario it claims to have
    /// shrunk: its digest, the minimized scenario's digest, the elements that
    /// remain, and that the final pass's single deletions are exactly those
    /// elements. A report alone can only be self-consistent; with the
    /// original it is checked against the thing it describes.
    pub fn verify(&self, original: &Scenario) -> Result<(), ShrinkReportError> {
        self.validate()?;
        let inconsistent = |field| Err(ShrinkReportError::Inconsistent { field });
        if self.original_digest != original.digest() {
            return inconsistent("original_digest");
        }
        let held: BTreeSet<Element> = original.elements().into_iter().collect();
        if !self.deleted.is_subset(&held) {
            return inconsistent("deleted");
        }
        // Every record names the scenario its deletions leave.
        for record in &self.candidates {
            if !record.deleted.is_subset(&held)
                || record.scenario_digest != original.without(&record.deleted).digest()
            {
                return inconsistent("candidates");
            }
        }
        let minimized = original.without(&self.deleted);
        if self.minimized_digest != minimized.digest() {
            return inconsistent("minimized_digest");
        }
        let elements: BTreeSet<Element> = minimized.elements().into_iter().collect();
        if self.remaining != elements.len() as u64 {
            return inconsistent("remaining");
        }
        let last = self
            .candidates
            .iter()
            .rposition(|record| record.verdict == CandidateVerdict::Reproduced)
            .unwrap_or(0);
        let mut tried = BTreeSet::new();
        for record in &self.candidates[last + 1..] {
            let extra: Vec<&Element> = record.deleted.difference(&self.deleted).collect();
            if let [element] = extra.as_slice() {
                if !elements.contains(element) {
                    return inconsistent("candidates");
                }
                tried.insert((*element).clone());
            }
        }
        match &self.minimality {
            Minimality::NotEstablished {
                reason: NotEstablishedReason::ReplayBudgetExhausted,
            } => {}
            Minimality::NotEstablished { .. } if tried != elements => {
                return inconsistent("minimality");
            }
            Minimality::NotEstablished { .. } => {}
            // A completed run tried exactly the transformations the original
            // had elements for, in the parent's order.
            Minimality::OneMinimal { transformations } => {
                let evidenced: Vec<Transformation> = Transformation::ORDER
                    .into_iter()
                    .filter(|t| held.iter().any(|e| e.transformation() == *t))
                    .collect();
                if tried != elements || *transformations != evidenced {
                    return inconsistent("minimality");
                }
            }
        }
        Ok(())
    }

    /// Digestible on both runtimes: no integer may leave the canonical safe
    /// range, or a Bun reader would corrupt it.
    pub fn serialize(&self) -> Result<Value, ShrinkReportError> {
        let value =
            serde_json::to_value(self).map_err(|e| ShrinkReportError::Shape(e.to_string()))?;
        canonical_json_encode(&value).map_err(ShrinkReportError::NotCanonical)?;
        self.validate()?;
        Ok(value)
    }
}

pub fn parse_shrink_report(value: &Value) -> Result<ShrinkReport, ShrinkReportError> {
    canonical_json_encode(value).map_err(ShrinkReportError::NotCanonical)?;
    let report =
        ShrinkReport::deserialize(value).map_err(|e| ShrinkReportError::Shape(e.to_string()))?;
    report.validate()?;
    let again =
        serde_json::to_value(&report).map_err(|e| ShrinkReportError::Shape(e.to_string()))?;
    if again != *value {
        return Err(ShrinkReportError::Lossy);
    }
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShrinkRefused {
    /// The pinned oracle is not a valid configuration; nothing is replayed.
    InvalidOracle(OracleRefused),
    /// A pinned field cannot be what it claims to pin; nothing is replayed.
    InvalidPredicate { field: &'static str },
    /// The original's fault episodes are not a valid set; nothing is replayed.
    InvalidEpisodes(EpisodeRefused),
    /// The budget would leave the report's canonical integer range.
    BudgetNotCanonical { max_replays: u64 },
    /// The original scenario itself did not reproduce the pinned predicate.
    OriginalNotReproduced { verdict: CandidateVerdict },
}

debug_display!(ShrinkRefused, ShrinkReportError);

struct Driver<'a> {
    fixture: &'a Value,
    predicate: &'a FailurePredicate,
    max_replays: u64,
    replay: &'a mut dyn FnMut(ReplayRequest<'_>) -> ReplayOutcome,
    verdicts: BTreeMap<String, CandidateVerdict>,
    candidates: Vec<CandidateRecord>,
    replays: u64,
}

impl Driver<'_> {
    /// Tries the candidate; a digest already answered is not replayed twice.
    /// `replay_candidate` refuses a replay past the budget for every caller.
    fn test(&mut self, candidate: &Scenario, deleted: &BTreeSet<Element>) -> CandidateVerdict {
        let digest = candidate.digest();
        let verdict = match self.verdicts.get(&digest) {
            Some(verdict) => verdict.clone(),
            None => {
                let verdict = self.replay_candidate(candidate, &digest);
                self.verdicts.insert(digest.clone(), verdict.clone());
                verdict
            }
        };
        self.candidates.push(CandidateRecord {
            scenario_digest: digest,
            deleted: deleted.clone(),
            verdict: verdict.clone(),
        });
        verdict
    }

    fn replay_candidate(&mut self, candidate: &Scenario, key: &str) -> CandidateVerdict {
        let set = match candidate.compile(self.fixture) {
            Ok(set) => set,
            Err(error) => {
                return CandidateVerdict::InvalidPair {
                    refusal: error.kind().to_string(),
                };
            }
        };
        if self.exhausted() {
            return CandidateVerdict::Unknown {
                reason: UnknownReason::ReplayBudgetExhausted,
            };
        }
        self.replays += 1;
        let outcome = (self.replay)(ReplayRequest {
            key,
            scenario: candidate,
            set: &set,
            oracle: &self.predicate.oracle,
            checkpoint: self.predicate.checkpoint,
            profile_digest: &self.predicate.profile_digest,
        });
        classify_replay(self.predicate, &outcome)
    }

    fn exhausted(&self) -> bool {
        self.replays >= self.max_replays
    }
}

/// Shrinks `original` until no single deletion under any tried transformation
/// still reproduces `predicate`, or the replay budget runs out. The returned
/// scenario reproduced the predicate on its last replay. An invalid pinned
/// oracle, episode set, or budget is refused before any replay.
pub fn shrink(
    original: &Scenario,
    fixture: &Value,
    predicate: &FailurePredicate,
    max_replays: u64,
    replay: &mut dyn FnMut(ReplayRequest<'_>) -> ReplayOutcome,
) -> Result<(Scenario, ShrinkReport), ShrinkRefused> {
    predicate.validate().map_err(|refused| match refused {
        PredicateRefused::Oracle(refused) => ShrinkRefused::InvalidOracle(refused),
        PredicateRefused::Malformed { field } => ShrinkRefused::InvalidPredicate { field },
    })?;
    validate_episodes(&original.episodes).map_err(ShrinkRefused::InvalidEpisodes)?;
    if canonical_json_encode(&Value::from(max_replays)).is_err() {
        return Err(ShrinkRefused::BudgetNotCanonical { max_replays });
    }
    let mut driver = Driver {
        fixture,
        predicate,
        max_replays,
        replay,
        verdicts: BTreeMap::new(),
        candidates: Vec::new(),
        replays: 0,
    };
    let verdict = driver.test(original, &BTreeSet::new());
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
        if kept.is_empty() || driver.exhausted() {
            continue;
        }
        tried.push(transformation);
        deleted = ddmin(&mut driver, original, deleted, kept);
    }
    let minimality = one_minimality(&mut driver, original, &mut deleted, &tried);
    let minimized = original.without(&deleted);
    let unknown_candidates = driver
        .verdicts
        .values()
        .filter(|verdict| matches!(verdict, CandidateVerdict::Unknown { .. }))
        .count() as u64;
    let report = ShrinkReport {
        schema: SHRINK_REPORT_SCHEMA.to_string(),
        predicate: predicate.clone(),
        original_digest: original.digest(),
        minimized_digest: minimized.digest(),
        deleted,
        remaining: minimized.elements().len() as u64,
        candidates: driver.candidates,
        replays: driver.replays,
        max_replays,
        unknown_candidates,
        minimality,
    };
    Ok((minimized, report))
}

/// Zeller's ddmin over `kept`, holding `deleted` from earlier transformations
/// fixed. Only `Reproduced` shrinks; `Unknown` stays in the set; an exhausted
/// budget stops the pass.
fn ddmin(
    driver: &mut Driver<'_>,
    original: &Scenario,
    deleted: BTreeSet<Element>,
    mut kept: Vec<Element>,
) -> BTreeSet<Element> {
    let all: BTreeSet<Element> = kept.iter().cloned().collect();
    let mut n = 2;
    while !kept.is_empty() && !driver.exhausted() {
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
                if candidate.len() == kept.len() || driver.exhausted() {
                    continue;
                }
                let drop: BTreeSet<Element> = deleted
                    .iter()
                    .cloned()
                    .chain(all.iter().filter(|e| !candidate.contains(e)).cloned())
                    .collect();
                if driver.test(&original.without(&drop), &drop) == CandidateVerdict::Reproduced {
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
    deleted
        .into_iter()
        .chain(all.into_iter().filter(|e| !kept.contains(e)))
        .collect()
}

/// Tries every single deletion against the minimized scenario until a full
/// pass rejects them all. A rejection under every tried transformation is
/// 1-minimality; an `Unknown` or an exhausted budget is not.
fn one_minimality(
    driver: &mut Driver<'_>,
    original: &Scenario,
    deleted: &mut BTreeSet<Element>,
    tried: &[Transformation],
) -> Minimality {
    loop {
        let mut unknown = 0;
        let mut reduced = false;
        for element in original.without(deleted).elements() {
            if driver.exhausted() {
                return Minimality::NotEstablished {
                    reason: NotEstablishedReason::ReplayBudgetExhausted,
                };
            }
            let mut drop = deleted.clone();
            drop.insert(element);
            match driver.test(&original.without(&drop), &drop) {
                CandidateVerdict::Reproduced => {
                    *deleted = drop;
                    reduced = true;
                    break;
                }
                CandidateVerdict::Unknown { .. } => unknown += 1,
                _ => {}
            }
        }
        if reduced {
            continue;
        }
        return if unknown == 0 {
            Minimality::OneMinimal {
                transformations: tried.to_vec(),
            }
        } else {
            Minimality::NotEstablished {
                reason: NotEstablishedReason::UnknownCandidates { count: unknown },
            }
        };
    }
}
