//! The paired-world compiler: a fresh and an aged arm over one task, one
//! truth, and one evidence set, with the controls that tell history
//! interference apart from a recency-only explanation.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::{EvaluatedSurface, SURFACE1_HINT_BOUNDS};
use crate::eligibility::Verdict;
use crate::event::{EventId, EventLog, LogError, Payload};
use crate::reducer::{Query, ReduceError, Truth, reduce};

pub const PAIRING_POLICY_VERSION: &str = "eval-pairing/v1";
pub const RECENCY_BASELINE_VERSION: &str = "eval-recency-baseline/v1";
/// Appended to every entity of an independently authored history so its
/// event identities cannot collide with the aged world's.
pub const NATURAL_FRESH_ENTITY_TAG: &str = "natural-fresh";

/// The window the recency baseline reads on a surface. Surface 1's is the
/// production hint candidate limit and refuses a declaration that disagrees;
/// the others have no production constant, so an undeclared bound refuses
/// rather than borrowing one.
pub fn recency_bound(
    surface: EvaluatedSurface,
    declared: Option<NonZeroU32>,
) -> Result<u32, PairError> {
    let pinned = u32::try_from(SURFACE1_HINT_BOUNDS.candidates).expect("small constant");
    match (surface, declared) {
        (EvaluatedSurface::Surface1, None) => Ok(pinned),
        (EvaluatedSurface::Surface1, Some(k)) if k.get() != pinned => {
            Err(PairError::RecencyBoundConflict {
                surface,
                pinned,
                declared: k.get(),
            })
        }
        (_, Some(k)) => Ok(k.get()),
        (_, None) => Err(PairError::UnresolvedRecencyBound { surface }),
    }
}

/// What a task claims about its truth. The compiler checks a falsification
/// claim against the aged history; the baseline check judges a positive
/// control, so that the control is a measurement and not a restatement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRole {
    /// Truth established before the aged history's median time and never
    /// corrected or retracted, so a retriever that prefers recent units
    /// cannot pass by accident.
    Falsification,
    /// Truth the baseline is expected to deliver; without one, an
    /// always-empty baseline fails every falsification pair for free.
    PositiveControl,
    Plain,
}

/// One bitemporal question and the evidence IDs its answer rests on.
/// `evidence` is the AND-support set; the reducer must judge every member
/// `Ok` on both arms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub role: TaskRole,
    pub query: Query,
    pub evidence: BTreeSet<EventId>,
}

/// The arms a runner executes. `FreshMinimal` is the truth and its context
/// alone, a diagnostic ceiling with nothing competing for attention; `Fresh`
/// is the natural-fresh control, an independently authored short history
/// with the truth spliced in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmKind {
    Aged,
    Fresh,
    FreshMinimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub task: Task,
    pub fresh: EventLog,
    pub fresh_minimal: EventLog,
}

/// One pair per input task, in input order, over one aged history and one
/// shared query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairSet {
    pub pairing_policy_version: String,
    pub surface: EvaluatedSurface,
    pub recency_bound: u32,
    pub aged: EventLog,
    /// The aged history's upper-median valid time; a falsification truth
    /// precedes it.
    #[serde(with = "crate::decimal")]
    pub aged_median_ms: i64,
    /// The task query with the control's entities in scope.
    pub fresh_query: Query,
    /// Eligible units of the aged arm inside the window, most recent first.
    pub recency_window: Vec<EventId>,
    pub pairs: Vec<Pair>,
}

