//! The shrinker: ddmin over paired worlds under a pinned failure predicate,
//! the replay-effect protocol, and the witness package around the result.

mod support;

use std::collections::BTreeSet;

use eval_core::{
    APPLICATION_CRASH, CandidateVerdict, Cut, Destination, Element, EvaluatedSurface, EventId,
    EventLog, FailureClass, FailurePredicate, FaultAction, FaultEpisode, FaultScope, KillLabel,
    MAX_OUTSTANDING_REPLAY_EFFECTS, MAX_VALID_TIME_MS, Minimality, Mode, NotEstablishedReason,
    Oracle, Payload, Query, ReplayEffects, ReplayOutcome, ReplayRefused, ReplayRequest,
    RepositorySpec, Scenario, Sensitivity, ServedClass, SessionSpec, ShrinkRefused, StoreFamily,
    TEST_BINARY_CHILD, Task, TaskRole, Transformation, UnknownReason, Visibility, WitnessClass,
    WorldConfig, classify_replay, serialize_spec, shrink,
};
use serde_json::Value;
use support::{WORLD_EPOCH_MS, WORLD_SEED, world_config};

const FRESH_SEED: u64 = WORLD_SEED ^ 0xABCD;
const PROFILE: &str = "profile-digest";
const CUT: Cut = Cut::AtQuiescence;
const BUDGET: u64 = 400;

fn fresh_config() -> WorldConfig {
    WorldConfig {
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
        epoch_ms: WORLD_EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 64,
        planted: Vec::new(),
    }
}

fn generate(seed: u64, config: &WorldConfig) -> EventLog {
    eval_core::generate_all(seed, config, Mode::Generate)
        .unwrap()
        .log
}

fn query() -> Query {
    Query {
        valid_time_ms: MAX_VALID_TIME_MS,
        observation_time_ms: MAX_VALID_TIME_MS,
        scope: BTreeSet::from([
            "session-0".to_string(),
            "session-1".to_string(),
            "repository-0".to_string(),
        ]),
        destination: Destination::Local,
        served: Some(ServedClass {
            sensitivity: Sensitivity::Normal,
            visibility: Visibility::Labeled,
            auto_inject: Visibility::Hidden,
            auto_search: Visibility::Hidden,
        }),
        registry_sensitivity: Sensitivity::Normal,
        max_events_per_log: 64,
    }
}

fn task(name: &str, role: TaskRole, evidence: &str) -> Task {
    Task {
        id: name.to_string(),
        role,
        query: query(),
        evidence: BTreeSet::from([EventId(evidence.to_string())]),
    }
}

fn episode(id: &str) -> FaultEpisode {
    let action = FaultAction::ProcessKill {
        cut: "local_staged".to_string(),
    };
    FaultEpisode {
        id: id.to_string(),
        trigger_step: 3,
        scope: FaultScope {
            store: StoreFamily::SearchProjection,
            operation: "acknowledge".to_string(),
        },
        heal: action.heal(),
        action,
        layer_contract: "search_catchup::EpisodeFault".to_string(),
        kill: Some(KillLabel {
            crash_model: APPLICATION_CRASH.to_string(),
            page_cache_intact: true,
            killed_process: TEST_BINARY_CHILD.to_string(),
        }),
    }
}

fn scenario() -> Scenario {
    Scenario {
        surface: EvaluatedSurface::Surface1,
        recency_bound: None,
        aged: generate(WORLD_SEED, &world_config()),
        natural_fresh: generate(FRESH_SEED, &fresh_config()),
        tasks: vec![
            task(
                "early-commit",
                TaskRole::Falsification,
                "repository:repository-0:0",
            ),
            task(
                "last-rename",
                TaskRole::PositiveControl,
                "repository:repository-0:11",
            ),
        ],
        episodes: vec![episode("kill-1"), episode("kill-2")],
    }
}

fn fixture() -> Value {
    serialize_spec()
}

fn oracle() -> Oracle {
    Oracle::RequiredCommits {
        failing_at: 3,
        slipping_at: 6,
    }
}

fn predicate(class: FailureClass) -> FailurePredicate {
    FailurePredicate {
        oracle: oracle().name(),
        checkpoint: CUT,
        profile_digest: PROFILE.to_string(),
        witness_class: WitnessClass::Failure { class },
    }
}

