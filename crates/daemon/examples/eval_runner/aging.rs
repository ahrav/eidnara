//! Suite C replays one generated history end to end and from a quiescent
//! checkpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daemon::embedding_publication::{EmbeddingPublisher, VectorPublication};
use daemon::harness_sources::{Harness, SessionIdentity, SourcePublisher, opencode_units};
use daemon::search_catchup::{CatchUpConsumer, EpisodeBounds, EpisodeEnd, SearchCatchUp};
use daemon::search_projection::SearchProjection;
use eval_core::{
    ConstructionKind, Death, Descriptor, EnvelopeExceeded, EventId, EventLog, GuardComparison,
    HistoricalRows, LiveRows, Mode, Payload, ProjectionRows, RenderConfig, Rendering, Segment,
    SessionSpec, StateSnapshot, StoreFamily, Unenumerated, WorkCounter, WorldConfig, generate_all,
    render,
};
use kernel::{
    ArtifactDestination, CommitPageBounds, CurrentInputDescriptor, EligibilityBinding,
    ProjectScope, ProviderEgress, Sensitivity, SourceRow,
};
use memory_store::{MemoryStore, StoredHistorySegment};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::campaign::{Charges, sha256_hex};
use super::support::embedding_fixtures::{
    Corpus, GENERATION, PROJECT, SCOPE, TestEngine, batch_bounds, generation, hold_admission,
    intent, kernel_incarnation_id, source_page_bounds,
};

