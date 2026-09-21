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
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, Approval, ArmKind, ArmRates, ArmRecord,
    ArmResult, Attestation, BaselineVerdict, BinaryDigest, BuildRecord, CampaignGates,
    CampaignProfile, ClaimBoundary, Claims, ClusterKey, ClusteringUnit, ComponentVersions,
    Construction, Cut, CutOutcome, CutReceipt, Destination, DisabledReason,
    ELIGIBILITY_SPEC_DIGEST, EVENT_SCHEMA_VERSION, Envelope, Established, EvaluatedSurface,
    EventId, EventLog, ExecutionMode, FAILURE_CLASS_TABLE_DIGEST, FrozenFamily, GENERATOR_VERSION,
    GatedBlocks, GovernanceArms, HistoryPolicy, IccPilot, Ingestion, IntervalMethod,
    IntervalOutcome, LINEARIZATION_RULE_VERSION, LivenessBounds, MANIFEST_SCHEMA, Manifest,
    MemoryReviewerModelCalls, Mode, MultiplicityCorrection, PAIRING_POLICY_VERSION, Pair,
    PairOutcome, PairSet, PairSetInput, ProfileError, Query, RANDOM_SCHEMA_VERSION,
    REDUCER_VERSION, RUN_PROFILE_SCHEMA, Ratio, Reachability, RecencyBaseline, RenderConfig,
    RenderedMessage, ReportOutcome, RepositorySpec, Required, Resource, ResourceLimits,
    RunIdentity, RunProfile, RunStatus, SUITE_B_REPORT_SCHEMA, SampleLedger, SampleRecord, Scale,
    Sensitivity, ServedClass, SessionSpec, SkipReason, StageVerdict, StoppingRule, SuiteBReport,
    Surface1Stage, Task, TaskBudgets, TaskRole, TaskUsage, Terminal, TokenizerProfile,
    UnsupportedReason, Visibility, WorldConfig, WorldProvenance, analyze, check_recency_baseline,
    compile_pair_set, eval_run_id, generate_all, parse_manifest, parse_report, render,
    serialize_spec, text_decision,
};
use memory_store::{MemoryStore, StoredHistorySegment};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::direct_host::{Backend, Launch, fixture_binary};
use support::eval_surface::{
    EPOCH_MS, Knobs, Pass, SurfaceLedger, World, block_on, lifecycle, observe_rendered, pass, text,
};

const SEED: u64 = 0x5EED_B000_0000_0002;
const PROJECT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SESSION: &str = "session-0";
const AGED_MESSAGES: u32 = 130;
const S0_ELAPSED_BOUND_MS: u64 = 1_200_000;
const S1_AGED_MESSAGES: u32 = 400;

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

