//! Residual judging: blinding, both presentation orders, the permutation
//! check, the human sample floor, identity-bound comparison, the judge-type
//! firewall around the gates, and the held-out live slice.

use std::collections::BTreeMap;

use eval_core::{
    AnalysisFamily, ArmRates, ArmResult, BlindingRefused, CalibrationRefused, CalibrationSet,
    CensorReason, ClusterKey, FrozenFamily, HUMAN_SAMPLE_MIN_PAIRS, JUDGE_SCHEMA, JudgeCall,
    JudgeIdentity, JudgeRefused, JudgedPair, LIVE_REPLAYABLE, LiveSettings, LiveSettingsRefused,
    LiveSliceRefused, LiveTask, Order, PairJudgment, PairOutcome, PassKBounds, PermutationCheck,
    PermutationRefused, Preference, ProviderProfile, RESIDUAL_REPORT_SCHEMA, Ratio, RawVerdict,
    ResidualRefused, ResidualReport, Rubric, SamplingPlan, analyze, blind, judge_pairs, live_slice,
};

fn provider(model: &str) -> ProviderProfile {
    ProviderProfile {
        provider: "anthropic".to_string(),
        model: model.to_string(),
        tokenizer_profile: "tp-1".to_string(),
    }
}

fn judge() -> JudgeIdentity {
    JudgeIdentity {
        provider: provider("judge-1"),
        prompt_digest: "aa".repeat(32),
        rubric_digest: Rubric {
            schema: JUDGE_SCHEMA.to_string(),
            criteria: vec!["correctness".to_string(), "uses the history".to_string()],
        }
        .digest()
        .unwrap(),
    }
}

fn pairs() -> Vec<JudgedPair> {
    (0..3)
        .map(|i| JudgedPair {
            id: format!("pair-{i}"),
            a: format!("the aged answer {i} cites the rename"),
            b: format!("the fresh answer {i}"),
        })
        .collect()
}

fn call(pair: &str, order: Order, verdict: RawVerdict) -> JudgeCall {
    JudgeCall {
        pair: pair.to_string(),
        judge: judge(),
        order,
        verdict,
    }
}

fn calibration() -> CalibrationSet {
    CalibrationSet {
        schema: JUDGE_SCHEMA.to_string(),
        judge: judge(),
        human_labels: BTreeMap::from([("anchor-1".to_string(), Preference::A)]),
    }
}

fn report(
    judge: JudgeIdentity,
    live: ProviderProfile,
    calibration: &CalibrationSet,
) -> ResidualReport {
    ResidualReport {
        schema: RESIDUAL_REPORT_SCHEMA.to_string(),
        judge,
        calibration_digest: calibration.digest(),
        live_provider: live,
        sampling: SamplingPlan {
            pairs: 40,
            human_sample: 20,
        },
        permutation: PermutationCheck {
            trials: 40,
            correct: 21,
        },
        judgments: (0..40)
            .map(|i| PairJudgment {
                pair: format!("pair-{i}"),
                preference: Preference::Tie,
                length_a: 1,
                length_b: 1,
            })
            .collect(),
    }
}

fn settings(k: u32) -> LiveSettings {
    LiveSettings {
        providers: vec![provider("live-1"), provider("live-2")],
        k,
        calibration: Some(calibration()),
        sampling: Some(SamplingPlan {
            pairs: 40,
            human_sample: 20,
        }),
    }
}

#[test]
fn blinding_refuses_a_canary_or_an_arm_name_and_shows_both_orders() {
    let pair = &pairs()[0];
    let ab = blind(pair, Order::AThenB, &[]).unwrap();
    let ba = blind(pair, Order::BThenA, &[]).unwrap();
    assert_eq!(
        (ab.first.as_str(), ab.second.as_str()),
        (pair.a.as_str(), pair.b.as_str())
    );
    assert_eq!(
        (ba.first.as_str(), ba.second.as_str()),
        (pair.b.as_str(), pair.a.as_str())
    );
    assert_eq!(
        blind(
            pair,
            Order::AThenB,
            &["CANARY-1".to_string(), "rename".to_string()]
        ),
        Err(BlindingRefused::CanaryInPrompt {
            pair: "pair-0".to_string(),
            canary: "rename".to_string()
        })
    );
    let mut named = pair.clone();
    named.b = "as the Fresh Arm I answer".to_string();
    assert_eq!(
        blind(&named, Order::AThenB, &[]),
        Err(BlindingRefused::ArmIdentifiable {
            pair: "pair-0".to_string(),
            token: "fresh arm".to_string()
        })
    );
}

