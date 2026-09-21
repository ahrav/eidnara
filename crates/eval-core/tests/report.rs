//! The Suite B report serializer: claim boundary verbatim, a suppression
//! removing the gates and nothing else, and no report claiming what its
//! class, samples, profile, or exclusions forbid.

use std::collections::BTreeMap;

use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, ArmKind, ArmRates, ArmResult,
    BaselineContrast, BaselineFailure, BlockedReason, CLAIM_BOUNDARY_SCHEMA, CampaignGates,
    CampaignProfile, Ceilings, CensorReason, ClaimBoundary, ClaimClass, Claims, ClusterKey,
    ClusteringUnit, Cut, Envelope, EnvelopeExceeded, Established, EvaluatedSurface, FrozenFamily,
    GatedBlocks, HistoryPolicy, IccPilot, IntervalMethod, LivenessBounds, MultiplicityCorrection,
    PairOutcome, PairedReport, Ratio, Reachability, ReportError, ReportOutcome, Resource,
    ResourceLimits, SUITE_B_REPORT_SCHEMA, SampleError, SampleLedger, SampleRecord, SkipReason,
    StopCondition, StoppingRule, SuiteBReport, Suppression, Terminal, WorldProvenance, analyze,
    parse_report, reachability_of,
};
use serde_json::json;

type Mutate = Box<dyn Fn(&mut SuiteBReport)>;

fn ratio(n: i64, d: u64) -> Ratio {
    Ratio::new(n, d)
}

fn family() -> AnalysisFamily {
    AnalysisFamily {
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
        item_count_threshold: 300,
        bootstrap_replicates: 40,
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
    }
}

fn arm_rates() -> BTreeMap<String, ArmRates> {
    let rate = |m: &str, r: &str| ArmRates {
        miss_rate: m.to_string(),
        refusal_rate: r.to_string(),
    };
    BTreeMap::from([
        ("aged".to_string(), rate("0.01", "0")),
        ("fresh".to_string(), rate("0.02", "0")),
    ])
}

fn pairs() -> Vec<PairOutcome> {
    (0..320)
        .map(|i| PairOutcome {
            pair_id: format!("p{i}"),
            cluster: ClusterKey {
                family: if i % 2 == 0 { "cargo" } else { "tokio" }.to_string(),
                world_seed: i % 8,
            },
            fresh: ArmResult::Pass,
            aged: if i % 50 == 0 {
                ArmResult::Fail
            } else {
                ArmResult::Pass
            },
        })
        .collect()
}

/// One sample per arm of every pair plus the ten terminals the rates test
/// reads: 640 attempted, 6 not.
fn samples() -> SampleLedger {
    let mut terminals: Vec<Terminal> = (0..640)
        .map(|i| {
            if i % 100 == 0 {
                Terminal::Fail
            } else {
                Terminal::Pass
            }
        })
        .collect();
    terminals.extend([
        Terminal::Censored {
            reason: CensorReason::Timeout,
        },
        Terminal::Indeterminate,
        Terminal::Skipped(SkipReason::RedactionRefused),
        Terminal::Skipped(SkipReason::CassetteMiss),
        Terminal::Unsupported(eval_core::UnsupportedReason::PackingHasNoCaller),
        Terminal::Disabled(eval_core::DisabledReason::FeatureOff),
    ]);
    let samples: BTreeMap<String, SampleRecord> = terminals
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let id = format!("s{i:03}");
            (
                id.clone(),
                SampleRecord {
                    id,
                    task: format!("task-{}", i % 3),
                    arm: if i % 2 == 0 {
                        ArmKind::Aged
                    } else {
                        ArmKind::Fresh
                    },
                    policy: HistoryPolicy::Raw,
                    cut: Cut::EndOfRun,
                    lineage: vec![],
                    terminal: *t,
                },
            )
        })
        .collect();
    SampleLedger {
        epoch: 1,
        order: samples.keys().cloned().collect(),
        samples,
    }
}

