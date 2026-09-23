//! The shrink shell. Every candidate is replayed in a fresh process that
//! compiles the pair set, reduces the aged truth at the pinned cut, and
//! reports the oracle's outcome over a barrier line; the parent keeps the
//! replay-effect ledger, drives the core shrinker, and publishes the witness
//! package and its manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use context_core::canonical_json::protocol_digest;
use context_core::redaction::Redactor;
use eval_core::{
    Approval, CandidateVerdict, ClaimBoundary, Coverage, Cut, CutOutcome, CutReceipt,
    EnvelopeExceeded, EvaluatedSurface, EventId, ExecutionMode, FailurePredicate, FaultAction,
    Generation, Manifest, Mode, MultiplicityRecipe, ObservationSchema, Oracle, OriginalFailure,
    Payload, ProfileError, ReplayEffects, ReplayOutcome, ReplayRefused, ReplayRequest,
    RepositorySpec, ResidueEntry, Rule, RunProfile, Scale, Scenario, SemanticTrace, SessionSpec,
    ShrinkRefused, Slice, StoreFamily, Task, TaskRole, UnknownReason, WITNESS_DIGEST_PROTOCOL,
    WITNESS_SCHEMA, WitnessError, WitnessPackage, WorldConfig, eval_run_id, generate_all, reduce,
    residue_drift, serialize_spec, shrink,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::aging::{ManifestInputs, suite_c_manifest};
use super::campaign::{Charges, identity, parse_flags, prepare_publish, publish_file};
use super::fault::ChildGuard;

pub const SIMULATOR_VERSION: &str = "eval-shrink-shell/v1";
pub const WITNESS_FILE: &str = "witness.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const BARRIER: &str = "eval-shrink-barrier";
pub const CHILD_ARGS: &str = "EIDNARA_EVAL_SHRINK_CHILD";
pub const SEED: u64 = 0x5EED_5000_0000_0005;
const FRESH_SEED: u64 = SEED ^ 0xABCD;
const CUT: Cut = Cut::AtQuiescence;
const REPLAY_TIMEOUT: Duration = Duration::from_secs(120);
/// Attempts under one receipt key when the child exits before its barrier.
const REPLAY_ATTEMPTS: u32 = 2;
const MAX_REPLAYS: u64 = 400;
const EPOCH_MS: i64 = 1_700_000_000_000;
pub const USAGE: &str = "shrink --scale <s0|s1|s2> --commits <n> --elapsed-bound-ms <n> \
--approved-by <name> --approval-run-id <hex64> --publish <dir>";
const FLAGS: [&str; 6] = [
    "scale",
    "commits",
    "elapsed-bound-ms",
    "approved-by",
    "approval-run-id",
    "publish",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scale: Scale,
    pub commits: u32,
    pub elapsed_bound_ms: u64,
    pub approval: Option<Approval>,
    pub publish: PathBuf,
    pub oracle: Oracle,
    pub replay_timeout: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("the original scenario did not fail: {outcome:?}")]
    NoFailure { outcome: ReplayOutcome },
    #[error("shrink refused: {0}")]
    Shrink(#[from] ShrinkRefused),
    #[error("replay effect refused: {0}")]
    Replay(#[from] ReplayRefused),
    #[error("witness refused: {0}")]
    Witness(#[from] WitnessError),
    #[error("publish refused at {}: {kind}", path.display())]
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn publish_refused((path, kind): (PathBuf, std::io::ErrorKind)) -> RunError {
    RunError::Publish { path, kind }
}

pub struct Run {
    pub witness: WitnessPackage,
    pub witness_bytes: Vec<u8>,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub coverage: Coverage,
    /// What the fresh process reported for the original scenario.
    pub original: Replayed,
}

/// What a child reports over its barrier line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replayed {
    pub outcome: ReplayOutcome,
    pub trace_digest: String,
    pub residue: BTreeSet<ResidueEntry>,
}

/// Everything a child needs, carried in one environment variable as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildArgs {
    pub scenario: PathBuf,
    pub oracle: Oracle,
    pub checkpoint: Cut,
    pub profile_digest: String,
}

impl ChildArgs {
    pub fn from_env() -> Option<Self> {
        let text = std::env::var(CHILD_ARGS).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn env(&self, command: &mut Command) {
        command.env(CHILD_ARGS, serde_json::to_string(self).unwrap());
    }
}

pub type Spawn = fn(&ChildArgs) -> Command;

/// The one observation a replay records: the scenario and what the oracle
/// said of it are kept; the process that said it is dropped.
fn replay_schema() -> ObservationSchema {
    ObservationSchema::new(
        "shrink_replay",
        [
            ("scenario_digest", Rule::Keep),
            ("outcome", Rule::Keep),
            ("pid", Rule::Drop),
        ],
    )
    .unwrap()
}

pub fn residue() -> BTreeSet<ResidueEntry> {
    replay_schema()
        .residue()
        .chain(Manifest::field_schema().residue())
        .collect()
}

/// The child: evaluates the oracle over the scenario at the cut and prints
/// it over the barrier line with the trace digest of the one observation and
/// the residue this build declares, then exits.
pub fn child_main(args: &ChildArgs) -> ! {
    let fixture = serialize_spec();
    let outcome = std::fs::read(&args.scenario)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Scenario>(&bytes).ok())
        .and_then(|scenario| {
            let set = scenario.compile(&fixture).ok()?;
            let truth = reduce(&set.aged, &fixture, &set.pairs.first()?.task.query).ok()?;
            Some((
                scenario.digest(),
                args.oracle
                    .evaluate(&set, &truth, args.checkpoint, &args.profile_digest),
            ))
        });
    let (scenario_digest, outcome) = outcome.unwrap_or_else(|| {
        (
            String::new(),
            ReplayOutcome::Unknown {
                reason: UnknownReason::ReadBackFailed,
            },
        )
    });
    let mut trace = SemanticTrace::new([replay_schema()]).unwrap();
    trace
        .record(
            "shrink_replay",
            &json!({
                "scenario_digest": scenario_digest,
                "outcome": outcome,
                "pid": std::process::id(),
            }),
        )
        .unwrap();
    let replayed = Replayed {
        outcome,
        trace_digest: trace.digest().unwrap(),
        residue: residue(),
    };
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{BARRIER} {}",
        serde_json::to_string(&replayed).unwrap()
    )
    .unwrap();
    stdout.flush().unwrap();
    std::process::exit(0)
}

/// Issues one replay per receipt key to a fresh process and resolves its
/// effect; a key already answered is read back, never replayed again.
struct Replayer<'a> {
    spawn: Spawn,
    timeout: Duration,
    /// The profile's elapsed bound; no replay waits past it.
    deadline: Instant,
    args: ChildArgs,
    charges: &'a mut Charges,
    effects: ReplayEffects,
    answered: BTreeMap<String, Replayed>,
    expected_residue: BTreeSet<ResidueEntry>,
}