/// The in-process replay: the planted oracle over the compiled candidate.
fn evaluate(request: ReplayRequest<'_>) -> ReplayOutcome {
    oracle().evaluate(request.set, &fixture(), CUT, PROFILE)
}

fn commits(log: &EventLog) -> usize {
    log.events
        .iter()
        .filter(|event| matches!(event.payload, Payload::Commit { .. }))
        .count()
}

#[test]
fn classify_keeps_unknown_unknown_for_every_reason() {
    let expected = predicate(FailureClass::Interference);
    for reason in UnknownReason::ALL {
        let verdict = classify_replay(&expected, &ReplayOutcome::Unknown { reason });
        assert_eq!(verdict, CandidateVerdict::Unknown { reason });
    }
    assert_eq!(
        classify_replay(&expected, &ReplayOutcome::Passed),
        CandidateVerdict::NotReproduced
    );
    let slipped = predicate(FailureClass::DurableState);
    assert_eq!(
        classify_replay(
            &expected,
            &ReplayOutcome::Failed {
                predicate: slipped.clone()
            }
        ),
        CandidateVerdict::Slipped { observed: slipped }
    );
    let mut other_cut = expected.clone();
    other_cut.checkpoint = Cut::EndOfRun;
    assert!(matches!(
        classify_replay(
            &expected,
            &ReplayOutcome::Failed {
                predicate: other_cut
            }
        ),
        CandidateVerdict::Slipped { .. }
    ));
}

#[test]
fn shrink_preserves_the_predicate_and_rejects_slipped_candidates() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();

    assert!(commits(&minimized.aged) < commits(&original.aged));
    assert_eq!(
        commits(&minimized.aged),
        6,
        "one fewer commit slips the class"
    );
    assert!(
        minimized.episodes.is_empty(),
        "episodes the oracle ignores are removed first"
    );
    assert_eq!(
        report.minimality,
        Minimality::OneMinimal {
            transformations: vec![
                Transformation::FaultEpisodeRemoval,
                Transformation::EventDeletion
            ],
        }
    );
    assert_eq!(report.minimized_digest, minimized.digest());

    let set = minimized
        .compile(&fixture())
        .expect("the minimized pair is valid");
    assert_eq!(
        oracle().evaluate(&set, &fixture(), CUT, PROFILE),
        ReplayOutcome::Failed {
            predicate: expected.clone()
        },
        "original and minimized agree"
    );

    let slipped: Vec<_> = report
        .candidates
        .iter()
        .filter(|record| matches!(record.verdict, CandidateVerdict::Slipped { .. }))
        .collect();
    assert!(
        !slipped.is_empty(),
        "a candidate with five commits still fails, differently"
    );
    for record in &slipped {
        let CandidateVerdict::Slipped { observed } = &record.verdict else {
            unreachable!()
        };
        assert_eq!(
            observed.witness_class,
            WitnessClass::Failure {
                class: FailureClass::DurableState
            }
        );
        assert!(
            !report.deleted.is_superset(&record.deleted),
            "a slipped deletion set was not accepted"
        );
    }
    assert!(
        report
            .candidates
            .iter()
            .any(|record| matches!(record.verdict, CandidateVerdict::InvalidPair { .. })),
        "deleting the evidence makes the pair invalid"
    );
    assert_eq!(report.unknown_candidates, 0);
    assert_eq!(
        report.deleted,
        report
            .candidates
            .iter()
            .find(|r| r.scenario_digest == report.minimized_digest)
            .unwrap()
            .deleted
    );
}

#[test]
fn pair_validity_is_recomputed_for_every_candidate() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    for record in &report.candidates {
        let candidate = original.without(&record.deleted);
        let compiled = candidate.compile(&fixture());
        match &record.verdict {
            CandidateVerdict::InvalidPair { .. } => assert!(compiled.is_err()),
            _ => {
                let set = compiled.expect("a replayed candidate compiled");
                let deleted: BTreeSet<&EventId> = record
                    .deleted
                    .iter()
                    .filter_map(|e| match e {
                        Element::Event { id } => Some(id),
                        Element::Episode { .. } => None,
                    })
                    .collect();
                for pair in &set.pairs {
                    for event in pair.fresh.events.iter().chain(&pair.fresh_minimal.events) {
                        assert!(
                            !deleted.contains(&event.id),
                            "both worlds are shrunk together"
                        );
                    }
                }
            }
        }
    }
    let set = minimized.compile(&fixture()).unwrap();
    assert!(set.pairs.iter().all(|pair| !pair.task.evidence.is_empty()));
}

