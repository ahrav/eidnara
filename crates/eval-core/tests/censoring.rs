use eval_core::{
    ArmResult, Attempt, BoundMethod, CensorReason, ClusteringUnit, Counter, FailureRate,
    LatencySummary, P99_MIN_RUNS, PassKBounds, Percentile, PercentileBound, Ratio, StatisticsError,
    pass_k,
};
use serde_json::{Value, json};

fn ratio(numerator: i64, denominator: u64) -> Ratio {
    Ratio::new(numerator, denominator)
}

fn completed(duration_ms: u64) -> Attempt {
    Attempt {
        duration_ms,
        censored: None,
    }
}

fn censored(duration_ms: u64, reason: CensorReason) -> Attempt {
    Attempt {
        duration_ms,
        censored: Some(reason),
    }
}

fn timed_out(deadline_ms: u64) -> Attempt {
    censored(deadline_ms, CensorReason::Timeout)
}

fn counter(n: u64, failures: u64) -> Counter {
    Counter {
        n,
        failures,
        unit: ClusteringUnit::WorldSeed,
    }
}

fn percentile(summary: &LatencySummary, p: u8) -> &Percentile {
    summary.percentiles.iter().find(|x| x.p == p).unwrap()
}

#[test]
fn the_frozen_reference_agrees_on_every_censored_case() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/statistics-golden.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut seen = 0;
    for case in golden["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let expected = &case["expected"];
        let actual = match case["kind"].as_str().unwrap() {
            "latency" => {
                let attempts: Vec<Attempt> =
                    serde_json::from_value(case["input"]["attempts"].clone()).unwrap();
                serde_json::to_value(LatencySummary::of(&attempts)).unwrap()
            }
            "counter" => {
                let counter: Counter = serde_json::from_value(case["input"].clone()).unwrap();
                serde_json::to_value(counter.rate().unwrap()).unwrap()
            }
            "pass_k" => {
                let attempts: Vec<ArmResult> =
                    serde_json::from_value(case["input"]["attempts"].clone()).unwrap();
                let k = case["input"]["k"].as_u64().unwrap() as u32;
                serde_json::to_value(pass_k(&attempts, k).unwrap()).unwrap()
            }
            _ => continue,
        };
        assert_eq!(&actual, expected, "{id}");
        seen += 1;
    }
    assert_eq!(seen, 16);
}

