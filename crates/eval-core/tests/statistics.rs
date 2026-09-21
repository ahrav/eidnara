mod support;

use std::collections::BTreeMap;

use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, ArmRates, ArmResult, BlockedReason,
    CampaignProfile, CensorReason, ClusterKey, ClusteringUnit, FrozenFamily, GATE_ENDPOINTS, Gates,
    ICC_THRESHOLD, ITEM_COUNT_THRESHOLD, IccPilot, Interval, IntervalMethod, IntervalOutcome,
    IntervalWithheld, LivenessBounds, MAX_BOOTSTRAP_REPLICATES, MIN_BOOTSTRAP_REPLICATES, Manifest,
    ManifestError, MultiplicityCorrection, PairCounts, PairOutcome, PilotObservation, Ratio,
    RunStatus, StatisticsError, StoppingRule, analyze, arm_miss_asymmetry,
    cluster_bootstrap_interval, intraclass_correlation, pair_table_digest, parse_analysis_family,
    parse_campaign_profile, run_icc_pilot,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REFERENCE_VERSION: &str = "statistics-reference-ts-v1";
const FAMILIES: [&str; 6] = ["cargo", "tokio", "django", "git", "docs", "tests"];

type Edit = (&'static str, Box<dyn Fn(&mut AnalysisFamily)>);

fn ratio(numerator: i64, denominator: u64) -> Ratio {
    Ratio::try_new(i128::from(numerator), i128::from(denominator)).unwrap()
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

/// A pilot whose recorded counts and ICCs imply the world unit and
/// `900 * 5/6 = 750` effective items at 300 affordable worlds.
fn pilot(required: u32) -> IccPilot {
    IccPilot {
        pilot_run_id: "ab".repeat(32),
        families: FAMILIES
            .iter()
            .map(|family| family.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        n_items: 360,
        n_families: 6,
        n_worlds: 120,
        icc_family: ratio(0, 1),
        icc_world_seed: ratio(1, 10),
        clustering_unit: ClusteringUnit::WorldSeed,
        max_affordable_worlds: 300,
        effective_n_at_max: ratio(750, 1),
        required_n_for_margin: required,
    }
}

fn family() -> AnalysisFamily {
    AnalysisFamily {
        schema: ANALYSIS_FAMILY_SCHEMA.to_string(),
        endpoints: GATE_ENDPOINTS.iter().map(|gate| gate.to_string()).collect(),
        families: FAMILIES.iter().map(|family| family.to_string()).collect(),
        exclusions: vec![],
        stopping_rule: StoppingRule::FixedN { pairs: 300 },
        multiplicity_correction: MultiplicityCorrection::None,
        profile: profile(),
        interval_method: IntervalMethod::ClusterBootstrap,
        item_count_threshold: ITEM_COUNT_THRESHOLD,
        bootstrap_replicates: 400,
        bootstrap_seed: 7,
        icc_pilot: pilot(300),
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

/// A manifest that recorded `frozen` before its first outcome, carries `rates`,
/// and names `pairs` as its samples.
fn recorded(
    frozen: &FrozenFamily,
    rates: BTreeMap<String, ArmRates>,
    pairs: &[PairOutcome],
) -> Manifest {
    let mut manifest = support::manifest();
    manifest.analysis_family_digest = Some(frozen.analysis_family_digest.clone());
    manifest.arm_rates = rates;
    manifest.sample_ids = pairs.iter().map(|pair| pair.pair_id.clone()).collect();
    manifest.sample_order = manifest.sample_ids.clone();
    manifest.result_digest = pair_table_digest(pairs).unwrap();
    manifest
}

/// Three hundred pairs over six families, mostly concordant.
fn pairs_at_threshold() -> Vec<PairOutcome> {
    FAMILIES
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
    assert_eq!(cases.len(), 14);
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
                    "quality_loss": counts.quality_loss().unwrap(), "harm": counts.harm().unwrap(),
                    "aged_pass_rate": counts.aged_pass_rate().unwrap(),
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
    // A profile that parses can be frozen: an integer past canonical JSON's safe
    // range is refused here, not first when the family is digested.
    let mut unsafe_bound = value.clone();
    unsafe_bound["liveness_bounds"]["catch_up_episodes"] = json!(9_007_199_254_740_992u64);
    assert!(matches!(
        parse_campaign_profile(&unsafe_bound),
        Err(StatisticsError::NotCanonical(_))
    ));
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
        ("endpoints", Box::new(|f| f.endpoints.reverse())),
        ("families", Box::new(|f| f.families.reverse())),
        (
            "exclusions",
            Box::new(|f| f.exclusions.push("drop timed-out fresh runs".into())),
        ),
        (
            "stopping",
            Box::new(|f| f.stopping_rule = StoppingRule::FixedN { pairs: 301 }),
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
            Box::new(|f| f.icc_pilot.required_n_for_margin -= 1),
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
            &recorded(&frozen, arm_rates("0", "0"), &pairs_at_threshold()),
            &edited,
            &pairs_at_threshold(),
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
    let mut many = family.clone();
    many.bootstrap_replicates = MAX_BOOTSTRAP_REPLICATES + 1;
    assert_eq!(
        many.validate(),
        Err(StatisticsError::TooManyReplicates(
            MAX_BOOTSTRAP_REPLICATES + 1
        ))
    );
    let mut empty = family.clone();
    empty.endpoints.clear();
    assert_eq!(empty.validate(), Err(StatisticsError::EmptyFamilyField));
    // The report always carries the three gates, so the frozen endpoint list
    // names exactly those: a subset, an unknown endpoint, or a repeat is refused.
    for declared in [
        vec!["harm"],
        vec!["quality_loss", "harm", "floor", "residual"],
        vec!["quality_loss", "harm", "floor", "harm"],
    ] {
        let mut edited = family.clone();
        edited.endpoints = declared.iter().map(|gate| gate.to_string()).collect();
        assert_eq!(
            edited.validate(),
            Err(StatisticsError::UnsupportedEndpoints {
                declared: edited.endpoints.clone()
            })
        );
    }
    let mut extra = value.clone();
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
    assert_eq!(
        analyze(&manifest, &family, &pairs_at_threshold()).err(),
        Some(StatisticsError::FamilyNotRecorded)
    );
    manifest.analysis_family_digest = Some(frozen.analysis_family_digest.clone());
    assert_eq!(FrozenFamily::from_manifest(&manifest).unwrap(), frozen);
    // A family that validates can always be frozen: an integer outside the
    // canonical-JSON safe range is refused at parse, not first at freeze.
    let mut unsafe_seed = value.clone();
    unsafe_seed["bootstrap_seed"] = json!(9007199254740992u64);
    assert!(matches!(
        parse_analysis_family(&unsafe_seed),
        Err(StatisticsError::NotCanonical(_))
    ));
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
    // Each affordable world lies in one family, so two worlds realize at most two
    // family clusters however many the pilot sampled: the four projected items
    // are deflated, not spread over three clusters and left undeflated.
    let two_worlds = run_icc_pilot("p", &observations, 2, 20).unwrap();
    assert_eq!(two_worlds.clustering_unit, ClusteringUnit::Family);
    assert!(two_worlds.effective_n_at_max < ratio(4, 1));

    assert_eq!(
        run_icc_pilot("p", &observations, 0, 20).err(),
        Some(StatisticsError::NoAffordableWorlds)
    );
    assert_eq!(
        run_icc_pilot("p", &observations, 8, 0).err(),
        Some(StatisticsError::NoRequiredN)
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
    // Unequal groups use n0 = (N - sum(n_i^2) / N) / (k - 1), not the mean size:
    // here the mean would put the ICC under the threshold and flip the unit.
    assert_eq!(
        intraclass_correlation(&[vec![0], vec![0, 2], vec![0, 2, 2, 2, 3]]).unwrap(),
        ratio(71, 1227)
    );
    assert!(ratio(71, 1227) > ICC_THRESHOLD && ratio(213, 4565) < ICC_THRESHOLD);
    assert_eq!(
        intraclass_correlation(&[vec![1 << 40, 1], vec![1 << 41, 2]]).err(),
        Some(StatisticsError::RationalOverflow)
    );

    // A plan that could reach the required N with its own pairs, but whose pilot
    // projects too few items at the affordable worlds, freezes and then blocks.
    let mut underpowered = family();
    underpowered.icc_pilot.icc_world_seed = Ratio::ZERO;
    underpowered.icc_pilot.max_affordable_worlds = 8;
    underpowered.icc_pilot.effective_n_at_max = ratio(24, 1);
    underpowered.icc_pilot.required_n_for_margin = 100;
    let frozen = FrozenFamily::freeze(&underpowered).unwrap();
    assert_eq!(
        analyze(
            &recorded(&frozen, arm_rates("0", "0"), &pairs_at_threshold()),
            &underpowered,
            &pairs_at_threshold(),
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::InsufficientEffectiveN {
            effective_n_at_max: ratio(24, 1),
            required_n_for_margin: 100,
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
            Ratio::from_decimal(&profile().noninferiority_margin).unwrap(),
            Ratio::from_decimal(&profile().harm_bound).unwrap(),
            Ratio::from_decimal(&profile().floor_threshold).unwrap()
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

    // Each censored arm resolves to the verdict least favorable to the aged arm: a censored
    // aged arm is not a pass, a censored fresh arm is a pass, and `c` needs a definite fresh
    // fail, so a censored fresh arm beside an aged pass is a concordant pair.
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
        pair(
            "fresh_lost",
            "f",
            2,
            ArmResult::Censored(CensorReason::Timeout),
            ArmResult::Fail,
        ),
        pair(
            "both",
            "f",
            3,
            ArmResult::Censored(CensorReason::MaxModelCalls),
            ArmResult::Censored(CensorReason::MaxToolCalls),
        ),
        pair("kept", "g", 0, ArmResult::Pass, ArmResult::Pass),
    ]);
    assert_eq!(
        censored,
        PairCounts {
            n: 5,
            b: 3,
            c: 0,
            aged_pass: 2,
            fresh_censored: 3,
            aged_censored: 2,
        }
    );
}

/// Every cell with a censored arm yields gate statistics at least as hard as
/// each definite resolution of that arm against the same background.
#[test]
fn censoring_never_makes_a_gate_easier_than_any_definite_resolution() {
    let definite = [ArmResult::Pass, ArmResult::Fail];
    let censored = ArmResult::Censored(CensorReason::Timeout);
    let arms = [ArmResult::Pass, ArmResult::Fail, censored];
    let background: Vec<PairOutcome> = (0..9u64)
        .map(|seed| {
            pair(
                &format!("bg-{seed}"),
                "bg",
                seed,
                ArmResult::Pass,
                ArmResult::Pass,
            )
        })
        .collect();
    let counts = |fresh, aged| {
        let mut pairs = background.clone();
        pairs.push(pair("cell", "cell", 0, fresh, aged));
        PairCounts::of(&pairs)
    };
    let resolutions = |arm: ArmResult| -> Vec<ArmResult> {
        if matches!(arm, ArmResult::Censored(_)) {
            definite.to_vec()
        } else {
            vec![arm]
        }
    };
    let mut cells = 0;
    for fresh in arms {
        for aged in arms {
            if !matches!(fresh, ArmResult::Censored(_)) && !matches!(aged, ArmResult::Censored(_)) {
                continue;
            }
            cells += 1;
            let actual = counts(fresh, aged);
            for fresh_resolved in resolutions(fresh) {
                for aged_resolved in resolutions(aged) {
                    let resolved = counts(fresh_resolved, aged_resolved);
                    let cell =
                        format!("{fresh:?}/{aged:?} vs {fresh_resolved:?}/{aged_resolved:?}");
                    assert!(
                        actual.quality_loss().unwrap() >= resolved.quality_loss().unwrap(),
                        "quality_loss easier: {cell}"
                    );
                    assert!(
                        actual.harm().unwrap() >= resolved.harm().unwrap(),
                        "harm easier: {cell}"
                    );
                    assert!(
                        actual.aged_pass_rate().unwrap() <= resolved.aged_pass_rate().unwrap(),
                        "floor easier: {cell}"
                    );
                }
            }
        }
    }
    assert_eq!(cells, 5, "every cell with a censored arm is covered");
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
    // A replicate count past the cap is refused before any replicate runs.
    assert_eq!(
        cluster_bootstrap_interval(&pairs, ClusteringUnit::Family, 300, 7, u32::MAX).err(),
        Some(StatisticsError::TooManyReplicates(u32::MAX))
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
        Some(StatisticsError::ArmsNotPaired { found: vec![] })
    );
    let mut one_arm = arm_rates("0.9", "0");
    one_arm.remove("aged");
    assert_eq!(
        arm_miss_asymmetry(&one_arm).err(),
        Some(StatisticsError::ArmsNotPaired {
            found: vec!["fresh".to_string()]
        })
    );
    // Two rates under other names are not the pair's arms, however equal they are.
    let mut renamed = arm_rates("0", "0");
    let control = renamed.remove("aged").unwrap();
    renamed.insert("control".to_string(), control);
    assert_eq!(
        arm_miss_asymmetry(&renamed).err(),
        Some(StatisticsError::ArmsNotPaired {
            found: vec!["control".to_string(), "fresh".to_string()]
        })
    );
    let mut third = arm_rates("0", "0");
    third.insert("control".to_string(), third["aged"].clone());
    assert!(matches!(
        arm_miss_asymmetry(&third),
        Err(StatisticsError::ArmsNotPaired { .. })
    ));
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let pairs = pairs_at_threshold();
    assert_eq!(
        analyze(
            &recorded(&frozen, arm_rates("0.1", "0.02"), &pairs),
            &family,
            &pairs
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::ArmMissAsymmetry {
            asymmetry: ratio(2, 25),
            bound: ratio(1, 20),
        })
    );
    assert!(matches!(
        analyze(&recorded(&frozen, renamed.clone(), &pairs), &family, &pairs),
        Err(StatisticsError::ArmsNotPaired { .. })
    ));
    let Analysis::Report(report) = analyze(
        &recorded(&frozen, arm_rates("0.05", "0"), &pairs),
        &family,
        &pairs,
    )
    .unwrap() else {
        panic!("at the bound reports");
    };
    assert_eq!(report.analysis_family_digest, frozen.analysis_family_digest);
    assert_eq!(report.counts, PairCounts::of(&pairs));
    assert_eq!(
        report.arm_rates,
        arm_rates("0.05", "0"),
        "miss and refusal rates travel"
    );
    let IntervalOutcome::Computed(Interval {
        unit, n_clusters, ..
    }) = &report.interval
    else {
        panic!("300 items over 300 worlds carry an interval");
    };
    assert_eq!((*unit, *n_clusters), (ClusteringUnit::WorldSeed, 300));
    // The report serializes with the three gates as separate fields and no collapsed effect.
    let value = serde_json::to_value(&report).unwrap();
    for gate in ["quality_loss", "harm", "floor"] {
        assert!(value["gates"][gate]["passed"].is_boolean(), "{gate}");
    }
    let keys: Vec<&String> = value["gates"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["floor", "harm", "quality_loss"]);
}

/// The pair table is read against the frozen plan: its size is the frozen
/// pair count, every pair falls in a frozen task family, and no pair repeats;
/// the pilot's unit and effective N match its own counts and ICCs and the plan
/// can reach its required N; a rate outside `[0, 1]` and counts no table
/// produces are refused; and `i128::MIN` is a refusal, never a wrap.
#[test]
fn the_pair_table_and_the_pilot_must_match_the_frozen_plan() {
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let pairs = pairs_at_threshold();
    let rates = arm_rates("0", "0");
    assert!(matches!(
        analyze(&recorded(&frozen, rates.clone(), &pairs), &family, &pairs).unwrap(),
        Analysis::Report(_)
    ));
    // One pair is not the frozen count, so no gate is computed from it.
    assert_eq!(
        analyze(
            &recorded(&frozen, rates.clone(), &pairs[..1]),
            &family,
            &pairs[..1]
        )
        .err(),
        Some(StatisticsError::PairCountMismatch {
            expected: 300,
            found: 1
        }),
        "an early-stopped table reports no gates"
    );
    assert_eq!(
        analyze(
            &recorded(&frozen, rates.clone(), &pairs[..299]),
            &family,
            &pairs[..299]
        )
        .err(),
        Some(StatisticsError::PairCountMismatch {
            expected: 300,
            found: 299
        })
    );
    // A pair from a family the plan did not freeze is refused, whatever the count.
    let mut foreign = pairs.clone();
    foreign[0].cluster.family = "rails".to_string();
    assert_eq!(
        analyze(
            &recorded(&frozen, rates.clone(), &foreign),
            &family,
            &foreign
        )
        .err(),
        Some(StatisticsError::PairOutsideFamilies {
            pair_id: "cargo-0".to_string(),
            family: "rails".to_string()
        })
    );
    // Three hundred copies of one pair are not three hundred pairs.
    let copies: Vec<PairOutcome> = std::iter::repeat_n(pairs[0].clone(), 300).collect();
    assert_eq!(
        analyze(&recorded(&frozen, rates.clone(), &pairs), &family, &copies).err(),
        Some(StatisticsError::DuplicatePair {
            pair_id: "cargo-0".to_string()
        })
    );
    // The pilot's unit and effective N are recomputed from its recorded counts and
    // ICCs, so a hand-written value that disagrees with its own evidence is refused.
    let mut inflated = family.clone();
    inflated.icc_pilot.effective_n_at_max = ratio(751, 1);
    assert_eq!(inflated.validate(), Err(StatisticsError::PilotInconsistent));
    let mut wrong_unit = family.clone();
    wrong_unit.icc_pilot.clustering_unit = ClusteringUnit::Family;
    assert_eq!(
        wrong_unit.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    let mut no_worlds = family.clone();
    no_worlds.icc_pilot.n_worlds = 0;
    assert_eq!(
        no_worlds.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    // Recorded counts the ICC could not have been estimated from are refused too,
    // however self-consistent the projection over them is.
    let mut one_observation = family.clone();
    one_observation.icc_pilot = IccPilot {
        n_items: 1,
        n_families: 1,
        n_worlds: 1,
        icc_family: Ratio::ZERO,
        icc_world_seed: Ratio::ZERO,
        max_affordable_worlds: 1000,
        effective_n_at_max: ratio(1000, 1),
        ..family.icc_pilot.clone()
    };
    assert_eq!(
        one_observation.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    let mut unreplicated = family.clone();
    unreplicated.icc_pilot.n_items = unreplicated.icc_pilot.n_worlds;
    assert_eq!(
        unreplicated.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    // When every family holds exactly one world the two partitions coincide, so
    // a pilot recording different ICCs for them did not come from the estimator.
    let mut coinciding = family.clone();
    coinciding.families = vec!["a".into(), "b".into()];
    coinciding.icc_pilot = IccPilot {
        families: vec!["a".into(), "b".into()],
        n_items: 300,
        n_families: 2,
        n_worlds: 2,
        icc_family: ratio(1, 20),
        icc_world_seed: Ratio::ZERO,
        max_affordable_worlds: 2,
        effective_n_at_max: ratio(300, 1),
        ..family.icc_pilot.clone()
    };
    assert_eq!(
        coinciding.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    coinciding.icc_pilot.icc_family = Ratio::ZERO;
    assert_eq!(coinciding.validate(), Ok(()), "equal ICCs are consistent");
    // The public constructor is fallible: a zero denominator or an unsafe
    // component is a typed refusal, never a panic.
    assert_eq!(
        Ratio::try_new(1, 0).err(),
        Some(StatisticsError::ZeroDenominator)
    );
    assert_eq!(
        Ratio::try_new(i128::from(i64::MAX), 1).err(),
        Some(StatisticsError::RationalOverflow)
    );
    // An ICC above one is outside the estimator's range; a negative one is
    // clamped to zero by the projection, so it can never raise effective N.
    let mut over_one = family.clone();
    over_one.icc_pilot.icc_world_seed = ratio(2, 1);
    assert_eq!(over_one.validate(), Err(StatisticsError::PilotInconsistent));
    for icc in [Ratio::ZERO, ratio(-100, 1)] {
        let mut clamped = family.clone();
        clamped.icc_pilot.icc_world_seed = icc;
        clamped.icc_pilot.effective_n_at_max = ratio(900, 1);
        assert_eq!(clamped.validate(), Ok(()), "{icc:?} projects undeflated");
    }
    // The three gates are one all-must-pass conclusion over fixed bounds, so no
    // correction applies; a plan declaring one is refused, not analyzed uncorrected.
    for correction in [
        MultiplicityCorrection::Holm,
        MultiplicityCorrection::BenjaminiHochberg,
    ] {
        let mut corrected = family.clone();
        corrected.multiplicity_correction = correction;
        assert_eq!(
            corrected.validate(),
            Err(StatisticsError::UnsupportedMultiplicity(correction))
        );
    }
    // A plan whose pair count is below its own required N cannot reach it.
    let mut short_plan = family.clone();
    short_plan.stopping_rule = StoppingRule::FixedN { pairs: 299 };
    assert_eq!(
        short_plan.validate(),
        Err(StatisticsError::PlanBelowRequiredN {
            attainable: ratio(299, 1),
            required_n_for_margin: 300
        })
    );
    // The bound is over whole pairs: 301 pairs over 150 worlds is one world of
    // three and 149 of two, not 150 worlds of 301/150, so under a world ICC of
    // one it supports 90601/605 items, short of 150.
    let mut uneven = family.clone();
    uneven.stopping_rule = StoppingRule::FixedN { pairs: 301 };
    uneven.icc_pilot.icc_world_seed = Ratio::ONE;
    uneven.icc_pilot.max_affordable_worlds = 150;
    uneven.icc_pilot.effective_n_at_max = ratio(150, 1);
    uneven.icc_pilot.required_n_for_margin = 150;
    assert_eq!(
        uneven.validate(),
        Err(StatisticsError::PlanBelowRequiredN {
            attainable: ratio(90_601, 605),
            required_n_for_margin: 150
        })
    );
    // Worlds nest in families, so the bound is over one joint allocation: two
    // families and three worlds put two worlds in one family, and a real pilot
    // with family ICC 43/195 and world ICC 4/9 supports 1755/443 of six pairs,
    // short of four, although each level balanced on its own would allow 54/13.
    let nested_pilot = run_icc_pilot(
        "p",
        &[
            observation("a", 0, "t", 0),
            observation("a", 0, "u", 0),
            observation("a", 1, "t", 1),
            observation("a", 1, "u", 2),
            observation("b", 0, "t", 1),
            observation("b", 0, "u", 3),
        ],
        3,
        4,
    )
    .unwrap();
    assert_eq!(
        (nested_pilot.icc_family, nested_pilot.icc_world_seed),
        (ratio(43, 195), ratio(4, 9))
    );
    let mut nested_plan = family.clone();
    nested_plan.families = vec!["a".into(), "b".into()];
    nested_plan.icc_pilot = nested_pilot;
    nested_plan.stopping_rule = StoppingRule::FixedN { pairs: 6 };
    assert_eq!(
        nested_plan.validate(),
        Err(StatisticsError::PlanBelowRequiredN {
            attainable: ratio(1755, 443),
            required_n_for_margin: 4
        })
    );
    // The larger worlds go where they even the family totals: 12 pairs over five
    // worlds (3, 3, 2, 2, 2) and two families (two and three worlds) is 6 and 6,
    // which under a family ICC of 2/5 is exactly the four required, not 5 and 7.
    let mut lopsided = family.clone();
    lopsided.families = vec!["a".into(), "b".into()];
    lopsided.icc_pilot = IccPilot {
        families: vec!["a".into(), "b".into()],
        n_items: 12,
        n_families: 2,
        n_worlds: 5,
        icc_family: ratio(2, 5),
        icc_world_seed: Ratio::ZERO,
        clustering_unit: ClusteringUnit::Family,
        max_affordable_worlds: 5,
        effective_n_at_max: ratio(4, 1),
        required_n_for_margin: 4,
        ..family.icc_pilot.clone()
    };
    lopsided.stopping_rule = StoppingRule::FixedN { pairs: 12 };
    assert_eq!(lopsided.validate(), Ok(()));
    // Nor can 300 pairs over at most 150 worlds: the best table has two pairs per
    // world, and under a world ICC of 1/10 that is 3000/11 effective items.
    let mut crowded = family.clone();
    crowded.icc_pilot.max_affordable_worlds = 150;
    crowded.icc_pilot.effective_n_at_max = ratio(375, 1);
    assert_eq!(
        crowded.validate(),
        Err(StatisticsError::PlanBelowRequiredN {
            attainable: ratio(3000, 11),
            required_n_for_margin: 300
        })
    );
    let honest_observations = [
        observation("cargo", 0, "a", 1),
        observation("cargo", 0, "b", 2),
        observation("tokio", 1, "a", 8),
        observation("tokio", 1, "b", 9),
    ];
    let mut honest = family.clone();
    honest.families = vec!["cargo".into(), "tokio".into()];
    honest.icc_pilot = run_icc_pilot("p", &honest_observations, 4, 1).unwrap();
    assert_eq!(honest.validate(), Ok(()), "a computed pilot validates");
    // The pilot sampled the registered population, so its family count is the
    // registered one; a one-family plan cannot carry a six-family projection.
    let mut narrowed = family.clone();
    narrowed.families = vec!["cargo".into()];
    assert_eq!(narrowed.validate(), Err(StatisticsError::PilotInconsistent));
    // Nor can a pilot over other families of the same count: the identities match.
    let mut elsewhere = honest.clone();
    elsewhere.families = vec!["django".into(), "git".into()];
    assert_eq!(
        elsewhere.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    assert_eq!(honest.icc_pilot.families, ["cargo", "tokio"]);
    let mut repeated_family = family.clone();
    repeated_family.families.push("cargo".into());
    assert_eq!(
        repeated_family.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    // The completed table is deflated at the clusters it spans: 300 pairs in one
    // world under a world ICC of 1/10 are fewer than ten effective items.
    let one_world: Vec<PairOutcome> = (0..300)
        .map(|i| {
            pair(
                &format!("w{i}"),
                "cargo",
                0,
                ArmResult::Pass,
                ArmResult::Pass,
            )
        })
        .collect();
    assert_eq!(
        analyze(
            &recorded(&frozen, rates.clone(), &one_world),
            &family,
            &one_world
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::TableUnderpowered {
            effective_n: ratio(3000, 309),
            n_clusters: 1,
            required_n_for_margin: 300
        })
    );
    // Unequal clusters deflate by the size-weighted mean `sum(m_i^2) / n`: a
    // 299/1 split is nearly one cluster, not two clusters of 150.
    let mut split = one_world.clone();
    split[0].cluster.world_seed = 1;
    let mut lenient = family.clone();
    lenient.icc_pilot.required_n_for_margin = 15;
    let lenient_frozen = FrozenFamily::freeze(&lenient).unwrap();
    assert_eq!(
        analyze(
            &recorded(&lenient_frozen, rates.clone(), &split),
            &lenient,
            &split
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::TableUnderpowered {
            effective_n: ratio(450_000, 46_051),
            n_clusters: 2,
            required_n_for_margin: 15
        })
    );
    // The manifest's samples are the pairs; a table of other pairs is refused.
    let mut relabeled = pairs.clone();
    relabeled[0].pair_id = "elsewhere".to_string();
    assert_eq!(
        analyze(
            &recorded(&frozen, rates.clone(), &pairs),
            &family,
            &relabeled
        )
        .err(),
        Some(StatisticsError::PairsNotManifestSamples {
            pairs: 300,
            samples: 300,
            first_unrecorded: Some("elsewhere".to_string())
        })
    );
    // The same ids with re-scored or relabeled rows are not the recorded table:
    // the manifest's `result_digest` is the table's digest, in any row order.
    let mut flipped = pairs.clone();
    flipped[0].fresh = ArmResult::Fail;
    assert!(matches!(
        analyze(&recorded(&frozen, rates.clone(), &pairs), &family, &flipped),
        Err(StatisticsError::PairsNotManifestResult { .. })
    ));
    let mut shuffled = pairs.clone();
    shuffled.reverse();
    assert_eq!(pair_table_digest(&shuffled), pair_table_digest(&pairs));
    assert!(matches!(
        analyze(
            &recorded(&frozen, rates.clone(), &pairs),
            &family,
            &shuffled
        )
        .unwrap(),
        Analysis::Report(_)
    ));
    // The rate helpers validate the counts they divide: no total is not a rate.
    assert_eq!(
        PairCounts {
            n: 0,
            b: 1,
            ..PairCounts::default()
        }
        .harm()
        .err(),
        Some(StatisticsError::NoPairs)
    );
    assert_eq!(
        PairCounts {
            n: 1,
            b: 2,
            ..PairCounts::default()
        }
        .harm()
        .err(),
        Some(StatisticsError::InconsistentCounts)
    );
    // A table over more worlds than the plan could afford is not the registered campaign.
    let mut narrow = family.clone();
    narrow.icc_pilot.max_affordable_worlds = 150;
    narrow.icc_pilot.effective_n_at_max = ratio(375, 1);
    narrow.icc_pilot.required_n_for_margin = 250;
    let narrow_frozen = FrozenFamily::freeze(&narrow).unwrap();
    assert_eq!(
        analyze(
            &recorded(&narrow_frozen, rates.clone(), &pairs),
            &narrow,
            &pairs
        )
        .err(),
        Some(StatisticsError::WorldsExceedAffordable {
            worlds: 300,
            max_affordable_worlds: 150
        })
    );
    // The pre-outcome blocks never read the table, so an underpowered plan blocks
    // before a bad seed in the table could be refused.
    let mut underpowered = family.clone();
    underpowered.icc_pilot.required_n_for_margin = 751;
    underpowered.stopping_rule = StoppingRule::FixedN { pairs: 1000 };
    let underpowered_frozen = FrozenFamily::freeze(&underpowered).unwrap();
    let mut wide = pairs.clone();
    wide[0].cluster.world_seed = 9_007_199_254_740_993;
    assert!(matches!(
        analyze(
            &recorded(&underpowered_frozen, rates.clone(), &pairs),
            &underpowered,
            &wide
        )
        .unwrap(),
        Analysis::Blocked(BlockedReason::InsufficientEffectiveN { .. })
    ));
    // A world seed past canonical JSON's safe integer is refused at both entries.
    assert_eq!(
        analyze(&recorded(&frozen, rates.clone(), &pairs), &family, &wide).err(),
        Some(StatisticsError::WorldSeedOutOfRange(9_007_199_254_740_993))
    );
    assert_eq!(
        run_icc_pilot(
            "p",
            &[
                observation("cargo", 9_007_199_254_740_992, "a", 1),
                observation("cargo", 0, "a", 2),
            ],
            4,
            1
        )
        .err(),
        Some(StatisticsError::WorldSeedOutOfRange(9_007_199_254_740_992))
    );
    // The standalone bootstrap refuses the same unsafe seeds, in the pairs and
    // in its own draw key.
    assert_eq!(
        cluster_bootstrap_interval(&wide, ClusteringUnit::WorldSeed, 300, 7, 40).err(),
        Some(StatisticsError::WorldSeedOutOfRange(9_007_199_254_740_993))
    );
    assert_eq!(
        cluster_bootstrap_interval(&pairs, ClusteringUnit::WorldSeed, 300, 1 << 53, 40).err(),
        Some(StatisticsError::BootstrapSeedOutOfRange(1 << 53))
    );
    // The manifest must be a valid record before anything is read from it.
    let mut wrong_schema = recorded(&frozen, rates.clone(), &pairs);
    wrong_schema.schema = "eval-manifest/v1".to_string();
    assert!(matches!(
        analyze(&wrong_schema, &family, &pairs),
        Err(StatisticsError::InvalidManifest(
            ManifestError::SchemaMismatch { .. }
        ))
    ));
    let mut unordered = recorded(&frozen, rates.clone(), &pairs);
    unordered.sample_order.pop();
    assert!(matches!(
        analyze(&unordered, &family, &pairs),
        Err(StatisticsError::InvalidManifest(_))
    ));
    // Only a completed run's outcomes are evidence.
    for status in [
        RunStatus::Incomplete,
        RunStatus::Refused,
        RunStatus::Blocked,
    ] {
        let mut unfinished = recorded(&frozen, rates.clone(), &pairs);
        unfinished.status = status;
        assert_eq!(
            analyze(&unfinished, &family, &pairs).err(),
            Some(StatisticsError::RunNotCompleted(status))
        );
    }
    // A projection that leaves the safe range reports the overflow, not a
    // disagreement with the recorded values.
    let mut vast = family.clone();
    vast.families = vec!["a".into(), "b".into()];
    vast.icc_pilot = IccPilot {
        families: vec!["a".into(), "b".into()],
        n_items: u32::MAX,
        n_families: 2,
        n_worlds: 3,
        icc_family: Ratio::ZERO,
        icc_world_seed: Ratio::ZERO,
        max_affordable_worlds: u32::MAX,
        effective_n_at_max: Ratio::ONE,
        required_n_for_margin: 1,
        ..family.icc_pilot.clone()
    };
    vast.stopping_rule = StoppingRule::FixedN { pairs: 1 };
    assert_eq!(vast.validate(), Err(StatisticsError::RationalOverflow));
    // Effective N is the smaller of the two nesting levels' deflations: three
    // families of two internally constant worlds have a family ICC of 1/9 and a
    // world ICC of one, and six two-item worlds support six items, not nine.
    let nested: Vec<PilotObservation> = [("a", [0, 0]), ("b", [0, 1]), ("c", [0, 1])]
        .iter()
        .flat_map(|(family, worlds)| {
            worlds.iter().enumerate().flat_map(move |(seed, value)| {
                ["t", "u"]
                    .into_iter()
                    .map(move |task| observation(family, seed as u64, task, *value))
            })
        })
        .collect();
    let two_level = run_icc_pilot("p", &nested, 6, 8).unwrap();
    assert_eq!(
        (
            two_level.icc_family,
            two_level.icc_world_seed,
            two_level.clustering_unit
        ),
        (ratio(1, 9), Ratio::ONE, ClusteringUnit::Family)
    );
    assert_eq!(two_level.effective_n_at_max, ratio(6, 1));
    // The bootstrap draws once per replicate per cluster; a plan or a table whose
    // draws exceed the budget is refused instead of run without bound.
    let mut vast_plan = family.clone();
    vast_plan.icc_pilot.max_affordable_worlds = 100_000;
    vast_plan.icc_pilot.effective_n_at_max = ratio(250_000, 1);
    vast_plan.bootstrap_replicates = MAX_BOOTSTRAP_REPLICATES;
    // A 300-pair table has at most 300 clusters, so this plan draws 3,000,000.
    assert_eq!(vast_plan.validate(), Ok(()));
    vast_plan.stopping_rule = StoppingRule::FixedN { pairs: 100_000 };
    assert_eq!(
        vast_plan.validate(),
        Err(StatisticsError::TooManyDraws(1_000_000_000))
    );
    // Under the family unit the bootstrap has at most `n_families` clusters, so
    // the same pairs and worlds over two families draw 20,000 and validate.
    let mut by_family = vast_plan.clone();
    by_family.families = vec!["a".into(), "b".into()];
    by_family.icc_pilot = IccPilot {
        families: vec!["a".into(), "b".into()],
        n_items: 300,
        n_families: 2,
        n_worlds: 100,
        icc_family: ratio(1, 10),
        icc_world_seed: Ratio::ZERO,
        clustering_unit: ClusteringUnit::Family,
        max_affordable_worlds: 100_000,
        effective_n_at_max: ratio(1_000_000, 50_003),
        required_n_for_margin: 1,
        ..by_family.icc_pilot.clone()
    };
    assert_eq!(by_family.validate(), Ok(()));
    let many_worlds: Vec<PairOutcome> = (0..501)
        .map(|i| {
            pair(
                &format!("w{i}"),
                "cargo",
                i,
                ArmResult::Pass,
                ArmResult::Pass,
            )
        })
        .collect();
    assert_eq!(
        cluster_bootstrap_interval(
            &many_worlds,
            ClusteringUnit::WorldSeed,
            300,
            7,
            MAX_BOOTSTRAP_REPLICATES
        )
        .err(),
        Some(StatisticsError::TooManyDraws(5_010_000))
    );
    // A required N of zero is no power target; the plan is refused before it freezes.
    let mut no_target = family.clone();
    no_target.icc_pilot.required_n_for_margin = 0;
    assert_eq!(
        no_target.validate(),
        Err(StatisticsError::PilotInconsistent)
    );
    // The standalone bootstrap refuses repeated pair ids like `analyze` does.
    let mut two_worlds = copies.clone();
    for pair in &mut two_worlds[150..] {
        pair.cluster.world_seed = 1;
    }
    assert_eq!(
        cluster_bootstrap_interval(&two_worlds, ClusteringUnit::WorldSeed, 300, 7, 40).err(),
        Some(StatisticsError::DuplicatePair {
            pair_id: "cargo-0".to_string()
        })
    );
    // A plan of zero pairs computes no gate and is refused before it freezes.
    let mut empty_plan = family.clone();
    empty_plan.stopping_rule = StoppingRule::FixedN { pairs: 0 };
    assert_eq!(empty_plan.validate(), Err(StatisticsError::NoPairs));
    // The rate helpers refuse counts past the safe range instead of panicking.
    assert_eq!(
        PairCounts {
            n: u64::MAX,
            b: u64::MAX - 1,
            ..PairCounts::default()
        }
        .harm()
        .err(),
        Some(StatisticsError::InconsistentCounts)
    );
    // One score per task per world: a repeated observation is not another item.
    let mut repeated = honest_observations.to_vec();
    repeated.push(observation("tokio", 1, "b", 9));
    assert_eq!(
        run_icc_pilot("p", &repeated, 4, 1).err(),
        Some(StatisticsError::DuplicateObservation {
            cluster: key("tokio", 1),
            task: "b".to_string()
        })
    );
    // Arm rates past one are not rates, whichever field carries them.
    assert_eq!(
        arm_miss_asymmetry(&arm_rates("2", "2")).err(),
        Some(StatisticsError::RateOutOfRange {
            field: "arm_rates.miss_rate"
        })
    );
    let mut refusing = arm_rates("0", "0");
    refusing.get_mut("aged").unwrap().refusal_rate = "2".to_string();
    assert_eq!(
        arm_miss_asymmetry(&refusing).err(),
        Some(StatisticsError::RateOutOfRange {
            field: "arm_rates.refusal_rate"
        })
    );
    // Through `analyze` the manifest's own validation refuses it first.
    assert!(matches!(
        analyze(
            &recorded(&frozen, refusing.clone(), &pairs),
            &family,
            &pairs
        ),
        Err(StatisticsError::InvalidManifest(
            ManifestError::RateOutOfRange { .. }
        ))
    ));
    // Hand-built counts that no pair table produces are refused before any rate.
    let profile_rates = profile().rates().unwrap();
    for counts in [
        PairCounts {
            n: 1,
            b: 2,
            c: 0,
            aged_pass: 2,
            fresh_censored: 0,
            aged_censored: 0,
        },
        PairCounts {
            n: 4,
            b: 2,
            c: 3,
            aged_pass: 0,
            fresh_censored: 0,
            aged_censored: 0,
        },
        PairCounts {
            n: u64::MAX,
            b: 0,
            c: 0,
            aged_pass: 0,
            fresh_censored: 0,
            aged_censored: 0,
        },
        // `b` excludes an aged pass; `c` requires one; a censored aged arm is not a pass.
        PairCounts {
            n: 1,
            b: 1,
            c: 0,
            aged_pass: 1,
            fresh_censored: 0,
            aged_censored: 0,
        },
        PairCounts {
            n: 2,
            b: 0,
            c: 1,
            aged_pass: 0,
            fresh_censored: 0,
            aged_censored: 0,
        },
        PairCounts {
            n: 2,
            b: 0,
            c: 0,
            aged_pass: 2,
            fresh_censored: 0,
            aged_censored: 1,
        },
        // A censored fresh arm is in `b` or beside an aged pass.
        PairCounts {
            n: 1,
            b: 0,
            c: 0,
            aged_pass: 0,
            fresh_censored: 1,
            aged_censored: 0,
        },
    ] {
        assert_eq!(
            Gates::of(&counts, &profile_rates).err(),
            Some(StatisticsError::InconsistentCounts),
            "{counts:?}"
        );
    }
    // The minimum i128 is refused rather than wrapped into a wrong ratio.
    assert_eq!(
        Ratio::try_new(i128::MIN, 1).err(),
        Some(StatisticsError::RationalOverflow)
    );
    assert_eq!(
        Ratio::try_new(1, i128::MIN).err(),
        Some(StatisticsError::RationalOverflow)
    );
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
