//! Residual judging: blinding, both presentation orders, the permutation
//! check, the human sample floor, identity-bound comparison, the judge-type
//! firewall around the gates, and the held-out live slice.

use std::collections::BTreeMap;

use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, AnalysisFamily, ArmRates, ArmResult, BlindingRefused,
    CalibrationRefused, CalibrationSet, CampaignProfile, CensorReason, ClusterKey, ClusteringUnit,
    FrozenFamily, HUMAN_SAMPLE_MIN_PAIRS, ITEM_COUNT_THRESHOLD, IccPilot, IntervalMethod,
    JUDGE_SCHEMA, JudgeCall, JudgeIdentity, JudgeRefused, JudgedPair, LIVE_REPLAYABLE,
    LiveSettings, LiveSettingsRefused, LiveSliceRefused, LiveTask, LivenessBounds,
    MultiplicityCorrection, Order, PairOutcome, PassKBounds, PermutationCheck, PermutationRefused,
    Preference, ProviderProfile, RESIDUAL_REPORT_SCHEMA, Ratio, RawVerdict, ResidualRefused,
    ResidualReport, Rubric, SamplingPlan, StoppingRule, analyze, blind, judge_pairs, live_slice,
};
use serde_json::json;

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
        .digest(),
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
        judgments: Vec::new(),
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
    let judged = judge_pairs(&pairs, &judge(), &calls).unwrap();
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
        judge_pairs(&pairs[..1], &judge(), &one_order),
        Err(JudgeRefused::OrderMissing {
            pair: "pair-0".to_string(),
            order: Order::BThenA
        }),
        "an omitted order swap fails the blinding check"
    );
    let mut other_judge = call("pair-0", Order::BThenA, RawVerdict::Second);
    other_judge.judge.prompt_digest = "bb".repeat(32);
    assert_eq!(
        judge_pairs(&pairs[..1], &judge(), &[one_order[0].clone(), other_judge]),
        Err(JudgeRefused::JudgeDiffers {
            pair: "pair-0".to_string()
        })
    );
    assert_eq!(
        judge_pairs(
            &pairs[..1],
            &judge(),
            &[call("pair-9", Order::AThenB, RawVerdict::Tie)]
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
    base.validate().unwrap();
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
    let mut leaked = base.clone();
    leaked.permutation.correct = 40;
    assert!(matches!(
        leaked.validate(),
        Err(ResidualRefused::Permutation(_))
    ));
    let mut under = base;
    under.sampling.human_sample = 1;
    assert!(matches!(
        under.validate(),
        Err(ResidualRefused::Calibration(_))
    ));
}

/// The gates take pair outcomes and arm rates; no judge type reaches them, so
/// a poisoned judge cannot change a byte of their output.
#[test]
fn the_gates_take_no_judge_verdict_and_are_byte_identical_under_a_poisoned_judge() {
    let ratio = |n: i64, d: u64| Ratio::new(n, d);
    let family = AnalysisFamily {
        schema: ANALYSIS_FAMILY_SCHEMA.to_string(),
        endpoints: vec!["quality_loss".into(), "harm".into(), "floor".into()],
        families: vec!["cargo".into(), "tokio".into()],
        exclusions: vec![],
        stopping_rule: StoppingRule::FixedN,
        multiplicity_correction: MultiplicityCorrection::Holm,
        profile: CampaignProfile {
            noninferiority_margin: "0.02".to_string(),
            harm_bound: "0.1".to_string(),
            floor_threshold: "0.7".to_string(),
            miss_asymmetry_bound: "0.05".to_string(),
            liveness_bounds: LivenessBounds {
                catch_up_episodes: 64,
                embedding_passes: 32,
                materialization_episodes: 16,
                reviewer_coordinator_passes: 8,
            },
        },
        interval_method: IntervalMethod::ClusterBootstrap,
        item_count_threshold: ITEM_COUNT_THRESHOLD,
        bootstrap_replicates: 400,
        bootstrap_seed: 7,
        trials_k: 3,
        icc_pilot: IccPilot {
            pilot_run_id: "ab".repeat(32),
            n_items: 360,
            n_families: 6,
            n_worlds: 120,
            icc_family: ratio(1, 4),
            icc_world_seed: ratio(0, 1),
            clustering_unit: ClusteringUnit::Family,
            max_affordable_worlds: 60,
            effective_n_at_max: ratio(400, 1),
            required_n_for_margin: 385,
        },
        transfer_criterion: None,
    };
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let families = ["cargo", "tokio", "django", "git", "docs", "tests"];
    let pairs: Vec<PairOutcome> = families
        .iter()
        .enumerate()
        .flat_map(|(f, family)| {
            (0..50u64).map(move |seed| PairOutcome {
                pair_id: format!("{family}-{seed}"),
                cluster: ClusterKey {
                    family: family.to_string(),
                    world_seed: seed,
                },
                fresh: ArmResult::Pass,
                aged: if (f as u64 * 7 + seed * 13).is_multiple_of(20) {
                    ArmResult::Fail
                } else {
                    ArmResult::Pass
                },
            })
        })
        .collect();
    let rates: BTreeMap<String, ArmRates> = ["fresh", "aged"]
        .iter()
        .map(|arm| {
            (
                arm.to_string(),
                ArmRates {
                    miss_rate: "0.01".to_string(),
                    refusal_rate: "0.01".to_string(),
                },
            )
        })
        .collect();
    let honest = serde_json::to_vec(&analyze(&frozen, &family, &pairs, &rates).unwrap()).unwrap();
    // A judge that prefers the aged arm everywhere, or is inconsistent
    // everywhere, exists beside the analysis and reaches nothing in it.
    for poison in [Preference::A, Preference::Inconsistent] {
        let _judged: Vec<Preference> = pairs.iter().map(|_| poison).collect();
        let again =
            serde_json::to_vec(&analyze(&frozen, &family, &pairs, &rates).unwrap()).unwrap();
        assert_eq!(
            honest, again,
            "the gates' bytes do not depend on any judge verdict"
        );
    }
    assert_eq!(
        json!({"residual": "preference"}).get("gate"),
        None,
        "a residual is a separate record, never a gate field"
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
    let report = live_slice(&provider("live-1"), 2, &tasks).unwrap();
    report.validate().unwrap();
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
        relabelled.validate(),
        Err(LiveSliceRefused::RelabelledReplayable)
    );
    assert!(
        matches!(
            live_slice(&provider("live-1"), 4, &tasks),
            Err(LiveSliceRefused::Statistics(_))
        ),
        "k past the repeats refuses"
    );
    assert_eq!(
        live_slice(&provider("live-1"), 2, &[]),
        Err(LiveSliceRefused::NoTasks)
    );
}

#[test]
fn live_settings_refuse_until_two_profiles_a_calibration_set_and_a_plan_exist() {
    let settings = LiveSettings {
        providers: vec![provider("live-1"), provider("live-2")],
        k: 3,
        calibration: Some(calibration()),
        sampling: Some(SamplingPlan {
            pairs: 40,
            human_sample: 20,
        }),
    };
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
