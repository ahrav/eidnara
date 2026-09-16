//! This harness measures one `exhaustive` call over N stored rows of dimension D; the kernel judges every page.
//! It is a research artifact for the row-allocation study in `docs/performance/dense-walk-row-allocation-log.md`, not a regression gate.
//! Fixture seeding, warmup, the reference ranking, and JSON output are outside every timed boundary.
//!
//! Modes (`EIDNARA_WALK_MODE`):
//! - `time` (default): wall time of whole `exhaustive` calls.
//! - `alloc`: Rust allocator calls and bytes during one `exhaustive` call.
//! - `stages`: wall time of each stage run alone over the same rows: SQLite paging with and without column materialization, decoding, kernel judgment, scoring.
//! - `matrix`: the prototype walk in four cells (judge every row or only rows that can enter the top-K; materialize rows or borrow them) against the production walk, interleaved.
//! - `layered`: the same against `rank_layers` over one resident layer.
//! - `lexical`: `lexical::retrieve` with one probe against the probe scan alone.
//!
//! Knobs: `EIDNARA_WALK_ROWS` (default 2000; the study used 20000), `EIDNARA_WALK_DIM` (default 384), `EIDNARA_WALK_SAMPLES` (default 2, a smoke), `EIDNARA_WALK_PAGE_ROWS` (default 1024), `EIDNARA_WALK_K` (default 10), `EIDNARA_WALK_KERNEL_OBJECTS`, `EIDNARA_WALK_EXCLUDE_EVERY`, `EIDNARA_WALK_ROW_BUFFER`, `EIDNARA_WALK_SCAN_ROWS`, `EIDNARA_WALK_LEXICAL_ORDER` (a syscall-attributable call sequence for the page-cache diagnosis).

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CommitIntent, DecisionPayload,
    DecisionSpec, Dimension, DomainSpec, EventKind, KernelStore, ProjectScope, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, ProjectionCheckpoint, VectorGeneration,
    apply_batch, register_generation,
};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::score::{Ranked, TopK, rank_order, score};
use retrieval::dense::{
    ExhaustiveQuery, ExhaustiveRanking, Layer, LayeredQuery, OracleBounds, Precedence, exhaustive,
    rank_layers,
};
use retrieval::eligibility::{Authority, OccurrenceCandidate, judge_tracked};
use retrieval::lexical::{LexicalBounds, Probe, RetrievalBounds, analyze, compile, retrieve};
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use rusqlite::params;
use sha2::{Digest, Sha256};
use storage::{
    GuardedConn, Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite,
};

// The counting allocator counts every thread's calls. The timed window runs on
// one thread and the kernel reads on the caller's thread, so the count belongs
// to the call. SQLite's C allocations are not Rust allocations and are not seen.

struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.load(Ordering::Relaxed) {
            DEALLOCS.fetch_add(1, Ordering::Relaxed);
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() && COUNTING.load(Ordering::Relaxed) {
            REALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
            if new_size >= layout.size() {
                grow(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Debug, Clone, Copy, Default)]
struct AllocReport {
    allocs: usize,
    reallocs: usize,
    deallocs: usize,
    bytes: usize,
    peak_live: usize,
}

fn count_allocations<T>(f: impl FnOnce() -> T) -> (T, AllocReport) {
    ALLOCS.store(0, Ordering::Relaxed);
    REALLOCS.store(0, Ordering::Relaxed);
    DEALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::SeqCst);
    let value = f();
    COUNTING.store(false, Ordering::SeqCst);
    (
        value,
        AllocReport {
            allocs: ALLOCS.load(Ordering::Relaxed),
            reallocs: REALLOCS.load(Ordering::Relaxed),
            deallocs: DEALLOCS.load(Ordering::Relaxed),
            bytes: BYTES.load(Ordering::Relaxed),
            peak_live: PEAK.load(Ordering::Relaxed),
        },
    )
}

// The fixture is a kernel that admits every object and a projection with N
// stored vectors of one generation.

const DOMAIN: &str = "domain";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";
const HOLD: &str = "hold-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const GENERATION: &str = "gen-1";
const TOLERANCE: f64 = 1e-3;
const KERNEL_COMMIT_ROWS: usize = 2_000;
const PROJECTION_BATCH_ROWS: usize = 4_000;

struct Config {
    rows: usize,
    dimension: u32,
    samples: usize,
    page_rows: usize,
    k: usize,
    /// Objects seeded in the kernel; at least `rows`. The surplus is never projected.
    kernel_objects: usize,
    /// Every `exclude_every`-th object gets a decision but no admission, so the kernel excludes it; 0 admits every object.
    exclude_every: usize,
    /// Borrowing cells decode each row into a scratch holding one row instead of one page.
    row_buffer: bool,
    mode: String,
}

impl Config {
    fn from_env() -> Self {
        fn var<T: std::str::FromStr>(name: &str, default: T) -> T {
            std::env::var(name)
                .ok()
                .map(|value| {
                    value
                        .parse()
                        .unwrap_or_else(|_| panic!("{name} must parse: {value:?}"))
                })
                .unwrap_or(default)
        }
        Self {
            rows: var("EIDNARA_WALK_ROWS", 2_000),
            dimension: var("EIDNARA_WALK_DIM", 384),
            samples: var("EIDNARA_WALK_SAMPLES", 2),
            page_rows: var("EIDNARA_WALK_PAGE_ROWS", 1_024),
            k: var("EIDNARA_WALK_K", 10),
            kernel_objects: var("EIDNARA_WALK_KERNEL_OBJECTS", 0),
            exclude_every: var("EIDNARA_WALK_EXCLUDE_EVERY", 0),
            row_buffer: var("EIDNARA_WALK_ROW_BUFFER", 0u8) != 0,
            mode: std::env::var("EIDNARA_WALK_MODE").unwrap_or_else(|_| "time".to_string()),
        }
    }

    fn admitted(&self, index: usize) -> bool {
        self.exclude_every == 0 || index % self.exclude_every != self.exclude_every - 1
    }

    fn layout(&self) -> RowLayout {
        RowLayout {
            dimension: self.dimension,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
        }
    }

    fn bounds(&self) -> OracleBounds {
        OracleBounds {
            k: NonZeroUsize::new(self.k).unwrap(),
            page_rows: NonZeroUsize::new(self.page_rows).unwrap(),
            max_rows: NonZeroUsize::new(self.rows.max(1)).unwrap(),
        }
    }
}

fn generation(dimension: u32) -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: dimension,
        generation_epoch: 1,
    }
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "dense-walk-bench".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "bench".to_string(),
        cause: "retrieval".to_string(),
    }
}

fn scope_spec() -> ScopeSpec {
    ScopeSpec {
        scope_id: SCOPE.to_string(),
        object_id: SCOPE.to_string(),
        source_id: SCOPE.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![ScopeTermSpec {
            dimension: Dimension::Project.as_str().to_string(),
            operator: "exact".to_string(),
            exact_value: Some(PROJECT.to_string()),
            ..ScopeTermSpec::default()
        }],
    }
}

fn decision(object: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: Some(SCOPE.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("summary {object}"),
            rationale: format!("rationale {object}"),
        },
        source_kind: "repository".to_string(),
        source_id: object.to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn admission(object: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "bench".to_string(),
        },
    }
}