#[test]
fn an_unknown_replay_is_kept_and_never_becomes_not_reproduced() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let stubborn = Element::Event {
        id: EventId("repository:repository-0:2".to_string()),
    };
    let mut replay = |request: ReplayRequest<'_>| {
        if request
            .scenario
            .aged
            .events
            .iter()
            .all(|e| Element::Event { id: e.id.clone() } != stubborn)
        {
            return ReplayOutcome::Unknown {
                reason: UnknownReason::ChildExitedBeforeBarrier,
            };
        }
        evaluate(request)
    };
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut replay).unwrap();
    assert!(report.unknown_candidates > 0);
    assert!(
        minimized
            .aged
            .events
            .iter()
            .any(|e| Element::Event { id: e.id.clone() } == stubborn),
        "an element whose deletion is unknown stays"
    );
    assert!(report.candidates.iter().all(|record| {
        let deleted_stubborn = record.deleted.contains(&stubborn);
        !(deleted_stubborn && record.verdict == CandidateVerdict::NotReproduced)
    }));
    assert!(matches!(
        report.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { count: 1 }
        }
    ));
}

#[test]
fn an_exhausted_replay_budget_is_unknown_not_not_reproduced() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (_, report) = shrink(&original, &fixture(), &expected, 5, &mut evaluate).unwrap();
    assert_eq!(report.replays, 5);
    assert!(report.candidates.iter().any(|record| record.verdict
        == CandidateVerdict::Unknown {
            reason: UnknownReason::ReplayBudgetExhausted
        }));
    assert_eq!(
        report.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::ReplayBudgetExhausted
        }
    );
}

#[test]
fn an_original_that_does_not_reproduce_is_refused() {
    let original = scenario();
    let refused = shrink(
        &original,
        &fixture(),
        &predicate(FailureClass::Reasoning),
        BUDGET,
        &mut evaluate,
    );
    assert!(matches!(
        refused,
        Err(ShrinkRefused::OriginalNotReproduced {
            verdict: CandidateVerdict::Slipped { .. }
        })
    ));
    let mut passing = |_: ReplayRequest<'_>| ReplayOutcome::Passed;
    assert_eq!(
        shrink(
            &original,
            &fixture(),
            &predicate(FailureClass::Interference),
            BUDGET,
            &mut passing
        )
        .err(),
        Some(ShrinkRefused::OriginalNotReproduced {
            verdict: CandidateVerdict::NotReproduced
        })
    );
}

#[test]
fn replay_effects_are_bounded_and_a_premature_verdict_is_refused() {
    let mut effects = ReplayEffects::new(MAX_OUTSTANDING_REPLAY_EFFECTS);
    for index in 0..MAX_OUTSTANDING_REPLAY_EFFECTS {
        effects.issue(&format!("candidate-{index}")).unwrap();
    }
    assert_eq!(
        effects.issue("one-too-many"),
        Err(ReplayRefused::OutstandingBound {
            bound: MAX_OUTSTANDING_REPLAY_EFFECTS
        })
    );
    assert_eq!(
        effects.outcome("candidate-0"),
        Err(ReplayRefused::Outstanding {
            key: "candidate-0".to_string()
        }),
        "classifying before the replay answered is premature"
    );
    assert_eq!(
        effects.retry("candidate-0"),
        Ok(2),
        "a retry keeps its receipt key"
    );
    effects.cancel("candidate-0").unwrap();
    let cancelled = effects.outcome("candidate-0").unwrap();
    assert_eq!(
        classify_replay(&predicate(FailureClass::Interference), cancelled),
        CandidateVerdict::Unknown {
            reason: UnknownReason::Cancelled
        }
    );
    assert_eq!(
        effects.issue("candidate-0"),
        Err(ReplayRefused::AlreadyResolved {
            key: "candidate-0".to_string()
        })
    );
    assert_eq!(
        effects.retry("candidate-0"),
        Err(ReplayRefused::AlreadyResolved {
            key: "candidate-0".to_string()
        })
    );
    assert_eq!(
        effects.resolve("never-issued", ReplayOutcome::Passed),
        Err(ReplayRefused::UnknownKey {
            key: "never-issued".to_string()
        })
    );
    effects.issue("one-too-many").unwrap();
}
