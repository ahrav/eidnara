//! The shrinker: ddmin over paired worlds under a pinned failure predicate
//! and the replay-effect ledger.

mod support;

use std::collections::BTreeSet;

use eval_core::{
    APPLICATION_CRASH, CandidateVerdict, Cut, Destination, Element, EpisodeRefused,
    EvaluatedSurface, EventId, EventLog, FailureClass, FailurePredicate, FaultAction, FaultEpisode,
    FaultScope, History, KillLabel, Lane, MAX_OUTSTANDING_REPLAY_EFFECTS, MAX_REPLAY_ATTEMPTS,
    MAX_VALID_TIME_MS, Minimality, Mode, NotEstablishedReason, Oracle, OracleRefused, Payload,
    Query, ReplayEffects, ReplayOutcome, ReplayRefused, ReplayRequest, RepositorySpec, Scenario,
    Sensitivity, ServedClass, SessionSpec, ShrinkRefused, ShrinkReportError, StoreFamily,
    TEST_BINARY_CHILD, Task, TaskRole, Transformation, UnknownReason, Visibility, WitnessClass,
    WorldConfig, classify_replay, parse_shrink_report, reduce, serialize_spec, shrink,
};
use serde_json::{Value, json};
use support::{WORLD_EPOCH_MS, WORLD_SEED, world_config};

const FRESH_SEED: u64 = WORLD_SEED ^ 0xABCD;
const PROFILE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
        oracle: oracle(),
        checkpoint: CUT,
        profile_digest: PROFILE.to_string(),
        witness_class: WitnessClass::Failure {
            task: "early-commit".to_string(),
            class,
        },
    }
}

/// The in-process replay: the planted oracle over the compiled candidate and
/// the aged truth reduced at the first task's cut.
fn evaluate(request: ReplayRequest<'_>) -> ReplayOutcome {
    let truth = reduce(
        &request.set.aged,
        &fixture(),
        &request.set.pairs[0].task.query,
    )
    .unwrap();
    request.oracle.evaluate(
        request.set,
        &request.set.pairs[0].task.id,
        &truth,
        request.checkpoint,
        request.profile_digest,
    )
}

fn commits(log: &EventLog) -> usize {
    log.events
        .iter()
        .filter(|event| matches!(event.payload, Payload::Commit { .. }))
        .count()
}

fn aged_event(id: &str) -> Element {
    Element::Event {
        history: History::Aged,
        id: EventId(id.to_string()),
    }
}

fn has(scenario: &Scenario, element: &Element) -> bool {
    scenario.elements().contains(element)
}

#[test]
fn classify_keeps_unknown_unknown_for_every_reason() {
    let expected = predicate(FailureClass::Interference);
    for reason in UnknownReason::ALL {
        // Every variant is listed; a new one fails to compile here.
        match reason {
            UnknownReason::ReplayBudgetExhausted
            | UnknownReason::EffectUnanswered
            | UnknownReason::ChildExitedBeforeBarrier
            | UnknownReason::ReadBackFailed
            | UnknownReason::Cancelled => {}
        }
        let verdict = classify_replay(&expected, &ReplayOutcome::Unknown { reason });
        assert_eq!(verdict, CandidateVerdict::Unknown { reason });
    }
    assert_eq!(
        classify_replay(&expected, &ReplayOutcome::Passed),
        CandidateVerdict::NotReproduced
    );
    assert_eq!(
        classify_replay(
            &expected,
            &ReplayOutcome::Failed {
                predicate: expected.clone()
            }
        ),
        CandidateVerdict::Reproduced
    );
    let slips: [fn(&mut FailurePredicate); 5] = [
        |p| {
            p.oracle = Oracle::RequiredCommits {
                failing_at: 1,
                slipping_at: 4,
            }
        },
        |p| p.checkpoint = Cut::EndOfRun,
        |p| p.profile_digest = "other-profile".to_string(),
        |p| {
            p.witness_class = WitnessClass::Failure {
                task: "early-commit".to_string(),
                class: FailureClass::DurableState,
            }
        },
        |p| {
            p.witness_class = WitnessClass::Failure {
                task: "last-rename".to_string(),
                class: FailureClass::Interference,
            }
        },
    ];
    for slip in slips {
        let mut observed = expected.clone();
        slip(&mut observed);
        assert_eq!(
            classify_replay(
                &expected,
                &ReplayOutcome::Failed {
                    predicate: observed.clone()
                }
            ),
            CandidateVerdict::Slipped { observed },
            "one differing field slips"
        );
    }
}

