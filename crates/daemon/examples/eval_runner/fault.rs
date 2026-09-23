//! Suite C fault campaign: contract-faithful fault episodes on the aging drive,
//! judged by `eval_core::fault`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daemon::embedding_publication::{
    EmbeddingPublisher, Publication, PublicationError, PublicationEvent, PublicationFault,
    VectorPublication,
};
use daemon::search_catchup::{Blocked, EpisodeEnd, EpisodeEvent, EpisodeFault, EpisodeReport};
use eval_core::{
    APPLICATION_CRASH, Approval, ArtifactDeletionFaultKind, ArtifactIngestFaultKind, ClaimBoundary,
    Coverage, Cut, CutCoverage, EffectLedger, EffectState, EnvelopeExceeded, ExecutionMode,
    ExpectedRefusal, FAULT_REPORT_SCHEMA, FaultAction, FaultEpisode, FaultReport, FaultReportError,
    FaultScope, Heal, KillLabel, LivenessBounds, ProfileError, PublicationFaultKind,
    RecordedRefusal, RestoreRefused, RunProfile, Scale, SearchEpisodeFault, StoreFamily,
    TEST_BINARY_CHILD, WorkCounter, cut_receipts, eval_run_id,
};
use kernel::{
    ArtifactDeletionFault, ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest,
    ArtifactDestination, ArtifactError, ArtifactIngestFault, ArtifactIngestRequest,
    CurrentInputDescriptor, DomainSpec, EligibilityBinding, ProjectScope, ProviderEgress,
    Sensitivity,
};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, EvidenceAvailability, MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT,
    MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES, MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
    MemoryReviewerJobError, MemoryReviewerJobOutcome, MemoryReviewerJobRefusal, ProducerBinding,
    ReviewTarget,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use super::aging::{
    self, ManifestInputs, Plan, Planned, Step, Stores, kernel_file, live, read_only, search_file,
    suite_c_manifest,
};
use super::campaign::{Charges, identity, prepare_publish, publish_file};
use super::support::embedding_fixtures::{CONSUMER, PROJECT, TestEngine, generation, intent};

pub const REPORT_FILE: &str = "suite-c-fault-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIMULATOR_VERSION: &str = "eval-fault-shell/v1";
const FAULT_DOMAIN: &str = "eval-fault-domain";