fn ceilings() -> Ceilings {
    Ceilings {
        indeterminate: ratio(1, 100),
        censoring: ratio(1, 100),
        redaction_refusals: ratio(1, 100),
    }
}

fn baseline() -> BaselineContrast {
    BaselineContrast {
        baseline_version: eval_core::RECENCY_BASELINE_VERSION.to_string(),
        surface: EvaluatedSurface::Surface1,
        recency_bound: 100,
        falsification_pairs_failed: 2,
        positive_controls_passed: 1,
        delivered_ids: 100,
    }
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        elapsed_ms: 1_800_000,
        store_bytes: 512 << 20,
        cassette_bytes: 64 << 20,
        artifact_bytes: 256 << 20,
        temp_roots: 4,
        retained_artifacts: 32,
        processes: 6,
    }
}

fn paired() -> PairedReport {
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    match analyze(&frozen, &family, &pairs(), &arm_rates()).unwrap() {
        Analysis::Report(report) => *report,
        Analysis::Blocked(reason) => panic!("{reason:?}"),
    }
}

fn open_report() -> SuiteBReport {
    let family = family();
    let samples = samples();
    let rates = samples.rates().unwrap();
    let gates = CampaignGates::of(&samples, &ceilings(), &family, &arm_rates()).unwrap();
    let derivation = family.claim_class(WorldProvenance::Generated, None);
    SuiteBReport {
        schema: SUITE_B_REPORT_SCHEMA.to_string(),
        eval_run_id: "cd".repeat(32),
        profile_name: "s0-default-surface".to_string(),
        profile_digest: "ef".repeat(32),
        ceilings: ceilings(),
        surface: EvaluatedSurface::Surface1,
        family,
        claims: Claims {
            boundary: ClaimBoundary::pinned(),
            established: vec![
                Established::RequiredEvidencePresentAtEveryLiveStage,
                Established::TaskOraclePasses,
            ],
            derivation,
            provenance: WorldProvenance::Generated,
            anchor_set: None,
        },
        outcome: ReportOutcome::Open {
            gated: Box::new(GatedBlocks {
                analysis: paired(),
                baseline: baseline(),
                gates,
            }),
        },
        samples,
        rates,
        arm_rates: arm_rates(),
        injection: vec![],
        envelope: Envelope::new(limits()),
    }
}

fn suppressed_report(by: Suppression) -> SuiteBReport {
    let mut report = open_report();
    report.outcome = ReportOutcome::Suppressed { by };
    report.claims.established.clear();
    report
}

fn gated(report: &mut SuiteBReport) -> &mut GatedBlocks {
    match &mut report.outcome {
        ReportOutcome::Open { gated } => gated,
        ReportOutcome::Suppressed { .. } => panic!("open"),
    }
}