#[test]
fn shrink_preserves_the_predicate_and_rejects_slipped_candidates() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();

    assert_eq!(
        commits(&minimized.aged),
        6,
        "one fewer commit slips the class"
    );
    assert!(minimized.episodes.is_empty());
    assert_eq!(
        report.minimality,
        Minimality::OneMinimal {
            transformations: vec![
                Transformation::FaultEpisodeRemoval,
                Transformation::EventDeletion
            ],
        }
    );
    assert_eq!(report.original_digest, original.digest());

    let set = minimized
        .compile(&fixture())
        .expect("the minimized pair is valid");
    let truth = reduce(&set.aged, &fixture(), &set.pairs[0].task.query).unwrap();
    assert_eq!(
        oracle().evaluate(&set, "early-commit", &truth, CUT, PROFILE),
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
    assert!(!slipped.is_empty(), "five commits still fail, differently");
    for record in &slipped {
        let CandidateVerdict::Slipped { observed } = &record.verdict else {
            unreachable!()
        };
        assert_eq!(
            observed.witness_class,
            WitnessClass::Failure {
                task: "early-commit".to_string(),
                class: FailureClass::DurableState
            }
        );
        assert!(
            !report.deleted.is_superset(&record.deleted),
            "a slipped deletion set was not accepted"
        );
    }
    let evidence_deleted = report.candidates.iter().find(|record| {
        record
            .deleted
            .contains(&aged_event("repository:repository-0:0"))
    });
    assert!(
        matches!(
            evidence_deleted.map(|record| &record.verdict),
            Some(CandidateVerdict::InvalidPair { refusal }) if refusal == "EvidenceNotRequired"
        ),
        "deleting the evidence makes the pair invalid: {evidence_deleted:?}"
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
fn fault_episodes_are_tried_before_events() {
    let original = scenario();
    let (_, report) = shrink(
        &original,
        &fixture(),
        &predicate(FailureClass::Interference),
        BUDGET,
        &mut evaluate,
    )
    .unwrap();
    let first_event = report
        .candidates
        .iter()
        .position(|record| {
            record
                .deleted
                .iter()
                .any(|e| matches!(e, Element::Event { .. }))
        })
        .unwrap();
    assert!(
        first_event > 1,
        "the original and at least one episode candidate come first"
    );
    for record in &report.candidates[1..first_event] {
        assert!(
            record
                .deleted
                .iter()
                .all(|e| matches!(e, Element::Episode { .. }))
        );
    }
    for record in &report.candidates[first_event..] {
        assert!(
            record.deleted.contains(&Element::Episode {
                id: "kill-1".to_string()
            }),
            "the episode pass settled before any event was tried"
        );
    }
}

#[test]
fn pair_validity_is_recomputed_and_both_worlds_are_shrunk_together() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (_, report) = shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    let shared = EventId("session:session-0:0".to_string());
    assert!(original.aged.events.iter().any(|e| e.id == shared));
    assert!(original.natural_fresh.events.iter().any(|e| e.id == shared));
    let fresh_deletion = original.without(&BTreeSet::from([Element::Event {
        history: History::NaturalFresh,
        id: shared.clone(),
    }]));
    assert_eq!(
        fresh_deletion.aged, original.aged,
        "a fresh deletion leaves the aged world"
    );
    assert_eq!(
        fresh_deletion.natural_fresh.events.len() + 1,
        original.natural_fresh.events.len()
    );
    let aged_deletion = original.without(&BTreeSet::from([aged_event("session:session-0:0")]));
    assert_eq!(aged_deletion.natural_fresh, original.natural_fresh);
    assert_eq!(
        aged_deletion.aged.events.len() + 1,
        original.aged.events.len()
    );
    let mut fresh_tried = 0;
    for record in &report.candidates {
        let candidate = original.without(&record.deleted);
        let compiled = candidate.compile(&fixture());
        let fresh_deleted: Vec<String> = record
            .deleted
            .iter()
            .filter_map(|element| match element {
                Element::Event {
                    history: History::NaturalFresh,
                    id,
                } => {
                    let (stream, rest) = id.0.split_once(':').unwrap();
                    let (entity, seq) = rest.split_once(':').unwrap();
                    Some(format!("{stream}:{entity}~natural-fresh:{seq}"))
                }
                _ => None,
            })
            .collect();
        fresh_tried += usize::from(!fresh_deleted.is_empty());
        match &record.verdict {
            CandidateVerdict::InvalidPair { .. } => assert!(compiled.is_err()),
            _ => {
                let set = compiled.expect("a replayed candidate compiled");
                for pair in &set.pairs {
                    for event in pair.fresh.events.iter() {
                        assert!(
                            !fresh_deleted.contains(&event.id.0),
                            "a deleted natural-fresh event left the fresh arm"
                        );
                    }
                }
            }
        }
    }
    assert!(fresh_tried > 0, "natural-fresh deletions were tried");
}

#[test]
fn a_deletion_set_removes_exactly_what_one_deletion_at_a_time_removes() {
    let original = scenario();
    let deleted: BTreeSet<Element> = original.elements().into_iter().step_by(3).collect();
    let mut expected = original.clone();
    for element in &deleted {
        match element {
            Element::Episode { id } => expected.episodes.retain(|episode| episode.id != *id),
            Element::Event {
                history: History::Aged,
                id,
            } => expected.aged = expected.aged.without(id),
            Element::Event {
                history: History::NaturalFresh,
                id,
            } => expected.natural_fresh = expected.natural_fresh.without(id),
        }
    }
    assert!(expected.episodes.len() < original.episodes.len());
    assert!(expected.aged.causal_edges.len() < original.aged.causal_edges.len());
    assert!(expected.natural_fresh.events.len() < original.natural_fresh.events.len());
    let batched = original.without(&deleted);
    assert_eq!(batched, expected);
    assert_eq!(batched.digest(), expected.digest());
}

#[test]
fn an_unknown_replay_is_kept_and_never_becomes_not_reproduced() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let stubborn = aged_event("repository:repository-0:2");
    let mut replay = |request: ReplayRequest<'_>| {
        if !has(request.scenario, &stubborn) {
            return ReplayOutcome::Unknown {
                reason: UnknownReason::ChildExitedBeforeBarrier,
            };
        }
        evaluate(request)
    };
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut replay).unwrap();
    assert!(
        has(&minimized, &stubborn),
        "an element whose deletion is unknown stays"
    );
    assert!(report.unknown_candidates > 0);
    for record in &report.candidates {
        if record.deleted.contains(&stubborn) {
            assert!(matches!(
                record.verdict,
                CandidateVerdict::Unknown { .. } | CandidateVerdict::InvalidPair { .. }
            ));
        }
    }
    assert!(matches!(
        report.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { count: 1 }
        }
    ));
}

