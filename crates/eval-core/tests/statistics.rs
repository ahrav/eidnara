mod support;

use std::collections::BTreeMap;

use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, ArmRates, ArmResult, BlockedReason,
    CampaignProfile, CensorReason, ClusterKey, ClusteringUnit, FrozenFamily, Gates, ICC_THRESHOLD,
    ITEM_COUNT_THRESHOLD, IccPilot, Interval, IntervalMethod, IntervalOutcome, IntervalWithheld,
    LivenessBounds, MIN_BOOTSTRAP_REPLICATES, MultiplicityCorrection, PairCounts, PairOutcome,
    PilotObservation, Ratio, StatisticsError, StoppingRule, analyze, arm_miss_asymmetry,
    cluster_bootstrap_interval, intraclass_correlation, parse_analysis_family,
    parse_campaign_profile, run_icc_pilot,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REFERENCE_VERSION: &str = "statistics-reference-ts-v1";

type Edit = (&'static str, Box<dyn Fn(&mut AnalysisFamily)>);

fn ratio(numerator: i64, denominator: u64) -> Ratio {
    Ratio::new(numerator, denominator)
}

fn profile() -> CampaignProfile {
    CampaignProfile {
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
    }
}

fn pilot(effective_n_at_max: Ratio, required: u32) -> IccPilot {
    IccPilot {
        pilot_run_id: "ab".repeat(32),
        n_items: 360,
        n_families: 6,
        n_worlds: 120,
        icc_family: ratio(1, 4),
        icc_world_seed: ratio(0, 1),
        clustering_unit: ClusteringUnit::Family,
        max_affordable_worlds: 60,
        effective_n_at_max,
        required_n_for_margin: required,
    }
}

fn family() -> AnalysisFamily {
    AnalysisFamily {
        schema: ANALYSIS_FAMILY_SCHEMA.to_string(),
        endpoints: vec!["quality_loss".into(), "harm".into(), "floor".into()],
        families: vec!["cargo".into(), "tokio".into()],
        exclusions: vec![],
        stopping_rule: StoppingRule::FixedN,
        multiplicity_correction: MultiplicityCorrection::Holm,
        profile: profile(),
        interval_method: IntervalMethod::ClusterBootstrap,
        item_count_threshold: ITEM_COUNT_THRESHOLD,
        bootstrap_replicates: 400,
        bootstrap_seed: 7,
        trials_k: 3,
        icc_pilot: pilot(ratio(400, 1), 385),
    }
}

fn key(family: &str, seed: u64) -> ClusterKey {
    ClusterKey {
        family: family.to_string(),
        world_seed: seed,
    }
}

fn pair(id: &str, family: &str, seed: u64, fresh: ArmResult, aged: ArmResult) -> PairOutcome {
    PairOutcome {
        pair_id: id.to_string(),
        cluster: key(family, seed),
        fresh,
        aged,
    }
}

fn observation(family: &str, seed: u64, task: &str, value: i64) -> PilotObservation {
    PilotObservation {
        cluster: key(family, seed),
        task: task.to_string(),
        value,
    }
}

fn arm_rates(fresh_miss: &str, aged_miss: &str) -> BTreeMap<String, ArmRates> {
    let arm = |miss: &str| ArmRates {
        miss_rate: miss.to_string(),
        refusal_rate: "0.01".to_string(),
    };
    BTreeMap::from([
        ("fresh".to_string(), arm(fresh_miss)),
        ("aged".to_string(), arm(aged_miss)),
    ])
}

/// Three hundred pairs over six families, mostly concordant.
fn pairs_at_threshold() -> Vec<PairOutcome> {
    let families = ["cargo", "tokio", "django", "git", "docs", "tests"];
    families
        .iter()
        .enumerate()
        .flat_map(|(f, family)| {
            (0..50u64).map(move |seed| {
                let aged = match (f as u64 * 7 + seed * 13) % 20 {
                    0 => ArmResult::Fail,
                    1 => ArmResult::Censored(CensorReason::Timeout),
                    _ => ArmResult::Pass,
                };
                pair(
                    &format!("{family}-{seed}"),
                    family,
                    seed,
                    ArmResult::Pass,
                    aged,
                )
            })
        })
        .collect()
}

fn computed(outcome: Result<IntervalOutcome, StatisticsError>) -> Interval {
    match outcome.unwrap() {
        IntervalOutcome::Computed(interval) => interval,
        IntervalOutcome::Withheld { reason } => panic!("withheld: {reason:?}"),
    }
}

#[test]
fn the_frozen_reference_agrees_on_every_golden_case() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/statistics-golden.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(golden["schema"], json!(1));
    assert_eq!(
        golden["provenance"]["generator_version"],
        json!(REFERENCE_VERSION)
    );
    // Provenance covers the whole case array, expectations included, so a hand-edited
    // expectation is caught before any assertion runs.
    let canonical = format!(
        "{}\n",
        serde_json::to_string_pretty(&golden["cases"]).unwrap()
    );
    assert_eq!(
        golden["provenance"]["input_sha256"],
        json!(format!("{:x}", Sha256::digest(canonical.as_bytes())))
    );
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 26);
    let rates = profile().rates().unwrap();
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let expected = &case["expected"];
        match case["kind"].as_str().unwrap() {
            "pair_counts" => {
                let pairs: Vec<PairOutcome> =
                    serde_json::from_value(case["input"]["pairs"].clone()).unwrap();
                let counts = PairCounts::of(&pairs);
                let actual = json!({
                    "n": counts.n, "b": counts.b, "c": counts.c, "aged_pass": counts.aged_pass,
                    "fresh_censored": counts.fresh_censored, "aged_censored": counts.aged_censored,
                    "quality_loss": counts.quality_loss(), "harm": counts.harm(),
                    "aged_pass_rate": counts.aged_pass_rate(),
                    "gates": Gates::of(&counts, &rates).unwrap(),
                });
                assert_eq!(&actual, expected, "{id}");
            }
            "icc_pilot" => {
                let observations: Vec<PilotObservation> =
                    serde_json::from_value(case["input"]["observations"].clone()).unwrap();
                let worlds = case["input"]["max_affordable_worlds"].as_u64().unwrap() as u32;
                let pilot = run_icc_pilot("pilot", &observations, worlds, 1).unwrap();
                let actual = json!({
                    "icc_family": pilot.icc_family, "icc_world_seed": pilot.icc_world_seed,
                    "clustering_unit": pilot.clustering_unit, "n_families": pilot.n_families,
                    "n_worlds": pilot.n_worlds, "effective_n_at_max": pilot.effective_n_at_max,
                });
                assert_eq!(&actual, expected, "{id}");
            }
            "cluster_bootstrap" => {
                let input = &case["input"];
                let pairs: Vec<PairOutcome> =
                    serde_json::from_value(input["pairs"].clone()).unwrap();
                let unit: ClusteringUnit = serde_json::from_value(input["unit"].clone()).unwrap();
                let interval = computed(cluster_bootstrap_interval(
                    &pairs,
                    unit,
                    ITEM_COUNT_THRESHOLD,
                    input["seed"].as_u64().unwrap(),
                    input["replicates"].as_u64().unwrap() as u32,
                ));
                let actual = json!({
                    "n_clusters": interval.n_clusters, "n_items": interval.n_items,
                    "lower": interval.lower, "upper": interval.upper,
                });
                assert_eq!(&actual, expected, "{id}");
                assert_eq!(interval.method, IntervalMethod::ClusterBootstrap);
            }
            // Censored-outcome cases are asserted by `tests/censoring.rs`.
            "latency" | "counter" | "pass_k" => {}
            other => panic!("unknown golden kind {other}"),
        }
    }
}