#[test]
fn a_report_carries_the_claim_boundary_verbatim_and_its_run_gates() {
    let report = open_report();
    let value = report.serialize().unwrap();
    assert_eq!(
        value["claims"]["boundary"]["schema"],
        json!(CLAIM_BOUNDARY_SCHEMA)
    );
    assert_eq!(
        value["claims"]["boundary"]["exclusions"],
        json!([
            "scheduler-order independence",
            "power-loss durability",
            "wall-clock retention behavior under manipulated age",
            "live-model quality"
        ])
    );
    assert_eq!(
        value["claims"]["derivation"]["class"],
        json!("generated_phase1")
    );
    assert_eq!(
        value["claims"]["established"],
        json!([
            "required_evidence_present_at_every_live_stage",
            "task_oracle_passes"
        ])
    );
    assert_eq!(value["outcome"]["kind"], json!("open"));
    assert_eq!(report.reachability(), Reachability::DefaultProduction);
    assert_eq!(parse_report(&value).unwrap(), report);

    let ReportOutcome::Open { gated } = &report.outcome else {
        panic!("open")
    };
    let gates = gated.gates;
    // One in 646 sits under a one-percent ceiling for each rate.
    assert_eq!(
        (gates.indeterminate.statistic, gates.indeterminate.passed),
        (ratio(1, 646), true)
    );
    assert_eq!(
        (gates.censoring.statistic, gates.censoring.passed),
        (ratio(1, 646), true)
    );
    assert_eq!(
        (
            gates.redaction_refusals.statistic,
            gates.redaction_refusals.passed
        ),
        (ratio(1, 646), true)
    );
    assert_eq!(
        (
            gates.arm_miss_asymmetry.statistic,
            gates.arm_miss_asymmetry.bound,
            gates.arm_miss_asymmetry.passed
        ),
        (ratio(1, 100), ratio(1, 20), true)
    );
    let mut tighter = ceilings();
    tighter.indeterminate = ratio(1, 1000);
    let failed =
        CampaignGates::of(&report.samples, &tighter, &report.family, &arm_rates()).unwrap();
    assert!(!failed.indeterminate.passed);
    assert!(failed.censoring.passed);
    // A ledger nobody attempted has no gates to pass.
    let mut untouched = report.samples.clone();
    for record in untouched.samples.values_mut() {
        record.terminal = Terminal::Skipped(SkipReason::CassetteMiss);
    }
    assert_eq!(
        CampaignGates::of(&untouched, &ceilings(), &report.family, &arm_rates()).unwrap_err(),
        ReportError::NoAttemptedSamples
    );

    for surface in [
        EvaluatedSurface::Surface1,
        EvaluatedSurface::Surface2,
        EvaluatedSurface::Surface3,
        EvaluatedSurface::QueryRoute,
        EvaluatedSurface::Packing,
    ] {
        let expected = match surface {
            EvaluatedSurface::Surface1 | EvaluatedSurface::Surface3 => {
                Reachability::DefaultProduction
            }
            _ => Reachability::ExplicitConfigOnly,
        };
        assert_eq!(reachability_of(surface), expected, "{surface:?}");
    }
}

#[test]
fn a_suppression_removes_the_gates_and_the_claims_and_keeps_the_accounting() {
    let suppressions = [
        (Suppression::TapRejected, Some(StopCondition::A)),
        (
            Suppression::Baseline {
                failure: BaselineFailure::Vacuous,
            },
            Some(StopCondition::B),
        ),
        (
            Suppression::Analysis {
                reason: BlockedReason::InsufficientEffectiveN {
                    effective_n_at_max: ratio(100, 1),
                    required_n_for_margin: 385,
                },
            },
            Some(StopCondition::C),
        ),
        (
            Suppression::Analysis {
                reason: BlockedReason::ArmMissAsymmetry {
                    asymmetry: ratio(1, 10),
                    bound: ratio(1, 20),
                },
            },
            None,
        ),
        (
            Suppression::Envelope {
                exceeded: EnvelopeExceeded {
                    resource: Resource::ElapsedMs,
                    bound: 1_800_000,
                    observed: 1_900_000,
                },
            },
            None,
        ),
    ];
    for (by, condition) in suppressions {
        assert_eq!(by.condition(), condition, "{by:?}");
        let report = suppressed_report(by.clone());
        let value = report.serialize().unwrap();
        assert_eq!(value["outcome"]["kind"], json!("suppressed"));
        assert_eq!(value["claims"]["established"], json!([]));
        assert_eq!(value["rates"]["samples"], json!(646));
        assert_eq!(value["samples"]["order"].as_array().unwrap().len(), 646);
        assert_eq!(value["envelope"]["bounds"]["processes"], json!(6));
        assert_eq!(parse_report(&value).unwrap(), report);
    }
    assert_eq!(
        serde_json::to_value(Suppression::Baseline {
            failure: BaselineFailure::Vacuous,
        })
        .unwrap(),
        json!({"kind": "baseline", "failure": {"kind": "vacuous"}})
    );
    // A suppressed report claiming anything, or an open one claiming nothing.
    let mut claiming = suppressed_report(Suppression::TapRejected);
    claiming
        .claims
        .established
        .push(Established::FirstLossStageNamed);
    assert_eq!(
        claiming.serialize(),
        Err(ReportError::ClaimsDisagreeWithOutcome)
    );
    let mut silent = open_report();
    silent.claims.established.clear();
    assert_eq!(
        silent.serialize(),
        Err(ReportError::ClaimsDisagreeWithOutcome)
    );
    // A suppressed report still validates its family.
    let mut broken = suppressed_report(Suppression::TapRejected);
    broken.family.bootstrap_replicates = 0;
    assert!(matches!(
        broken.serialize(),
        Err(ReportError::Statistics(_))
    ));
    // Gates and a suppression at once have no wire form.
    assert!(
        serde_json::from_value::<ReportOutcome>(json!({
            "kind": "suppressed", "by": {"kind": "tap_rejected"}, "gated": {}
        }))
        .is_err()
    );
}