#[test]
fn the_final_pass_deletes_an_episode_that_events_made_deletable() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let kill_1 = Element::Episode {
        id: "kill-1".to_string(),
    };
    // Reproduces with kill-1 and at least six commits, or with at most four
    // commits regardless of episodes.
    let reproduces = |scenario: &Scenario| {
        let commits = commits(&scenario.aged);
        (has(scenario, &kill_1) && commits >= 6) || commits <= 4
    };
    let outcome = |scenario: &Scenario| {
        if reproduces(scenario) {
            ReplayOutcome::Failed {
                predicate: expected.clone(),
            }
        } else {
            ReplayOutcome::Passed
        }
    };
    let mut replay = |request: ReplayRequest<'_>| outcome(request.scenario);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut replay).unwrap();
    assert!(
        !has(&minimized, &kill_1),
        "kill-1 became deletable once the commits shrank"
    );
    assert!(commits(&minimized.aged) <= 4);
    assert!(matches!(report.minimality, Minimality::OneMinimal { .. }));
    for element in minimized.elements() {
        let mut drop = report.deleted.clone();
        drop.insert(element);
        let single = original.without(&drop);
        let still = single.compile(&fixture()).is_ok() && reproduces(&single);
        assert!(!still, "a single deletion still reproduces: {drop:?}");
    }
}

#[test]
fn an_exhausted_replay_budget_stops_the_pass_and_keeps_the_last_reproduced_scenario() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) = shrink(&original, &fixture(), &expected, 5, &mut evaluate).unwrap();
    assert_eq!(report.replays, 5);
    assert!(
        report
            .candidates
            .iter()
            .all(|r| !matches!(r.verdict, CandidateVerdict::Unknown { .. })),
        "the pass stops at the budget instead of labelling candidates"
    );
    assert_eq!(
        report.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::ReplayBudgetExhausted
        }
    );
    let last_reproduced = report
        .candidates
        .iter()
        .rev()
        .find(|r| r.verdict == CandidateVerdict::Reproduced)
        .unwrap();
    assert_eq!(report.deleted, last_reproduced.deleted);
    let set = minimized.compile(&fixture()).unwrap();
    let truth = reduce(&set.aged, &fixture(), &set.pairs[0].task.query).unwrap();
    assert_eq!(
        oracle().evaluate(&set, "early-commit", &truth, CUT, PROFILE),
        ReplayOutcome::Failed {
            predicate: expected
        }
    );
}

#[test]
fn a_zero_replay_budget_issues_no_replay_and_refuses_the_original_scenario() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let mut issued = 0u32;
    let mut replay = |request: ReplayRequest<'_>| {
        issued += 1;
        evaluate(request)
    };
    let refused = shrink(&original, &fixture(), &expected, 0, &mut replay);
    assert_eq!(issued, 0, "no replay is issued past the budget");
    assert_eq!(
        refused.err(),
        Some(ShrinkRefused::OriginalNotReproduced {
            verdict: CandidateVerdict::Unknown {
                reason: UnknownReason::ReplayBudgetExhausted
            }
        })
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
    let expected = predicate(FailureClass::Interference);
    let mut passing = |_: ReplayRequest<'_>| ReplayOutcome::Passed;
    assert_eq!(
        shrink(&original, &fixture(), &expected, BUDGET, &mut passing).err(),
        Some(ShrinkRefused::OriginalNotReproduced {
            verdict: CandidateVerdict::NotReproduced
        })
    );
    let mut unknown = |_: ReplayRequest<'_>| ReplayOutcome::Unknown {
        reason: UnknownReason::ReadBackFailed,
    };
    assert_eq!(
        shrink(&original, &fixture(), &expected, BUDGET, &mut unknown).err(),
        Some(ShrinkRefused::OriginalNotReproduced {
            verdict: CandidateVerdict::Unknown {
                reason: UnknownReason::ReadBackFailed
            }
        })
    );
}