impl Replayer<'_> {
    fn replay(&mut self, key: &str, scenario: &Scenario) -> Result<Replayed, RunError> {
        if let Some(answered) = self.answered.get(key) {
            return Ok(answered.clone());
        }
        std::fs::write(&self.args.scenario, serde_json::to_vec(scenario).unwrap())?;
        self.effects.issue(key)?;
        let mut attempts = 1;
        let mut replayed = loop {
            match self.attempt()? {
                Some(replayed) => break replayed,
                None if attempts < REPLAY_ATTEMPTS => attempts = self.effects.retry(key)?,
                None => break self.unanswered(UnknownReason::ChildExitedBeforeBarrier),
            }
        };
        self.effects.resolve(key, replayed.outcome.clone())?;
        replayed.outcome = self.effects.outcome(key)?.clone();
        residue_drift(&self.expected_residue, &replayed.residue)?;
        self.answered.insert(key.to_string(), replayed.clone());
        Ok(replayed)
    }

    /// The reason, no trace, and this build's own residue.
    fn unanswered(&self, reason: UnknownReason) -> Replayed {
        Replayed {
            outcome: ReplayOutcome::Unknown { reason },
            trace_digest: String::new(),
            residue: self.expected_residue.clone(),
        }
    }

    /// `None` when the child exited before its barrier; a timeout answers
    /// `Unknown { cancelled }`. The process charge is released either way.
    fn attempt(&mut self) -> Result<Option<Replayed>, RunError> {
        let mut command = (self.spawn)(&self.args);
        self.args.env(&mut command);
        self.charges.process_started()?;
        let outcome = self.wait_for_barrier(command);
        self.charges.process_ended();
        self.charges.elapsed()?;
        outcome
    }

    fn wait_for_barrier(&self, mut command: Command) -> Result<Option<Replayed>, RunError> {
        let mut child = ChildGuard(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?,
        );
        let stdout = child.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let line = BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .find(|line| line.contains(BARRIER));
            let _ = tx.send(line);
        });
        let wait = self
            .timeout
            .min(self.deadline.saturating_duration_since(Instant::now()));
        Ok(match rx.recv_timeout(wait) {
            Ok(Some(line)) => {
                let json = &line[line.find(BARRIER).unwrap() + BARRIER.len()..];
                Some(
                    serde_json::from_str::<Replayed>(json.trim())
                        .unwrap_or_else(|_| self.unanswered(UnknownReason::ReadBackFailed)),
                )
            }
            Ok(None) | Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => Some(self.unanswered(UnknownReason::Cancelled)),
        })
    }
}

