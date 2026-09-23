//! Run profiles, the closed terminal vocabulary, sample accounting, and the
//! resource envelope.

use std::collections::BTreeMap;

use context_core::canonical_json::ContractError;
use eval_core::{
    Approval, ArmKind, CampaignProfile, CensorReason, Cut, DisabledReason, Envelope,
    EnvelopeExceeded, EvaluatedSurface, HistoryPolicy, LivenessBounds, PairError, ProfileError,
    RUN_PROFILE_SCHEMA, Ratio, Resource, ResourceLimits, RunProfile, SampleError, SampleLedger,
    SampleRecord, Scale, SkipReason, StatisticsError, StopCondition, TaskBudgets, TaskUsage,
    Terminal, UnsupportedReason, parse_run_profile,
};
use serde_json::{Value, json};

fn ratio(numerator: i64, denominator: u64) -> Ratio {
    Ratio::try_new(i128::from(numerator), i128::from(denominator)).unwrap()
}

type Mutate<T> = Box<dyn Fn(&mut T)>;

fn statistics() -> CampaignProfile {
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

fn budgets() -> TaskBudgets {
    TaskBudgets {
        max_model_calls: 12,
        max_tool_calls: 40,
        max_tokens_in: 200_000,
        max_tokens_out: 32_000,
        hard_deadline_ms: 600_000,
        max_no_progress_iterations: 3,
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

fn profile() -> RunProfile {
    RunProfile {
        schema: RUN_PROFILE_SCHEMA.to_string(),
        name: "s0-default-surface".to_string(),
        scale: Scale::S0,
        worlds: 4,
        tasks_per_world: 3,
        max_events_per_log: 64,
        budgets: budgets(),
        envelope: limits(),
        indeterminate_ceiling: "0.1".to_string(),
        censoring_ceiling: "0.2".to_string(),
        redaction_refusal_ceiling: "0".to_string(),
        baseline_bounds: RunProfile::grounded_baseline_bounds(),
        statistics: statistics(),
        approval: None,
    }
}

fn value(profile: &RunProfile) -> Value {
    serde_json::to_value(profile).unwrap()
}

#[test]
fn a_profile_pins_every_number_and_runs_only_once_approved() {
    let profile = profile();
    profile.validate().unwrap();
    assert_eq!(parse_run_profile(&value(&profile)).unwrap(), profile);
    assert_eq!(
        profile.baseline_bounds,
        BTreeMap::from([(EvaluatedSurface::Surface1, 100)]),
        "the one grounded default is surface 1's hint candidate limit"
    );
    let ceilings = profile.ceilings().unwrap();
    assert_eq!(
        (
            ceilings.indeterminate,
            ceilings.censoring,
            ceilings.redaction_refusals
        ),
        (ratio(1, 10), ratio(1, 5), Ratio::ZERO)
    );
    assert_eq!(
        profile.approved(),
        Err(ProfileError::NotApproved {
            name: "s0-default-surface".to_string(),
        }),
        "code and refusal tests need no approval; a campaign does"
    );
    let mut approved = profile.clone();
    approved.approval = Some(Approval {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "ab".repeat(32),
    });
    assert_eq!(approved.approved().unwrap().approved_by, "maintainer");
    assert_eq!(approved.digest().unwrap().len(), 64);
    assert!(
        matches!(
            profile.fault_profile(),
            Err(ProfileError::NotApproved { .. })
        ),
        "a fault campaign runs only under an approved profile"
    );
    let fault_profile = approved.fault_profile().unwrap();
    assert_eq!(fault_profile.digest(), approved.digest().unwrap());
    assert_eq!(
        *fault_profile.liveness(),
        approved.statistics.liveness_bounds
    );
    assert_eq!(*fault_profile.envelope(), approved.envelope);
    assert_ne!(approved.digest().unwrap(), profile.digest().unwrap());
    for nobody in ["", " \t"] {
        approved.approval.as_mut().unwrap().approved_by = nobody.to_string();
        assert_eq!(
            approved.validate(),
            Err(ProfileError::Empty {
                field: "approval.approved_by",
            }),
            "{nobody:?}"
        );
    }
    approved.approval = Some(Approval {
        approved_by: "maintainer".to_string(),
        approved_at_run_id: "AB".repeat(32),
    });
    assert_eq!(
        approved.validate(),
        Err(ProfileError::MalformedRunId {
            field: "approval.approved_at_run_id",
        })
    );
    assert_eq!(Scale::S0.budget_env(), "EIDNARA_EVAL_S0_BUDGET_MS");
    assert_eq!(Scale::S1.budget_env(), "EIDNARA_EVAL_S1_BUDGET_MS");
    assert_eq!(Scale::S2.budget_env(), "EIDNARA_EVAL_S2_BUDGET_MS");
    assert_eq!(serde_json::to_value(Scale::S2).unwrap(), json!("s2"));
}

#[test]
fn a_profile_refuses_every_absent_or_zero_setting_by_name() {
    // Every setting is required; a profile missing one never parses. Even
    // the approval must be written down, as `null` when there is none.
    let full = value(&profile());
    assert_eq!(full["approval"], Value::Null);
    for field in full.as_object().unwrap().keys() {
        let mut missing = full.clone();
        missing.as_object_mut().unwrap().remove(field);
        if field == "approval" {
            assert_eq!(parse_run_profile(&missing), Err(ProfileError::Lossy));
            continue;
        }
        assert!(
            matches!(parse_run_profile(&missing), Err(ProfileError::Shape(_))),
            "{field} is required"
        );
    }
    let mut extra = full.clone();
    extra["defaults"] = json!({});
    assert!(matches!(
        parse_run_profile(&extra),
        Err(ProfileError::Shape(_))
    ));
    // Every nested setting is required too, and every zero names its field
    // at its own nesting: the twenty settings a zero would make "unbounded".
    for parent in [
        "budgets",
        "envelope",
        "statistics",
        "statistics.liveness_bounds",
    ] {
        let keys: Vec<String> = full
            .pointer(&format!("/{}", parent.replace('.', "/")))
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for key in keys {
            let mut missing = full.clone();
            missing
                .pointer_mut(&format!("/{}", parent.replace('.', "/")))
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(&key);
            assert!(
                matches!(parse_run_profile(&missing), Err(ProfileError::Shape(_))),
                "{parent}.{key} is required"
            );
        }
    }
    let zeros = [
        "worlds",
        "tasks_per_world",
        "max_events_per_log",
        "budgets.max_model_calls",
        "budgets.max_tool_calls",
        "budgets.max_tokens_in",
        "budgets.max_tokens_out",
        "budgets.hard_deadline_ms",
        "budgets.max_no_progress_iterations",
        "envelope.elapsed_ms",
        "envelope.store_bytes",
        "envelope.cassette_bytes",
        "envelope.artifact_bytes",
        "envelope.temp_roots",
        "envelope.retained_artifacts",
        "envelope.processes",
        "statistics.liveness_bounds.catch_up_episodes",
        "statistics.liveness_bounds.embedding_passes",
        "statistics.liveness_bounds.materialization_episodes",
        "statistics.liveness_bounds.reviewer_coordinator_passes",
    ];
    assert_eq!(zeros.len(), 20);
    for field in zeros {
        let mut zeroed = full.clone();
        *zeroed
            .pointer_mut(&format!("/{}", field.replace('.', "/")))
            .unwrap() = json!(0);
        assert_eq!(
            parse_run_profile(&zeroed),
            Err(ProfileError::Zero { field }),
            "{field}"
        );
    }

    let mutations: Vec<(&str, Mutate<RunProfile>, ProfileError)> = vec![
        (
            "another schema",
            Box::new(|p| p.schema = "eval-run-profile/v0".into()),
            ProfileError::SchemaMismatch {
                found: "eval-run-profile/v0".into(),
            },
        ),
        (
            "no name",
            Box::new(|p| p.name.clear()),
            ProfileError::Empty { field: "name" },
        ),
        (
            "a blank name",
            Box::new(|p| p.name = " \t".into()),
            ProfileError::Empty { field: "name" },
        ),
        (
            "zero worlds",
            Box::new(|p| p.worlds = 0),
            ProfileError::Zero { field: "worlds" },
        ),
        (
            "zero tasks",
            Box::new(|p| p.tasks_per_world = 0),
            ProfileError::Zero {
                field: "tasks_per_world",
            },
        ),
        (
            "no event limit",
            Box::new(|p| p.max_events_per_log = 0),
            ProfileError::Zero {
                field: "max_events_per_log",
            },
        ),
        (
            "no token budget",
            Box::new(|p| p.budgets.max_tokens_out = 0),
            ProfileError::Zero {
                field: "budgets.max_tokens_out",
            },
        ),
        (
            "no deadline",
            Box::new(|p| p.budgets.hard_deadline_ms = 0),
            ProfileError::Zero {
                field: "budgets.hard_deadline_ms",
            },
        ),
        (
            "no store envelope",
            Box::new(|p| p.envelope.store_bytes = 0),
            ProfileError::Zero {
                field: "envelope.store_bytes",
            },
        ),
        (
            "no process envelope",
            Box::new(|p| p.envelope.processes = 0),
            ProfileError::Zero {
                field: "envelope.processes",
            },
        ),
        (
            "a liveness bound of zero",
            Box::new(|p| p.statistics.liveness_bounds.embedding_passes = 0),
            ProfileError::Zero {
                field: "statistics.liveness_bounds.embedding_passes",
            },
        ),
        (
            "a ceiling that is not a decimal",
            Box::new(|p| p.indeterminate_ceiling = "ten percent".into()),
            ProfileError::MalformedDecimal {
                field: "indeterminate_ceiling",
            },
        ),
        (
            "a ceiling over one",
            Box::new(|p| p.indeterminate_ceiling = "1.5".into()),
            ProfileError::RateOutOfRange {
                field: "indeterminate_ceiling",
            },
        ),
        (
            "a censoring ceiling nobody set",
            Box::new(|p| p.censoring_ceiling.clear()),
            ProfileError::MalformedDecimal {
                field: "censoring_ceiling",
            },
        ),
        (
            "a refusal ceiling over one",
            Box::new(|p| p.redaction_refusal_ceiling = "2".into()),
            ProfileError::RateOutOfRange {
                field: "redaction_refusal_ceiling",
            },
        ),
        (
            "no baseline bounds",
            Box::new(|p| p.baseline_bounds.clear()),
            ProfileError::Empty {
                field: "baseline_bounds",
            },
        ),
        (
            "surface 1 off its pin",
            Box::new(|p| {
                p.baseline_bounds.insert(EvaluatedSurface::Surface1, 50);
            }),
            ProfileError::Baseline(PairError::RecencyBoundConflict {
                surface: EvaluatedSurface::Surface1,
                pinned: 100,
                declared: 50,
            }),
        ),
        (
            "a zero bound on surface 1 does not fall back to the pin",
            Box::new(|p| {
                p.baseline_bounds.insert(EvaluatedSurface::Surface1, 0);
            }),
            ProfileError::ZeroBaselineBound {
                surface: EvaluatedSurface::Surface1,
            },
        ),
        (
            "a zero bound elsewhere",
            Box::new(|p| {
                p.baseline_bounds.insert(EvaluatedSurface::Surface3, 0);
            }),
            ProfileError::ZeroBaselineBound {
                surface: EvaluatedSurface::Surface3,
            },
        ),
        (
            "a margin nobody set",
            Box::new(|p| p.statistics.noninferiority_margin.clear()),
            ProfileError::Statistics(StatisticsError::MalformedDecimal {
                field: "noninferiority_margin",
            }),
        ),
        (
            "a budget past the canonical safe range",
            Box::new(|p| p.budgets.max_tokens_in = 9_007_199_254_740_993),
            ProfileError::NotCanonical(ContractError::NotCanonical(
                "number 9007199254740993 is not a safe integer".into(),
            )),
        ),
    ];
    for (name, mutate, expected) in mutations {
        let mut mutated = profile();
        mutate(&mut mutated);
        assert_eq!(mutated.validate(), Err(expected.clone()), "{name}");
        assert_eq!(parse_run_profile(&value(&mutated)), Err(expected), "{name}");
    }
    // A declared bound on another surface is accepted as declared.
    let mut declared = profile();
    declared
        .baseline_bounds
        .insert(EvaluatedSurface::QueryRoute, 3);
    declared.validate().unwrap();
}

#[test]
fn each_budget_censors_with_its_own_reason_in_declared_order() {
    let budgets = budgets();
    assert_eq!(budgets.exhausted(&TaskUsage::default()), None);
    let cases = [
        (
            TaskUsage {
                model_calls: 12,
                ..TaskUsage::default()
            },
            CensorReason::MaxModelCalls,
        ),
        (
            TaskUsage {
                tool_calls: 40,
                ..TaskUsage::default()
            },
            CensorReason::MaxToolCalls,
        ),
        (
            TaskUsage {
                tokens_in: 200_000,
                ..TaskUsage::default()
            },
            CensorReason::MaxTokensIn,
        ),
        (
            TaskUsage {
                tokens_out: 32_000,
                ..TaskUsage::default()
            },
            CensorReason::MaxTokensOut,
        ),
        (
            TaskUsage {
                elapsed_ms: 600_000,
                ..TaskUsage::default()
            },
            CensorReason::HardDeadlineMs,
        ),
        (
            TaskUsage {
                no_progress_iterations: 3,
                ..TaskUsage::default()
            },
            CensorReason::MaxNoProgressIterations,
        ),
    ];
    for (usage, reason) in &cases {
        assert_eq!(budgets.exhausted(usage), Some(*reason), "{reason:?}");
    }
    let one_short = TaskUsage {
        model_calls: 11,
        tool_calls: 39,
        tokens_in: 199_999,
        tokens_out: 31_999,
        elapsed_ms: 599_999,
        no_progress_iterations: 2,
    };
    assert_eq!(budgets.exhausted(&one_short), None);
    // Two budgets reached at once report the earlier declared one.
    let two = TaskUsage {
        tokens_out: 32_000,
        elapsed_ms: 600_000,
        ..TaskUsage::default()
    };
    assert_eq!(budgets.exhausted(&two), Some(CensorReason::MaxTokensOut));
}

fn record(id: &str, terminal: Terminal) -> SampleRecord {
    SampleRecord {
        id: id.to_string(),
        task: "early-commit".to_string(),
        arm: ArmKind::Aged,
        policy: HistoryPolicy::Raw,
        cut: Cut::EndOfRun,
        lineage: vec![],
        terminal,
    }
}

fn ledger(terminals: &[Terminal]) -> SampleLedger {
    let samples: BTreeMap<String, SampleRecord> = terminals
        .iter()
        .enumerate()
        .map(|(i, t)| (format!("s{i}"), record(&format!("s{i}"), *t)))
        .collect();
    SampleLedger {
        epoch: 3,
        order: samples.keys().rev().cloned().collect(),
        samples,
    }
}

#[test]
fn every_sample_ends_in_exactly_one_closed_vocabulary_terminal() {
    let terminals = [
        Terminal::Pass,
        Terminal::Pass,
        Terminal::Fail,
        Terminal::Censored {
            reason: CensorReason::Timeout,
        },
        Terminal::Indeterminate,
        Terminal::Skipped(SkipReason::StopCondition {
            condition: StopCondition::B,
        }),
        Terminal::Unsupported(UnsupportedReason::SurfaceNotActivated {
            surface: EvaluatedSurface::Surface2,
        }),
        Terminal::Disabled(DisabledReason::ScaleNotBudgeted { scale: Scale::S1 }),
    ];
    let ledger = ledger(&terminals);
    let rates = ledger.rates().unwrap();
    assert_eq!(rates.samples, 8);
    assert_eq!(rates.passed, ratio(1, 4));
    assert_eq!(rates.failed, ratio(1, 8));
    assert_eq!(rates.censored, ratio(1, 8));
    assert_eq!(rates.indeterminate, ratio(1, 8));
    assert_eq!(rates.skipped, ratio(1, 8));
    assert_eq!(rates.unsupported, ratio(1, 8));
    assert_eq!(rates.disabled, ratio(1, 8));
    let round: SampleLedger =
        serde_json::from_value(serde_json::to_value(&ledger).unwrap()).unwrap();
    assert_eq!(round, ledger);
    // Every reason's wire form is pinned; the shell emits exactly these.
    let wire = [
        (Terminal::Pass, json!({"kind": "pass"})),
        (Terminal::Fail, json!({"kind": "fail"})),
        (
            Terminal::Censored {
                reason: CensorReason::MaxTokensOut,
            },
            json!({"kind": "censored", "reason": "max_tokens_out"}),
        ),
        (Terminal::Indeterminate, json!({"kind": "indeterminate"})),
        (
            Terminal::Skipped(SkipReason::ProfileNotApproved),
            json!({"kind": "skipped", "reason": "profile_not_approved"}),
        ),
        (
            terminals[5],
            json!({"kind": "skipped", "reason": "stop_condition", "condition": "b"}),
        ),
        (
            Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
                resource: Resource::StoreBytes,
                bound: 10,
                observed: 11,
            })),
            json!({"kind": "skipped", "reason": "envelope_exceeded", "resource": "store_bytes", "bound": 10, "observed": 11}),
        ),
        (
            Terminal::Skipped(SkipReason::CassetteMiss),
            json!({"kind": "skipped", "reason": "cassette_miss"}),
        ),
        (
            Terminal::Skipped(SkipReason::RedactionRefused),
            json!({"kind": "skipped", "reason": "redaction_refused"}),
        ),
        (
            Terminal::Skipped(SkipReason::NoContainment),
            json!({"kind": "skipped", "reason": "no_containment"}),
        ),
        (
            terminals[6],
            json!({"kind": "unsupported", "reason": "surface_not_activated", "surface": "surface2"}),
        ),
        (
            Terminal::Unsupported(UnsupportedReason::NoMediationBoundary),
            json!({"kind": "unsupported", "reason": "no_mediation_boundary"}),
        ),
        (
            Terminal::Unsupported(UnsupportedReason::PackingHasNoCaller),
            json!({"kind": "unsupported", "reason": "packing_has_no_caller"}),
        ),
        (
            Terminal::Unsupported(UnsupportedReason::PolicyNotOnSurface {
                policy: HistoryPolicy::Pruned,
                surface: EvaluatedSurface::Surface1,
            }),
            json!({"kind": "unsupported", "reason": "policy_not_on_surface", "policy": "pruned", "surface": "surface1"}),
        ),
        (
            terminals[7],
            json!({"kind": "disabled", "reason": "scale_not_budgeted", "scale": "s1"}),
        ),
        (
            Terminal::Disabled(DisabledReason::FeatureOff),
            json!({"kind": "disabled", "reason": "feature_off"}),
        ),
    ];
    for (terminal, expected) in wire {
        assert_eq!(serde_json::to_value(terminal).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<Terminal>(expected).unwrap(),
            terminal
        );
    }
    // A reason outside the vocabulary does not parse.
    for bad in [
        json!({"kind": "skipped", "reason": "flaky"}),
        json!({"kind": "skipped"}),
        json!({"kind": "censored", "reason": "timeout", "note": "x"}),
        json!({"kind": "censored"}),
        json!({"kind": "maybe"}),
    ] {
        assert!(
            serde_json::from_value::<Terminal>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    assert_eq!(
        SampleLedger {
            epoch: 0,
            order: vec![],
            samples: BTreeMap::new(),
        }
        .rates()
        .unwrap()
        .passed,
        Ratio::ZERO
    );

    let mutations: Vec<(&str, Mutate<SampleLedger>, SampleError)> = vec![
        (
            "a sample never ordered",
            Box::new(|l| {
                l.order.pop();
            }),
            SampleError::OrderNotAPermutation,
        ),
        (
            "a sample ordered twice",
            Box::new(|l| l.order.push("s0".into())),
            SampleError::OrderNotAPermutation,
        ),
        (
            "an ordered sample without a record",
            Box::new(|l| {
                l.samples.remove("s3");
            }),
            SampleError::OrderNotAPermutation,
        ),
        (
            "a record under another key",
            Box::new(|l| l.samples.get_mut("s1").unwrap().id = "s9".into()),
            SampleError::IdMismatch {
                key: "s1".into(),
                id: "s9".into(),
            },
        ),
        (
            "a blank sample id",
            Box::new(|l| {
                let mut record = l.samples.remove("s1").unwrap();
                record.id = " ".into();
                l.samples.insert(" ".into(), record);
                l.order = l.samples.keys().cloned().collect();
            }),
            SampleError::Blank {
                sample: " ".into(),
                field: "id",
            },
        ),
        (
            "a blank task",
            Box::new(|l| l.samples.get_mut("s1").unwrap().task = " \t".into()),
            SampleError::Blank {
                sample: "s1".into(),
                field: "task",
            },
        ),
        (
            "a lineage entry that is not a run id",
            Box::new(|l| l.samples.get_mut("s2").unwrap().lineage = vec!["retry-1".into()]),
            SampleError::MalformedLineage {
                sample: "s2".into(),
                entry: "retry-1".into(),
            },
        ),
        (
            "a lineage entry in upper-case hex",
            Box::new(|l| l.samples.get_mut("s2").unwrap().lineage = vec!["AB".repeat(32)]),
            SampleError::MalformedLineage {
                sample: "s2".into(),
                entry: "AB".repeat(32),
            },
        ),
        (
            "the same earlier attempt twice",
            Box::new(|l| {
                l.samples.get_mut("s2").unwrap().lineage = vec!["cd".repeat(32), "cd".repeat(32)]
            }),
            SampleError::RepeatedLineage {
                sample: "s2".into(),
                entry: "cd".repeat(32),
            },
        ),
    ];
    for (name, mutate, expected) in mutations {
        let mut mutated = ledger.clone();
        mutate(&mut mutated);
        assert_eq!(mutated.validate(), Err(expected.clone()), "{name}");
        assert_eq!(mutated.rates(), Err(expected), "{name}");
    }
    let mut retried = ledger.clone();
    retried.samples.get_mut("s2").unwrap().lineage = vec!["cd".repeat(32)];
    retried.validate().unwrap();
    assert_eq!(ledger.attempted(), 5, "pass, fail, censored, indeterminate");
}

#[test]
fn the_envelope_records_the_peak_that_crossed_it_and_refuses_from_that_reading() {
    let mut envelope = Envelope::new(limits());
    envelope.check().unwrap();
    envelope.observe(Resource::StoreBytes, 100 << 20).unwrap();
    envelope.observe(Resource::StoreBytes, 90 << 20).unwrap();
    assert_eq!(envelope.peaks.store_bytes, 100 << 20, "peaks never fall");
    envelope.observe(Resource::Processes, 6).unwrap();
    assert_eq!(
        envelope.observe(Resource::Processes, 7),
        Err(EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        })
    );
    assert_eq!(
        envelope.peaks.processes, 7,
        "the crossing reading is on record"
    );
    // A later reading within the bound does not unlatch the breach: the peak
    // is what is over, and the peak is what the refusal names.
    assert_eq!(
        envelope.observe(Resource::Processes, 3),
        Err(EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        })
    );
    assert_eq!(
        envelope.check(),
        Err(EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        })
    );
    // A breach stays latched across resources: an in-bound reading elsewhere
    // is refused with the breach still on record.
    assert_eq!(
        envelope.observe(Resource::StoreBytes, 10 << 20),
        Err(EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 7,
        })
    );
    // Two resources over report the earlier declared one.
    envelope
        .observe(Resource::ElapsedMs, 2_000_000)
        .unwrap_err();
    assert_eq!(envelope.check().unwrap_err().resource, Resource::ElapsedMs);
    let fields: Vec<String> = serde_json::to_value(limits())
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let names: Vec<String> = Resource::ALL
        .iter()
        .map(|r| {
            serde_json::to_value(r)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, fields, "every envelope field is a resource");
    for resource in Resource::ALL {
        let mut fresh = Envelope::new(limits());
        let bound = resource.of(&fresh.bounds);
        fresh.observe(resource, bound).unwrap();
        assert_eq!(
            fresh.observe(resource, bound + 1),
            Err(EnvelopeExceeded {
                resource,
                bound,
                observed: bound + 1,
            }),
            "{resource:?}"
        );
    }
    assert_eq!(
        serde_json::to_value(EnvelopeExceeded {
            resource: Resource::TempRoots,
            bound: 4,
            observed: 5,
        })
        .unwrap(),
        json!({"resource": "temp_roots", "bound": 4, "observed": 5})
    );
}

#[test]
fn an_envelope_skip_must_name_a_breach() {
    let mut ledger = ledger(&[Terminal::Pass, Terminal::Pass]);
    ledger.samples.get_mut("s1").unwrap().terminal =
        Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
            resource: Resource::StoreBytes,
            bound: 100,
            observed: 1,
        }));
    let expected = SampleError::EnvelopeNotExceeded {
        sample: "s1".into(),
    };
    assert_eq!(ledger.validate(), Err(expected.clone()));
    assert_eq!(ledger.rates(), Err(expected));
    ledger.samples.get_mut("s1").unwrap().terminal =
        Terminal::Skipped(SkipReason::EnvelopeExceeded(EnvelopeExceeded {
            resource: Resource::StoreBytes,
            bound: 100,
            observed: 101,
        }));
    ledger.validate().unwrap();
    assert!(
        !EnvelopeExceeded {
            resource: Resource::Processes,
            bound: 6,
            observed: 6,
        }
        .is_breach(),
        "at the bound is within it"
    );
}