/// Replay closures drawing per-candidate outcomes from the whole vocabulary,
/// under several seeds: the shrinker's invariants hold whatever it is told.
#[test]
fn the_shrinker_invariants_hold_under_arbitrary_replay_answers() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let mut other = expected.clone();
    other.checkpoint = Cut::EndOfRun;
    let vocabulary = [
        ReplayOutcome::Failed {
            predicate: expected.clone(),
        },
        ReplayOutcome::Failed {
            predicate: expected.clone(),
        },
        ReplayOutcome::Failed { predicate: other },
        ReplayOutcome::Passed,
        ReplayOutcome::Unknown {
            reason: UnknownReason::EffectUnanswered,
        },
        ReplayOutcome::Unknown {
            reason: UnknownReason::Cancelled,
        },
    ];
    for seed in 1u64..=6 {
        let mut replay = |request: ReplayRequest<'_>| {
            if request.scenario.digest() == original.digest() {
                return vocabulary[0].clone();
            }
            let byte = u64::from(u8::from_str_radix(&request.key[..2], 16).unwrap());
            vocabulary[((byte ^ seed) % vocabulary.len() as u64) as usize].clone()
        };
        let (minimized, report) =
            shrink(&original, &fixture(), &expected, BUDGET, &mut replay).unwrap();
        report
            .verify(&original)
            .unwrap_or_else(|e| panic!("seed {seed}: the report accounts for itself: {e}"));
        let recorded = |digest: &str| {
            report
                .candidates
                .iter()
                .find(|r| r.scenario_digest == digest)
                .map(|r| r.verdict.clone())
        };
        assert_eq!(
            recorded(&minimized.digest()),
            Some(CandidateVerdict::Reproduced),
            "seed {seed}: the returned scenario reproduced"
        );
        for record in &report.candidates {
            let candidate = original.without(&record.deleted);
            let invalid = candidate.compile(&fixture()).is_err();
            assert_eq!(
                matches!(record.verdict, CandidateVerdict::InvalidPair { .. }),
                invalid,
                "seed {seed}: InvalidPair exactly when the compiler refuses"
            );
        }
        if matches!(report.minimality, Minimality::OneMinimal { .. }) {
            for element in minimized.elements() {
                let mut drop = report.deleted.clone();
                drop.insert(element);
                let verdict = recorded(&original.without(&drop).digest());
                assert!(
                    matches!(
                        verdict,
                        Some(
                            CandidateVerdict::NotReproduced
                                | CandidateVerdict::Slipped { .. }
                                | CandidateVerdict::InvalidPair { .. }
                        )
                    ),
                    "seed {seed}: 1-minimality rests on a rejection, found {verdict:?}"
                );
            }
        }
    }
}

#[test]
fn replay_effects_are_bounded_and_a_premature_verdict_is_refused() {
    let key = |index: usize| format!("candidate-{index}");
    let mut effects = ReplayEffects::default();
    for index in 0..MAX_OUTSTANDING_REPLAY_EFFECTS {
        effects.issue(&key(index)).unwrap();
    }
    assert_eq!(
        effects.issue("one-too-many"),
        Err(ReplayRefused::OutstandingBound {
            bound: MAX_OUTSTANDING_REPLAY_EFFECTS
        })
    );
    assert_eq!(
        effects.issue(&key(0)),
        Err(ReplayRefused::Outstanding { key: key(0) }),
        "an outstanding key is not reissued"
    );
    assert_eq!(
        effects.outcome(&key(0)),
        Err(ReplayRefused::Outstanding { key: key(0) }),
        "classifying before the replay answered is premature"
    );
    assert_eq!(
        effects.retry(&key(0)),
        Ok(2),
        "a retry keeps its receipt key"
    );
    effects
        .resolve(
            &key(0),
            2,
            ReplayOutcome::Failed {
                predicate: predicate(FailureClass::Interference),
            },
        )
        .unwrap();
    assert_eq!(
        classify_replay(
            &predicate(FailureClass::Interference),
            effects.outcome(&key(0)).unwrap()
        ),
        CandidateVerdict::Reproduced,
        "the retried attempt answered under the original key"
    );
    effects.cancel(&key(1), 1).unwrap();
    assert_eq!(
        classify_replay(
            &predicate(FailureClass::Interference),
            effects.outcome(&key(1)).unwrap()
        ),
        CandidateVerdict::Unknown {
            reason: UnknownReason::Cancelled
        }
    );
    for resolved in [key(0), key(1)] {
        assert_eq!(
            effects.issue(&resolved),
            Err(ReplayRefused::AlreadyResolved {
                key: resolved.clone()
            })
        );
        assert_eq!(
            effects.retry(&resolved),
            Err(ReplayRefused::AlreadyResolved {
                key: resolved.clone()
            })
        );
        assert_eq!(
            effects.cancel(&resolved, 1),
            Err(ReplayRefused::AlreadyResolved { key: resolved })
        );
    }
    assert_eq!(
        effects.resolve("never-issued", 1, ReplayOutcome::Passed),
        Err(ReplayRefused::UnknownKey {
            key: "never-issued".to_string()
        })
    );
    effects.issue("one-too-many").unwrap();
    effects.issue("and-another").unwrap();
    assert_eq!(
        effects.issue("past-the-bound"),
        Err(ReplayRefused::OutstandingBound {
            bound: MAX_OUTSTANDING_REPLAY_EFFECTS
        })
    );
}