pub type Config = aging::Config;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("plan refused: {0}")]
    Plan(#[from] aging::RunError),
    #[error(
        "history leaves {after_checkpoint} steps after the checkpoint; the fault phase drives {FAULT_PHASE_STEPS}, then a publish step for each of two publication faults and a step to live after recovery"
    )]
    HistoryTooShort { after_checkpoint: usize },
    #[error("report refused: {0}")]
    Report(#[from] FaultReportError),
    #[error("episode {episode}: expected {expected}, observed {observed}")]
    Unexpected {
        episode: String,
        expected: String,
        observed: String,
    },
    #[error(
        "read-back of {identity}: checkpoint {checkpoint} is past the faulted episode's {left_at}"
    )]
    ReadBackMasked {
        identity: String,
        left_at: i64,
        checkpoint: i64,
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

/// The steps the fault phase drives after the checkpoint: a lock-holder
/// episode, two healthy steps, and two reply-loss episodes, then the
/// corruption and CAS episodes at the sixth step's time.
const FAULT_PHASE_STEPS: usize = 6;

/// From the sixth step on, each publication fault applies steps until a
/// publish opens an embedding job (a retirement opens none), and recovery
/// needs a step left to live after the second.
fn publication_probes_fit(steps: &[Planned]) -> bool {
    let mut publishes =
        (FAULT_PHASE_STEPS - 1..steps.len()).filter(|i| matches!(steps[*i].step, Step::Publish(_)));
    publishes
        .nth(1)
        .is_some_and(|second| second + 1 < steps.len())
}

/// The aging plan, refused before any store opens when its checkpoint leaves
/// fewer steps than the fault phase drives or too few publishes for the
/// publication faults.
pub fn plan(messages: u32) -> Result<Plan, RunError> {
    let plan = aging::plan(messages)?;
    let k = plan.checkpoint_step as usize;
    let after_checkpoint = plan.steps.len() - k;
    if after_checkpoint < FAULT_PHASE_STEPS || !publication_probes_fit(&plan.steps[k..]) {
        return Err(RunError::HistoryTooShort { after_checkpoint });
    }
    Ok(plan)
}

pub fn profile(
    scale: Scale,
    messages: u32,
    elapsed_ms: u64,
    approval: Option<Approval>,
) -> RunProfile {
    let mut profile = aging::profile(scale, messages, elapsed_ms, approval);
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
    /// The approved profile's limits the report was validated against.
    pub limits: eval_core::ResourceLimits,
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
    pub left_at: BTreeMap<String, i64>,
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
            left_at: BTreeMap::new(),
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

    /// The safety check after an episode returned or the stores reopened:
    /// the projection's connection verifies and the invariants hold. No fault
    /// is armed then, so the check is not counted as one made while armed.
    pub fn safety_check(&mut self, stores: &Stores) {
        stores.projection.verify_connection().unwrap();
        safety_invariants(stores);
    }

    /// The safety check at a cut where a fault is armed, run from the
    /// episode's observer. At `LocalStaged` the projection connection is held
    /// by the episode, so this reads the files and the kernel, not the
    /// projection handle; it is the check `safety_checks_while_armed` counts.
    pub fn safety_check_while_armed(&mut self, stores: &Stores) {
        safety_invariants(stores);
        self.safety_checks += 1;
    }
}

/// No descriptor claims a commit past the tip or an invalidation before its
/// creation, and the projection never runs ahead of the kernel.
fn safety_invariants(stores: &Stores) {
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
}

const EVENT_CUTS: [&str; 14] = [
    "local_staged",
    "local_released",
    "acknowledgement_requested",
    "acknowledged",
    "lock_blocked",
    "lock_released",
    "integrity_refused",
    "deletion_unpropagated",
    "quota_refused",
    "publication_reconciled",
    "reconciling",
    "reconciliation_read",
    "artifact_fault_named",
    "ingestion_latched",
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

struct Seam {
    production: EpisodeFault,
    operation: &'static str,
    contract: &'static str,
    fixed: EffectState,
}

fn seam(fault: SearchEpisodeFault) -> Seam {
    match fault {
        SearchEpisodeFault::LoseLocalCommitReply => Seam {
            production: EpisodeFault::LoseLocalCommitReply,
            operation: "local_commit",
            contract: "search_catchup::EpisodeFault::LoseLocalCommitReply: the batch commits, then its reply arrives as a store failure whose effect is unknown",
            fixed: EffectState::Applied,
        },
        SearchEpisodeFault::LoseAcknowledgementReply => Seam {
            production: EpisodeFault::LoseAcknowledgementReply,
            operation: "acknowledge",
            contract: "search_catchup::EpisodeFault::LoseAcknowledgementReply: the acknowledgement commits, then its reply arrives as a kernel I/O failure",
            fixed: EffectState::Applied,
        },
        SearchEpisodeFault::LoseAcknowledgementReplyAndCancel
        | SearchEpisodeFault::AcknowledgeInsideLocalTransaction => {
            unreachable!("the campaign declares only reply-loss episodes")
        }
    }
}

pub fn lost_reply_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    id: &str,
    step: u32,
    now: i64,
    fault: SearchEpisodeFault,
) -> Result<BTreeMap<String, EffectState>, RunError> {
    declare_lost_reply_episode(witness, id, step, fault);
    stores.publish_outbox_now();
    let mut events = Vec::new();
    let stores = &*stores;
    // The fault is armed for the whole episode and consumed when it returns,
    // so the counted safety check runs at the cut whose reply the fault loses.
    let report = stores.episode(now, Some(seam(fault).production), &mut |event| {
        if matches!(
            (fault, &event),
            (
                SearchEpisodeFault::LoseLocalCommitReply,
                EpisodeEvent::LocalStaged { .. }
            ) | (
                SearchEpisodeFault::LoseAcknowledgementReply,
                EpisodeEvent::AcknowledgementRequested { .. }
            )
        ) {
            witness.safety_check_while_armed(stores);
        }
        events.push(event)
    });
    let fixed = receipt_lost_reply_episode(witness, id, fault, &report, &events)?;
    witness.safety_check(stores);
    Ok(fixed)
}

pub fn declare_lost_reply_episode(
    witness: &mut Witness,
    id: &str,
    step: u32,
    fault: SearchEpisodeFault,
) {
    let Seam {
        operation,
        contract,
        ..
    } = seam(fault);
    witness.declare(episode(
        id,
        step,
        StoreFamily::SearchProjection,
        operation,
        FaultAction::SearchEpisode { fault },
        contract,
    ));
}

pub fn receipt_lost_reply_episode(
    witness: &mut Witness,
    id: &str,
    fault: SearchEpisodeFault,
    report: &EpisodeReport,
    events: &[EpisodeEvent],
) -> Result<BTreeMap<String, EffectState>, RunError> {
    if report.end != EpisodeEnd::ReachedTarget {
        return Err(unexpected(
            id,
            "ReachedTarget after reconciling the lost reply",
            &report.end,
        ));
    }
    let mut lost = Vec::new();
    for event in events {
        if EVENT_CUTS.contains(&cut_of(event)) {
            witness.receipt(cut_of(event));
        }
        match (fault, event) {
            (SearchEpisodeFault::LoseLocalCommitReply, EpisodeEvent::LocalStaged { through }) => {
                lost.push(format!("search_commit:{through}"));
            }
            (
                SearchEpisodeFault::LoseAcknowledgementReply,
                EpisodeEvent::AcknowledgementRequested { through },
            ) => {
                lost.push(format!("search_ack:{through}"));
            }
            _ => {}
        }
    }
    if lost.is_empty() {
        return Err(unexpected(id, "the faulted effect was requested", events));
    }
    let state = seam(fault).fixed;
    let mut fixed = BTreeMap::new();
    for identity in lost {
        witness.effects.attempt(&identity);
        witness.effects.lose_reply(&identity, id).unwrap();
        witness
            .left_at
            .insert(identity.clone(), report.acknowledged_through);
        fixed.insert(identity, state);
    }
    witness.receipt(id);
    Ok(fixed)
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
    witness.safety_check_while_armed(stores);
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

fn ingest_request(key: &str, payload: &[u8]) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        intent: intent(&format!("fault-ingest-{key}")),
        payload: payload.to_vec(),
        evidence_id: format!("eval-fault-evidence-{key}"),
        object_id: format!("eval-fault-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: FAULT_DOMAIN.to_string(),
        source_kind: "conversation".to_string(),
        source_id: format!("eval-fault/{key}"),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "durable".to_string(),
        retain_until: None,
        asserted_sensitivity: Sensitivity::Normal,
        provider_egress: ProviderEgress::LocalOnly,
        provenance: None,
    }
}

fn seed_fault_domain(stores: &Stores) {
    stores
        .corpus
        .kernel
        .commit(intent("fault-domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: FAULT_DOMAIN.into(),
                object_id: "eval-fault-domain-object".into(),
                name: "eval-fault".into(),
                source_kind: "fixture".into(),
                source_id: FAULT_DOMAIN.into(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(FAULT_DOMAIN.into())
        })
        .unwrap();
}

fn artifact_error_text(error: &ArtifactError) -> String {
    format!("{:?}: {error}", error.kind())
}

/// A plain ingest after a CAS EIO must be refused `IngestionFailClosed`: the
/// runner observes the latch before the reopen that heals it.
fn observe_latched(stores: &Stores, witness: &mut Witness, id: &str) -> Result<(), RunError> {
    let latched = stores
        .corpus
        .kernel
        .ingest_artifact(ingest_request(&format!("{id}-latched"), id.as_bytes()))
        .err()
        .ok_or_else(|| unexpected(id, "ingestion latched closed", "Ok"))?;
    if latched.kind() != kernel::ArtifactErrorKind::IngestionFailClosed {
        return Err(unexpected(
            id,
            "IngestionFailClosed while latched",
            artifact_error_text(&latched),
        ));
    }
    witness.receipt("ingestion_latched");
    Ok(())
}

/// Closes and reopens the stores: the heal every latched CAS fault permits.
/// Closing checkpoints the WALs away, so the footprint is charged first.
fn reopen(stores: Stores, charges: &mut Charges, now: i64) -> Result<Stores, RunError> {
    charges.store_bytes(stores.root())?;
    Ok(stores.close().reopen(now))
}

/// Every CAS ingest fault fails closed with its named kind, latches ingestion
/// closed until the store reopens, and leaves no reference; after the reopen
/// the same payload ingests under a fresh intent.
pub fn artifact_ingest_episodes(
    mut stores: Stores,
    witness: &mut Witness,
    charges: &mut Charges,
    step: u32,
    now: i64,
) -> Result<(Stores, String), RunError> {
    seed_fault_domain(&stores);
    let faults = [
        (
            ArtifactIngestFaultKind::Write,
            ArtifactIngestFault::Write,
            "EIO on the temporary object write",
        ),
        (
            ArtifactIngestFaultKind::FileSync,
            ArtifactIngestFault::FileSync,
            "EIO on the object fsync",
        ),
        (
            ArtifactIngestFaultKind::Rename,
            ArtifactIngestFault::Rename,
            "the object rename fails before publication",
        ),
        (
            ArtifactIngestFaultKind::AfterDirectorySync,
            ArtifactIngestFault::AfterDirectorySync,
            "the directory fsync hook fails after the object is published to its directories",
        ),
    ];
    let mut evidence = String::new();
    for (kind, fault, contract) in faults {
        let id = format!(
            "artifact-ingest-{}",
            serde_json::to_value(kind).unwrap().as_str().unwrap()
        );
        witness.declare(episode(
            &id,
            step,
            StoreFamily::Kernel,
            "ingest_artifact",
            FaultAction::ArtifactIngest { fault: kind },
            &format!("kernel::ArtifactIngestFault: {contract}; the ingest is refused IngestionFailClosed, publishes no reference, and latches CAS ingestion closed until the store reopens"),
        ));
        let payload = format!("fault payload {id} {now}");
        let request = ingest_request(&id, payload.as_bytes());
        let evidence_id = request.evidence_id.clone();
        let error = stores
            .corpus
            .kernel
            .ingest_artifact_with_fault_for_test(request, fault)
            .err()
            .ok_or_else(|| unexpected(&id, "an ingest refusal", "Ok"))?;
        let named = artifact_error_text(&error);
        if error.kind() != kernel::ArtifactErrorKind::IngestionFailClosed {
            return Err(unexpected(&id, "IngestionFailClosed", named));
        }
        let references: i64 = read_only(&kernel_file(stores.root()))
            .query_row(
                "SELECT COUNT(*) FROM evidence_meta WHERE evidence_id=?1",
                [&evidence_id],
                |row| row.get(0),
            )
            .unwrap();
        if references != 0 {
            return Err(unexpected(
                &id,
                "no reference published by the faulted ingest",
                references,
            ));
        }
        observe_latched(&stores, witness, &id)?;
        witness.receipt("artifact_fault_named");
        // The latch holds until the reopen, so the fault is still armed here.
        witness.safety_check_while_armed(&stores);
        stores = reopen(stores, charges, now)?;
        let healed = stores
            .corpus
            .kernel
            .ingest_artifact(ingest_request(&format!("{id}-healed"), payload.as_bytes()))
            .map_err(|e| {
                unexpected(&id, "a healed ingest after reopen", artifact_error_text(&e))
            })?;
        evidence = healed.evidence_id;
        witness.receipt(&id);
    }
    witness
        .coverage
        .record("flt_artifact_fault_named_errno")
        .unwrap();
    Ok((stores, evidence))
}

fn evidence_digest(root: &Path, evidence_id: &str) -> String {
    read_only(&kernel_file(root))
        .query_row(
            "SELECT artifact_digest FROM evidence_meta WHERE evidence_id=?1",
            [evidence_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn deletion_request(
    key: &str,
    digest: &str,
    kind: ArtifactDeletionKind,
) -> ArtifactDeletionRequest {
    let purge = kind == ArtifactDeletionKind::Purge;
    ArtifactDeletionRequest {
        intent: intent(&format!("fault-delete-{key}")),
        identity: ArtifactDeletionIdentity::Digest(digest.to_string()),
        kind,
        operator_id: purge.then(|| "eval-fault".to_string()),
        target_locator: purge.then(|| "incident://eval-fault".to_string()),
        reason: purge.then(|| "fault campaign".to_string()),
        deleted_at: 42,
    }
}

/// Purge-intent faults fail with their named kind: ENOSPC is refused and
/// consumed, EIO is refused and latches CAS until reopen. The healed
/// deletion then commits a deletion the projection refuses to acknowledge
/// past (R11), a permanent stall production heals only by rebuilding.
pub fn artifact_deletion_episodes(
    mut stores: Stores,
    witness: &mut Witness,
    charges: &mut Charges,
    evidence_id: &str,
    step: u32,
    now: i64,
) -> Result<Stores, RunError> {
    let digest = evidence_digest(stores.root(), evidence_id);
    let faults = [
        (
            ArtifactDeletionFaultKind::IntentStorageExhausted,
            ArtifactDeletionFault::IntentStorageExhausted,
            kernel::ArtifactErrorKind::StorageExhausted,
            "ENOSPC while appending the purge intent; nothing is latched",
        ),
        (
            ArtifactDeletionFaultKind::IntentAppend,
            ArtifactDeletionFault::IntentAppend,
            kernel::ArtifactErrorKind::PurgeIntent,
            "EIO while appending the purge intent; CAS ingestion latches closed until reopen",
        ),
    ];
    for (kind, fault, expected_kind, contract) in faults {
        let id = format!(
            "artifact-deletion-{}",
            serde_json::to_value(kind).unwrap().as_str().unwrap()
        );
        let heal = FaultAction::ArtifactDeletion { fault: kind }.heal();
        witness.declare(episode(
            &id,
            step,
            StoreFamily::Kernel,
            "delete_artifact",
            FaultAction::ArtifactDeletion { fault: kind },
            &format!("kernel::ArtifactDeletionFault: {contract}"),
        ));
        let error = stores
            .corpus
            .kernel
            .delete_artifact_with_fault_for_test(
                deletion_request(&id, &digest, ArtifactDeletionKind::Purge),
                fault,
            )
            .err()
            .ok_or_else(|| unexpected(&id, "a deletion refusal", "Ok"))?;
        if error.kind() != expected_kind {
            return Err(unexpected(
                &id,
                &format!("{expected_kind:?}"),
                artifact_error_text(&error),
            ));
        }
        witness.receipt("artifact_fault_named");
        if heal == Heal::Reopen {
            // The latch holds until the reopen, so the fault is still armed here.
            witness.safety_check_while_armed(&stores);
            observe_latched(&stores, witness, &id)?;
            stores = reopen(stores, charges, now)?;
        } else {
            witness.safety_check(&stores);
        }
        let probe = stores
            .corpus
            .kernel
            .ingest_artifact(ingest_request(&format!("{id}-probe"), b"probe"));
        if let Err(e) = probe {
            return Err(unexpected(
                &id,
                "ingestion open after the heal",
                artifact_error_text(&e),
            ));
        }
        witness.receipt(&id);
    }
    Ok(stores)
}

/// The healed deletion commits a deletion the projection refuses to
/// acknowledge past (R11): a permanent stall production heals only by
/// rebuilding the projection.
pub fn r11_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    evidence_id: &str,
    step: u32,
    now: i64,
) -> Result<(), RunError> {
    let digest = evidence_digest(stores.root(), evidence_id);
    let r11 = witness.declare(episode(
        "r11-deletion-bearing-catch-up",
        step,
        StoreFamily::SearchProjection,
        "acknowledge",
        FaultAction::ExpectedRefusal {
            refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        },
        "search_catchup::Blocked::DeletionUnpropagated: a plain deletion puts a deletion in the next window, which is refused so the barrier stays unsatisfied while the projection serves the text; production heals it by rebuilding the projection",
    ));
    let before = stores.projection_rows();
    stores
        .corpus
        .kernel
        .delete_artifact(deletion_request(
            "healed",
            &digest,
            ArtifactDeletionKind::Delete,
        ))
        .map_err(|e| unexpected(&r11, "a healed deletion", artifact_error_text(&e)))?;
    stores.publish_outbox_now();
    let report = stores.episode(now, None, &mut |_| {});
    let blocked = match &report.end {
        EpisodeEnd::Blocked(Blocked::DeletionUnpropagated { commit_seq }) => {
            format!("DeletionUnpropagated {{ commit_seq: {commit_seq} }}")
        }
        other => return Err(unexpected(&r11, "Blocked(DeletionUnpropagated)", other)),
    };
    let again = stores.episode(now, None, &mut |_| {});
    if again.end != report.end || again.acknowledged_through != report.acknowledged_through {
        return Err(unexpected(&r11, "a permanent stall", &again));
    }
    // The refused window applied nothing: the rows the projection served
    // before the deletion are the rows it serves while stalled, and nothing
    // was tombstoned on the way to the refusal.
    let after = stores.projection_rows();
    if after != before {
        return Err(unexpected(
            &r11,
            "the projection's rows unchanged by the refused window",
            (&before, &after),
        ));
    }
    witness.receipt("deletion_unpropagated");
    witness.receipt(&r11);
    witness.refusals.push(RecordedRefusal {
        episode: r11,
        refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        production_error: blocked,
    });
    // The stall holds until the reopen rebuilds the projection, so the refusal
    // is still in force here.
    witness.safety_check_while_armed(stores);
    witness
        .coverage
        .record("flt_r11_recorded_as_expected_refusal")
        .unwrap();
    Ok(())
}

pub fn reviewer_producer(firing: &str) -> ProducerBinding {
    ProducerBinding {
        producer: "history-summarizer".to_string(),
        firing_id: firing.to_string(),
        ordinal: 0,
    }
}

pub fn reviewer_inputs(candidate: &str) -> CausalInputs {
    CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: "0a".repeat(16),
            candidate_id: candidate.to_string(),
            payload_digest: "0d".repeat(32),
        },
        question_template: "extracted_facts".to_string(),
        signals: vec!["contradiction".to_string()],
        required_evidence: vec![EvidenceAvailability {
            evidence_id: "ev-1".to_string(),
            available: true,
        }],
        policy_versions: BTreeMap::from([("disclosure".to_string(), "3".to_string())]),
    }
}

/// The receipt quota refuses new reviewer work by name and deletes nothing.
pub fn quota_episode(
    root: &Path,
    witness: &mut Witness,
    step: u32,
    now: i64,
) -> Result<(), RunError> {
    let id = witness.declare(episode(
        "r24-receipt-quota",
        step,
        StoreFamily::Memory,
        "reserve_memory_reviewer_job",
        FaultAction::ExpectedRefusal {
            refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
        },
        "memory_reviewer_jobs::MemoryReviewerJobRefusal::MetadataQuota: receipt charges retained by a terminal job leave less than one admission's charge and allowance under the project quota, so admission refuses with no allowance left to release and deletes no receipt",
    ));
    let store = MemoryStore::open(&daemon::store_descriptor_in(root)).unwrap();
    let reserved = store
        .reserve_memory_reviewer_job(
            PROJECT,
            &reviewer_producer("f1"),
            &reviewer_inputs("cand-1"),
            now,
        )
        .unwrap();
    let causal_identity = match reserved {
        memory_store::memory_reviewer_jobs::ReserveOutcome::Reserved(job) => job.causal_identity,
        other => return Err(unexpected(&id, "a fresh reservation", other)),
    };
    let retained = i64::try_from(
        MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT - MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
    )
    .unwrap();
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "UPDATE memory_reviewer_jobs SET receipt_charge_bytes = ?1 WHERE causal_identity = ?2",
                rusqlite::params![retained, causal_identity],
            )
        })
        .unwrap();
    store
        .finish_memory_reviewer_job(
            PROJECT,
            &causal_identity,
            MemoryReviewerJobOutcome::Failed,
            now,
        )
        .map_err(|e| unexpected(&id, "the planted job closed", e))?;
    let before = store.memory_reviewer_headroom(PROJECT).unwrap();
    let admission = MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES + MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES;
    if before.pending_jobs != 0 || before.project_metadata_remaining >= admission {
        return Err(unexpected(
            &id,
            "no open allowance and less than one admission's headroom",
            before,
        ));
    }
    let error = store
        .reserve_memory_reviewer_job(
            PROJECT,
            &reviewer_producer("f2"),
            &reviewer_inputs("cand-2"),
            now,
        )
        .err()
        .ok_or_else(|| unexpected(&id, "MetadataQuota", "Ok"))?;
    let refusal = match error {
        MemoryReviewerJobError::Refused(MemoryReviewerJobRefusal::MetadataQuota) => {
            "MemoryReviewerJobRefusal::MetadataQuota".to_string()
        }
        other => return Err(unexpected(&id, "MetadataQuota", other)),
    };
    let after = store.memory_reviewer_headroom(PROJECT).unwrap();
    if after.pending_jobs != before.pending_jobs
        || after.project_metadata_bytes != before.project_metadata_bytes
    {
        return Err(unexpected(
            &id,
            "nothing deleted by the refusal",
            (before, after),
        ));
    }
    witness.receipt("quota_refused");
    witness.receipt(&id);
    witness.refusals.push(RecordedRefusal {
        episode: id,
        refusal: ExpectedRefusal::R24ReceiptQuotaExhausted,
        production_error: refusal,
    });
    witness
        .coverage
        .record("flt_r24_recorded_as_expected_refusal")
        .unwrap();
    Ok(())
}