#[test]
fn a_judgment_needs_both_orders_under_one_judge_and_records_lengths() {
    let pairs = pairs();
    let calls = vec![
        call("pair-0", Order::AThenB, RawVerdict::First),
        call("pair-0", Order::BThenA, RawVerdict::Second),
        call("pair-1", Order::AThenB, RawVerdict::First),
        call("pair-1", Order::BThenA, RawVerdict::First),
        call("pair-2", Order::AThenB, RawVerdict::Tie),
        call("pair-2", Order::BThenA, RawVerdict::Tie),
    ];
    let judged = judge_pairs(&pairs, &judge(), &calls, &[]).unwrap();
    assert_eq!(
        judged[0].preference,
        Preference::A,
        "first in AB and second in BA is A both times"
    );
    assert_eq!(
        judged[1].preference,
        Preference::Inconsistent,
        "first in both orders followed position"
    );
    assert_eq!(judged[2].preference, Preference::Tie);
    assert_eq!(
        (judged[0].length_a, judged[0].length_b),
        (pairs[0].a.len() as u64, pairs[0].b.len() as u64)
    );

    let one_order = vec![call("pair-0", Order::AThenB, RawVerdict::First)];
    assert_eq!(
        judge_pairs(&pairs[..1], &judge(), &one_order, &[]),
        Err(JudgeRefused::OrderMissing {
            pair: "pair-0".to_string(),
            order: Order::BThenA
        }),
        "an omitted order swap fails the blinding check"
    );
    let mut other_judge = call("pair-0", Order::BThenA, RawVerdict::Second);
    other_judge.judge.prompt_digest = "bb".repeat(32);
    assert_eq!(
        judge_pairs(
            &pairs[..1],
            &judge(),
            &[one_order[0].clone(), other_judge],
            &[]
        ),
        Err(JudgeRefused::JudgeDiffers {
            pair: "pair-0".to_string()
        })
    );
    assert_eq!(
        judge_pairs(
            &pairs[..1],
            &judge(),
            &[call("pair-9", Order::AThenB, RawVerdict::Tie)],
            &[]
        ),
        Err(JudgeRefused::UnknownPair {
            pair: "pair-9".to_string()
        })
    );
}

#[test]
fn the_permutation_check_and_the_human_floor_gate_calibrated_acceptance() {
    PermutationCheck {
        trials: 40,
        correct: 24,
    }
    .validate()
    .unwrap();
    assert_eq!(
        PermutationCheck {
            trials: 40,
            correct: 25
        }
        .validate(),
        Err(PermutationRefused::ArmsIdentifiable {
            trials: 40,
            correct: 25
        }),
        "wrong-arm leakage above the ceiling fails the blinding check"
    );
    assert_eq!(
        PermutationCheck {
            trials: 0,
            correct: 0
        }
        .validate(),
        Err(PermutationRefused::NoTrials)
    );

    assert_eq!(
        SamplingPlan::required_human_sample(40),
        HUMAN_SAMPLE_MIN_PAIRS
    );
    assert_eq!(SamplingPlan::required_human_sample(250), 25);
    assert_eq!(
        SamplingPlan::required_human_sample(251),
        26,
        "ten percent rounds up"
    );
    SamplingPlan {
        pairs: 250,
        human_sample: 25,
    }
    .validate()
    .unwrap();
    assert_eq!(
        SamplingPlan {
            pairs: 250,
            human_sample: 24
        }
        .validate(),
        Err(CalibrationRefused::HumanSampleBelowFloor {
            required: 25,
            planned: 24
        })
    );
    assert_eq!(
        SamplingPlan {
            pairs: 20,
            human_sample: 21
        }
        .validate(),
        Err(CalibrationRefused::HumanSampleExceedsPairs {
            pairs: 20,
            planned: 21
        }),
        "humans cannot review more pairs than were judged"
    );
    assert_eq!(
        SamplingPlan {
            pairs: 19,
            human_sample: 19
        }
        .validate(),
        Err(CalibrationRefused::TooFewPairs {
            pairs: 19,
            minimum: 20
        }),
        "a campaign under the floor cannot claim calibrated acceptance"
    );
}