#[test]
fn wire_names_are_pinned() {
    let predicate = predicate(FailureClass::Interference);
    assert_eq!(
        serde_json::to_value(&predicate).unwrap(),
        json!({
            "oracle": {"kind": "required_commits", "failing_at": 3, "slipping_at": 6},
            "checkpoint": "AtQuiescence",
            "profile_digest": PROFILE,
            "witness_class": {"kind": "failure", "task": "early-commit", "class": "interference"},
        })
    );
    assert_eq!(
        serde_json::to_value(aged_event("repository:repository-0:0")).unwrap(),
        json!({"kind": "event", "history": "aged", "id": "repository:repository-0:0"})
    );
    assert_eq!(
        serde_json::to_value(CandidateVerdict::InvalidPair {
            refusal: "NoTasks".to_string()
        })
        .unwrap(),
        json!({"kind": "invalid_pair", "refusal": "NoTasks"})
    );
    assert_eq!(
        serde_json::to_value(Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { count: 2 }
        })
        .unwrap(),
        json!({"kind": "not_established", "reason": {"reason": "unknown_candidates", "count": 2}})
    );
    assert_eq!(
        serde_json::to_value(WitnessClass::Recovery {
            effect: "effect-1".to_string()
        })
        .unwrap(),
        json!({"kind": "recovery", "effect": "effect-1"})
    );
    assert_eq!(
        serde_json::to_value(WitnessClass::Liveness {
            lane: Lane::CatchUpEpisodes
        })
        .unwrap(),
        json!({"kind": "liveness", "lane": "catch_up_episodes"})
    );
    let (_, report) = shrink(&scenario(), &fixture(), &predicate, 3, &mut evaluate).unwrap();
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["schema"], "eval-shrink/v1");
    let again: eval_core::ShrinkReport = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(again, report);
    let mut extra = value;
    extra["extra"] = Value::Bool(true);
    assert!(serde_json::from_value::<eval_core::ShrinkReport>(extra).is_err());
}

#[test]
fn a_replay_under_other_thresholds_slips_even_when_it_fails_the_same_class() {
    // The original has enough commits to fail as `interference` under both
    // `(3, 6)` and `(1, 4)`; only the pinned thresholds tell them apart.
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let other = Oracle::RequiredCommits {
        failing_at: 1,
        slipping_at: 4,
    };
    let mut replay = |request: ReplayRequest<'_>| {
        let truth = reduce(
            &request.set.aged,
            &fixture(),
            &request.set.pairs[0].task.query,
        )
        .unwrap();
        other.evaluate(
            request.set,
            &request.set.pairs[0].task.id,
            &truth,
            request.checkpoint,
            request.profile_digest,
        )
    };
    let refused = shrink(&original, &fixture(), &expected, BUDGET, &mut replay);
    assert!(
        matches!(
            refused,
            Err(ShrinkRefused::OriginalNotReproduced {
                verdict: CandidateVerdict::Slipped { .. }
            })
        ),
        "a differently parameterised oracle is a different predicate: {refused:?}"
    );
}

#[test]
fn inverted_thresholds_are_refused_before_any_replay() {
    let inverted = Oracle::RequiredCommits {
        failing_at: 6,
        slipping_at: 3,
    };
    let expected = FailurePredicate {
        oracle: inverted,
        ..predicate(FailureClass::Interference)
    };
    let mut issued = 0u32;
    let mut replay = |request: ReplayRequest<'_>| {
        issued += 1;
        evaluate(request)
    };
    let refused = shrink(&scenario(), &fixture(), &expected, BUDGET, &mut replay);
    assert_eq!(
        refused.err(),
        Some(ShrinkRefused::InvalidOracle(
            OracleRefused::InvertedThresholds {
                failing_at: 6,
                slipping_at: 3,
            }
        ))
    );
    assert_eq!(issued, 0, "nothing is replayed under an invalid oracle");
}

#[test]
fn a_report_is_read_back_only_under_its_schema_and_a_valid_oracle() {
    let (_, report) = shrink(
        &scenario(),
        &fixture(),
        &predicate(FailureClass::Interference),
        3,
        &mut evaluate,
    )
    .unwrap();
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(parse_shrink_report(&value).unwrap(), report);

    let mut other_schema = value.clone();
    other_schema["schema"] = Value::String("eval-shrink/v0".to_string());
    assert_eq!(
        parse_shrink_report(&other_schema),
        Err(ShrinkReportError::SchemaMismatch {
            found: "eval-shrink/v0".to_string()
        })
    );

    let mut inverted = value.clone();
    inverted["predicate"]["oracle"]["slipping_at"] = json!(1);
    assert_eq!(
        parse_shrink_report(&inverted),
        Err(ShrinkReportError::Oracle(
            OracleRefused::InvertedThresholds {
                failing_at: 3,
                slipping_at: 1,
            }
        ))
    );

    let mut extra = value;
    extra["extra"] = Value::Bool(true);
    assert!(matches!(
        parse_shrink_report(&extra),
        Err(ShrinkReportError::Shape(_))
    ));
}

#[test]
fn a_report_whose_accounting_disagrees_with_its_ledger_is_refused() {
    let (_, report) = shrink(
        &scenario(),
        &fixture(),
        &predicate(FailureClass::Interference),
        BUDGET,
        &mut evaluate,
    )
    .unwrap();
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(parse_shrink_report(&value).unwrap(), report);
    let tampered = |edit: fn(&mut Value)| {
        let mut copy = value.clone();
        edit(&mut copy);
        parse_shrink_report(&copy)
    };
    type Edit = fn(&mut Value);
    let cases: [(&str, Edit); 4] = [
        ("replays", |v| v["replays"] = json!(0)),
        ("unknown_candidates", |v| v["unknown_candidates"] = json!(7)),
        ("minimized_digest", |v| {
            v["minimized_digest"] = v["original_digest"].clone()
        }),
        ("deleted", |v| v["deleted"] = json!([])),
    ];
    for (field, edit) in cases {
        assert_eq!(
            tampered(edit),
            Err(ShrinkReportError::Inconsistent { field }),
            "{field} disagrees with the candidate ledger"
        );
    }
}

