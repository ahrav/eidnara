//! One Suite B campaign at scale S0 on the default surface: compiled pairs,
//! both arms driven through the direct-host fixture, the baseline contrast,
//! the run gates, and one report published write-then-rename, all under an
//! approved profile and inside a declared envelope.

#![cfg(all(unix, feature = "test-support"))]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Instant;

use daemon::transform::UserHintPass;
use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, Approval, ArmKind, ArmRates, ArmResult,
    BaselineVerdict, CampaignGates, CampaignProfile, ClaimBoundary, Claims, ClusterKey,
    ClusteringUnit, Cut, Destination, DisabledReason, Envelope, Established, EvaluatedSurface,
    EventId, EventLog, FrozenFamily, GatedBlocks, HistoryPolicy, IccPilot, IntervalMethod,
    IntervalOutcome, LivenessBounds, Mode, MultiplicityCorrection, Pair, PairOutcome, PairSet,
    PairSetInput, ProfileError, Query, RUN_PROFILE_SCHEMA, Ratio, RenderConfig, RenderedMessage,
    ReportOutcome, RepositorySpec, Required, Resource, ResourceLimits, RunProfile,
    SUITE_B_REPORT_SCHEMA, SampleLedger, SampleRecord, Scale, Sensitivity, ServedClass,
    SessionSpec, StageVerdict, StoppingRule, SuiteBReport, Surface1Stage, Task, TaskBudgets,
    TaskRole, TaskUsage, Terminal, Visibility, WorldConfig, WorldProvenance, analyze,
    check_recency_baseline, compile_pair_set, generate_all, parse_report, render, serialize_spec,
};
use memory_store::StoredHistorySegment;
use support::direct_host::FixtureProcess;
use support::eval_surface::{
    EPOCH_MS, Knobs, SurfaceLedger, World, block_on, mid, observe, pass, seed_store, segment, text,
};

const SEED: u64 = 0x5EED_B000_0000_0002;
const PROJECT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SESSION: &str = "session-0";
const AGED_MESSAGES: u32 = 130;

fn one_session(messages: u32, max_events_per_log: u32) -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages,
            tool_span_every: 0,
            correction_every: 0,
            invalidation_every: 0,
        }],
        repositories: Vec::<RepositorySpec>::new(),
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log,
    }
}

fn profile(scale: Scale, max_events_per_log: u32, approval: Option<Approval>) -> RunProfile {
    let label = serde_json::to_value(scale).unwrap();
    RunProfile {
        schema: RUN_PROFILE_SCHEMA.to_string(),
        name: format!("{}-surface1-raw", label.as_str().unwrap()),
        scale,
        worlds: 1,
        tasks_per_world: 3,
        max_events_per_log,
        budgets: TaskBudgets {
            max_model_calls: 1,
            max_tool_calls: 1,
            max_tokens_in: 4_096,
            max_tokens_out: 1_024,
            hard_deadline_ms: 60_000,
            max_no_progress_iterations: 1,
        },
        envelope: ResourceLimits {
            elapsed_ms: 1_200_000,
            store_bytes: 64 << 20,
            cassette_bytes: 1,
            artifact_bytes: 1 << 20,
            temp_roots: 1,
            retained_artifacts: 1,
            processes: 1,
        },
        indeterminate_ceiling: "0".to_string(),
        censoring_ceiling: "0".to_string(),
        redaction_refusal_ceiling: "0".to_string(),
        baseline_bounds: RunProfile::grounded_baseline_bounds(),
        statistics: CampaignProfile {
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
        approval,
    }
}

fn family(profile: &RunProfile) -> AnalysisFamily {
    AnalysisFamily {
        schema: ANALYSIS_FAMILY_SCHEMA.to_string(),
        endpoints: vec!["quality_loss".into(), "harm".into(), "floor".into()],
        families: vec!["generated".into()],
        exclusions: vec![],
        stopping_rule: StoppingRule::FixedN,
        multiplicity_correction: MultiplicityCorrection::Holm,
        profile: profile.statistics.clone(),
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
            icc_family: Ratio::new(1, 4),
            icc_world_seed: Ratio::new(0, 1),
            clustering_unit: ClusteringUnit::WorldSeed,
            max_affordable_worlds: 60,
            effective_n_at_max: Ratio::new(400, 1),
            required_n_for_margin: 385,
        },
        transfer_criterion: None,
    }
}