pub const SEED: u64 = 0x5EED_C000_0000_0004;
const SESSION: &str = "session-0";
const EPOCH_MS: i64 = 1_700_000_000_000;
const MAX_EPISODES_PER_DRAIN: u32 = 64;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("envelope exceeded: {0:?}")]
    Envelope(#[from] EnvelopeExceeded),
    #[error("unenumerated divergence: {0}")]
    Guard(#[from] Unenumerated),
    #[error("no step straddles a supersession and a retirement")]
    NoStraddlingStep,
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
        max_events_per_log: messages.max(64) * 2,
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

fn episode_bounds() -> EpisodeBounds {
    EpisodeBounds {
        commits: CommitPageBounds {
            max_commits: 64.try_into().unwrap(),
            max_rows: 64.try_into().unwrap(),
            max_payload_bytes: (1u64 << 20).try_into().unwrap(),
        },
        hold_admission: hold_admission(),
        source_page: source_page_bounds(),
        max_source_pages: 8.try_into().unwrap(),
        max_source_encoded_bytes: (1u64 << 20).try_into().unwrap(),
        batch: batch_bounds(),
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
    pub projection_snapshot: i64,
    memory: MemoryStore,
    rendering: Rendering,
    chains: BTreeMap<String, Vec<String>>,
    dead: BTreeSet<String>,
    applied: u32,
}

impl Stores {
    pub fn open(root: &Path, rendering: Rendering) -> Self {
        let corpus = Corpus::open(root);
        corpus.seed();
        let memory = MemoryStore::open(&daemon::store_descriptor_in(root)).unwrap();
        let (projection, consumer, snapshot) = Self::bootstrap(&corpus, root, EPOCH_MS);
        Self {
            root: root.to_path_buf(),
            corpus,
            projection,
            consumer,
            projection_snapshot: snapshot,
            memory,
            rendering,
            chains: BTreeMap::new(),
            dead: BTreeSet::new(),
            applied: 0,
        }
    }

    fn bootstrap(
        corpus: &Corpus,
        root: &Path,
        now: i64,
    ) -> (SearchProjection, CatchUpConsumer, i64) {
        let (projection, hold, _) = corpus.bootstrap_with_hold(root);
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
        (projection, consumer, hold.snapshot)
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

    pub fn applied(&self) -> u32 {
        self.applied
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
        let mut catch_up = SearchCatchUp::new(&self.corpus.kernel, &self.projection);
        for _ in 0..MAX_EPISODES_PER_DRAIN {
            let report = catch_up
                .run_episode(&self.consumer, &episode_bounds(), now, &mut |_| {})
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
        embed_pending(&self.corpus, &self.projection, &self.root, now);
    }

    fn pending(&self, counter: WorkCounter) -> u64 {
        match counter {
            WorkCounter::OutboxUnpublished => {
                self.corpus.kernel.pending_outbox(1024).unwrap().len() as u64
            }
            WorkCounter::CatchUpLag => {
                let acknowledged: i64 = read_only(&search_file(&self.root))
                    .query_row(
                        "SELECT checkpoint_commit_seq FROM projection_checkpoint",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                u64::try_from(self.tip() - acknowledged).unwrap()
            }
            WorkCounter::EmbeddingOpen => count(
                &search_file(&self.root),
                "SELECT COUNT(*) FROM embedding_jobs WHERE state IN ('pending','admitted')",
            ),
            WorkCounter::CaptureJobsPending => count(
                &memory_file(&self.root),
                "SELECT COUNT(*) FROM memory_capture_jobs WHERE commit_seq IS NULL AND abandoned_at_ms IS NULL",
            ),
            WorkCounter::ReviewerJobsOpen => count(
                &memory_file(&self.root),
                "SELECT COUNT(*) FROM memory_reviewer_jobs WHERE state<>'terminal'",
            ),
        }
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

fn embed_pending(corpus: &Corpus, projection: &SearchProjection, root: &Path, now: i64) {
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
    let rows: Vec<SourceRow> = corpus.export();
    let project = ProjectScope::new(PROJECT).unwrap();
    let mut publisher = EmbeddingPublisher::new(&corpus.kernel, projection);
    for occurrence in open {
        let Some(row) = rows
            .iter()
            .find(|row| row.detail.occurrence_id == occurrence)
        else {
            continue;
        };
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

fn bulk_scaffold(corpus: &Corpus, home: &Path, now: i64) -> ProjectionRows {
    let (projection, hold, _) = corpus.bootstrap_with_hold(home);
    embed_pending(corpus, &projection, home, now);
    let rows = projection_rows(&search_file(home));
    let (_, lease) = projection.close();
    drop(lease);
    corpus
        .kernel
        .release_source_hold(&corpus.binding(), &hold.hold_id, hold.captured_at)
        .unwrap();
    rows
}

pub struct Plan {
    pub rendering: Rendering,
    pub steps: Vec<Planned>,
    pub checkpoint_step: u32,
}

pub fn plan(messages: u32) -> Result<Plan, RunError> {
    let log = generate_all(SEED, &world(messages), Mode::Generate)
        .unwrap()
        .log;
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
    Ok(Plan {
        rendering,
        steps,
        checkpoint_step,
    })
}

pub fn live(stores: &mut Stores, steps: &[Planned]) {
    for planned in steps {
        stores.apply(planned);
        stores.drain(planned.now_ms);
    }
}

fn count(file: &Path, sql: &str) -> u64 {
    read_only(file)
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| u64::try_from(n).unwrap())
        .unwrap()
}

pub struct Full {
    pub incarnation_id: String,
    pub state: StateSnapshot,
    pub rows: ProjectionRows,
    pub against_bulk: GuardComparison,
}

pub fn full_life(plan: &Plan, charges: &mut Charges) -> Result<Full, RunError> {
    let root = charges.occupy()?;
    let mut stores = Stores::open(root.path(), plan.rendering.clone());
    live(&mut stores, &plan.steps);
    let state = stores.snapshot();
    let rows = stores.projection_rows();
    let last_now = plan.steps.last().unwrap().now_ms;
    let bulk_home = charges.occupy()?;
    let bulk_rows = bulk_scaffold(&stores.corpus, bulk_home.path(), last_now);
    let against_bulk = GuardComparison::of(
        (&rows, ConstructionKind::CatchUp),
        (&bulk_rows, ConstructionKind::Bulk),
    )?;
    let incarnation_id = stores.incarnation();
    drop(stores);
    charges.vacate(bulk_home)?;
    charges.vacate(root)?;
    Ok(Full {
        incarnation_id,
        state,
        rows,
        against_bulk,
    })
}
