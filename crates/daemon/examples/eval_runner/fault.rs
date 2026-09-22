//! Suite C fault campaign: contract-faithful fault episodes on the aging drive,
//! judged by `eval_core::fault`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use daemon::search_catchup::{Blocked, EpisodeEnd, EpisodeEvent, EpisodeFault, EpisodeReport};
use eval_core::{
    APPLICATION_CRASH, Approval, ClaimBoundary, Coverage, Cut, CutCoverage, EffectLedger,
    EffectState, EnvelopeExceeded, ExecutionMode, FAULT_REPORT_SCHEMA, FaultAction, FaultEpisode,
    FaultReport, FaultReportError, FaultScope, KillLabel, LivenessBounds, ProfileError,
    RecordedRefusal, RunProfile, Scale, SearchEpisodeFault, StoreFamily, TEST_BINARY_CHILD,
    WorkCounter, cut_receipts, eval_run_id,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use super::aging::{
    self, ManifestInputs, Plan, Stores, kernel_file, live, read_only, search_file, suite_c_manifest,
};
use super::campaign::{Charges, identity, prepare_publish, publish_file};

pub const REPORT_FILE: &str = "suite-c-fault-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIMULATOR_VERSION: &str = "eval-fault-shell/v1";
const CONSUMER: &str = "search";

pub type Config = aging::Config;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("plan refused: {0}")]
    Plan(#[from] aging::RunError),
    #[error("report refused: {0}")]
    Report(#[from] FaultReportError),
    #[error("episode {episode}: expected {expected}, observed {observed}")]
    Unexpected {
        episode: String,
        expected: String,
        observed: String,
    },
    #[error("publish {}: {kind}", path.display())]
    Publish {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
}

fn publish_refused((path, kind): (PathBuf, std::io::ErrorKind)) -> RunError {
    RunError::Publish { path, kind }
}

pub fn profile(
    scale: Scale,
    steps: u32,
    elapsed_ms: u64,
    approval: Option<Approval>,
) -> RunProfile {
    let mut profile = aging::profile(scale, steps, elapsed_ms, approval);
    profile.name = profile.name.replace("suite-c-aging", "suite-c-fault");
    profile.envelope.temp_roots = 6;
    profile
}

/// What one campaign publishes and what its tests read back.
pub struct Run {
    pub report: FaultReport,
    pub report_bytes: Vec<u8>,
    pub manifest: eval_core::Manifest,
    pub manifest_bytes: Vec<u8>,
    pub bounds: LivenessBounds,
    pub coverage: Coverage,
}

fn unexpected(episode: &str, expected: &str, observed: impl std::fmt::Debug) -> RunError {
    RunError::Unexpected {
        episode: episode.to_string(),
        expected: expected.to_string(),
        observed: format!("{observed:?}"),
    }
}

/// The campaign's mutable evidence while episodes run.
pub struct Witness {
    pub episodes: Vec<FaultEpisode>,
    pub cuts: CutCoverage,
    pub effects: EffectLedger,
    pub refusals: Vec<RecordedRefusal>,
    pub checkpoints: BTreeMap<Cut, u64>,
    pub safety_checks: u64,
    pub coverage: Coverage,
}

impl Witness {
    pub fn new() -> Self {
        let mut cuts = CutCoverage::default();
        for cut in EVENT_CUTS {
            cuts.declare(cut);
        }
        Self {
            episodes: Vec::new(),
            cuts,
            effects: EffectLedger::default(),
            refusals: Vec::new(),
            checkpoints: BTreeMap::new(),
            safety_checks: 0,
            coverage: Coverage::default(),
        }
    }

    fn declare(&mut self, episode: FaultEpisode) -> String {
        let id = episode.id.clone();
        self.cuts.declare(id.clone());
        self.episodes.push(episode);
        id
    }

    fn receipt(&mut self, cut: &str) {
        self.cuts.receipt(cut).unwrap();
    }

    pub fn checkpoint(&mut self, cut: Cut) {
        *self.checkpoints.entry(cut).or_insert(0) += 1;
    }

    /// The invariants that must hold while a fault is armed: the projection's
    /// connection verifies, no descriptor claims a commit past the tip or an
    /// invalidation before its creation, and the projection never runs ahead of the kernel.
    pub fn safety_check(&mut self, stores: &Stores) {
        stores.projection.verify_connection().unwrap();
        let snapshot = stores.snapshot();
        for (object_id, descriptor) in &snapshot.kernel {
            assert!(
                descriptor.created_commit_seq <= snapshot.commit_seq,
                "{object_id} created after the tip"
            );
            if let Some(invalidated) = descriptor.invalidated_commit_seq {
                assert!(
                    invalidated > descriptor.created_commit_seq,
                    "{object_id} invalidated before it was created"
                );
            }
        }
        let acknowledged = snapshot.commit_seq - stores.pending(WorkCounter::CatchUpLag) as i64;
        assert!(acknowledged <= snapshot.commit_seq);
        self.safety_checks += 1;
    }
}

const EVENT_CUTS: [&str; 6] = [
    "local_staged",
    "local_released",
    "acknowledgement_requested",
    "acknowledged",
    "lock_blocked",
    "lock_released",
];

fn episode(
    id: &str,
    step: u32,
    store: StoreFamily,
    operation: &str,
    action: FaultAction,
    contract: &str,
) -> FaultEpisode {
    let heal = action.heal();
    let kill = action.is_kill().then(|| KillLabel {
        crash_model: APPLICATION_CRASH.to_string(),
        page_cache_intact: true,
        killed_process: TEST_BINARY_CHILD.to_string(),
    });
    FaultEpisode {
        id: id.to_string(),
        trigger_step: step,
        scope: FaultScope {
            store,
            operation: operation.to_string(),
        },
        action,
        heal,
        layer_contract: contract.to_string(),
        kill,
    }
}

fn cut_of(event: &EpisodeEvent) -> &'static str {
    match event {
        EpisodeEvent::HoldExtensionRequested { .. } => "hold_extension_requested",
        EpisodeEvent::LocalStaged { .. } => "local_staged",
        EpisodeEvent::LocalReleased { .. } => "local_released",
        EpisodeEvent::AcknowledgementRequested { .. } => "acknowledgement_requested",
        EpisodeEvent::Acknowledged { .. } => "acknowledged",
    }
}

/// A catch-up episode under `fault`; the effect whose reply the fault loses is
/// attempted when the drive requests it and left `Unknown` when the episode ends.
pub fn lost_reply_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    id: &str,
    step: u32,
    now: i64,
    fault: SearchEpisodeFault,
) -> Result<EpisodeReport, RunError> {
    let (production, operation, contract) = match fault {
        SearchEpisodeFault::LoseLocalCommitReply => (
            EpisodeFault::LoseLocalCommitReply,
            "local_commit",
            "search_catchup::EpisodeFault::LoseLocalCommitReply: the batch commits, then its reply arrives as a store failure whose effect is unknown",
        ),
        SearchEpisodeFault::LoseAcknowledgementReply => (
            EpisodeFault::LoseAcknowledgementReply,
            "acknowledge",
            "search_catchup::EpisodeFault::LoseAcknowledgementReply: the acknowledgement commits, then its reply arrives as a kernel I/O failure",
        ),
        SearchEpisodeFault::LoseAcknowledgementReplyAndCancel
        | SearchEpisodeFault::AcknowledgeInsideLocalTransaction => {
            unreachable!("the campaign declares only reply-loss episodes")
        }
    };
    witness.declare(episode(
        id,
        step,
        StoreFamily::SearchProjection,
        operation,
        FaultAction::SearchEpisode { fault },
        contract,
    ));
    stores.publish_outbox_now();
    let mut events = Vec::new();
    let report = stores.episode(now, Some(production), &mut |event| events.push(event));
    let mut lost = None;
    for event in &events {
        if EVENT_CUTS.contains(&cut_of(event)) {
            witness.receipt(cut_of(event));
        }
        match (fault, event) {
            (SearchEpisodeFault::LoseLocalCommitReply, EpisodeEvent::LocalStaged { through }) => {
                let identity = format!("search_commit:{through}");
                witness.effects.attempt(&identity);
                lost = Some(identity);
            }
            (
                SearchEpisodeFault::LoseAcknowledgementReply,
                EpisodeEvent::AcknowledgementRequested { through },
            ) => {
                let identity = format!("search_ack:{through}");
                witness.effects.attempt(&identity);
                lost = Some(identity);
            }
            _ => {}
        }
    }
    let identity =
        lost.ok_or_else(|| unexpected(id, "the faulted effect was requested", &events))?;
    witness.effects.lose_reply(&identity).unwrap();
    witness.receipt(id);
    witness.safety_check(stores);
    Ok(report)
}

