//! The Suite B report serializer: claim boundary verbatim, a suppression
//! removing the gates and nothing else, and no report claiming what its
//! class, samples, profile, or exclusions forbid.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::ContractError;
use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, Approval, ArmKind, ArmRates, ArmResult,
    AxisValue, BaselineContrast, BaselineFailure, BlockedReason, CLAIM_BOUNDARY_SCHEMA,
    CampaignGates, CampaignProfile, Ceilings, CensorReason, ClaimBoundary, ClaimClass, Claims,
    ClusterKey, ClusteringUnit, Cut, Envelope, EnvelopeExceeded, Established, EvaluatedSurface,
    FrozenFamily, GATE_ENDPOINTS, GatedBlocks, HistoryPolicy, ITEM_COUNT_THRESHOLD, IccPilot,
    InjectionScore, IntervalMethod, IntervalOutcome, IntervalWithheld, LivenessBounds, Manifest,
    MultiplicityCorrection, PairOutcome, PairedReport, ProfileError, RECENCY_BASELINE_VERSION,
    RUN_PROFILE_SCHEMA, Ratio, Reachability, RecencyBaseline, ReportError, ReportOutcome, Resource,
    ResourceLimits, RunProfile, SUITE_B_REPORT_SCHEMA, SampleError, SampleLedger, SampleRecord,
    Scale, SkipReason, StatisticsError, StopCondition, StoppingRule, SuiteBReport, Suppression,
    TaskBudgets, Terminal, WorldProvenance, analyze, pair_table_digest, parse_report,
    reachability_of,
};
use serde_json::json;

type Mutate = Box<dyn Fn(&mut SuiteBReport)>;

const FAMILIES: [&str; 6] = ["cargo", "tokio", "django", "git", "docs", "tests"];

fn ratio(n: i64, d: u64) -> Ratio {
    Ratio::try_new(i128::from(n), i128::from(d)).unwrap()
}

/// A pilot whose recorded counts and ICCs imply the world unit and
/// `900 * 5/6 = 750` effective items at 300 affordable worlds.
fn pilot() -> IccPilot {
    IccPilot {
        pilot_run_id: "ab".repeat(32),
        families: FAMILIES
            .iter()
            .map(|family| family.to_string())
            .collect::<BTreeSet<_>>()
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
        required_n_for_margin: 300,
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
        bootstrap_replicates: 40,
        bootstrap_seed: 7,
        trials_k: 3,
        icc_pilot: pilot(),
        transfer_criterion: None,
    }
}

/// A plan that could reach the required N with its own pairs, but whose pilot
/// projects too few items at the affordable worlds: it freezes and then blocks.
fn underpowered_pilot(pilot: &mut IccPilot) {
    pilot.icc_world_seed = Ratio::ZERO;
    pilot.max_affordable_worlds = 8;
    pilot.effective_n_at_max = ratio(24, 1);
    pilot.required_n_for_margin = 100;
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

/// Three hundred pairs over six families and fifty worlds, mostly concordant:
/// the aged arm fails or is censored on a fixed one in twenty each.
fn pairs() -> Vec<PairOutcome> {
    FAMILIES
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
                aged: match (f as u64 * 7 + seed * 13) % 20 {
                    0 => ArmResult::Fail,
                    1 => ArmResult::Censored(CensorReason::Timeout),
                    _ => ArmResult::Pass,
                },
            })
        })
        .collect()
}

/// The manifest that recorded `frozen` before its first outcome, carries the
/// arm rates, names the pairs as its samples, and records the baseline.
fn recorded(frozen: &FrozenFamily, pairs: &[PairOutcome]) -> Manifest {
    let mut manifest = support::manifest();
    manifest.analysis_family_digest = Some(frozen.analysis_family_digest.clone());
    manifest.recency_baseline = Some(RecencyBaseline {
        version: RECENCY_BASELINE_VERSION.to_string(),
        bounds: BTreeMap::from([(EvaluatedSurface::Surface1, 100)]),
    });
    manifest.arm_rates = arm_rates();
    manifest.sample_ids = pairs.iter().map(|pair| pair.pair_id.clone()).collect();
    manifest.sample_order = manifest.sample_ids.clone();
    manifest.result_digest = pair_table_digest(pairs).unwrap();
    manifest
}

