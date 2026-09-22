//! Suite C replays one generated history end to end and from a quiescent
//! checkpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::embedding_publication::{EmbeddingPublisher, VectorPublication};
use daemon::harness_sources::{Harness, SessionIdentity, SourcePublisher, opencode_units};
use daemon::search_catchup::{CatchUpConsumer, EpisodeBounds, EpisodeEnd, SearchCatchUp};
use daemon::search_projection::SearchProjection;
use eval_core::{
    AGING_REPORT_SCHEMA, AgingReport, AgingReportError, Approval, Attestation, Checkpoint,
    CheckpointRefused, ClaimBoundary, ComponentVersions, Construction, ConstructionKind, Coverage,
    Cut, CutOutcome, CutReceipt, Death, Descriptor, EVENT_SCHEMA_VERSION, EnvelopeExceeded,
    EventId, EventLog, ExecutionMode, FAILURE_CLASS_TABLE_DIGEST, GENERATOR_VERSION,
    GuardComparison, HistoricalRows, Ingestion, LiveRows, MANIFEST_SCHEMA, Manifest,
    MemoryReviewerModelCalls, Mode, PAIRING_POLICY_VERSION, Payload, PrefixRefused, ProfileError,
    ProjectionRows, QuiescenceReceipt, REDUCER_VERSION, Reachability, RenderConfig, Rendering,
    Reopened, RestoreRefused, RunIdentity, RunProfile, RunStatus, Scale, Segment, SessionSpec,
    StateSnapshot, StoreFamily, StoreIntegrity, StoreQuiescence, TokenizerProfile, Unenumerated,
    WalCheckpoint, WindowDeaths, WorkCounter, WorldConfig, WorldError, eval_run_id, generate_all,
    render,
};
use kernel::{
    ArtifactDestination, CommitPageBounds, CurrentInputDescriptor, EligibilityBinding, KernelError,
    KernelStore, ProjectScope, ProviderEgress, Sensitivity, SourceHoldAdmission, SourceHoldBounds,
    SourceRow,
};
use lease::{HeldFileLease, LeaseError};
use memory_store::{MemoryStore, MemoryStoreError, StoredHistorySegment};
use retrieval::PersistBounds;
use retrieval::batch::BatchBounds;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use storage::StoreError;

use super::campaign::{
    Charges, identity, parse_flags, prepare_publish, profile as suite_b, publish_file, sha256_hex,
};
use super::support::embedding_fixtures::{
    Corpus, GENERATION, PROJECT, SCOPE, TestEngine, batch_bounds, generation, hold_bounds, intent,
    kernel_incarnation_id, source_page_bounds,
};

pub const SEED: u64 = 0x5EED_C000_0000_0004;
const SESSION: &str = "session-0";
const EPOCH_MS: i64 = 1_700_000_000_000;
const MAX_EPISODES_PER_DRAIN: u32 = 64;
const CHECKPOINT_WAIT: Duration = Duration::from_secs(3);
const KERNEL_ARTIFACTS: &str = "kernel/artifacts/objects";
pub const REPORT_FILE: &str = "suite-c-aging-report.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIMULATOR_VERSION: &str = "eval-aging-shell/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scale: Scale,
    pub messages: u32,
    pub elapsed_bound_ms: u64,
    pub approval: Option<Approval>,
    pub publish: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("profile refused: {0}")]
    Profile(#[from] ProfileError),
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("checkpoint refused: {0}")]
    Checkpoint(#[from] CheckpointRefused),
    #[error("restore refused: {0}")]
    Restore(#[from] RestoreRefused),
    #[error("prefix refused: {0}")]
    Prefix(#[from] PrefixRefused),
    #[error("unenumerated divergence: {0}")]
    Guard(#[from] Unenumerated),
    #[error("report refused: {0}")]
    Report(#[from] AgingReportError),
    #[error("history refused: {0:?}")]
    World(#[from] WorldError),
    #[error("no step straddles a supersession and a retirement")]
    NoStraddlingStep,
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
    messages: u32,
    elapsed_ms: u64,
    approval: Option<Approval>,
) -> RunProfile {
    let mut profile = suite_b(scale, event_bound(messages), elapsed_ms, approval);
    profile.name = profile.name.replace("surface1-raw", "suite-c-aging");
    profile.tasks_per_world = 1;
    profile.envelope.temp_roots = 4;
    profile
}

/// The one event bound the generator enforces on the log and the profile
/// declares, so the two cannot drift; saturating, so an absurd message count
/// reaches the profile's refusal instead of overflowing here.
fn event_bound(messages: u32) -> u32 {
    messages.max(64).saturating_mul(2)
}

fn world(messages: u32) -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages,
            tool_span_every: 4,
            correction_every: 3,
            invalidation_every: 5,
        }],
        repositories: Vec::new(),
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: event_bound(messages),
        planted: Vec::new(),
    }
}

fn session() -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::OpenCode,
        session_id: SESSION.to_string(),
    }
}

#[derive(Debug, Clone)]
pub enum Step {
    Publish(EventId),
    Retire(EventId),
}

#[derive(Debug, Clone)]
pub struct Planned {
    pub step: Step,
    pub now_ms: i64,
}