/// An external `BEGIN IMMEDIATE` holder on the projection blocks the local
/// commit; releasing it lets the next episode reach the target.
pub fn lock_holder_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    id: &str,
    step: u32,
    now: i64,
) -> Result<(), RunError> {
    witness.declare(episode(
        id,
        step,
        StoreFamily::SearchProjection,
        "local_commit",
        FaultAction::ExternalLockHolder,
        "an external connection holds BEGIN IMMEDIATE on the projection; the batch never commits and nothing is acknowledged",
    ));
    stores.publish_outbox_now();
    let holder = hold_write_lock(&search_file(stores.root()));
    let blocked = stores.episode(now, None, &mut |_| {});
    match &blocked.end {
        EpisodeEnd::Blocked(Blocked::LocalCommitUnresolved) => witness.receipt("lock_blocked"),
        other => return Err(unexpected(id, "Blocked(LocalCommitUnresolved)", other)),
    }
    witness.safety_check(stores);
    drop(holder);
    let released = stores.episode(now, None, &mut |_| {});
    if released.end != EpisodeEnd::ReachedTarget {
        return Err(unexpected(id, "ReachedTarget after release", &released.end));
    }
    witness.receipt("lock_released");
    witness.receipt(id);
    witness
        .coverage
        .record("flt_external_lock_holder_released")
        .unwrap();
    Ok(())
}