#[test]
fn ratios_are_exact_normalized_and_refuse_overflow() {
    assert_eq!(ratio(6, 8), ratio(3, 4));
    assert_eq!(ratio(-6, 8), ratio(-3, 4));
    assert_eq!(Ratio::try_new(3, -4).unwrap(), ratio(-3, 4));
    assert_eq!(Ratio::from_decimal("0.25"), Some(ratio(1, 4)));
    assert_eq!(Ratio::from_decimal("0.020"), None, "not canonical");
    assert_eq!(Ratio::from_decimal("1"), Some(Ratio::ONE));
    assert!(ratio(1, 3) < ratio(34, 100));
    assert!(ratio(-1, 10) < Ratio::ZERO);
    assert_eq!(ratio(1, 3).checked_add(ratio(1, 6)).unwrap(), ratio(1, 2));
    assert_eq!(ratio(1, 3).checked_sub(ratio(1, 2)).unwrap(), ratio(-1, 6));
    assert_eq!(ratio(2, 3).checked_mul(ratio(3, 4)).unwrap(), ratio(1, 2));
    assert_eq!(ratio(1, 2).checked_div(ratio(3, 4)).unwrap(), ratio(2, 3));
    assert_eq!(
        ratio(1, 2).checked_div(Ratio::ZERO).err(),
        Some(StatisticsError::ZeroDenominator)
    );
    // A difference of two tiny rationals is exact, never wrapped to zero.
    let tiny = Ratio::try_new(1, 10_000_000_000).unwrap();
    let tinier = Ratio::try_new(1, 10_000_000_001).unwrap();
    assert_eq!(
        tiny.checked_sub(tinier).err(),
        Some(StatisticsError::RationalOverflow),
        "past the safe range the arithmetic refuses"
    );
    assert_eq!(
        Ratio::try_new(1 << 53, 1).err(),
        Some(StatisticsError::RationalOverflow)
    );
    assert_eq!(
        Ratio::try_new(1, 0).err(),
        Some(StatisticsError::ZeroDenominator)
    );
    // Deserialization normalizes, so equality and ordering agree on any wire form.
    let unnormalized: Ratio =
        serde_json::from_value(json!({"numerator": 2, "denominator": 4})).unwrap();
    assert_eq!(unnormalized, ratio(1, 2));
    assert!(serde_json::from_value::<Ratio>(json!({"numerator": 1, "denominator": 0})).is_err());
    assert!(
        serde_json::from_value::<Ratio>(json!({"numerator": 1, "denominator": 2, "x": 0})).is_err()
    );
}