#[derive(Debug, Clone, Copy)]
pub struct PairSetInput<'a> {
    pub surface: EvaluatedSurface,
    pub declared_bound: Option<NonZeroU32>,
    pub aged: &'a EventLog,
    /// A short history authored apart from the aged one: the same generator
    /// under another seed and configuration, never a copy of any run of it.
    pub natural_fresh: &'a EventLog,
    pub fixture: &'a Value,
    pub tasks: &'a [Task],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    UnresolvedRecencyBound {
        surface: EvaluatedSurface,
    },
    RecencyBoundConflict {
        surface: EvaluatedSurface,
        pinned: u32,
        declared: u32,
    },
    EmptyAged,
    EmptyNaturalFresh,
    /// The natural-fresh events, compared by content, are a contiguous run
    /// of the aged ones.
    NaturalFreshCopiedFromAged {
        at: usize,
    },
    /// No unit of the independent history is eligible at the task's cut, so
    /// the fresh arm competes with nothing.
    NaturalFreshInert {
        task: String,
    },
    AgedHistoryTooShort {
        median_ms: i64,
    },
    NoTasks,
    DuplicateTask {
        task: String,
    },
    /// A task query that differs from the set's. A task-specific query picks
    /// the units eligible for that task alone, which defeats the control.
    MixedQueries {
        task: String,
    },
    EmptyEvidence {
        task: String,
    },
    EvidenceNotRequired {
        task: String,
        id: EventId,
        verdict: Option<Verdict>,
    },
    EvidenceNotRequiredOnArm {
        task: String,
        arm: ArmKind,
        id: EventId,
    },
    /// A unit both arms carry that the reducer judges required on one and
    /// not the other.
    SharedVerdictDisagreement {
        task: String,
        arm: ArmKind,
        differing: BTreeSet<EventId>,
    },
    TruthNotEarly {
        task: String,
        id: EventId,
        valid_time_ms: i64,
        median_ms: i64,
    },
    SupersededFalsifier {
        task: String,
        id: EventId,
        by: EventId,
    },
    NoFalsificationPair,
    NoPositiveControl,
    /// A deserialized set whose recorded version, bound, median, or window
    /// disagrees with a recomputation from the set's own aged history.
    Tampered {
        field: &'static str,
    },
    Reduce(ReduceError),
    Log(LogError),
}

debug_display!(PairError);

/// The events the retriever needs to reach `evidence` and the events that
/// decide its verdict: the evidence, every unit it descends from or refers
/// to, and every correction or retraction aimed at any of those, closed under
/// the same rule, so the ceiling judges shared units as the aged arm does.
fn minimal_closure(aged: &EventLog, evidence: &BTreeSet<EventId>) -> EventLog {
    let mut parents: BTreeMap<&EventId, Vec<&EventId>> = BTreeMap::new();
    for edge in &aged.causal_edges {
        parents.entry(&edge.to).or_default().push(&edge.from);
    }
    let mut supersessions: BTreeMap<&EventId, Vec<&EventId>> = BTreeMap::new();
    let mut references: BTreeMap<&EventId, &EventId> = BTreeMap::new();
    for event in &aged.events {
        if let Some((_, target)) = event.payload.supersedes() {
            supersessions.entry(target).or_default().push(&event.id);
        }
        if let Some(target) = event.payload.reference() {
            references.insert(&event.id, target);
        }
    }
    let mut keep: BTreeSet<&EventId> = BTreeSet::new();
    let mut pending: Vec<&EventId> = evidence.iter().collect();
    while let Some(id) = pending.pop() {
        if !keep.insert(id) {
            continue;
        }
        pending.extend(parents.get(id).into_iter().flatten());
        pending.extend(supersessions.get(id).into_iter().flatten());
        pending.extend(references.get(id));
    }
    let events = aged
        .events
        .iter()
        .filter(|e| keep.contains(&e.id))
        .cloned()
        .collect();
    let edges = aged
        .causal_edges
        .iter()
        .filter(|edge| keep.contains(&edge.from) && keep.contains(&edge.to))
        .cloned()
        .collect();
    EventLog::new(events, edges)
}

/// The k eligible units with the largest valid time, most recent first; ties
/// fall to linearization order, later position first. `truth` is the
/// reduction of `aged`, whose events are in linearization order.
fn recency_baseline(aged: &EventLog, truth: &Truth, k: u32) -> Vec<EventId> {
    aged.events
        .iter()
        .rev()
        .filter(|event| truth.required.contains(&event.id))
        .map(|event| event.id.clone())
        .take(k as usize)
        .collect()
}

fn required_set(
    task: &Task,
    arm: ArmKind,
    truth: &Truth,
    reference: &Truth,
) -> Result<(), PairError> {
    if let Some(id) = task
        .evidence
        .iter()
        .find(|id| !truth.required.contains(*id))
    {
        return Err(PairError::EvidenceNotRequiredOnArm {
            task: task.id.clone(),
            arm,
            id: id.clone(),
        });
    }
    let differing: BTreeSet<EventId> = truth
        .units
        .keys()
        .filter(|id| reference.units.contains_key(*id))
        .filter(|id| reference.required.contains(*id) != truth.required.contains(*id))
        .cloned()
        .collect();
    if differing.is_empty() {
        Ok(())
    } else {
        Err(PairError::SharedVerdictDisagreement {
            task: task.id.clone(),
            arm,
            differing,
        })
    }
}

