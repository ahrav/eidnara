//! The Suite B campaign shell: compiled pairs, both arms of every pair driven
//! through the direct-host fixture under the raw and structured history
//! policies, the baseline contrast, the run gates, and one report with its
//! manifest published write-then-rename, all under an approved profile and
//! inside a declared envelope. The `eval_runner` example drives it from the
//! command line; the daemon's campaign test drives it in-process and asserts
//! what a run found.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;

use daemon::transform::UserHintPass;
use eval_core::{
    ANALYSIS_FAMILY_SCHEMA, Analysis, AnalysisFamily, Approval, ArmKind, ArmRates, ArmRecord,
    ArmResult, Attestation, Baseline, BaselineVerdict, BinaryDigest, BuildRecord, CampaignGates,
    CampaignProfile, Carrier, ClaimBoundary, Claims, ClusterKey, ClusteringUnit, ComponentVersions,
    Construction, Cut, CutOutcome, CutReceipt, Destination, ELIGIBILITY_SPEC_DIGEST,
    EVENT_SCHEMA_VERSION, Envelope, EnvelopeExceeded, Established, EvaluatedSurface, EventId,
    EventLog, ExecutionMode, FAILURE_CLASS_TABLE_DIGEST, FrozenFamily, GENERATOR_VERSION,
    GatedBlocks, GovernanceArms, HistoryPolicy, IccPilot, Ingestion, InjectionCase,
    InjectionObservation, InjectionScore, IntervalMethod, LINEARIZATION_RULE_VERSION,
    LivenessBounds, MANIFEST_SCHEMA, Manifest, MemoryReviewerModelCalls, Mode,
    MultiplicityCorrection, PAIRING_POLICY_VERSION, Pair, PairOutcome, PairSet, PairSetInput,
    Planted, ProfileError, Query, RANDOM_SCHEMA_VERSION, REDUCER_VERSION, RUN_PROFILE_SCHEMA,
    Ratio, Reachability, RecencyBaseline, RenderConfig, RenderedMessage, ReportOutcome,
    RepositorySpec, Required, Resource, ResourceLimits, RunIdentity, RunProfile, RunStatus,
    SUITE_B_REPORT_SCHEMA, SampleLedger, SampleRecord, Scale, Sensitivity, ServedClass,
    SessionSpec, SkipReason, StageValue, StageVerdict, StoppingRule, SuiteBReport, Surface1Stage,
    Task, TaskBudgets, TaskRole, TaskUsage, Terminal, TokenizerProfile, UnsupportedReason,
    Visibility, WorldConfig, WorldProvenance, analyze, check_recency_baseline, compile_pair_set,
    eval_run_id, generate_all, pair_table_digest, plan_injection_cases, render, score_injection,
    serialize_spec, text_decision,
};
use memory_store::{MemoryStore, StoredHistorySegment};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::support::direct_host::{Backend, Launch, fixture_binary};
use super::support::eval_surface::{
    EPOCH_MS, Knobs, Pass, SurfaceLedger, World, block_on, drain, lifecycle, observe_rendered,
    pass, text,
};
use super::support::publish::{staged_path, write_then_rename};

pub const SEED: u64 = 0x5EED_B000_0000_0002;
/// The campaign's one world, as its pairs' cluster key names it: a canonical
/// JSON number cannot carry the generator's 64-bit seed, so the world is
/// named by ordinal, and the manifest carries the seed itself.
const WORLD: u64 = 0;
const PROJECT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SESSION: &str = "session-0";
/// Messages in the natural-fresh control: long enough that the hint scorer's
/// rarity rule has a pool to judge against, short enough to sit inside every
/// surface's recency window.
pub const FRESH_MESSAGES: u32 = 12;

/// One campaign's inputs: the scale and its aged history's length, the
/// elapsed bound the envelope enforces, the approval the profile carries (or
/// none, which the profile refuses), and the directory the report and
/// manifest are published into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scale: Scale,
    pub aged_messages: u32,
    pub elapsed_bound_ms: u64,
    pub approval: Option<Approval>,
    pub publish: PathBuf,
}

/// Why a campaign did not run to a report: the profile refused, the aged
/// history is too short to hold a falsifier outside the window and a plain
/// task inside it, or the envelope refused a reading. A fixture breaking its
/// contract with the shell is a panic, not one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Profile(ProfileError),
    AgedHistoryTooShort {
        aged_messages: u32,
        window: u32,
    },
    /// Twice the message count, the event bound the profile declares, does
    /// not fit a `u32`.
    AgedHistoryTooLong {
        aged_messages: u32,
    },
    /// The publish directory could not be created, or a staged file already
    /// sits where the create-new publisher stages its own.
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
    Envelope(EnvelopeExceeded),
}

impl From<ProfileError> for RunError {
    fn from(error: ProfileError) -> Self {
        Self::Profile(error)
    }
}

impl From<EnvelopeExceeded> for RunError {
    fn from(error: EnvelopeExceeded) -> Self {
        Self::Envelope(error)
    }
}