#[test]
fn a_changed_judge_provider_or_tokenizer_refuses_cross_run_residual_comparison() {
    let calibration = calibration();
    let base = report(judge(), provider("live-1"), &calibration);
    base.validate(&calibration).unwrap();
    base.comparable(&base).unwrap();
    let mut other_judge = base.clone();
    other_judge.judge.provider.model = "judge-2".to_string();
    assert_eq!(
        base.comparable(&other_judge),
        Err(ResidualRefused::ReanchorRequired { changed: "judge" })
    );
    let mut other_live = base.clone();
    other_live.live_provider = provider("live-2");
    assert_eq!(
        base.comparable(&other_live),
        Err(ResidualRefused::ReanchorRequired {
            changed: "live_provider"
        })
    );
    let mut other_tokenizer = base.clone();
    other_tokenizer.live_provider.tokenizer_profile = "tp-2".to_string();
    assert_eq!(
        base.comparable(&other_tokenizer),
        Err(ResidualRefused::ReanchorRequired {
            changed: "live_provider"
        }),
        "a tokenizer accounting profile is part of the identity, not a name"
    );
    let mut rescored = calibration.clone();
    rescored.judge.prompt_digest = "bb".repeat(32);
    let mut other_anchor = base.clone();
    other_anchor.calibration_digest = rescored.digest();
    assert_eq!(
        base.comparable(&other_anchor),
        Err(ResidualRefused::ReanchorRequired {
            changed: "calibration_digest"
        })
    );
    let mut foreign = base.clone();
    foreign.schema = "eval-residual-report/v999".to_string();
    let foreign_schema = Err(ResidualRefused::SchemaMismatch {
        found: "eval-residual-report/v999".to_string(),
    });
    assert_eq!(
        base.comparable(&foreign),
        foreign_schema,
        "a report under another contract compares with nothing"
    );
    assert_eq!(foreign.comparable(&base), foreign_schema);
    let mut unbound = base.clone();
    unbound.judge.rubric_digest = "not-a-digest".to_string();
    let malformed = Err(ResidualRefused::Calibration(
        CalibrationRefused::MalformedDigest {
            field: "rubric_digest",
        },
    ));
    assert_eq!(
        unbound.comparable(&unbound),
        malformed,
        "two reports sharing a malformed digest share no judge"
    );
    assert_eq!(base.comparable(&unbound), malformed);
    let mut leaked = base.clone();
    leaked.permutation.correct = 40;
    assert!(matches!(
        leaked.validate(&calibration),
        Err(ResidualRefused::Permutation(_))
    ));
    let mut under = base;
    under.sampling.human_sample = 1;
    assert!(matches!(
        under.validate(&calibration),
        Err(ResidualRefused::Calibration(_))
    ));
}

/// Coercing `analyze` to this pointer type stops compiling if the gates gain
/// an input, so a judge verdict can reach them only through a `PairOutcome`
/// or the arm rates, whose serialized fields are pinned here beside the
/// residual report's.
#[test]
fn the_gates_take_only_oracle_inputs_and_the_residual_report_carries_no_gate_field() {
    let _gates: fn(
        &FrozenFamily,
        &AnalysisFamily,
        &[PairOutcome],
        &BTreeMap<String, ArmRates>,
    ) -> _ = analyze;
    let keys = |value: serde_json::Value| -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    let outcome = PairOutcome {
        pair_id: "cargo-0".to_string(),
        cluster: ClusterKey {
            family: "cargo".to_string(),
            world_seed: 0,
        },
        fresh: ArmResult::Pass,
        aged: ArmResult::Fail,
    };
    assert_eq!(
        keys(serde_json::to_value(outcome).unwrap()),
        ["aged", "cluster", "fresh", "pair_id"]
    );
    let rates = ArmRates {
        miss_rate: "0.01".to_string(),
        refusal_rate: "0.01".to_string(),
    };
    assert_eq!(
        keys(serde_json::to_value(rates).unwrap()),
        ["miss_rate", "refusal_rate"]
    );
    let residual = report(judge(), provider("live-1"), &calibration());
    assert_eq!(
        keys(serde_json::to_value(residual).unwrap()),
        [
            "calibration_digest",
            "judge",
            "judgments",
            "live_provider",
            "permutation",
            "sampling",
            "schema"
        ],
        "a residual is a separate record with no gate field"
    );
}