/// Every evidence unit precedes the median, then none is ever corrected or
/// retracted, in that order.
fn check_falsifier(task: &Task, aged: &EventLog, median: i64) -> Result<(), PairError> {
    for event in aged.events.iter().filter(|e| task.evidence.contains(&e.id)) {
        if event.valid_time_ms >= median {
            return Err(PairError::TruthNotEarly {
                task: task.id.clone(),
                id: event.id.clone(),
                valid_time_ms: event.valid_time_ms,
                median_ms: median,
            });
        }
    }
    for event in &aged.events {
        if let Some((_, target)) = event.payload.supersedes()
            && task.evidence.contains(target)
        {
            return Err(PairError::SupersededFalsifier {
                task: task.id.clone(),
                id: target.clone(),
                by: event.id.clone(),
            });
        }
    }
    Ok(())
}

fn content(log: &EventLog) -> Vec<(i64, u32, Payload)> {
    log.events
        .iter()
        .map(|e| (e.valid_time_ms, e.local_seq, e.payload.content()))
        .collect()
}

/// The aged history's upper-median valid time. Refuses an empty history and
/// one whose earliest time is its median, where "early" would be vacuous.
fn aged_median(aged: &EventLog) -> Result<i64, PairError> {
    let events = &aged.events;
    if events.is_empty() {
        return Err(PairError::EmptyAged);
    }
    let median_ms = events[events.len() / 2].valid_time_ms;
    if events[0].valid_time_ms == median_ms {
        return Err(PairError::AgedHistoryTooShort { median_ms });
    }
    Ok(median_ms)
}

/// The one query every task shares. Refuses no tasks, a repeated task ID,
/// and a task whose query differs from the first's.
fn shared_query<'a>(tasks: impl IntoIterator<Item = &'a Task>) -> Result<&'a Query, PairError> {
    let mut tasks = tasks.into_iter();
    let first = tasks.next().ok_or(PairError::NoTasks)?;
    let mut seen = BTreeSet::from([first.id.as_str()]);
    for task in tasks {
        if !seen.insert(task.id.as_str()) {
            return Err(PairError::DuplicateTask {
                task: task.id.clone(),
            });
        }
        if task.query != first.query {
            return Err(PairError::MixedQueries {
                task: task.id.clone(),
            });
        }
    }
    Ok(&first.query)
}

/// The evidence is non-empty and required on the aged arm; a falsification
/// claim is also early and never superseded.
fn aged_arm(task: &Task, aged: &EventLog, truth: &Truth, median_ms: i64) -> Result<(), PairError> {
    if task.evidence.is_empty() {
        return Err(PairError::EmptyEvidence {
            task: task.id.clone(),
        });
    }
    if let Some(id) = task
        .evidence
        .iter()
        .find(|id| !truth.required.contains(*id))
    {
        return Err(PairError::EvidenceNotRequired {
            task: task.id.clone(),
            id: id.clone(),
            verdict: truth.units.get(id).map(|unit| unit.verdict),
        });
    }
    if task.role == TaskRole::Falsification {
        check_falsifier(task, aged, median_ms)?;
    }
    Ok(())
}

/// The bound, the two histories' independence, and the aged history's span.
fn admit(input: &PairSetInput<'_>) -> Result<(u32, i64), PairError> {
    let bound = recency_bound(input.surface, input.declared_bound)?;
    if input.aged.events.is_empty() {
        return Err(PairError::EmptyAged);
    }
    if input.natural_fresh.events.is_empty() {
        return Err(PairError::EmptyNaturalFresh);
    }
    let (aged, fresh) = (content(input.aged), content(input.natural_fresh));
    if let Some(at) = aged
        .windows(fresh.len())
        .position(|window| window == fresh.as_slice())
    {
        return Err(PairError::NaturalFreshCopiedFromAged { at });
    }
    Ok((bound, aged_median(input.aged)?))
}

