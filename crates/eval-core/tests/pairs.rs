//! The paired-world compiler, its controls, and the recency baseline.

mod support;

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use eval_core::{
    ArmKind, Baseline, BaselineFailure, BaselineVerdict, CausalEdge, Coverage, Destination,
    EvaluatedSurface, EventId, EventLog, LogError, MAX_VALID_TIME_MS, Mode,
    NATURAL_FRESH_ENTITY_TAG, PAIRING_POLICY_VERSION, Pair, PairError, PairSet, PairSetInput,
    Query, RECENCY_BASELINE_VERSION, RepositorySpec, Sensitivity, ServedClass, SessionSpec,
    StopCondition, Suite, Task, TaskRole, Verdict, Visibility, WorldConfig, check_recency_baseline,
    compile_pair_set, recency_bound, reduce, serialize_spec,
};
use serde_json::{Value, json};
use support::{WORLD_EPOCH_MS as EPOCH_MS, WORLD_SEED as SEED, world_config as config};

const END: i64 = MAX_VALID_TIME_MS;

type Compile<'a> = Box<dyn Fn() -> Result<PairSet, PairError> + 'a>;
type Mutate = Box<dyn Fn(&mut PairSet)>;

fn labeled() -> ServedClass {
    ServedClass {
        sensitivity: Sensitivity::Normal,
        visibility: Visibility::Labeled,
        auto_inject: Visibility::Hidden,
        auto_search: Visibility::Hidden,
    }
}

fn aged() -> EventLog {
    eval_core::generate_all(SEED, &config(), Mode::Generate)
        .unwrap()
        .log
}

/// Three messages and two commits under another seed: a history nobody
/// copied out of the aged one.
fn natural_fresh() -> EventLog {
    let short = WorldConfig {
        sessions: vec![SessionSpec {
            messages: 3,
            tool_span_every: 2,
            correction_every: 0,
            invalidation_every: 0,
        }],
        repositories: vec![RepositorySpec {
            commits: 2,
            rename_every: 0,
        }],
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 64,
    };
    eval_core::generate_all(SEED ^ 0xABCD, &short, Mode::Generate)
        .unwrap()
        .log
}

fn id(text: &str) -> EventId {
    EventId(text.to_string())
}

fn k(bound: u32) -> Option<NonZeroU32> {
    NonZeroU32::new(bound)
}

fn query(valid_time_ms: i64, max_events_per_log: u32) -> Query {
    Query {
        valid_time_ms,
        observation_time_ms: END,
        scope: BTreeSet::from([
            "session-0".to_string(),
            "session-1".to_string(),
            "repository-0".to_string(),
        ]),
        destination: Destination::Local,
        served: Some(labeled()),
        registry_sensitivity: Sensitivity::Normal,
        max_events_per_log,
    }
}

fn task(name: &str, role: TaskRole, evidence: &[&str]) -> Task {
    Task {
        id: name.to_string(),
        role,
        query: query(END, 64),
        evidence: evidence.iter().map(|e| id(e)).collect(),
    }
}

/// The commit at the epoch is the falsifier (never corrected; every later
/// unit is more recent) and the last rename is the positive control.
fn tasks() -> Vec<Task> {
    vec![
        task(
            "early-commit",
            TaskRole::Falsification,
            &["repository:repository-0:0"],
        ),
        task(
            "last-rename",
            TaskRole::PositiveControl,
            &["repository:repository-0:11"],
        ),
        task("mid-message", TaskRole::Plain, &["session:session-1:4"]),
    ]
}

fn fixture() -> Value {
    serialize_spec()
}

fn compile_with(
    surface: EvaluatedSurface,
    bound: Option<NonZeroU32>,
    aged: &EventLog,
    natural_fresh: &EventLog,
    tasks: &[Task],
) -> Result<PairSet, PairError> {
    compile_pair_set(PairSetInput {
        surface,
        declared_bound: bound,
        aged,
        natural_fresh,
        fixture: &fixture(),
        tasks,
    })
}

fn compile(
    surface: EvaluatedSurface,
    bound: Option<NonZeroU32>,
    tasks: &[Task],
) -> Result<PairSet, PairError> {
    compile_with(surface, bound, &aged(), &natural_fresh(), tasks)
}

fn ids(log: &EventLog) -> Vec<&str> {
    log.events.iter().map(|e| e.id.0.as_str()).collect()
}

/// The fresh arm's units on the control's entities: the independent history.
fn independent_of(pair: &Pair) -> Vec<&eval_core::Event> {
    pair.fresh
        .events
        .iter()
        .filter(|e| {
            e.entity_id
                .ends_with(&format!("~{NATURAL_FRESH_ENTITY_TAG}"))
        })
        .collect()
}