fn query(max_events_per_log: u32) -> Query {
    Query {
        valid_time_ms: eval_core::MAX_VALID_TIME_MS,
        observation_time_ms: eval_core::MAX_VALID_TIME_MS,
        scope: BTreeSet::from([SESSION.to_string()]),
        destination: Destination::Local,
        served: Some(ServedClass {
            sensitivity: Sensitivity::Normal,
            visibility: Visibility::Labeled,
            auto_inject: Visibility::Hidden,
            auto_search: Visibility::Hidden,
        }),
        registry_sensitivity: Sensitivity::Normal,
        max_events_per_log,
    }
}

/// The aged history is one long session; each task's truth is one message:
/// early for the falsifier, last for the positive control, and half a window
/// from the end (inside surface 1's window) for the plain task.
fn tasks(aged: &EventLog, window: u32, max_events_per_log: u32) -> Vec<Task> {
    let message = |index: usize| aged.events[index].id.clone();
    let task = |name: &str, role, id: EventId| Task {
        id: name.to_string(),
        role,
        query: query(max_events_per_log),
        evidence: BTreeSet::from([id]),
    };
    let inside = aged.events.len() - (window / 2) as usize;
    vec![
        task("early-message", TaskRole::Falsification, message(2)),
        task(
            "last-message",
            TaskRole::PositiveControl,
            message(aged.events.len() - 1),
        ),
        task("recent-message", TaskRole::Plain, message(inside)),
    ]
}

/// The arm's history as one OpenCode session: every rendered message under
/// the task's session id in valid-time order, and one segment per message
/// whose summary names the message's text and a marker only that message
/// has, so the hint scorer's rarity rule never decides a task. The renderer's
/// expected occurrence ids name the original entities and are not read here;
/// the ledger is keyed on event ids.
struct Arm {
    world: World,
    segments: Vec<StoredHistorySegment>,
    events: BTreeMap<i64, EventId>,
}

fn summary(message: &RenderedMessage) -> String {
    let marker = mid(message).rsplit('-').next().unwrap();
    format!("{} decision recorded as note{marker}", text(message))
}

fn arm(log: &EventLog) -> Arm {
    let rendering = render(
        log,
        &RenderConfig {
            project_id: PROJECT.to_string(),
            repository_id: "repo-0".to_string(),
            object_format: "sha1".to_string(),
        },
    )
    .unwrap();
    assert!(
        rendering.excluded_by_rule.is_empty(),
        "{:?}",
        rendering.excluded_by_rule
    );
    let mut messages = rendering.messages;
    for message in &mut messages {
        message.session_id = SESSION.to_string();
        message.message["info"]["sessionID"] = SESSION.into();
    }
    let markers: BTreeSet<String> = messages.iter().map(summary).collect();
    assert_eq!(markers.len(), messages.len(), "every summary is unique");
    let segments: Vec<StoredHistorySegment> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| segment(index as i64 + 1, message, &summary(message)))
        .collect();
    // The sequence-to-event map comes from what the store will hold, the
    // segment's stored native identity, and must agree with the order the
    // messages were seeded in.
    let by_mid: BTreeMap<&str, &EventId> = messages.iter().map(|m| (mid(m), &m.event_id)).collect();
    let events: BTreeMap<i64, EventId> = segments
        .iter()
        .map(|segment| {
            let (end_mid, _) = daemon::wire::split_block_id(&segment.end_message_id).unwrap();
            (segment.sequence, by_mid[end_mid].clone())
        })
        .collect();
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(events[&(index as i64 + 1)], message.event_id);
    }
    Arm {
        world: World {
            session: SESSION.to_string(),
            messages,
        },
        segments,
        events,
    }
}