fn aged_config(commits: u32) -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages: 6,
            tool_span_every: 2,
            correction_every: 3,
            invalidation_every: 5,
        }],
        repositories: vec![RepositorySpec {
            commits,
            rename_every: 2,
        }],
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 128,
        planted: Vec::new(),
    }
}

fn fresh_config() -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages: 3,
            tool_span_every: 2,
            correction_every: 0,
            invalidation_every: 0,
        }],
        repositories: vec![RepositorySpec {
            commits: 2,
            rename_every: 0,
        }],
        ..aged_config(0)
    }
}

/// The falsifier is the first commit and the positive control the last
/// rename; both survive every valid candidate.
pub fn scenario(commits: u32) -> (Scenario, eval_core::Tape) {
    let world = generate_all(SEED, &aged_config(commits), Mode::Generate).unwrap();
    let natural_fresh = generate_all(FRESH_SEED, &fresh_config(), Mode::Generate)
        .unwrap()
        .log;
    let repository = |kind: fn(&Payload) -> bool| {
        world
            .log
            .events
            .iter()
            .filter(|e| kind(&e.payload))
            .map(|e| e.id.clone())
            .collect::<Vec<EventId>>()
    };
    let commit_ids = repository(|p| matches!(p, Payload::Commit { .. }));
    let renames = repository(|p| matches!(p, Payload::Rename { .. }));
    let task = |name: &str, role, id: &EventId| Task {
        id: name.to_string(),
        role,
        query: eval_core::Query {
            valid_time_ms: eval_core::MAX_VALID_TIME_MS,
            observation_time_ms: eval_core::MAX_VALID_TIME_MS,
            scope: BTreeSet::from(["session-0".to_string(), "repository-0".to_string()]),
            destination: eval_core::Destination::Local,
            served: Some(eval_core::ServedClass {
                sensitivity: eval_core::Sensitivity::Normal,
                visibility: eval_core::Visibility::Labeled,
                auto_inject: eval_core::Visibility::Hidden,
                auto_search: eval_core::Visibility::Hidden,
            }),
            registry_sensitivity: eval_core::Sensitivity::Normal,
            max_events_per_log: 128,
        },
        evidence: BTreeSet::from([id.clone()]),
    };
    let scenario = Scenario {
        surface: EvaluatedSurface::Surface1,
        recency_bound: None,
        aged: world.log,
        natural_fresh,
        tasks: vec![
            task("first-commit", TaskRole::Falsification, &commit_ids[0]),
            task(
                "last-rename",
                TaskRole::PositiveControl,
                renames.last().expect("two commits make a rename"),
            ),
        ],
        episodes: ["kill-1", "kill-2"]
            .map(|id| {
                super::fault::episode(
                    id,
                    3,
                    StoreFamily::SearchProjection,
                    "acknowledge",
                    FaultAction::ProcessKill {
                        cut: "local_staged".to_string(),
                    },
                    "search_catchup::EpisodeFault",
                )
            })
            .to_vec(),
    };
    (scenario, world.tape)
}