#[test]
fn invalid_episodes_are_refused_before_any_replay() {
    let mut original = scenario();
    original.episodes.push(episode("kill-1"));
    let mut issued = 0u32;
    let mut replay = |request: ReplayRequest<'_>| {
        issued += 1;
        evaluate(request)
    };
    let refused = shrink(
        &original,
        &fixture(),
        &predicate(FailureClass::Interference),
        BUDGET,
        &mut replay,
    );
    assert_eq!(
        refused.err(),
        Some(ShrinkRefused::InvalidEpisodes(
            EpisodeRefused::DuplicateEpisode {
                id: "kill-1".to_string()
            }
        ))
    );
    assert_eq!(issued, 0, "nothing is replayed under invalid episodes");
}

#[test]
fn a_report_that_understates_its_replays_or_overstates_its_minimality_is_refused() {
    let expected = predicate(FailureClass::Interference);
    let stubborn = aged_event("repository:repository-0:2");
    let mut unknown_replay = |request: ReplayRequest<'_>| {
        if !has(request.scenario, &stubborn) {
            return ReplayOutcome::Unknown {
                reason: UnknownReason::ChildExitedBeforeBarrier,
            };
        }
        evaluate(request)
    };
    let (_, unknown) = shrink(
        &scenario(),
        &fixture(),
        &expected,
        BUDGET,
        &mut unknown_replay,
    )
    .unwrap();
    let (_, exhausted) = shrink(&scenario(), &fixture(), &expected, 5, &mut evaluate).unwrap();
    let (_, minimal) = shrink(&scenario(), &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    assert!(matches!(
        unknown.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::UnknownCandidates { .. }
        }
    ));
    assert!(matches!(
        exhausted.minimality,
        Minimality::NotEstablished {
            reason: NotEstablishedReason::ReplayBudgetExhausted
        }
    ));
    assert!(matches!(minimal.minimality, Minimality::OneMinimal { .. }));
    for report in [&unknown, &exhausted, &minimal] {
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(parse_shrink_report(&value).unwrap(), *report);
    }

    let completed = unknown
        .candidates
        .iter()
        .filter(|record| {
            matches!(
                record.verdict,
                CandidateVerdict::Reproduced
                    | CandidateVerdict::NotReproduced
                    | CandidateVerdict::Slipped { .. }
            )
        })
        .map(|record| record.scenario_digest.as_str())
        .collect::<BTreeSet<_>>()
        .len() as u64;
    assert!(
        completed < unknown.replays,
        "the unknown replays were issued too"
    );
    let mut understated = serde_json::to_value(&unknown).unwrap();
    understated["replays"] = json!(completed);
    assert_eq!(
        parse_shrink_report(&understated),
        Err(ShrinkReportError::Inconsistent { field: "replays" }),
        "every unknown answer in a returned report took a replay"
    );

    let one_minimal = json!({"kind": "one_minimal", "transformations": []});
    let mut claimed = serde_json::to_value(&exhausted).unwrap();
    claimed["minimality"] = one_minimal.clone();
    assert_eq!(
        parse_shrink_report(&claimed),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        }),
        "a budget-exhausted run cannot claim 1-minimality"
    );
    let mut claimed = serde_json::to_value(&unknown).unwrap();
    claimed["minimality"] = one_minimal;
    assert_eq!(
        parse_shrink_report(&claimed),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        }),
        "an unknown single deletion cannot claim 1-minimality"
    );
    let mut miscounted = serde_json::to_value(&unknown).unwrap();
    miscounted["minimality"]["reason"]["count"] = json!(9);
    assert_eq!(
        parse_shrink_report(&miscounted),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        })
    );
    let mut disproved = serde_json::to_value(&minimal).unwrap();
    disproved["minimality"] =
        json!({"kind": "not_established", "reason": {"reason": "replay_budget_exhausted"}});
    assert_eq!(
        parse_shrink_report(&disproved),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        }),
        "a run that stopped short of its budget did not exhaust it"
    );
    let mut reordered = serde_json::to_value(&minimal).unwrap();
    reordered["minimality"]["transformations"] = json!(["event_deletion", "fault_episode_removal"]);
    assert_eq!(
        parse_shrink_report(&reordered),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        })
    );
}

#[test]
fn a_candidate_that_moves_the_failure_to_another_task_slips() {
    // The replay reports the pinned class, but for the positive control
    // rather than the falsification task the original failed on.
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let mut replay = |request: ReplayRequest<'_>| {
        if request.scenario.digest() == original.digest() {
            return evaluate(request);
        }
        ReplayOutcome::Failed {
            predicate: FailurePredicate {
                witness_class: WitnessClass::Failure {
                    task: "last-rename".to_string(),
                    class: FailureClass::Interference,
                },
                ..expected.clone()
            },
        }
    };
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut replay).unwrap();
    assert_eq!(minimized, original, "no candidate was accepted");
    assert!(report.candidates.len() > 1);
    for record in &report.candidates[1..] {
        assert!(
            matches!(
                record.verdict,
                CandidateVerdict::Slipped { .. } | CandidateVerdict::InvalidPair { .. }
            ),
            "another task's failure is a different predicate: {:?}",
            record.verdict
        );
    }
}