#[test]
fn the_live_slice_reports_trials_intervals_and_censoring_and_is_never_replayable() {
    let tasks = vec![
        LiveTask {
            task: "t1".to_string(),
            attempts: vec![ArmResult::Pass, ArmResult::Pass, ArmResult::Fail],
        },
        LiveTask {
            task: "t2".to_string(),
            attempts: vec![
                ArmResult::Censored(CensorReason::HardDeadlineMs),
                ArmResult::Censored(CensorReason::MaxToolCalls),
                ArmResult::Censored(CensorReason::Timeout),
            ],
        },
    ];
    let report = live_slice(&settings(2), &provider("live-1"), &tasks).unwrap();
    report.validate(&settings(2)).unwrap();
    assert!(!report.replayable);
    const { assert!(!LIVE_REPLAYABLE) };
    assert_eq!(report.tasks[0].pass_k.repeats, 3);
    assert_eq!(report.tasks[0].pass_k.k, 2);
    assert!(matches!(
        report.tasks[0].pass_k.pass_k,
        PassKBounds::Bounds { .. }
    ));
    assert!(!report.tasks[0].indeterminate);
    assert!(
        report.tasks[1].indeterminate,
        "all attempts censored is indeterminate, never zero"
    );
    assert_eq!(report.tasks[1].pass_k.pass_k, PassKBounds::Indeterminate);
    assert_eq!(report.tasks[1].pass_k.censoring_rate, eval_core::Ratio::ONE);
    let mut relabelled = report.clone();
    relabelled.replayable = true;
    assert_eq!(
        relabelled.validate(&settings(2)),
        Err(LiveSliceRefused::RelabelledReplayable)
    );
    assert!(
        matches!(
            live_slice(&settings(4), &provider("live-1"), &tasks),
            Err(LiveSliceRefused::Statistics(_))
        ),
        "k past the repeats refuses"
    );
    assert_eq!(
        live_slice(&settings(2), &provider("live-1"), &[]),
        Err(LiveSliceRefused::NoTasks)
    );
}

#[test]
fn live_settings_refuse_until_two_profiles_a_calibration_set_and_a_plan_exist() {
    let settings = settings(3);
    settings.validate().unwrap();
    let mut one = settings.clone();
    one.providers.pop();
    assert_eq!(
        one.validate(),
        Err(LiveSettingsRefused::ProviderCount { found: 1 })
    );
    let mut same = settings.clone();
    same.providers[1] = provider("live-1");
    assert_eq!(
        same.validate(),
        Err(LiveSettingsRefused::ProviderCount { found: 2 })
    );
    let mut zero = settings.clone();
    zero.k = 0;
    assert_eq!(zero.validate(), Err(LiveSettingsRefused::ZeroRepeats));
    let mut no_cal = settings.clone();
    no_cal.calibration = None;
    assert_eq!(
        no_cal.validate(),
        Err(LiveSettingsRefused::NoCalibrationSet)
    );
    let mut empty = settings.clone();
    empty.calibration.as_mut().unwrap().human_labels.clear();
    assert_eq!(
        empty.validate(),
        Err(LiveSettingsRefused::Calibration(
            CalibrationRefused::EmptyCalibrationSet
        ))
    );
    let mut no_plan = settings.clone();
    no_plan.sampling = None;
    assert_eq!(no_plan.validate(), Err(LiveSettingsRefused::NoSamplingPlan));
    let mut thin = settings;
    thin.sampling = Some(SamplingPlan {
        pairs: 40,
        human_sample: 3,
    });
    assert!(matches!(
        thin.validate(),
        Err(LiveSettingsRefused::Calibration(_))
    ));
}

#[test]
fn the_judge_view_serializes_only_the_two_texts() {
    let pair = &pairs()[0];
    for order in Order::BOTH {
        let prompt = blind(pair, order, &[]).unwrap();
        let shown = serde_json::to_value(&prompt).unwrap();
        let keys: Vec<&str> = shown
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["first", "second"], "no order and no pair id");
        assert_eq!((prompt.pair.as_str(), prompt.order), ("pair-0", order));
    }
}

