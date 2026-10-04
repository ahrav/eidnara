use eval_core::{
    ArtifactIdentity, BoundaryState, DriverIdentity, Histogram, HostManifest, OpenLoopCounts,
    PassOutcome, PassRow, PercentileBound, Ratio, RefusalRate, RefusalReason, ScaleHarness,
    ScaleInputs, ScaleReport, ScaleReportError, ScaleTier, TierRatio, bucket_index,
    bucket_upper_bound, parse_pass_row, parse_scale_report,
};
use serde_json::{Value, json};

fn ratio(numerator: i64, denominator: u64) -> Ratio {
    Ratio::try_new(i128::from(numerator), i128::from(denominator)).unwrap()
}

fn row(tier: ScaleTier, session: &str, turn: u32, response_us: u64) -> PassRow {
    PassRow {
        harness: ScaleHarness::Opencode,
        tier,
        session: session.to_string(),
        turn,
        boundary_state: BoundaryState::Steady,
        outcome: PassOutcome::Completed,
        refusal: None,
        response_us,
        service_us: Some(response_us / 2),
        rss_bytes: 100_000_000,
        ipc_bytes: 4_096,
    }
}

/// `passes` steady passes per session, every pass of a session at its one value.
fn constant_sessions(tier: ScaleTier, values: &[u64], passes: u32) -> Vec<PassRow> {
    values
        .iter()
        .enumerate()
        .flat_map(|(index, value)| {
            let session = format!("{tier:?}-{index}");
            (0..passes).map(move |turn| row(tier, &session, turn, *value))
        })
        .collect()
}

fn inputs(rows: Vec<PassRow>) -> ScaleInputs {
    ScaleInputs {
        driver: DriverIdentity {
            name: "opencode-pass".to_string(),
            budget_seconds: 1_800,
        },
        artifact: ArtifactIdentity {
            commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            bun_version: "1.3.14".to_string(),
            daemon_build: "release".to_string(),
        },
        host: HostManifest {
            cpu_model: "AMD EPYC 7763 64-Core Processor".to_string(),
            core_count: 4,
            memory_bytes: 16 * 1024 * 1024 * 1024,
            kernel: "6.8.0".to_string(),
            glibc: "2.39".to_string(),
            disk: "ssd".to_string(),
        },
        open_loop: Some(OpenLoopCounts {
            offered: 10,
            sent: 10,
            completed: 9,
        }),
        seed: 825,
        rows,
    }
}

/// Three 10k control sessions at 1000 us and three 1M sessions at 1100, 1200, and 1300 us.
fn powered_report() -> ScaleReport {
    let mut rows = constant_sessions(ScaleTier::Control10k, &[1_000, 1_000, 1_000], 299);
    rows.extend(constant_sessions(
        ScaleTier::S4_1m,
        &[1_100, 1_200, 1_300],
        299,
    ));
    ScaleReport::build(inputs(rows)).unwrap()
}

fn tier(report: &ScaleReport, tier: ScaleTier) -> &eval_core::TierSummary {
    report
        .tiers
        .iter()
        .find(|summary| summary.tier == tier)
        .unwrap()
}

#[test]
fn a_report_round_trips_losslessly() {
    let report = powered_report();
    let value = report.serialize().unwrap();
    assert_eq!(parse_scale_report(&value).unwrap(), report);
}

#[test]
fn the_block_bootstrap_ratio_matches_the_hand_computed_interval() {
    let report = powered_report();
    let TierRatio::Computed(claim) = &tier(&report, ScaleTier::S4_1m).ratio else {
        panic!("three powered sessions per tier compute a ratio");
    };
    // Every pass of a session shares its value, so a replicate's pooled p99 is the largest
    // value among the sessions it drew, and the control's p99 is always 1000. The pooled p99
    // of all three sessions is 1300; the 1000 replicates draw the 1300 session in about 70% of
    // replicates and only 1100 sessions in about 3.7%, so the 975th smallest is 13/10 and the
    // 25th smallest is 11/10.
    assert_eq!(claim.point, ratio(13, 10));
    assert_eq!(claim.lower, ratio(11, 10));
    assert_eq!(claim.upper, ratio(13, 10));
    assert_eq!(claim.gate, ratio(6, 5));
    assert!(!claim.passes);
    assert_eq!(
        (
            claim.sessions,
            claim.control_sessions,
            claim.min_steady_uncensored
        ),
        (3, 3, 299)
    );
    assert_eq!(
        tier(&report, ScaleTier::Control10k).ratio,
        TierRatio::Control
    );
}

