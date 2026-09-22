//! Suite C fault campaign: contract-faithful fault episodes on the aging drive,
//! judged by `eval_core::fault`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daemon::embedding_publication::{
    EmbeddingPublisher, Publication, PublicationError, PublicationFault, VectorPublication,
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
    MemoryReviewerJobError, MemoryReviewerJobRefusal, ProducerBinding, ReviewTarget,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use super::aging::{
    self, ManifestInputs, Plan, Stores, kernel_file, live, read_only, search_file, suite_c_manifest,
};
use super::campaign::{Charges, identity, prepare_publish, publish_file};
use super::support::embedding_fixtures::{PROJECT, TestEngine, generation, intent};

pub const REPORT_FILE: &str = "suite-c-fault-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIMULATOR_VERSION: &str = "eval-fault-shell/v1";
const CONSUMER: &str = "search";
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
    pub barriers: Vec<BarrierReceipt>,
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
            barriers: Vec::new(),
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

const EVENT_CUTS: [&str; 13] = [
    "publication_held",
    "publication_released",
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
    "artifact_fault_named",
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
                let identity = format!("search_commit:{through}@{id}");
                witness.effects.attempt(&identity);
                lost = Some(identity);
            }
            (
                SearchEpisodeFault::LoseAcknowledgementReply,
                EpisodeEvent::AcknowledgementRequested { through },
            ) => {
                let identity = format!("search_ack:{through}@{id}");
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

/// Closes and reopens the stores: the heal every latched CAS fault permits.
fn reopen(stores: Stores, now: i64) -> Stores {
    stores.close().reopen(now)
}

/// Every CAS ingest fault fails closed with its named kind, latches ingestion
/// closed until the store reopens, and leaves no reference; after the reopen
/// the same payload ingests under a fresh intent.
pub fn artifact_ingest_episodes(
    mut stores: Stores,
    witness: &mut Witness,
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
            &format!("kernel::ArtifactIngestFault: {contract}; the ingest fails closed, publishes no reference, and latches CAS ingestion closed until the store reopens"),
        ));
        let payload = format!("fault payload {id} {now}");
        let error = stores
            .corpus
            .kernel
            .ingest_artifact_with_fault_for_test(ingest_request(&id, payload.as_bytes()), fault)
            .err()
            .ok_or_else(|| unexpected(&id, "an ingest refusal", "Ok"))?;
        let named = artifact_error_text(&error);
        if !matches!(
            error.kind(),
            kernel::ArtifactErrorKind::IngestionFailClosed
                | kernel::ArtifactErrorKind::ReferenceCommit
        ) {
            return Err(unexpected(
                &id,
                "IngestionFailClosed | ReferenceCommit",
                named,
            ));
        }
        let latched = stores
            .corpus
            .kernel
            .ingest_artifact(ingest_request(&format!("{id}-latched"), payload.as_bytes()))
            .err()
            .ok_or_else(|| unexpected(&id, "ingestion latched closed", "Ok"))?;
        if latched.kind() != kernel::ArtifactErrorKind::IngestionFailClosed {
            return Err(unexpected(
                &id,
                "IngestionFailClosed while latched",
                artifact_error_text(&latched),
            ));
        }
        witness.receipt("artifact_fault_named");
        witness.safety_check(&stores);
        stores = reopen(stores, now);
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
        witness.safety_check(&stores);
        if heal == Heal::Reopen {
            stores = reopen(stores, now);
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
        FaultAction::ArtifactDeletion {
            fault: ArtifactDeletionFaultKind::AfterCommit,
        },
        "search_catchup::Blocked::DeletionUnpropagated: a window holding a deletion is refused so the barrier stays unsatisfied while the projection serves the text; production heals it by rebuilding the projection",
    ));
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
    witness.receipt("deletion_unpropagated");
    witness.receipt(&r11);
    witness.refusals.push(RecordedRefusal {
        episode: r11,
        refusal: ExpectedRefusal::R11DeletionBearingCatchUp,
        production_error: blocked,
    });
    witness.safety_check(stores);
    witness
        .coverage
        .record("flt_r11_recorded_as_expected_refusal")
        .unwrap();
    Ok(())
}