#[test]
fn a_budget_outside_the_canonical_range_is_refused_and_so_is_such_a_report() {
    let expected = predicate(FailureClass::Interference);
    let mut issued = 0u32;
    let mut replay = |request: ReplayRequest<'_>| {
        issued += 1;
        evaluate(request)
    };
    let refused = shrink(&scenario(), &fixture(), &expected, u64::MAX, &mut replay);
    assert_eq!(
        refused.err(),
        Some(ShrinkRefused::BudgetNotCanonical {
            max_replays: u64::MAX
        })
    );
    assert_eq!(issued, 0);

    let (_, mut report) = shrink(&scenario(), &fixture(), &expected, 3, &mut evaluate).unwrap();
    let value = report.serialize().unwrap();
    assert_eq!(parse_shrink_report(&value).unwrap(), report);
    let mut huge = value;
    huge["max_replays"] = json!(9_007_199_254_740_992u64);
    assert!(matches!(
        parse_shrink_report(&huge),
        Err(ShrinkReportError::NotCanonical(_))
    ));
    report.max_replays = 9_007_199_254_740_992;
    assert!(matches!(
        report.serialize(),
        Err(ShrinkReportError::NotCanonical(_))
    ));
}

#[test]
fn a_ledger_that_contradicts_itself_about_one_candidate_is_refused() {
    let (_, report) = shrink(
        &scenario(),
        &fixture(),
        &predicate(FailureClass::Interference),
        BUDGET,
        &mut evaluate,
    )
    .unwrap();
    let mut value = serde_json::to_value(&report).unwrap();
    let records = value["candidates"].as_array().unwrap();
    let repeated = records
        .iter()
        .enumerate()
        .find(|(index, record)| {
            records[index + 1..]
                .iter()
                .any(|later| later["scenario_digest"] == record["scenario_digest"])
                && record["verdict"]["kind"] == "slipped"
        })
        .map(|(index, _)| index)
        .expect("the final pass re-records a candidate ddmin already answered");
    value["candidates"][repeated]["verdict"] =
        json!({"kind": "unknown", "reason": "effect_unanswered"});
    assert_eq!(
        parse_shrink_report(&value),
        Err(ShrinkReportError::Inconsistent {
            field: "candidates"
        }),
        "one digest was answered once and has one verdict"
    );
}

#[test]
fn a_report_verifies_against_the_scenario_it_shrank() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (minimized, report) =
        shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    report.verify(&original).unwrap();
    assert_eq!(original.without(&report.deleted), minimized);

    let mut truncated = report.clone();
    let last = truncated
        .candidates
        .iter()
        .rposition(|record| record.verdict == CandidateVerdict::Reproduced)
        .unwrap();
    truncated.candidates.truncate(last + 1);
    truncated.remaining = 0;
    let distinct: BTreeSet<&str> = truncated
        .candidates
        .iter()
        .filter(|record| !matches!(record.verdict, CandidateVerdict::InvalidPair { .. }))
        .map(|record| record.scenario_digest.as_str())
        .collect();
    truncated.replays = distinct.len() as u64;
    truncated.unknown_candidates = 0;
    truncated.validate().unwrap();
    assert_eq!(
        truncated.verify(&original),
        Err(ShrinkReportError::Inconsistent { field: "remaining" }),
        "the scenario, not the report, says how many elements remain"
    );

    let mut other = original.clone();
    other.episodes.pop();
    assert_eq!(
        report.verify(&other),
        Err(ShrinkReportError::Inconsistent {
            field: "original_digest"
        }),
        "a report does not verify against a scenario it did not shrink"
    );
}

#[test]
fn replay_attempts_are_bounded_and_a_stale_attempt_cannot_resolve() {
    let mut effects = ReplayEffects::default();
    assert_eq!(effects.issue("candidate"), Ok(1));
    for attempt in 2..=MAX_REPLAY_ATTEMPTS {
        assert_eq!(effects.retry("candidate"), Ok(attempt));
    }
    assert_eq!(
        effects.retry("candidate"),
        Err(ReplayRefused::AttemptsExhausted {
            key: "candidate".to_string(),
            attempts: MAX_REPLAY_ATTEMPTS,
        }),
        "a retry past the bound is refused, not launched"
    );
    assert_eq!(
        effects.resolve("candidate", 1, ReplayOutcome::Passed),
        Err(ReplayRefused::StaleAttempt {
            key: "candidate".to_string(),
            attempt: 1,
            current: MAX_REPLAY_ATTEMPTS,
        }),
        "a superseded attempt's answer is fenced"
    );
    assert_eq!(
        effects.outcome("candidate"),
        Err(ReplayRefused::Outstanding {
            key: "candidate".to_string()
        })
    );
    effects
        .resolve("candidate", MAX_REPLAY_ATTEMPTS, ReplayOutcome::Passed)
        .unwrap();
    assert_eq!(effects.outcome("candidate"), Ok(&ReplayOutcome::Passed));
}