#[test]
fn a_report_refuses_missing_blocks_forbidden_claims_and_what_it_did_not_derive() {
    let full = open_report().serialize().unwrap();
    for block in full.as_object().unwrap().keys() {
        let mut missing = full.clone();
        missing.as_object_mut().unwrap().remove(block);
        assert!(
            matches!(parse_report(&missing), Err(ReportError::Shape(_))),
            "{block} is a required block"
        );
    }
    let mut extra = full.clone();
    extra["summary_score"] = json!("0.98");
    assert!(matches!(parse_report(&extra), Err(ReportError::Shape(_))));
    for block in [
        "boundary",
        "established",
        "derivation",
        "provenance",
        "anchor_set",
    ] {
        let mut missing = full.clone();
        missing["claims"].as_object_mut().unwrap().remove(block);
        let refused = parse_report(&missing);
        assert!(
            matches!(
                refused,
                Err(ReportError::Shape(_)) | Err(ReportError::Lossy)
            ),
            "claims.{block}: {refused:?}"
        );
    }
    // A claim outside the vocabulary has no wire form: that is how an
    // excluded claim is refused.
    for forbidden in [
        "live-model quality",
        "scheduler-order independence",
        "Live-Model Quality holds",
        "retention holds under manipulated age",
    ] {
        let mut claimed = full.clone();
        claimed["claims"]["established"] = json!([forbidden]);
        assert!(
            matches!(parse_report(&claimed), Err(ReportError::Shape(_))),
            "{forbidden}"
        );
    }
    // A field a tagged unit variant would swallow is caught on the way back out.
    let mut swallowed = full.clone();
    swallowed["samples"]["samples"]["s000"]["terminal"]["note"] = json!("x");
    assert_eq!(parse_report(&swallowed), Err(ReportError::Lossy));

    let mutations: Vec<(&str, Mutate, ReportError)> = vec![
        (
            "another schema",
            Box::new(|r| r.schema = "eval-suite-b-report/v0".into()),
            ReportError::SchemaMismatch {
                found: "eval-suite-b-report/v0".into(),
            },
        ),
        (
            "a run id in upper-case hex",
            Box::new(|r| r.eval_run_id = "CD".repeat(32)),
            ReportError::MalformedDigest {
                field: "eval_run_id",
            },
        ),
        (
            "a profile digest that is not one",
            Box::new(|r| r.profile_digest = "s0".into()),
            ReportError::MalformedDigest {
                field: "profile_digest",
            },
        ),
        (
            "an exclusion dropped from the boundary",
            Box::new(|r| {
                r.claims.boundary.exclusions.pop();
            }),
            ReportError::ClaimBoundaryMismatch,
        ),
        (
            "an exclusion reworded",
            Box::new(|r| r.claims.boundary.exclusions[3] = "model quality".into()),
            ReportError::ClaimBoundaryMismatch,
        ),
        (
            "exclusions reordered",
            Box::new(|r| r.claims.boundary.exclusions.reverse()),
            ReportError::ClaimBoundaryMismatch,
        ),
        (
            "a stored transfer class over a generated world",
            Box::new(|r| {
                r.claims.derivation.class = ClaimClass::Transfer;
                r.claims.derivation.unmet.clear();
            }),
            ReportError::ClaimNotDerived {
                stored: eval_core::ClaimDerivation {
                    class: ClaimClass::Transfer,
                    unmet: vec![],
                    skipped: vec![],
                },
                derived: family().claim_class(WorldProvenance::Generated, None),
            },
        ),
        (
            "a transfer class with no anchor set on real history",
            Box::new(|r| {
                r.claims.provenance = WorldProvenance::RealHistory;
                r.claims.derivation.class = ClaimClass::Transfer;
                r.claims.derivation.unmet.clear();
            }),
            ReportError::ClaimNotDerived {
                stored: eval_core::ClaimDerivation {
                    class: ClaimClass::Transfer,
                    unmet: vec![],
                    skipped: vec![],
                },
                derived: family().claim_class(WorldProvenance::RealHistory, None),
            },
        ),
        (
            "rates that do not follow from the samples",
            Box::new(|r| r.rates.passed = ratio(1, 1)),
            ReportError::RatesDisagree,
        ),
        (
            "a sample dropped from the order",
            Box::new(|r| {
                r.samples.order.pop();
            }),
            ReportError::Samples(SampleError::OrderNotAPermutation),
        ),
        (
            "an analysis read under another family",
            Box::new(|r| r.family.bootstrap_seed += 1),
            ReportError::FamilyDigestMismatch,
        ),
        (
            "arm rates that disagree with the analysis",
            Box::new(|r| {
                r.arm_rates.get_mut("aged").unwrap().miss_rate = "0.03".into();
            }),
            ReportError::ArmRatesDisagree,
        ),
        (
            "more pairs than attempted samples",
            Box::new(|r| {
                for record in r.samples.samples.values_mut().take(400) {
                    record.terminal = Terminal::Skipped(SkipReason::CassetteMiss);
                }
                r.rates = r.samples.rates().unwrap();
                gated(r).gates =
                    CampaignGates::of(&r.samples, &r.ceilings, &r.family, &r.arm_rates).unwrap();
            }),
            ReportError::PairsExceedSamples {
                pairs: 320,
                attempted: 242,
            },
        ),
        (
            "a gate marked passed over a failing rate",
            Box::new(|r| {
                r.ceilings.indeterminate = ratio(1, 1000);
            }),
            ReportError::GatesNotDerived,
        ),
        (
            "a gate statistic edited",
            Box::new(|r| gated(r).gates.censoring.statistic = Ratio::ZERO),
            ReportError::GatesNotDerived,
        ),
        (
            "an open report whose peaks are over its bounds",
            Box::new(|r| r.envelope.peaks.processes = 7),
            ReportError::EnvelopeNotHonoured(EnvelopeExceeded {
                resource: Resource::Processes,
                bound: 6,
                observed: 7,
            }),
        ),
    ];
    for (name, mutate, expected) in mutations {
        let mut mutated = open_report();
        mutate(&mut mutated);
        assert_eq!(mutated.serialize(), Err(expected.clone()), "{name}");
        let value = serde_json::to_value(&mutated).unwrap();
        assert_eq!(parse_report(&value), Err(expected), "{name}");
    }
    // A run stopped by its envelope publishes its peaks as the suppression.
    let mut stopped = suppressed_report(Suppression::Envelope {
        exceeded: EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        },
    });
    stopped.envelope.peaks.processes = 7;
    stopped.serialize().unwrap();
}