#[test]
fn a_pair_set_carries_a_falsification_pair_and_a_natural_fresh_control() {
    let set = compile(EvaluatedSurface::QueryRoute, k(3), &tasks()).unwrap();
    assert_eq!(set.pairing_policy_version, PAIRING_POLICY_VERSION);
    assert_eq!((set.recency_bound, set.pairs.len()), (3, 3));
    let earliest = set.aged.events[0].valid_time_ms;
    assert!(
        earliest < set.aged_median_ms,
        "the early rule has room to bite"
    );
    let mut coverage = Coverage::default();
    coverage
        .record("wm_pair_set_aged_history_spans_median")
        .unwrap();

    let falsifier = &set.pairs[0];
    assert_eq!(falsifier.task.role, TaskRole::Falsification);
    // The fresh arm holds the truth plus units the aged history never had,
    // and the reducer requires at least one of those on the fresh arm: the
    // control competes rather than sitting out of scope.
    let aged_ids: BTreeSet<&EventId> = set.aged.events.iter().map(|e| &e.id).collect();
    let control: BTreeSet<&EventId> = falsifier
        .fresh
        .events
        .iter()
        .map(|e| &e.id)
        .filter(|i| !aged_ids.contains(i))
        .collect();
    assert_eq!(control.len(), natural_fresh().events.len());
    assert!(
        control
            .iter()
            .all(|i| i.0.contains(NATURAL_FRESH_ENTITY_TAG))
    );
    let fresh_truth = reduce(&falsifier.fresh, &fixture(), &set.fresh_query).unwrap();
    assert!(
        fresh_truth
            .required
            .contains(&id("repository:repository-0:0"))
    );
    assert!(control.iter().any(|i| fresh_truth.required.contains(i)));
    assert!(
        set.fresh_query.scope.len() > falsifier.task.query.scope.len(),
        "the control's entities join the scope"
    );
    falsifier.fresh.validate(64).unwrap();
    // The ceiling is the truth alone, since nothing precedes the first commit.
    assert_eq!(
        ids(&falsifier.fresh_minimal),
        vec!["repository:repository-0:0"]
    );
    // A message that cites a commit drags the commit and the commit's own
    // parent into the ceiling, and nothing else.
    let mid = &set.pairs[2];
    assert_eq!(
        ids(&mid.fresh_minimal),
        vec![
            "repository:repository-0:0",
            "repository:repository-0:1",
            "session:session-1:4"
        ]
    );
    mid.fresh_minimal.validate(64).unwrap();
    assert_eq!(
        set.recency_window,
        vec![
            id("repository:repository-0:11"),
            id("repository:repository-0:10"),
            id("repository:repository-0:9"),
        ],
        "the three most recent eligible units, most recent first"
    );
    assert_eq!(
        serde_json::to_value(ArmKind::FreshMinimal).unwrap(),
        json!("fresh_minimal")
    );
    let round: PairSet = serde_json::from_value(serde_json::to_value(&set).unwrap()).unwrap();
    assert_eq!(round, set);
    round.validate(&fixture()).unwrap();
}