/// The receipt quota refuses new reviewer work by name and deletes nothing.
pub fn quota_episode(
    root: &Path,
    witness: &mut Witness,
    step: u32,
    now: i64,
) -> Result<(), RunError> {
    let id = witness.declare(FaultEpisode {
        id: "r24-receipt-quota".to_string(),
        trigger_step: step,
        scope: FaultScope {
            store: StoreFamily::Memory,
            operation: "reserve_memory_reviewer_job".to_string(),
        },
        action: FaultAction::ExternalLockHolder,
        heal: Heal::Released,
        layer_contract: "memory_reviewer_jobs::MemoryReviewerJobRefusal::MetadataQuota: a receipt charge at the project quota refuses new admissions and deletes no receipt; the quota is permanent and this run releases nothing".to_string(),
        kill: None,
    });
    let store = MemoryStore::open(&daemon::store_descriptor_in(root)).unwrap();
    let producer = |firing: &str| ProducerBinding {
        producer: "history-summarizer".to_string(),
        firing_id: firing.to_string(),
        ordinal: 0,
    };
    let inputs = |candidate: &str| CausalInputs {
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
    };
    let reserved = store
        .reserve_memory_reviewer_job(PROJECT, &producer("f1"), &inputs("cand-1"), now)
        .unwrap();
    let causal_identity = match reserved {
        memory_store::memory_reviewer_jobs::ReserveOutcome::Reserved(job) => job.causal_identity,
        other => return Err(unexpected(&id, "a fresh reservation", other)),
    };
    let near_quota = i64::try_from(
        MAX_MEMORY_REVIEWER_METADATA_BYTES_PER_PROJECT
            - MEMORY_REVIEWER_JOB_ALLOWANCE_BYTES
            - MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES,
    )
    .unwrap();
    store
        .with_fenced_conn_for_test(|conn| {
            conn.execute(
                "UPDATE memory_reviewer_jobs SET receipt_charge_bytes = ?1 WHERE causal_identity = ?2",
                rusqlite::params![near_quota, causal_identity],
            )
        })
        .unwrap();
    let before = store.memory_reviewer_headroom(PROJECT).unwrap();
    let error = store
        .reserve_memory_reviewer_job(PROJECT, &producer("f2"), &inputs("cand-2"), now)
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
    witness.safety_checks += 1;
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
        "Checkpoint::accept: a copied file whose integrity_check is not ok is refused before any store opens; the corruption is detected, not repaired",
    ));
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
    match copied.reopen(&checkpoint, now) {
        Err(RestoreRefused::IntegrityCheck {
            family: StoreFamily::Kernel,
            ..
        }) => {
            witness.receipt("integrity_refused");
        }
        Err(other) => return Err(unexpected(&id, "IntegrityCheck { kernel }", other)),
        Ok(_) => return Err(unexpected(&id, "IntegrityCheck { kernel }", "Ok")),
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

/// One pending embedding job published under a publication fault; its
/// identity stays `Unknown` until the durable row is read back after reopen.
pub fn publication_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    id: &str,
    step: u32,
    now: i64,
    fault: PublicationFaultKind,
) -> Result<String, RunError> {
    let (production, contract) = match fault {
        PublicationFaultKind::LoseLocalCommitReply => (
            PublicationFault::LoseLocalCommitReply,
            "embedding_publication::PublicationFault::LoseLocalCommitReply: the vector and completion commit, then the reply is lost; the durable rows say it landed",
        ),
        PublicationFaultKind::LoseLocalCommit => (
            PublicationFault::LoseLocalCommit,
            "embedding_publication::PublicationFault::LoseLocalCommit: the commit rolls back and the reply is lost; the durable rows say it did not land",
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
    let identity = format!("embedding:{occurrence}@{id}");
    witness.effects.attempt(&identity);
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
        &mut |_| {},
        production,
    );
    match (fault, &result) {
        (PublicationFaultKind::LoseLocalCommitReply, Ok(Publication::Embedded))
        | (PublicationFaultKind::LoseLocalCommit, Err(PublicationError::LocalCommitUnresolved)) => {
        }
        _ => return Err(unexpected(id, "the fault's documented outcome", &result)),
    }
    witness.effects.lose_reply(&identity).unwrap();
    witness.receipt("publication_reconciled");
    witness.receipt(id);
    witness.safety_check(stores);
    Ok(identity)
}

/// Reads every lost reply back by its identity from the closed files.
pub fn read_back(root: &Path, witness: &mut Witness) {
    let search = read_only(&search_file(root));
    let kernel = read_only(&kernel_file(root));
    for identity in witness.effects.unknown() {
        let (effect, _episode) = identity.split_once('@').unwrap();
        let (kind, key) = effect.split_once(':').unwrap();
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
            "embedding" => {
                let state: String = search
                    .query_row(
                        "SELECT state FROM embedding_jobs WHERE occurrence_id=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .unwrap();
                applied(state == "embedded")
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
        steps.len() >= 7,
        "the fault phase needs seven steps after the checkpoint"
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

    let stores = corruption_episode(stores, witness, charges, step(3), steps[3].now_ms)?;
    let (stores, evidence) = artifact_ingest_episodes(stores, witness, step(3), steps[3].now_ms)?;
    let mut stores =
        artifact_deletion_episodes(stores, witness, &evidence, step(3), steps[3].now_ms)?;
    stores.drain(steps[3].now_ms);

    stores.apply(&steps[3]);
    stores.catch_up(steps[3].now_ms);
    held_publication_episode(&mut stores, witness, step(3), steps[3].now_ms)?;
    stores.apply(&steps[4]);
    stores.catch_up(steps[4].now_ms);
    let applied_id = publication_episode(
        &mut stores,
        witness,
        "publication-commit-reply-lost",
        step(4),
        steps[4].now_ms,
        PublicationFaultKind::LoseLocalCommitReply,
    )?;
    stores.apply(&steps[5]);
    stores.catch_up(steps[5].now_ms);
    let rolled_back_id = publication_episode(
        &mut stores,
        witness,
        "publication-commit-lost",
        step(5),
        steps[5].now_ms,
        PublicationFaultKind::LoseLocalCommit,
    )?;

    let quota_root = charges.occupy()?;
    quota_episode(quota_root.path(), witness, step(6), steps[6].now_ms)?;
    charges.vacate(quota_root)?;
    r11_episode(&mut stores, witness, &evidence, step(6), steps[6].now_ms)?;
    witness.checkpoint(Cut::AfterFaultPhase);

    let closed = stores.close();
    read_back(closed.root(), witness);
    let expected: BTreeMap<String, EffectState> = [
        (applied_id, EffectState::Applied),
        (rolled_back_id, EffectState::NotApplied),
    ]
    .into_iter()
    .collect();
    let mut stores = closed.reopen(steps[6].now_ms);
    witness.checkpoint(Cut::AfterRecovery);
    witness
        .coverage
        .record("flt_lost_reply_unknown_until_readback")
        .unwrap();
    witness.safety_check(&stores);
    live(&mut stores, &steps[6..]);
    witness.checkpoint(Cut::EndOfRun);
    drop(stores.close());
    charges.vacate(root)?;
    Ok(expected)
}

pub fn run(config: &Config, spawn: Spawn) -> Result<Run, RunError> {
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
    let bounds = profile.statistics.liveness_bounds.clone();
    let expected = campaign(&plan, &mut charges, &mut witness)?;
    for cut in KillCut::ALL {
        kill_episode(
            &plan,
            config.messages,
            &mut charges,
            &mut witness,
            spawn,
            cut,
        )?;
    }
    let liveness = liveness(&plan, &mut charges, &mut witness, &bounds)?;
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
    let mut report = FaultReport {
        schema: FAULT_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        profile_digest: profile.digest().unwrap(),
        claim_boundary: ClaimBoundary::pinned(),
        episodes: witness.episodes.clone(),
        barriers: witness.barriers.clone(),
        cuts: cut_receipts(&declared, &witness.checkpoints),
        coverage: witness.cuts.clone(),
        effects: witness.effects.clone(),
        expected_refusals: witness.refusals.clone(),
        safety_checks_while_armed: witness.safety_checks,
        liveness: Some(liveness),
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

// Process kill at a named cut, the held publication gate, and liveness mode.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use daemon::claim_sources::{ClaimMaterializer, MaterializationEnd};
use daemon::embedding_dispatch::{DispatchBounds, DispatchEvent, EmbeddingDispatcher};
use eval_core::{BarrierReceipt, HealthyCore, Lane, LaneProgress, LivenessReport};
use host_runtime::local_embeddings::LocalEmbeddingsLimits;
use kernel::CommitPageBounds;

use super::aging::memory_file;
use super::support::embedding_fixtures::{
    GateGuard, bounds as dispatch_bounds, budget as dispatch_budget, component, grant,
};

/// The drive publishes its rows `LocalOnly`, so the dispatcher's eligibility
/// names the local destination.
fn local_eligibility(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Local,
    }
}

pub const BARRIER: &str = "eval-fault-barrier";
pub const CHILD_ROOT: &str = "EIDNARA_EVAL_FAULT_CHILD_ROOT";
pub const CHILD_MESSAGES: &str = "EIDNARA_EVAL_FAULT_CHILD_MESSAGES";
pub const CHILD_APPLIED: &str = "EIDNARA_EVAL_FAULT_CHILD_APPLIED";
pub const CHILD_CUT: &str = "EIDNARA_EVAL_FAULT_CHILD_CUT";

/// The named cuts a child can park at inside one catch-up episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillCut {
    /// The batch is staged and its local transaction is still open.
    LocalStaged,
    /// The local prefix committed; the kernel writer is about to acknowledge it.
    AcknowledgementRequested,
}

impl KillCut {
    pub const ALL: [KillCut; 2] = [KillCut::LocalStaged, KillCut::AcknowledgementRequested];

    pub fn name(self) -> &'static str {
        match self {
            KillCut::LocalStaged => "local_staged",
            KillCut::AcknowledgementRequested => "acknowledgement_requested",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cut| cut.name() == name)
    }

    fn through(self, event: &EpisodeEvent) -> Option<i64> {
        match (self, event) {
            (KillCut::LocalStaged, EpisodeEvent::LocalStaged { through })
            | (
                KillCut::AcknowledgementRequested,
                EpisodeEvent::AcknowledgementRequested { through },
            ) => Some(*through),
            _ => None,
        }
    }

    fn effect(self, through: i64, episode: &str) -> String {
        match self {
            KillCut::LocalStaged => format!("search_commit:{through}@{episode}"),
            KillCut::AcknowledgementRequested => format!("search_ack:{through}@{episode}"),
        }
    }
}

/// What a child needs to reach its cut on a root the parent prepared.
#[derive(Debug, Clone)]
pub struct ChildArgs {
    pub root: PathBuf,
    pub messages: u32,
    pub applied: u32,
    pub cut: KillCut,
}

impl ChildArgs {
    pub fn from_env() -> Option<Self> {
        let root = std::env::var(CHILD_ROOT).ok()?;
        Some(Self {
            root: PathBuf::from(root),
            messages: std::env::var(CHILD_MESSAGES).ok()?.parse().ok()?,
            applied: std::env::var(CHILD_APPLIED).ok()?.parse().ok()?,
            cut: KillCut::parse(&std::env::var(CHILD_CUT).ok()?)?,
        })
    }

    pub fn env(&self, command: &mut Command) {
        command
            .env(CHILD_ROOT, &self.root)
            .env(CHILD_MESSAGES, self.messages.to_string())
            .env(CHILD_APPLIED, self.applied.to_string())
            .env(CHILD_CUT, self.cut.name());
    }
}

/// How the campaign starts its child: the test re-executes the test binary at
/// its entrypoint, the example re-executes itself with `fault-child`.
pub type Spawn = fn(&ChildArgs) -> Command;

/// The child: reopen the root, apply the next step, run one episode, print the
/// barrier at the cut, and park until killed. Never returns.
pub fn child_main(args: &ChildArgs) -> ! {
    let plan = aging::plan(args.messages).expect("the parent planned the same history");
    let planned = &plan.steps[args.applied as usize];
    let mut stores = Stores::reconstruct(
        &args.root,
        plan.rendering.clone(),
        args.applied,
        planned.now_ms,
    );
    stores.apply(planned);
    stores.publish_outbox_now();
    let cut = args.cut;
    let report = stores.episode(planned.now_ms, None, &mut |event| {
        if let Some(through) = cut.through(&event) {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{BARRIER} {through} {}", cut.name()).unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
    panic!("the child was not killed at its barrier: {report:?}");
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// A test-binary child is killed at a named cut inside a catch-up episode;
/// the parent reads the barrier before the kill, reads the lost effect back
/// from the closed files, reopens, and catches up to the tip.
pub fn kill_episode(
    plan: &Plan,
    messages: u32,
    charges: &mut Charges,
    witness: &mut Witness,
    spawn: Spawn,
    cut: KillCut,
) -> Result<(), RunError> {
    let id = format!("kill-at-{}", cut.name());
    witness.declare(episode(
        &id,
        plan.checkpoint_step,
        StoreFamily::SearchProjection,
        match cut {
            KillCut::LocalStaged => "local_commit",
            KillCut::AcknowledgementRequested => "acknowledge",
        },
        FaultAction::ProcessKill {
            cut: cut.name().to_string(),
        },
        "a SIGKILL of the test-binary child parked at the named cut: application-crash recovery with the page cache intact; not power loss, not a daemon crash, not a SQLite I/O fault",
    ));
    let k = plan.checkpoint_step as usize;
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps[..k]);
    drop(stores.close());
    let args = ChildArgs {
        root: root.path().to_path_buf(),
        messages,
        applied: k as u32,
        cut,
    };
    let mut command = spawn(&args);
    command.stdout(Stdio::piped());
    charges.observe(eval_core::Resource::Processes, 1)?;
    let mut child = ChildGuard(command.spawn().expect("the child spawns"));
    let pid = child.0.id();
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let barrier = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find(|line| line.contains(BARRIER));
        let _ = tx.send(barrier);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(120))
        .ok()
        .flatten()
        .ok_or_else(|| unexpected(&id, "a barrier line before the kill", "none"))?;
    let line = line[line.find(BARRIER).unwrap()..].to_string();
    let through: i64 = line
        .split(' ')
        .nth(1)
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| unexpected(&id, "a barrier naming the window", &line))?;
    let effect = cut.effect(through, &id);
    witness.effects.attempt(&effect);
    child.0.kill().unwrap();
    let status = child.0.wait().unwrap();
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().unwrap_or(0)
    };
    drop(child);
    witness.barriers.push(BarrierReceipt {
        episode: id.clone(),
        cut: cut.name().to_string(),
        pid,
        line,
        signal,
    });
    witness.effects.lose_reply(&effect).unwrap();
    witness.receipt(cut.name());
    read_back(root.path(), witness);
    let state = witness.effects.effects[&effect].outcome;
    if state != eval_core::EffectOutcome::NotApplied {
        return Err(unexpected(
            &id,
            "NotApplied: the killed step never committed its effect",
            state,
        ));
    }
    let now = plan.steps[k].now_ms;
    let mut stores = Stores::reconstruct(root.path(), plan.rendering.clone(), k as u32 + 1, now);
    witness.checkpoint(Cut::AfterRecovery);
    stores.drain(now);
    witness.safety_check(&stores);
    witness.receipt(&id);
    witness
        .coverage
        .record("flt_kill_barrier_read_before_kill")
        .unwrap();
    drop(stores.close());
    charges.vacate(root)?;
    Ok(())
}

/// A dispatcher pass with inference held behind the fixture gate admits the
/// job once and publishes nothing; passes while held re-admit nothing; the
/// release publishes it.
pub fn held_publication_episode(
    stores: &mut Stores,
    witness: &mut Witness,
    step: u32,
    now: i64,
) -> Result<(), RunError> {
    let id = witness.declare(episode(
        "publication-held-at-gate",
        step,
        StoreFamily::SearchProjection,
        "dispatch_pass",
        FaultAction::HeldPublication,
        "embedding_fixtures::TestEngine::block_calls: inference does not answer until released; the row stays admitted, later passes poll it by identity without re-admitting, and the release publishes it once",
    ));
    let pending_before = stores.pending(WorkCounter::EmbeddingOpen);
    if pending_before == 0 {
        return Err(unexpected(&id, "a pending embedding job", 0));
    }
    let engine = TestEngine::new();
    let gate = GateGuard(engine.block_calls());
    let local = component(&engine, LocalEmbeddingsLimits::default());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let project = ProjectScope::new(PROJECT).unwrap();
    let short = DispatchBounds {
        result_wait: Duration::from_millis(50),
        grant: grant(3, now + 86_400_000),
        ..dispatch_bounds()
    };
    let pass = |bounds: &DispatchBounds| -> Vec<DispatchEvent> {
        let mut events = Vec::new();
        let mut dispatcher =
            EmbeddingDispatcher::new(&stores.corpus.kernel, &stores.projection, &local);
        let end = runtime.block_on(async {
            dispatcher.run_pass(
                local_eligibility(&project),
                bounds,
                &dispatch_budget(Duration::from_secs(30)),
                now,
                &mut |event| events.push(event),
            )
        });
        assert!(end.unwrap().is_none(), "the pass is not blocked");
        events
    };
    let held = pass(&short);
    let admitted = held
        .iter()
        .filter(|e| matches!(e, DispatchEvent::Admitted { .. }))
        .count();
    let published = |events: &[DispatchEvent]| {
        events
            .iter()
            .filter(|e| matches!(e, DispatchEvent::Published { .. }))
            .count()
    };
    if admitted == 0 || published(&held) != 0 {
        return Err(unexpected(&id, "admitted, nothing published", &held));
    }
    let again = pass(&short);
    if again
        .iter()
        .any(|e| matches!(e, DispatchEvent::Admitted { .. }))
        || published(&again) != 0
    {
        return Err(unexpected(&id, "no re-admission while held", &again));
    }
    witness.receipt("publication_held");
    TestEngine::release(&gate.0);
    let released = pass(&dispatch_bounds());
    if published(&released) == 0 {
        return Err(unexpected(&id, "a publication after release", &released));
    }
    witness.receipt("publication_released");
    embed_pending_all(stores, now);
    witness.receipt(&id);
    witness.safety_check(stores);
    witness
        .coverage
        .record("sls_embedding_publication_held_then_released")
        .unwrap();
    Ok(())
}

fn embed_pending_all(stores: &Stores, now: i64) {
    aging::embed_pending(&stores.corpus, &stores.projection, stores.root(), now);
}

/// Liveness mode on its own root: the healthy core is the kernel, the
/// projection, the catch-up driver, the dispatcher, and the materializer;
/// a held memory-store write lock and a latched CAS stay armed for the
/// whole window; each lane is driven to its profile bound in its own unit.
pub fn liveness(
    plan: &Plan,
    charges: &mut Charges,
    witness: &mut Witness,
    bounds: &LivenessBounds,
) -> Result<LivenessReport, RunError> {
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    let k = plan.checkpoint_step as usize;
    live(&mut stores, &plan.steps[..k]);
    ClaimMaterializer::register(&stores.corpus.kernel, plan.steps[k].now_ms).unwrap();
    for planned in &plan.steps[k..] {
        stores.apply(planned);
    }
    let now = plan.steps.last().unwrap().now_ms;
    stores.publish_outbox_now();

    let memory_lock = witness.declare(episode(
        "liveness-memory-lock-holder",
        k as u32,
        StoreFamily::Memory,
        "write",
        FaultAction::ExternalLockHolder,
        "an external connection holds BEGIN IMMEDIATE on the memory store for the whole liveness window; the memory store is outside the healthy core and the lock is never released inside it",
    ));
    let holder = hold_write_lock(&memory_file(stores.root()));
    let cas_latch = witness.declare(episode(
        "liveness-cas-latched",
        k as u32,
        StoreFamily::Kernel,
        "ingest_artifact",
        FaultAction::ArtifactIngest {
            fault: ArtifactIngestFaultKind::Write,
        },
        "kernel::ArtifactIngestFault::Write: EIO latches CAS ingestion closed; the store is never reopened inside the window, so the latch stays armed",
    ));
    seed_fault_domain(&stores);
    let latched = stores
        .corpus
        .kernel
        .ingest_artifact_with_fault_for_test(
            ingest_request("liveness", b"liveness"),
            ArtifactIngestFault::Write,
        )
        .err()
        .ok_or_else(|| unexpected(&cas_latch, "a latched ingest", "Ok"))?;
    assert_eq!(
        latched.kind(),
        kernel::ArtifactErrorKind::IngestionFailClosed
    );
    let outside_core: BTreeSet<String> = [memory_lock.clone(), cas_latch.clone()]
        .into_iter()
        .collect();
    let armed = |stores: &Stores| -> BTreeSet<String> {
        let mut armed = BTreeSet::new();
        let probe = Connection::open_with_flags(
            memory_file(stores.root()),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
        if probe.execute_batch("BEGIN IMMEDIATE").is_err() {
            armed.insert(memory_lock.clone());
        }
        if stores
            .corpus
            .kernel
            .ingest_artifact(ingest_request("liveness-probe", b"probe"))
            .is_err()
        {
            armed.insert(cas_latch.clone());
        }
        armed
    };

    let tip = stores.tip();
    let mut lanes = BTreeMap::new();
    // Catch-up: acknowledged_through reaches the tip and holds there.
    let bound = bounds.catch_up_episodes;
    let mut met_at = None;
    let mut blocked = None;
    for step in 1..=bound {
        let report = stores.episode(now, None, &mut |_| {});
        let holds = report.end == EpisodeEnd::ReachedTarget && report.acknowledged_through >= tip;
        if let EpisodeEnd::Blocked(b) = &report.end {
            blocked = Some(format!("{b:?}"));
        }
        if holds && met_at.is_none() {
            met_at = Some(step);
        }
        if step == bound && !holds {
            met_at = None;
        }
        witness.safety_check(&stores);
    }
    let holds_at_bound = met_at.is_some();
    lanes.insert(
        Lane::CatchUpEpisodes,
        LaneProgress {
            bound,
            steps: bound,
            met_at,
            holds_at_bound,
            blocked,
        },
    );

    // Embedding: every eligible job reaches embedded through dispatcher passes.
    let engine = TestEngine::new();
    let local = component(&engine, LocalEmbeddingsLimits::default());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let project = ProjectScope::new(PROJECT).unwrap();
    let bound = bounds.embedding_passes;
    let mut met_at = None;
    let mut blocked = None;
    for step in 1..=bound {
        let mut dispatcher =
            EmbeddingDispatcher::new(&stores.corpus.kernel, &stores.projection, &local);
        let end = runtime.block_on(async {
            dispatcher.run_pass(
                local_eligibility(&project),
                &DispatchBounds {
                    grant: grant(3, now + 86_400_000),
                    ..dispatch_bounds()
                },
                &dispatch_budget(Duration::from_secs(30)),
                now,
                &mut |_| {},
            )
        });
        if let Ok(Some(b)) = &end {
            blocked = Some(format!("{b:?}"));
        }
        let holds = end.is_ok() && stores.pending(WorkCounter::EmbeddingOpen) == 0;
        if holds && met_at.is_none() {
            met_at = Some(step);
        }
        if step == bound && !holds {
            met_at = None;
        }
    }
    lanes.insert(
        Lane::EmbeddingPasses,
        LaneProgress {
            bound,
            steps: bound,
            met_at,
            holds_at_bound: met_at.is_some(),
            blocked,
        },
    );

    // Materialization: the materializer acknowledges the tip and holds there.
    let bound = bounds.materialization_episodes;
    let mut met_at = None;
    let mut blocked = None;
    let mut materializer = ClaimMaterializer::new(&stores.corpus.kernel, ProviderEgress::LocalOnly);
    let page = CommitPageBounds {
        max_commits: 8.try_into().unwrap(),
        max_rows: 64.try_into().unwrap(),
        max_payload_bytes: (1u64 << 20).try_into().unwrap(),
    };
    for step in 1..=bound {
        let report = materializer.run_episode(page, now).unwrap();
        let holds = matches!(report.end, MaterializationEnd::ReachedTarget)
            && report.acknowledged_through >= tip;
        if let MaterializationEnd::Blocked(b) = &report.end {
            blocked = Some(format!("{b:?}"));
        }
        if holds && met_at.is_none() {
            met_at = Some(step);
        }
        if step == bound && !holds {
            met_at = None;
        }
    }
    lanes.insert(
        Lane::MaterializationEpisodes,
        LaneProgress {
            bound,
            steps: bound,
            met_at,
            holds_at_bound: met_at.is_some(),
            blocked,
        },
    );

    let armed_at_bound = armed(&stores);
    drop(holder);
    witness.receipt(&memory_lock);
    witness.receipt(&cas_latch);
    let report = LivenessReport {
        core: HealthyCore {
            families: [StoreFamily::Kernel, StoreFamily::SearchProjection]
                .into_iter()
                .collect(),
            lanes: [
                Lane::CatchUpEpisodes,
                Lane::EmbeddingPasses,
                Lane::MaterializationEpisodes,
            ]
            .into_iter()
            .collect(),
        },
        outside_core,
        armed_at_bound,
        lanes,
        permanent_stalls: witness
            .refusals
            .iter()
            .filter(|r| r.refusal == ExpectedRefusal::R11DeletionBearingCatchUp)
            .cloned()
            .collect(),
    };
    report.verdict(bounds).map_err(FaultReportError::Liveness)?;
    witness
        .coverage
        .record("flt_liveness_bounds_met_with_faults_armed")
        .unwrap();
    drop(stores.close());
    charges.vacate(root)?;
    Ok(report)
}