#[test]
fn timeouts_stay_in_every_denominator_and_percentiles_carry_their_counts() {
    let mut attempts: Vec<Attempt> = (0..100).map(|_| completed(10)).collect();
    attempts.extend((0..5).map(|_| timed_out(1_000)));
    let summary = LatencySummary::of(&attempts);
    assert_eq!((summary.n, summary.censored), (105, 5));
    assert_eq!(
        summary.percentiles,
        vec![
            Percentile {
                p: 50,
                value: 10,
                n: 105,
                censored: 5,
                bound: PercentileBound::Point
            },
            Percentile {
                p: 95,
                value: 10,
                n: 105,
                censored: 5,
                bound: PercentileBound::Point
            },
        ],
        "no p99 below {P99_MIN_RUNS} runs"
    );

    // With more censoring the 95th rank lands inside the censored tail: a lower bound at the deadline.
    let mut heavy: Vec<Attempt> = (0..90).map(|_| completed(10)).collect();
    heavy.extend((0..15).map(|_| timed_out(1_000)));
    let p95 = percentile(&LatencySummary::of(&heavy), 95).clone();
    assert_eq!((p95.value, p95.bound), (1_000, PercentileBound::Lower));

    // A censored attempt below the rank could have displaced the order statistic, so a completion
    // past it is still only a lower bound; a completion below every censored attempt is a point.
    let mixed = LatencySummary::of(&[completed(10), timed_out(500), completed(900)]);
    assert_eq!(
        (percentile(&mixed, 50).value, percentile(&mixed, 50).bound),
        (500, PercentileBound::Lower)
    );
    assert_eq!(
        (percentile(&mixed, 95).value, percentile(&mixed, 95).bound),
        (900, PercentileBound::Lower)
    );
    let early = LatencySummary::of(&[completed(10), completed(20), timed_out(500)]);
    assert_eq!(
        (percentile(&early, 50).value, percentile(&early, 50).bound),
        (20, PercentileBound::Point)
    );
    // A censored attempt below the rank cannot move the order statistic when enough completions
    // tie at the picked value: however long the censored attempt really ran, the median is 2.
    let tied_over = LatencySummary::of(&[timed_out(1), completed(2), completed(2)]);
    assert_eq!(
        (
            percentile(&tied_over, 50).value,
            percentile(&tied_over, 50).bound
        ),
        (2, PercentileBound::Point)
    );
    // A censored attempt sorts after a completed one of equal duration, on either input order.
    for tie in [
        [completed(500), timed_out(500)],
        [timed_out(500), completed(500)],
    ] {
        let p50 = percentile(&LatencySummary::of(&tie), 50).clone();
        assert_eq!((p50.value, p50.bound), (500, PercentileBound::Point));
    }
    let single = LatencySummary::of(&[timed_out(250)]);
    assert_eq!((single.n, single.censored), (1, 1));
    assert_eq!(percentile(&single, 95).bound, PercentileBound::Lower);

    let exactly: Vec<Attempt> = (0..P99_MIN_RUNS as u64).map(completed).collect();
    let p99 = percentile(&LatencySummary::of(&exactly), 99).clone();
    assert_eq!((p99.n, p99.value), (299, 296));
    assert!(
        LatencySummary::of(&exactly[..298])
            .percentiles
            .iter()
            .all(|x| x.p != 99)
    );
    assert_eq!(LatencySummary::of(&[]).percentiles, vec![]);

    // The timeout and every one of the six budgets is its own censoring reason, and each stays in
    // the distribution at its own censoring point.
    let reasons = [
        ("timeout", CensorReason::Timeout),
        ("max_model_calls", CensorReason::MaxModelCalls),
        ("max_tool_calls", CensorReason::MaxToolCalls),
        ("max_tokens_in", CensorReason::MaxTokensIn),
        ("max_tokens_out", CensorReason::MaxTokensOut),
        ("hard_deadline_ms", CensorReason::HardDeadlineMs),
        (
            "max_no_progress_iterations",
            CensorReason::MaxNoProgressIterations,
        ),
    ];
    let mut budgets = Vec::new();
    for (index, (wire, reason)) in reasons.iter().enumerate() {
        let attempt: Attempt = serde_json::from_value(
            json!({"duration_ms": 100 * (index as u64 + 1), "censored": wire}),
        )
        .unwrap();
        assert_eq!(attempt.censored, Some(*reason), "{wire}");
        budgets.push(attempt);
    }
    budgets.push(completed(50));
    let summary = LatencySummary::of(&budgets);
    assert_eq!((summary.n, summary.censored), (8, 7));
    assert_eq!(
        (
            percentile(&summary, 50).value,
            percentile(&summary, 50).bound
        ),
        (300, PercentileBound::Lower)
    );
    let mut extra = json!({"duration_ms": 7, "censored": null});
    extra["deadline_ms"] = json!(1);
    assert!(serde_json::from_value::<Attempt>(extra).is_err());
    assert_eq!(
        serde_json::to_value(PercentileBound::Point).unwrap(),
        json!("point")
    );
}