fn open_store(dir: &Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-bench".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir
                    .join("search")
                    .join("search.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap()
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(PROJECTION_BATCH_ROWS).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 24).unwrap(),
        max_local_mutations: NonZeroUsize::new(1 << 16).unwrap(),
        max_pending: NonZeroUsize::new(1 << 16).unwrap(),
    }
}

/// A fixed linear congruential stream keeps every run on identical rows.
fn unit_rows(rows: usize, dimension: u32) -> Vec<Vec<f32>> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..rows)
        .map(|_| {
            let raw: Vec<f64> = (0..dimension)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
                })
                .collect();
            let norm = raw.iter().map(|v| v * v).sum::<f64>().sqrt();
            raw.iter().map(|v| (v / norm) as f32).collect()
        })
        .collect()
}

fn object_id(index: usize) -> String {
    format!("obj-{index:07}")
}

fn occurrence_id(object: &str) -> String {
    let identity = [("object_id", object)];
    encode_preserving_span(&Occurrence {
        class: OccurrenceClass::CanonicalClaims.code(),
        identity: &identity,
        revision: "1",
        representation: "decision_summary",
        span: None,
    })
    .unwrap()
    .occurrence_id
}

struct Fixture {
    _root: tempfile::TempDir,
    kernel: KernelStore,
    store: SqliteStore,
    project: ProjectScope,
    generation: VectorGeneration,
    layout: RowLayout,
    /// Parallel to `occurrence_ids`, in insertion (object) order, not identifier order.
    rows: Vec<Vec<f32>>,
    occurrence_ids: Vec<String>,
    admitted: Vec<bool>,
    query: Vec<f32>,
    seed_seconds: f64,
}

impl Fixture {
    fn build(config: &Config) -> Self {
        let started = Instant::now();
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
        let objects: Vec<String> = (0..config.rows).map(object_id).collect();
        let kernel_objects: Vec<String> = (0..config.kernel_objects.max(config.rows))
            .map(object_id)
            .collect();
        kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "bench".to_string(),
                    source_kind: "bench".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(scope_spec())?;
                Ok(String::new())
            })
            .unwrap();
        for (chunk, objects) in kernel_objects.chunks(KERNEL_COMMIT_ROWS).enumerate() {
            kernel
                .commit(intent(&format!("objects-{chunk}")), |envelope| {
                    for (offset, object) in objects.iter().enumerate() {
                        envelope.insert_decision(decision(object))?;
                        if config.admitted(chunk * KERNEL_COMMIT_ROWS + offset) {
                            envelope.record_admission(admission(object))?;
                        }
                    }
                    Ok(String::new())
                })
                .unwrap();
        }
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let through = kernel.tip().unwrap();
        let generation = generation(config.dimension);
        let store = open_store(root.path());
        store
            .with_conn_fenced(|conn| {
                install_identity(
                    conn,
                    &ProjectionIdentity {
                        schema_version: retrieval::SCHEMA_VERSION,
                        kernel_incarnation_id: incarnation.clone(),
                        projection_policy_version: "source-policy.v1".to_string(),
                        identity_contract_version: "search-projection-identity-v3".to_string(),
                        limit_manifest_protocol_version: "limits.v1".to_string(),
                        embedding_model: "model-a".to_string(),
                        tokenizer_fingerprint: "fp-a".to_string(),
                        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                            .as_str()
                            .to_string(),
                        vector_dimension: config.dimension,
                        generation_epoch: 1,
                    },
                    1,
                )
                .unwrap();
                register_generation(conn, &generation, 1).unwrap();
                Ok(())
            })
            .unwrap();
        let rows = unit_rows(config.rows, config.dimension);
        let texts: Vec<String> = objects.iter().map(|o| format!("text {o}")).collect();
        for range in (0..config.rows)
            .step_by(PROJECTION_BATCH_ROWS)
            .map(|start| start..(start + PROJECTION_BATCH_ROWS).min(config.rows))
        {
            let identities: Vec<[(&str, &str); 1]> = range
                .clone()
                .map(|i| [("object_id", objects[i].as_str())])
                .collect();
            let records: Vec<OccurrenceRecord<'_>> = range
                .clone()
                .zip(&identities)
                .map(|(i, identity)| OccurrenceRecord {
                    occurrence: Occurrence {
                        class: OccurrenceClass::CanonicalClaims.code(),
                        identity,
                        revision: "1",
                        representation: "decision_summary",
                        span: None,
                    },
                    payload: Payload::Whole(&texts[i]),
                    domain_id: DOMAIN,
                    sensitivity: Sensitivity::Normal,
                    source_object_id: &objects[i],
                    source_evidence_id: "evidence",
                    source_artifact_digest: DIGEST,
                    created_commit_seq: through,
                })
                .collect();
            let batch = ProjectionBatch {
                identity: MutationIdentity {
                    kernel_incarnation_id: incarnation.clone(),
                    hold_id: HOLD.to_string(),
                    snapshot_commit_seq: 0,
                    through_commit_seq: through,
                },
                records,
                invalidations: vec![],
                generation_id: Some(GENERATION),
            };
            store
                .with_conn_fenced(|conn| {
                    apply_batch(conn, &batch, batch_bounds(), through).unwrap();
                    Ok(())
                })
                .unwrap();
        }
        let occurrence_ids: Vec<String> = objects.iter().map(|o| occurrence_id(o)).collect();
        store
            .with_conn_fenced(|conn| {
                {
                    let mut insert = conn.prepare(
                        "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,?4,1,1,1)",
                    )?;
                    for (id, row) in occurrence_ids.iter().zip(&rows) {
                        insert.execute(params![id, GENERATION, codec::encode(row), config.dimension])?;
                    }
                }
                conn.execute("UPDATE embedding_jobs SET state='embedded'", [])?;
                Ok(())
            })
            .unwrap();
        let query = unit_rows(1, config.dimension).pop().unwrap();
        Self {
            _root: root,
            kernel,
            store,
            project: ProjectScope::new(PROJECT).unwrap(),
            generation,
            layout: config.layout(),
            rows,
            occurrence_ids,
            admitted: (0..config.rows).map(|i| config.admitted(i)).collect(),
            query,
            seed_seconds: started.elapsed().as_secs_f64(),
        }
    }

    fn authority(&self) -> Authority<'_> {
        Authority {
            project: &self.project,
            destination: ArtifactDestination::Local,
        }
    }

    fn request(&self, bounds: OracleBounds) -> ExhaustiveQuery<'_> {
        ExhaustiveQuery {
            generation: &self.generation,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            query: &self.query,
            authority: self.authority(),
            bounds,
        }
    }

    fn rank(&self, bounds: OracleBounds) -> ExhaustiveRanking {
        let request = self.request(bounds);
        self.store
            .with_conn(|conn| {
                Ok(exhaustive(conn, &self.kernel, &request, &EvalBudget::unbounded()).unwrap())
            })
            .unwrap()
    }

    /// f64 inner product in coordinate order, score descending then identifier ascending: the independent expectation.
    fn reference(&self, k: usize) -> Vec<(String, f64)> {
        let mut scored: Vec<(String, f64)> = self
            .occurrence_ids
            .iter()
            .zip(&self.rows)
            .zip(&self.admitted)
            .filter(|(_, admitted)| **admitted)
            .map(|((id, row), _)| {
                let mut sum = 0.0f64;
                for (q, r) in self.query.iter().zip(row) {
                    sum += f64::from(*q) * f64::from(*r);
                }
                (id.clone(), sum)
            })
            .collect();
        scored.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
        scored.truncate(k);
        scored
    }
}