#[test]
fn censored_percentiles_match_the_hand_computed_fixture() {
    // Ten passes: nine completed at 1..=9 us and one censored at its 10 us budget.
    let mut rows: Vec<PassRow> = (1..=9)
        .map(|value| row(ScaleTier::Control10k, "c", value, u64::from(value)))
        .collect();
    let mut cut = row(ScaleTier::Control10k, "c", 10, 10);
    cut.outcome = PassOutcome::Censored;
    rows.push(cut);
    let report = ScaleReport::build(inputs(rows)).unwrap();
    let pooled = &tier(&report, ScaleTier::Control10k).response_percentiles;
    // p50: rank 5 of 10 is 5, with five completions at or below it.
    assert_eq!((pooled[0].p, pooled[0].value), (50, 5));
    assert_eq!(pooled[0].bound, PercentileBound::Point);
    // p95: rank 10 is the censored pass, so 10 is only a lower bound.
    assert_eq!((pooled[1].p, pooled[1].value), (95, 10));
    assert_eq!(pooled[1].bound, PercentileBound::Lower);
    assert_eq!((pooled[1].n, pooled[1].censored), (10, 1));
    // No p99 below 299 passes.
    assert_eq!(pooled.len(), 2);
}

#[test]
fn histogram_merge_is_associative_and_order_independent() {
    let a = Histogram::of(&[(1, false), (40, false), (1_000, false), (1_000_000, true)]);
    let b = Histogram::of(&[(40, false), (41, false), (5_000_000, false)]);
    let c = Histogram::of(&[(0, false), (1_000, false), (77, true)]);
    let left = a.merge(&b).merge(&c);
    assert_eq!(left, a.merge(&b.merge(&c)));
    assert_eq!(left, c.merge(&a).merge(&b));
    assert_eq!(left, b.merge(&c).merge(&a));
    assert_eq!(left.count(), 10);
    assert_eq!(left.censored, 2);
    for value in [
        0,
        1,
        31,
        32,
        33,
        63,
        64,
        1_000,
        123_456,
        u64::from(u32::MAX),
        1 << 50,
    ] {
        let index = bucket_index(value);
        let upper = bucket_upper_bound(index);
        assert!(upper >= value, "{value}");
        assert!(
            upper - value <= value / 32,
            "{value} in a bucket wider than 1/32"
        );
        if index > 0 {
            assert!(bucket_upper_bound(index - 1) < value, "{value}");
        }
    }
}

fn mutate(report: &ScaleReport, edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = report.serialize().unwrap();
    edit(&mut value);
    value
}

#[test]
fn parse_names_a_missing_field_and_refuses_lossy_and_out_of_range_values() {
    let report = powered_report();
    let missing = mutate(&report, |value| {
        value["rows"][0].as_object_mut().unwrap().remove("tier");
    });
    assert_eq!(
        parse_scale_report(&missing),
        Err(ScaleReportError::MissingField {
            field: "tier".to_string()
        })
    );
    let host = mutate(&report, |value| {
        value["host"].as_object_mut().unwrap().remove("glibc");
    });
    assert_eq!(
        parse_scale_report(&host),
        Err(ScaleReportError::MissingField {
            field: "glibc".to_string()
        })
    );
    // An absent optional reads as `null` and would not round-trip byte for byte.
    let lossy = mutate(&report, |value| {
        value["rows"][0].as_object_mut().unwrap().remove("refusal");
    });
    assert_eq!(parse_scale_report(&lossy), Err(ScaleReportError::Lossy));
    let fraction = mutate(&report, |value| {
        value["rows"][0]["response_us"] = json!(1.5);
    });
    assert!(matches!(
        parse_scale_report(&fraction),
        Err(ScaleReportError::Shape(_))
    ));
    let huge = mutate(&report, |value| {
        value["rows"][0]["rss_bytes"] = json!(1u64 << 60);
    });
    assert!(matches!(
        parse_scale_report(&huge),
        Err(ScaleReportError::NotCanonical(_))
    ));
}