/// One sample per arm of every pair, ending as the pair's arm did, plus the
/// six terminals the rates test reads: 602 attempted, 4 not.
fn samples() -> SampleLedger {
    let mut terminals: Vec<Terminal> = pairs()
        .iter()
        .flat_map(|pair| {
            let aged = match pair.aged {
                ArmResult::Pass => Terminal::Pass,
                ArmResult::Fail => Terminal::Fail,
                ArmResult::Censored(reason) => Terminal::Censored { reason },
            };
            [aged, Terminal::Pass]
        })
        .collect();
    terminals.extend([
        Terminal::Censored {
            reason: CensorReason::Timeout,
        },
        Terminal::Indeterminate,
        Terminal::Skipped(SkipReason::RedactionRefused),
        Terminal::Skipped(SkipReason::CassetteMiss),
        Terminal::Unsupported(eval_core::UnsupportedReason::NoMediationBoundary),
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
        censoring: ratio(1, 20),
        redaction_refusals: ratio(1, 100),
    }
}

/// An approved profile whose margins are the family's and whose ceilings are
/// one percent, five for censoring.
fn profile() -> RunProfile {
    RunProfile {
        schema: RUN_PROFILE_SCHEMA.to_string(),
        name: "s0-default-surface".to_string(),
        scale: Scale::S0,
        worlds: 300,
        tasks_per_world: 1,
        max_events_per_log: 64,
        budgets: TaskBudgets {
            max_model_calls: 12,
            max_tool_calls: 40,
            max_tokens_in: 200_000,
            max_tokens_out: 32_000,
            hard_deadline_ms: 600_000,
            max_no_progress_iterations: 3,
        },
        envelope: limits(),
        indeterminate_ceiling: "0.01".to_string(),
        censoring_ceiling: "0.05".to_string(),
        redaction_refusal_ceiling: "0.01".to_string(),
        baseline_bounds: RunProfile::grounded_baseline_bounds(),
        statistics: family().profile,
        approval: Some(Approval {
            approved_by: "maintainer".to_string(),
            approved_at_run_id: "ab".repeat(32),
        }),
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
    let pairs = pairs();
    match analyze(&recorded(&frozen, &pairs), &family, &pairs).unwrap() {
        Analysis::Report(report) => *report,
        Analysis::Blocked(reason) => panic!("{reason:?}"),
    }
}

/// The class the fixture family derives with no anchor set.
fn derived(provenance: WorldProvenance) -> eval_core::ClaimDerivation {
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    family.claim_class(&frozen, provenance, None).unwrap()
}

fn open_report() -> SuiteBReport {
    let family = family();
    let profile = profile();
    let samples = samples();
    let rates = samples.rates().unwrap();
    assert_eq!(profile.ceilings().unwrap(), ceilings());
    let gates = CampaignGates::of(&samples, &ceilings(), &family, &arm_rates()).unwrap();
    let derivation = derived(WorldProvenance::Generated);
    SuiteBReport {
        schema: SUITE_B_REPORT_SCHEMA.to_string(),
        eval_run_id: "cd".repeat(32),
        profile_digest: profile.digest().unwrap(),
        profile,
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
    // One indeterminate and sixteen censored in 602 attempted, and one
    // refusal in 606 declared, each sit under their ceiling.
    assert_eq!(
        (gates.indeterminate.statistic, gates.indeterminate.passed),
        (ratio(1, 602), true)
    );
    assert_eq!(
        (gates.censoring.statistic, gates.censoring.passed),
        (ratio(8, 301), true)
    );
    assert_eq!(
        (
            gates.redaction_refusals.statistic,
            gates.redaction_refusals.passed
        ),
        (ratio(1, 606), true)
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
        // The query route and the packer have no production caller, the
        // label the ledger's `ChainStage` already gives them.
        let expected = match surface {
            EvaluatedSurface::Surface1 | EvaluatedSurface::Surface3 => {
                Reachability::DefaultProduction
            }
            EvaluatedSurface::Surface2 => Reachability::ExplicitConfigOnly,
            EvaluatedSurface::QueryRoute | EvaluatedSurface::Packing => Reachability::TestOnly,
        };
        assert_eq!(reachability_of(surface), expected, "{surface:?}");
    }
    // A censored fresh arm beside an aged non-pass is counted in `b`, so the
    // fresh arm's censored samples back `b` beside its passes: every fresh
    // arm censored still analyzes and still serializes.
    let mut censored_fresh = pairs();
    for pair in &mut censored_fresh {
        pair.fresh = ArmResult::Censored(CensorReason::Timeout);
    }
    let family = family();
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let Analysis::Report(analysis) = analyze(
        &recorded(&frozen, &censored_fresh),
        &family,
        &censored_fresh,
    )
    .unwrap() else {
        panic!("report")
    };
    assert_eq!(
        (analysis.counts.b, analysis.counts.fresh_censored),
        (33, 300)
    );
    let mut censored = open_report();
    for record in censored.samples.samples.values_mut() {
        if record.arm == ArmKind::Fresh && record.terminal == Terminal::Pass {
            record.terminal = Terminal::Censored {
                reason: CensorReason::Timeout,
            };
        }
    }
    censored.rates = censored.samples.rates().unwrap();
    let run_gates = CampaignGates::of(
        &censored.samples,
        &censored.profile.ceilings().unwrap(),
        &censored.family,
        &censored.arm_rates,
    )
    .unwrap();
    censored.outcome = ReportOutcome::Open {
        gated: Box::new(GatedBlocks {
            analysis: *analysis,
            baseline: baseline(),
            gates: run_gates,
        }),
    };
    assert_eq!(
        parse_report(&censored.serialize().unwrap()).unwrap(),
        censored
    );
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
                    effective_n_at_max: ratio(24, 1),
                    required_n_for_margin: 100,
                },
            },
            Some(StopCondition::C),
        ),
        (
            // The completed table's own power, which only the pair table
            // shows: 300 pairs in one world under a world ICC of 1/10.
            Suppression::Analysis {
                reason: BlockedReason::TableUnderpowered {
                    effective_n: ratio(3000, 309),
                    n_clusters: 1,
                    required_n_for_margin: 300,
                },
            },
            None,
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
        let mut report = suppressed_report(by.clone());
        // Each suppression is what the report's own evidence derives.
        match &by {
            Suppression::Analysis {
                reason: BlockedReason::InsufficientEffectiveN { .. },
            } => underpowered_pilot(&mut report.family.icc_pilot),
            Suppression::Analysis {
                reason: BlockedReason::ArmMissAsymmetry { .. },
            } => report.arm_rates.get_mut("fresh").unwrap().miss_rate = "0.11".into(),
            Suppression::Envelope { .. } => report.envelope.peaks.elapsed_ms = 1_900_000,
            Suppression::TapRejected
            | Suppression::Baseline { .. }
            | Suppression::Analysis {
                reason: BlockedReason::TableUnderpowered { .. },
            } => {}
        }
        let value = report.serialize().unwrap();
        assert_eq!(value["outcome"]["kind"], json!("suppressed"));
        assert_eq!(value["claims"]["established"], json!([]));
        assert_eq!(value["rates"]["samples"], json!(606));
        assert_eq!(value["samples"]["order"].as_array().unwrap().len(), 606);
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
            "a profile digest that is not the profile's",
            Box::new(|r| r.profile_digest = "ef".repeat(32)),
            ReportError::ProfileDigestMismatch,
        ),
        (
            "a profile nobody approved",
            Box::new(|r| {
                r.profile.approval = None;
                r.profile_digest = r.profile.digest().unwrap();
            }),
            ReportError::Profile(ProfileError::NotApproved {
                name: "s0-default-surface".into(),
            }),
        ),
        (
            "a ceiling relaxed after approval",
            Box::new(|r| r.profile.indeterminate_ceiling = "1".into()),
            ReportError::ProfileDigestMismatch,
        ),
        (
            "a ceiling over one",
            Box::new(|r| {
                r.profile.indeterminate_ceiling = "1.5".into();
                gated(r).gates.indeterminate.bound = ratio(3, 2);
            }),
            ReportError::Profile(ProfileError::RateOutOfRange {
                field: "indeterminate_ceiling",
            }),
        ),
        (
            "a profile whose margins are not the family's",
            Box::new(|r| {
                r.profile.statistics.harm_bound = "0.2".into();
                r.profile_digest = r.profile.digest().unwrap();
            }),
            ReportError::ProfileDisagreesWithFamily,
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
                derived: derived(WorldProvenance::Generated),
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
                derived: derived(WorldProvenance::RealHistory),
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
            "an analysis over fewer pairs than the plan froze",
            Box::new(|r| gated(r).analysis.counts.n = 299),
            ReportError::PairCountNotFrozen {
                frozen: 300,
                found: 299,
            },
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
            "more pairs than the attempted samples can back at one per arm",
            Box::new(|r| {
                // 267 aged passes in the table need 267 aged-arm passes in
                // the ledger; s002 is one of them (s000 is an aged fail).
                for record in r.samples.samples.values_mut().take(3) {
                    record.terminal = Terminal::Skipped(SkipReason::CassetteMiss);
                }
                r.rates = r.samples.rates().unwrap();
                let ceilings = r.profile.ceilings().unwrap();
                gated(r).gates =
                    CampaignGates::of(&r.samples, &ceilings, &r.family, &r.arm_rates).unwrap();
            }),
            ReportError::PairsExceedSamples {
                arm: ArmKind::Aged,
                terminal: "pass",
                pairs: 267,
                samples: 266,
            },
        ),
        (
            "a gate marked passed over a failing rate",
            Box::new(|r| {
                r.profile.indeterminate_ceiling = "0.001".into();
                r.profile_digest = r.profile.digest().unwrap();
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

#[test]
fn a_report_refuses_what_its_own_evidence_refutes() {
    // Each mutation states something the report's own carried data
    // contradicts; the serializer refuses every one of them by name.
    let refuted: Vec<(&str, Mutate, ReportError)> = vec![
        (
            "a paired gate marked passed over a failing statistic",
            Box::new(|r| gated(r).analysis.gates.quality_loss.statistic = ratio(9, 10)),
            ReportError::PairedGatesNotDerived,
        ),
        (
            "paired counts no table can produce",
            Box::new(|r| gated(r).analysis.counts.b = 300),
            ReportError::Statistics(StatisticsError::InconsistentCounts),
        ),
        (
            "a paired gate verdict flipped",
            Box::new(|r| gated(r).analysis.gates.floor.passed = false),
            ReportError::PairedGatesNotDerived,
        ),
        (
            "an open report under a family whose pilot blocks",
            Box::new(|r| {
                underpowered_pilot(&mut r.family.icc_pilot);
                let digest = r.family.digest().unwrap();
                gated(r).analysis.analysis_family_digest = digest;
            }),
            ReportError::OpenWhileBlocked(BlockedReason::InsufficientEffectiveN {
                effective_n_at_max: ratio(24, 1),
                required_n_for_margin: 100,
            }),
        ),
        (
            "an open report whose arm-miss asymmetry is over the bound",
            Box::new(|r| {
                r.arm_rates.get_mut("aged").unwrap().miss_rate = "0.5".into();
                let arm_rates = r.arm_rates.clone();
                let ceilings = r.profile.ceilings().unwrap();
                let gates =
                    CampaignGates::of(&r.samples, &ceilings, &r.family, &arm_rates).unwrap();
                let g = gated(r);
                g.analysis.arm_rates = arm_rates;
                g.gates = gates;
            }),
            ReportError::OpenWhileBlocked(BlockedReason::ArmMissAsymmetry {
                asymmetry: ratio(12, 25),
                bound: ratio(1, 20),
            }),
        ),
        (
            "an envelope suppression whose reading is not a breach",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Envelope {
                        exceeded: EnvelopeExceeded {
                            resource: Resource::Processes,
                            bound: 6,
                            observed: 3,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an envelope suppression the peaks do not show",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Envelope {
                        exceeded: EnvelopeExceeded {
                            resource: Resource::ElapsedMs,
                            bound: 1_800_000,
                            observed: 1_900_000,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an asymmetry suppression the arm rates do not show",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::ArmMissAsymmetry {
                            asymmetry: ratio(1, 2),
                            bound: ratio(1, 3),
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an effective-N suppression the pilot does not show",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::InsufficientEffectiveN {
                            effective_n_at_max: ratio(100, 1),
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression against a floor the family does not set",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(3000, 309),
                            n_clusters: 1,
                            required_n_for_margin: 299,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression whose effective N meets the floor",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(300, 1),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression over a negative effective N",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(-1, 1),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression over more clusters than the plan has pairs",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(3000, 309),
                            n_clusters: 301,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression over more family clusters than affordable worlds",
            Box::new(|r| {
                // A family-unit pilot affording four worlds over six families
                // realizes at most four family clusters.
                let pilot = &mut r.family.icc_pilot;
                pilot.icc_family = ratio(1, 2);
                pilot.icc_world_seed = Ratio::ZERO;
                pilot.clustering_unit = ClusteringUnit::Family;
                pilot.max_affordable_worlds = 4;
                pilot.effective_n_at_max = ratio(6, 1);
                pilot.required_n_for_margin = 5;
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(3, 1),
                            n_clusters: 5,
                            required_n_for_margin: 5,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression under ICCs that deflate nothing",
            Box::new(|r| {
                // With both ICCs at zero the table's effective N is its pair
                // count, which the plan already holds to the floor.
                let pilot = &mut r.family.icc_pilot;
                pilot.icc_world_seed = Ratio::ZERO;
                pilot.effective_n_at_max = ratio(900, 1);
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(1, 1),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression with an effective N its clusters do not deflate to",
            Box::new(|r| {
                // 300 pairs in one world under a world ICC of 1/10 deflate to
                // exactly 3000/309, not to one.
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(1, 1),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression deflated below what its clusters allow",
            Box::new(|r| {
                // Three hundred pairs in three hundred worlds are three hundred
                // singleton clusters: nothing deflates, whatever the world ICC.
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(10, 1),
                            n_clusters: 300,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression on a surface whose bound does not resolve",
            Box::new(|r| {
                r.surface = EvaluatedSurface::Surface2;
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(3000, 309),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression deflated as if clusters split pairs",
            Box::new(|r| {
                // Seven world clusters over 300 pairs are six of 43 and one of
                // 42, whose mean 2143/50 deflates to 150000/2593; a fractional
                // 300/7 per cluster would deflate to 7000/121, which no table
                // reaches.
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(7000, 121),
                            n_clusters: 7,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression deflated as if one world spanned many families",
            Box::new(|r| {
                // One world lies in one family, so a family ICC of 1/20 deflates
                // 300 pairs in one world to 6000/319 whatever the world ICC.
                let pilot = &mut r.family.icc_pilot;
                pilot.icc_family = ratio(1, 20);
                pilot.icc_world_seed = Ratio::ZERO;
                pilot.effective_n_at_max = ratio(18_000, 169);
                pilot.required_n_for_margin = 80;
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(50, 1),
                            n_clusters: 1,
                            required_n_for_margin: 80,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an interval over more clusters than the approved profile's worlds",
            Box::new(|r| {
                r.profile.worlds = 40;
                r.profile_digest = r.profile.digest().unwrap();
            }),
            ReportError::IntervalNotDerived {
                field: "n_clusters",
            },
        ),
        (
            "an underpowered-table suppression under a pilot that already blocks",
            Box::new(|r| {
                underpowered_pilot(&mut r.family.icc_pilot);
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(10, 1),
                            n_clusters: 1,
                            required_n_for_margin: 100,
                        },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "a tap-rejected suppression with peaks over the bounds",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::TapRejected,
                };
                r.claims.established.clear();
                r.envelope.peaks.processes = 99;
            }),
            ReportError::EnvelopeNotHonoured(EnvelopeExceeded {
                resource: Resource::Processes,
                bound: 6,
                observed: 99,
            }),
        ),
        (
            "a baseline that delivered more ids than its window holds",
            Box::new(|r| gated(r).baseline.delivered_ids = 101),
            ReportError::BaselineDisagrees {
                field: "delivered_ids",
            },
        ),
        (
            "a baseline whose failed falsifications outnumber the table",
            Box::new(|r| gated(r).baseline.falsification_pairs_failed = 301),
            ReportError::BaselineDisagrees {
                field: "falsification_pairs_failed",
            },
        ),
        (
            "a baseline judged over more control pairs than the table has",
            Box::new(|r| {
                let g = gated(r);
                g.baseline.falsification_pairs_failed = 200;
                g.baseline.positive_controls_passed = 101;
            }),
            ReportError::BaselineDisagrees {
                field: "positive_controls_passed",
            },
        ),
        (
            "a baseline contrast on another surface",
            Box::new(|r| gated(r).baseline.surface = EvaluatedSurface::Packing),
            ReportError::BaselineDisagrees { field: "surface" },
        ),
        (
            "a baseline contrast under another version",
            Box::new(|r| gated(r).baseline.baseline_version = "not-a-baseline".into()),
            ReportError::BaselineDisagrees {
                field: "baseline_version",
            },
        ),
        (
            "a baseline contrast off the profile's bound",
            Box::new(|r| gated(r).baseline.recency_bound = 7),
            ReportError::BaselineDisagrees {
                field: "recency_bound",
            },
        ),
        (
            "a vacuous baseline contrast",
            Box::new(|r| {
                let b = &mut gated(r).baseline;
                b.delivered_ids = 0;
                b.falsification_pairs_failed = 0;
                b.positive_controls_passed = 0;
            }),
            ReportError::BaselineDisagrees {
                field: "delivered_ids",
            },
        ),
        (
            "a baseline contrast with no positive control",
            Box::new(|r| gated(r).baseline.positive_controls_passed = 0),
            ReportError::BaselineDisagrees {
                field: "positive_controls_passed",
            },
        ),
        (
            "a sample skipped for an envelope bound this run did not hold",
            Box::new(|r| {
                r.samples.samples.get_mut("s000").unwrap().terminal =
                    Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
                        resource: Resource::Processes,
                        bound: 2,
                        observed: 3,
                    }));
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::SampleEnvelopeDisagrees {
                sample: "s000".into(),
            },
        ),
        (
            "a sample skipped for a reading the peaks never reached",
            Box::new(|r| {
                r.samples.samples.get_mut("s000").unwrap().terminal =
                    Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
                        resource: Resource::Processes,
                        bound: 6,
                        observed: 7,
                    }));
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::SampleEnvelopeDisagrees {
                sample: "s000".into(),
            },
        ),
        (
            "an envelope bound raised above the approved profile's",
            Box::new(|r| r.envelope.bounds.processes = 7),
            ReportError::EnvelopeDisagreesWithProfile,
        ),
        (
            "attempted samples all on one arm",
            Box::new(|r| {
                for record in r.samples.samples.values_mut() {
                    record.arm = ArmKind::Aged;
                }
            }),
            ReportError::PairsExceedSamples {
                arm: ArmKind::Fresh,
                terminal: "pass_or_censored",
                pairs: 33,
                samples: 0,
            },
        ),
        (
            "a pair backed by an indeterminate sample",
            Box::new(|r| {
                // s001 is a fresh-arm pass; indeterminate is no arm result.
                r.samples.samples.get_mut("s001").unwrap().terminal = Terminal::Indeterminate;
                r.rates = r.samples.rates().unwrap();
                let ceilings = r.profile.ceilings().unwrap();
                gated(r).gates =
                    CampaignGates::of(&r.samples, &ceilings, &r.family, &r.arm_rates).unwrap();
            }),
            ReportError::PairsExceedSamples {
                arm: ArmKind::Fresh,
                terminal: "any",
                pairs: 300,
                samples: 299,
            },
        ),
        (
            "an interval whose replicate count is not the family's",
            Box::new(|r| {
                let IntervalOutcome::Computed(interval) = &mut gated(r).analysis.interval else {
                    panic!("computed");
                };
                interval.replicates += 1;
            }),
            ReportError::IntervalNotDerived {
                field: "replicates",
            },
        ),
        (
            "an interval over more items than the pairs",
            Box::new(|r| {
                let IntervalOutcome::Computed(interval) = &mut gated(r).analysis.interval else {
                    panic!("computed");
                };
                interval.n_items += 1;
            }),
            ReportError::IntervalNotDerived { field: "n_items" },
        ),
        (
            "an open report whose ledger records a stop condition",
            Box::new(|r| {
                r.samples.samples.get_mut("s600").unwrap().terminal =
                    Terminal::Skipped(SkipReason::StopCondition {
                        condition: StopCondition::A,
                    });
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::StopConditionDisagrees {
                sample: "s600".into(),
            },
        ),
        (
            "a sample whose lineage names this run",
            Box::new(|r| {
                let id = r.eval_run_id.clone();
                r.samples.samples.get_mut("s000").unwrap().lineage = vec![id];
            }),
            ReportError::LineageNamesThisRun {
                sample: "s000".into(),
            },
        ),
        (
            "a sample skipped for an unapproved profile in an approved report",
            Box::new(|r| {
                r.samples.samples.get_mut("s603").unwrap().terminal =
                    Terminal::Skipped(SkipReason::ProfileNotApproved);
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::SkipDisagreesWithProfile {
                sample: "s603".into(),
            },
        ),
        (
            "two injection scores for one case",
            Box::new(|r| {
                let score = InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::NotMeasurable,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::NotReached,
                };
                r.injection = vec![score.clone(), score];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "an injection score with a blank case",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: " ".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::NotMeasurable,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::NotReached,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: " ".into(),
            },
        ),
        (
            "an injection score with an axis its scorer cannot produce",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::NotMeasurable,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::NotMeasurable,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::NotReached,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "an injection score whose obedience was never reached",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::NotReached,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::NotReached,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "an injection score whose exposure was not measurable",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::No,
                    written_back_cross_session: AxisValue::NotReached,
                    exposure: AxisValue::NotMeasurable,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "an injection score written back without a boundary to observe it",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::Yes,
                    packed: AxisValue::Yes,
                    obeyed: AxisValue::NotMeasurable,
                    written_back_cross_session: AxisValue::Yes,
                    exposure: AxisValue::No,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "an injection score whose write-back was unmeasurable at a boundary that measured obedience",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: "c1".into(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::Yes,
                    packed: AxisValue::Yes,
                    obeyed: AxisValue::No,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::No,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: "c1".into(),
            },
        ),
        (
            "a sample unsupported for packing's missing caller off the packing surface",
            Box::new(|r| {
                r.samples.samples.get_mut("s604").unwrap().terminal =
                    Terminal::Unsupported(eval_core::UnsupportedReason::PackingHasNoCaller);
            }),
            ReportError::SampleAxisDisagrees {
                sample: "s604".into(),
            },
        ),
        (
            "an injection score with no case",
            Box::new(|r| {
                r.injection = vec![InjectionScore {
                    case_id: String::new(),
                    ingested: AxisValue::Yes,
                    retrieved: AxisValue::No,
                    packed: AxisValue::No,
                    obeyed: AxisValue::NotMeasurable,
                    written_back_cross_session: AxisValue::NotMeasurable,
                    exposure: AxisValue::NotReached,
                }];
            }),
            ReportError::InjectionScoreDisagrees {
                case_id: String::new(),
            },
        ),
        (
            "a sample unsupported on another surface",
            Box::new(|r| {
                r.samples.samples.get_mut("s604").unwrap().terminal =
                    Terminal::Unsupported(eval_core::UnsupportedReason::SurfaceNotActivated {
                        surface: EvaluatedSurface::Surface2,
                    });
            }),
            ReportError::SampleAxisDisagrees {
                sample: "s604".into(),
            },
        ),
        (
            "a sample disabled for another scale",
            Box::new(|r| {
                r.samples.samples.get_mut("s605").unwrap().terminal =
                    Terminal::Disabled(eval_core::DisabledReason::ScaleNotBudgeted {
                        scale: Scale::S2,
                    });
            }),
            ReportError::SampleAxisDisagrees {
                sample: "s605".into(),
            },
        ),
        (
            "a tap-rejected suppression over malformed arm rates",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::TapRejected,
                };
                r.claims.established.clear();
                r.arm_rates.get_mut("aged").unwrap().miss_rate = "garbage".into();
            }),
            ReportError::Statistics(StatisticsError::MalformedDecimal {
                field: "arm_rates.miss_rate",
            }),
        ),
        (
            "a baseline suppression on a surface whose bound does not resolve",
            Box::new(|r| {
                r.surface = EvaluatedSurface::Surface2;
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Baseline {
                        failure: BaselineFailure::Vacuous,
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "a sample disabled for an unbudgeted s0",
            Box::new(|r| {
                r.samples.samples.get_mut("s605").unwrap().terminal =
                    Terminal::Disabled(eval_core::DisabledReason::ScaleNotBudgeted {
                        scale: Scale::S0,
                    });
            }),
            ReportError::SampleAxisDisagrees {
                sample: "s605".into(),
            },
        ),
        (
            "an interval over more clusters than the plan has worlds",
            Box::new(|r| {
                // Eight affordable worlds under a consistent pilot that still
                // meets its floor; the fixture's interval spans fifty.
                underpowered_pilot(&mut r.family.icc_pilot);
                r.family.icc_pilot.required_n_for_margin = 20;
                let digest = r.family.digest().unwrap();
                gated(r).analysis.analysis_family_digest = digest;
            }),
            ReportError::IntervalNotDerived {
                field: "n_clusters",
            },
        ),
        (
            "a baseline suppression naming a blank task",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Baseline {
                        failure: BaselineFailure::DeliveredFalsifier { task: " ".into() },
                    },
                };
                r.claims.established.clear();
            }),
            ReportError::SuppressionNotDerived,
        ),
        (
            "an underpowered-table suppression over a ledger that backs no table",
            Box::new(|r| {
                r.outcome = ReportOutcome::Suppressed {
                    by: Suppression::Analysis {
                        reason: BlockedReason::TableUnderpowered {
                            effective_n: ratio(3000, 309),
                            n_clusters: 1,
                            required_n_for_margin: 300,
                        },
                    },
                };
                r.claims.established.clear();
                for record in r.samples.samples.values_mut() {
                    if record.arm == ArmKind::Fresh {
                        record.terminal = Terminal::Skipped(SkipReason::CassetteMiss);
                    }
                }
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::PairsExceedSamples {
                arm: ArmKind::Fresh,
                terminal: "any",
                pairs: 300,
                samples: 0,
            },
        ),
        (
            "an interval outside the statistic's range",
            Box::new(|r| {
                let IntervalOutcome::Computed(interval) = &mut gated(r).analysis.interval else {
                    panic!("computed");
                };
                interval.lower = ratio(2, 1);
                interval.upper = ratio(3, 1);
            }),
            ReportError::IntervalNotDerived { field: "bounds" },
        ),
        (
            "an interval withheld over no clusters",
            Box::new(|r| {
                gated(r).analysis.interval = IntervalOutcome::Withheld {
                    reason: IntervalWithheld::FewerThanTwoClusters { n_clusters: 0 },
                };
            }),
            ReportError::IntervalNotDerived {
                field: "n_clusters",
            },
        ),
        (
            "an aged pass the ledger records as a fail",
            Box::new(|r| {
                r.samples.samples.get_mut("s002").unwrap().terminal = Terminal::Fail;
                r.rates = r.samples.rates().unwrap();
            }),
            ReportError::PairsExceedSamples {
                arm: ArmKind::Aged,
                terminal: "pass",
                pairs: 267,
                samples: 266,
            },
        ),
        (
            "a sample not activated on a default-production surface",
            Box::new(|r| {
                r.samples.samples.get_mut("s604").unwrap().terminal =
                    Terminal::Unsupported(eval_core::UnsupportedReason::SurfaceNotActivated {
                        surface: EvaluatedSurface::Surface1,
                    });
            }),
            ReportError::SampleAxisDisagrees {
                sample: "s604".into(),
            },
        ),
        (
            "an epoch past the canonical safe range",
            Box::new(|r| r.samples.epoch = 9_007_199_254_740_993),
            ReportError::NotCanonical(ContractError::NotCanonical(
                "number 9007199254740993 is not a safe integer".into(),
            )),
        ),
    ];
    for (name, mutate, expected) in refuted {
        let mut mutated = open_report();
        mutate(&mut mutated);
        assert_eq!(mutated.serialize(), Err(expected.clone()), "{name}");
        let value = serde_json::to_value(&mutated).unwrap();
        assert_eq!(parse_report(&value), Err(expected), "{name}");
    }
    // A run its envelope stopped skips the rest under the reading that
    // crossed the bound, and names that reading as its suppression.
    let mut stopped = suppressed_report(Suppression::Envelope {
        exceeded: EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        },
    });
    stopped.envelope.peaks.processes = 7;
    stopped.samples.samples.get_mut("s000").unwrap().terminal =
        Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        }));
    stopped.rates = stopped.samples.rates().unwrap();
    assert_eq!(
        parse_report(&stopped.serialize().unwrap()).unwrap(),
        stopped
    );
    // A run a stop condition halted skips the rest under that condition, and
    // only that condition.
    let mut halted = suppressed_report(Suppression::TapRejected);
    halted.samples.samples.get_mut("s000").unwrap().terminal =
        Terminal::Skipped(SkipReason::StopCondition {
            condition: StopCondition::A,
        });
    halted.rates = halted.samples.rates().unwrap();
    assert_eq!(parse_report(&halted.serialize().unwrap()).unwrap(), halted);
    halted.samples.samples.get_mut("s000").unwrap().terminal =
        Terminal::Skipped(SkipReason::StopCondition {
            condition: StopCondition::B,
        });
    assert_eq!(
        halted.serialize(),
        Err(ReportError::StopConditionDisagrees {
            sample: "s000".into(),
        })
    );
}

#[test]
fn run_gates_are_shares_of_attempted_samples_and_padding_does_not_move_them() {
    let report = open_report();
    let padded = {
        let mut ledger = report.samples.clone();
        for i in 0..20_000 {
            let id = format!("pad{i:05}");
            ledger.order.push(id.clone());
            ledger.samples.insert(
                id.clone(),
                SampleRecord {
                    id,
                    task: "pad".to_string(),
                    arm: ArmKind::Aged,
                    policy: HistoryPolicy::Raw,
                    cut: Cut::EndOfRun,
                    lineage: vec![],
                    terminal: Terminal::Disabled(eval_core::DisabledReason::FeatureOff),
                },
            );
        }
        ledger
    };
    let before =
        CampaignGates::of(&report.samples, &ceilings(), &report.family, &arm_rates()).unwrap();
    let after = CampaignGates::of(&padded, &ceilings(), &report.family, &arm_rates()).unwrap();
    // 602 attempted: one indeterminate and sixteen censored among them.
    assert_eq!(before.indeterminate.statistic, ratio(1, 602));
    assert_eq!(before.censoring.statistic, ratio(8, 301));
    assert_eq!(
        after.indeterminate.statistic,
        before.indeterminate.statistic
    );
    assert_eq!(after.censoring.statistic, before.censoring.statistic);
    // A refusal is a skip, so its share is of every declared sample.
    assert_eq!(before.redaction_refusals.statistic, ratio(1, 606));
    assert_eq!(after.redaction_refusals.statistic, ratio(1, 20_606));
}