#[test]
fn every_profile_input_is_required_and_bounded() {
    let value = serde_json::to_value(profile()).unwrap();
    assert_eq!(parse_campaign_profile(&value).unwrap(), profile());
    for field in [
        "noninferiority_margin",
        "harm_bound",
        "floor_threshold",
        "miss_asymmetry_bound",
        "liveness_bounds",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            matches!(
                parse_campaign_profile(&missing),
                Err(StatisticsError::Shape(_))
            ),
            "{field} is required"
        );
    }
    for bound in [
        "catch_up_episodes",
        "embedding_passes",
        "materialization_episodes",
        "reviewer_coordinator_passes",
    ] {
        let mut partial = value.clone();
        partial["liveness_bounds"]
            .as_object_mut()
            .unwrap()
            .remove(bound);
        assert!(
            matches!(
                parse_campaign_profile(&partial),
                Err(StatisticsError::Shape(_))
            ),
            "{bound} is required"
        );
    }
    let mut wide = value.clone();
    wide["harm_bound"] = json!("1.5");
    assert_eq!(
        parse_campaign_profile(&wide).err(),
        Some(StatisticsError::RateOutOfRange {
            field: "harm_bound"
        })
    );
    let mut malformed = value;
    malformed["floor_threshold"] = json!(".7");
    assert_eq!(
        parse_campaign_profile(&malformed).err(),
        Some(StatisticsError::MalformedDecimal {
            field: "floor_threshold"
        })
    );
}