/// A quiescent copy with one page of the kernel file overwritten is refused
/// before any store opens; the original reopens as it was.
pub fn corruption_episode(
    stores: Stores,
    witness: &mut Witness,
    charges: &mut Charges,
    step: u32,
    now: i64,
) -> Result<Stores, RunError> {
    let id = witness.declare(episode(
        "corrupt-quiescent-kernel-file",
        step,
        StoreFamily::Kernel,
        "reopen",
        FaultAction::CorruptQuiescentFile,
        "Copied::reopen: a copied file whose bytes no longer match the checkpoint's digest is refused FileDiffers before any store opens; the corruption is detected, not repaired",
    ));
    charges.store_bytes(stores.root())?;
    let mut closed = stores.close();
    let copy_root = charges.occupy()?;
    let (checkpoint, copied) = closed
        .copy(copy_root.path())
        .map_err(|e| unexpected(&id, "an admitted quiescent copy", e))?;
    let target = kernel_file(copied.root());
    let mut bytes = std::fs::read(&target).unwrap();
    let page = 4096usize;
    let start = page * 2;
    assert!(
        bytes.len() > start + page,
        "the kernel copy has a third page"
    );
    for byte in &mut bytes[start..start + page] {
        *byte ^= 0xA5;
    }
    std::fs::write(&target, &bytes).unwrap();
    let relative = target
        .strip_prefix(copied.root())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let expected = format!("FileDiffers {{ path: {relative:?} }}");
    match copied.reopen(&checkpoint, now) {
        Err(RestoreRefused::FileDiffers { path }) if path == relative => {
            witness.receipt("integrity_refused");
        }
        Err(other) => return Err(unexpected(&id, &expected, other)),
        Ok(_) => return Err(unexpected(&id, &expected, "Ok")),
    }
    charges.vacate(copy_root)?;
    let stores = closed.reopen(now);
    witness.receipt(&id);
    witness.safety_check(&stores);
    witness
        .coverage
        .record("flt_corruption_detected_at_quiescence")
        .unwrap();
    Ok(stores)
}