/// The task asks for the decision in the words its summary uses.
fn prompt(arm: &Arm, evidence: &EventId) -> String {
    let message = arm
        .world
        .messages
        .iter()
        .find(|m| m.event_id == *evidence)
        .expect("the evidence is a rendered message");
    summary(message)
}

/// Every regular file under the arm's root: the kernel store with its WAL and
/// shm, and the fixture's own files beside it; the control socket has no size.
fn root_bytes(root: &Path) -> u64 {
    fn walk(path: &Path) -> u64 {
        let kind = std::fs::symlink_metadata(path).unwrap();
        if kind.is_file() {
            return kind.len();
        }
        if !kind.is_dir() {
            return 0;
        }
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| walk(&entry.unwrap().path()))
            .sum()
    }
    walk(root)
}

/// What the run holds at once, so the envelope reads a live count rather
/// than each acquisition as one.
#[derive(Default)]
struct Held {
    roots: u64,
    processes: u64,
}

struct ArmRun {
    result: ArmResult,
    verdict: StageVerdict<Surface1Stage>,
}

/// Seeds a root of its own with the arm's history, runs the task's prompt
/// through one fixture process, and reads delivered evidence from the host's
/// own selection. The attempt is timed against the task budgets; the root,
/// the process, and the store's bytes are charged to the envelope as they
/// peak; the fixture's backend counters prove no model call was made.
fn run_arm(
    arm: &Arm,
    task: &Task,
    profile: &RunProfile,
    envelope: &mut Envelope,
    held: &mut Held,
    started: Instant,
) -> ArmRun {
    assert_eq!(task.evidence.len(), 1, "one truth per task on surface 1");
    let evidence = task.evidence.iter().next().unwrap();
    let root = tempfile::tempdir().unwrap();
    held.roots += 1;
    envelope.observe(Resource::TempRoots, held.roots).unwrap();
    seed_store(root.path(), &arm.world.session, &arm.segments);
    let fixture = FixtureProcess::start_at(root.path().to_path_buf());
    held.processes += 1;
    envelope
        .observe(Resource::Processes, held.processes)
        .unwrap();
    let knobs = Knobs {
        threshold: 0.5,
        ..Knobs::default()
    };
    let attempt = Instant::now();
    let pass = block_on(pass(&fixture, &arm.world, &prompt(arm, evidence), &knobs));
    let usage = TaskUsage {
        elapsed_ms: u64::try_from(attempt.elapsed().as_millis()).unwrap(),
        ..TaskUsage::default()
    };
    let counters = fixture.counters(7);
    assert_eq!(counters["started"], 0, "surface 1 makes no model call");
    envelope.observe(Resource::CassetteBytes, 0).unwrap();
    envelope
        .observe(Resource::StoreBytes, root_bytes(root.path()))
        .unwrap();
    fixture.shutdown();
    held.processes -= 1;
    drop(root);
    held.roots -= 1;
    envelope
        .observe(
            Resource::ElapsedMs,
            u64::try_from(started.elapsed().as_millis()).unwrap(),
        )
        .unwrap();
    let identities: BTreeMap<i64, String> = arm
        .events
        .iter()
        .map(|(sequence, id)| (*sequence, id.0.clone()))
        .collect();
    let mut ledger = SurfaceLedger::default();
    observe(&mut ledger, pass.outcome.as_ref(), &identities);
    let delivered: BTreeSet<EventId> = match &pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome
            .trace
            .selected
            .iter()
            .map(|sequence| arm.events[sequence].clone())
            .collect(),
        _ => BTreeSet::new(),
    };
    let result = match profile.budgets.exhausted(&usage) {
        Some(reason) => ArmResult::Censored(reason),
        None if task.evidence.is_subset(&delivered) => ArmResult::Pass,
        None => ArmResult::Fail,
    };
    let verdict = ledger.verdict(
        &[Required {
            occurrence: evidence.0.clone(),
            entry: Surface1Stage::TailEligibility,
        }],
        &BTreeSet::new(),
        Surface1Stage::Attachment,
    );
    ArmRun { result, verdict }
}