#[test]
fn zero_failures_is_a_bound_never_a_proof() {
    let bound = |n, upper| FailureRate::Bound {
        upper_bound_95: upper,
        bound_method: BoundMethod::RuleOfThree,
        n,
        unit: ClusteringUnit::WorldSeed,
    };
    assert_eq!(counter(400, 0).rate().unwrap(), bound(400, ratio(3, 400)));
    assert_eq!(counter(20, 0).rate().unwrap(), bound(20, ratio(3, 20)));
    // Sixty stall-free schedules cannot rule out one stall in twenty; a 1/100 gate reads 1/20.
    assert_eq!(counter(60, 0).rate().unwrap(), bound(60, ratio(1, 20)));
    // Below three trials the rule would exceed one; a rate is at most one.
    assert_eq!(counter(2, 0).rate().unwrap(), bound(2, Ratio::ONE));
    // An observed rate carries a bound on the same scale as the zero-failure case, so a gate
    // never compares a bound in one branch with a point estimate in the other:
    // (2 * failures + 3) / n envelopes the one-sided 95 percent limit.
    assert_eq!(
        counter(60, 3).rate().unwrap(),
        FailureRate::Observed {
            rate: ratio(1, 20),
            upper_bound_95: ratio(3, 20),
            bound_method: BoundMethod::PoissonEnvelope,
            n: 60,
            unit: ClusteringUnit::WorldSeed,
        }
    );
    assert_eq!(
        counter(60, 1).rate().unwrap(),
        FailureRate::Observed {
            rate: ratio(1, 60),
            upper_bound_95: ratio(1, 12),
            bound_method: BoundMethod::PoissonEnvelope,
            n: 60,
            unit: ClusteringUnit::WorldSeed,
        }
    );
    // The envelope caps at one like the rule of three does.
    assert_eq!(counter(2, 1).rate().unwrap().upper_bound_95(), Ratio::ONE);
    // Strictly worse evidence never reads as a smaller bound: one failure in sixty must not
    // pass a 1/30 gate that zero failures in sixty fails.
    let gate = ratio(1, 30);
    let bounds: Vec<Ratio> = (0..=5)
        .map(|failures| counter(60, failures).rate().unwrap().upper_bound_95())
        .collect();
    assert!(
        bounds.windows(2).all(|pair| pair[0] < pair[1]),
        "{bounds:?}"
    );
    assert!(bounds[0] > gate && bounds[1] > gate);
    assert_eq!(
        counter(0, 0).rate().err(),
        Some(StatisticsError::MalformedCounter { n: 0, failures: 0 })
    );
    assert_eq!(
        counter(5, 6).rate().err(),
        Some(StatisticsError::MalformedCounter { n: 5, failures: 6 })
    );
    let rendered = serde_json::to_value(counter(400, 0).rate().unwrap()).unwrap();
    assert_eq!(rendered["evidence_kind"], json!("bound"));
    assert_eq!(rendered["unit"], json!("world_seed"));
    assert!(
        rendered.get("rate").is_none(),
        "a bound never renders as a rate"
    );
    assert!(rendered.get("proven").is_none());
    let observed = serde_json::to_value(counter(60, 1).rate().unwrap()).unwrap();
    assert_eq!(observed["evidence_kind"], json!("observed"));
    assert_eq!(observed["bound_method"], json!("poisson_envelope"));
    assert_eq!(
        observed["upper_bound_95"],
        json!({"numerator": 1, "denominator": 12})
    );
}

