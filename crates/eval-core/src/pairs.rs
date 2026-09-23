//! The paired-world compiler: a fresh and an aged arm over one task, one
//! truth, and one evidence set, with the controls that tell history
//! interference apart from a recency-only explanation.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::{EvaluatedSurface, SURFACE1_HINT_BOUNDS};
use crate::eligibility::Verdict;
use crate::event::{Event, EventId, EventLog, LogError, Payload};
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
    /// A deserialized set whose recorded version, bound, median, window,
    /// widened query, or pairs disagree with the compiler's recomputation
    /// from the set's own parts.
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

fn content<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<(i64, u32, Payload)> {
    events
        .into_iter()
        .map(|e| (e.valid_time_ms, e.local_seq, e.payload.content()))
        .collect()
}

/// Refuses an independent history that does not validate on its own, one
/// with nothing to compete, and one whose events, compared by content, are
/// a contiguous run of the aged ones.
fn independent_of(aged: &EventLog, fresh: &EventLog, max_events: u32) -> Result<(), PairError> {
    fresh
        .validate(max_events as usize)
        .map_err(PairError::Log)?;
    let fresh = content(&fresh.events);
    if fresh.is_empty() {
        return Err(PairError::EmptyNaturalFresh);
    }
    if let Some(at) = content(&aged.events)
        .windows(fresh.len())
        .position(|window| window == fresh.as_slice())
    {
        return Err(PairError::NaturalFreshCopiedFromAged { at });
    }
    Ok(())
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

/// The query with the independent history's entities added to its scope.
fn widen(query: &Query, independent: &EventLog) -> Query {
    let mut fresh_query = query.clone();
    fresh_query
        .scope
        .extend(independent.events.iter().map(|e| e.entity_id.clone()));
    fresh_query
}

fn ids_of(log: &EventLog) -> BTreeSet<&EventId> {
    log.events.iter().map(|e| &e.id).collect()
}

/// Both fresh arms keep the task's evidence required and judge every unit
/// they share with the aged arm as it does, and the control competes: some
/// unit of the independent history is required on the fresh arm.
fn compile_one(
    aged: &EventLog,
    independent: &EventLog,
    fixture: &Value,
    fresh_query: &Query,
    aged_truth: &Truth,
    task: &Task,
    median_ms: i64,
) -> Result<Pair, PairError> {
    aged_arm(task, aged, aged_truth, median_ms)?;
    let fresh_minimal = minimal_closure(aged, &task.evidence);
    let fresh = EventLog::new(
        [independent.events.clone(), fresh_minimal.events.clone()].concat(),
        [
            independent.causal_edges.clone(),
            fresh_minimal.causal_edges.clone(),
        ]
        .concat(),
    );
    let minimal_truth = reduce(&fresh_minimal, fixture, &task.query).map_err(PairError::Reduce)?;
    required_set(task, ArmKind::FreshMinimal, &minimal_truth, aged_truth)?;
    let fresh_truth = reduce(&fresh, fixture, fresh_query).map_err(PairError::Reduce)?;
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

/// The set the compiler produces from an aged history, an independent
/// history already on its own entities, and the tasks; `validate` produces
/// it again from a set's own parts.
fn assemble(
    surface: EvaluatedSurface,
    bound: u32,
    aged: &EventLog,
    independent: &EventLog,
    fixture: &Value,
    tasks: &[Task],
) -> Result<PairSet, PairError> {
    let query = shared_query(tasks)?;
    if aged.events.is_empty() {
        return Err(PairError::EmptyAged);
    }
    independent_of(aged, independent, query.max_events_per_log)?;
    let aged_median_ms = aged_median(aged)?;
    let aged_truth = reduce(aged, fixture, query).map_err(PairError::Reduce)?;
    let fresh_query = widen(query, independent);
    let pairs = tasks
        .iter()
        .map(|task| {
            compile_one(
                aged,
                independent,
                fixture,
                &fresh_query,
                &aged_truth,
                task,
                aged_median_ms,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let set = PairSet {
        pairing_policy_version: PAIRING_POLICY_VERSION.to_string(),
        surface,
        recency_bound: bound,
        aged: aged.clone(),
        aged_median_ms,
        fresh_query,
        recency_window: recency_baseline(aged, &aged_truth, bound),
        pairs,
    };
    set.roles_present()?;
    Ok(set)
}

/// Compiles one pair per task. Refuses an empty or copied natural-fresh
/// history, an aged history too short for "early" to mean anything, tasks
/// whose queries differ, evidence the reducer does not require, a
/// falsification claim the aged history contradicts, an arm that judges a
/// shared unit differently, and a set missing either control class. The
/// natural-fresh history is validated as supplied: `on_distinct_entities`
/// sorts and re-derives, so a shuffled slice of the aged history would
/// otherwise pass the copy check and be normalized back into the copy.
pub fn compile_pair_set(input: PairSetInput<'_>) -> Result<PairSet, PairError> {
    let query = shared_query(input.tasks)?;
    let bound = recency_bound(input.surface, input.declared_bound)?;
    input
        .natural_fresh
        .validate(query.max_events_per_log as usize)
        .map_err(PairError::Log)?;
    let independent = input
        .natural_fresh
        .on_distinct_entities(NATURAL_FRESH_ENTITY_TAG)
        .map_err(PairError::Log)?;
    assemble(
        input.surface,
        bound,
        input.aged,
        &independent,
        input.fixture,
        input.tasks,
    )
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

    /// The independent history every pair's fresh arm carries: its units and
    /// edges the aged history lacks. Pairs that disagree are `Tampered`.
    fn independent(&self) -> Result<EventLog, PairError> {
        let aged_ids = ids_of(&self.aged);
        let project = |pair: &Pair| {
            EventLog::new(
                pair.fresh
                    .events
                    .iter()
                    .filter(|e| !aged_ids.contains(&e.id))
                    .cloned()
                    .collect(),
                pair.fresh
                    .causal_edges
                    .iter()
                    .filter(|edge| !aged_ids.contains(&edge.from) || !aged_ids.contains(&edge.to))
                    .cloned()
                    .collect(),
            )
        };
        let common = self
            .pairs
            .first()
            .map(project)
            .unwrap_or_else(|| EventLog::new(vec![], vec![]));
        if self.pairs.iter().any(|pair| project(pair) != common) {
            return Err(PairError::Tampered { field: "pairs" });
        }
        Ok(common)
    }

    /// A set read back from the wire must be the one the compiler produces
    /// from the set's own parts: the policy version, the surface's bound,
    /// and `assemble` over its aged history, the independent history common
    /// to every pair's fresh arm, and its tasks under `fixture`. Every
    /// compile-time refusal applies again, and `Tampered {field}` names the
    /// recorded field that differs from the recomputation.
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
        let tasks: Vec<Task> = self.pairs.iter().map(|p| p.task.clone()).collect();
        let independent = self.independent()?;
        let expected = assemble(
            self.surface,
            self.recency_bound,
            &self.aged,
            &independent,
            fixture,
            &tasks,
        )?;
        let differing = [
            (
                "aged_median_ms",
                expected.aged_median_ms != self.aged_median_ms,
            ),
            (
                "recency_window",
                expected.recency_window != self.recency_window,
            ),
            ("fresh_query", expected.fresh_query != self.fresh_query),
            ("pairs", expected.pairs != self.pairs),
        ];
        match differing.into_iter().find(|(_, differs)| *differs) {
            Some((field, _)) => Err(PairError::Tampered { field }),
            None => Ok(()),
        }
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

/// The baseline under test. The versioned one is the set's compiled
/// `recency_window`, so an `Established` contrast is always stamped with
/// the window it was judged on; the negative control delivers nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    Versioned,
    AlwaysEmpty,
}

/// Stop condition (b). The window is shared across every pair because the
/// tasks share one query. Vacuity is decided first over both classes, then
/// every falsification pair must be missed, then every positive control must
/// be delivered.
pub fn check_recency_baseline(
    set: &PairSet,
    fixture: &Value,
    baseline: Baseline,
) -> Result<BaselineVerdict, PairError> {
    set.validate(fixture)?;
    let blocked = |failure| {
        Ok(BaselineVerdict::Blocked {
            condition: StopCondition::B,
            failure,
        })
    };
    let delivered: BTreeSet<&EventId> = match baseline {
        Baseline::Versioned => set.recency_window.iter().collect(),
        Baseline::AlwaysEmpty => BTreeSet::new(),
    };
    if delivered.is_empty() {
        return blocked(BaselineFailure::Vacuous);
    }
    let superset = |pair: &Pair| pair.task.evidence.iter().all(|id| delivered.contains(id));
    let mut contrast = BaselineContrast {
        baseline_version: RECENCY_BASELINE_VERSION.to_string(),
        surface: set.surface,
        recency_bound: set.recency_bound,
        falsification_pairs_failed: 0,
        positive_controls_passed: 0,
        delivered_ids: u32::try_from(delivered.len()).expect("bounded by the log"),
    };
    let of = |role| set.pairs.iter().filter(move |p| p.task.role == role);
    for pair in of(TaskRole::Falsification) {
        if superset(pair) {
            return blocked(BaselineFailure::DeliveredFalsifier {
                task: pair.task.id.clone(),
            });
        }
        contrast.falsification_pairs_failed += 1;
    }
    for pair in of(TaskRole::PositiveControl) {
        if !superset(pair) {
            return blocked(BaselineFailure::MissedPositiveControl {
                task: pair.task.id.clone(),
            });
        }
        contrast.positive_controls_passed += 1;
    }
    Ok(BaselineVerdict::Established { contrast })
}