#[test]
fn the_family_is_frozen_before_outcomes_and_any_post_hoc_edit_refuses() {
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    assert_eq!(frozen.analysis_family_digest.len(), 64);
    assert_eq!(frozen.check(&family), Ok(()));
    let value = serde_json::to_value(&family).unwrap();
    assert_eq!(parse_analysis_family(&value).unwrap(), family);

    let edits: Vec<Edit> = vec![
        (
            "endpoints",
            Box::new(|f| f.endpoints.push("residual".into())),
        ),
        (
            "families",
            Box::new(|f| f.families.pop().map(drop).unwrap()),
        ),
        (
            "exclusions",
            Box::new(|f| f.exclusions.push("drop timed-out fresh runs".into())),
        ),
        (
            "multiplicity",
            Box::new(|f| f.multiplicity_correction = MultiplicityCorrection::None),
        ),
        (
            "margin",
            Box::new(|f| f.profile.noninferiority_margin = "0.03".into()),
        ),
        (
            "floor",
            Box::new(|f| f.profile.floor_threshold = "0.6".into()),
        ),
        ("threshold", Box::new(|f| f.item_count_threshold += 1)),
        ("seed", Box::new(|f| f.bootstrap_seed += 1)),
        (
            "pilot",
            Box::new(|f| f.icc_pilot.required_n_for_margin += 1),
        ),
    ];
    for (label, edit) in edits {
        let mut edited = family.clone();
        edit(&mut edited);
        assert!(
            matches!(
                frozen.check(&edited),
                Err(StatisticsError::FamilyChangedAfterResults { recorded, found })
                    if recorded == frozen.analysis_family_digest && found != recorded
            ),
            "{label}"
        );
    }
    let mut edited = family.clone();
    edited.exclusions.push("post hoc".into());
    assert!(matches!(
        analyze(
            &frozen,
            &edited,
            &pairs_at_threshold(),
            &arm_rates("0", "0")
        ),
        Err(StatisticsError::FamilyChangedAfterResults { .. })
    ));

    let mut low_threshold = family.clone();
    low_threshold.item_count_threshold = 299;
    assert_eq!(
        low_threshold.validate(),
        Err(StatisticsError::ItemCountThresholdBelowFloor(299))
    );
    let mut few = family.clone();
    few.bootstrap_replicates = 39;
    assert_eq!(few.validate(), Err(StatisticsError::TooFewReplicates(39)));
    let mut empty = family.clone();
    empty.endpoints.clear();
    assert_eq!(empty.validate(), Err(StatisticsError::EmptyFamilyField));
    let mut no_repeats = family.clone();
    no_repeats.trials_k = 0;
    assert_eq!(
        no_repeats.validate(),
        Err(StatisticsError::EmptyFamilyField)
    );
    let mut extra = value;
    extra["interval_tolerance"] = json!("0");
    assert!(matches!(
        parse_analysis_family(&extra),
        Err(StatisticsError::Shape(_))
    ));

    // The manifest's recorded digest is the freeze; a manifest without one supports no report.
    let mut manifest = support::manifest();
    assert_eq!(
        FrozenFamily::from_manifest(&manifest).err(),
        Some(StatisticsError::FamilyNotRecorded)
    );
    manifest.analysis_family_digest = Some(frozen.analysis_family_digest.clone());
    assert_eq!(FrozenFamily::from_manifest(&manifest).unwrap(), frozen);
}