pub fn hold_write_lock(path: &Path) -> Connection {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn
}

/// Reads every lost reply back by its identity from the closed files.
pub fn read_back(root: &Path, witness: &mut Witness) {
    let search = read_only(&search_file(root));
    let kernel = read_only(&kernel_file(root));
    for identity in witness.effects.unknown() {
        let (kind, key) = identity.split_once(':').unwrap();
        let state = match kind {
            "search_commit" => {
                let through: i64 = key.parse().unwrap();
                let checkpoint: i64 = search
                    .query_row(
                        "SELECT checkpoint_commit_seq FROM projection_checkpoint",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                applied(checkpoint >= through)
            }
            "search_ack" => {
                let through: i64 = key.parse().unwrap();
                let checkpoint: i64 = kernel
                    .query_row(
                        "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                        [CONSUMER],
                        |row| row.get(0),
                    )
                    .unwrap();
                applied(checkpoint >= through)
            }
            other => panic!("no read-back for effect kind {other}"),
        };
        witness.effects.read_back(&identity, state).unwrap();
    }
}

fn applied(is: bool) -> EffectState {
    if is {
        EffectState::Applied
    } else {
        EffectState::NotApplied
    }
}

/// The campaign: a healthy prefix, the fault phase, recovery by reopen with
/// read-back, and the rest of the history.
pub fn campaign(
    plan: &Plan,
    charges: &mut Charges,
    witness: &mut Witness,
) -> Result<BTreeMap<String, EffectState>, RunError> {
    let k = plan.checkpoint_step as usize;
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..k]);
    witness.checkpoint(Cut::AtQuiescence);
    let steps = &plan.steps[k..];
    assert!(
        steps.len() >= 6,
        "the fault phase needs six steps after the checkpoint"
    );
    let step = |i: usize| (k + i) as u32;

    stores.apply(&steps[0]);
    lost_reply_episode(
        &mut stores,
        witness,
        "search-commit-reply-lost",
        step(0),
        steps[0].now_ms,
        SearchEpisodeFault::LoseLocalCommitReply,
    )?;
    stores.drain(steps[0].now_ms);

    stores.apply(&steps[1]);
    lost_reply_episode(
        &mut stores,
        witness,
        "search-ack-reply-lost",
        step(1),
        steps[1].now_ms,
        SearchEpisodeFault::LoseAcknowledgementReply,
    )?;
    stores.drain(steps[1].now_ms);

    stores.apply(&steps[2]);
    lock_holder_episode(
        &mut stores,
        witness,
        "projection-lock-holder",
        step(2),
        steps[2].now_ms,
    )?;
    stores.drain(steps[2].now_ms);

    for planned in &steps[3..5] {
        stores.apply(planned);
        stores.drain(planned.now_ms);
    }
    witness.checkpoint(Cut::AfterFaultPhase);

    let closed = stores.close();
    read_back(closed.root(), witness);
    let expected: BTreeMap<String, EffectState> = BTreeMap::new();
    let mut stores = closed.reopen(steps[5].now_ms);
    witness.checkpoint(Cut::AfterRecovery);
    witness
        .coverage
        .record("flt_lost_reply_unknown_until_readback")
        .unwrap();
    witness.safety_check(&stores);
    live(&mut stores, &steps[5..]);
    witness.checkpoint(Cut::EndOfRun);
    drop(stores.close());
    charges.vacate(root)?;
    Ok(expected)
}