fn keyed(ranking: &ExhaustiveRanking) -> Vec<(String, f64)> {
    ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

const CURRENT_PENDING: &str = "EXISTS(SELECT 1 FROM embedding_jobs j
    WHERE j.occurrence_id=o.occurrence_id AND j.generation_id=?1 AND j.stop_reason IS NULL
      AND (j.state='pending' OR (j.state='admitted' AND j.host_job_id IS NOT NULL)))";

/// The oracle's page query, issued directly so paging is timed without decode or judgment.
fn page_sql() -> String {
    let classes = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| retrieval::batch::dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,v.vector,
                CASE WHEN v.vector IS NULL THEN {} ELSE 0 END
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         LEFT JOIN occurrence_vectors v ON v.occurrence_id=o.occurrence_id AND v.generation_id=?1
         WHERE t.occurrence_id IS NULL AND +o.class IN ({}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3",
        CURRENT_PENDING,
        classes
    )
}

/// The layered walk's page query: the same live rows without the stored vector.
fn live_sql() -> String {
    let classes = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| retrieval::batch::dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest,NULL,
                {CURRENT_PENDING}
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE t.occurrence_id IS NULL AND +o.class IN ({classes}) AND o.occurrence_id>?2
         ORDER BY o.occurrence_id
         LIMIT ?3"
    )
}

/// One resident base layer holding every row, in identifier byte order as the resolver requires.
struct ResidentLayer {
    checkpoint: ProjectionCheckpoint,
    occurrence_ids: Vec<String>,
    rows: Vec<Vec<f32>>,
}

impl ResidentLayer {
    fn build(fixture: &Fixture) -> Self {
        let mut pairs: Vec<(&String, &Vec<f32>)> =
            fixture.occurrence_ids.iter().zip(&fixture.rows).collect();
        pairs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        Self {
            checkpoint: ProjectionCheckpoint {
                snapshot_commit_seq: 0,
                checkpoint_commit_seq: 1,
                hold_id: HOLD.to_string(),
            },
            occurrence_ids: pairs.iter().map(|(id, _)| (*id).clone()).collect(),
            rows: pairs.iter().map(|(_, row)| (*row).clone()).collect(),
        }
    }

    fn layer(&self) -> Layer<'_> {
        Layer {
            precedence: Precedence {
                base_epoch: 1,
                delta_ordinal: 0,
            },
            checkpoint: &self.checkpoint,
            occurrence_ids: &self.occurrence_ids,
            rows: &self.rows,
            tombstones: &[],
        }
    }
}

fn rank_resident(fixture: &Fixture, layer: &ResidentLayer, config: &Config) -> ExhaustiveRanking {
    let layers = [layer.layer()];
    let query = LayeredQuery {
        generation: &fixture.generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query: &fixture.query,
        authority: fixture.authority(),
        bounds: config.bounds(),
        layers: &layers,
        max_entries: NonZeroUsize::new(config.rows.max(1)).unwrap(),
    };
    fixture
        .store
        .with_conn(|conn| {
            Ok(rank_layers(conn, &fixture.kernel, &query, &EvalBudget::unbounded()).unwrap())
        })
        .unwrap()
        .ranking
}

struct PageRow {
    candidate: OccurrenceCandidate,
    stored: Option<Vec<u8>>,
}

/// The walk's paging exactly as `oracle::read_page` materializes it, without decode or judgment.
fn stage_page_materialized(conn: &GuardedConn<'_>, sql: &str, page_rows: usize) -> Vec<PageRow> {
    let mut all = Vec::new();
    let mut after = String::new();
    let mut statement = conn.prepare_cached(sql).unwrap();
    loop {
        let mut rows = statement
            .query(params![GENERATION, after, page_rows as i64 + 1])
            .unwrap();
        let mut page = Vec::with_capacity(page_rows);
        let mut more = false;
        while let Some(row) = rows.next().unwrap() {
            if page.len() == page_rows {
                more = true;
                break;
            }
            let class = OccurrenceClass::from_code(&row.get::<_, String>(1).unwrap()).unwrap();
            page.push(PageRow {
                candidate: OccurrenceCandidate::new(
                    row.get(0).unwrap(),
                    class,
                    row.get(2).unwrap(),
                    row.get(3).unwrap(),
                    row.get(4).unwrap(),
                ),
                stored: row.get(5).unwrap(),
            });
        }
        drop(rows);
        if let Some(last) = page.last() {
            after.clone_from(&last.candidate.occurrence_id);
        }
        all.extend(page);
        if !more {
            break;
        }
    }
    all
}

/// The same paging with every column read as a borrowed reference and only a checksum kept: the SQLite floor for this query.
fn stage_page_borrowed(conn: &GuardedConn<'_>, sql: &str, page_rows: usize) -> (usize, u64) {
    let mut count = 0usize;
    let mut checksum = 0u64;
    let mut after = String::new();
    let mut last = String::new();
    let mut statement = conn.prepare_cached(sql).unwrap();
    loop {
        let mut rows = statement
            .query(params![GENERATION, after, page_rows as i64 + 1])
            .unwrap();
        let mut seen = 0usize;
        let mut more = false;
        while let Some(row) = rows.next().unwrap() {
            if seen == page_rows {
                more = true;
                break;
            }
            seen += 1;
            let id = row.get_ref(0).unwrap().as_str().unwrap();
            let class = row.get_ref(1).unwrap().as_str().unwrap();
            let object = row.get_ref(2).unwrap().as_str().unwrap();
            let revision: i64 = row.get(3).unwrap();
            let digest = row.get_ref(4).unwrap().as_str().unwrap();
            let blob = row.get_ref(5).unwrap().as_blob_or_null().unwrap();
            checksum = checksum
                .wrapping_add(id.len() as u64)
                .wrapping_add(class.len() as u64)
                .wrapping_add(object.len() as u64)
                .wrapping_add(revision as u64)
                .wrapping_add(digest.len() as u64)
                .wrapping_add(blob.map_or(0, |b| b.len() as u64));
            last.clear();
            last.push_str(id);
        }
        drop(rows);
        count += seen;
        std::mem::swap(&mut after, &mut last);
        if !more {
            break;
        }
    }
    (count, checksum)
}

fn stage_decode(blobs: &[&[u8]], layout: &RowLayout) -> Vec<Vec<f32>> {
    blobs
        .iter()
        .map(|bytes| codec::decode(bytes, layout).unwrap())
        .collect()
}

/// Decodes into one reused buffer of `page_rows * dimension` as the proposed local optimization would; the checksum keeps the work observable.
fn stage_decode_into(blobs: &[&[u8]], layout: &RowLayout, page_rows: usize) -> f64 {
    let dimension = layout.dimension as usize;
    let mut buffer: Vec<f32> = Vec::with_capacity(page_rows * dimension);
    let mut checksum = 0.0f64;
    for page in blobs.chunks(page_rows) {
        buffer.clear();
        for bytes in page {
            let (words, rest) = bytes.as_chunks::<4>();
            assert!(rest.is_empty());
            assert_eq!(words.len(), dimension);
            let start = buffer.len();
            buffer.extend(words.iter().map(|w| f32::from_le_bytes(*w)));
            let row = &buffer[start..];
            assert!(row.iter().all(|v| v.is_finite()));
            let mut sum = 0.0f64;
            for v in row {
                sum += f64::from(*v) * f64::from(*v);
            }
            assert!((sum.sqrt() - 1.0).abs() <= layout.unit_norm_tolerance);
            checksum += f64::from(row[0]);
        }
    }
    checksum
}