#[test]
fn the_window_breaks_equal_times_by_linearization_order() {
    // At the epoch plus eleven seconds four eligible units share one valid
    // time; a window of two takes the two latest in linearization order.
    let set = compile(
        EvaluatedSurface::QueryRoute,
        k(2),
        &[
            task(
                "early-commit",
                TaskRole::Falsification,
                &["repository:repository-0:0"],
            ),
            Task {
                query: query(EPOCH_MS + 11_000, 64),
                ..task(
                    "tie",
                    TaskRole::PositiveControl,
                    &["session:session-0:6", "repository:repository-0:8"],
                )
            },
        ],
    );
    assert_eq!(
        set.unwrap_err(),
        PairError::MixedQueries { task: "tie".into() }
    );
    let mut tasks = vec![
        task(
            "early-commit",
            TaskRole::Falsification,
            &["repository:repository-0:0"],
        ),
        task(
            "tie",
            TaskRole::PositiveControl,
            &["session:session-0:6", "repository:repository-0:8"],
        ),
    ];
    for t in &mut tasks {
        t.query.valid_time_ms = EPOCH_MS + 11_000;
    }
    let set = compile(EvaluatedSurface::QueryRoute, k(2), &tasks).unwrap();
    let shared_time: Vec<&str> = set
        .aged
        .events
        .iter()
        .filter(|e| e.valid_time_ms == EPOCH_MS + 11_000)
        .map(|e| e.id.0.as_str())
        .collect();
    assert_eq!(shared_time.len(), 4);
    assert_eq!(
        set.recency_window,
        vec![id("session:session-0:6"), id("repository:repository-0:8")]
    );
    assert!(matches!(
        check_recency_baseline(&set, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Established { .. }
    ));
}

#[test]
fn the_recency_baseline_misses_every_falsifier_or_blocks_suite_b() {
    let set = compile(EvaluatedSurface::QueryRoute, k(3), &tasks()).unwrap();
    assert!(
        set.pairs
            .iter()
            .any(|p| p.task.role == TaskRole::Falsification)
    );
    let mut coverage = Coverage::default();
    coverage
        .record("wm_baseline_ran_on_falsification_pair")
        .unwrap();
    let BaselineVerdict::Established { contrast } =
        check_recency_baseline(&set, &fixture(), Baseline::Versioned).unwrap()
    else {
        panic!("contrast established");
    };
    assert_eq!(contrast.baseline_version, RECENCY_BASELINE_VERSION);
    assert_eq!(
        (
            contrast.falsification_pairs_failed,
            contrast.positive_controls_passed,
            contrast.delivered_ids
        ),
        (1, 1, 3),
        "one window, counted once"
    );

    // On a thirty-four-event world the surface-1 window (100) still holds the
    // epoch commit: the falsifier is delivered, which is stop condition (b).
    let short = compile(EvaluatedSurface::Surface1, None, &tasks()).unwrap();
    assert_eq!(short.recency_bound, 100);
    assert_eq!(
        check_recency_baseline(&short, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Blocked {
            condition: StopCondition::B,
            failure: BaselineFailure::DeliveredFalsifier {
                task: "early-commit".to_string(),
            },
        }
    );
    let table = [
        (Suite::A, false),
        (Suite::B, true),
        (Suite::C, false),
        (Suite::D, true),
    ];
    for condition in [StopCondition::A, StopCondition::B, StopCondition::C] {
        for (suite, suppressed) in table {
            assert_eq!(
                condition.suppresses(suite),
                suppressed,
                "{condition:?} {suite:?}"
            );
        }
    }
    assert_eq!(serde_json::to_value(Suite::D).unwrap(), json!("d"));
}

#[test]
fn the_recency_baseline_delivers_a_positive_control_or_is_vacuous() {
    let set = compile(EvaluatedSurface::QueryRoute, k(3), &tasks()).unwrap();
    let control = &set.pairs[1];
    assert_eq!(control.task.role, TaskRole::PositiveControl);
    assert!(!set.recency_window.is_empty());
    let mut coverage = Coverage::default();
    coverage
        .record("wm_baseline_ran_on_positive_control_pair")
        .unwrap();
    assert!(matches!(
        check_recency_baseline(&set, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Established { .. }
    ));

    // A control whose evidence a narrower window no longer covers is missed
    // by the versioned baseline itself, from compiler output alone.
    let narrow = compile(EvaluatedSurface::QueryRoute, k(1), &tasks()).unwrap();
    assert!(matches!(
        check_recency_baseline(&narrow, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Established { .. }
    ));
    let mut tasks = tasks();
    tasks[1] = task(
        "second-last",
        TaskRole::PositiveControl,
        &["repository:repository-0:10"],
    );
    let missed = compile(EvaluatedSurface::QueryRoute, k(1), &tasks).unwrap();
    assert_eq!(
        check_recency_baseline(&missed, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Blocked {
            condition: StopCondition::B,
            failure: BaselineFailure::MissedPositiveControl {
                task: "second-last".to_string(),
            },
        }
    );

    // The negative control: an always-empty baseline misses every falsifier
    // for free, and both classes empty is a refusal, never a pass.
    assert_eq!(
        check_recency_baseline(&set, &fixture(), Baseline::AlwaysEmpty).unwrap(),
        BaselineVerdict::Blocked {
            condition: StopCondition::B,
            failure: BaselineFailure::Vacuous,
        }
    );
    // Vacuity is judged before the falsifiers, so an empty baseline never
    // reads as "missed positive control" either.
    let verdict = check_recency_baseline(&missed, &fixture(), Baseline::AlwaysEmpty).unwrap();
    assert_eq!(
        serde_json::to_value(&verdict).unwrap(),
        json!({"kind": "blocked", "condition": "b", "failure": {"kind": "vacuous"}})
    );
    assert!(
        serde_json::from_value::<BaselineVerdict>(json!({"kind": "established", "contrast": {
            "baseline_version": RECENCY_BASELINE_VERSION, "surface": "surface1", "recency_bound": 100,
            "falsification_pairs_failed": 1, "positive_controls_passed": 1, "delivered_ids": 5,
            "proven": true
        }}))
        .is_err()
    );
}

#[test]
fn a_long_aged_history_pushes_the_falsifier_out_of_the_surface_1_window() {
    let big = WorldConfig {
        sessions: vec![
            SessionSpec {
                messages: 40,
                tool_span_every: 3,
                correction_every: 7,
                invalidation_every: 11,
            },
            SessionSpec {
                messages: 40,
                tool_span_every: 4,
                correction_every: 9,
                invalidation_every: 0,
            },
        ],
        repositories: vec![RepositorySpec {
            commits: 40,
            rename_every: 5,
        }],
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 256,
    };
    let aged = eval_core::generate_all(SEED, &big, Mode::Generate)
        .unwrap()
        .log;
    let last = aged.events.last().unwrap().id.0.clone();
    let mut tasks = vec![
        task(
            "early-commit",
            TaskRole::Falsification,
            &["repository:repository-0:0"],
        ),
        task("last-unit", TaskRole::PositiveControl, &[&last]),
    ];
    for t in &mut tasks {
        t.query.max_events_per_log = 256;
    }
    let set = compile_with(
        EvaluatedSurface::Surface1,
        k(100),
        &aged,
        &natural_fresh(),
        &tasks,
    )
    .unwrap();
    assert_eq!(set.recency_window.len(), 100);
    let BaselineVerdict::Established { contrast } =
        check_recency_baseline(&set, &fixture(), Baseline::Versioned).unwrap()
    else {
        panic!("the epoch commit is outside a hundred more recent units");
    };
    assert_eq!(
        (
            contrast.surface,
            contrast.recency_bound,
            contrast.falsification_pairs_failed,
            contrast.positive_controls_passed,
            contrast.delivered_ids
        ),
        (EvaluatedSurface::Surface1, 100, 1, 1, 100)
    );
}

#[test]
fn pair_validation_refuses_what_would_make_the_controls_vacuous() {
    let base = tasks();
    let aged = aged();
    let fresh = natural_fresh();
    let at = |cut: i64, tasks: &mut Vec<Task>| {
        for t in tasks.iter_mut() {
            t.query.valid_time_ms = cut;
        }
    };
    let cases: Vec<(&str, Compile<'_>, PairError)> = vec![
        (
            "an empty aged history",
            Box::new(|| {
                let empty = EventLog {
                    events: vec![],
                    causal_edges: vec![],
                    ..aged.clone()
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &empty, &fresh, &base)
            }),
            PairError::EmptyAged,
        ),
        (
            "an empty natural-fresh history",
            Box::new(|| {
                let empty = EventLog {
                    events: vec![],
                    causal_edges: vec![],
                    ..fresh.clone()
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &empty, &base)
            }),
            PairError::EmptyNaturalFresh,
        ),
        (
            "a truncation posing as natural-fresh",
            Box::new(|| {
                let keep: BTreeSet<EventId> =
                    aged.events[..6].iter().map(|e| e.id.clone()).collect();
                let prefix = EventLog {
                    events: aged.events[..6].to_vec(),
                    causal_edges: aged
                        .causal_edges
                        .iter()
                        .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
                        .cloned()
                        .collect(),
                    ..aged.clone()
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &prefix, &base)
            }),
            PairError::NaturalFreshCopiedFromAged { at: 0 },
        ),
        (
            "a relabelled slice from the middle posing as natural-fresh",
            Box::new(|| {
                let slice = EventLog {
                    events: aged.events[6..11].to_vec(),
                    causal_edges: vec![],
                    ..aged.clone()
                }
                .on_distinct_entities("elsewhere")
                .unwrap();
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &slice, &base)
            }),
            PairError::NaturalFreshCopiedFromAged { at: 6 },
        ),
        (
            "a superseded falsifier",
            Box::new(|| {
                // At a cut before its correction the first message is required;
                // the aged history still corrects it later.
                let mut tasks = vec![
                    task(
                        "corrected-later",
                        TaskRole::Falsification,
                        &["session:session-0:0"],
                    ),
                    task(
                        "control",
                        TaskRole::PositiveControl,
                        &["repository:repository-0:3"],
                    ),
                ];
                at(EPOCH_MS + 3_000, &mut tasks);
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::SupersededFalsifier {
                task: "corrected-later".to_string(),
                id: id("session:session-0:0"),
                by: id("session:session-0:4"),
            },
        ),
        (
            "a late truth claiming to be early",
            Box::new(|| {
                let mut tasks = tasks();
                tasks[0] = task(
                    "late",
                    TaskRole::Falsification,
                    &["repository:repository-0:11"],
                );
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::TruthNotEarly {
                task: "late".to_string(),
                id: id("repository:repository-0:11"),
                valid_time_ms: EPOCH_MS + 15_000,
                median_ms: EPOCH_MS + 5_000,
            },
        ),
        (
            "a falsifier that is both late and corrected reports late first",
            Box::new(|| {
                // At the epoch plus six seconds the second message sits on the
                // median and is corrected seven seconds later.
                let mut tasks = vec![
                    task(
                        "late-and-corrected",
                        TaskRole::Falsification,
                        &["session:session-0:1"],
                    ),
                    task(
                        "control",
                        TaskRole::PositiveControl,
                        &["repository:repository-0:6"],
                    ),
                ];
                at(EPOCH_MS + 6_000, &mut tasks);
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::TruthNotEarly {
                task: "late-and-corrected".to_string(),
                id: id("session:session-0:1"),
                valid_time_ms: EPOCH_MS + 5_000,
                median_ms: EPOCH_MS + 5_000,
            },
        ),
        (
            "evidence the reducer judges superseded",
            Box::new(|| {
                let mut tasks = tasks();
                tasks[2] = task("superseded", TaskRole::Plain, &["session:session-0:0"]);
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::EvidenceNotRequired {
                task: "superseded".to_string(),
                id: id("session:session-0:0"),
                verdict: Some(Verdict::Superseded),
            },
        ),
        (
            "evidence the log never had",
            Box::new(|| {
                let mut tasks = tasks();
                tasks[2] = task("absent", TaskRole::Plain, &["session:session-9:0"]);
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::EvidenceNotRequired {
                task: "absent".to_string(),
                id: id("session:session-9:0"),
                verdict: None,
            },
        ),
        (
            "tasks at different cuts",
            Box::new(|| {
                let mut tasks = tasks();
                tasks[2].query.observation_time_ms = EPOCH_MS + 20_000;
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::MixedQueries {
                task: "mid-message".to_string(),
            },
        ),
        (
            "no tasks",
            Box::new(|| compile(EvaluatedSurface::QueryRoute, k(3), &[])),
            PairError::NoTasks,
        ),
        (
            "no falsification pair",
            Box::new(|| compile(EvaluatedSurface::QueryRoute, k(3), &base[1..])),
            PairError::NoFalsificationPair,
        ),
        (
            "no positive control",
            Box::new(|| {
                compile(
                    EvaluatedSurface::QueryRoute,
                    k(3),
                    &[base[0].clone(), base[2].clone()],
                )
            }),
            PairError::NoPositiveControl,
        ),
        (
            "a duplicate task",
            Box::new(|| {
                compile(
                    EvaluatedSurface::QueryRoute,
                    k(3),
                    &[base[0].clone(), base[0].clone(), base[1].clone()],
                )
            }),
            PairError::DuplicateTask {
                task: "early-commit".to_string(),
            },
        ),
        (
            "empty evidence",
            Box::new(|| {
                let mut tasks = tasks();
                tasks[2].evidence.clear();
                compile(EvaluatedSurface::QueryRoute, k(3), &tasks)
            }),
            PairError::EmptyEvidence {
                task: "mid-message".to_string(),
            },
        ),
        (
            "an aged history too short to have an early half",
            Box::new(|| {
                let flat = EventLog {
                    events: aged.events[..3].to_vec(),
                    causal_edges: vec![],
                    ..aged.clone()
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &flat, &fresh, &base)
            }),
            PairError::AgedHistoryTooShort {
                median_ms: EPOCH_MS,
            },
        ),
        (
            "a natural-fresh history the task's cut leaves inert",
            Box::new(|| {
                let late = EventLog {
                    events: fresh
                        .events
                        .iter()
                        .map(|e| eval_core::Event {
                            valid_time_ms: e.valid_time_ms + 1_000_000,
                            observation_time_ms: e.observation_time_ms + 1_000_000,
                            ..e.clone()
                        })
                        .collect(),
                    ..fresh.clone()
                };
                let mut tasks = vec![
                    task(
                        "early-commit",
                        TaskRole::Falsification,
                        &["repository:repository-0:0"],
                    ),
                    task(
                        "control",
                        TaskRole::PositiveControl,
                        &["repository:repository-0:11"],
                    ),
                ];
                at(EPOCH_MS + 15_000, &mut tasks);
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &late, &tasks)
            }),
            PairError::NaturalFreshInert {
                task: "early-commit".to_string(),
            },
        ),
        (
            "a natural-fresh history naming an event it does not hold",
            Box::new(|| {
                let dangling = aged.without(&id("session:session-1:0"));
                let dangling = EventLog {
                    events: dangling
                        .events
                        .iter()
                        .filter(|e| e.entity_id == "session-1")
                        .cloned()
                        .collect(),
                    causal_edges: vec![],
                    ..dangling
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &dangling, &base)
            }),
            PairError::Log(LogError::DanglingReference {
                id: id("session:session-1:1"),
                target: id("repository:repository-0:0"),
            }),
        ),
        (
            "a natural-fresh history with a causal edge to an event it does not hold",
            Box::new(|| {
                let mut dangling = fresh.clone();
                dangling.causal_edges.push(CausalEdge {
                    from: fresh.events[0].id.clone(),
                    to: id("session:session-9:0"),
                });
                dangling.causal_edges.sort();
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &dangling, &base)
            }),
            PairError::Log(LogError::DanglingEdge {
                edge: CausalEdge {
                    from: fresh.events[0].id.clone(),
                    to: id("session:session-9:0"),
                },
            }),
        ),
        (
            "a natural-fresh history under another schema",
            Box::new(|| {
                let stale = EventLog {
                    schema: "eval-event/v0".to_string(),
                    ..fresh.clone()
                };
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &stale, &base)
            }),
            PairError::Log(LogError::SchemaMismatch {
                field: "schema",
                found: "eval-event/v0".to_string(),
            }),
        ),
        (
            "a shuffled slice of the aged history posing as natural-fresh",
            Box::new(|| {
                // Normalization would sort it back into the copy the
                // independence check refuses, so it is validated as supplied.
                let mut shuffled = EventLog {
                    events: aged.events[6..11].to_vec(),
                    causal_edges: vec![],
                    ..aged.clone()
                };
                shuffled.events.reverse();
                compile_with(EvaluatedSurface::QueryRoute, k(3), &aged, &shuffled, &base)
            }),
            PairError::Log(LogError::NotLinearized { position: 1 }),
        ),
        (
            "a control that narrows the scope to its own entity",
            Box::new(|| {
                // Under the shared scope the last session-0 message is behind a
                // window of one; alone in its scope it is the window.
                let mut tasks = vec![
                    task(
                        "early-commit",
                        TaskRole::Falsification,
                        &["repository:repository-0:0"],
                    ),
                    task(
                        "own-scope",
                        TaskRole::PositiveControl,
                        &["session:session-0:10"],
                    ),
                ];
                tasks[1].query.scope = BTreeSet::from(["session-0".to_string()]);
                compile(EvaluatedSurface::QueryRoute, k(1), &tasks)
            }),
            PairError::MixedQueries {
                task: "own-scope".to_string(),
            },
        ),
    ];
    for (name, run, expected) in cases {
        assert_eq!(run().unwrap_err(), expected, "{name}");
    }
}

#[test]
fn a_set_read_back_must_be_one_the_compiler_could_have_produced() {
    let set = compile(EvaluatedSurface::QueryRoute, k(3), &tasks()).unwrap();
    let mutations: Vec<(&str, Mutate, PairError)> = vec![
        (
            "another policy version",
            Box::new(|s| s.pairing_policy_version = "eval-pairing/v0".to_string()),
            PairError::Tampered {
                field: "pairing_policy_version",
            },
        ),
        (
            "surface 1 under a bound of three",
            Box::new(|s| s.surface = EvaluatedSurface::Surface1),
            PairError::Tampered {
                field: "recency_bound",
            },
        ),
        (
            "a zero bound",
            Box::new(|s| s.recency_bound = 0),
            PairError::Tampered {
                field: "recency_bound",
            },
        ),
        (
            "a window wider than the bound",
            Box::new(|s| s.recency_window.push(id("repository:repository-0:8"))),
            PairError::Tampered {
                field: "recency_window",
            },
        ),
        (
            "the widened query's scope cleared",
            Box::new(|s| s.fresh_query.scope.clear()),
            PairError::Tampered {
                field: "fresh_query",
            },
        ),
        (
            "the widened query's cut moved",
            Box::new(|s| s.fresh_query.valid_time_ms = EPOCH_MS),
            PairError::Tampered {
                field: "fresh_query",
            },
        ),
        (
            "the falsifier's evidence dropped from its fresh arm",
            Box::new(|s| {
                s.pairs[0].fresh = s.pairs[0].fresh.without(&id("repository:repository-0:0"))
            }),
            PairError::Tampered { field: "pairs" },
        ),
        (
            "the control's evidence dropped from its ceiling",
            Box::new(|s| {
                s.pairs[1].fresh_minimal = s.pairs[1]
                    .fresh_minimal
                    .without(&id("repository:repository-0:11"))
            }),
            PairError::Tampered { field: "pairs" },
        ),
        (
            "an eligible aged unit outside the closure added to one fresh arm",
            Box::new(|s| {
                let extra = s
                    .aged
                    .events
                    .iter()
                    .find(|e| e.id == id("session:session-1:4"))
                    .unwrap()
                    .clone();
                assert!(!s.pairs[0].fresh.events.iter().any(|e| e.id == extra.id));
                s.pairs[0].fresh.events.push(extra);
                s.pairs[0]
                    .fresh
                    .events
                    .sort_by(|a, b| a.key().cmp(&b.key()));
            }),
            PairError::Tampered { field: "pairs" },
        ),
        (
            "one causal edge of the control dropped from one fresh arm",
            Box::new(|s| {
                let control = independent_of(&s.pairs[0])[0].entity_id.clone();
                let edges = &mut s.pairs[0].fresh.causal_edges;
                let at = edges
                    .iter()
                    .position(|e| e.from.0.contains(&control))
                    .expect("the control has a causal edge");
                edges.remove(at);
            }),
            PairError::Tampered { field: "pairs" },
        ),
        (
            "one competitor dropped from one pair's fresh arm",
            Box::new(|s| {
                // The other pairs still hold it; the set no longer carries one
                // independent history.
                let competitor = independent_of(&s.pairs[0])[1].id.clone();
                s.pairs[0].fresh = s.pairs[0].fresh.without(&competitor);
            }),
            PairError::Tampered { field: "pairs" },
        ),
        (
            "the control emptied out of every fresh arm",
            Box::new(|s| {
                for pair in &mut s.pairs {
                    pair.fresh = pair.fresh_minimal.clone();
                }
            }),
            PairError::EmptyNaturalFresh,
        ),
        (
            "a relabelled slice of the aged history as every pair's control",
            Box::new(|s| {
                let slice = EventLog {
                    events: s.aged.events[6..11].to_vec(),
                    causal_edges: vec![],
                    ..s.aged.clone()
                }
                .on_distinct_entities(NATURAL_FRESH_ENTITY_TAG)
                .unwrap();
                for pair in &mut s.pairs {
                    let mut events = pair.fresh_minimal.events.clone();
                    events.extend(slice.events.iter().cloned());
                    events.sort_by(|a, b| a.key().cmp(&b.key()));
                    pair.fresh = EventLog {
                        events,
                        ..pair.fresh_minimal.clone()
                    };
                }
                s.fresh_query.scope = s.pairs[0].task.query.scope.clone();
                s.fresh_query
                    .scope
                    .extend(slice.events.iter().map(|e| e.entity_id.clone()));
            }),
            PairError::NaturalFreshCopiedFromAged { at: 6 },
        ),
        (
            "the positive control relabelled",
            Box::new(|s| s.pairs[1].task.role = TaskRole::Plain),
            PairError::NoPositiveControl,
        ),
        (
            "evidence emptied",
            Box::new(|s| s.pairs[2].task.evidence.clear()),
            PairError::EmptyEvidence {
                task: "mid-message".to_string(),
            },
        ),
        (
            "the median moved",
            Box::new(|s| s.aged_median_ms = 0),
            PairError::Tampered {
                field: "aged_median_ms",
            },
        ),
        (
            "a duplicate task",
            Box::new(|s| s.pairs[2].task.id = "early-commit".to_string()),
            PairError::DuplicateTask {
                task: "early-commit".to_string(),
            },
        ),
        (
            "a task at its own cut",
            Box::new(|s| s.pairs[2].task.query.valid_time_ms = EPOCH_MS + 3_000),
            PairError::MixedQueries {
                task: "mid-message".to_string(),
            },
        ),
        (
            "evidence the aged history lacks",
            Box::new(|s| s.pairs[2].task.evidence = BTreeSet::from([id("session:session-9:0")])),
            PairError::EvidenceNotRequired {
                task: "mid-message".to_string(),
                id: id("session:session-9:0"),
                verdict: None,
            },
        ),
        (
            "a falsifier's evidence moved late",
            Box::new(|s| {
                s.pairs[0].task.evidence = BTreeSet::from([id("repository:repository-0:11")])
            }),
            PairError::TruthNotEarly {
                task: "early-commit".to_string(),
                id: id("repository:repository-0:11"),
                valid_time_ms: EPOCH_MS + 15_000,
                median_ms: EPOCH_MS + 5_000,
            },
        ),
        (
            "a window naming an event the aged history lacks",
            Box::new(|s| s.recency_window[0] = id("session:session-9:0")),
            PairError::Tampered {
                field: "recency_window",
            },
        ),
        (
            "a window out of order",
            Box::new(|s| s.recency_window.reverse()),
            PairError::Tampered {
                field: "recency_window",
            },
        ),
        (
            "a window narrowed",
            Box::new(|s| s.recency_window.truncate(1)),
            PairError::Tampered {
                field: "recency_window",
            },
        ),
    ];
    for (name, mutate, expected) in mutations {
        let mut tampered = set.clone();
        mutate(&mut tampered);
        assert_eq!(
            tampered.validate(&fixture()),
            Err(expected.clone()),
            "{name}"
        );
        assert_eq!(
            check_recency_baseline(&tampered, &fixture(), Baseline::Versioned),
            Err(expected),
            "{name}: the check refuses before judging"
        );
    }
}

#[test]
fn a_window_edited_on_the_wire_cannot_manufacture_an_established_contrast() {
    // Surface 1's window of 100 holds every eligible unit of the 34-event
    // aged history, the epoch commit included, so the verdict is blocked.
    let set = compile(EvaluatedSurface::Surface1, None, &tasks()).unwrap();
    assert!(matches!(
        check_recency_baseline(&set, &fixture(), Baseline::Versioned).unwrap(),
        BaselineVerdict::Blocked { .. }
    ));
    // A window without the epoch commit misses the falsifier and delivers the
    // control; `check_recency_baseline` must reject the edited set.
    let mut edited: PairSet = serde_json::from_value(serde_json::to_value(&set).unwrap()).unwrap();
    edited
        .recency_window
        .retain(|i| *i != id("repository:repository-0:0"));
    assert_eq!(
        check_recency_baseline(&edited, &fixture(), Baseline::Versioned),
        Err(PairError::Tampered {
            field: "recency_window",
        })
    );
}

#[test]
fn every_surface_resolves_its_recency_bound_or_refuses() {
    assert_eq!(recency_bound(EvaluatedSurface::Surface1, None), Ok(100));
    assert_eq!(recency_bound(EvaluatedSurface::Surface1, k(100)), Ok(100));
    assert_eq!(
        recency_bound(EvaluatedSurface::Surface1, k(50)),
        Err(PairError::RecencyBoundConflict {
            surface: EvaluatedSurface::Surface1,
            pinned: 100,
            declared: 50,
        })
    );
    for surface in [
        EvaluatedSurface::Surface2,
        EvaluatedSurface::Surface3,
        EvaluatedSurface::QueryRoute,
        EvaluatedSurface::Packing,
    ] {
        assert_eq!(
            recency_bound(surface, None),
            Err(PairError::UnresolvedRecencyBound { surface }),
            "{surface:?} has no production constant to borrow"
        );
        assert_eq!(recency_bound(surface, k(7)), Ok(7));
    }
    assert_eq!(
        compile(EvaluatedSurface::Surface3, None, &tasks()).unwrap_err(),
        PairError::UnresolvedRecencyBound {
            surface: EvaluatedSurface::Surface3,
        }
    );
    assert_eq!(
        serde_json::to_value(EvaluatedSurface::QueryRoute).unwrap(),
        json!("query_route")
    );
}

#[test]
fn an_independent_history_moves_onto_its_own_entities_with_every_reference() {
    for seed in [SEED, SEED ^ 1, SEED ^ 0xABCD] {
        let aged = eval_core::generate_all(seed, &config(), Mode::Generate)
            .unwrap()
            .log;
        let moved = aged.on_distinct_entities("other").unwrap();
        moved.validate(64).unwrap();
        // The move is exactly the re-derivation on every ID, edge, and target.
        let rename = |id: &EventId| {
            let event = aged.events.iter().find(|e| e.id == *id).unwrap();
            EventId::derive(
                event.stream,
                &format!("{}~other", event.entity_id),
                event.local_seq,
            )
        };
        assert_eq!(
            moved
                .events
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>(),
            aged.events
                .iter()
                .map(|e| rename(&e.id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            moved
                .causal_edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect::<Vec<_>>(),
            aged.causal_edges
                .iter()
                .map(|e| (rename(&e.from), rename(&e.to)))
                .collect::<Vec<_>>()
        );
        let mut references = 0;
        for (moved, original) in moved.events.iter().zip(&aged.events) {
            match (moved.payload.reference(), original.payload.reference()) {
                (Some(target), Some(from)) => {
                    references += 1;
                    assert_eq!(*target, rename(from));
                }
                (None, None) => {}
                other => panic!("{other:?}"),
            }
            assert_eq!(moved.payload.content(), original.payload.content());
        }
        assert!(references > 10, "{seed:#x} exercises cites and corrections");
        let mut events = aged.events.clone();
        events.extend(moved.events.iter().cloned());
        let mut edges = aged.causal_edges.clone();
        edges.extend(moved.causal_edges.iter().cloned());
        events.sort_by(|a, b| a.key().cmp(&b.key()));
        edges.sort();
        let joined = EventLog {
            events,
            causal_edges: edges,
            ..aged.clone()
        };
        joined.validate(128).unwrap();
    }
    assert_eq!(
        aged().on_distinct_entities("a:b").unwrap_err(),
        LogError::InvalidEntityTag {
            tag: "a:b".to_string()
        }
    );
    let mut dangling = aged();
    let edge = CausalEdge {
        from: dangling.events[0].id.clone(),
        to: id("session:session-9:0"),
    };
    dangling.causal_edges.push(edge.clone());
    assert_eq!(
        dangling.on_distinct_entities("other").unwrap_err(),
        LogError::DanglingEdge { edge }
    );
}