#[test]
fn arm_names_match_as_whole_words_after_folding_case_width_and_separators() {
    let refused = |text: &str| {
        let mut pair = pairs()[0].clone();
        pair.b = text.to_string();
        blind(&pair, Order::AThenB, &[])
    };
    for (leaked, token) in [
        ("as the fresh-arm I answer", "fresh arm"),
        ("as the fresh\narm I answer", "fresh arm"),
        ("as the fresh\u{a0}arm I answer", "fresh arm"),
        ("as the Fresh  Arm I answer", "fresh arm"),
        (
            "as the \u{ff26}\u{ff32}\u{ff25}\u{ff33}\u{ff28} \u{ff21}\u{ff32}\u{ff2d} I answer",
            "fresh arm",
        ),
        ("this is arm_a speaking", "arm a"),
        ("ARM B.", "arm b"),
    ] {
        assert_eq!(
            refused(leaked),
            Err(BlindingRefused::ArmIdentifiable {
                pair: "pair-0".to_string(),
                token: token.to_string()
            }),
            "{leaked:?}"
        );
    }
    for benign in [
        "the harm bound holds",
        "warm bread",
        "the alarm appears",
        "ARM and x86",
        "a firearm aside",
    ] {
        assert!(refused(benign).is_ok(), "{benign:?} names no arm");
    }
}

#[test]
fn judge_pairs_refuses_duplicate_calls_duplicate_pairs_and_unblinded_pairs() {
    let pairs = pairs();
    let rerolled = [
        call("pair-0", Order::AThenB, RawVerdict::First),
        call("pair-0", Order::BThenA, RawVerdict::First),
        call("pair-0", Order::BThenA, RawVerdict::Second),
    ];
    assert_eq!(
        judge_pairs(&pairs[..1], &judge(), &rerolled, &[]),
        Err(JudgeRefused::DuplicateCall {
            pair: "pair-0".to_string(),
            order: Order::BThenA
        }),
        "a rerolled call cannot replace an inconsistent verdict"
    );
    let both = [
        call("pair-0", Order::AThenB, RawVerdict::First),
        call("pair-0", Order::BThenA, RawVerdict::Second),
    ];
    assert_eq!(
        judge_pairs(&[pairs[0].clone(), pairs[0].clone()], &judge(), &both, &[]),
        Err(JudgeRefused::DuplicatePair {
            pair: "pair-0".to_string()
        }),
        "one pair's calls cannot judge two pairs"
    );
    let mut named = pairs[0].clone();
    named.b = "as the fresh arm I answer".to_string();
    assert_eq!(
        judge_pairs(&[named], &judge(), &both, &[]),
        Err(JudgeRefused::Blinding(BlindingRefused::ArmIdentifiable {
            pair: "pair-0".to_string(),
            token: "fresh arm".to_string()
        }))
    );
    assert_eq!(
        judge_pairs(&pairs[..1], &judge(), &both, &["rename".to_string()]),
        Err(JudgeRefused::Blinding(BlindingRefused::CanaryInPrompt {
            pair: "pair-0".to_string(),
            canary: "rename".to_string()
        }))
    );
}

#[test]
fn the_permutation_check_is_two_sided() {
    for correct in [16, 24] {
        PermutationCheck {
            trials: 40,
            correct,
        }
        .validate()
        .unwrap();
    }
    for correct in [0, 15] {
        assert_eq!(
            PermutationCheck {
                trials: 40,
                correct
            }
            .validate(),
            Err(PermutationRefused::ArmsIdentifiable {
                trials: 40,
                correct
            }),
            "naming the arm wrong {correct} of 40 times identifies it"
        );
    }
    assert_eq!(
        PermutationCheck {
            trials: 40,
            correct: 41
        }
        .validate(),
        Err(PermutationRefused::CorrectExceedsTrials {
            trials: 40,
            correct: 41
        })
    );
}

