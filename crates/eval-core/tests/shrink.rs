//! The shrinker: ddmin over paired worlds under a pinned failure predicate
//! and the replay-effect ledger.

mod support;

use std::collections::BTreeSet;

use eval_core::{
    CandidateVerdict, Cut, Element, EventId, EventLog, FailureClass, FailurePredicate, History,
    MAX_OUTSTANDING_REPLAY_EFFECTS, Minimality, NotEstablishedReason, Payload, ReplayEffects,
    ReplayOutcome, ReplayRefused, ReplayRequest, Scenario, ShrinkRefused, Transformation,
    UnknownReason, WitnessClass, classify_replay, reduce, shrink,
};
use serde_json::{Value, json};
use support::shrink::{BUDGET, CUT, PROFILE, evaluate, fixture, oracle, predicate, scenario};

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
    let slips: [fn(&mut FailurePredicate); 4] = [
        |p| p.oracle = "planted:other".to_string(),
        |p| p.checkpoint = Cut::EndOfRun,
        |p| p.profile_digest = "other-profile".to_string(),
        |p| {
            p.witness_class = WitnessClass::Failure {
                class: FailureClass::DurableState,
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
        oracle().evaluate(&set, &truth, CUT, PROFILE),
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
        oracle().evaluate(&set, &truth, CUT, PROFILE),
        ReplayOutcome::Failed {
            predicate: expected
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
    let mut effects = ReplayEffects::new(MAX_OUTSTANDING_REPLAY_EFFECTS);
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
    effects.cancel(&key(1)).unwrap();
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
            effects.cancel(&resolved),
            Err(ReplayRefused::AlreadyResolved { key: resolved })
        );
    }
    assert_eq!(
        effects.resolve("never-issued", ReplayOutcome::Passed),
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
            "oracle": "planted:required-commits",
            "checkpoint": "AtQuiescence",
            "profile_digest": PROFILE,
            "witness_class": {"kind": "failure", "class": "interference"},
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
        serde_json::to_value(WitnessClass::Recovery).unwrap(),
        json!({"kind": "recovery"})
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