#[test]
fn impossible_ledger_shapes_are_refused_on_read_and_ghosts_on_verify() {
    let original = scenario();
    let expected = predicate(FailureClass::Interference);
    let (_, report) = shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    let value = serde_json::to_value(&report).unwrap();

    // A slipped verdict whose observed predicate equals the pinned one is
    // what `classify_replay` calls `Reproduced`; it cannot appear.
    let mut echoed = value.clone();
    let records = echoed["candidates"].as_array_mut().unwrap();
    let slipped: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r["verdict"]["kind"] == "slipped")
        .map(|(i, _)| i)
        .collect();
    let digest = records[slipped[0]]["scenario_digest"].clone();
    for record in records
        .iter_mut()
        .filter(|r| r["scenario_digest"] == digest)
    {
        record["verdict"]["observed"] = serde_json::to_value(&expected).unwrap();
    }
    assert_eq!(
        parse_shrink_report(&echoed),
        Err(ShrinkReportError::Inconsistent {
            field: "candidates"
        }),
        "a slip that observed the pinned predicate is impossible"
    );

    // Digests are what `Scenario::digest` produces: 64 lowercase hex chars.
    let mut renamed = value.clone();
    renamed["original_digest"] = json!("the-original");
    renamed["candidates"][0]["scenario_digest"] = json!("the-original");
    assert_eq!(
        parse_shrink_report(&renamed),
        Err(ShrinkReportError::Inconsistent {
            field: "original_digest"
        })
    );
    let mut renamed = value.clone();
    renamed["candidates"][1]["scenario_digest"] = json!("CAFE");
    assert_eq!(
        parse_shrink_report(&renamed),
        Err(ShrinkReportError::Inconsistent {
            field: "candidates"
        })
    );

    // A deletion the original never held is a ghost, whatever the digests say.
    let ghost = aged_event("repository:repository-9:99");
    assert!(!has(&original, &ghost));
    let mut haunted = report.clone();
    haunted.deleted.insert(ghost.clone());
    let last = haunted
        .candidates
        .iter()
        .rposition(|record| record.verdict == CandidateVerdict::Reproduced)
        .unwrap();
    for record in &mut haunted.candidates[last..] {
        record.deleted.insert(ghost.clone());
    }
    assert_eq!(
        original.without(&haunted.deleted).digest(),
        haunted.minimized_digest,
        "the ghost changes nothing the digests can see"
    );
    haunted.validate().unwrap();
    assert_eq!(
        haunted.verify(&original),
        Err(ShrinkReportError::Inconsistent { field: "deleted" })
    );

    // The transformations tried are exactly those the original had elements
    // for; a scenario with no episodes never tried episode removal.
    let mut eventless = original.clone();
    eventless.episodes.clear();
    let (_, report) = shrink(&eventless, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();
    assert_eq!(
        report.minimality,
        Minimality::OneMinimal {
            transformations: vec![Transformation::EventDeletion]
        }
    );
    report.verify(&eventless).unwrap();
    let mut padded = report;
    padded.minimality = Minimality::OneMinimal {
        transformations: Transformation::ORDER.to_vec(),
    };
    padded.validate().unwrap();
    assert_eq!(
        padded.verify(&eventless),
        Err(ShrinkReportError::Inconsistent {
            field: "minimality"
        })
    );
}

#[test]
fn a_malformed_profile_digest_is_refused_and_every_candidate_digest_is_recomputed() {
    let original = scenario();
    let mut expected = predicate(FailureClass::Interference);
    let (_, report) = shrink(&original, &fixture(), &expected, BUDGET, &mut evaluate).unwrap();

    let mut renamed = report.clone();
    let victim = renamed
        .candidates
        .iter()
        .position(|record| matches!(record.verdict, CandidateVerdict::Slipped { .. }))
        .unwrap();
    let digest = renamed.candidates[victim].scenario_digest.clone();
    let other = format!(
        "{}{}",
        "0".repeat(63),
        if digest.ends_with('0') { "1" } else { "0" }
    );
    for record in renamed
        .candidates
        .iter_mut()
        .filter(|record| record.scenario_digest == digest)
    {
        record.scenario_digest = other.clone();
    }
    renamed.validate().unwrap();
    assert_eq!(
        renamed.verify(&original),
        Err(ShrinkReportError::Inconsistent {
            field: "candidates"
        }),
        "a candidate's digest is the digest of the scenario its deletions leave"
    );

    expected.profile_digest = "profile-digest".to_string();
    let mut issued = 0u32;
    let mut replay = |request: ReplayRequest<'_>| {
        issued += 1;
        evaluate(request)
    };
    assert_eq!(
        shrink(&original, &fixture(), &expected, BUDGET, &mut replay).err(),
        Some(ShrinkRefused::InvalidPredicate {
            field: "profile_digest"
        })
    );
    assert_eq!(issued, 0);
    let mut value = serde_json::to_value(&report).unwrap();
    value["predicate"]["profile_digest"] = json!("profile-digest");
    assert_eq!(
        parse_shrink_report(&value),
        Err(ShrinkReportError::Inconsistent {
            field: "profile_digest"
        })
    );

    let mut effects = ReplayEffects::default();
    assert_eq!(effects.issue("candidate"), Ok(1));
    assert_eq!(effects.retry("candidate"), Ok(2));
    assert_eq!(
        effects.cancel("candidate", 1),
        Err(ReplayRefused::StaleAttempt {
            key: "candidate".to_string(),
            attempt: 1,
            current: 2,
        }),
        "a superseded attempt's cancellation is fenced like its answer"
    );
    effects.cancel("candidate", 2).unwrap();
}