/// One pending embedding job published under a publication fault. Under
/// `LoseLocalCommitReply` the outcome is unknown to the caller, so the job's
/// identity enters the ledger as a lost reply and is returned for the
/// read-back after reopen. Under `LoseLocalCommit` the commit rolled back,
/// which the seam's contract fixes, so the row is read at once and nothing is
/// returned.
pub fn publication_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    id: &str,
    step: u32,
    now: i64,
    fault: PublicationFaultKind,
) -> Result<Option<String>, RunError> {
    let (production, contract) = match fault {
        PublicationFaultKind::LoseLocalCommitReply => (
            PublicationFault::LoseLocalCommitReply,
            "embedding_publication::PublicationFault::LoseLocalCommitReply: the vector and completion commit, then the reply is lost; the durable rows say it landed",
        ),
        PublicationFaultKind::LoseLocalCommit => (
            PublicationFault::LoseLocalCommit,
            "embedding_publication::PublicationFault::LoseLocalCommit: the commit rolls back and the reply arrives as a store failure; the durable rows say it did not land",
        ),
    };
    witness.declare(episode(
        id,
        step,
        StoreFamily::SearchProjection,
        "publish_embedding",
        FaultAction::EmbeddingPublication { fault },
        contract,
    ));
    let occurrence: String = read_only(&search_file(stores.root()))
        .query_row(
            "SELECT occurrence_id FROM embedding_jobs WHERE state IN ('pending','admitted') \
             ORDER BY occurrence_id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| unexpected(id, "a pending embedding job", e))?;
    let rows = stores.corpus.export();
    let row = rows
        .iter()
        .find(|row| row.detail.occurrence_id == occurrence)
        .ok_or_else(|| unexpected(id, "the pending occurrence in the export", &occurrence))?;
    let project = ProjectScope::new(PROJECT).unwrap();
    let vector = TestEngine::vector_for(row.text.as_deref().unwrap_or_default());
    let identity = format!("embedding:{occurrence}");
    let lost = FaultAction::EmbeddingPublication { fault }.loses_reply();
    if lost {
        witness.effects.attempt(&identity);
    }
    let mut events = Vec::new();
    let stores = &*stores;
    let mut publisher = EmbeddingPublisher::new(&stores.corpus.kernel, &stores.projection);
    let result = publisher.publish_with_fault_for_test(
        &VectorPublication {
            input: CurrentInputDescriptor {
                object_id: row.object_id.clone(),
                source_revision: row.revision,
                detail: row.detail.clone(),
                domain_id: row.domain_id.clone(),
                sensitivity: row.sensitivity,
                created_commit_seq: row.created_commit_seq,
            },
            generation: &generation(),
            vector: &vector,
            input_bytes: row.text.as_ref().map_or(0, |t| t.len() as u64),
            input_tokens: 3,
        },
        EligibilityBinding {
            project: &project,
            destination: ArtifactDestination::Local,
        },
        Instant::now() + Duration::from_secs(10),
        now,
        &mut |event| {
            // The reply is lost and the publisher is reconciling: the fault is
            // armed here, so this is the counted safety check.
            if event == PublicationEvent::Reconciling {
                witness.safety_check_while_armed(stores);
            }
            events.push(event)
        },
        production,
    );
    match (fault, &result) {
        (PublicationFaultKind::LoseLocalCommitReply, Ok(Publication::Embedded))
        | (PublicationFaultKind::LoseLocalCommit, Err(PublicationError::LocalCommitUnresolved)) => {
        }
        _ => return Err(unexpected(id, "the fault's documented outcome", &result)),
    }
    let reconciling = events
        .iter()
        .position(|e| *e == PublicationEvent::Reconciling);
    let read = events
        .iter()
        .rposition(|e| *e == PublicationEvent::ReconciliationRead);
    match (reconciling, read) {
        (Some(started), Some(read)) if started < read => {
            witness.receipt("reconciling");
            witness.receipt("reconciliation_read");
        }
        _ => {
            return Err(unexpected(
                id,
                "Reconciling then ReconciliationRead",
                &events,
            ));
        }
    }
    let job_state = || -> Result<String, RunError> {
        read_only(&search_file(stores.root()))
            .query_row(
                "SELECT state FROM embedding_jobs WHERE occurrence_id=?1",
                [&occurrence],
                |row| row.get(0),
            )
            .map_err(|e| unexpected(id, "the job's row", e))
    };
    let lost = if lost {
        witness.effects.lose_reply(&identity, id).unwrap();
        Some(identity)
    } else {
        // The rollback is the contract's fixed outcome, so the row is read
        // now: it must not say the vector landed.
        let state = job_state()?;
        if state == "embedded" {
            return Err(unexpected(id, "a job the rollback left open", state));
        }
        None
    };
    witness.receipt("publication_reconciled");
    witness.receipt(id);
    witness.safety_check(stores);
    Ok(lost)
}

/// Applies planned steps from `next`, catching up after each, until an
/// embedding job is open, and returns the index of the last step applied; a
/// retirement opens none, so a history that runs out first is refused.
fn open_embedding_job(
    stores: &mut Stores,
    steps: &[Planned],
    next: &mut usize,
    id: &str,
) -> Result<usize, RunError> {
    while stores.pending(WorkCounter::EmbeddingOpen) == 0 {
        let planned = steps
            .get(*next)
            .ok_or_else(|| unexpected(id, "a planned step that opens an embedding job", *next))?;
        stores.apply(planned);
        stores.catch_up(planned.now_ms);
        *next += 1;
    }
    Ok(*next - 1)
}

/// Reads every lost reply back by its identity from the closed files.
/// Read-back refuses checkpoints past the recorded fault boundary because monotonic checkpoints cannot prove the faulted effect.
/// An embedding job's row names its own state, so it is read directly.
pub fn read_back(root: &Path, witness: &mut Witness) -> Result<(), RunError> {
    let search = read_only(&search_file(root));
    let projection: i64 = search
        .query_row(
            "SELECT checkpoint_commit_seq FROM projection_checkpoint",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let kernel: i64 = read_only(&kernel_file(root))
        .query_row(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
            [CONSUMER],
            |row| row.get(0),
        )
        .unwrap();
    let mut states = Vec::new();
    for identity in witness.effects.unknown() {
        let (kind, key) = identity.split_once(':').unwrap();
        let checkpoint = match kind {
            "search_commit" => projection,
            "search_ack" => kernel,
            "embedding" => {
                let state: String = search
                    .query_row(
                        "SELECT state FROM embedding_jobs WHERE occurrence_id=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .unwrap();
                states.push((identity, applied(state == "embedded")));
                continue;
            }
            other => panic!("no read-back for effect kind {other}"),
        };
        let left_at = witness.left_at[&identity];
        if checkpoint > left_at {
            return Err(RunError::ReadBackMasked {
                identity,
                left_at,
                checkpoint,
            });
        }
        let through: i64 = key.parse().unwrap();
        states.push((identity, applied(checkpoint >= through)));
    }
    for (identity, state) in states {
        witness.effects.read_back(&identity, state).unwrap();
    }
    Ok(())
}

fn applied(is: bool) -> EffectState {
    if is {
        EffectState::Applied
    } else {
        EffectState::NotApplied
    }
}

/// Each lost reply is recovered before any later catch-up can advance the
/// checkpoint its read-back reads.
pub fn campaign(
    plan: &Plan,
    charges: &mut Charges,
    witness: &mut Witness,
) -> Result<BTreeMap<String, EffectState>, RunError> {
    let k = plan.checkpoint_step as usize;
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan);
    live(&mut stores, &plan.steps[..k]);
    witness.checkpoint(Cut::AtQuiescence);
    let steps = &plan.steps[k..];
    assert!(
        steps.len() >= FAULT_PHASE_STEPS,
        "plan() refuses a history with fewer than {FAULT_PHASE_STEPS} steps after the checkpoint"
    );
    let step = |i: usize| (k + i) as u32;
    let mut expected = BTreeMap::new();

    stores.apply(&steps[0]);
    lock_holder_episode(
        &mut stores,
        witness,
        "projection-lock-holder",
        step(0),
        steps[0].now_ms,
    )?;
    stores.drain(steps[0].now_ms);

    for planned in &steps[1..3] {
        stores.apply(planned);
        stores.drain(planned.now_ms);
    }

    stores.apply(&steps[3]);
    expected.extend(lost_reply_episode(
        &mut stores,
        witness,
        "search-commit-reply-lost",
        step(3),
        steps[3].now_ms,
        SearchEpisodeFault::LoseLocalCommitReply,
    )?);
    let mut stores = recover(stores, witness, charges, steps[4].now_ms)?;

    stores.apply(&steps[4]);
    expected.extend(lost_reply_episode(
        &mut stores,
        witness,
        "search-ack-reply-lost",
        step(4),
        steps[4].now_ms,
        SearchEpisodeFault::LoseAcknowledgementReply,
    )?);
    let stores = recover(stores, witness, charges, steps[5].now_ms)?;

    let stores = corruption_episode(stores, witness, charges, step(5), steps[5].now_ms)?;
    let (stores, evidence) =
        artifact_ingest_episodes(stores, witness, charges, step(5), steps[5].now_ms)?;
    let mut stores = artifact_deletion_episodes(
        stores,
        witness,
        charges,
        &evidence,
        step(5),
        steps[5].now_ms,
    )?;
    stores.drain(steps[5].now_ms);

    let mut next = 5;
    for (id, fault) in [
        (
            "publication-commit-reply-lost",
            PublicationFaultKind::LoseLocalCommitReply,
        ),
        (
            "publication-commit-lost",
            PublicationFaultKind::LoseLocalCommit,
        ),
    ] {
        let at = open_embedding_job(&mut stores, steps, &mut next, id)?;
        if let Some(lost) =
            publication_episode(&mut stores, witness, id, step(at), steps[at].now_ms, fault)?
        {
            // The vector and completion committed before the reply was lost.
            expected.insert(lost, EffectState::Applied);
        }
    }
    let rest = next;
    let resumed = steps
        .get(rest)
        .ok_or_else(|| {
            unexpected(
                "r11-deletion-bearing-catch-up",
                "a step left to live after recovery",
                rest,
            )
        })?
        .now_ms;

    let quota_root = charges.occupy()?;
    quota_episode(quota_root.path(), witness, step(rest), resumed)?;
    charges.vacate(quota_root)?;
    r11_episode(&mut stores, witness, &evidence, step(rest), resumed)?;
    witness.checkpoint(Cut::AfterFaultPhase);
    let mut stores = recover(stores, witness, charges, resumed)?;

    live(&mut stores, &steps[rest..]);
    witness.checkpoint(Cut::EndOfRun);
    // The stores' bytes are charged while they are open; closing them
    // checkpoints the WAL away.
    charges.store_bytes(root.path())?;
    drop(stores.close());
    charges.vacate(root)?;
    Ok(expected)
}

fn recover(
    stores: Stores,
    witness: &mut Witness,
    charges: &mut Charges,
    now: i64,
) -> Result<Stores, RunError> {
    // Closing checkpoints the WALs away, so the footprint is charged first.
    charges.store_bytes(stores.root())?;
    let closed = stores.close();
    read_back(closed.root(), witness)?;
    let stores = closed.reopen(now);
    witness.checkpoint(Cut::AfterRecovery);
    witness
        .coverage
        .record("flt_lost_reply_unknown_until_readback")
        .unwrap();
    witness.safety_check(&stores);
    Ok(stores)
}

pub fn check_expectations(
    expected: &BTreeMap<String, EffectState>,
    effects: &EffectLedger,
) -> Result<(), RunError> {
    let lost: BTreeSet<&String> = effects
        .effects
        .iter()
        .filter(|(_, effect)| effect.reply_lost())
        .map(|(identity, _)| identity)
        .collect();
    let fixed: BTreeSet<&String> = expected.keys().collect();
    if lost != fixed {
        return Err(unexpected(
            "campaign",
            &format!("fixed expectations for exactly {lost:?}"),
            fixed,
        ));
    }
    for (identity, state) in expected {
        let effect = &effects.effects[identity];
        if !effect.read_back || effect.expected != (eval_core::Expected::Exactly { state: *state })
        {
            return Err(unexpected(
                identity,
                &format!("read back {state:?}"),
                &effect.expected,
            ));
        }
    }
    Ok(())
}

pub fn run(config: &Config) -> Result<Run, RunError> {
    let started_at_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let profile = profile(
        config.scale,
        config.messages,
        config.elapsed_bound_ms,
        config.approval.clone(),
    );
    profile.approved()?;
    prepare_publish(&config.publish, &[REPORT_FILE, MANIFEST_FILE]).map_err(publish_refused)?;
    let mut charges = Charges::new(profile.envelope.clone());
    let mut witness = Witness::new();
    // Planning runs under the clock: the elapsed bound covers the whole run.
    let plan = plan(config.messages)?;
    let steps = plan.steps.len() as u32;
    // The build identity is frozen before the campaign runs: the checkout,
    // the lockfile, and the executable the outcomes come from, not whatever
    // the tree holds when the report is written.
    let identity = identity(
        &profile,
        SIMULATOR_VERSION,
        aging::SEED,
        json!({
            "steps": steps,
            "fault_phase_step": plan.checkpoint_step,
            "messages": config.messages,
        }),
        &[std::env::current_exe().unwrap()],
    );
    let expected = campaign(&plan, &mut charges, &mut witness)?;
    check_expectations(&expected, &witness.effects)?;
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
        let bytes =
            serde_json::to_vec_pretty(&report.serialize(&bounds, &profile.envelope)?).unwrap();
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
    // A manifest the directory then refuses to take takes the report back out
    // with it, as the aging shell does: a reader finds both files or none.
    let report_path = config.publish.join(REPORT_FILE);
    publish_file(&report_path, &bytes).map_err(publish_refused)?;
    if let Err(error) = publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes) {
        let _ = std::fs::remove_file(&report_path);
        return Err(publish_refused(error));
    }
    Ok(Run {
        report,
        report_bytes: bytes,
        manifest,
        manifest_bytes,
        bounds,
        limits: profile.envelope,
        coverage: witness.coverage,
    })
}

pub const USAGE: &str = "fault --scale <s0|s1|s2> --messages <n> --elapsed-bound-ms <n> \
--approved-by <name> --approval-run-id <hex64> --publish <dir>";

pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    aging::config_from_args(args).map_err(|error| error.replace(aging::USAGE, USAGE))
}