#[test]
fn a_residual_report_reconciles_its_judgments_and_its_calibration_set() {
    let calibration = calibration();
    let base = report(judge(), provider("live-1"), &calibration);
    base.validate(&calibration).unwrap();

    let mut empty = base.clone();
    empty.judgments.clear();
    assert_eq!(
        empty.validate(&calibration),
        Err(ResidualRefused::JudgmentCountMismatch {
            declared: 40,
            judged: 0
        })
    );
    let mut understated = base.clone();
    understated
        .judgments
        .extend((40..1000).map(|i| PairJudgment {
            pair: format!("pair-{i}"),
            preference: Preference::A,
            length_a: 1,
            length_b: 1,
        }));
    assert_eq!(
        understated.validate(&calibration),
        Err(ResidualRefused::JudgmentCountMismatch {
            declared: 40,
            judged: 1000
        }),
        "1000 judgments cannot claim the 40-pair human floor"
    );
    let mut repeated = base.clone();
    repeated.judgments[1].pair = "pair-0".to_string();
    assert_eq!(
        repeated.validate(&calibration),
        Err(ResidualRefused::DuplicateJudgment {
            pair: "pair-0".to_string()
        })
    );

    let mut other_judge = judge();
    other_judge.provider.model = "judge-2".to_string();
    let uncalibrated = report(other_judge, provider("live-1"), &calibration);
    assert_eq!(
        uncalibrated.validate(&calibration),
        Err(ResidualRefused::Calibration(
            CalibrationRefused::CalibrationJudgeDiffers
        ))
    );
    let mut relabelled = calibration.clone();
    relabelled
        .human_labels
        .insert("anchor-2".to_string(), Preference::B);
    assert_eq!(
        base.validate(&relabelled),
        Err(ResidualRefused::Calibration(
            CalibrationRefused::DigestMismatch
        ))
    );
    let mut unlabelled = calibration;
    unlabelled.human_labels.clear();
    assert_eq!(
        base.validate(&unlabelled),
        Err(ResidualRefused::Calibration(
            CalibrationRefused::EmptyCalibrationSet
        ))
    );
}

#[test]
fn live_slice_validation_recomputes_each_task_and_checks_the_schema() {
    let tasks = vec![LiveTask {
        task: "t".to_string(),
        attempts: vec![
            ArmResult::Censored(CensorReason::Timeout),
            ArmResult::Censored(CensorReason::Timeout),
        ],
    }];
    let report = live_slice(&settings(2), &provider("live-1"), &tasks).unwrap();
    report.validate(&settings(2)).unwrap();
    let mut schema = report.clone();
    schema.schema = "eval-live-slice/v999".to_string();
    assert_eq!(
        schema.validate(&settings(2)),
        Err(LiveSliceRefused::SchemaMismatch {
            found: "eval-live-slice/v999".to_string()
        })
    );
    let inconsistent = Err(LiveSliceRefused::InconsistentTask {
        task: "t".to_string(),
    });
    let mut k = report.clone();
    k.k = 1;
    k.settings_digest = settings(1).digest();
    assert_eq!(
        k.validate(&settings(1)),
        inconsistent,
        "outer k disagrees with the task"
    );
    assert_eq!(
        k.validate(&settings(2)),
        Err(LiveSliceRefused::RepeatCountDiffers {
            report: 1,
            settings: 2
        }),
        "a deserialized report is held to the settings' k"
    );
    let mut foreign = report.clone();
    foreign.provider = provider("live-3");
    assert_eq!(
        foreign.validate(&settings(2)),
        Err(LiveSliceRefused::UnapprovedProvider {
            provider: "anthropic/live-3@tp-1".to_string()
        }),
        "a deserialized report names an approved profile"
    );
    let mut other_plan = settings(2);
    other_plan.sampling = Some(SamplingPlan {
        pairs: 60,
        human_sample: 20,
    });
    other_plan.validate().unwrap();
    assert_eq!(
        report.validate(&other_plan),
        Err(LiveSliceRefused::SettingsDigestMismatch),
        "a report ran under one pre-registration, not any valid one"
    );
    let mut unsettled = settings(2);
    unsettled.calibration = None;
    assert_eq!(
        report.validate(&unsettled),
        Err(LiveSliceRefused::Settings(
            LiveSettingsRefused::NoCalibrationSet
        )),
        "validation consumes the settings"
    );
    let mut flipped = report.clone();
    flipped.tasks[0].indeterminate = false;
    assert_eq!(
        flipped.validate(&settings(2)),
        inconsistent,
        "all censored is indeterminate"
    );
    let mut fabricated = report;
    fabricated.tasks[0].indeterminate = false;
    fabricated.tasks[0].pass_k.pass_k = PassKBounds::Bounds {
        censored_as_fail: Ratio::ONE,
        censored_excluded: Ratio::ONE,
    };
    assert_eq!(
        fabricated.validate(&settings(2)),
        inconsistent,
        "bounds the attempts do not give"
    );
}