#[test]
fn the_pilot_picks_the_highest_level_over_the_threshold_and_blocks_when_underpowered() {
    // Three families, four worlds each, two tasks per world; a strong family effect.
    let observations: Vec<PilotObservation> = ["a", "b", "c"]
        .iter()
        .enumerate()
        .flat_map(|(f, family)| {
            (0..4u64).flat_map(move |seed| {
                (0..2i64).map(move |task| {
                    observation(
                        family,
                        seed,
                        &format!("t{task}"),
                        f as i64 * 10 + seed as i64 % 2 + task,
                    )
                })
            })
        })
        .collect();
    let pilot = run_icc_pilot("p", &observations, 8, 20).unwrap();
    assert!(pilot.icc_family > ICC_THRESHOLD);
    assert_eq!(pilot.clustering_unit, ClusteringUnit::Family);
    assert_eq!(
        (pilot.n_families, pilot.n_worlds, pilot.n_items),
        (3, 12, 24)
    );
    // Sixteen items at eight worlds, deflated by the family design effect, never inflated.
    assert!(pilot.effective_n_at_max < ratio(16, 1));
    assert!(pilot.effective_n_at_max > Ratio::ZERO);

    let flat: Vec<PilotObservation> = observations
        .iter()
        .map(|o| PilotObservation {
            value: o.cluster.world_seed as i64 % 2,
            ..o.clone()
        })
        .collect();
    let flat_pilot = run_icc_pilot("p", &flat, 8, 20).unwrap();
    assert!(flat_pilot.icc_family <= ICC_THRESHOLD);
    assert_eq!(flat_pilot.clustering_unit, ClusteringUnit::WorldSeed);
    // Both tasks in a world score alike, so the world ICC is one and each world counts once.
    assert_eq!(flat_pilot.icc_world_seed, Ratio::ONE);
    assert_eq!(flat_pilot.effective_n_at_max, ratio(8, 1));
    // Fewer affordable worlds than the pilot had shrinks N to what those worlds hold.
    assert_eq!(
        run_icc_pilot("p", &flat, 1, 20).unwrap().effective_n_at_max,
        Ratio::ONE
    );

    assert_eq!(
        run_icc_pilot("p", &observations, 0, 20).err(),
        Some(StatisticsError::NoAffordableWorlds)
    );
    assert_eq!(
        run_icc_pilot("p", &observations[..2], 8, 20).err(),
        Some(StatisticsError::PilotTooSmall)
    );
    assert_eq!(
        intraclass_correlation(&[vec![1, 2]]),
        Err(StatisticsError::PilotTooSmall)
    );
    assert_eq!(
        intraclass_correlation(&[vec![5, 5], vec![9, 9]]),
        Ok(Ratio::ONE),
        "constant groups"
    );
    assert_eq!(
        intraclass_correlation(&[vec![5, 5], vec![5, 5]]),
        Err(StatisticsError::PilotTooSmall),
        "no variance at all"
    );
    assert_eq!(
        intraclass_correlation(&[vec![1, 1, 1], vec![9]]).unwrap(),
        Ratio::ONE,
        "unbalanced constant groups"
    );
    assert_eq!(
        intraclass_correlation(&[vec![1 << 40, 1], vec![1 << 41, 2]]).err(),
        Some(StatisticsError::RationalOverflow)
    );

    let mut underpowered = family();
    underpowered.icc_pilot = pilot.clone();
    let frozen = FrozenFamily::freeze(&underpowered).unwrap();
    assert_eq!(
        analyze(
            &frozen,
            &underpowered,
            &pairs_at_threshold(),
            &arm_rates("0", "0")
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::InsufficientEffectiveN {
            effective_n_at_max: pilot.effective_n_at_max,
            required_n_for_margin: 20,
        })
    );
    // A hand-written pilot cannot slip past the block with a degenerate ratio.
    let mut degenerate = serde_json::to_value(&underpowered).unwrap();
    degenerate["icc_pilot"]["effective_n_at_max"] = json!({"numerator": 0, "denominator": 0});
    assert!(matches!(
        parse_analysis_family(&degenerate),
        Err(StatisticsError::Shape(_))
    ));
}