fn sample(pair: &Pair, arm: ArmKind, label: &str, result: ArmResult) -> SampleRecord {
    SampleRecord {
        id: format!("{}:{label}", pair.task.id),
        task: pair.task.id.clone(),
        arm,
        policy: HistoryPolicy::Raw,
        cut: Cut::EndOfRun,
        lineage: vec![],
        terminal: match result {
            ArmResult::Pass => Terminal::Pass,
            ArmResult::Fail => Terminal::Fail,
            ArmResult::Censored(reason) => Terminal::Censored { reason },
        },
    }
}

/// Stages the bytes beside the target, syncs them, renames into place, and
/// syncs the directory, so a reader sees the whole report or none of it.
fn write_then_rename(path: &Path, bytes: &[u8]) {
    let staged = path.with_extension("json.staged");
    let file = std::fs::File::create(&staged).unwrap();
    std::io::Write::write_all(&mut &file, bytes).unwrap();
    file.sync_all().unwrap();
    std::fs::rename(&staged, path).unwrap();
    std::fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

#[test]
fn an_unapproved_profile_runs_no_campaign() {
    let unapproved = profile(Scale::S0, 512, None);
    assert_eq!(
        unapproved.approved(),
        Err(ProfileError::NotApproved {
            name: "s0-surface1-raw".to_string(),
        })
    );
}

/// One campaign at the given scale: compile, drive both arms of every pair
/// through the fixture, analyze, gate, publish, and read the report back.
fn campaign(scale: Scale, aged_messages: u32) -> SuiteBReport {
    let started = Instant::now();
    let max_events_per_log = aged_messages.max(64) * 2;
    let profile = profile(
        scale,
        max_events_per_log,
        Some(Approval {
            approved_by: "test-approval".to_string(),
            approved_at_run_id: "ab".repeat(32),
        }),
    );
    profile.approved().unwrap();
    let mut envelope = Envelope::new(profile.envelope.clone());
    let mut held = Held::default();
    let window = profile.baseline_bounds[&EvaluatedSurface::Surface1];

    let aged = generate_all(
        SEED,
        &one_session(aged_messages, max_events_per_log),
        Mode::Generate,
    )
    .unwrap()
    .log;
    let natural_fresh = generate_all(
        SEED ^ 0x77,
        &one_session(5, max_events_per_log),
        Mode::Generate,
    )
    .unwrap()
    .log;
    let tasks = tasks(&aged, window, max_events_per_log);
    let set: PairSet = compile_pair_set(PairSetInput {
        surface: EvaluatedSurface::Surface1,
        declared_bound: NonZeroU32::new(window),
        aged: &aged,
        natural_fresh: &natural_fresh,
        fixture: &serialize_spec(),
        tasks: &tasks,
    })
    .unwrap();
    let BaselineVerdict::Established { contrast } =
        check_recency_baseline(&set, |pair| pair.recency_window.clone()).unwrap()
    else {
        panic!("{aged_messages} messages push the third one out of a window of {window}");
    };
    assert_eq!(
        (
            contrast.falsification_pairs_failed,
            contrast.positive_controls_passed
        ),
        (1, 1)
    );

    let aged_arm = arm(&set.aged);
    assert_eq!(aged_arm.segments.len(), aged_messages as usize);
    let mut outcomes = Vec::new();
    let mut samples = BTreeMap::new();
    let mut order = Vec::new();
    let mut verdicts: BTreeMap<(&str, &str), StageVerdict<Surface1Stage>> = BTreeMap::new();
    for pair in &set.pairs {
        let fresh_arm = arm(&pair.fresh);
        assert!(
            fresh_arm.segments.len() < aged_arm.segments.len(),
            "the control is short"
        );
        let aged_run = run_arm(
            &aged_arm,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
        let fresh_run = run_arm(
            &fresh_arm,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
        for (kind, label, run) in [
            (ArmKind::Aged, "aged", &aged_run),
            (ArmKind::Fresh, "fresh", &fresh_run),
        ] {
            let record = sample(pair, kind, label, run.result);
            order.push(record.id.clone());
            samples.insert(record.id.clone(), record);
            verdicts.insert((pair.task.id.as_str(), label), run.verdict);
        }
        outcomes.push(PairOutcome {
            pair_id: pair.task.id.clone(),
            cluster: ClusterKey {
                family: "generated".to_string(),
                world_seed: SEED,
            },
            fresh: fresh_run.result,
            aged: aged_run.result,
        });
    }

    // The early truth is outside surface 1's window on the aged arm and
    // delivered on the short control; the truths inside the window are
    // delivered on both arms.
    let by_task: BTreeMap<&str, &PairOutcome> =
        outcomes.iter().map(|o| (o.pair_id.as_str(), o)).collect();
    let expected = [
        (
            "early-message",
            ArmResult::Fail,
            StageVerdict::FirstLoss(Surface1Stage::CandidateWindow),
        ),
        ("recent-message", ArmResult::Pass, StageVerdict::Clean),
        ("last-message", ArmResult::Pass, StageVerdict::Clean),
    ];
    for (name, aged_result, aged_verdict) in expected {
        assert_eq!(
            (by_task[name].aged, by_task[name].fresh),
            (aged_result, ArmResult::Pass),
            "{name}"
        );
        assert_eq!(verdicts[&(name, "aged")], aged_verdict, "{name}");
        assert_eq!(verdicts[&(name, "fresh")], StageVerdict::Clean, "{name}");
    }

    let ledger = SampleLedger {
        epoch: 1,
        order,
        samples,
    };
    let rates = ledger.rates().unwrap();
    assert_eq!((rates.samples, ledger.attempted()), (6, 6));
    // Every arm's backend counters read zero model calls, so both arms' miss
    // and refusal rates are zero by observation.
    let zero = || ArmRates {
        miss_rate: "0".to_string(),
        refusal_rate: "0".to_string(),
    };
    let arm_rates = BTreeMap::from([("aged".to_string(), zero()), ("fresh".to_string(), zero())]);
    let family = family(&profile);
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let Analysis::Report(analysis) = analyze(&frozen, &family, &outcomes, &arm_rates).unwrap()
    else {
        panic!("three pairs report, with the interval withheld");
    };
    assert_eq!(
        (analysis.counts.n, analysis.counts.b, analysis.counts.c),
        (3, 1, 0)
    );
    assert!(
        !analysis.gates.quality_loss.passed && !analysis.gates.harm.passed,
        "one loss in three pairs fails the paired gates at these margins: {:?}",
        analysis.gates
    );
    assert!(matches!(
        analysis.interval,
        IntervalOutcome::Withheld { .. }
    ));
    let ceilings = profile.ceilings().unwrap();
    let gates = CampaignGates::of(&ledger, &ceilings, &family, &arm_rates).unwrap();
    // The claims follow from what the run found.
    let mut established = Vec::new();
    if verdicts
        .values()
        .any(|v| matches!(v, StageVerdict::FirstLoss(_)))
    {
        established.push(Established::FirstLossStageNamed);
    }
    if outcomes
        .iter()
        .any(|o| o.aged == ArmResult::Pass || o.fresh == ArmResult::Pass)
    {
        established.push(Established::TaskOraclePasses);
    }
    let mut report = SuiteBReport {
        schema: SUITE_B_REPORT_SCHEMA.to_string(),
        eval_run_id: "ee".repeat(32),
        profile_name: profile.name.clone(),
        profile_digest: profile.digest().unwrap(),
        ceilings,
        surface: EvaluatedSurface::Surface1,
        family: family.clone(),
        claims: Claims {
            boundary: ClaimBoundary::pinned(),
            established,
            derivation: family.claim_class(WorldProvenance::Generated, None),
            provenance: WorldProvenance::Generated,
            anchor_set: None,
        },
        outcome: ReportOutcome::Open {
            gated: Box::new(GatedBlocks {
                analysis: *analysis,
                baseline: contrast,
                gates,
            }),
        },
        samples: ledger,
        rates,
        arm_rates,
        injection: vec![],
        envelope: envelope.clone(),
    };
    // The publish root, the artifact's bytes, and the retained artifact are
    // charged before the envelope is copied into the report, so the published
    // peaks include the publication itself; the report's size is charged from
    // this first serialization.
    let publish = tempfile::tempdir().unwrap();
    held.roots += 1;
    envelope.observe(Resource::TempRoots, held.roots).unwrap();
    let sized = serde_json::to_vec_pretty(&report.serialize().unwrap()).unwrap();
    envelope
        .observe(Resource::ArtifactBytes, sized.len() as u64)
        .unwrap();
    envelope.observe(Resource::RetainedArtifacts, 1).unwrap();
    envelope
        .observe(
            Resource::ElapsedMs,
            u64::try_from(started.elapsed().as_millis()).unwrap(),
        )
        .unwrap();
    envelope.check().unwrap();
    report.envelope = envelope.clone();
    let bytes = serde_json::to_vec_pretty(&report.serialize().unwrap()).unwrap();
    let path = publish.path().join("suite-b-report.json");
    write_then_rename(&path, &bytes);
    assert!(!path.with_extension("json.staged").exists());
    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(parse_report(&published).unwrap(), report);
    let peaks = &published["envelope"]["peaks"];
    for resource in ["store_bytes", "elapsed_ms", "artifact_bytes"] {
        assert!(
            peaks[resource].as_u64().unwrap() > 0,
            "{resource} on disk: {peaks}"
        );
    }
    assert_eq!(peaks["processes"], 1);
    assert_eq!(peaks["temp_roots"], 1);
    assert_eq!(peaks["cassette_bytes"], 0);
    report
}

#[test]
fn an_s0_campaign_on_the_default_surface_publishes_one_gated_report() {
    let report = campaign(Scale::S0, AGED_MESSAGES);
    assert_eq!(report.profile_name, "s0-surface1-raw");
    assert_eq!(
        report.claims.established,
        vec![
            Established::FirstLossStageNamed,
            Established::TaskOraclePasses
        ]
    );
    assert_eq!(
        report.reachability(),
        eval_core::Reachability::DefaultProduction
    );
}

/// Runs a longer history only when `EIDNARA_EVAL_S1_BUDGET_MS` grants a
/// budget. Without one the campaign is disabled at its scale, which is the
/// closed-vocabulary terminal the sample ledger records; a budget that is set
/// but not a number is refused rather than read as absent.
#[test]
#[ignore = "S1 runs under an explicit budget: set EIDNARA_EVAL_S1_BUDGET_MS and run with --ignored"]
fn an_s1_campaign_runs_only_under_its_budget() {
    let variable = Scale::S1.budget_env().unwrap();
    let budget_ms = match std::env::var(variable) {
        Err(_) => {
            let record = SampleRecord {
                id: "s1-surface1-raw".to_string(),
                task: "campaign".to_string(),
                arm: ArmKind::Aged,
                policy: HistoryPolicy::Raw,
                cut: Cut::EndOfRun,
                lineage: vec![],
                terminal: Terminal::Disabled(DisabledReason::ScaleNotBudgeted { scale: Scale::S1 }),
            };
            let ledger = SampleLedger {
                epoch: 1,
                order: vec![record.id.clone()],
                samples: BTreeMap::from([(record.id.clone(), record)]),
            };
            assert_eq!(ledger.rates().unwrap().disabled, Ratio::ONE);
            return;
        }
        Ok(text) => text
            .parse::<u64>()
            .unwrap_or_else(|e| panic!("{variable}={text:?} is not a millisecond budget: {e}")),
    };
    let started = Instant::now();
    let report = campaign(Scale::S1, 400);
    assert!(
        u64::try_from(started.elapsed().as_millis()).unwrap() <= budget_ms,
        "the S1 campaign stayed inside {variable}={budget_ms}"
    );
    assert_eq!(report.profile_name, "s1-surface1-raw");
}