/// What the aged world's recorded life established: how often the daemon's
/// summarizer fired, whether a frame was refused, and the messages each of
/// its segments covers, by sequence.
pub struct RecordedLife {
    pub firings: u32,
    pub refused: bool,
    pub covered: BTreeMap<i64, Vec<EventId>>,
}

/// One campaign as run: the published report and manifest with their bytes,
/// the pair set, every arm's stage verdict by task and arm label, the pair
/// outcomes per policy, and the aged world's recorded life.
pub struct Run {
    pub report: SuiteBReport,
    pub report_bytes: Vec<u8>,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub set: PairSet,
    pub verdicts: BTreeMap<(String, &'static str), StageVerdict<Surface1Stage>>,
    pub outcomes: Vec<PairOutcome>,
    pub structured_outcomes: Vec<PairOutcome>,
    pub aged: RecordedLife,
}

fn one_session(
    messages: u32,
    tool_span_every: u32,
    commits: u32,
    max_events_per_log: u32,
    planted: Vec<Planted>,
) -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages,
            tool_span_every,
            correction_every: 0,
            invalidation_every: 0,
        }],
        repositories: (commits > 0)
            .then_some(RepositorySpec {
                commits,
                rename_every: 0,
            })
            .into_iter()
            .collect(),
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log,
        planted,
    }
}

/// Every tenth message of the aged history carries a tool span, as the
/// harness sends a completed tool call and its result.
const AGED_TOOL_SPAN_EVERY: u32 = 10;
/// The aged world's repository: commits the session's messages cite, one of
/// which carries the commit-message carrier's canary. Surface 1 reads no
/// commit, so the shell presents none to the daemon.
const AGED_COMMITS: u32 = 5;
const PLANTED_COMMIT_SLOT: u32 = 2;
/// The slot of the aged history the summary carrier's canary is planted on:
/// deep enough that the summarizer folds it at every scale, past the
/// falsifier's segment. The tool-output carrier's goes on the tool span just
/// before it.
pub const PLANTED_SLOT: u32 = 60;
pub const PLANTED_TOOL_SLOT: u32 = PLANTED_SLOT - 1;

/// The carriers this campaign plants into its aged world: a message's text
/// for the summary carrier and a tool span's output for the tool-output
/// carrier, both presented to the daemon turn by turn, and a commit's message
/// for the commit carrier, which surface 1 never reads. The generated world
/// has no issue and no memory payload, so those two carriers are planted
/// nowhere.
fn planted(cases: &[InjectionCase]) -> Vec<Planted> {
    cases
        .iter()
        .filter_map(|case| {
            let (entity, slot) = match case.carrier {
                Carrier::Summary => (SESSION, PLANTED_SLOT),
                Carrier::ToolOutput => (SESSION, PLANTED_TOOL_SLOT),
                Carrier::CommitMessage => ("repository-0", PLANTED_COMMIT_SLOT),
                Carrier::IssueText | Carrier::Memory => return None,
            };
            Some(Planted {
                carrier: case.carrier,
                entity: entity.to_string(),
                slot,
                canary: case.canary.clone(),
            })
        })
        .collect()
}