/// The query with the independent history's entities added to its scope.
fn widen(query: &Query, independent: &EventLog) -> Query {
    let mut fresh_query = query.clone();
    fresh_query
        .scope
        .extend(independent.events.iter().map(|e| e.entity_id.clone()));
    fresh_query
}

fn compile_one(
    input: &PairSetInput<'_>,
    independent: &EventLog,
    fresh_query: &Query,
    aged_truth: &Truth,
    task: &Task,
    median_ms: i64,
) -> Result<Pair, PairError> {
    aged_arm(task, input.aged, aged_truth, median_ms)?;
    let fresh_minimal = minimal_closure(input.aged, &task.evidence);
    let fresh = EventLog::new(
        [independent.events.clone(), fresh_minimal.events.clone()].concat(),
        [
            independent.causal_edges.clone(),
            fresh_minimal.causal_edges.clone(),
        ]
        .concat(),
    );
    let minimal_truth =
        reduce(&fresh_minimal, input.fixture, &task.query).map_err(PairError::Reduce)?;
    required_set(task, ArmKind::FreshMinimal, &minimal_truth, aged_truth)?;
    let fresh_truth = reduce(&fresh, input.fixture, fresh_query).map_err(PairError::Reduce)?;
    required_set(task, ArmKind::Fresh, &fresh_truth, aged_truth)?;
    if !independent
        .events
        .iter()
        .any(|e| fresh_truth.required.contains(&e.id))
    {
        return Err(PairError::NaturalFreshInert {
            task: task.id.clone(),
        });
    }
    Ok(Pair {
        task: task.clone(),
        fresh,
        fresh_minimal,
    })
}