#[test]
fn pass_k_bounds_resolve_censoring_both_ways_and_are_indeterminate_when_all_are_censored() {
    use ArmResult::{Fail, Pass};
    let clean = pass_k(&[Pass, Pass, Fail, Pass, Pass], 3).unwrap();
    assert_eq!(
        (clean.k, clean.repeats, clean.uncensored_repeats),
        (3, 5, 5)
    );
    assert_eq!(clean.pass_at_1, ratio(4, 5));
    assert_eq!(clean.censoring_rate, Ratio::ZERO);
    // C(4,3)/C(5,3) = 4/10 under both conventions when nothing is censored.
    assert_eq!(
        clean.pass_k,
        PassKBounds::Bounds {
            censored_as_fail: ratio(2, 5),
            censored_excluded: ratio(2, 5),
        }
    );

    let mixed = pass_k(
        &[
            Pass,
            Pass,
            Pass,
            Fail,
            ArmResult::Censored(CensorReason::MaxTokensOut),
        ],
        3,
    )
    .unwrap();
    assert_eq!(mixed.pass_at_1, ratio(3, 5), "censored is not a pass");
    assert_eq!(
        (mixed.censoring_rate, mixed.uncensored_repeats),
        (ratio(1, 5), 4)
    );
    // Censored as fail: C(3,3)/C(5,3) = 1/10. Censored excluded: C(3,3)/C(4,3) = 1/4.
    assert_eq!(
        mixed.pass_k,
        PassKBounds::Bounds {
            censored_as_fail: ratio(1, 10),
            censored_excluded: ratio(1, 4),
        }
    );

    let few = pass_k(
        &[
            Pass,
            ArmResult::Censored(CensorReason::Timeout),
            ArmResult::Censored(CensorReason::Timeout),
        ],
        3,
    )
    .unwrap();
    assert_eq!(
        few.uncensored_repeats, 1,
        "fewer than k uncensored attempts"
    );
    assert_eq!(
        few.pass_k,
        PassKBounds::Bounds {
            censored_as_fail: Ratio::ZERO,
            censored_excluded: Ratio::ONE,
        }
    );

    let all = pass_k(
        &[
            ArmResult::Censored(CensorReason::Timeout),
            ArmResult::Censored(CensorReason::HardDeadlineMs),
        ],
        2,
    )
    .unwrap();
    assert_eq!(all.pass_k, PassKBounds::Indeterminate);
    assert_eq!(
        (all.pass_at_1, all.censoring_rate),
        (Ratio::ZERO, Ratio::ONE)
    );
    assert_eq!(
        serde_json::to_value(&all.pass_k).unwrap(),
        json!({"kind": "indeterminate"})
    );

    assert_eq!(
        pass_k(&[Pass], 0).err(),
        Some(StatisticsError::MalformedTrials { k: 0, repeats: 1 })
    );
    assert_eq!(
        pass_k(&[], 3).err(),
        Some(StatisticsError::MalformedTrials { k: 3, repeats: 0 })
    );
    assert_eq!(
        pass_k(&[Pass; 3], 4).err(),
        Some(StatisticsError::MalformedTrials { k: 4, repeats: 3 }),
        "k past the repeat count"
    );
    // A binomial past the safe range refuses rather than wraps; one that reduces stays exact.
    assert_eq!(
        pass_k(&[Pass; 200], 100).err(),
        Some(StatisticsError::RationalOverflow)
    );
    // C(130,129)/C(130,129) is 1; the central coefficients on the way there are not visited.
    assert_eq!(
        pass_k(&[Pass; 130], 129).unwrap().pass_k,
        PassKBounds::Bounds {
            censored_as_fail: Ratio::ONE,
            censored_excluded: Ratio::ONE,
        }
    );
    // C(125,61)/C(126,61) = 65/126: the coefficients fit, so the walk must not overflow on the
    // way to them.
    let mut one_fail = vec![Pass; 126];
    one_fail[0] = Fail;
    assert_eq!(
        pass_k(&one_fail, 61).unwrap().pass_k,
        PassKBounds::Bounds {
            censored_as_fail: ratio(65, 126),
            censored_excluded: ratio(65, 126),
        }
    );
    // C(131,65) sits between i128::MAX and u128::MAX; the ratio reduces to 66/131 before it is
    // narrowed.
    let mut one_fail = vec![Pass; 131];
    one_fail[0] = Fail;
    assert_eq!(
        pass_k(&one_fail, 65).unwrap().pass_k,
        PassKBounds::Bounds {
            censored_as_fail: ratio(66, 131),
            censored_excluded: ratio(66, 131),
        }
    );
    let mut nearly = vec![Pass; 100];
    nearly[0] = Fail;
    assert_eq!(
        pass_k(&nearly, 20).unwrap().pass_k,
        PassKBounds::Bounds {
            censored_as_fail: ratio(4, 5),
            censored_excluded: ratio(4, 5),
        }
    );
}