/// Replays the original in a fresh process, pins its failure, shrinks it with
/// every candidate replayed the same way, and publishes the witness package
/// with its manifest.
pub fn run(config: &Config, spawn: Spawn) -> Result<Run, RunError> {
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let profile: RunProfile = super::campaign::profile(
        config.scale,
        128,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    profile.approved()?;
    let profile_digest = profile.digest()?;
    let mut charges = Charges::new(profile.envelope.clone());
    prepare_publish(&config.publish, &[WITNESS_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let root = charges.occupy()?;
    let (original, tape) = scenario(config.commits);
    let mut replayer = Replayer {
        spawn,
        timeout: config.replay_timeout,
        deadline: Instant::now() + Duration::from_millis(config.elapsed_bound_ms),
        args: ChildArgs {
            scenario: root.path().join("candidate.json"),
            oracle: config.oracle.clone(),
            checkpoint: CUT,
            profile_digest: profile_digest.clone(),
        },
        charges: &mut charges,
        effects: ReplayEffects::default(),
        answered: BTreeMap::new(),
        expected_residue: residue(),
    };
    let first = replayer.replay(&original.digest(), &original)?;
    let ReplayOutcome::Failed { predicate } = &first.outcome else {
        return Err(RunError::NoFailure {
            outcome: first.outcome,
        });
    };
    let predicate: FailurePredicate = predicate.clone();
    let fixture = serialize_spec();
    // The shrinker's callback cannot fail, so the first refusal is kept and
    // every later request is answered `Unknown` without a replay; the run
    // then returns that refusal.
    let mut refused = None;
    let mut callback = |request: ReplayRequest<'_>| {
        let unanswered = ReplayOutcome::Unknown {
            reason: UnknownReason::EffectUnanswered,
        };
        if refused.is_some() {
            return unanswered;
        }
        match replayer.replay(request.key, request.scenario) {
            Ok(replayed) => replayed.outcome,
            Err(error) => {
                refused = Some(error);
                unanswered
            }
        }
    };
    let shrunk = shrink(&original, &fixture, &predicate, MAX_REPLAYS, &mut callback);
    if let Some(error) = refused {
        return Err(error);
    }
    let (minimized, report) = shrunk?;
    let mut coverage = Coverage::default();
    coverage
        .record("flt_shrink_fresh_process_reproduced")
        .unwrap();
    if report
        .candidates
        .iter()
        .any(|record| matches!(record.verdict, CandidateVerdict::Slipped { .. }))
    {
        coverage
            .record("flt_shrink_slipped_candidate_rejected")
            .unwrap();
    }
    if report.unknown_candidates > 0 {
        coverage
            .record("flt_shrink_unknown_effect_preserved")
            .unwrap();
    }
    let run_identity = identity(
        &profile,
        SIMULATOR_VERSION,
        SEED,
        json!({"commits": config.commits, "oracle": config.oracle}),
        &std::env::current_exe().unwrap(),
    );
    let mut witness = WitnessPackage {
        schema: WITNESS_SCHEMA.to_string(),
        original: OriginalFailure {
            eval_run_id: eval_run_id(&run_identity).unwrap(),
            tape,
            trace_digest: first.trace_digest.clone(),
            causal_trace: original.aged.causal_edges.clone(),
            predicate,
            coverage: coverage.fired().iter().map(|m| m.to_string()).collect(),
        },
        slice: Slice::Cassette,
        replayable: true,
        residue: residue(),
        minimized,
        recipe: None,
        shrink: report,
        claim_boundary: ClaimBoundary::pinned(),
    };
    if witness.validate() == Err(WitnessError::RecipeRequired) {
        witness.recipe = Some(MultiplicityRecipe {
            aged: Generation {
                config: aged_config(config.commits),
                root_seed: SEED,
            },
            natural_fresh: Generation {
                config: fresh_config(),
                root_seed: FRESH_SEED,
            },
            multiplicities: witness.count_triggered(),
        });
    }
    charges.vacate(root)?;
    charges.retain_publish_root()?;
    let redactor = Redactor::new().map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    let (value, text) = witness.serialize(&redactor, profile.envelope.artifact_bytes)?;
    let witness_bytes = charges.publish_bytes(|_| text.clone().into_bytes())?;
    let report_value = serde_json::to_value(&witness.shrink).unwrap();
    let mut manifest = suite_c_manifest(ManifestInputs {
        identity: run_identity,
        eval_run_id: witness.original.eval_run_id.clone(),
        sample: format!("shrink:{}", witness.shrink.minimized_digest),
        result_digest: protocol_digest("eval-shrink-result/v1", &report_value).unwrap(),
        witness_digest: protocol_digest(WITNESS_DIGEST_PROTOCOL, &value).unwrap(),
        cut_receipts: vec![CutReceipt {
            cut: CUT,
            outcome: CutOutcome::Reached,
        }],
        execution_mode: ExecutionMode::Generate,
        envelope: charges.envelope.clone(),
        started_at_ms,
    });
    // The Suite C builder names the aging corpus and an in-process image; this
    // run generated its worlds from `SEED` and replayed each in a fresh child.
    manifest.component_versions.task_corpus = format!("generated:{SEED:#x}");
    manifest.component_versions.execution_image = "fresh-process".to_string();
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    publish_file(&config.publish.join(WITNESS_FILE), &witness_bytes).map_err(publish_refused)?;
    publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes).map_err(publish_refused)?;
    Ok(Run {
        witness,
        witness_bytes,
        manifest,
        manifest_bytes,
        coverage,
        original: first,
    })
}

pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let values = parse_flags(args, &FLAGS, USAGE)?;
    let take = |name: &str| values[name].clone();
    let scale: Scale = serde_json::from_value(Value::String(take("scale")))
        .map_err(|error| format!("--scale: {error}"))?;
    let number = |name: &str| {
        take(name)
            .parse::<u64>()
            .map_err(|error| format!("--{name}: {error}"))
    };
    Ok(Config {
        scale,
        commits: match number("commits")? {
            commits @ 2.. => {
                let commits =
                    u32::try_from(commits).map_err(|error| format!("--commits: {error}"))?;
                aged_config(commits)
                    .validate()
                    .map_err(|error| format!("--commits: {error:?}"))?;
                commits
            }
            _ => return Err("--commits needs at least two, so a rename exists".to_string()),
        },
        elapsed_bound_ms: number("elapsed-bound-ms")?,
        approval: Some(Approval {
            approved_by: take("approved-by"),
            approved_at_run_id: take("approval-run-id"),
        }),
        publish: PathBuf::from(take("publish")),
        oracle: Oracle::RequiredCommits {
            failing_at: 3,
            slipping_at: 6,
        },
        replay_timeout: REPLAY_TIMEOUT,
    })
}
