//! One Suite B campaign at scale S0 on the default surface: compiled pairs,
//! both arms driven through the direct-host fixture, the baseline contrast,
//! the run gates, and one report published write-then-rename, all under an
//! approved profile and inside a declared envelope.

#![cfg(all(unix, feature = "test-support"))]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use daemon::history_summarizer_evaluation::{
    ChunkLine, HistorySummarizerChunk, ValidateOptions, stored_history_segment,
    validate_history_summarizer_output,
};
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
    Sensitivity, ServedClass, SessionSpec, StageVerdict, StoppingRule, SuiteBReport, Surface1Stage,
    Task, TaskBudgets, TaskRole, TaskUsage, Terminal, TokenizerProfile, UnsupportedReason,
    Visibility, WorldConfig, WorldProvenance, analyze, check_recency_baseline, compile_pair_set,
    eval_run_id, generate_all, parse_manifest, parse_report, render, serialize_spec,
};
use host_runtime::CancellationToken;
use host_runtime::model_execution::backend::{
    BackendEvent, BackendFuture, BackendRequest, BackendTerminal, ContextCapabilities, EventSink,
    FinishReason, Harness, LlmExecutionBackend, OPENCODE_CONTEXT_CAPABILITIES, SinkStatus,
};
use memory_store::StoredHistorySegment;
use sha2::{Digest, Sha256};
use support::direct_host::{FixtureProcess, fixture_binary};
use support::eval_cassette::CassetteBackend;
use support::eval_surface::{
    EPOCH_MS, Knobs, SurfaceLedger, World, block_on, mid, observe_rendered, pass, seed_store,
    segment, text,
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
            hard_deadline_ms: 600_000,
            max_no_progress_iterations: 1,
        },
        envelope: ResourceLimits {
            elapsed_ms: 1_200_000,
            store_bytes: 64 << 20,
            cassette_bytes: 1 << 20,
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
    /// The messages each segment covers, by sequence.
    covered: BTreeMap<i64, Vec<EventId>>,
}