pub fn run(config: &Config) -> Result<Run, RunError> {
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let plan = aging::plan(config.messages)?;
    let steps = plan.steps.len() as u32;
    let profile = profile(
        config.scale,
        steps,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    profile.approved()?;
    prepare_publish(&config.publish, &[REPORT_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let mut charges = Charges::new(profile.envelope.clone());
    let mut witness = Witness::new();
    let expected = campaign(&plan, &mut charges, &mut witness)?;
    for (identity, state) in &expected {
        let effect = &witness.effects.effects[identity];
        if effect.expected != (eval_core::Expected::Exactly { state: *state }) {
            return Err(unexpected(
                identity,
                &format!("{state:?}"),
                &effect.expected,
            ));
        }
    }
    witness.cuts.verdict().map_err(FaultReportError::Coverage)?;
    witness
        .coverage
        .record("flt_every_declared_cut_receipted")
        .unwrap();
    let declared = [
        Cut::AtQuiescence,
        Cut::AfterFaultPhase,
        Cut::AfterRecovery,
        Cut::EndOfRun,
    ];
    let identity = identity(
        &profile,
        SIMULATOR_VERSION,
        aging::SEED,
        json!({
            "steps": steps,
            "fault_phase_step": plan.checkpoint_step,
            "messages": config.messages,
        }),
        &std::env::current_exe().unwrap(),
    );
    let bounds = profile.statistics.liveness_bounds.clone();
    let mut report = FaultReport {
        schema: FAULT_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        profile_digest: profile.digest().unwrap(),
        claim_boundary: ClaimBoundary::pinned(),
        episodes: witness.episodes.clone(),
        barriers: Vec::new(),
        cuts: cut_receipts(&declared, &witness.checkpoints),
        coverage: witness.cuts.clone(),
        effects: witness.effects.clone(),
        expected_refusals: witness.refusals.clone(),
        safety_checks_while_armed: witness.safety_checks,
        liveness: None,
        markers: witness
            .coverage
            .fired()
            .iter()
            .map(|m| m.to_string())
            .collect(),
        envelope: charges.envelope.clone(),
    };
    charges.retain_publish_root()?;
    let bytes = loop {
        report.envelope = charges.envelope.clone();
        let bytes = serde_json::to_vec_pretty(&report.serialize(&bounds)?).unwrap();
        let peak = charges.envelope.peaks.artifact_bytes;
        charges.observe(eval_core::Resource::ArtifactBytes, bytes.len() as u64)?;
        if charges.envelope.peaks.artifact_bytes == peak {
            break bytes;
        }
    };
    let published: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let manifest = suite_c_manifest(ManifestInputs {
        identity,
        eval_run_id: report.eval_run_id.clone(),
        sample: format!("fault:{}", plan.checkpoint_step),
        result_digest: FaultReport::result_digest(&published)?,
        witness_digest: FaultReport::result_digest(&published)?,
        cut_receipts: report.cuts.clone(),
        execution_mode: ExecutionMode::Generate,
        envelope: report.envelope.clone(),
        started_at_ms,
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    publish_file(&config.publish.join(REPORT_FILE), &bytes).map_err(publish_refused)?;
    publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes).map_err(publish_refused)?;
    Ok(Run {
        report,
        report_bytes: bytes,
        manifest,
        manifest_bytes,
        bounds,
        coverage: witness.coverage,
    })
}

pub const USAGE: &str = "fault --scale <s0|s1|s2> --messages <n> --elapsed-bound-ms <n> \
--approved-by <name> --approval-run-id <hex64> --publish <dir>";

pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    aging::config_from_args(args).map_err(|error| error.replace(aging::USAGE, USAGE))
}