#[test]
fn calibration_refuses_a_foreign_schema_and_a_malformed_judge_digest() {
    let mut old = calibration();
    old.schema = "eval-judge/v0".to_string();
    assert_eq!(
        old.validate(),
        Err(CalibrationRefused::SchemaMismatch {
            found: "eval-judge/v0".to_string()
        })
    );
    assert_eq!(
        Rubric {
            schema: "eval-judge/v0".to_string(),
            criteria: vec!["correctness".to_string()],
        }
        .digest(),
        Err(CalibrationRefused::SchemaMismatch {
            found: "eval-judge/v0".to_string()
        }),
        "a rubric under another schema has no digest to bind a judge to"
    );
    let mut short = calibration();
    short.judge.prompt_digest = "abc".to_string();
    assert_eq!(
        short.validate(),
        Err(CalibrationRefused::MalformedDigest {
            field: "prompt_digest"
        })
    );
    let mut upper = calibration();
    upper.judge.rubric_digest = "AA".repeat(32);
    assert_eq!(
        upper.validate(),
        Err(CalibrationRefused::MalformedDigest {
            field: "rubric_digest"
        })
    );
    let mut judge_only = calibration();
    judge_only
        .human_labels
        .insert("anchor-2".to_string(), Preference::Inconsistent);
    assert_eq!(
        judge_only.validate(),
        Err(CalibrationRefused::InconsistentHumanLabel {
            pair: "anchor-2".to_string()
        }),
        "a human labels A, B, or a tie; only two judge orders are inconsistent"
    );
    let mut unbound = judge();
    unbound.prompt_digest = String::new();
    let both = [
        call("pair-0", Order::AThenB, RawVerdict::First),
        call("pair-0", Order::BThenA, RawVerdict::Second),
    ];
    assert_eq!(
        judge_pairs(&pairs()[..1], &unbound, &both, &[]),
        Err(JudgeRefused::MalformedDigest {
            field: "prompt_digest"
        }),
        "an empty digest is not a judge identity"
    );
}

#[test]
fn the_live_slice_refuses_a_repeated_task() {
    let task = LiveTask {
        task: "t".to_string(),
        attempts: vec![ArmResult::Pass, ArmResult::Fail],
    };
    assert_eq!(
        live_slice(
            &settings(1),
            &provider("live-1"),
            &[task.clone(), task.clone()]
        ),
        Err(LiveSliceRefused::DuplicateTask {
            task: "t".to_string()
        })
    );
    let mut report = live_slice(&settings(1), &provider("live-1"), &[task]).unwrap();
    report.tasks.push(report.tasks[0].clone());
    assert_eq!(
        report.validate(&settings(1)),
        Err(LiveSliceRefused::DuplicateTask {
            task: "t".to_string()
        })
    );
}

#[test]
fn the_live_slice_is_constructed_only_from_validated_settings_and_an_approved_profile() {
    let tasks = vec![LiveTask {
        task: "t".to_string(),
        attempts: vec![ArmResult::Pass],
    }];
    assert_eq!(
        live_slice(&settings(1), &provider("live-3"), &tasks),
        Err(LiveSliceRefused::UnapprovedProvider {
            provider: "anthropic/live-3@tp-1".to_string()
        }),
        "a third profile is not one of the two approved"
    );
    let mut no_plan = settings(1);
    no_plan.sampling = None;
    assert_eq!(
        live_slice(&no_plan, &provider("live-1"), &tasks),
        Err(LiveSliceRefused::Settings(
            LiveSettingsRefused::NoSamplingPlan
        )),
        "unvalidated settings construct no live evidence"
    );
    assert_eq!(
        live_slice(&settings(1), &provider("live-1"), &tasks)
            .unwrap()
            .k,
        1,
        "k is the settings' repeat count"
    );
}