/// The marker only this message's summary has: its native message id with
/// every separator removed, one lexical token to the hint scorer.
fn marker(message_id: &str) -> String {
    message_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// The summary a segment carries for one message, whether the shell writes
/// it as a raw segment or the scripted summarizer folds it into a chunk.
fn summary_text(text: &str, message_id: &str) -> String {
    format!("{text} decision recorded as note{}", marker(message_id))
}

fn summary(message: &RenderedMessage) -> String {
    summary_text(text(message), mid(message))
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
    assert!(
        segments
            .iter()
            .all(|s| s.sequence == s.start_message && s.sequence == s.end_message),
        "a raw segment's sequence is its message's ordinal"
    );
    Arm {
        world: World {
            session: SESSION.to_string(),
            messages,
        },
        segments,
        covered: events.into_iter().map(|(k, v)| (k, vec![v])).collect(),
    }
}

/// Messages per summarizer segment on the structured arm.
const CHUNK: usize = 5;
const SUMMARIZER_NAMESPACE: &str = "eval-run:campaign:summarizer";

/// The chunk the summarizer is asked to cover: every message of the arm, one
/// anchorable line per message, as the producer would build it.
fn chunk_of(messages: &[RenderedMessage]) -> HistorySummarizerChunk {
    let n = messages.len() as u64;
    HistorySummarizerChunk {
        start_index: 1,
        end_index: n,
        lines: messages
            .iter()
            .enumerate()
            .map(|(index, message)| ChunkLine {
                ordinal: index as u64 + 1,
                message_id: format!("{}#0", mid(message)),
                anchorable: true,
            })
            .collect(),
        aliases: Default::default(),
        present_ordinals: (1..=n).collect(),
        tool_only_ranges: Vec::new(),
        completed_tool_arcs: Vec::new(),
    }
}

/// The transcript the summarizer request carries: one line per message.
fn transcript(messages: &[RenderedMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| format!("{}|{}|{}", index + 1, mid(message), text(message)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarizer_request(messages: &[RenderedMessage]) -> BackendRequest {
    BackendRequest {
        prompt: transcript(messages),
        system: Some("Summarize the history into history_segments.".to_string()),
        provider: "anthropic".to_string(),
        model: "claude".to_string(),
        max_output_tokens: 32_000,
        temperature: Some(0.1),
        harness: Harness::OpenCode,
        session: SESSION.to_string(),
        run_id: "model_execution-eval-campaign-1".to_string(),
    }
}

/// The provider whose answers the cassette records: a summarizer that covers
/// each run of `CHUNK` messages with one segment whose text keeps every
/// message's summary, in the output document the real validator reads.
struct ScriptedSummarizer;

impl LlmExecutionBackend for ScriptedSummarizer {
    fn execute(
        &self,
        request: BackendRequest,
        events: EventSink,
        _: CancellationToken,
    ) -> BackendFuture {
        Box::pin(async move {
            let lines: Vec<(u64, String, String)> = request
                .prompt
                .lines()
                .map(|line| {
                    let mut parts = line.splitn(3, '|');
                    let ordinal = parts.next().unwrap().parse().unwrap();
                    let mid = parts.next().unwrap().to_string();
                    let text = parts.next().unwrap().to_string();
                    (ordinal, mid, text)
                })
                .collect();
            let mut body = String::new();
            for group in lines.chunks(CHUNK) {
                let (start, _, _) = &group[0];
                let (end, _, _) = &group[group.len() - 1];
                let summaries: Vec<String> = group
                    .iter()
                    .map(|(_, mid, text)| summary_text(text, mid))
                    .collect();
                let joined = summaries.join("; ");
                body.push_str(&format!(
                    r#"<history_segment start="{start}" end="{end}" title="decisions {start} to {end}" episode_type="feature" importance="50"><p1>{joined}</p1><p2>{joined}</p2><p3>decisions {start} to {end}</p3><p4 /></history_segment>"#
                ));
            }
            let next = lines.len() + 1;
            events.emit(BackendEvent::AssistantText {
                text: format!(
                    "<output><history_segments>{body}</history_segments><meta><unprocessed_from>{next}</unprocessed_from></meta></output>"
                ),
                finish_reason: Some(FinishReason::Completed),
            });
            BackendTerminal::Completed {
                finish_reason: FinishReason::Completed,
            }
        })
    }

    fn unavailable_reason(&self, _: Harness) -> Option<&'static str> {
        None
    }

    fn context_capabilities(&self, _: Harness) -> ContextCapabilities {
        OPENCODE_CONTEXT_CAPABILITIES
    }
}

/// Runs one summarizer request through a backend and returns its text.
fn summarize_through(backend: &Arc<dyn LlmExecutionBackend>, request: BackendRequest) -> String {
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    let sink = {
        let seen = seen.clone();
        EventSink::new(Arc::new(move |event| {
            if let BackendEvent::AssistantText { text, .. } = event {
                seen.lock().unwrap().push_str(&text);
            }
            SinkStatus::Accepted
        }))
    };
    let terminal = block_on(backend.execute(request, sink, CancellationToken::new()));
    assert!(
        matches!(terminal, BackendTerminal::Completed { .. }),
        "{terminal:?}"
    );
    seen.lock().unwrap().clone()
}

/// The structured arm: the raw arm's history with its segments replaced by
/// the summarizer's, produced from a cassette replayed strictly and validated
/// by the summarizer's own validator. Returns the arm and the cassette file.
fn structured(raw: &Arm) -> (Arm, serde_json::Value) {
    let recording = CassetteBackend::recording(SUMMARIZER_NAMESPACE, Arc::new(ScriptedSummarizer));
    let recorded: Arc<dyn LlmExecutionBackend> = recording.clone();
    let answer = summarize_through(&recorded, summarizer_request(&raw.world.messages));
    let file = recording.file().unwrap();
    let replaying = CassetteBackend::replaying(SUMMARIZER_NAMESPACE, &file).unwrap();
    let replayed: Arc<dyn LlmExecutionBackend> = replaying.clone();
    let output = summarize_through(&replayed, summarizer_request(&raw.world.messages));
    assert_eq!(output, answer, "the replay serves the recorded frame");
    assert_eq!(replaying.terminal(), None, "the replay hit its one frame");
    // A one-byte change in the transcript is a strict miss, never an answer.
    let mut edited = summarizer_request(&raw.world.messages);
    edited.prompt.push('.');
    let strict = CassetteBackend::replaying(SUMMARIZER_NAMESPACE, &file).unwrap();
    let strict_backend: Arc<dyn LlmExecutionBackend> = strict.clone();
    let miss = block_on(strict_backend.execute(
        edited,
        EventSink::new(Arc::new(|_| SinkStatus::Accepted)),
        CancellationToken::new(),
    ));
    assert!(matches!(miss, BackendTerminal::Failed(_)), "{miss:?}");
    assert!(strict.terminal().is_some(), "the miss latched");

    let chunk = chunk_of(&raw.world.messages);
    let options = ValidateOptions {
        sequence_offset: 1,
        ..ValidateOptions::default()
    };
    let validated = validate_history_summarizer_output(&output, &chunk, &[], options)
        .unwrap_or_else(|error| panic!("the summarizer's own validator: {error:?}"));
    // The validator keeps the newest segment out so the tail stays raw, as the
    // producer would leave it; the messages from `unprocessed_from` on keep
    // their raw segments.
    assert!(validated.discarded_last, "the newest segment is held back");
    let covered_through = validated.unprocessed_from - 1;
    let mut segments: Vec<StoredHistorySegment> = validated
        .history_segments
        .iter()
        .map(|segment| stored_history_segment(segment, 0, &BTreeMap::new()))
        .collect();
    assert_eq!(segments.first().map(|s| s.sequence), Some(1));
    assert_eq!(
        segments.iter().map(|s| s.end_message).max(),
        Some(covered_through as i64)
    );
    let next_sequence = segments.iter().map(|s| s.sequence).max().unwrap_or(0) + 1;
    for (offset, raw_segment) in raw
        .segments
        .iter()
        .filter(|s| s.end_message > covered_through as i64)
        .enumerate()
    {
        segments.push(StoredHistorySegment {
            sequence: next_sequence + offset as i64,
            ..raw_segment.clone()
        });
    }
    let messages = raw.world.messages.len();
    assert_eq!(
        segments.len(),
        messages.div_ceil(CHUNK) - 1 + (messages - covered_through as usize),
        "every summarized run but the newest, then the raw tail"
    );
    // A segment covers every message from its start to its end.
    let events: BTreeMap<i64, Vec<EventId>> = segments
        .iter()
        .map(|segment| {
            let covered = (segment.start_message..=segment.end_message)
                .flat_map(|ordinal| raw.covered[&ordinal].iter().cloned())
                .collect();
            (segment.sequence, covered)
        })
        .collect();
    (
        Arm {
            world: World {
                session: raw.world.session.clone(),
                messages: raw.world.messages.clone(),
            },
            segments,
            covered: events,
        },
        file,
    )
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
    fixture.shutdown();
    envelope
        .observe(Resource::StoreBytes, root_bytes(root.path()))
        .unwrap();
    held.processes -= 1;
    drop(root);
    held.roots -= 1;
    envelope
        .observe(
            Resource::ElapsedMs,
            u64::try_from(started.elapsed().as_millis()).unwrap(),
        )
        .unwrap();
    // A segment stands for the evidence at every stage while it covers it;
    // the served fragment must still carry the evidence's own marker, or the
    // segment reached render with the evidence absent.
    let identities: BTreeMap<i64, String> = arm
        .covered
        .iter()
        .map(|(sequence, ids)| {
            let id = ids.iter().find(|id| *id == evidence).unwrap_or(&ids[0]);
            (*sequence, id.0.clone())
        })
        .collect();
    let hint_text = match &pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome.hint_text.clone(),
        _ => String::new(),
    };
    let served_marker = |id: &EventId| {
        let message = arm
            .world
            .messages
            .iter()
            .find(|m| m.event_id == *id)
            .expect("a covered message is rendered");
        hint_text.contains(&format!("note{}", marker(mid(message))))
    };
    let mut ledger = SurfaceLedger::default();
    observe_rendered(
        &mut ledger,
        pass.outcome.as_ref(),
        &identities,
        |sequence| {
            arm.covered[&sequence]
                .iter()
                .filter(|id| *id == evidence)
                .all(served_marker)
        },
    );
    let delivered: BTreeSet<EventId> = match &pass.outcome {
        Some(UserHintPass::Decided(outcome)) => outcome
            .trace
            .selected
            .iter()
            .flat_map(|sequence| arm.covered[sequence].iter().cloned())
            .filter(served_marker)
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

    let aged_arm = arm(&set.aged);
    assert_eq!(aged_arm.segments.len(), aged_messages as usize);
    let (aged_structured, cassette) = structured(&aged_arm);
    let mut cassette_bytes = serde_json::to_vec(&cassette).unwrap().len() as u64;
    envelope
        .observe(Resource::CassetteBytes, cassette_bytes)
        .unwrap();
    let mut structured_outcomes: Vec<PairOutcome> = Vec::new();
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
        let (fresh_structured, fresh_cassette) = structured(&fresh_arm);
        cassette_bytes += serde_json::to_vec(&fresh_cassette).unwrap().len() as u64;
        envelope
            .observe(Resource::CassetteBytes, cassette_bytes)
            .unwrap();
        let aged_structured_run = run_arm(
            &aged_structured,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
        let fresh_structured_run = run_arm(
            &fresh_structured,
            &pair.task,
            &profile,
            &mut envelope,
            &mut held,
            started,
        );
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
            let record = sample(
                pair,
                kind,
                HistoryPolicy::Structured,
                terminal_of(run.result),
            );
            order.push(record.id.clone());
            samples.insert(record.id.clone(), record);
            verdicts.insert((pair.task.id.as_str(), label), run.verdict);
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
        structured_outcomes.push(PairOutcome {
            pair_id: pair.task.id.clone(),
            cluster: ClusterKey {
                family: "generated".to_string(),
                world_seed: SEED,
            },
            fresh: fresh_structured_run.result,
            aged: aged_structured_run.result,
        });
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

    // Under the summarizer's segments the aged history is a fifth as many
    // units, so surface 1's window reaches the early message; but the served
    // fragment is capped, so a truth folded past the cap of its segment is
    // lost at render on both arms, while a truth at the head of its segment
    // or left raw in the tail is delivered.
    let structured_by_task: BTreeMap<&str, &PairOutcome> = structured_outcomes
        .iter()
        .map(|o| (o.pair_id.as_str(), o))
        .collect();
    let expected = [
        (
            "early-message",
            (ArmResult::Fail, ArmResult::Fail),
            StageVerdict::FirstLoss(Surface1Stage::Render),
        ),
        (
            "recent-message",
            (ArmResult::Pass, ArmResult::Pass),
            StageVerdict::Clean,
        ),
        (
            "last-message",
            (ArmResult::Pass, ArmResult::Pass),
            StageVerdict::Clean,
        ),
    ];
    for (name, results, aged_verdict) in expected {
        assert_eq!(
            (
                structured_by_task[name].aged,
                structured_by_task[name].fresh
            ),
            results,
            "{name} structured"
        );
        assert_eq!(
            verdicts[&(name, "aged/structured")],
            aged_verdict,
            "{name} structured"
        );
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
                    policy_version: format!("history_summarizer_validate/chunk-{CHUNK}"),
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
    assert_eq!((rates.samples, ledger.attempted()), (18, 12));
    assert_eq!(rates.unsupported, Ratio::new(1, 3));
    // Surface 1 made no model call on any arm (the fixture's counters), and
    // every summarizer replay served its frame with no miss and no refusal,
    // so both arms' miss and refusal rates are zero by observation; the
    // strictness probe is a separate cassette instance and not an arm.
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
    assert!(peaks["cassette_bytes"].as_u64().unwrap() > 0);

    // The manifest beside the report says how the world reached the store:
    // the segments were written straight into it, so the run is `bulk` and
    // `direct-database, non-aged`, never a replay-built aged world.
    let manifest = manifest(&profile, &set, &report, &bytes, &frozen, started_at_ms);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    write_then_rename(&publish.path().join("manifest.json"), &manifest_bytes);
    let read_back: serde_json::Value =
        serde_json::from_slice(&std::fs::read(publish.path().join("manifest.json")).unwrap())
            .unwrap();
    let parsed = parse_manifest(&read_back).unwrap();
    assert_eq!(parsed, manifest);
    assert_eq!(parsed.digest().unwrap(), manifest.digest().unwrap());
    // A seeded history may not call itself aged: the same manifest under a
    // `replay` construction is refused.
    let mut relabelled = read_back.clone();
    relabelled["construction"] = serde_json::json!("replay");
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
        construction: Construction::Bulk,
        execution_mode: ExecutionMode::Generate,
        failure_class_table_digest: FAILURE_CLASS_TABLE_DIGEST.to_string(),
        ingestion: Ingestion::DirectDatabaseNonAged,
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