fn stage_judge(
    fixture: &Fixture,
    candidates: &[OccurrenceCandidate],
    page_rows: usize,
) -> (usize, usize) {
    let mut snapshot = None;
    let mut incarnation = None;
    let mut eligible = 0usize;
    let mut batches = 0usize;
    for batch in candidates.chunks(page_rows) {
        let (report, moved) = judge_tracked(
            &fixture.kernel,
            fixture.authority(),
            batch,
            &EvalBudget::unbounded(),
            &mut snapshot,
            &mut incarnation,
        )
        .unwrap();
        assert!(moved.is_none());
        batches += 1;
        eligible += report
            .occurrences
            .iter()
            .filter(|j| j.disposition == retrieval::eligibility::Disposition::Eligible)
            .count();
    }
    (eligible, batches)
}

fn stage_score(
    candidates: &[OccurrenceCandidate],
    vectors: &[Vec<f32>],
    query: &[f32],
    k: usize,
) -> Vec<Ranked> {
    let mut top: TopK<()> = TopK::new(NonZeroUsize::new(k).unwrap());
    for (candidate, vector) in candidates.iter().zip(vectors) {
        let ranked = Ranked {
            occurrence_id: candidate.occurrence_id.clone(),
            class: candidate.class,
            score: score(Metric::InnerProduct, query, vector),
        };
        top.offer(ranked, ());
    }
    top.into_ranked().into_iter().map(|(r, ())| r).collect()
}