/// Compiles one pair per task. Refuses an empty or copied natural-fresh
/// history, an aged history too short for "early" to mean anything, tasks
/// whose queries differ, evidence the reducer does not require, a
/// falsification claim the aged history contradicts, an arm that judges a
/// shared unit differently, and a set missing either control class.
pub fn compile_pair_set(input: PairSetInput<'_>) -> Result<PairSet, PairError> {
    let (bound, aged_median_ms) = admit(&input)?;
    let query = shared_query(input.tasks)?;
    let independent = input
        .natural_fresh
        .on_distinct_entities(NATURAL_FRESH_ENTITY_TAG)
        .map_err(PairError::Log)?;
    let aged_truth = reduce(input.aged, input.fixture, query).map_err(PairError::Reduce)?;
    let fresh_query = widen(query, &independent);
    let pairs = input
        .tasks
        .iter()
        .map(|task| {
            compile_one(
                &input,
                &independent,
                &fresh_query,
                &aged_truth,
                task,
                aged_median_ms,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let set = PairSet {
        pairing_policy_version: PAIRING_POLICY_VERSION.to_string(),
        surface: input.surface,
        recency_bound: bound,
        aged: input.aged.clone(),
        aged_median_ms,
        fresh_query,
        recency_window: recency_baseline(input.aged, &aged_truth, bound),
        pairs,
    };
    set.roles_present()?;
    Ok(set)
}

impl PairSet {
    fn roles_present(&self) -> Result<(), PairError> {
        let roles: BTreeSet<TaskRole> = self.pairs.iter().map(|p| p.task.role).collect();
        if !roles.contains(&TaskRole::Falsification) {
            return Err(PairError::NoFalsificationPair);
        }
        if !roles.contains(&TaskRole::PositiveControl) {
            return Err(PairError::NoPositiveControl);
        }
        Ok(())
    }

    /// A set read back from the wire must still be one the compiler could
    /// have produced from its own aged history: the policy version, the
    /// surface's bound, one query across the tasks, both control classes,
    /// evidence the reducer requires on the aged arm, early and unsuperseded
    /// falsification truths, and the median and window recomputed from
    /// `aged` under `fixture`. The fresh arms are the runner's inputs and are
    /// not re-derived here.
    pub fn validate(&self, fixture: &Value) -> Result<(), PairError> {
        if self.pairing_policy_version != PAIRING_POLICY_VERSION {
            return Err(PairError::Tampered {
                field: "pairing_policy_version",
            });
        }
        let declared = NonZeroU32::new(self.recency_bound);
        if recency_bound(self.surface, declared) != Ok(self.recency_bound) {
            return Err(PairError::Tampered {
                field: "recency_bound",
            });
        }
        let median_ms = aged_median(&self.aged)?;
        if median_ms != self.aged_median_ms {
            return Err(PairError::Tampered {
                field: "aged_median_ms",
            });
        }
        let query = shared_query(self.pairs.iter().map(|p| &p.task))?;
        self.roles_present()?;
        let truth = reduce(&self.aged, fixture, query).map_err(PairError::Reduce)?;
        for pair in &self.pairs {
            aged_arm(&pair.task, &self.aged, &truth, median_ms)?;
        }
        if self.recency_window != recency_baseline(&self.aged, &truth, self.recency_bound) {
            return Err(PairError::Tampered {
                field: "recency_window",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Suite {
    A,
    B,
    C,
    D,
}

/// The parent's stop conditions: (a) an approved production tap is rejected,
/// (b) the recency baseline cannot be made to fail the falsification pairs
/// while passing its positive controls, (c) the ICC pilot leaves the
/// effective sample below the required N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopCondition {
    A,
    B,
    C,
}

impl StopCondition {
    /// Each condition suppresses Suite B and D reports and gates and leaves
    /// Suite A and C work untouched.
    pub fn suppresses(self, suite: Suite) -> bool {
        match (self, suite) {
            (Self::A | Self::B | Self::C, Suite::B | Suite::D) => true,
            (Self::A | Self::B | Self::C, Suite::A | Suite::C) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineContrast {
    pub baseline_version: String,
    pub surface: EvaluatedSurface,
    pub recency_bound: u32,
    pub falsification_pairs_failed: u32,
    pub positive_controls_passed: u32,
    /// Distinct IDs the baseline delivered across both classes.
    pub delivered_ids: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaselineFailure {
    /// Zero IDs delivered on both classes: the contrast was never exercised.
    Vacuous,
    DeliveredFalsifier {
        task: String,
    },
    MissedPositiveControl {
        task: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaselineVerdict {
    Established {
        contrast: BaselineContrast,
    },
    Blocked {
        condition: StopCondition,
        failure: BaselineFailure,
    },
}

/// Stop condition (b). `deliver` is the baseline under test, given each pair;
/// the compiled `recency_window` is the versioned one, and a negative control
/// substitutes an always-empty baseline. Vacuity is decided first over both
/// classes, then every falsification pair must be missed, then every
/// positive control must be delivered.
pub fn check_recency_baseline(
    set: &PairSet,
    fixture: &Value,
    deliver: impl Fn(&Pair) -> Vec<EventId>,
) -> Result<BaselineVerdict, PairError> {
    set.validate(fixture)?;
    let blocked = |failure| {
        Ok(BaselineVerdict::Blocked {
            condition: StopCondition::B,
            failure,
        })
    };
    let judged: Vec<(&Pair, BTreeSet<EventId>)> = set
        .pairs
        .iter()
        .filter(|pair| pair.task.role != TaskRole::Plain)
        .map(|pair| (pair, deliver(pair).into_iter().collect()))
        .collect();
    let delivered: BTreeSet<&EventId> = judged.iter().flat_map(|(_, ids)| ids).collect();
    if delivered.is_empty() {
        return blocked(BaselineFailure::Vacuous);
    }
    let superset = |pair: &Pair, ids: &BTreeSet<EventId>| pair.task.evidence.is_subset(ids);
    let mut contrast = BaselineContrast {
        baseline_version: RECENCY_BASELINE_VERSION.to_string(),
        surface: set.surface,
        recency_bound: set.recency_bound,
        falsification_pairs_failed: 0,
        positive_controls_passed: 0,
        delivered_ids: u32::try_from(delivered.len()).expect("bounded by the log"),
    };
    for (pair, ids) in judged
        .iter()
        .filter(|(p, _)| p.task.role == TaskRole::Falsification)
    {
        if superset(pair, ids) {
            return blocked(BaselineFailure::DeliveredFalsifier {
                task: pair.task.id.clone(),
            });
        }
        contrast.falsification_pairs_failed += 1;
    }
    for (pair, ids) in judged
        .iter()
        .filter(|(p, _)| p.task.role == TaskRole::PositiveControl)
    {
        if !superset(pair, ids) {
            return blocked(BaselineFailure::MissedPositiveControl {
                task: pair.task.id.clone(),
            });
        }
        contrast.positive_controls_passed += 1;
    }
    Ok(BaselineVerdict::Established { contrast })
}