#[test]
fn the_three_gates_are_separate_signed_and_bound_by_the_profile() {
    let rates = profile().rates().unwrap();
    let counts = |b, c, n, aged_pass| PairCounts {
        n,
        b,
        c,
        aged_pass,
        fresh_censored: 0,
        aged_censored: 0,
    };
    let gates = Gates::of(&counts(3, 5, 20, 17), &rates).unwrap();
    assert_eq!(gates.quality_loss.statistic, ratio(-1, 10));
    assert!(
        gates.quality_loss.passed,
        "aged better passes noninferiority"
    );
    assert_eq!(gates.harm.statistic, ratio(3, 20));
    assert!(!gates.harm.passed, "three of twenty pairs lost knowledge");
    assert_eq!(gates.floor.statistic, ratio(17, 20));
    assert!(gates.floor.passed);
    assert_eq!(
        (
            gates.quality_loss.bound,
            gates.harm.bound,
            gates.floor.bound
        ),
        (
            rates.noninferiority_margin,
            rates.harm_bound,
            rates.floor_threshold
        ),
        "bounds come from the profile, never the counts"
    );

    let equal = Gates::of(&counts(4, 4, 20, 4), &rates).unwrap();
    assert_eq!(equal.quality_loss.statistic, Ratio::ZERO);
    assert!(equal.quality_loss.passed && !equal.harm.passed && !equal.floor.passed);

    let worse = Gates::of(&counts(5, 0, 20, 15), &rates).unwrap();
    assert_eq!(worse.quality_loss.statistic, ratio(1, 4));
    assert!(!worse.quality_loss.passed);
    let at_margin = Gates::of(&counts(1, 0, 50, 49), &rates).unwrap();
    assert_eq!(at_margin.quality_loss.statistic, ratio(1, 50));
    assert!(at_margin.quality_loss.passed && at_margin.harm.passed);
    let at_floor = Gates::of(&counts(0, 0, 10, 7), &rates).unwrap();
    assert!(at_floor.floor.passed);
    let under_floor = Gates::of(&counts(0, 0, 100, 69), &rates).unwrap();
    assert!(!under_floor.floor.passed);
    assert_eq!(
        Gates::of(&PairCounts::default(), &rates).err(),
        Some(StatisticsError::NoPairs)
    );

    // Censoring only ever makes a gate harder: a censored aged arm is a loss, a censored
    // fresh arm is neither a pass nor a fail, and no censored arm is a pass.
    let censored = PairCounts::of(&[
        pair(
            "loss",
            "f",
            0,
            ArmResult::Pass,
            ArmResult::Censored(CensorReason::MaxTokensOut),
        ),
        pair(
            "unknown",
            "f",
            1,
            ArmResult::Censored(CensorReason::HardDeadlineMs),
            ArmResult::Pass,
        ),
        pair("kept", "g", 0, ArmResult::Pass, ArmResult::Pass),
    ]);
    assert_eq!(
        censored,
        PairCounts {
            n: 3,
            b: 1,
            c: 0,
            aged_pass: 2,
            fresh_censored: 1,
            aged_censored: 1,
        }
    );
}

#[test]
fn intervals_name_their_unit_and_counts_and_are_withheld_below_the_floor() {
    let pairs = pairs_at_threshold();
    let interval = computed(cluster_bootstrap_interval(
        &pairs,
        ClusteringUnit::Family,
        300,
        7,
        400,
    ));
    assert_eq!(
        (
            interval.unit,
            interval.n_clusters,
            interval.n_items,
            interval.replicates
        ),
        (ClusteringUnit::Family, 6, 300, 400)
    );
    assert_eq!(
        (interval.lower, interval.upper),
        (ratio(1, 10), ratio(7, 60))
    );
    let by_world = computed(cluster_bootstrap_interval(
        &pairs,
        ClusteringUnit::WorldSeed,
        300,
        7,
        400,
    ));
    assert_eq!(by_world.n_clusters, 300);
    assert_eq!(
        cluster_bootstrap_interval(&pairs[..299], ClusteringUnit::Family, 300, 7, 400).unwrap(),
        IntervalOutcome::Withheld {
            reason: IntervalWithheld::ItemCountBelowThreshold {
                n_items: 299,
                threshold: 300
            }
        }
    );
    // A caller cannot lower the floor or the replicate count below the pinned minimums.
    assert!(matches!(
        cluster_bootstrap_interval(&pairs[..2], ClusteringUnit::Family, 0, 7, 400).unwrap(),
        IntervalOutcome::Withheld {
            reason: IntervalWithheld::ItemCountBelowThreshold { threshold: 300, .. }
        }
    ));
    assert_eq!(
        cluster_bootstrap_interval(
            &pairs,
            ClusteringUnit::Family,
            300,
            7,
            MIN_BOOTSTRAP_REPLICATES - 1
        )
        .err(),
        Some(StatisticsError::TooFewReplicates(39))
    );
    let one_family: Vec<PairOutcome> = pairs
        .iter()
        .map(|p| PairOutcome {
            cluster: key("only", p.cluster.world_seed),
            ..p.clone()
        })
        .collect();
    assert_eq!(
        cluster_bootstrap_interval(&one_family, ClusteringUnit::Family, 300, 7, 400).unwrap(),
        IntervalOutcome::Withheld {
            reason: IntervalWithheld::FewerThanTwoClusters { n_clusters: 1 }
        }
    );
    // The draw is the pinned digest, so the interval is a function of the seed alone.
    let again = computed(cluster_bootstrap_interval(
        &pairs,
        ClusteringUnit::Family,
        300,
        7,
        400,
    ));
    assert_eq!(again, interval);
    let other_seed = computed(cluster_bootstrap_interval(
        &pairs,
        ClusteringUnit::Family,
        300,
        8,
        400,
    ));
    assert_ne!(
        (other_seed.lower, other_seed.upper),
        (interval.lower, interval.upper)
    );
    let outcome = serde_json::to_value(IntervalOutcome::Computed(interval)).unwrap();
    assert_eq!(outcome["outcome"], json!("computed"));
    assert_eq!(outcome["method"], json!("cluster_bootstrap"));
}