#[test]
fn parse_refuses_a_percentile_averaged_across_sessions() {
    let mut rows = constant_sessions(ScaleTier::Control10k, &[1_000, 3_000], 10);
    rows.extend(constant_sessions(ScaleTier::S3_100k, &[1_000], 10));
    let report = ScaleReport::build(inputs(rows)).unwrap();
    // The pooled p50 of 1000 and 3000 sessions is 1000; the mean of their p50s is 2000.
    assert_eq!(
        tier(&report, ScaleTier::Control10k).response_percentiles[0].value,
        1_000
    );
    let averaged = mutate(&report, |value| {
        let tiers = value["tiers"].as_array_mut().unwrap();
        let control = tiers.iter_mut().find(|tier| tier["tier"] == "10k").unwrap();
        control["response_percentiles"][0]["value"] = json!(2_000);
    });
    assert_eq!(
        parse_scale_report(&averaged),
        Err(ScaleReportError::PercentileNotPooled {
            harness: ScaleHarness::Opencode,
            tier: ScaleTier::Control10k,
        })
    );
}

#[test]
fn parse_refuses_an_underpowered_ratio_claim() {
    let report = powered_report();
    let TierRatio::Computed(claim) = tier(&report, ScaleTier::S4_1m).ratio.clone() else {
        panic!("powered report computes a ratio");
    };
    for (field, low) in [
        ("sessions", 2),
        ("control_sessions", 2),
        ("min_steady_uncensored", 298),
    ] {
        let tampered = mutate(&report, |value| {
            let tiers = value["tiers"].as_array_mut().unwrap();
            let large = tiers
                .iter_mut()
                .find(|tier| tier["tier"] == "s4_1m")
                .unwrap();
            large["ratio"][field] = json!(low);
        });
        assert!(
            matches!(
                parse_scale_report(&tampered),
                Err(ScaleReportError::RatioUnderpowered { .. })
            ),
            "{field}"
        );
    }

    // Built from two sessions per tier, or 298 passes per session, the ratio is withheld.
    for (sessions, passes) in [(2, 299), (3, 298)] {
        let mut rows = constant_sessions(ScaleTier::Control10k, &vec![1_000; sessions], passes);
        rows.extend(constant_sessions(
            ScaleTier::S4_1m,
            &vec![1_100; sessions],
            passes,
        ));
        let report = ScaleReport::build(inputs(rows)).unwrap();
        assert!(matches!(
            tier(&report, ScaleTier::S4_1m).ratio,
            TierRatio::Withheld(_)
        ));
        let claimed = mutate(&report, |value| {
            let tiers = value["tiers"].as_array_mut().unwrap();
            let large = tiers
                .iter_mut()
                .find(|tier| tier["tier"] == "s4_1m")
                .unwrap();
            let mut forged = serde_json::to_value(&claim).unwrap();
            forged["sessions"] = json!(sessions);
            forged["control_sessions"] = json!(sessions);
            forged["min_steady_uncensored"] = json!(passes);
            forged["kind"] = json!("computed");
            large["ratio"] = forged;
        });
        assert!(matches!(
            parse_scale_report(&claimed),
            Err(ScaleReportError::RatioUnderpowered { .. })
        ));
    }
}

#[test]
fn zero_refusals_are_a_rule_of_three_bound() {
    let report = powered_report();
    let refusals = &tier(&report, ScaleTier::S4_1m).refusals;
    assert_eq!(
        *refusals,
        RefusalRate::Bound {
            upper_bound_95: ratio(3, 897),
            bound_method: eval_core::BoundMethod::RuleOfThree,
            n: 897,
        }
    );
    let observed_zero = mutate(&report, |value| {
        value["tiers"][0]["refusals"] = json!({
            "evidence_kind": "observed",
            "rate": { "numerator": 0, "denominator": 1 },
            "upper_bound_95": { "numerator": 0, "denominator": 1 },
            "bound_method": "poisson_envelope",
            "n": 897,
        });
    });
    assert!(matches!(
        parse_scale_report(&observed_zero),
        Err(ScaleReportError::ZeroRefusalNotBounded { .. })
    ));

    let mut rows = constant_sessions(ScaleTier::Control10k, &[1_000], 10);
    rows[3].outcome = PassOutcome::Refused;
    rows[3].refusal = Some(RefusalReason::Declined);
    let report = ScaleReport::build(inputs(rows)).unwrap();
    assert!(matches!(
        tier(&report, ScaleTier::Control10k).refusals,
        RefusalRate::Observed { .. }
    ));
}