pub fn profile(
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

/// The task families the analysis registers: the campaign's two generated
/// histories, sorted as the family lists them. Every task is drawn from the
/// aged one, so every pair lies in `TASK_FAMILY`; the fresh control is
/// registered because a pilot samples at least two families.
const TASK_FAMILIES: [&str; 2] = ["generated/aged", "generated/fresh"];
const TASK_FAMILY: &str = TASK_FAMILIES[0];

fn ratio(numerator: i128, denominator: i128) -> Ratio {
    Ratio::try_new(numerator, denominator).unwrap()
}

/// The frozen family the campaign is read under: its table is the profile's
/// tasks over its worlds, one pair each, and its pilot is declared, not run:
/// the two histories' tasks, one world each, with no correlation at either
/// level, so the affordable worlds' items are its effective N and the
/// required N, which the plan meets exactly.
fn family(profile: &RunProfile) -> AnalysisFamily {
    let families: Vec<String> = TASK_FAMILIES.iter().map(|f| f.to_string()).collect();
    let n_families = u32::try_from(TASK_FAMILIES.len()).unwrap();
    let pairs = profile.worlds * profile.tasks_per_world;
    AnalysisFamily {
        schema: ANALYSIS_FAMILY_SCHEMA.to_string(),
        endpoints: vec!["quality_loss".into(), "harm".into(), "floor".into()],
        families: families.clone(),
        exclusions: vec![],
        stopping_rule: StoppingRule::FixedN { pairs },
        multiplicity_correction: MultiplicityCorrection::None,
        profile: profile.statistics.clone(),
        interval_method: IntervalMethod::ClusterBootstrap,
        item_count_threshold: 300,
        bootstrap_replicates: 40,
        bootstrap_seed: 7,
        trials_k: 3,
        icc_pilot: IccPilot {
            pilot_run_id: "ab".repeat(32),
            families,
            n_items: n_families * profile.tasks_per_world,
            n_families,
            n_worlds: n_families,
            icc_family: Ratio::ZERO,
            icc_world_seed: Ratio::ZERO,
            clustering_unit: ClusteringUnit::WorldSeed,
            max_affordable_worlds: profile.worlds,
            effective_n_at_max: ratio(
                i128::from(n_families * profile.tasks_per_world * profile.worlds),
                i128::from(n_families),
            ),
            required_n_for_margin: pairs,
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
/// The three tasks' ids, fixed before the world exists so the injection cases
/// planted into it can be planned from them.
const TASK_IDS: [&str; 3] = ["early-message", "last-message", "recent-message"];

fn tasks(aged: &EventLog, window: u32, max_events_per_log: u32) -> Vec<Task> {
    let messages: Vec<&EventId> = aged
        .events
        .iter()
        .filter(|event| matches!(event.payload, eval_core::Payload::Message { .. }))
        .map(|event| &event.id)
        .collect();
    let task = |name: &str, role, id: &EventId| Task {
        id: name.to_string(),
        role,
        query: query(max_events_per_log),
        evidence: BTreeSet::from([id.clone()]),
    };
    let inside = messages.len() - (window / 2) as usize;
    vec![
        task("early-message", TaskRole::Falsification, messages[2]),
        task(
            "last-message",
            TaskRole::PositiveControl,
            messages[messages.len() - 1],
        ),
        task("recent-message", TaskRole::Plain, messages[inside]),
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
    charges: &mut Charges,
) -> Result<Lived, EnvelopeExceeded> {
    let home = charges.occupy()?;
    let root = charges.occupy()?;
    let mut launch = Launch::at(root.path().to_path_buf()).config_home(home.path());
    if let Replacement::Summarizer(backend) = replacement {
        write_summarizer_config(home.path());
        launch = launch.backend(backend.clone());
    }
    let fixture = launch.start();
    charges.process_started()?;
    let knobs = Knobs {
        usage: usage(world.messages.len() + 1),
        ..Knobs::default()
    };
    let (turns, pass) = block_on(async {
        let turns = lifecycle(&fixture, world, usage).await;
        let pass = pass(&fixture, world, prompt, &knobs).await;
        (turns, pass)
    });
    // The task turn drains like every lifecycle turn: a firing it spawned
    // finishes before its diagnostics, the counters, and the store are read.
    drain(&fixture);
    // The store is read while the fixture holds it: closing the last
    // connection checkpoints the WAL away, so the root after shutdown is the
    // smaller reading.
    charges.observe(Resource::StoreBytes, root_bytes(root.path()))?;
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
            assert_eq!(backend_calls, 0, "{counters}");
        }
    }
    // A turn's diagnostics describe the firings before it, so a refusal on
    // the last firing is in the cassette's counter and in no snapshot; a
    // failure the snapshots do show must be a refusal the counter has.
    assert!(
        !failures_seen || refusals > 0,
        "the daemon reports a refused frame only when the cassette refused one: {counters}"
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
    charges.process_ended();
    let descriptor = daemon::managed_store_descriptor(root.path()).unwrap();
    let store = MemoryStore::open(&descriptor).unwrap();
    let segments = store.load_history_segments(&world.session).unwrap();
    drop(store);
    charges.vacate(root)?;
    charges.vacate(home)?;
    Ok(Lived {
        segments,
        pass,
        counters,
        firings,
        refusals,
        stderr: output.stderr,
    })
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
    charges: &mut Charges,
) -> Result<Recording, EnvelopeExceeded> {
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
        charges,
    )?;
    if lived.refusals > 0 {
        assert!(!file.exists(), "a refused recording writes no cassette");
        return Ok(Recording {
            replay: None,
            segments: lived.segments,
            cassette_bytes: 0,
            firings: lived.firings,
            refusals: lived.refusals,
        });
    }
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
    Ok(Recording {
        replay: Some(Backend::Replay {
            file: file.clone(),
            namespace: SUMMARIZER_NAMESPACE.to_string(),
        }),
        segments: lived.segments,
        cassette_bytes: std::fs::metadata(&file).unwrap().len(),
        firings: lived.firings,
        refusals: 0,
    })
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

/// The run's envelope with what it holds at once, so a reading is a live
/// count rather than each acquisition as one, and the clock the elapsed bound
/// is read against. Every charge is a reading the envelope may refuse.
struct Charges {
    envelope: Envelope,
    roots: u64,
    processes: u64,
    started: Instant,
}

impl Charges {
    fn new(bounds: ResourceLimits) -> Self {
        Self {
            envelope: Envelope::new(bounds),
            roots: 0,
            processes: 0,
            started: Instant::now(),
        }
    }

    fn observe(&mut self, resource: Resource, observed: u64) -> Result<(), EnvelopeExceeded> {
        self.envelope.observe(resource, observed)
    }

    /// A root of its own for one fixture, charged while held.
    fn occupy(&mut self) -> Result<tempfile::TempDir, EnvelopeExceeded> {
        let root = tempfile::tempdir().unwrap();
        self.roots += 1;
        self.observe(Resource::TempRoots, self.roots)?;
        Ok(root)
    }

    /// Releases a root after its fixture exited: the store's bytes are charged
    /// once more as the checkpoint left them, then the root goes, and the
    /// run's elapsed time is read.
    fn vacate(&mut self, root: tempfile::TempDir) -> Result<(), EnvelopeExceeded> {
        self.observe(Resource::StoreBytes, root_bytes(root.path()))?;
        drop(root);
        self.roots -= 1;
        self.elapsed()
    }

    fn process_started(&mut self) -> Result<(), EnvelopeExceeded> {
        self.processes += 1;
        self.observe(Resource::Processes, self.processes)
    }

    fn process_ended(&mut self) {
        self.processes -= 1;
    }

    fn elapsed(&mut self) -> Result<(), EnvelopeExceeded> {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap();
        self.observe(Resource::ElapsedMs, elapsed)
    }
}

struct ArmRun {
    result: ArmResult,
    verdict: StageVerdict<Surface1Stage>,
    /// The history segments the arm's daemon held when the task ran, and the
    /// sequences the host selected for the task's prompt.
    segments: Vec<StoredHistorySegment>,
    selected: Vec<i64>,
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
    charges: &mut Charges,
) -> Result<ArmRun, EnvelopeExceeded> {
    assert_eq!(task.evidence.len(), 1, "one truth per task on surface 1");
    let evidence = task.evidence.iter().next().unwrap();
    let message = world
        .messages
        .iter()
        .find(|m| m.event_id == *evidence)
        .expect("the evidence is a rendered message");
    let attempt = Instant::now();
    let lived = live(world, replacement, &prompt(message), charges)?;
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
    let covered = covered(&lived.segments, world);
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
    let selected = match &lived.pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome.trace.selected.clone(),
        _ => Vec::new(),
    };
    Ok(ArmRun {
        result,
        verdict,
        segments: lived.segments,
        selected,
    })
}

/// The task set's five injection cases, scored by what this campaign
/// observed of them. The summary and tool-output carriers' canaries were
/// planted into a message and a tool span of the aged history: each is
/// ingested when the daemon's own summarizer folded it into a segment (the
/// recorded life's segments carry it), retrieved when the host selected such
/// a segment for a task's prompt on the structured aged arm, and never
/// packed, quoted, or obeyed on surface 1, which has no packing, no model
/// output, and no mediation boundary. The other carriers were planted
/// nowhere, and the commit carrier's canary is in a commit surface 1 never
/// reads and the shell never presents, so their cases read `not_reached`.
fn injection_scores(
    cases: &[InjectionCase],
    recorded: &[StoredHistorySegment],
    structured_aged: &[ArmRun],
) -> Vec<InjectionScore> {
    cases
        .iter()
        .map(|case| {
            let carries = |segment: &StoredHistorySegment| segment.content.contains(&case.canary);
            let (ingested, retrieved) = match case.carrier {
                Carrier::Summary | Carrier::ToolOutput => {
                    let ingested = recorded.iter().any(carries);
                    let retrieved = structured_aged.iter().any(|run| {
                        run.segments
                            .iter()
                            .filter(|segment| run.selected.contains(&segment.sequence))
                            .any(carries)
                    });
                    let yes_no = |held: bool| {
                        if held {
                            StageValue::Yes
                        } else {
                            StageValue::No
                        }
                    };
                    // Retrieval is judged only where a structured aged arm ran a
                    // task with the canary already in its store.
                    let retrieval = if ingested && !structured_aged.is_empty() {
                        yes_no(retrieved)
                    } else {
                        StageValue::NotReached
                    };
                    (yes_no(ingested), retrieval)
                }
                _ => (StageValue::NotReached, StageValue::NotReached),
            };
            score_injection(
                case,
                &InjectionObservation {
                    ingested,
                    retrieved,
                    packed: StageValue::NotReached,
                    mediation: None,
                    outputs: Vec::new(),
                    later_session: None,
                },
            )
        })
        .collect()
}

/// The messages each segment covers, by sequence.
fn covered(segments: &[StoredHistorySegment], world: &World) -> BTreeMap<i64, Vec<EventId>> {
    segments
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
        .collect()
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

#[cfg(test)]
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

fn prepare_publish(publish: &Path) -> Result<(), RunError> {
    let refused = |path: &Path, error: std::io::Error| RunError::Publish {
        path: path.to_path_buf(),
        kind: error.kind(),
    };
    std::fs::create_dir_all(publish).map_err(|error| refused(publish, error))?;
    // A leftover staged file or a prior run's final file is refused: the
    // publisher never renames over either, so one directory holds one
    // generation's report and manifest or none.
    for file in [REPORT_FILE, MANIFEST_FILE] {
        let path = publish.join(file);
        for path in [staged_path(&path), path] {
            if path.symlink_metadata().is_ok() {
                return Err(refused(
                    &path,
                    std::io::Error::from(std::io::ErrorKind::AlreadyExists),
                ));
            }
        }
    }
    // The directory must take a staged file now, not after every life has
    // run: a probe at the report's staged path is created and removed, so a
    // permission publication would hit refuses before a fixture starts.
    let probe = staged_path(&publish.join(REPORT_FILE));
    std::fs::File::create_new(&probe).map_err(|error| refused(&probe, error))?;
    std::fs::remove_file(&probe).map_err(|error| refused(&probe, error))?;
    Ok(())
}

fn publish_file(path: &Path, bytes: &[u8]) -> Result<(), RunError> {
    write_then_rename(path, bytes).map_err(|error| RunError::Publish {
        path: path.to_path_buf(),
        kind: error.kind(),
    })
}

/// Runs one campaign under `config`: compiles the pairs, drives every arm of
/// every pair through the fixture under both history policies, gates the
/// report, and publishes the report and its manifest write-then-rename into
/// `config.publish`. An unapproved profile and an aged history too short for
/// its window are refused before anything runs; the envelope refuses the
/// first reading past a bound rather than reporting an overrun afterwards.
pub fn run(config: &Config) -> Result<Run, RunError> {
    let Config {
        scale,
        aged_messages,
        elapsed_bound_ms,
        approval,
        publish,
    } = config;
    let (scale, aged_messages, elapsed_ms) = (*scale, *aged_messages, *elapsed_bound_ms);
    let max_events_per_log = aged_messages
        .max(64)
        .checked_mul(2)
        .ok_or(RunError::AgedHistoryTooLong { aged_messages })?;
    let profile = profile(scale, max_events_per_log, elapsed_ms, approval.clone());
    profile.approved()?;
    let window = profile.baseline_bounds[&EvaluatedSurface::Surface1];
    // The falsifier must sit outside the window and the plain task inside it.
    if aged_messages <= window {
        return Err(RunError::AgedHistoryTooShort {
            aged_messages,
            window,
        });
    }
    prepare_publish(publish)?;
    // The fixture is built before the envelope is held: compiling it is the
    // harness's work, and its processes and time are not the campaign's.
    fixture_binary();
    // The manifest's clock and the envelope's start together, after the build.
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut charges = Charges::new(profile.envelope.clone());

    let task_set = plan_injection_cases(SEED, &TASK_IDS.iter().map(|id| id.to_string()).collect());
    let aged = generate_all(
        SEED,
        &one_session(
            aged_messages,
            AGED_TOOL_SPAN_EVERY,
            AGED_COMMITS,
            max_events_per_log,
            planted(&task_set.cases),
        ),
        Mode::Generate,
    )
    .unwrap()
    .log;
    let natural_fresh = generate_all(
        SEED ^ 0x77,
        &one_session(FRESH_MESSAGES, 0, 0, max_events_per_log, Vec::new()),
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
        check_recency_baseline(&set, &serialize_spec(), Baseline::Versioned).unwrap()
    else {
        panic!("{aged_messages} messages push the third one out of a window of {window}");
    };

    // The build identity is frozen before the first arm runs: the checkout,
    // the lockfile, and the binaries the outcomes come from, not whatever the
    // tree holds when the report is written.
    let identity = identity(&profile, &set);
    let eval_run_id = eval_run_id(&identity).unwrap();

    let aged_world = world(&set.aged);
    assert_eq!(aged_world.messages.len(), aged_messages as usize);
    let cassettes = charges.occupy()?;
    let aged_recording = record("aged", &aged_world, cassettes.path(), &mut charges)?;
    assert!(
        aged_recording.firings > 0,
        "the aged history reaches the pressure the summarizer fires at"
    );
    let mut cassette_bytes = aged_recording.cassette_bytes;
    charges.observe(Resource::CassetteBytes, cassette_bytes)?;
    // Refusals over firings per arm, as the lives observed them.
    let aged_fired = (aged_recording.firings, aged_recording.refusals);
    let mut fresh_fired = (0, 0);
    let mut structured_outcomes: Vec<PairOutcome> = Vec::new();
    let mut structured_aged_runs: Vec<ArmRun> = Vec::new();
    let mut outcomes = Vec::new();
    let mut samples = BTreeMap::new();
    let mut order = Vec::new();
    let mut verdicts: BTreeMap<(String, &'static str), StageVerdict<Surface1Stage>> =
        BTreeMap::new();
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
            &mut charges,
        )?;
        let fresh_run = run_arm(
            &fresh_world,
            &Replacement::Raw,
            &pair.task,
            &profile,
            &mut charges,
        )?;
        let fresh_recording = record(
            &format!("fresh-{}", pair.task.id),
            &fresh_world,
            cassettes.path(),
            &mut charges,
        )?;
        fresh_fired.0 += fresh_recording.firings;
        fresh_fired.1 += fresh_recording.refusals;
        // Twelve turns never reach the pressure the summarizer fires at, so
        // the short control's structured arm is its raw history.
        assert_eq!(
            (fresh_recording.firings, fresh_recording.segments.len()),
            (0, 0),
            "the summarizer leaves the short control raw"
        );
        cassette_bytes += fresh_recording.cassette_bytes;
        charges.observe(Resource::CassetteBytes, cassette_bytes)?;
        // A structured arm runs only under a cassette; a refused recording
        // leaves none, and the arm's samples are skipped as refused.
        let aged_structured_run = match aged_recording.replay.as_ref() {
            None => None,
            Some(replay) => {
                let run = run_arm(
                    &aged_world,
                    &Replacement::Summarizer(replay.clone()),
                    &pair.task,
                    &profile,
                    &mut charges,
                )?;
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
                Some(run)
            }
        };
        let fresh_structured_run = match fresh_recording.replay.as_ref() {
            None => None,
            Some(replay) => Some(run_arm(
                &fresh_world,
                &Replacement::Summarizer(replay.clone()),
                &pair.task,
                &profile,
                &mut charges,
            )?),
        };
        // Samples in the order the arms ran: raw, then structured; the pruned
        // arms, never attempted, are declared last.
        for (kind, label, run) in [
            (ArmKind::Aged, "aged", &aged_run),
            (ArmKind::Fresh, "fresh", &fresh_run),
        ] {
            let record = sample(pair, kind, HistoryPolicy::Raw, terminal_of(run.result));
            order.push(record.id.clone());
            samples.insert(record.id.clone(), record);
            verdicts.insert((pair.task.id.clone(), label), run.verdict);
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
                verdicts.insert((pair.task.id.clone(), label), run.verdict);
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
                    family: TASK_FAMILY.to_string(),
                    world_seed: WORLD,
                },
                fresh: fresh.result,
                aged: aged.result,
            });
        }
        if let Some(run) = aged_structured_run {
            structured_aged_runs.push(run);
        }
        outcomes.push(PairOutcome {
            pair_id: pair.task.id.clone(),
            cluster: ClusterKey {
                family: TASK_FAMILY.to_string(),
                world_seed: WORLD,
            },
            fresh: fresh_run.result,
            aged: aged_run.result,
        });
    }

    let refused_aged = aged_recording.replay.is_none();
    // The messages each of the recording's segments covers, by sequence; every
    // replayed arm published the same segments.
    let aged_structured_covered = covered(&aged_recording.segments, &aged_world);
    let arms = GovernanceArms {
        control_run_id: "ee".repeat(32),
        pair_set_digest: eval_core::pair_set_digest(&set).unwrap(),
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
    arms.validate(&set, &serialize_spec()).unwrap();

    let ledger = SampleLedger {
        epoch: 1,
        order,
        samples,
    };
    let rates = ledger.rates().unwrap();
    // Surface 1 made no model call on any arm (the fixture's counters), and a
    // replayed frame that missed would fail the life's assertion and end the
    // campaign before any report (a world replaying its own recording in one
    // process misses only through a determinism fault), so a published
    // report's miss rates are zero by observation; the refusal rates are the
    // cassette's refusals over the summarizer's firings, as the lives saw
    // them.
    let arm_rates: BTreeMap<String, ArmRates> = [("aged", aged_fired), ("fresh", fresh_fired)]
        .into_iter()
        .map(|(arm, (firings, refusals))| {
            (
                arm.to_string(),
                ArmRates {
                    miss_rate: "0".to_string(),
                    refusal_rate: decimal_ceil(refusals, firings),
                },
            )
        })
        .collect();
    let family = family(&profile);
    let frozen = FrozenFamily::freeze(&family).unwrap();
    // The manifest is the run's record and the analysis reads the table under
    // it: it names the pairs as its samples, binds the completed table by
    // digest, carries the frozen family and the arm rates, and says how the
    // world reached the store: every arm was lived through the daemon's own
    // transform route one turn at a time in one store incarnation, so the run
    // is `replay` over `transform-route, turn by turn`.
    charges.elapsed()?;
    let mut manifest = manifest(
        identity,
        &eval_run_id,
        &set,
        &profile,
        &frozen,
        &outcomes,
        &arm_rates,
        &charges.envelope,
        started_at_ms,
    );
    let Analysis::Report(analysis) = analyze(&manifest, &family, &outcomes).unwrap() else {
        panic!("a pair set with pairs reports");
    };
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
        eval_run_id,
        profile: profile.clone(),
        profile_digest: profile.digest().unwrap(),
        surface: EvaluatedSurface::Surface1,
        family: family.clone(),
        claims: Claims {
            boundary: ClaimBoundary::pinned(),
            established,
            derivation: family
                .claim_class(&frozen, WorldProvenance::Generated, None)
                .unwrap(),
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
        injection: injection_scores(
            &task_set.cases,
            &aged_recording.segments,
            &structured_aged_runs,
        ),
        envelope: charges.envelope.clone(),
    };
    // The publish root, the artifact's bytes, and the retained artifact are
    // charged before the envelope is copied into the report, so the published
    // peaks include the publication. The report carries its own size as a
    // peak, so it is serialized until the bytes written carry the peak they
    // are: each reading is charged, and the envelope refuses on any of them.
    charges.roots += 1;
    charges.observe(Resource::TempRoots, charges.roots)?;
    charges.observe(Resource::RetainedArtifacts, 1)?;
    charges.elapsed()?;
    charges.envelope.check()?;
    let bytes = loop {
        report.envelope = charges.envelope.clone();
        let bytes = serde_json::to_vec_pretty(&report.serialize().unwrap()).unwrap();
        let peak = charges.envelope.peaks.artifact_bytes;
        charges.observe(Resource::ArtifactBytes, bytes.len() as u64)?;
        if charges.envelope.peaks.artifact_bytes == peak {
            break bytes;
        }
    };
    // The manifest carries the envelope the report was published under: the
    // peaks and the clock are measurements outside its digest, so refreshing
    // them changes nothing the analysis read it for.
    manifest.envelope_peaks = charges.envelope.peaks.clone();
    manifest.end_ms = started_at_ms + i64::try_from(charges.envelope.peaks.elapsed_ms).unwrap();
    // The manifest's bytes are ready before either file is linked into place,
    // so a report is never published without it, and a manifest the directory
    // then refuses to take (out of space, a file that arrived meanwhile)
    // takes the report back out with it: a reader finds both files or none.
    // A process killed between the two links still leaves the report alone;
    // the next run into the directory is refused rather than mixed.
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    let report_path = publish.join(REPORT_FILE);
    publish_file(&report_path, &bytes)?;
    if let Err(error) = publish_file(&publish.join(MANIFEST_FILE), &manifest_bytes) {
        let _ = std::fs::remove_file(&report_path);
        return Err(error);
    }
    Ok(Run {
        report,
        report_bytes: bytes,
        manifest,
        manifest_bytes,
        set,
        verdicts,
        outcomes,
        structured_outcomes,
        aged: RecordedLife {
            firings: aged_recording.firings,
            refused: refused_aged,
            covered: aged_structured_covered,
        },
    })
}

/// The report's file name under the publish directory.
pub const REPORT_FILE: &str = "suite-b-report.json";
/// The manifest's file name under the publish directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The `campaign` subcommand's flags, every one required: a campaign runs
/// only under values someone wrote down.
pub const USAGE: &str = "campaign --scale <s0|s1|s2> --aged-messages <n> \
--elapsed-bound-ms <n> --approved-by <name> --approval-run-id <hex64> --publish <dir>";

const FLAGS: [&str; 6] = [
    "scale",
    "aged-messages",
    "elapsed-bound-ms",
    "approved-by",
    "approval-run-id",
    "publish",
];

/// Reads a `Config` from the `campaign` subcommand's arguments. A flag that is
/// unknown, repeated, missing, or missing its value is refused with the flag
/// named, before any value is read.
pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let Some(name) = flag.strip_prefix("--") else {
            return Err(format!("unexpected argument {flag:?}; {USAGE}"));
        };
        if !FLAGS.contains(&name) {
            return Err(format!("unknown flag --{name}; {USAGE}"));
        }
        let value = match args.next() {
            Some(value) if !value.starts_with("--") => value,
            _ => return Err(format!("--{name} needs a value")),
        };
        if values.insert(name.to_string(), value).is_some() {
            return Err(format!("--{name} given twice"));
        }
    }
    if let Some(missing) = FLAGS.iter().find(|flag| !values.contains_key(**flag)) {
        return Err(format!("--{missing} is required; {USAGE}"));
    }
    let take = |name: &str| values[name].clone();
    let scale: Scale = serde_json::from_value(Value::String(take("scale")))
        .map_err(|error| format!("--scale: {error}"))?;
    let number = |name: &str| {
        take(name)
            .parse::<u64>()
            .map_err(|error| format!("--{name}: {error}"))
    };
    let aged_messages = u32::try_from(number("aged-messages")?)
        .map_err(|error| format!("--aged-messages: {error}"))?;
    Ok(Config {
        scale,
        aged_messages,
        elapsed_bound_ms: number("elapsed-bound-ms")?,
        approval: Some(Approval {
            approved_by: take("approved-by"),
            approved_at_run_id: take("approval-run-id"),
        }),
        publish: PathBuf::from(take("publish")),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn command(program: &str, args: &[&str]) -> String {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(super::support::direct_host::workspace_root())
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(output.status.success(), "{program} {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// The run's manifest: the identity of this checkout and toolchain, the
/// profile and scenario the run was declared under, every sample in the order
/// it ran, the report's digest as its result, the pair set's digest as its
/// witness, and the envelope as bounded and as peaked.
/// The daemon features this shell was compiled under, as the build record
/// names them: the example carries `eval-runner`, and both callers carry
/// `test-support`.
fn enabled_features() -> BTreeSet<String> {
    let mut features = BTreeSet::from(["test-support".to_string()]);
    if cfg!(feature = "eval-runner") {
        features.insert("eval-runner".to_string());
    }
    if cfg!(feature = "direct-host-fixture") {
        features.insert("direct-host-fixture".to_string());
    }
    features
}

/// The run's identity: this checkout and toolchain, the profile as its
/// config, the surface and tasks as its scenario, and the generator's
/// versions. It does not depend on what the run found, so the report is
/// stamped with it before the manifest is.
fn identity(profile: &RunProfile, set: &PairSet) -> RunIdentity {
    let dirty = !command("git", &["status", "--porcelain"]).is_empty();
    let lockfile =
        std::fs::read(super::support::direct_host::workspace_root().join("Cargo.lock")).unwrap();
    RunIdentity {
        build: BuildRecord {
            code_sha: command("git", &["rev-parse", "HEAD"]),
            dirty,
            lockfile_digest: sha256_hex(&lockfile),
            rustc_version: command("rustc", &["--version"]),
            features: enabled_features(),
            target_triple: command("rustc", &["-vV"])
                .lines()
                .find_map(|line| line.strip_prefix("host: "))
                .expect("rustc names its host")
                .to_string(),
            // The fixture and the executable driving it: a dirty tree that
            // changes only the shell changes this digest too.
            binary_digest: BinaryDigest::Present {
                sha256: sha256_hex(
                    &[
                        std::fs::read(fixture_binary()).unwrap(),
                        std::fs::read(std::env::current_exe().unwrap()).unwrap(),
                    ]
                    .concat(),
                ),
            },
        },
        simulator_version: "eval-campaign-shell/v1".to_string(),
        config: serde_json::to_value(profile).unwrap(),
        scenario: serde_json::json!({
            "surface": EvaluatedSurface::Surface1,
            "tasks": set.pairs.iter().map(|p| p.task.id.clone()).collect::<Vec<_>>(),
            "aged_events": set.aged.events.len(),
        }),
        root_seed: SEED,
        random_schema_version: RANDOM_SCHEMA_VERSION.to_string(),
        generator_version: GENERATOR_VERSION.to_string(),
        eligibility_spec_digest: ELIGIBILITY_SPEC_DIGEST.to_string(),
        linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn manifest(
    identity: RunIdentity,
    eval_run_id: &str,
    set: &PairSet,
    profile: &RunProfile,
    frozen: &FrozenFamily,
    outcomes: &[PairOutcome],
    arm_rates: &BTreeMap<String, ArmRates>,
    envelope: &Envelope,
    started_at_ms: i64,
) -> Manifest {
    let set_value = serde_json::to_value(set).unwrap();
    let sample_order: Vec<String> = outcomes.iter().map(|pair| pair.pair_id.clone()).collect();
    Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        eval_run_id: eval_run_id.to_string(),
        run_identity: identity,
        start_ms: started_at_ms,
        end_ms: started_at_ms + i64::try_from(envelope.peaks.elapsed_ms).unwrap(),
        status: RunStatus::Completed,
        error: None,
        sample_ids: sample_order.clone(),
        sample_order,
        sample_epoch: 1,
        retry_lineage: Vec::new(),
        result_digest: pair_table_digest(outcomes).unwrap(),
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
        envelope_bounds: envelope.bounds.clone(),
        envelope_peaks: envelope.peaks.clone(),
        arm_rates: arm_rates.clone(),
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const FULL: [&str; 12] = [
        "--scale",
        "s0",
        "--aged-messages",
        "130",
        "--elapsed-bound-ms",
        "1200000",
        "--approved-by",
        "someone",
        "--approval-run-id",
        "ab",
        "--publish",
        "/tmp/out",
    ];

    #[test]
    fn every_flag_given_once_is_a_config() {
        let config = config_from_args(args(&FULL)).unwrap();
        assert_eq!(
            config,
            Config {
                scale: Scale::S0,
                aged_messages: 130,
                elapsed_bound_ms: 1_200_000,
                approval: Some(Approval {
                    approved_by: "someone".to_string(),
                    approved_at_run_id: "ab".to_string(),
                }),
                publish: PathBuf::from("/tmp/out"),
            }
        );
    }

    #[test]
    fn every_refusal_names_its_flag() {
        let cases: [(&[&str], &str); 7] = [
            (&["--scale", "s0"], "--aged-messages is required"),
            (&["--aged_messages", "5"], "unknown flag --aged_messages"),
            (
                &["--scale", "--aged-messages", "5"],
                "--scale needs a value",
            ),
            (&["--scale", "s0", "--scale", "s1"], "--scale given twice"),
            (&["s0"], "unexpected argument \"s0\""),
            (&["--publish"], "--publish needs a value"),
            (&[], "--scale is required"),
        ];
        for (list, expected) in cases {
            let error = config_from_args(args(list)).unwrap_err();
            assert!(error.contains(expected), "{list:?}: {error}");
        }
        let mut bad_scale = args(&FULL);
        bad_scale[1] = "s9".to_string();
        assert!(
            config_from_args(bad_scale)
                .unwrap_err()
                .starts_with("--scale:")
        );
        let mut bad_number = args(&FULL);
        bad_number[3] = "many".to_string();
        assert!(
            config_from_args(bad_number)
                .unwrap_err()
                .starts_with("--aged-messages:")
        );
    }
}