fn profile(
    scale: Scale,
    max_events_per_log: u32,
    elapsed_ms: u64,
    approval: Option<Approval>,
) -> RunProfile {
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
            hard_deadline_ms: 600_000,
            max_no_progress_iterations: 1,
        },
        envelope: ResourceLimits {
            elapsed_ms,
            store_bytes: 64 << 20,
            cassette_bytes: 1 << 20,
            artifact_bytes: 1 << 20,
            temp_roots: 3,
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
/// the task's session id in valid-time order. The renderer's expected
/// occurrence ids name the original entities and are not read here; the
/// ledger is keyed on event ids.
fn world(log: &EventLog) -> World {
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
    let words: BTreeSet<&str> = messages.iter().map(text).collect();
    assert_eq!(
        words.len(),
        messages.len(),
        "every message has words of its own"
    );
    World {
        session: SESSION.to_string(),
        messages,
    }
}

/// Whether `served` carries `phrase` as whole words: `slot4` is not served by
/// a fragment saying `slot47`.
fn carries(served: &str, phrase: &str) -> bool {
    assert!(!phrase.is_empty(), "a truth has words");
    let in_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    served.match_indices(phrase).any(|(at, _)| {
        let before = served[..at].chars().next_back();
        let after = served[at + phrase.len()..].chars().next();
        !before.is_some_and(in_word) && !after.is_some_and(in_word)
    })
}

/// Messages per summarizer segment: the fixture's scripted summarizer folds
/// this many presented lines into one `history_segment`.
const CHUNK: usize = 5;
const SUMMARIZER_NAMESPACE: &str = "eval-run:campaign:summarizer";
/// The context limit the summarizer is configured with, in tokens.
const CONTEXT_LIMIT_TOKENS: u64 = 40_000;
/// The harness's model of context pressure: the provider's input grows by
/// this many tokens a turn, and the harness reports it up to the limit.
const TOKENS_PER_TURN: u64 = 300;
/// The prompt the recording run's final turn carries; long enough for the
/// hint gate, asking for nothing a segment would serve.
const BUILD_PROMPT: &str = "summarize the session so far";

/// The context pressure the harness reports on turn `turn`.
fn usage(turn: usize) -> Option<(u64, u64)> {
    Some((
        (TOKENS_PER_TURN * turn as u64).min(CONTEXT_LIMIT_TOKENS - 500),
        CONTEXT_LIMIT_TOKENS,
    ))
}

/// Writes the user config tier the fixture's daemon reads the summarizer's
/// model chain from under `home`; the keys live only in that tier.
fn write_summarizer_config(home: &Path) {
    let dir = home.join("eidnara");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("eidnara.jsonc"),
        serde_json::to_vec_pretty(&json!({
            "history_summarizer": {
                "model": "fixture/summarizer",
                "context_limit_tokens": CONTEXT_LIMIT_TOKENS,
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

/// The stored segment with the clock the daemon stamped it with removed:
/// everything else two runs of one life must agree on.
fn timeless(segment: &StoredHistorySegment) -> StoredHistorySegment {
    StoredHistorySegment {
        created_at: 0,
        ..segment.clone()
    }
}

/// How an arm's daemon replaces history: not at all, or through its own
/// summarizer against the fixture's backend served from a cassette.
#[derive(Debug, Clone)]
enum Replacement {
    Raw,
    Summarizer(Backend),
}

/// What one life of a world through the fixture left behind: the store's
/// history segments, the final pass, the fixture's backend counters, how many
/// times the daemon's summarizer fired and how many of those firings the
/// cassette refused as carrying a secret-shaped span.
struct Lived {
    segments: Vec<StoredHistorySegment>,
    pass: Pass,
    counters: Value,
    firings: u32,
    refusals: u32,
    stderr: String,
}

/// Whether a firing's recorded failure is the cassette refusing its frame.
fn refused(diagnostics: &Value) -> bool {
    diagnostics["last_failure"]
        .as_str()
        .is_some_and(|failure| failure.contains("redaction_refused"))
}

/// Lives `world` through one fixture process on a root of its own, one
/// harness turn at a time with the harness's context pressure, then issues
/// `prompt` as the next turn; the store moves through every turn in one
/// incarnation and the daemon's own summarizer, when configured, fires by its
/// own trigger. Returns what the life left behind once the fixture has
/// exited.
fn live(
    world: &World,
    replacement: &Replacement,
    prompt: &str,
    envelope: &mut Envelope,
    held: &mut Held,
    started: Instant,
) -> Lived {
    let home = occupy(held, envelope);
    let root = occupy(held, envelope);
    let mut launch = Launch::at(root.path().to_path_buf()).config_home(home.path());
    if let Replacement::Summarizer(backend) = replacement {
        write_summarizer_config(home.path());
        launch = launch.backend(backend.clone());
    }
    let fixture = launch.start();
    held.processes += 1;
    envelope
        .observe(Resource::Processes, held.processes)
        .unwrap();
    let knobs = Knobs {
        usage: usage(world.messages.len() + 1),
        ..Knobs::default()
    };
    let (turns, pass) = block_on(async {
        let turns = lifecycle(&fixture, world, usage).await;
        let pass = pass(&fixture, world, prompt, &knobs).await;
        (turns, pass)
    });
    let mut firings: u32 = 0;
    let mut failures_seen = false;
    for diagnostics in turns
        .iter()
        .map(|turn| &turn["history_summarizer"])
        .chain([&pass.response["history_summarizer"]])
    {
        if diagnostics["fired"] == json!(true) {
            firings += 1;
        }
        // A refused frame is the cassette's typed refusal; any other failure
        // of the daemon's own summarizer ends the campaign.
        if refused(diagnostics) {
            failures_seen = true;
        } else {
            assert_eq!(diagnostics["last_failure"], Value::Null, "{diagnostics}");
        }
    }
    let counters = fixture.counters(11);
    // The recording cassette counts the frames it refused; every firing
    // reached the fixture's backend once on a recording run, and a replaying
    // fixture has no controlled backend to count.
    let backend_calls = u32::try_from(counters["started"].as_u64().unwrap()).unwrap();
    let refusals = u32::try_from(counters["cassette_refused"].as_u64().unwrap()).unwrap();
    match replacement {
        Replacement::Summarizer(Backend::Record { .. }) => {
            assert_eq!(
                backend_calls, firings,
                "{counters} against {firings} firings"
            );
        }
        Replacement::Raw | Replacement::Summarizer(Backend::Replay { .. }) => {
            assert_eq!((backend_calls, refusals), (0, 0), "{counters}");
        }
    }
    assert_eq!(
        refusals > 0,
        failures_seen,
        "the daemon reports a failure exactly when the cassette refused a frame: {counters}"
    );
    let (status, output) = fixture.shutdown_with_status();
    // A recording fixture refuses to write a cassette holding a refused
    // frame and says so at exit; every other exit is clean.
    let refused_recording =
        refusals > 0 && matches!(replacement, Replacement::Summarizer(Backend::Record { .. }));
    assert_eq!(
        status.success(),
        !refused_recording,
        "fixture exit {status}:\n{}",
        output.stderr
    );
    if refused_recording {
        assert!(
            output.stderr.contains("cassette refused: RedactionRefused"),
            "{}",
            output.stderr
        );
    }
    held.processes -= 1;
    let descriptor = daemon::managed_store_descriptor(root.path()).unwrap();
    let store = MemoryStore::open(&descriptor).unwrap();
    let segments = store.load_history_segments(&world.session).unwrap();
    drop(store);
    vacate(root, held, envelope, started);
    vacate(home, held, envelope, started);
    Lived {
        segments,
        pass,
        counters,
        firings,
        refusals,
        stderr: output.stderr,
    }
}

/// One world's summarizer traffic recorded: the cassette's backend for the
/// arm's replays and the segments the recording published, or the refusal
/// that left no cassette to replay, with the firing count the refusal rate
/// is over.
struct Recording {
    replay: Option<Backend>,
    segments: Vec<StoredHistorySegment>,
    /// The cassette file's size on disk; zero when nothing was written.
    cassette_bytes: u64,
    firings: u32,
    refusals: u32,
}

/// Records one life of `world` under the summarizer into a cassette of its
/// own. A world the summarizer never fired on records no frame and its arm
/// replays an empty cassette; a frame the cassette refused as carrying a
/// secret-shaped span leaves no cassette at all, and the arm is skipped as
/// refused.
fn record(
    label: &str,
    world: &World,
    cassettes: &Path,
    envelope: &mut Envelope,
    held: &mut Held,
    started: Instant,
) -> Recording {
    let file = cassettes.join(format!("{label}.cassette.json"));
    assert!(
        !file.exists(),
        "one recording per world: {}",
        file.display()
    );
    let recording = Backend::Record {
        file: file.clone(),
        namespace: SUMMARIZER_NAMESPACE.to_string(),
    };
    let lived = live(
        world,
        &Replacement::Summarizer(recording),
        BUILD_PROMPT,
        envelope,
        held,
        started,
    );
    // The fixture folds `CHUNK` presented lines into one segment, so every
    // segment but the newest spans exactly that many messages, from the first
    // message on, and no segment reaches the last message: the daemon
    // protects a tail.
    for (index, segment) in lived.segments.iter().enumerate() {
        let span = segment.end_message - segment.start_message + 1;
        let last = index + 1 == lived.segments.len();
        assert!(
            span == CHUNK as i64 || (last && span < CHUNK as i64),
            "{segment:?}"
        );
        assert_eq!(
            segment.start_message,
            index as i64 * CHUNK as i64 + 1,
            "{segment:?}"
        );
        assert!(
            segment.end_message < world.messages.len() as i64,
            "{segment:?}"
        );
    }
    if lived.refusals > 0 {
        assert!(!file.exists(), "a refused recording writes no cassette");
        return Recording {
            replay: None,
            segments: lived.segments,
            cassette_bytes: 0,
            firings: lived.firings,
            refusals: lived.refusals,
        };
    }
    let cassette: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let frames = cassette["cases"].as_array().unwrap().len();
    assert_eq!(
        frames as u32, lived.firings,
        "every firing was recorded once: {}",
        lived.stderr
    );
    if frames == 0 {
        assert!(lived.segments.is_empty(), "no firing, no segments");
    }
    Recording {
        replay: Some(Backend::Replay {
            file: file.clone(),
            namespace: SUMMARIZER_NAMESPACE.to_string(),
        }),
        segments: lived.segments,
        cassette_bytes: std::fs::metadata(&file).unwrap().len(),
        firings: lived.firings,
        refusals: 0,
    }
}

/// The task asks in the message's own words.
fn prompt(message: &RenderedMessage) -> String {
    text(message).to_string()
}

/// The decision a message records, as the generator defines it.
fn decision(message: &RenderedMessage) -> &str {
    text_decision(text(message))
}

/// Every regular file under the arm's root: the kernel store with its WAL and
/// shm, and the fixture's own files beside it; the control socket has no size.
fn root_bytes(root: &Path) -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(kind) = std::fs::symlink_metadata(path) else {
            return 0;
        };
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

/// A root of its own for one fixture, charged to the envelope while held.
fn occupy(held: &mut Held, envelope: &mut Envelope) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    held.roots += 1;
    envelope.observe(Resource::TempRoots, held.roots).unwrap();
    root
}

/// Releases a root after its fixture exited: the store's bytes are charged as
/// they peaked, then the root goes, and the run's elapsed time is read.
fn vacate(root: tempfile::TempDir, held: &mut Held, envelope: &mut Envelope, started: Instant) {
    envelope
        .observe(Resource::StoreBytes, root_bytes(root.path()))
        .unwrap();
    drop(root);
    held.roots -= 1;
    envelope
        .observe(
            Resource::ElapsedMs,
            u64::try_from(started.elapsed().as_millis()).unwrap(),
        )
        .unwrap();
}

struct ArmRun {
    result: ArmResult,
    verdict: StageVerdict<Surface1Stage>,
    /// The history segments the arm's daemon held when the task ran.
    segments: Vec<StoredHistorySegment>,
}

/// Lives the arm's world through one fixture process under its replacement
/// and reads delivered evidence from the host's own selection on the task's
/// turn. The attempt is timed against the task budgets; the roots, the
/// process, and the store's bytes are charged to the envelope as they peak;
/// the fixture's backend counters prove the task's turn made no model call
/// and a replayed summarizer never reached the controlled backend. A segment
/// stands for every message it covers at every stage up to render; at render
/// the served fragment must carry the message's own words, or the segment
/// reached render with the evidence absent.
fn run_arm(
    world: &World,
    replacement: &Replacement,
    task: &Task,
    profile: &RunProfile,
    envelope: &mut Envelope,
    held: &mut Held,
    started: Instant,
) -> ArmRun {
    assert_eq!(task.evidence.len(), 1, "one truth per task on surface 1");
    let evidence = task.evidence.iter().next().unwrap();
    let message = world
        .messages
        .iter()
        .find(|m| m.event_id == *evidence)
        .expect("the evidence is a rendered message");
    let attempt = Instant::now();
    let lived = live(
        world,
        replacement,
        &prompt(message),
        envelope,
        held,
        started,
    );
    let usage = TaskUsage {
        elapsed_ms: u64::try_from(attempt.elapsed().as_millis()).unwrap(),
        ..TaskUsage::default()
    };
    assert_eq!(
        lived.counters["started"], 0,
        "surface 1 makes no model call, and a replay never reaches the controlled backend: {}",
        lived.counters
    );
    if matches!(replacement, Replacement::Raw) {
        assert!(
            lived.segments.is_empty(),
            "raw history has no segments to serve"
        );
    }
    let covered: BTreeMap<i64, Vec<EventId>> = lived
        .segments
        .iter()
        .map(|segment| {
            let ids = (segment.start_message..=segment.end_message)
                .map(|ordinal| {
                    world.messages[usize::try_from(ordinal - 1).unwrap()]
                        .event_id
                        .clone()
                })
                .collect();
            (segment.sequence, ids)
        })
        .collect();
    let mut identities: BTreeMap<i64, String> = covered
        .iter()
        .map(|(sequence, ids)| {
            let id = ids.iter().find(|id| *id == evidence).unwrap_or(&ids[0]);
            (*sequence, id.0.clone())
        })
        .collect();
    // A truth no segment holds is still the task's truth: it enters the ledger
    // as a unit the store never had (sequences start at one), so the window
    // is where the surface is seen to lack it.
    if !covered.values().any(|ids| ids.contains(evidence)) {
        identities.insert(0, evidence.0.clone());
    }
    if let Some(UserHintPass::Decided(outcome)) = &lived.pass.outcome {
        assert!(
            !outcome.trace.window.contains(&0) && !outcome.trace.selected.contains(&0),
            "the store has no sequence zero"
        );
    }
    let hint_text = match &lived.pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome.hint_text.clone(),
        _ => String::new(),
    };
    let served = |id: &EventId| {
        let message = world
            .messages
            .iter()
            .find(|m| m.event_id == *id)
            .expect("a covered message is rendered");
        carries(&hint_text, decision(message))
    };
    let mut ledger = SurfaceLedger::default();
    observe_rendered(
        &mut ledger,
        lived.pass.outcome.as_ref(),
        &identities,
        |sequence| {
            covered[&sequence]
                .iter()
                .filter(|id| *id == evidence)
                .all(served)
        },
    );
    let delivered: BTreeSet<EventId> = match &lived.pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome
            .trace
            .selected
            .iter()
            .flat_map(|sequence| covered[sequence].iter().cloned())
            .filter(served)
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
    ArmRun {
        result,
        verdict,
        segments: lived.segments,
    }
}

fn sample(pair: &Pair, arm: ArmKind, policy: HistoryPolicy, terminal: Terminal) -> SampleRecord {
    SampleRecord {
        id: format!("{}:{}:{}", pair.task.id, wire_name(arm), wire_name(policy)),
        task: pair.task.id.clone(),
        arm,
        policy,
        cut: Cut::EndOfRun,
        lineage: vec![],
        terminal,
    }
}

/// `part / whole` as a canonical decimal to six places, rounded up so a rate
/// is never understated; zero over zero is zero.
fn decimal_ceil(part: u32, whole: u32) -> String {
    assert!(part <= whole, "{part} of {whole}");
    if whole == 0 {
        return "0".to_string();
    }
    let scaled = (u64::from(part) * 1_000_000).div_ceil(u64::from(whole));
    let fraction = format!("{:06}", scaled % 1_000_000);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        (scaled / 1_000_000).to_string()
    } else {
        format!("{}.{fraction}", scaled / 1_000_000)
    }
}

#[test]
fn a_rate_is_a_canonical_decimal_rounded_up() {
    assert_eq!(decimal_ceil(0, 0), "0");
    assert_eq!(decimal_ceil(0, 3), "0");
    assert_eq!(decimal_ceil(1, 2), "0.5");
    assert_eq!(decimal_ceil(1, 3), "0.333334");
    assert_eq!(decimal_ceil(3, 3), "1");
    assert!(Ratio::from_decimal(&decimal_ceil(2, 7)).is_some());
}

/// The snake-case wire name of a unit variant, for sample ids.
fn wire_name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn terminal_of(result: ArmResult) -> Terminal {
    match result {
        ArmResult::Pass => Terminal::Pass,
        ArmResult::Fail => Terminal::Fail,
        ArmResult::Censored(reason) => Terminal::Censored { reason },
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
    let unapproved = profile(Scale::S0, 512, 1_200_000, None);
    assert_eq!(
        unapproved.approved(),
        Err(ProfileError::NotApproved {
            name: "s0-surface1-raw".to_string(),
        })
    );
}

/// One campaign at the given scale: compile, drive both arms of every pair
/// through the fixture, analyze, gate, publish, and read the report back.
/// `elapsed_ms` is the run's wall-clock bound; the envelope refuses the
/// first reading past it rather than reporting an overrun afterwards.
fn campaign(scale: Scale, aged_messages: u32, elapsed_ms: u64) -> SuiteBReport {
    let started = Instant::now();
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let max_events_per_log = aged_messages.max(64) * 2;
    let profile = profile(
        scale,
        max_events_per_log,
        elapsed_ms,
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
    // Twelve messages: long enough that the hint scorer's rarity rule has a
    // pool to judge against once the summarizer folds them into segments.
    let natural_fresh = generate_all(
        SEED ^ 0x77,
        &one_session(12, max_events_per_log),
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

    let aged_world = world(&set.aged);
    assert_eq!(aged_world.messages.len(), aged_messages as usize);
    let cassettes = occupy(&mut held, &mut envelope);
    let aged_recording = record(
        "aged",
        &aged_world,
        cassettes.path(),
        &mut envelope,
        &mut held,
        started,
    );
    assert!(
        aged_recording.firings > 0,
        "the aged history reaches the pressure the summarizer fires at"
    );
    let mut cassette_bytes = aged_recording.cassette_bytes;
    envelope
        .observe(Resource::CassetteBytes, cassette_bytes)
        .unwrap();
    // Refusals over firings, per arm, as the lives observed them.
    let mut fired: BTreeMap<&str, (u32, u32)> =
        BTreeMap::from([("aged", (0, 0)), ("fresh", (0, 0))]);
    let mut observe_rates = |arm: &'static str, recording: &Recording| {
        let (firings, refusals) = fired.get_mut(arm).unwrap();
        *firings += recording.firings;
        *refusals += recording.refusals;
    };
    observe_rates("aged", &aged_recording);
    let mut structured_outcomes: Vec<PairOutcome> = Vec::new();
    let mut outcomes = Vec::new();
    let mut samples = BTreeMap::new();
    let mut order = Vec::new();
    let mut verdicts: BTreeMap<(&str, &str), StageVerdict<Surface1Stage>> = BTreeMap::new();
    for pair in &set.pairs {
        let fresh_world = world(&pair.fresh);
        assert!(
            fresh_world.messages.len() < aged_world.messages.len(),
            "the control is short"
        );
        let aged_run = run_arm(
            &aged_world,
            &Replacement::Raw,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
        let fresh_run = run_arm(
            &fresh_world,
            &Replacement::Raw,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
        let fresh_recording = record(
            &format!("fresh-{}", pair.task.id),
            &fresh_world,
            cassettes.path(),
            &mut envelope,
            &mut held,
            started,
        );
        observe_rates("fresh", &fresh_recording);
        // Twelve turns never reach the pressure the summarizer fires at, so
        // the short control's structured arm is its raw history.
        assert_eq!(
            (fresh_recording.firings, fresh_recording.segments.len()),
            (0, 0),
            "the summarizer leaves the short control raw"
        );
        cassette_bytes += fresh_recording.cassette_bytes;
        envelope
            .observe(Resource::CassetteBytes, cassette_bytes)
            .unwrap();
        // A structured arm runs only under a cassette; a refused recording
        // leaves none, and the arm's samples are skipped as refused.
        let aged_structured_run = aged_recording.replay.as_ref().map(|replay| {
            let run = run_arm(
                &aged_world,
                &Replacement::Summarizer(replay.clone()),
                &pair.task,
                &profile,
                &mut envelope,
                &mut held,
                started,
            );
            // The replayed life publishes what the recorded one did, task
            // after task: the segments are the daemon's, reproduced from the
            // cassette.
            assert_eq!(
                run.segments.iter().map(timeless).collect::<Vec<_>>(),
                aged_recording
                    .segments
                    .iter()
                    .map(timeless)
                    .collect::<Vec<_>>(),
                "the replay publishes the recording's segments"
            );
            run
        });
        let fresh_structured_run = fresh_recording.replay.as_ref().map(|replay| {
            run_arm(
                &fresh_world,
                &Replacement::Summarizer(replay.clone()),
                &pair.task,
                &profile,
                &mut envelope,
                &mut held,
                started,
            )
        });
        // Samples in the order the arms ran: raw, then structured; the pruned
        // arms, never attempted, are declared last.
        for (kind, label, run) in [
            (ArmKind::Aged, "aged", &aged_run),
            (ArmKind::Fresh, "fresh", &fresh_run),
        ] {
            let record = sample(pair, kind, HistoryPolicy::Raw, terminal_of(run.result));
            order.push(record.id.clone());
            samples.insert(record.id.clone(), record);
            verdicts.insert((pair.task.id.as_str(), label), run.verdict);
        }
        for (kind, label, run) in [
            (ArmKind::Aged, "aged/structured", &aged_structured_run),
            (ArmKind::Fresh, "fresh/structured", &fresh_structured_run),
        ] {
            let terminal = match run {
                Some(run) => terminal_of(run.result),
                None => Terminal::Skipped(SkipReason::RedactionRefused),
            };
            let record = sample(pair, kind, HistoryPolicy::Structured, terminal);
            order.push(record.id.clone());
            samples.insert(record.id.clone(), record);
            if let Some(run) = run {
                verdicts.insert((pair.task.id.as_str(), label), run.verdict);
            }
        }
        for kind in [ArmKind::Aged, ArmKind::Fresh] {
            // The pruned policy reclaims projection rows; surface 1 reads
            // history segments, so the arm is declared and not attempted.
            let pruned = sample(
                pair,
                kind,
                HistoryPolicy::Pruned,
                Terminal::Unsupported(UnsupportedReason::PolicyNotOnSurface {
                    policy: HistoryPolicy::Pruned,
                    surface: EvaluatedSurface::Surface1,
                }),
            );
            order.push(pruned.id.clone());
            samples.insert(pruned.id.clone(), pruned);
        }
        if let (Some(aged), Some(fresh)) = (&aged_structured_run, &fresh_structured_run) {
            structured_outcomes.push(PairOutcome {
                pair_id: pair.task.id.clone(),
                cluster: ClusterKey {
                    family: "generated".to_string(),
                    world_seed: SEED,
                },
                fresh: fresh.result,
                aged: aged.result,
            });
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

    // Surface 1 serves history segments and nothing else, and only the
    // summarizer writes them. Under raw history no arm has a unit for any
    // truth, so every task is lost at the candidate window on both arms. The
    // short control never reaches the pressure the summarizer fires at, so its
    // structured arm is as empty. On the structured aged arm the daemon's
    // summarizer folded the older history five messages to a segment: a truth
    // at the head of its segment is served whole, a truth folded past the
    // fragment cap reaches render with its words cut off, and a truth in the
    // protected tail has no unit at all.
    let by_task: BTreeMap<&str, &PairOutcome> =
        outcomes.iter().map(|o| (o.pair_id.as_str(), o)).collect();
    let structured_by_task: BTreeMap<&str, &PairOutcome> = structured_outcomes
        .iter()
        .map(|o| (o.pair_id.as_str(), o))
        .collect();
    for pair in &set.pairs {
        let name = pair.task.id.as_str();
        assert_eq!(
            (by_task[name].aged, by_task[name].fresh),
            (ArmResult::Fail, ArmResult::Fail),
            "{name} raw"
        );
        // With no segment at all the window cannot hold the truth; this pins
        // that the three hint gates passed and the store held no unit, not
        // that the window itself selected anything.
        for label in ["aged", "fresh", "fresh/structured"] {
            assert_eq!(
                verdicts[&(name, label)],
                StageVerdict::FirstLoss(Surface1Stage::CandidateWindow),
                "{name} {label}"
            );
        }
    }
    let refused_aged = aged_recording.replay.is_none();
    // The messages each of the recording's segments covers, by sequence; every
    // replayed arm published the same segments.
    let aged_structured_covered: BTreeMap<i64, Vec<EventId>> = aged_recording
        .segments
        .iter()
        .map(|segment| {
            let ids = (segment.start_message..=segment.end_message)
                .map(|ordinal| {
                    aged_world.messages[usize::try_from(ordinal - 1).unwrap()]
                        .event_id
                        .clone()
                })
                .collect();
            (segment.sequence, ids)
        })
        .collect();
    assert_eq!(
        refused_aged,
        aged_recording.refusals > 0,
        "a recording is refused exactly when a frame was"
    );
    assert_eq!(
        structured_outcomes.len(),
        if refused_aged { 0 } else { set.pairs.len() },
        "structured pairs are judged only when both arms ran"
    );
    for pair in set.pairs.iter().filter(|_| !refused_aged) {
        let name = pair.task.id.as_str();
        assert_eq!(structured_by_task[name].fresh, ArmResult::Fail, "{name}");
        let evidence = pair.task.evidence.iter().next().unwrap();
        let covering = aged_structured_covered
            .values()
            .find(|ids| ids.contains(evidence));
        let expected = match covering {
            None => (
                ArmResult::Fail,
                StageVerdict::FirstLoss(Surface1Stage::CandidateWindow),
            ),
            Some(ids) if ids[0] == *evidence => (ArmResult::Pass, StageVerdict::Clean),
            Some(_) => (
                ArmResult::Fail,
                StageVerdict::FirstLoss(Surface1Stage::Render),
            ),
        };
        assert_eq!(
            (
                structured_by_task[name].aged,
                verdicts[&(name, "aged/structured")]
            ),
            expected,
            "{name} aged/structured"
        );
    }
    let folded = |name: &str| {
        let evidence = set
            .pairs
            .iter()
            .find(|p| p.task.id == name)
            .unwrap()
            .task
            .evidence
            .iter()
            .next()
            .unwrap();
        aged_structured_covered
            .values()
            .find(|ids| ids.contains(evidence))
            .map(|ids| ids.iter().position(|id| id == evidence).unwrap())
    };
    // The falsifier is the third message, folded third into the first
    // segment; the positive control is the last message, in the protected
    // tail; the plain task sits at the head of its segment at S0.
    if !refused_aged {
        assert_eq!(folded("early-message"), Some(2));
        assert_eq!(folded("last-message"), None);
    }
    if aged_messages == AGED_MESSAGES {
        assert!(!refused_aged, "S0's firings carry no refused frame");
        assert_eq!(
            (aged_recording.firings, aged_recording.segments.len()),
            (4, 20),
            "the trigger fires four times over the life: projected headroom, then the force band"
        );
        assert_eq!(folded("recent-message"), Some(0));
    }
    let arms = GovernanceArms {
        control_run_id: "ee".repeat(32),
        task_ids: set.pairs.iter().map(|p| p.task.id.clone()).collect(),
        evidence_ids: set
            .pairs
            .iter()
            .flat_map(|p| p.task.evidence.iter().cloned())
            .collect(),
        arms: BTreeMap::from([
            (
                HistoryPolicy::Raw,
                ArmRecord {
                    policy_version: "raw/v1".to_string(),
                    absent_evidence: BTreeSet::new(),
                },
            ),
            (
                HistoryPolicy::Pruned,
                ArmRecord {
                    policy_version: "message_cleanup/v1".to_string(),
                    absent_evidence: BTreeSet::new(),
                },
            ),
            (
                HistoryPolicy::Structured,
                ArmRecord {
                    policy_version: format!("history_summarizer/fixture-summarizer/chunk-{CHUNK}"),
                    absent_evidence: BTreeSet::new(),
                },
            ),
        ]),
    };
    arms.validate(&set).unwrap();

    let ledger = SampleLedger {
        epoch: 1,
        order,
        samples,
    };
    let rates = ledger.rates().unwrap();
    let skipped = if refused_aged { set.pairs.len() } else { 0 };
    assert_eq!((rates.samples, ledger.attempted()), (18, 12 - skipped));
    assert_eq!(rates.unsupported, Ratio::new(1, 3));
    // Surface 1 made no model call on any arm (the fixture's counters) and a
    // replayed frame that missed would be a recorded failure and no segments,
    // so the miss rates are zero by observation; the refusal rates are the
    // cassette's refusals over the summarizer's firings, as the lives saw
    // them.
    let arm_rates: BTreeMap<String, ArmRates> = fired
        .iter()
        .map(|(arm, (firings, refusals))| {
            (
                arm.to_string(),
                ArmRates {
                    miss_rate: "0".to_string(),
                    refusal_rate: decimal_ceil(*refusals, *firings),
                },
            )
        })
        .collect();
    let family = family(&profile);
    let frozen = FrozenFamily::freeze(&family).unwrap();
    let Analysis::Report(analysis) = analyze(&frozen, &family, &outcomes, &arm_rates).unwrap()
    else {
        panic!("three pairs report, with the interval withheld");
    };
    // Both raw arms are inert, so the pairs are concordant and the paired
    // gates see no loss; the floor, which asks the control to deliver at all,
    // is what fails.
    assert_eq!(
        (analysis.counts.n, analysis.counts.b, analysis.counts.c),
        (3, 0, 0)
    );
    assert!(
        analysis.gates.quality_loss.passed && analysis.gates.harm.passed,
        "{:?}",
        analysis.gates
    );
    assert!(
        !analysis.gates.floor.passed,
        "an inert control fails the floor: {:?}",
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
        .chain(&structured_outcomes)
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
    assert_eq!(peaks["temp_roots"], 3);
    assert!(peaks["cassette_bytes"].as_u64().unwrap() > 0);

    // The manifest beside the report says how the world reached the store:
    // every arm was lived through the daemon's own transform route one turn
    // at a time in one store incarnation, so the run is `replay` over
    // `transform-route, turn by turn`.
    let manifest = manifest(&profile, &set, &report, &bytes, &frozen, started_at_ms);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    write_then_rename(&publish.path().join("manifest.json"), &manifest_bytes);
    let read_back: serde_json::Value =
        serde_json::from_slice(&std::fs::read(publish.path().join("manifest.json")).unwrap())
            .unwrap();
    let parsed = parse_manifest(&read_back).unwrap();
    assert_eq!(parsed, manifest);
    assert_eq!(parsed.digest().unwrap(), manifest.digest().unwrap());
    // A seeded history may not call itself aged: the same manifest relabelled
    // as written straight into the store is refused.
    let mut relabelled = read_back.clone();
    relabelled["ingestion"] = serde_json::json!("direct-database, non-aged");
    assert_eq!(
        parse_manifest(&relabelled),
        Err(eval_core::ManifestError::DirectDatabaseAged)
    );
    report
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn command(program: &str, args: &[&str]) -> String {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(support::direct_host::workspace_root())
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(output.status.success(), "{program} {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// The run's manifest: the identity of this checkout and toolchain, the
/// profile and scenario the run was declared under, every sample in the order
/// it ran, the report's digest as its result, the pair set's digest as its
/// witness, and the envelope as bounded and as peaked.
fn manifest(
    profile: &RunProfile,
    set: &PairSet,
    report: &SuiteBReport,
    report_bytes: &[u8],
    frozen: &FrozenFamily,
    started_at_ms: i64,
) -> Manifest {
    let dirty = !command("git", &["status", "--porcelain"]).is_empty();
    let lockfile =
        std::fs::read(support::direct_host::workspace_root().join("Cargo.lock")).unwrap();
    let identity = RunIdentity {
        build: BuildRecord {
            code_sha: command("git", &["rev-parse", "HEAD"]),
            dirty,
            lockfile_digest: sha256_hex(&lockfile),
            rustc_version: command("rustc", &["--version"]),
            features: BTreeSet::from(["test-support".to_string()]),
            target_triple: command("rustc", &["-vV"])
                .lines()
                .find_map(|line| line.strip_prefix("host: "))
                .expect("rustc names its host")
                .to_string(),
            binary_digest: BinaryDigest::Present {
                sha256: sha256_hex(&std::fs::read(fixture_binary()).unwrap()),
            },
        },
        simulator_version: "eval-campaign-shell/v1".to_string(),
        config: serde_json::to_value(profile).unwrap(),
        scenario: serde_json::json!({
            "surface": report.surface,
            "tasks": set.pairs.iter().map(|p| p.task.id.clone()).collect::<Vec<_>>(),
            "aged_messages": set.aged.events.len(),
        }),
        root_seed: SEED,
        random_schema_version: RANDOM_SCHEMA_VERSION.to_string(),
        generator_version: GENERATOR_VERSION.to_string(),
        eligibility_spec_digest: ELIGIBILITY_SPEC_DIGEST.to_string(),
        linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
    };
    let set_value = serde_json::to_value(set).unwrap();
    Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        run_identity: identity,
        start_ms: started_at_ms,
        end_ms: started_at_ms + i64::try_from(report.envelope.peaks.elapsed_ms).unwrap(),
        status: RunStatus::Completed,
        error: None,
        sample_ids: report.samples.samples.keys().cloned().collect(),
        sample_order: report.samples.order.clone(),
        sample_epoch: report.samples.epoch,
        retry_lineage: Vec::new(),
        result_digest: sha256_hex(report_bytes),
        witness_digest: context_core::canonical_json::protocol_digest(
            "eval-campaign-witness/v1",
            &set_value,
        )
        .unwrap(),
        attestation: Attestation::None,
        tokenizer_profile: TokenizerProfile {
            name: "none".to_string(),
            revision: "surface-1-hint-lexical".to_string(),
            digest: sha256_hex(b"surface-1-hint-lexical"),
        },
        cut_receipts: vec![CutReceipt {
            cut: Cut::EndOfRun,
            outcome: CutOutcome::Reached,
        }],
        residue: Manifest::field_schema().residue().collect(),
        construction: Construction::Replay,
        execution_mode: ExecutionMode::Generate,
        failure_class_table_digest: FAILURE_CLASS_TABLE_DIGEST.to_string(),
        ingestion: Ingestion::TransformRouteTurnByTurn,
        memory_reviewer_model_calls: MemoryReviewerModelCalls::Excluded,
        analysis_family_digest: Some(frozen.analysis_family_digest.clone()),
        recency_baseline: Some(RecencyBaseline {
            version: eval_core::RECENCY_BASELINE_VERSION.to_string(),
            bounds: profile.baseline_bounds.clone(),
        }),
        reachability: Reachability::DefaultProduction,
        claim_boundary: ClaimBoundary::pinned(),
        component_versions: ComponentVersions {
            generator: GENERATOR_VERSION.to_string(),
            event_schema: EVENT_SCHEMA_VERSION.to_string(),
            reducer: REDUCER_VERSION.to_string(),
            oracles: PAIRING_POLICY_VERSION.to_string(),
            execution_image: "direct_host_fixture".to_string(),
            task_corpus: format!("generated:{SEED:#x}"),
            judge: "none".to_string(),
        },
        envelope_bounds: report.envelope.bounds.clone(),
        envelope_peaks: report.envelope.peaks.clone(),
        arm_rates: report.arm_rates.clone(),
    }
}

#[test]
fn an_s0_campaign_on_the_default_surface_publishes_one_gated_report() {
    let report = campaign(Scale::S0, AGED_MESSAGES, S0_ELAPSED_BOUND_MS);
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
    // The budget is the profile's elapsed bound, so an overrun is refused by
    // the envelope at the reading that crosses it.
    let report = campaign(Scale::S1, S1_AGED_MESSAGES, budget_ms);
    assert!(report.envelope.peaks.elapsed_ms <= budget_ms);
    assert_eq!(report.profile_name, "s1-surface1-raw");
    // Over 400 turns the daemon's summarizer fires more than at S0, and one
    // of its prompts draws a calibration example from the daemon's own seed
    // corpus that the secret scanner reads as a key, so the cassette refuses
    // the frame: the aged structured arm has no recording, its three samples
    // are skipped as refused, the aged arm's refusal rate is on the report,
    // and the refusal gate fails at the profile's ceiling of zero.
    let skipped = report
        .samples
        .samples
        .values()
        .filter(|s| matches!(s.terminal, Terminal::Skipped(SkipReason::RedactionRefused)))
        .count();
    assert_eq!(skipped, 3, "{:?}", report.samples.order);
    assert_ne!(report.arm_rates["aged"].refusal_rate, "0");
    assert_eq!(report.arm_rates["fresh"].refusal_rate, "0");
    let ReportOutcome::Open { gated } = &report.outcome else {
        panic!("{:?}", report.outcome);
    };
    assert!(!gated.gates.redaction_refusals.passed, "{:?}", gated.gates);
}