fn planned(log: &EventLog) -> Vec<Planned> {
    log.events
        .iter()
        .filter_map(|event| {
            let step = match &event.payload {
                Payload::Message { .. } | Payload::Correction { .. } => {
                    Step::Publish(event.id.clone())
                }
                Payload::Invalidation { target } => Step::Retire(target.clone()),
                Payload::ToolSpan { .. } | Payload::Commit { .. } | Payload::Rename { .. } => {
                    return None;
                }
            };
            Some(Planned {
                step,
                now_ms: event.valid_time_ms,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Died {
    Supersession,
    Retirement,
}

fn deaths(log: &EventLog, steps: &[Planned]) -> Vec<Option<(usize, Died)>> {
    let lineage_of = |id: &EventId| {
        let event = log.events.iter().find(|event| event.id == *id).unwrap();
        match &event.payload {
            Payload::Correction { target, .. } => target.clone(),
            _ => event.id.clone(),
        }
    };
    let mut born: BTreeMap<EventId, usize> = BTreeMap::new();
    steps
        .iter()
        .enumerate()
        .map(|(index, planned)| {
            let (id, death) = match &planned.step {
                Step::Publish(id) => (id, Died::Supersession),
                Step::Retire(id) => (id, Died::Retirement),
            };
            let lineage = lineage_of(id);
            match born.get(&lineage) {
                Some(&at) => Some((at, death)),
                None => {
                    born.insert(lineage, index);
                    None
                }
            }
        })
        .collect()
}

fn straddling_step(log: &EventLog, steps: &[Planned]) -> Option<u32> {
    let deaths = deaths(log, steps);
    let middle = steps.len() / 2;
    let mut candidates: Vec<usize> = (1..steps.len())
        .filter(|&k| {
            let after = |kind| {
                deaths[k..]
                    .iter()
                    .flatten()
                    .any(|(born, death)| *born < k && *death == kind)
            };
            after(Died::Supersession)
                && after(Died::Retirement)
                && deaths[..k].iter().any(Option::is_some)
        })
        .collect();
    candidates.sort_by_key(|k| k.abs_diff(middle));
    candidates.first().map(|k| *k as u32)
}

/// The hold admission limit counts total references, so it must cover every
/// published unit.
#[derive(Debug, Clone, Copy)]
pub struct DriveBounds {
    pub hold: SourceHoldBounds,
    pub batch: BatchBounds,
}

impl DriveBounds {
    fn admitting(rendering: &Rendering, steps: &[Planned]) -> Self {
        let messages: BTreeMap<&EventId, &Value> = rendering
            .messages
            .iter()
            .map(|m| (&m.event_id, &m.message))
            .collect();
        let (mut units, mut bytes) = (0usize, 0usize);
        for planned in steps {
            let Step::Publish(id) = &planned.step else {
                continue;
            };
            for unit in opencode_units(&session(), messages[id]).unwrap() {
                units += 1;
                bytes += unit.text.len();
            }
        }
        let raise = |floor: NonZeroUsize, demand: usize| {
            floor.max(NonZeroUsize::new(demand).unwrap_or(floor))
        };
        let hold = hold_bounds();
        let batch = batch_bounds();
        let encoded = hold.admission.max_encoded_bytes;
        Self {
            hold: SourceHoldBounds {
                max_descriptor_rows: raise(hold.max_descriptor_rows, units),
                admission: SourceHoldAdmission {
                    max_references: raise(hold.admission.max_references, units),
                    max_encoded_bytes: encoded
                        .max(NonZeroU64::new(bytes as u64).unwrap_or(encoded)),
                },
                ..hold
            },
            batch: BatchBounds {
                persist: PersistBounds {
                    max_records: raise(batch.persist.max_records, units),
                    ..batch.persist
                },
                max_source_bytes: raise(batch.max_source_bytes, bytes),
                max_local_mutations: raise(batch.max_local_mutations, units),
                max_pending: raise(batch.max_pending, units),
            },
        }
    }
}

fn episode_bounds(bounds: &DriveBounds) -> EpisodeBounds {
    EpisodeBounds {
        commits: CommitPageBounds {
            max_commits: 64.try_into().unwrap(),
            max_rows: 64.try_into().unwrap(),
            max_payload_bytes: (1u64 << 20).try_into().unwrap(),
        },
        hold_admission: bounds.hold.admission,
        source_page: source_page_bounds(),
        max_source_pages: 8.try_into().unwrap(),
        max_source_encoded_bytes: (1u64 << 20).try_into().unwrap(),
        batch: bounds.batch,
    }
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn kernel_file(root: &Path) -> PathBuf {
    root.join("kernel").join("kernel.sqlite")
}

fn search_file(root: &Path) -> PathBuf {
    root.join("search").join("search.sqlite")
}

fn memory_file(root: &Path) -> PathBuf {
    root.join(daemon::STORE_FILE_NAME)
}

fn store_file(root: &Path, family: StoreFamily) -> PathBuf {
    match family {
        StoreFamily::Kernel => kernel_file(root),
        StoreFamily::Memory => memory_file(root),
        StoreFamily::SearchProjection => search_file(root),
    }
}

pub struct Stores {
    root: PathBuf,
    corpus: Corpus,
    projection: SearchProjection,
    consumer: CatchUpConsumer,
    memory: MemoryStore,
    rendering: Rendering,
    bounds: DriveBounds,
    chains: BTreeMap<String, Vec<String>>,
    dead: BTreeSet<String>,
    applied: u32,
}

impl Stores {
    pub fn open(root: &Path, plan: &Plan) -> Self {
        let corpus = Corpus::open(root);
        corpus.seed();
        let memory = MemoryStore::open(&daemon::store_descriptor_in(root)).unwrap();
        let (projection, consumer) = Self::bootstrap(&corpus, root, &plan.bounds, EPOCH_MS);
        Self {
            root: root.to_path_buf(),
            corpus,
            projection,
            consumer,
            memory,
            rendering: plan.rendering.clone(),
            bounds: plan.bounds,
            chains: BTreeMap::new(),
            dead: BTreeSet::new(),
            applied: 0,
        }
    }

    fn bootstrap(
        corpus: &Corpus,
        root: &Path,
        bounds: &DriveBounds,
        now: i64,
    ) -> (SearchProjection, CatchUpConsumer) {
        let (projection, hold, _) = corpus.bootstrap_within(root, bounds.hold, bounds.batch);
        let binding = corpus.binding();
        corpus
            .kernel
            .acknowledge_through_source_hold(&binding, &hold.hold_id, hold.snapshot, now)
            .unwrap();
        let consumer = CatchUpConsumer {
            binding,
            hold_id: hold.hold_id,
            kernel_incarnation_id: kernel_incarnation_id(root),
            generation_id: Some(GENERATION.to_string()),
        };
        (projection, consumer)
    }

    pub fn projection_path(&self) -> PathBuf {
        search_file(&self.root)
    }

    pub fn tip(&self) -> i64 {
        self.corpus.tip()
    }

    pub fn incarnation(&self) -> String {
        kernel_incarnation_id(&self.root)
    }

    pub fn apply(&mut self, planned: &Planned) {
        match &planned.step {
            Step::Publish(id) => self.publish(id),
            Step::Retire(id) => self.retire(id),
        }
        self.applied += 1;
    }

    fn publish(&mut self, id: &EventId) {
        let ordinal = self
            .rendering
            .messages
            .iter()
            .position(|m| m.event_id == *id)
            .expect("every published step is a rendered message");
        let message = &self.rendering.messages[ordinal];
        let publisher = SourcePublisher {
            kernel: &self.corpus.kernel,
            domain_id: "domain",
            scope_id: Some(SCOPE),
            egress: ProviderEgress::LocalOnly,
            sensitivity: Sensitivity::Normal,
        };
        let units = opencode_units(&session(), &message.message).unwrap();
        assert_eq!(units.len(), message.expected.len(), "{}", message.message);
        for (unit, expected) in units.iter().zip(&message.expected) {
            let published = publisher
                .publish(unit, message.observation_time_ms)
                .unwrap();
            assert_eq!(published.occurrence_id, expected.identity.occurrence_id);
            let chain = self
                .chains
                .entry(expected.identity.lineage_id.clone())
                .or_default();
            if let Some(replaced) = &published.replaced_object_id {
                assert_eq!(chain.last(), Some(replaced));
            }
            chain.push(published.object_id);
        }
        let ordinal = ordinal as i64 + 1;
        let text = message.message["parts"][0]["text"].as_str().unwrap();
        let block = format!("{}#0", message.message["info"]["id"].as_str().unwrap());
        self.memory
            .append_history_segments(
                SESSION,
                &[StoredHistorySegment {
                    sequence: ordinal,
                    start_message: ordinal,
                    end_message: ordinal,
                    start_message_id: block.clone(),
                    end_message_id: block,
                    title: format!("C{ordinal}"),
                    content: text.to_string(),
                    p1: Some(text.to_string()),
                    importance: 50,
                    created_at: message.observation_time_ms,
                    ..Default::default()
                }],
            )
            .unwrap();
    }

    fn retire(&mut self, target: &EventId) {
        let message = self
            .rendering
            .messages
            .iter()
            .find(|m| m.event_id == *target)
            .expect("an invalidation targets a rendered message");
        let lineage = &message.expected[0].identity.lineage_id;
        let Some(object) = self.chains.get(lineage).and_then(|chain| chain.last()) else {
            return;
        };
        if !self.dead.insert(object.clone()) {
            return;
        }
        let object = object.clone();
        self.corpus
            .kernel
            .commit(intent(&format!("retire-{object}")), |envelope| {
                envelope.retire_observation(&object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    fn publish_outbox(&self) {
        let pending = self.corpus.kernel.pending_outbox(1024).unwrap();
        if let Some(last) = pending.iter().rev().find(|entry| entry.commit_boundary) {
            self.corpus
                .kernel
                .mark_outbox_published_through(last.outbox_position, 1)
                .unwrap();
        }
    }

    pub fn drain(&mut self, now: i64) {
        self.publish_outbox();
        let tip = self.tip();
        let bounds = episode_bounds(&self.bounds);
        let mut catch_up = SearchCatchUp::new(&self.corpus.kernel, &self.projection);
        for _ in 0..MAX_EPISODES_PER_DRAIN {
            let report = catch_up
                .run_episode(&self.consumer, &bounds, now, &mut |_| {})
                .unwrap();
            assert_eq!(report.end, EpisodeEnd::ReachedTarget, "{report:?}");
            if report.acknowledged_through >= tip {
                break;
            }
        }
        assert_eq!(
            self.pending(WorkCounter::CatchUpLag),
            0,
            "the drain reaches the tip"
        );
        embed_pending(
            &self.corpus,
            &self.projection,
            &self.root,
            self.bounds.hold,
            now,
        );
        assert_eq!(
            self.pending(WorkCounter::EmbeddingOpen),
            0,
            "the drain embeds every open job"
        );
    }

    fn pending(&self, counter: WorkCounter) -> u64 {
        pending(&self.root, &self.corpus.kernel, counter)
    }

    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            commit_seq: self.tip(),
            kernel: descriptors(&kernel_file(&self.root)),
            projection_live: projection_rows(&search_file(&self.root)).live,
            memory: segments(&self.memory),
        }
    }

    pub fn projection_rows(&self) -> ProjectionRows {
        projection_rows(&search_file(&self.root))
    }

    pub fn close(self) -> Closed {
        let pending = pending_counters(&self.root, &self.corpus.kernel);
        let Stores {
            root,
            corpus,
            projection,
            memory,
            rendering,
            bounds,
            chains,
            dead,
            applied,
            ..
        } = self;
        let mut wal = BTreeMap::new();
        let mut handles_closed = BTreeMap::new();
        wal.insert(
            StoreFamily::SearchProjection,
            projection
                .checkpoint_truncate(Instant::now() + CHECKPOINT_WAIT)
                .map(triple)
                .unwrap_or(NOT_TRUNCATED),
        );
        let (_, search_lease) = projection.close();
        handles_closed.insert(StoreFamily::SearchProjection, true);
        drop(memory);
        handles_closed.insert(StoreFamily::Memory, true);
        wal.insert(StoreFamily::Memory, truncate(&memory_file(&root)));
        let kernel_closed = match Arc::try_unwrap(corpus.kernel) {
            Ok(kernel) => {
                drop(kernel);
                true
            }
            Err(_) => false,
        };
        handles_closed.insert(StoreFamily::Kernel, kernel_closed);
        wal.insert(
            StoreFamily::Kernel,
            if kernel_closed {
                truncate(&kernel_file(&root))
            } else {
                NOT_TRUNCATED
            },
        );
        let receipt = QuiescenceReceipt {
            step: applied,
            stores: StoreFamily::ALL
                .into_iter()
                .map(|family| {
                    (
                        family,
                        StoreQuiescence {
                            pending: pending[&family].clone(),
                            wal: wal[&family],
                            wal_sidecar_bytes: sidecar_len(&store_file(&root, family)),
                            handles_closed: handles_closed[&family],
                        },
                    )
                })
                .collect(),
        };
        Closed {
            root,
            receipt,
            search_lease,
            rendering,
            bounds,
            chains,
            dead,
            applied,
        }
    }
}

fn pending(root: &Path, kernel: &KernelStore, counter: WorkCounter) -> u64 {
    match counter {
        WorkCounter::OutboxUnpublished => kernel.pending_outbox(1024).unwrap().len() as u64,
        WorkCounter::CatchUpLag => {
            let acknowledged: i64 = read_only(&search_file(root))
                .query_row(
                    "SELECT checkpoint_commit_seq FROM projection_checkpoint",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            u64::try_from(kernel.tip().unwrap() - acknowledged).unwrap()
        }
        WorkCounter::EmbeddingOpen => count(
            &search_file(root),
            "SELECT COUNT(*) FROM embedding_jobs WHERE state IN ('pending','admitted')",
        ),
        WorkCounter::CaptureJobsPending => count(
            &memory_file(root),
            "SELECT COUNT(*) FROM memory_capture_jobs WHERE commit_seq IS NULL AND abandoned_at_ms IS NULL",
        ),
        WorkCounter::ReviewerJobsOpen => count(
            &memory_file(root),
            "SELECT COUNT(*) FROM memory_reviewer_jobs WHERE state<>'terminal'",
        ),
    }
}

fn pending_counters(
    root: &Path,
    kernel: &KernelStore,
) -> BTreeMap<StoreFamily, BTreeMap<WorkCounter, u64>> {
    StoreFamily::ALL
        .into_iter()
        .map(|family| {
            let counters = family
                .counters()
                .iter()
                .map(|counter| (*counter, pending(root, kernel, *counter)))
                .collect();
            (family, counters)
        })
        .collect()
}

const NOT_TRUNCATED: WalCheckpoint = WalCheckpoint {
    busy: 1,
    wal_frames: -1,
    checkpointed_frames: -1,
};

fn triple((busy, wal_frames, checkpointed_frames): (i64, i64, i64)) -> WalCheckpoint {
    WalCheckpoint {
        busy,
        wal_frames,
        checkpointed_frames,
    }
}

fn truncate(file: &Path) -> WalCheckpoint {
    let conn = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    triple(
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap(),
    )
}

pub fn sidecar_len(file: &Path) -> u64 {
    assert!(file.is_file(), "{} exists", file.display());
    let mut wal = file.as_os_str().to_owned();
    wal.push("-wal");
    match std::fs::metadata(&wal) {
        Ok(metadata) => metadata.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => panic!("{}: {e}", PathBuf::from(wal).display()),
    }
}

fn count(file: &Path, sql: &str) -> u64 {
    read_only(file)
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| u64::try_from(n).unwrap())
        .unwrap()
}

pub struct Closed {
    root: PathBuf,
    pub receipt: QuiescenceReceipt,
    search_lease: Option<HeldFileLease>,
    rendering: Rendering,
    bounds: DriveBounds,
    chains: BTreeMap<String, Vec<String>>,
    dead: BTreeSet<String>,
    applied: u32,
}

impl Closed {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn copy(mut self, into: &Path) -> Result<(Checkpoint, Copied), CheckpointRefused> {
        // Each probe stays held until the copy is done, as the projection's
        // lease does, so no other holder can take the store while its bytes
        // are read.
        let memory_probe = match MemoryStore::open(&daemon::store_descriptor_in(&self.root)) {
            Ok(probe) => Some(probe),
            Err(MemoryStoreError::Store(StoreError::Lease(LeaseError::Held { .. }))) => None,
            Err(e) => panic!("memory store probe at {}: {e}", self.root.display()),
        };
        self.seal_after_probe(StoreFamily::Memory, memory_probe.is_none());
        let kernel_probe = match KernelStore::open(kernel_file(&self.root).parent().unwrap()) {
            Ok(probe) => Some(probe),
            Err(KernelError::Held) => None,
            Err(e) => panic!("kernel probe at {}: {e}", self.root.display()),
        };
        self.seal_after_probe(StoreFamily::Kernel, kernel_probe.is_none());
        // Work left by a holder that took a lease between the close and its
        // probe is counted here, while both probes are held.
        if let Some(kernel) = &kernel_probe {
            for (family, counters) in pending_counters(&self.root, kernel) {
                self.receipt.stores.get_mut(&family).unwrap().pending = counters;
            }
        }
        let incarnation_id = kernel_incarnation_id(&self.root);
        Checkpoint::admit(&self.receipt, &incarnation_id)?;
        let mut files = BTreeMap::new();
        for relative in copied_paths(&self.root) {
            copy_file(&self.root, into, &relative, &mut files);
        }
        drop(self.search_lease);
        drop(memory_probe);
        drop(kernel_probe);
        let checkpoint = Checkpoint::new(self.receipt, incarnation_id, files)?;
        Ok((
            checkpoint,
            Copied {
                root: into.to_path_buf(),
                rendering: self.rendering,
                bounds: self.bounds,
                chains: self.chains,
                dead: self.dead,
                applied: self.applied,
            },
        ))
    }

    /// Close-time evidence cannot see a handle opened after the close. A probe
    /// refused by another holder's lease marks the family's handle open.
    fn seal_after_probe(&mut self, family: StoreFamily, held: bool) {
        let file = store_file(&self.root, family);
        let store = self.receipt.stores.get_mut(&family).unwrap();
        if held {
            store.handles_closed = false;
        } else {
            store.wal = truncate(&file);
            store.wal_sidecar_bytes = sidecar_len(&file);
        }
    }
}

fn copied_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = StoreFamily::ALL
        .into_iter()
        .map(|family| {
            store_file(root, family)
                .strip_prefix(root)
                .unwrap()
                .to_path_buf()
        })
        .collect();
    let objects = root.join(KERNEL_ARTIFACTS);
    if objects.is_dir() {
        for entry in walk(&objects) {
            paths.push(entry.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    paths
}

pub struct Copied {
    root: PathBuf,
    rendering: Rendering,
    bounds: DriveBounds,
    chains: BTreeMap<String, Vec<String>>,
    dead: BTreeSet<String>,
    applied: u32,
}

impl Copied {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn reopen(self, checkpoint: &Checkpoint, now: i64) -> Result<Stores, RestoreRefused> {
        let integrity = |file: PathBuf| {
            let conn = read_only(&file);
            let integrity_check: String = conn
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .unwrap();
            let violations: i64 = conn
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .unwrap();
            StoreIntegrity {
                integrity_check,
                foreign_key_violations: u64::try_from(violations).unwrap(),
            }
        };
        let files: BTreeMap<String, String> = checkpoint
            .files
            .keys()
            .filter_map(|relative| {
                let bytes = std::fs::read(self.root.join(relative)).ok()?;
                Some((relative.clone(), sha256_hex(&bytes)))
            })
            .collect();
        // A read-only open of an absent store file fails and a malformed one
        // panics in its first query, so absent and changed files are refused
        // before the incarnation and integrity reads open any store.
        for (path, digest) in &checkpoint.files {
            match files.get(path) {
                None => return Err(RestoreRefused::FileMissing { path: path.clone() }),
                Some(found) if found != digest => {
                    return Err(RestoreRefused::FileDiffers { path: path.clone() });
                }
                Some(_) => {}
            }
        }
        checkpoint.accept(&Reopened {
            incarnation_id: kernel_incarnation_id(&self.root),
            stores: StoreFamily::ALL
                .into_iter()
                .map(|family| (family, integrity(store_file(&self.root, family))))
                .collect(),
            files,
        })?;
        let corpus = Corpus::open(&self.root);
        let memory = MemoryStore::open(&daemon::store_descriptor_in(&self.root)).unwrap();
        let copied = SearchProjection::open(&self.root).unwrap();
        copied.verify_connection().unwrap();
        let (path, lease) = copied.close();
        drop(lease);
        std::fs::remove_file(&path).unwrap();
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            match std::fs::remove_file(&sidecar) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{}: {e}", PathBuf::from(sidecar).display()),
            }
        }
        let (projection, consumer) = Stores::bootstrap(&corpus, &self.root, &self.bounds, now);
        embed_pending(&corpus, &projection, &self.root, self.bounds.hold, now);
        Ok(Stores {
            root: self.root,
            corpus,
            projection,
            consumer,
            memory,
            rendering: self.rendering,
            bounds: self.bounds,
            chains: self.chains,
            dead: self.dead,
            applied: self.applied,
        })
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else if path.is_file() {
            files.push(path);
        }
    }
    files
}

fn copy_file(from: &Path, into: &Path, relative: &Path, files: &mut BTreeMap<String, String>) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let source = from.join(relative);
    let target = into.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .unwrap();
    }
    let bytes = std::fs::read(&source).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&target)
        .and_then(|mut file| std::io::Write::write_all(&mut file, &bytes))
        .unwrap();
    files.insert(relative.to_string_lossy().into_owned(), sha256_hex(&bytes));
}

fn descriptors(kernel: &Path) -> BTreeMap<String, Descriptor> {
    let conn = read_only(kernel);
    let mut statement = conn
        .prepare(
            "SELECT object_id,source_revision,created_commit_seq,invalidated_commit_seq,superseded_by \
             FROM object_registry WHERE object_id GLOB 'srcdesc:*'",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Descriptor {
                    source_revision: row.get(1)?,
                    created_commit_seq: row.get(2)?,
                    invalidated_commit_seq: row.get(3)?,
                    superseded_by: row.get(4)?,
                },
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn tombstones(conn: &Connection) -> BTreeMap<String, Death> {
    conn.prepare("SELECT occurrence_id,invalidated_commit_seq,reason FROM occurrence_tombstones")
        .unwrap()
        .query_map([], |row| {
            let reason: String = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                Death {
                    invalidated_commit_seq: row.get(1)?,
                    reason: serde_json::from_value(Value::String(reason))
                        .expect("the projection's tombstone reasons are the closed set"),
                },
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn occurrences(
    conn: &Connection,
    tombstones: &BTreeMap<String, Death>,
) -> (BTreeMap<String, String>, BTreeMap<String, i64>) {
    let mut live = BTreeMap::new();
    let mut created = BTreeMap::new();
    let mut statement = conn
        .prepare(
            "SELECT o.occurrence_id,o.created_commit_seq,p.bytes FROM occurrences o \
             JOIN payloads p USING(payload_id)",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (occurrence_id, created_commit_seq, bytes) = row.unwrap();
        if !tombstones.contains_key(&occurrence_id) {
            live.insert(occurrence_id.clone(), sha256_hex(&bytes));
        }
        created.insert(occurrence_id, created_commit_seq);
    }
    (live, created)
}

fn projection_rows(search: &Path) -> ProjectionRows {
    let conn = read_only(search);
    let strings = |sql: &str| -> BTreeSet<String> {
        conn.prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let tombstones = tombstones(&conn);
    let (live, created) = occurrences(&conn, &tombstones);
    let generation_state: String = conn
        .query_row(
            "SELECT state FROM vector_generations WHERE generation_id=?1",
            [GENERATION],
            |row| row.get(0),
        )
        .unwrap();
    let snapshot_commit_seq: i64 = conn
        .query_row(
            "SELECT snapshot_commit_seq FROM projection_checkpoint",
            [],
            |row| row.get(0),
        )
        .unwrap();
    ProjectionRows {
        snapshot_commit_seq,
        live: LiveRows {
            occurrences: live,
            lexical: strings("SELECT occurrence_id FROM lexical"),
            pending_embedding: strings(
                "SELECT occurrence_id FROM embedding_jobs WHERE state IN ('pending','admitted')",
            ),
        },
        historical: HistoricalRows {
            occurrences: created,
            tombstones,
            generation_state: serde_json::from_value(Value::String(generation_state))
                .expect("the projection's generation states are the closed set"),
        },
    }
}

fn segments(memory: &MemoryStore) -> BTreeMap<i64, Segment> {
    memory
        .load_history_segments(SESSION)
        .unwrap()
        .into_iter()
        .map(|segment| {
            (
                segment.sequence,
                Segment {
                    start_message: segment.start_message,
                    end_message: segment.end_message,
                    content: segment.content,
                },
            )
        })
        .collect()
}

fn embed_pending(
    corpus: &Corpus,
    projection: &SearchProjection,
    root: &Path,
    hold: SourceHoldBounds,
    now: i64,
) {
    let open: Vec<String> = read_only(&search_file(root))
        .prepare(
            "SELECT occurrence_id FROM embedding_jobs WHERE state IN ('pending','admitted') \
             ORDER BY occurrence_id",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    if open.is_empty() {
        return;
    }
    let rows: Vec<SourceRow> = corpus.export_within(hold);
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, projection);
    for occurrence in open {
        let row = rows
            .iter()
            .find(|row| row.detail.occurrence_id == occurrence)
            .expect("an open embedding job names an exported occurrence");
        let vector = TestEngine::vector_for(row.text.as_deref().unwrap_or_default());
        publisher
            .publish(
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
            )
            .unwrap();
    }
}

fn bulk_scaffold(corpus: &Corpus, home: &Path, bounds: &DriveBounds, now: i64) -> ProjectionRows {
    let (projection, hold, _) = corpus.bootstrap_within(home, bounds.hold, bounds.batch);
    embed_pending(corpus, &projection, home, bounds.hold, now);
    let rows = projection_rows(&search_file(home));
    assert!(
        rows.live.pending_embedding.is_empty(),
        "the bulk scaffold embeds every open job"
    );
    let (_, lease) = projection.close();
    drop(lease);
    corpus
        .kernel
        .release_source_hold(&corpus.binding(), &hold.hold_id, hold.captured_at)
        .unwrap();
    rows
}

pub struct Run {
    pub report: AgingReport,
    pub report_bytes: Vec<u8>,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub checkpoint: Checkpoint,
    pub full_incarnation_id: String,
    pub full: StateSnapshot,
    pub resumed: StateSnapshot,
    pub coverage: Coverage,
}

pub struct Plan {
    pub rendering: Rendering,
    pub steps: Vec<Planned>,
    pub checkpoint_step: u32,
    pub bounds: DriveBounds,
}

pub fn plan(messages: u32) -> Result<Plan, RunError> {
    let log = generate_all(SEED, &world(messages), Mode::Generate)?.log;
    let rendering = render(
        &log,
        &RenderConfig {
            project_id: PROJECT.to_string(),
            repository_id: "repo-0".to_string(),
            object_format: "sha1".to_string(),
        },
    )
    .unwrap();
    for message in &rendering.messages {
        assert_eq!(message.session_id, SESSION);
    }
    let steps = planned(&log);
    let checkpoint_step = straddling_step(&log, &steps).ok_or(RunError::NoStraddlingStep)?;
    let bounds = DriveBounds::admitting(&rendering, &steps);
    Ok(Plan {
        rendering,
        steps,
        checkpoint_step,
        bounds,
    })
}

pub fn live(stores: &mut Stores, steps: &[Planned]) {
    for planned in steps {
        stores.apply(planned);
        stores.drain(planned.now_ms);
    }
}

pub struct Full {
    pub incarnation_id: String,
    pub state: StateSnapshot,
    pub rows: ProjectionRows,
    pub against_bulk: GuardComparison,
}

pub fn full_life(plan: &Plan, charges: &mut Charges) -> Result<Full, RunError> {
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan);
    live(&mut stores, &plan.steps);
    let state = stores.snapshot();
    let rows = stores.projection_rows();
    let last_now = plan.steps.last().unwrap().now_ms;
    let bulk_home = charges.occupy()?;
    let bulk_rows = bulk_scaffold(&stores.corpus, bulk_home.path(), &plan.bounds, last_now);
    let against_bulk = GuardComparison::of(
        (&rows, ConstructionKind::CatchUp),
        (&bulk_rows, ConstructionKind::Bulk),
    )?;
    let incarnation_id = stores.incarnation();
    drop(stores.close());
    charges.vacate(bulk_home)?;
    charges.vacate(root)?;
    Ok(Full {
        incarnation_id,
        state,
        rows,
        against_bulk,
    })
}

struct Resumed {
    checkpoint: Checkpoint,
    reopened: StateSnapshot,
    state: StateSnapshot,
    rows: ProjectionRows,
}

fn resumed_life(
    plan: &Plan,
    charges: &mut Charges,
    coverage: &mut Coverage,
) -> Result<Resumed, RunError> {
    let k = plan.checkpoint_step as usize;
    let prefix_root = charges.occupy()?;
    let mut prefix = Stores::open(prefix_root.path(), plan);
    live(&mut prefix, &plan.steps[..k]);
    let prefix_state = prefix.snapshot();
    let closed = prefix.close();
    let copy_root = charges.occupy()?;
    let (checkpoint, copied) = closed.copy(copy_root.path())?;
    coverage.record("flt_quiescence_receipt_all_zero").unwrap();
    charges.vacate(prefix_root)?;
    let mut resumed = copied.reopen(&checkpoint, plan.steps[k].now_ms)?;
    let reopened = resumed.snapshot();
    StateSnapshot::compare(&prefix_state, &reopened)?;
    coverage
        .record("ing_aged_arm_restarted_between_sessions")
        .unwrap();
    live(&mut resumed, &plan.steps[k..]);
    let state = resumed.snapshot();
    let rows = resumed.projection_rows();
    drop(resumed.close());
    charges.vacate(copy_root)?;
    Ok(Resumed {
        checkpoint,
        reopened,
        state,
        rows,
    })
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
    let mut coverage = Coverage::default();
    // Planning runs under the clock: the elapsed bound covers the whole run.
    let plan = plan(config.messages)?;
    let steps = plan.steps.len() as u32;

    let full = full_life(&plan, &mut charges)?;
    let resumed = resumed_life(&plan, &mut charges, &mut coverage)?;
    StateSnapshot::advanced(&resumed.reopened, &resumed.state)?;
    StateSnapshot::compare(&full.state, &resumed.state)?;
    let against_resumed = GuardComparison::of(
        (&full.rows, ConstructionKind::CatchUp),
        (&resumed.rows, ConstructionKind::Bulk),
    )?;
    let window_deaths = WindowDeaths::count(
        &resumed.state.kernel,
        resumed.reopened.commit_seq,
        resumed.state.commit_seq,
    );
    if window_deaths.supersessions > 0 {
        coverage
            .record("ing_window_has_pre_snapshot_supersession")
            .unwrap();
    }
    if window_deaths.retirements > 0 {
        coverage
            .record("ing_window_has_pre_snapshot_retirement")
            .unwrap();
    }

    let identity = identity(
        &profile,
        SIMULATOR_VERSION,
        SEED,
        json!({
            "steps": steps,
            "checkpoint_step": plan.checkpoint_step,
            "messages": config.messages,
        }),
        &std::env::current_exe().unwrap(),
    );
    let mut report = AgingReport {
        schema: AGING_REPORT_SCHEMA.to_string(),
        eval_run_id: eval_run_id(&identity).unwrap(),
        profile_digest: profile.digest().unwrap(),
        claim_boundary: ClaimBoundary::pinned(),
        steps,
        checkpoint_step: plan.checkpoint_step,
        checkpoint_digest: resumed.checkpoint.digest()?,
        receipt: resumed.checkpoint.receipt.clone(),
        full_guard_digest: full.state.guard_digest()?,
        resumed_guard_digest: resumed.state.guard_digest()?,
        commit_seq_at_checkpoint: resumed.reopened.commit_seq,
        commit_seq_at_end: full.state.commit_seq,
        against_resumed,
        against_bulk: full.against_bulk,
        window_deaths,
        markers: coverage.fired().iter().map(|m| m.to_string()).collect(),
        envelope: charges.envelope.clone(),
    };
    charges.retain_publish_root()?;
    let bytes = loop {
        report.envelope = charges.envelope.clone();
        let bytes = serde_json::to_vec_pretty(&report.serialize()?).unwrap();
        let peak = charges.envelope.peaks.artifact_bytes;
        charges.observe(eval_core::Resource::ArtifactBytes, bytes.len() as u64)?;
        if charges.envelope.peaks.artifact_bytes == peak {
            break bytes;
        }
    };
    let manifest = manifest(
        identity,
        &report,
        &bytes,
        &resumed.checkpoint,
        started_at_ms,
    )?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_value()).unwrap();
    publish_file(&config.publish.join(REPORT_FILE), &bytes).map_err(publish_refused)?;
    publish_file(&config.publish.join(MANIFEST_FILE), &manifest_bytes).map_err(publish_refused)?;
    Ok(Run {
        report,
        report_bytes: bytes,
        manifest,
        manifest_bytes,
        checkpoint: resumed.checkpoint,
        full_incarnation_id: full.incarnation_id,
        full: full.state,
        resumed: resumed.state,
        coverage,
    })
}

fn manifest(
    identity: RunIdentity,
    report: &AgingReport,
    report_bytes: &[u8],
    checkpoint: &Checkpoint,
    started_at_ms: i64,
) -> Result<Manifest, RunError> {
    let sample = format!("aging:{}", report.checkpoint_step);
    let published: Value = serde_json::from_slice(report_bytes).expect("the report is JSON");
    Ok(Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        eval_run_id: report.eval_run_id.clone(),
        run_identity: identity,
        start_ms: started_at_ms,
        end_ms: started_at_ms + i64::try_from(report.envelope.peaks.elapsed_ms).unwrap(),
        status: RunStatus::Completed,
        error: None,
        sample_ids: vec![sample.clone()],
        sample_order: vec![sample],
        sample_epoch: 1,
        retry_lineage: Vec::new(),
        result_digest: AgingReport::result_digest(&published)?,
        witness_digest: checkpoint.digest()?,
        attestation: Attestation::None,
        tokenizer_profile: TokenizerProfile {
            name: "none".to_string(),
            revision: "lexical-projection".to_string(),
            digest: sha256_hex(b"lexical-projection"),
        },
        cut_receipts: [Cut::AtQuiescence, Cut::AfterRecovery, Cut::EndOfRun]
            .into_iter()
            .map(|cut| CutReceipt {
                cut,
                outcome: CutOutcome::Reached,
            })
            .collect(),
        residue: Manifest::field_schema().residue().collect(),
        construction: Construction::Replay,
        execution_mode: ExecutionMode::PrefixThenGenerate,
        failure_class_table_digest: FAILURE_CLASS_TABLE_DIGEST.to_string(),
        ingestion: Ingestion::AdapterIngestedNoProductionCaller,
        memory_reviewer_model_calls: MemoryReviewerModelCalls::Excluded,
        analysis_family_digest: None,
        recency_baseline: None,
        reachability: Reachability::TestOnly,
        claim_boundary: ClaimBoundary::pinned(),
        component_versions: ComponentVersions {
            generator: GENERATOR_VERSION.to_string(),
            event_schema: EVENT_SCHEMA_VERSION.to_string(),
            reducer: REDUCER_VERSION.to_string(),
            oracles: PAIRING_POLICY_VERSION.to_string(),
            execution_image: "in-process".to_string(),
            task_corpus: format!("generated:{SEED:#x}"),
            judge: "none".to_string(),
        },
        envelope_bounds: report.envelope.bounds.clone(),
        envelope_peaks: report.envelope.peaks.clone(),
        arm_rates: BTreeMap::new(),
    })
}

pub const USAGE: &str = "aging --scale <s0|s1|s2> --messages <n> --elapsed-bound-ms <n> \
--approved-by <name> --approval-run-id <hex64> --publish <dir>";

const FLAGS: [&str; 6] = [
    "scale",
    "messages",
    "elapsed-bound-ms",
    "approved-by",
    "approval-run-id",
    "publish",
];

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
        messages: u32::try_from(number("messages")?)
            .map_err(|error| format!("--messages: {error}"))?,
        elapsed_bound_ms: number("elapsed-bound-ms")?,
        approval: Some(Approval {
            approved_by: take("approved-by"),
            approved_at_run_id: take("approval-run-id"),
        }),
        publish: PathBuf::from(take("publish")),
    })
}