/// One member of the prototype's top-K: the ranking key plus the candidate the final re-judgment needs.
struct Held {
    ranked: Ranked,
    candidate: OccurrenceCandidate,
}

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for Held {}
impl PartialOrd for Held {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Held {
    /// The heap's maximum is the worst-ranked member, as in `score::TopK`.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        rank_order(
            (self.ranked.score, &self.ranked.occurrence_id),
            (other.ranked.score, &other.ranked.occurrence_id),
        )
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct LazyStats {
    pages: usize,
    visited: usize,
    judged: usize,
    batches: usize,
    excluded: usize,
}

/// Prototype walk with two independent switches.
/// `lazy`: only a row that would enter the current top-K is judged, one kernel batch per page; the result set equals the judge-everything walk's because a row in the final top-K beats the worst held member at every earlier point of the walk. Off, every row of the page is judged as the oracle does.
/// `materialize`: every column and the blob are read as owned `String`/`Vec<u8>` and each row is decoded into its own `Vec<f32>`, as `read_page` and `codec::decode` do. Off, columns are borrowed and rows decode into one reused page buffer.
fn proto_walk(
    fixture: &Fixture,
    sql: &str,
    config: &Config,
    lazy: bool,
    materialize: bool,
    resident: Option<&ResidentLayer>,
) -> (Vec<Ranked>, LazyStats) {
    let layout = fixture.layout;
    let dimension = layout.dimension as usize;
    let k = config.k;
    let mut stats = LazyStats::default();
    let mut heap: std::collections::BinaryHeap<Held> =
        std::collections::BinaryHeap::with_capacity(k);
    let mut buffer: Vec<f32> = Vec::with_capacity(config.page_rows * dimension);
    let mut snapshot = None;
    let mut incarnation = None;
    fixture
        .store
        .with_conn(|conn| {
            let mut after = String::new();
            let mut last = String::new();
            let mut statement = conn.prepare_cached(sql)?;
            // Cursor into the resident layer's identifier-ordered rows; the walk visits ids in the same order.
            let mut next_resident = 0usize;
            loop {
                let mut rows =
                    statement.query(params![GENERATION, after, config.page_rows as i64 + 1])?;
                let mut seen = 0usize;
                let mut more = false;
                buffer.clear();
                // Rows of this page that could enter the top-K, with their scores.
                let mut selected: Vec<(OccurrenceCandidate, f64)> = Vec::new();
                while let Some(row) = rows.next()? {
                    if seen == config.page_rows {
                        more = true;
                        break;
                    }
                    seen += 1;
                    let (owned_candidate, owned_vector): (
                        Option<OccurrenceCandidate>,
                        Option<Vec<f32>>,
                    ) = if materialize {
                        let class = OccurrenceClass::from_code(&row.get::<_, String>(1)?).unwrap();
                        let candidate = OccurrenceCandidate::new(
                            row.get(0)?,
                            class,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        );
                        let vector = match resident {
                            // `RowAccess for Vec<Vec<f32>>` clones the row; the same copy is made here.
                            Some(layer) => {
                                assert_eq!(
                                    layer.occurrence_ids[next_resident],
                                    candidate.occurrence_id
                                );
                                let vector = layer.rows[next_resident].clone();
                                codec::validate(&vector, &layout).unwrap();
                                vector
                            }
                            None => {
                                let blob: Vec<u8> = row.get(5)?;
                                codec::decode(&blob, &layout).unwrap()
                            }
                        };
                        (Some(candidate), Some(vector))
                    } else {
                        (None, None)
                    };
                    let id: &str = match &owned_candidate {
                        Some(candidate) => &candidate.occurrence_id,
                        None => row.get_ref(0)?.as_str()?,
                    };
                    let vector: &[f32] = match (&owned_vector, resident) {
                        (Some(vector), _) => vector,
                        (None, Some(layer)) => {
                            assert_eq!(layer.occurrence_ids[next_resident], id);
                            let vector = layer.rows[next_resident].as_slice();
                            codec::validate(vector, &layout).unwrap();
                            vector
                        }
                        (None, None) => {
                            let blob = row.get_ref(5)?.as_blob()?;
                            if config.row_buffer {
                                buffer.clear();
                            }
                            let start = buffer.len();
                            let (words, rest) = blob.as_chunks::<4>();
                            assert!(rest.is_empty());
                            buffer.extend(words.iter().map(|word| f32::from_le_bytes(*word)));
                            let vector = &buffer[start..];
                            codec::validate(vector, &layout).unwrap();
                            vector
                        }
                    };
                    next_resident += 1;
                    let score = score(Metric::InnerProduct, &fixture.query, vector);
                    let enters = !lazy
                        || heap.len() < k
                        || heap.peek().is_some_and(|worst| {
                            rank_order(
                                (score, id),
                                (worst.ranked.score, &worst.ranked.occurrence_id),
                            ) == std::cmp::Ordering::Less
                        });
                    last.clear();
                    last.push_str(id);
                    if enters {
                        let candidate = match owned_candidate {
                            Some(candidate) => candidate,
                            None => OccurrenceCandidate::new(
                                last.clone(),
                                OccurrenceClass::from_code(row.get_ref(1)?.as_str()?).unwrap(),
                                row.get_ref(2)?.as_str()?.to_owned(),
                                row.get(3)?,
                                row.get_ref(4)?.as_str()?.to_owned(),
                            ),
                        };
                        selected.push((candidate, score));
                    }
                }
                drop(rows);
                stats.pages += 1;
                stats.visited += seen;
                if !selected.is_empty() {
                    let candidates: Vec<OccurrenceCandidate> =
                        selected.iter().map(|(c, _)| c.clone()).collect();
                    let (report, moved) = judge_tracked(
                        &fixture.kernel,
                        fixture.authority(),
                        &candidates,
                        &EvalBudget::unbounded(),
                        &mut snapshot,
                        &mut incarnation,
                    )
                    .unwrap();
                    assert!(moved.is_none());
                    stats.batches += 1;
                    stats.judged += candidates.len();
                    for ((candidate, score), judged) in selected.into_iter().zip(report.occurrences)
                    {
                        if judged.disposition != retrieval::eligibility::Disposition::Eligible {
                            stats.excluded += 1;
                            continue;
                        }
                        let held = Held {
                            ranked: Ranked {
                                occurrence_id: candidate.occurrence_id.clone(),
                                class: candidate.class,
                                score,
                            },
                            candidate,
                        };
                        if heap.len() < k {
                            heap.push(held);
                        } else if heap
                            .peek()
                            .is_some_and(|worst| held.cmp(worst) == std::cmp::Ordering::Less)
                        {
                            heap.pop();
                            heap.push(held);
                        }
                    }
                }
                std::mem::swap(&mut after, &mut last);
                if !more {
                    break;
                }
            }
            Ok(())
        })
        .unwrap();
    // Final re-judgment of the held set, as the oracle's `revalidate` does.
    let held: Vec<Held> = heap.into_sorted_vec();
    if held.is_empty() {
        return (Vec::new(), stats);
    }
    let candidates: Vec<OccurrenceCandidate> = held.iter().map(|h| h.candidate.clone()).collect();
    let (report, _moved) = judge_tracked(
        &fixture.kernel,
        fixture.authority(),
        &candidates,
        &EvalBudget::unbounded(),
        &mut snapshot,
        &mut incarnation,
    )
    .unwrap();
    stats.batches += 1;
    stats.judged += candidates.len();
    let ranked = held
        .into_iter()
        .zip(report.occurrences)
        .filter(|(_, judged)| judged.disposition == retrieval::eligibility::Disposition::Eligible)
        .map(|(h, _)| h.ranked)
        .collect();
    (ranked, stats)
}

/// The lexical probe query, issued directly so the engine scan is timed without judgment.
const PROBE_SQL: &str = "SELECT l.occurrence_id, l.rank, o.class, o.source_object_id, o.revision, o.source_artifact_digest
     FROM lexical l JOIN occurrences o ON o.occurrence_id=l.occurrence_id
     WHERE lexical MATCH ?1
       AND NOT EXISTS(SELECT 1 FROM occurrence_tombstones t WHERE t.occurrence_id=o.occurrence_id)
     ORDER BY l.rank, l.occurrence_id
     LIMIT ?2";

fn lexical_probes() -> Vec<Probe> {
    compile(
        &analyze(
            "text",
            LexicalBounds {
                max_input_bytes: NonZeroUsize::new(256).unwrap(),
                max_atoms: NonZeroUsize::new(8).unwrap(),
            },
        )
        .unwrap(),
    )
}

fn lexical_bounds(scan_rows: usize) -> RetrievalBounds {
    RetrievalBounds {
        max_probes: NonZeroUsize::new(8).unwrap(),
        scan_rows: NonZeroUsize::new(scan_rows).unwrap(),
        max_accepted: NonZeroUsize::new(scan_rows.min(1024)).unwrap(),
        batch_rows: NonZeroUsize::new(scan_rows.min(1024)).unwrap(),
    }
}

/// The probe scan with every column borrowed and only a checksum kept: the FTS5 floor for one probe.
fn lexical_scan_borrowed(conn: &GuardedConn<'_>, probe: &Probe, scan_rows: usize) -> (usize, u64) {
    let mut statement = conn.prepare_cached(PROBE_SQL).unwrap();
    let mut rows = statement
        .query(params![probe, scan_rows as i64 + 1])
        .unwrap();
    let mut seen = 0usize;
    let mut checksum = 0u64;
    while let Some(row) = rows.next().unwrap() {
        if seen == scan_rows {
            break;
        }
        seen += 1;
        let id = row.get_ref(0).unwrap().as_str().unwrap();
        let rank: f64 = row.get(1).unwrap();
        let class = row.get_ref(2).unwrap().as_str().unwrap();
        let object = row.get_ref(3).unwrap().as_str().unwrap();
        let revision: i64 = row.get(4).unwrap();
        let digest = row.get_ref(5).unwrap().as_str().unwrap();
        checksum = checksum
            .wrapping_add(id.len() as u64)
            .wrapping_add(rank.to_bits())
            .wrapping_add(class.len() as u64)
            .wrapping_add(object.len() as u64)
            .wrapping_add(revision as u64)
            .wrapping_add(digest.len() as u64);
    }
    (seen, checksum)
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn summary(samples: &[f64]) -> String {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    format!(
        "min={:.3} median={:.3} max={:.3} n={}",
        sorted[0],
        median,
        sorted[sorted.len() - 1],
        sorted.len()
    )
}

fn main() {
    let config = Config::from_env();
    let fixture = Fixture::build(&config);
    let bounds = config.bounds();
    eprintln!(
        "fixture rows={} dim={} page_rows={} k={} seed_seconds={:.1}",
        config.rows, config.dimension, config.page_rows, config.k, fixture.seed_seconds
    );

    // The walk's top-k must equal the independent reference before anything is timed.
    let ranking = fixture.rank(bounds);
    let reference = fixture.reference(config.k);
    assert_eq!(
        keyed(&ranking),
        reference,
        "walk disagrees with the f64 reference"
    );
    let admitted_rows = fixture.admitted.iter().filter(|a| **a).count();
    let excluded_rows: usize = ranking.consumed.excluded.iter().map(|(_, n)| n).sum();
    assert!(
        excluded_rows >= config.rows - admitted_rows,
        "every inadmissible row must be excluded at least once: {excluded_rows} < {}",
        config.rows - admitted_rows
    );
    assert_eq!(ranking.coverage.required, config.rows);
    assert_eq!(ranking.coverage.with_vector, config.rows);
    assert_eq!(
        ranking.completion,
        retrieval::dense::Completion::Complete,
        "{:?}",
        ranking.consumed
    );
    eprintln!(
        "correctness ok: judged={} batches={} pages={} excluded={:?}",
        ranking.consumed.judged,
        ranking.consumed.batches,
        ranking.consumed.pages,
        ranking.consumed.excluded
    );

    match config.mode.as_str() {
        "time" => {
            let mut samples = Vec::with_capacity(config.samples);
            for _ in 0..config.samples {
                let start = Instant::now();
                let ranking = fixture.rank(bounds);
                samples.push(ms(start.elapsed()));
                black_box(ranking);
            }
            for (i, sample) in samples.iter().enumerate() {
                println!("{{\"mode\":\"time\",\"sample\":{i},\"ms\":{sample:.3}}}");
            }
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            eprintln!(
                "exhaustive ms: {} ({:.3} us/row at median)",
                summary(&samples),
                sorted[sorted.len() / 2] * 1e3 / config.rows as f64
            );
        }
        "alloc" => {
            let (ranking, report) = count_allocations(|| fixture.rank(bounds));
            black_box(ranking);
            println!(
                "{{\"mode\":\"alloc\",\"rows\":{},\"allocs\":{},\"reallocs\":{},\"deallocs\":{},\"bytes\":{},\"peak_live\":{}}}",
                config.rows,
                report.allocs,
                report.reallocs,
                report.deallocs,
                report.bytes,
                report.peak_live
            );
            eprintln!(
                "allocations per row: alloc={:.2} realloc={:.2} dealloc={:.2} bytes/row={:.0} peak_live={}",
                report.allocs as f64 / config.rows as f64,
                report.reallocs as f64 / config.rows as f64,
                report.deallocs as f64 / config.rows as f64,
                report.bytes as f64 / config.rows as f64,
                report.peak_live
            );
        }
        "stages" => {
            let sql = page_sql();
            let layout = fixture.layout;
            // Materialize once, outside timing, for the stages that consume rows.
            let pages = fixture
                .store
                .with_conn(|conn| Ok(stage_page_materialized(conn, &sql, config.page_rows)))
                .unwrap();
            assert_eq!(pages.len(), config.rows);
            let candidates: Vec<OccurrenceCandidate> =
                pages.iter().map(|p| p.candidate.clone()).collect();
            let blobs: Vec<&[u8]> = pages.iter().map(|p| p.stored.as_deref().unwrap()).collect();
            let vectors = stage_decode(&blobs, &layout);

            let mut results: Vec<(&str, Vec<f64>, AllocReport)> = Vec::new();
            let mut record = |name: &'static str, f: &mut dyn FnMut()| {
                let mut samples = Vec::with_capacity(config.samples);
                for _ in 0..config.samples {
                    let start = Instant::now();
                    f();
                    samples.push(ms(start.elapsed()));
                }
                let ((), report) = count_allocations(&mut *f);
                results.push((name, samples, report));
            };
            record("page_borrowed", &mut || {
                let out = fixture
                    .store
                    .with_conn(|conn| Ok(stage_page_borrowed(conn, &sql, config.page_rows)))
                    .unwrap();
                assert_eq!(out.0, config.rows);
                black_box(out);
            });
            record("page_materialized", &mut || {
                let out = fixture
                    .store
                    .with_conn(|conn| Ok(stage_page_materialized(conn, &sql, config.page_rows)))
                    .unwrap();
                assert_eq!(out.len(), config.rows);
                black_box(out);
            });
            record("decode_alloc", &mut || {
                black_box(stage_decode(&blobs, &layout));
            });
            record("decode_into_page_buffer", &mut || {
                black_box(stage_decode_into(&blobs, &layout, config.page_rows));
            });
            record("judge", &mut || {
                let out = stage_judge(&fixture, &candidates, config.page_rows);
                assert_eq!(out.0, config.rows);
                black_box(out);
            });
            record("score_topk", &mut || {
                black_box(stage_score(&candidates, &vectors, &fixture.query, config.k));
            });
            record("clone_candidates", &mut || {
                let cloned: Vec<OccurrenceCandidate> = candidates.to_vec();
                black_box(cloned);
            });
            record("exhaustive", &mut || {
                black_box(fixture.rank(bounds));
            });
            for (name, samples, report) in &results {
                let mut sorted = samples.clone();
                sorted.sort_by(f64::total_cmp);
                let median = sorted[sorted.len() / 2];
                println!(
                    "{{\"mode\":\"stages\",\"stage\":\"{name}\",\"median_ms\":{median:.3},\"min_ms\":{:.3},\"max_ms\":{:.3},\"us_per_row\":{:.3},\"n\":{},\"allocs\":{},\"bytes\":{}}}",
                    sorted[0],
                    sorted[sorted.len() - 1],
                    median * 1e3 / config.rows as f64,
                    sorted.len(),
                    report.allocs,
                    report.bytes
                );
                eprintln!(
                    "{name:>24}: {} allocs/row={:.2} bytes/row={:.0}",
                    summary(samples),
                    report.allocs as f64 / config.rows as f64,
                    report.bytes as f64 / config.rows as f64
                );
            }
        }
        "matrix" => {
            let sql = page_sql();
            let cells: [(&str, bool, bool); 4] = [
                ("judge_all_materialize", false, true),
                ("judge_all_borrow", false, false),
                ("lazy_materialize", true, true),
                ("lazy_borrow", true, false),
            ];
            let mut stats_by_cell = Vec::new();
            for (name, lazy, materialize) in cells {
                let (ranked, stats) = proto_walk(&fixture, &sql, &config, lazy, materialize, None);
                let proto_keyed: Vec<(String, f64)> = ranked
                    .iter()
                    .map(|r| (r.occurrence_id.clone(), r.score))
                    .collect();
                assert_eq!(
                    proto_keyed, reference,
                    "{name} disagrees with the f64 reference"
                );
                assert_eq!(
                    proto_keyed,
                    keyed(&ranking),
                    "{name} disagrees with exhaustive"
                );
                eprintln!("{name} correctness ok: {stats:?}");
                stats_by_cell.push((name, stats));
            }
            // Interleave every cell and the production walk in each round so drift affects all alike.
            let mut samples: Vec<Vec<f64>> = vec![Vec::new(); 5];
            for _ in 0..config.samples {
                for (index, (_, lazy, materialize)) in cells.iter().enumerate() {
                    let start = Instant::now();
                    black_box(proto_walk(
                        &fixture,
                        &sql,
                        &config,
                        *lazy,
                        *materialize,
                        None,
                    ));
                    samples[index].push(ms(start.elapsed()));
                }
                let start = Instant::now();
                black_box(fixture.rank(bounds));
                samples[4].push(ms(start.elapsed()));
            }
            // Rounds are columns of the per-cell sample vectors; each line reports one round across the cells.
            let per_round: Vec<[f64; 5]> = (0..config.samples)
                .map(|round| std::array::from_fn(|cell| samples[cell][round]))
                .collect();
            for (round, at) in per_round.iter().enumerate() {
                println!(
                    "{{\"mode\":\"matrix_round\",\"round\":{round},\"judge_all_materialize\":{:.3},\"judge_all_borrow\":{:.3},\"lazy_materialize\":{:.3},\"lazy_borrow\":{:.3},\"exhaustive\":{:.3}}}",
                    at[0], at[1], at[2], at[3], at[4]
                );
            }
            for (index, (name, lazy, materialize)) in cells.iter().enumerate() {
                let ((_, _), alloc) = count_allocations(|| {
                    proto_walk(&fixture, &sql, &config, *lazy, *materialize, None)
                });
                let stats = stats_by_cell[index].1;
                let mut sorted = samples[index].clone();
                sorted.sort_by(f64::total_cmp);
                println!(
                    "{{\"mode\":\"matrix\",\"cell\":\"{name}\",\"median_ms\":{:.3},\"min_ms\":{:.3},\"max_ms\":{:.3},\"n\":{},\"judged\":{},\"batches\":{},\"allocs\":{},\"bytes\":{}}}",
                    sorted[sorted.len() / 2],
                    sorted[0],
                    sorted[sorted.len() - 1],
                    sorted.len(),
                    stats.judged,
                    stats.batches,
                    alloc.allocs,
                    alloc.bytes
                );
                eprintln!(
                    "{name:>24}: {} judged={} batches={} allocs/row={:.2}",
                    summary(&samples[index]),
                    stats.judged,
                    stats.batches,
                    alloc.allocs as f64 / config.rows as f64
                );
            }
            let mut sorted = samples[4].clone();
            sorted.sort_by(f64::total_cmp);
            println!(
                "{{\"mode\":\"matrix\",\"cell\":\"exhaustive\",\"median_ms\":{:.3},\"min_ms\":{:.3},\"max_ms\":{:.3},\"n\":{}}}",
                sorted[sorted.len() / 2],
                sorted[0],
                sorted[sorted.len() - 1],
                sorted.len()
            );
            eprintln!("{:>24}: {}", "exhaustive", summary(&samples[4]));
        }
        "layered" => {
            let sql = live_sql();
            let layer = ResidentLayer::build(&fixture);
            let layered = rank_resident(&fixture, &layer, &config);
            assert_eq!(
                keyed(&layered),
                reference,
                "rank_layers disagrees with the f64 reference"
            );
            assert_eq!(layered.coverage.with_vector, config.rows);
            eprintln!(
                "rank_layers correctness ok: judged={} batches={} pages={}",
                layered.consumed.judged, layered.consumed.batches, layered.consumed.pages
            );
            let cells: [(&str, bool, bool); 2] = [
                ("layered_judge_all_materialize", false, true),
                ("layered_lazy_borrow", true, false),
            ];
            let mut stats_by_cell = Vec::new();
            for (name, lazy, materialize) in cells {
                let (ranked, stats) =
                    proto_walk(&fixture, &sql, &config, lazy, materialize, Some(&layer));
                let proto_keyed: Vec<(String, f64)> = ranked
                    .iter()
                    .map(|r| (r.occurrence_id.clone(), r.score))
                    .collect();
                assert_eq!(
                    proto_keyed, reference,
                    "{name} disagrees with the f64 reference"
                );
                eprintln!("{name} correctness ok: {stats:?}");
                stats_by_cell.push((name, stats));
            }
            let mut samples: Vec<Vec<f64>> = vec![Vec::new(); 3];
            for _ in 0..config.samples {
                for (index, (_, lazy, materialize)) in cells.iter().enumerate() {
                    let start = Instant::now();
                    black_box(proto_walk(
                        &fixture,
                        &sql,
                        &config,
                        *lazy,
                        *materialize,
                        Some(&layer),
                    ));
                    samples[index].push(ms(start.elapsed()));
                }
                let start = Instant::now();
                black_box(rank_resident(&fixture, &layer, &config));
                samples[2].push(ms(start.elapsed()));
            }
            let (_, base_alloc) = count_allocations(|| rank_resident(&fixture, &layer, &config));
            for (index, (name, lazy, materialize)) in cells.iter().enumerate() {
                let ((_, _), alloc) = count_allocations(|| {
                    proto_walk(&fixture, &sql, &config, *lazy, *materialize, Some(&layer))
                });
                let stats = stats_by_cell[index].1;
                let mut sorted = samples[index].clone();
                sorted.sort_by(f64::total_cmp);
                println!(
                    "{{\"mode\":\"layered\",\"cell\":\"{name}\",\"median_ms\":{:.3},\"min_ms\":{:.3},\"max_ms\":{:.3},\"n\":{},\"judged\":{},\"batches\":{},\"allocs\":{},\"bytes\":{}}}",
                    sorted[sorted.len() / 2],
                    sorted[0],
                    sorted[sorted.len() - 1],
                    sorted.len(),
                    stats.judged,
                    stats.batches,
                    alloc.allocs,
                    alloc.bytes
                );
                eprintln!(
                    "{name:>30}: {} judged={} batches={} allocs/row={:.2}",
                    summary(&samples[index]),
                    stats.judged,
                    stats.batches,
                    alloc.allocs as f64 / config.rows as f64
                );
            }
            let mut sorted = samples[2].clone();
            sorted.sort_by(f64::total_cmp);
            println!(
                "{{\"mode\":\"layered\",\"cell\":\"rank_layers\",\"median_ms\":{:.3},\"min_ms\":{:.3},\"max_ms\":{:.3},\"n\":{},\"allocs\":{},\"bytes\":{}}}",
                sorted[sorted.len() / 2],
                sorted[0],
                sorted[sorted.len() - 1],
                sorted.len(),
                base_alloc.allocs,
                base_alloc.bytes
            );
            eprintln!(
                "{:>30}: {} allocs/row={:.2}",
                "rank_layers",
                summary(&samples[2]),
                base_alloc.allocs as f64 / config.rows as f64
            );
        }
        "lexical" => {
            let scan_rows: usize = std::env::var("EIDNARA_WALK_SCAN_ROWS")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(1024);
            let probes = lexical_probes();
            let bounds = lexical_bounds(scan_rows);
            let run = || {
                fixture
                    .store
                    .with_conn(|conn| {
                        Ok(retrieve(
                            conn,
                            &fixture.kernel,
                            &probes,
                            fixture.authority(),
                            bounds,
                            &EvalBudget::unbounded(),
                        )
                        .unwrap())
                    })
                    .unwrap()
            };
            let result = run();
            let admitted_rows = fixture.admitted.iter().filter(|a| **a).count();
            eprintln!(
                "lexical: probes={} contributions={} completion={:?} consumed={:?}",
                probes.len(),
                result.contributions.len(),
                result.completion,
                result.consumed
            );
            assert_eq!(result.consumed.scanned_rows, scan_rows.min(config.rows));
            assert!(result.contributions.len() <= admitted_rows.min(scan_rows));
            if std::env::var("EIDNARA_WALK_LEXICAL_ORDER").is_ok() {
                fn proc_stat() -> (u64, u64, u64, u64) {
                    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
                    let after = &stat[stat.rfind(')').unwrap() + 2..];
                    let fields: Vec<&str> = after.split_whitespace().collect();
                    // Fields after the command: state is index 0; minflt 7, majflt 9, utime 11, stime 12.
                    (
                        fields[7].parse().unwrap(),
                        fields[9].parse().unwrap(),
                        fields[11].parse().unwrap(),
                        fields[12].parse().unwrap(),
                    )
                }
                // A failing metadata call on a marker path shows up in a syscall trace, so trace segments can be attributed to calls.
                let mark = |label: &str| {
                    let _ = std::fs::metadata(format!("/eidnara-walk-mark-{label}"));
                };
                let heap = || unsafe {
                    (
                        rusqlite::ffi::sqlite3_soft_heap_limit64(-1),
                        rusqlite::ffi::sqlite3_hard_heap_limit64(-1),
                        rusqlite::ffi::sqlite3_memory_used(),
                        rusqlite::ffi::sqlite3_memory_highwater(0),
                    )
                };
                eprintln!("sqlite heap (soft, hard, used, highwater): {:?}", heap());
                let time_scan = |label: &str| {
                    mark(label);
                    let before = proc_stat();
                    let start = Instant::now();
                    let out = fixture
                        .store
                        .with_conn(|conn| {
                            let out = lexical_scan_borrowed(conn, &probes[0], scan_rows);
                            let statement = conn.prepare_cached(PROBE_SQL)?;
                            use rusqlite::StatementStatus as S;
                            eprintln!(
                                "    status: vm_step={} sort={} fullscan={} autoindex={} reprepare={} run={} memused={}",
                                statement.reset_status(S::VmStep),
                                statement.reset_status(S::Sort),
                                statement.reset_status(S::FullscanStep),
                                statement.reset_status(S::AutoIndex),
                                statement.reset_status(S::RePrepare),
                                statement.reset_status(S::Run),
                                statement.get_status(S::MemUsed),
                            );
                            Ok(out)
                        })
                        .unwrap();
                    black_box(out);
                    let after = proc_stat();
                    eprintln!(
                        "{label}: scan {:.1} ms minflt={} majflt={} utime_ticks={} stime_ticks={} heap={:?}",
                        ms(start.elapsed()),
                        after.0 - before.0,
                        after.1 - before.1,
                        after.2 - before.2,
                        after.3 - before.3,
                        heap()
                    );
                };
                let time_owned = |label: &str| {
                    mark(label);
                    let start = Instant::now();
                    let out = fixture
                        .store
                        .with_conn(|conn| {
                            let mut statement = conn.prepare_cached(PROBE_SQL)?;
                            let mut rows =
                                statement.query(params![&probes[0], scan_rows as i64 + 1])?;
                            let mut seen = 0usize;
                            let mut total = 0usize;
                            while let Some(row) = rows.next()? {
                                if seen == scan_rows {
                                    break;
                                }
                                seen += 1;
                                let id: String = row.get(0)?;
                                let _rank: f64 = row.get(1)?;
                                let class: String = row.get(2)?;
                                let object: String = row.get(3)?;
                                let _revision: i64 = row.get(4)?;
                                let digest: String = row.get(5)?;
                                total += id.len() + class.len() + object.len() + digest.len();
                            }
                            Ok((seen, total))
                        })
                        .unwrap();
                    black_box(out);
                    eprintln!("{label}: owned-scan {:.1} ms", ms(start.elapsed()));
                };
                let time_retrieve = |label: &str| {
                    mark(label);
                    let before = proc_stat();
                    let start = Instant::now();
                    black_box(run());
                    let after = proc_stat();
                    eprintln!(
                        "{label}: retrieve {:.1} ms minflt={} majflt={} utime_ticks={} stime_ticks={} heap={:?}",
                        ms(start.elapsed()),
                        after.0 - before.0,
                        after.1 - before.1,
                        after.2 - before.2,
                        after.3 - before.3,
                        heap()
                    );
                };
                let raw = rusqlite::Connection::open(
                    fixture._root.path().join("search").join("search.sqlite"),
                )
                .unwrap();
                let raw_status = |reset: i32| -> (i32, i32, i32, i64) {
                    let mut hit = 0i32;
                    let mut miss = 0i32;
                    let mut used = 0i32;
                    let mut hi = 0i32;
                    unsafe {
                        let db = raw.handle();
                        rusqlite::ffi::sqlite3_db_status(
                            db,
                            rusqlite::ffi::SQLITE_DBSTATUS_CACHE_HIT,
                            &mut hit,
                            &mut hi,
                            reset,
                        );
                        rusqlite::ffi::sqlite3_db_status(
                            db,
                            rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS,
                            &mut miss,
                            &mut hi,
                            reset,
                        );
                        rusqlite::ffi::sqlite3_db_status(
                            db,
                            rusqlite::ffi::SQLITE_DBSTATUS_CACHE_USED,
                            &mut used,
                            &mut hi,
                            0,
                        );
                    }
                    let cache_size: i64 = raw
                        .query_row("PRAGMA cache_size", [], |r| r.get(0))
                        .unwrap();
                    (hit, miss, used, cache_size)
                };
                let time_raw = |label: &str| {
                    mark(label);
                    raw_status(1);
                    let start = Instant::now();
                    let mut statement = raw.prepare_cached(PROBE_SQL).unwrap();
                    let mut rows = statement
                        .query(params![&probes[0], scan_rows as i64 + 1])
                        .unwrap();
                    let mut seen = 0usize;
                    let mut total = 0usize;
                    while let Some(row) = rows.next().unwrap() {
                        if seen == scan_rows {
                            break;
                        }
                        seen += 1;
                        total += row.get_ref(0).unwrap().as_str().unwrap().len();
                    }
                    drop(rows);
                    drop(statement);
                    black_box(total);
                    eprintln!(
                        "{label}: raw-scan {:.1} ms cache (hit, miss, used_bytes, cache_size)={:?}",
                        ms(start.elapsed()),
                        raw_status(0)
                    );
                };
                for i in 0..3 {
                    time_raw(&format!("R{i}"));
                }
                for i in 0..3 {
                    time_scan(&format!("A{i}"));
                }
                for i in 0..3 {
                    time_retrieve(&format!("B{i}"));
                }
                for i in 0..3 {
                    time_scan(&format!("C{i}"));
                }
                for i in 0..3 {
                    time_owned(&format!("D{i}"));
                }
                for i in 0..3 {
                    time_retrieve(&format!("E{i}"));
                }
                for i in 0..3 {
                    time_raw(&format!("S{i}"));
                }
                return;
            }
            let mut retrieve_samples = Vec::new();
            let mut scan_samples = Vec::new();
            for _ in 0..config.samples {
                let start = Instant::now();
                black_box(run());
                retrieve_samples.push(ms(start.elapsed()));
                let start = Instant::now();
                let out = fixture
                    .store
                    .with_conn(|conn| Ok(lexical_scan_borrowed(conn, &probes[0], scan_rows)))
                    .unwrap();
                scan_samples.push(ms(start.elapsed()));
                black_box(out);
            }
            fixture
                .store
                .with_conn(|conn| {
                    let plan: Vec<String> = conn
                        .prepare(&format!("EXPLAIN QUERY PLAN {PROBE_SQL}"))?
                        .query_map(params![&probes[0], scan_rows as i64 + 1], |row| {
                            row.get::<_, String>(3)
                        })?
                        .collect::<rusqlite::Result<_>>()?;
                    eprintln!("probe plan: {plan:#?}");
                    Ok(())
                })
                .unwrap();
            let (_, alloc) = count_allocations(run);
            println!(
                "{{\"mode\":\"lexical\",\"rows\":{},\"scan_rows\":{scan_rows},\"retrieve_median_ms\":{:.3},\"scan_borrowed_median_ms\":{:.3},\"allocs\":{},\"bytes\":{},\"scanned\":{},\"judged\":{}}}",
                config.rows,
                {
                    let mut v = retrieve_samples.clone();
                    v.sort_by(f64::total_cmp);
                    v[v.len() / 2]
                },
                {
                    let mut v = scan_samples.clone();
                    v.sort_by(f64::total_cmp);
                    v[v.len() / 2]
                },
                alloc.allocs,
                alloc.bytes,
                result.consumed.scanned_rows,
                result.consumed.judged
            );
            eprintln!(
                "     retrieve ms: {} {:?}",
                summary(&retrieve_samples),
                retrieve_samples
            );
            eprintln!(
                "scan_borrowed ms: {} {:?}",
                summary(&scan_samples),
                scan_samples
            );
            eprintln!(
                "allocs per scanned row={:.2} bytes per scanned row={:.0}",
                alloc.allocs as f64 / result.consumed.scanned_rows as f64,
                alloc.bytes as f64 / result.consumed.scanned_rows as f64
            );
        }
        other => panic!("unknown EIDNARA_WALK_MODE {other:?}"),
    }
}