#[test]
fn the_result_digest_excludes_latency_rss_and_bytes() {
    let rows = constant_sessions(ScaleTier::Control10k, &[1_000, 1_100], 5);
    let base = ScaleReport::build(inputs(rows.clone())).unwrap();
    let mut measured = rows.clone();
    for row in &mut measured {
        row.response_us *= 3;
        row.service_us = row.service_us.map(|service| service + 7);
        row.rss_bytes += 1;
        row.ipc_bytes += 1;
    }
    let remeasured = ScaleReport::build(inputs(measured)).unwrap();
    let digest = |report: &ScaleReport| ScaleReport::result_digest(&report.serialize().unwrap());
    assert_eq!(digest(&base).unwrap(), digest(&remeasured).unwrap());

    let mut refused = rows;
    refused[0].outcome = PassOutcome::Refused;
    refused[0].refusal = Some(RefusalReason::DaemonError);
    let changed = ScaleReport::build(inputs(refused)).unwrap();
    assert_ne!(digest(&base).unwrap(), digest(&changed).unwrap());
}

#[test]
fn flatness_reports_the_exact_rss_drift_over_the_steady_span() {
    let mut rows: Vec<PassRow> = (0..=100)
        .map(|turn| {
            let mut pass = row(ScaleTier::S3_100k, "flat", turn, 1_000);
            // 1,000 bytes per turn over 100 turns: 100,000 bytes on a 100 MB entry.
            pass.rss_bytes = 100_000_000 + 1_000 * u64::from(turn);
            pass
        })
        .collect();
    rows[0].boundary_state = BoundaryState::Warming;
    let report = ScaleReport::build(inputs(rows)).unwrap();
    let flatness = report.sessions[0].flatness.clone().unwrap();
    assert_eq!(flatness.steady_rows, 100);
    assert_eq!(flatness.span_turns, 99);
    assert_eq!(flatness.entry_rss_bytes, 100_001_000);
    assert_eq!(flatness.drift_bytes, 99_000);
    assert!(flatness.passes);

    let growing: Vec<PassRow> = (0..10)
        .map(|turn| {
            let mut pass = row(ScaleTier::S3_100k, "grow", turn, 1_000);
            pass.rss_bytes = 1_000 + 100 * u64::from(turn);
            pass
        })
        .collect();
    let report = ScaleReport::build(inputs(growing)).unwrap();
    assert!(!report.sessions[0].flatness.as_ref().unwrap().passes);
}

#[test]
fn build_refuses_inconsistent_inputs() {
    let mut bad = inputs(constant_sessions(ScaleTier::Control10k, &[1_000], 2));
    bad.open_loop = Some(OpenLoopCounts {
        offered: 1,
        sent: 2,
        completed: 0,
    });
    assert!(matches!(
        ScaleReport::build(bad),
        Err(ScaleReportError::OpenLoopInconsistent(_))
    ));
    let mut rows = constant_sessions(ScaleTier::Control10k, &[1_000], 2);
    rows[1].turn = 0;
    assert!(matches!(
        ScaleReport::build(inputs(rows)),
        Err(ScaleReportError::DuplicateTurn { .. })
    ));
    let mut rows = constant_sessions(ScaleTier::Control10k, &[1_000], 2);
    rows[1].refusal = Some(RefusalReason::Declined);
    assert_eq!(
        ScaleReport::build(inputs(rows)),
        Err(ScaleReportError::RefusalOutcomeMismatch { row: 1 })
    );
    let mut commit = inputs(Vec::new());
    commit.artifact.commit = "HEAD".to_string();
    assert_eq!(
        ScaleReport::build(commit),
        Err(ScaleReportError::MalformedCommit)
    );
}

/// The TypeScript row writer's output for its fixed fixture, committed beside its source
/// (`packages/e2e-tests/src/scale-report/rows.test.ts` writes the same bytes).
const WRITER_ROWS: &str = include_str!("../testdata/scale/writer-rows.jsonl");

#[test]
fn rows_the_typescript_writer_emits_parse_losslessly() {
    let rows: Vec<PassRow> = WRITER_ROWS
        .lines()
        .map(|line| parse_pass_row(&serde_json::from_str(line).unwrap()).unwrap())
        .collect();
    assert!(rows.len() >= 4);
    for (line, row) in WRITER_ROWS.lines().zip(&rows) {
        assert_eq!(serde_json::to_string(row).unwrap(), line);
    }
    assert!(rows.iter().any(|row| row.outcome == PassOutcome::Censored));
    assert!(rows.iter().any(|row| row.refusal.is_some()));
    assert!(rows.iter().any(|row| row.service_us.is_none()));
    assert!(rows.iter().any(|row| row.harness == ScaleHarness::Pi));
    ScaleReport::build(inputs(rows)).unwrap();
}