#[test]
fn arm_miss_asymmetry_past_the_bound_blocks_with_no_gates_and_rates_are_retained() {
    assert_eq!(
        arm_miss_asymmetry(&arm_rates("0.1", "0.02")).unwrap(),
        ratio(2, 25)
    );
    assert_eq!(
        arm_miss_asymmetry(&BTreeMap::new()).err(),
        Some(StatisticsError::TooFewArms(0))
    );
    let mut one_arm = arm_rates("0.9", "0");
    one_arm.remove("aged");
    assert_eq!(
        arm_miss_asymmetry(&one_arm).err(),
        Some(StatisticsError::TooFewArms(1))
    );
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let pairs = pairs_at_threshold();
    assert_eq!(
        analyze(&frozen, &family, &pairs, &arm_rates("0.1", "0.02")).unwrap(),
        Analysis::Blocked(BlockedReason::ArmMissAsymmetry {
            asymmetry: ratio(2, 25),
            bound: ratio(1, 20),
        })
    );
    assert_eq!(
        analyze(&frozen, &family, &pairs, &one_arm).err(),
        Some(StatisticsError::TooFewArms(1))
    );
    let Analysis::Report(report) =
        analyze(&frozen, &family, &pairs, &arm_rates("0.05", "0")).unwrap()
    else {
        panic!("at the bound reports");
    };
    assert_eq!(report.analysis_family_digest, frozen.analysis_family_digest);
    assert_eq!(report.counts, PairCounts::of(&pairs));
    assert_eq!(
        report.arm_rates,
        arm_rates("0.05", "0"),
        "miss and refusal rates travel"
    );
    let IntervalOutcome::Computed(Interval { unit, .. }) = &report.interval else {
        panic!("300 items over six families carry an interval");
    };
    assert_eq!(*unit, ClusteringUnit::Family);
    // The report serializes with the three gates as separate fields and no collapsed effect.
    let value = serde_json::to_value(&report).unwrap();
    for gate in ["quality_loss", "harm", "floor"] {
        assert!(value["gates"][gate]["passed"].is_boolean(), "{gate}");
    }
    let keys: Vec<&String> = value["gates"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["floor", "harm", "quality_loss"]);
}

/// Gates take oracle verdicts only: the statistics module names no judge, so
/// no judge verdict type can reach a control, floor, or gate.
#[test]
fn no_judge_type_reaches_the_gates() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/statistics.rs"),
    )
    .unwrap();
    assert!(!source.to_ascii_lowercase().contains("judge"));
}
