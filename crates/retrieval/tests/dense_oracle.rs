use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, BackupRequest, CommitIntent,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EligibilityVerdict, EventKind,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ScopeSpec, ScopeTermSpec, Sensitivity,
    SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch,
    register_generation,
};
use retrieval::dense::codec::{
    self, ARTIFACT_HEADER_BYTES, ARTIFACT_MAGIC, ARTIFACT_VERSION, ArtifactRejection, Metric,
    RowLayout, RowRejection,
};
use retrieval::dense::export::{ExportRefusal, LiveRows, live_rows};
use retrieval::dense::oracle::exhaustive_with_hook_for_test;
use retrieval::dense::{
    Completion, DenseCoverage, ExhaustiveQuery, ExhaustiveRanking, IncompleteReason, OracleBounds,
    OracleRefusal, Ranked, Window, exhaustive, inner_product, rank_order, rescore,
};
use retrieval::eligibility::{
    Authority, AuthorityMoved, EligibilityReport, authority_moved, judge_occurrences,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity, install_identity,
};
use sha2::{Digest, Sha256};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

const DOMAIN: &str = "domain";
const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE_A: &str = "project:a";
const HOLD: &str = "hold-1";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const COMMIT_OID: &str = "0123456789abcdef0123456789abcdef01234567";
const GENERATION: &str = "gen-1";
const DIMENSION: u32 = 8;
const TOLERANCE: f64 = 1e-3;

fn layout() -> RowLayout {
    RowLayout {
        dimension: DIMENSION,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
    }
}

fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: DIMENSION,
        generation_epoch: 1,
    }
}

/// f64 normalization limits rounding error before fixture rows narrow to f32.
fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

fn axis(index: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[index] = 1.0;
    unit(raw)
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "dense-oracle-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "retrieval".to_string(),
    }
}

fn scope(scope_id: &str, digest: &str) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: scope_id.to_string(),
        source_id: scope_id.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![ScopeTermSpec {
            dimension: Dimension::Project.as_str().to_string(),
            operator: "exact".to_string(),
            exact_value: Some(digest.to_string()),
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
        scope_id: Some(SCOPE_A.to_string()),
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
            reason: "test".to_string(),
        },
    }
}

fn open_store(dir: &Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
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
            max_records: NonZeroUsize::new(4096).unwrap(),
            max_payload_bytes: NonZeroUsize::new(4096).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 20).unwrap(),
        max_local_mutations: NonZeroUsize::new(8192).unwrap(),
        max_pending: NonZeroUsize::new(8192).unwrap(),
    }
}

fn bounds(k: usize) -> OracleBounds {
    OracleBounds {
        k: NonZeroUsize::new(k).unwrap(),
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
    }
}

#[derive(Debug, Clone)]
struct Row {
    class: OccurrenceClass,
    object: String,
    vector: Option<Vec<f32>>,
}

impl Row {
    fn claim(object: &str, vector: Vec<f32>) -> Self {
        Self {
            class: OccurrenceClass::CanonicalClaims,
            object: object.to_string(),
            vector: Some(vector),
        }
    }

    fn identity(&self) -> Vec<(&str, &str)> {
        match self.class {
            OccurrenceClass::GitCommits => vec![
                ("repository_id", "repo"),
                ("object_format", "sha1"),
                ("oid", COMMIT_OID),
            ],
            OccurrenceClass::RawToolSpans => vec![
                ("project_id", "proj-a"),
                ("harness", "pi"),
                ("session_id", "sess-01"),
                ("parent_message_id", "msg-2"),
                ("tool_call_id", "call-1"),
                ("result_revision", "1"),
                ("block_index", "0"),
            ],
            _ => vec![("object_id", self.object.as_str())],
        }
    }

    fn representation(&self) -> &'static str {
        match self.class {
            OccurrenceClass::GitCommits => "commit_message",
            OccurrenceClass::RawToolSpans => "tool_output",
            _ => "decision_summary",
        }
    }

    fn occurrence<'a>(&'a self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: self.class.code(),
            identity,
            revision: "1",
            representation: self.representation(),
            span: None,
        }
    }

    fn occurrence_id(&self) -> String {
        kernel::source_identity::encode_preserving_span(&self.occurrence(&self.identity()))
            .unwrap()
            .occurrence_id
    }
}

/// Against `axis(0)` each score is the row's first coordinate, so the reference order is legible by eye.
fn corpus() -> Vec<Row> {
    vec![
        Row::claim("alpha", unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1])),
        Row::claim("beta", unit([0.7, 0.3, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0])),
        Row::claim("gamma", unit([0.5, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0])),
        Row::claim("delta", unit([0.3, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0])),
        // `epsilon` and `eta` carry identical vectors, so only identifier bytes can order them.
        Row::claim("epsilon", unit([0.4, 0.4, 0.4, 0.4, 0.0, 0.0, 0.0, 0.0])),
        Row::claim("eta", unit([0.4, 0.4, 0.4, 0.4, 0.0, 0.0, 0.0, 0.0])),
        Row::claim("theta", unit([-0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0])),
        Row {
            class: OccurrenceClass::GitCommits,
            object: "zeta".to_string(),
            vector: Some(unit([0.1, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0])),
        },
        Row {
            class: OccurrenceClass::RawToolSpans,
            object: "tool".to_string(),
            vector: None,
        },
    ]
}

const OBJECTS: [&str; 9] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "eta", "theta", "zeta", "tool",
];

struct Fixture {
    root: tempfile::TempDir,
    kernel: KernelStore,
    store: SqliteStore,
    incarnation: String,
    project: ProjectScope,
    rows: Vec<Row>,
}

impl Fixture {
    fn new(admitted: &[&str], rows: Vec<Row>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path().join("kernel")).unwrap();
        kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(scope(SCOPE_A, PROJECT_A))?;
                for object in OBJECTS {
                    envelope.insert_decision(decision(object))?;
                    if admitted.contains(&object) {
                        envelope.record_admission(admission(object))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        let incarnation = kernel
            .database_incarnation_id_within_budget(&EvalBudget::unbounded())
            .unwrap();
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
                        vector_dimension: DIMENSION,
                        generation_epoch: 1,
                    },
                    1,
                )
                .unwrap();
                register_generation(conn, &generation(), 1).unwrap();
                Ok(())
            })
            .unwrap();
        let fixture = Self {
            root,
            kernel,
            store,
            incarnation,
            project: ProjectScope::new(PROJECT_A).unwrap(),
            rows,
        };
        fixture.project();
        fixture
    }

    fn all_admitted() -> Self {
        Self::new(&OBJECTS, corpus())
    }

    /// Projects every row with a queued embedding job, then completes the jobs whose rows carry a vector.
    fn project(&self) {
        let through = self.kernel.tip().unwrap();
        let texts: Vec<String> = self
            .rows
            .iter()
            .map(|row| format!("text {}", row.object))
            .collect();
        let identities: Vec<Vec<(&str, &str)>> = self.rows.iter().map(Row::identity).collect();
        let records: Vec<OccurrenceRecord<'_>> = self
            .rows
            .iter()
            .zip(&identities)
            .zip(&texts)
            .map(|((row, identity), text)| OccurrenceRecord {
                occurrence: row.occurrence(identity),
                payload: Payload::Whole(text),
                domain_id: DOMAIN,
                sensitivity: Sensitivity::Normal,
                source_object_id: &row.object,
                source_evidence_id: "evidence",
                source_artifact_digest: DIGEST,
                created_commit_seq: through,
            })
            .collect();
        let batch = ProjectionBatch {
            identity: MutationIdentity {
                kernel_incarnation_id: self.incarnation.clone(),
                hold_id: HOLD.to_string(),
                snapshot_commit_seq: 0,
                through_commit_seq: through,
            },
            records,
            invalidations: vec![],
            generation_id: Some(GENERATION),
        };
        self.store
            .with_conn_fenced(|conn| {
                apply_batch(conn, &batch, batch_bounds(), through).unwrap();
                for row in &self.rows {
                    if let Some(vector) = &row.vector {
                        conn.execute(
                            "INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,?4,1,1,1)",
                            rusqlite::params![row.occurrence_id(), GENERATION, codec::encode(vector), DIMENSION],
                        )?;
                        conn.execute(
                            "UPDATE embedding_jobs SET state='embedded' WHERE occurrence_id=?1",
                            [row.occurrence_id()],
                        )?;
                    }
                }
                Ok(())
            })
            .unwrap();
    }

    fn id(&self, object: &str) -> String {
        self.row(object).occurrence_id()
    }

    fn row(&self, object: &str) -> &Row {
        self.rows.iter().find(|row| row.object == object).unwrap()
    }

    fn retire(&self, object: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object}")), |envelope| {
                envelope.retire_decision(object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Snapshots the kernel so a later `restore` replaces its incarnation.
    fn backup(&self) -> std::path::PathBuf {
        let backup_dir = tempfile::tempdir().unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(backup_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let manifest = self
            .kernel
            .backup(BackupRequest {
                destination_directory: backup_dir.path().to_path_buf(),
                deadline: Instant::now() + Duration::from_secs(10),
                capture_pin_expires_at: None,
            })
            .unwrap();
        // The directory outlives the fixture; restore reads it after this function returns.
        let _ = backup_dir.keep();
        manifest.destination_path
    }

    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("search").join("search.sqlite")).unwrap()
    }

    fn drop_vector(&self, object: &str) {
        let changed = self
            .raw()
            .execute(
                "DELETE FROM occurrence_vectors WHERE occurrence_id=?1",
                [self.id(object)],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    fn job_state(&self, object: &str, state: &str) {
        let changed = self
            .raw()
            .execute(
                "UPDATE embedding_jobs SET state=?2 WHERE occurrence_id=?1",
                rusqlite::params![self.id(object), state],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    fn authority(&self) -> Authority<'_> {
        Authority {
            project: &self.project,
            destination: ArtifactDestination::Local,
        }
    }

    fn query<'a>(
        &'a self,
        query: &'a [f32],
        generation: &'a VectorGeneration,
        bounds: OracleBounds,
    ) -> ExhaustiveQuery<'a> {
        ExhaustiveQuery {
            generation,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            query,
            authority: self.authority(),
            bounds,
        }
    }

    fn rank(
        &self,
        query: &[f32],
        bounds: OracleBounds,
        budget: &EvalBudget,
    ) -> Result<ExhaustiveRanking, OracleRefusal> {
        let generation = generation();
        self.store
            .with_conn(|conn| {
                Ok(exhaustive(
                    conn,
                    &self.kernel,
                    &self.query(query, &generation, bounds),
                    budget,
                ))
            })
            .unwrap()
    }

    fn rank_with_hook(
        &self,
        query: &[f32],
        bounds: OracleBounds,
        budget: &EvalBudget,
        hook: impl FnMut(Window<'_>),
    ) -> Result<ExhaustiveRanking, OracleRefusal> {
        let generation = generation();
        self.store
            .with_conn(|conn| {
                Ok(exhaustive_with_hook_for_test(
                    conn,
                    &self.kernel,
                    &self.query(query, &generation, bounds),
                    budget,
                    hook,
                ))
            })
            .unwrap()
    }

    fn rank_recording(
        &self,
        query: &[f32],
        bounds: OracleBounds,
    ) -> (ExhaustiveRanking, Vec<String>) {
        let mut visited = Vec::new();
        let ranking = self
            .rank_with_hook(query, bounds, &EvalBudget::unbounded(), |window| {
                if let Window::Visited(id) = window {
                    visited.push(id.to_string());
                }
            })
            .unwrap();
        (ranking, visited)
    }

    /// The independent expectation: every dense-required row with a vector among `objects`, scored in f64 coordinate order and sorted by score descending then identifier ascending.
    fn reference(&self, query: &[f32], objects: &[&str]) -> Vec<(String, f64)> {
        let mut scored: Vec<(String, f64)> = self
            .rows
            .iter()
            .filter(|row| objects.contains(&row.object.as_str()))
            .filter(|row| row.class != OccurrenceClass::RawToolSpans)
            .filter_map(|row| {
                let vector = row.vector.as_ref()?;
                let mut score = 0.0f64;
                for j in 0..vector.len() {
                    score += f64::from(query[j]) * f64::from(vector[j]);
                }
                Some((row.occurrence_id(), score))
            })
            .collect();
        scored.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
        scored
    }

    fn dense_ids(&self) -> BTreeSet<String> {
        self.rows
            .iter()
            .filter(|row| row.class != OccurrenceClass::RawToolSpans)
            .map(Row::occurrence_id)
            .collect()
    }
}

fn keyed(ranking: &ExhaustiveRanking) -> Vec<(String, f64)> {
    ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

fn ids_of(ranking: &ExhaustiveRanking) -> Vec<String> {
    ranking
        .ranked
        .iter()
        .map(|row| row.occurrence_id.clone())
        .collect()
}

fn assert_visited_once(fixture: &Fixture, visited: &[String]) {
    let unique: BTreeSet<String> = visited.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        visited.len(),
        "a row was visited twice: {visited:?}"
    );
    assert_eq!(unique, fixture.dense_ids());
}

#[test]
fn a_population_larger_than_k_yields_the_reference_prefix_and_visits_every_required_row_once() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let reference = fixture.reference(&query, &OBJECTS);
    assert_eq!(reference.len(), 8);

    let (ranking, visited) = fixture.rank_recording(&query, bounds(3));
    assert_eq!(ranking.completion, Completion::Complete);
    assert_eq!(keyed(&ranking), reference[..3]);
    assert_visited_once(&fixture, &visited);
    let mut by_id = visited.clone();
    by_id.sort();
    assert_eq!(
        visited, by_id,
        "the walk visits rows in identifier order across classes"
    );
    // Against a unit axis the score is exactly the row's first coordinate widened to f64.
    for row in &ranking.ranked {
        let source = fixture
            .rows
            .iter()
            .find(|r| r.occurrence_id() == row.occurrence_id)
            .unwrap();
        assert_eq!(
            row.score.to_bits(),
            f64::from(source.vector.as_ref().unwrap()[0]).to_bits()
        );
    }
    let alpha = unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1])[0];
    assert_eq!(ranking.ranked[0].occurrence_id, fixture.id("alpha"));
    assert_eq!(ranking.ranked[0].score, f64::from(alpha));
    assert_eq!(
        ranking.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 8,
            missing_pending: 0,
            missing_without_pending: 0,
        }
    );
    assert!(ranking.consumed.pages >= 4, "{:?}", ranking.consumed);
    assert_eq!(ranking.consumed.judged, 8 + 3);
    assert_eq!(ranking.consumed.batches, ranking.consumed.pages + 1);
    assert!(ranking.consumed.excluded.is_empty());
    assert!(ranking.snapshot.is_some());
    for row in &ranking.ranked {
        let expected = if row.occurrence_id == fixture.id("zeta") {
            OccurrenceClass::GitCommits
        } else {
            OccurrenceClass::CanonicalClaims
        };
        assert_eq!(row.class, expected);
    }
}

#[test]
fn exactly_k_and_fewer_than_k_populations_return_every_eligible_row_in_reference_order() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let reference = fixture.reference(&query, &OBJECTS);

    let exact = fixture
        .rank(&query, bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(exact.completion, Completion::Complete);
    assert_eq!(keyed(&exact), reference);

    let roomy = fixture
        .rank(&query, bounds(64), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(roomy.completion, Completion::Complete);
    assert_eq!(keyed(&roomy), reference);
    assert_eq!(roomy.ranked.len(), 8);
}

#[test]
fn a_zero_population_is_a_complete_empty_ranking_that_ran_no_batch() {
    let tool_only = Fixture::new(
        &OBJECTS,
        corpus()
            .into_iter()
            .filter(|row| row.class == OccurrenceClass::RawToolSpans)
            .collect(),
    );
    let (ranking, visited) = tool_only.rank_recording(&axis(0), bounds(3));
    assert_eq!(ranking.completion, Completion::Complete);
    assert!(ranking.ranked.is_empty());
    assert!(visited.is_empty());
    assert_eq!(ranking.coverage, DenseCoverage::default());
    assert_eq!(ranking.consumed.pages, 0);
    assert_eq!(ranking.consumed.batches, 0);
    assert_eq!(ranking.snapshot, None);

    let empty = Fixture::new(&OBJECTS, Vec::new());
    let ranking = empty
        .rank(&axis(0), bounds(3), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ranking.completion, Completion::Complete);
    assert!(ranking.ranked.is_empty());
}

#[test]
fn tied_scores_order_by_identifier_bytes_ascending_and_k_cuts_the_tie_deterministically() {
    let fixture = Fixture::all_admitted();
    let (epsilon, eta) = (fixture.id("epsilon"), fixture.id("eta"));
    let (first, second) = if epsilon < eta {
        (epsilon.clone(), eta.clone())
    } else {
        (eta.clone(), epsilon.clone())
    };
    // A query along the tied pair's direction scores both 1.0 and everything else lower.
    let query = fixture.row("epsilon").vector.clone().unwrap();
    assert_eq!(query, fixture.row("eta").vector.clone().unwrap());

    let both = fixture
        .rank(&query, bounds(2), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&both), vec![first.clone(), second]);
    assert_eq!(both.ranked[0].score, both.ranked[1].score);

    let one = fixture
        .rank(&query, bounds(1), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&one), vec![first]);
    assert_eq!(one.completion, Completion::Complete);

    // The reverse-ID insertion order of the fixture does not leak into the ranking.
    let reversed = Fixture::new(&OBJECTS, corpus().into_iter().rev().collect());
    let again = reversed
        .rank(&query, bounds(2), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(keyed(&again), keyed(&both));

    // A tie across classes: the git commit shares its vector with one claim. Whichever identifier is
    // lower wins `k = 1`, so this holds regardless of how the identifiers hash.
    let mut rows = corpus();
    let zeta = rows.iter().position(|row| row.object == "zeta").unwrap();
    rows[zeta].vector = Some(unit([0.3, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0]));
    let cross = Fixture::new(&OBJECTS, rows);
    let (low, high) = {
        let (d, z) = (cross.id("delta"), cross.id("zeta"));
        if d < z { (d, z) } else { (z, d) }
    };
    let query = cross.row("delta").vector.clone().unwrap();
    let pair = cross
        .rank(&query, bounds(2), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&pair), vec![low.clone(), high]);
    assert_eq!(pair.ranked[0].score, pair.ranked[1].score);
    let one = cross
        .rank(&query, bounds(1), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ids_of(&one), vec![low]);
}

#[test]
fn a_higher_scoring_excluded_row_never_displaces_an_eligible_one_and_stays_a_policy_exclusion() {
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "alpha")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    let query = axis(0);
    let reference = fixture.reference(&query, &admitted);
    assert_eq!(
        fixture.reference(&query, &OBJECTS)[0].0,
        fixture.id("alpha")
    );

    let ranking = fixture
        .rank(&query, bounds(3), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(ranking.completion, Completion::Complete);
    assert_eq!(keyed(&ranking), reference[..3]);
    assert!(!ids_of(&ranking).contains(&fixture.id("alpha")));
    assert_eq!(
        ranking.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)]
    );
    assert_eq!(
        ranking.coverage.with_vector, 8,
        "exclusion is not a coverage shortfall"
    );
    assert_eq!(ranking.coverage.missing(), 0);
}

#[test]
fn a_missing_required_vector_is_a_coverage_shortfall_and_never_a_complete_result() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    fixture.drop_vector("beta");
    fixture.job_state("beta", "pending");
    let without_beta: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "beta")
        .collect();

    let pending = fixture
        .rank(&query, bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        pending.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(keyed(&pending), fixture.reference(&query, &without_beta));
    assert_eq!(
        pending.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 7,
            missing_pending: 1,
            missing_without_pending: 0
        }
    );
    assert!(
        pending.consumed.excluded.is_empty(),
        "a shortfall is not an exclusion"
    );
    assert_eq!(pending.consumed.judged, 7 + 7);

    fixture.job_state("beta", "failed");
    let stalled = fixture
        .rank(&query, bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        stalled.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(
        stalled.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 7,
            missing_pending: 0,
            missing_without_pending: 1
        }
    );

    // Lexical presence is untouched: the row is live and indexed even though it has no vector.
    let present: bool = fixture
        .raw()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM lexical WHERE occurrence_id=?1)",
            [fixture.id("beta")],
            |row| row.get(0),
        )
        .unwrap();
    assert!(present);

    // A tombstoned row leaves the population: neither required nor missing.
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id, invalidated_commit_seq, reason, recorded_at) VALUES (?1, 99, 'retired', 0)",
            [fixture.id("beta")],
        )
        .unwrap();
    let (complete, visited) = fixture.rank_recording(&query, bounds(8));
    assert_eq!(complete.completion, Completion::Complete);
    assert_eq!(complete.coverage.required, 7);
    assert!(!visited.contains(&fixture.id("beta")));
}

#[test]
fn a_missing_vector_and_an_exclusion_on_different_rows_stay_distinct() {
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "gamma")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    fixture.drop_vector("delta");
    fixture.job_state("delta", "pending");
    let ranking = fixture
        .rank(&axis(0), bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(ranking.coverage.missing_pending, 1);
    assert_eq!(
        ranking.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)]
    );
    assert_eq!(ranking.ranked.len(), 6);
}

#[test]
fn a_missing_vector_on_an_excluded_row_is_a_shortfall_and_the_row_is_never_judged() {
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "gamma")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    fixture.drop_vector("gamma");
    fixture.job_state("gamma", "pending");
    let ranking = fixture
        .rank(&axis(0), bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(ranking.coverage.missing_pending, 1);
    assert!(
        ranking.consumed.excluded.is_empty(),
        "a row without a vector is not judged"
    );
    assert_eq!(ranking.consumed.judged, 7 + 7);
    assert_eq!(ranking.ranked.len(), 7);
}

#[test]
fn a_walk_that_stops_early_names_its_stop_over_a_coverage_shortfall() {
    let fixture = Fixture::all_admitted();
    fixture.drop_vector("alpha");
    let query = axis(0);

    let budget = EvalBudget::unbounded();
    let cancelled = fixture
        .rank_with_hook(&query, bounds(3), &budget, |window| {
            if window == Window::BeforeRevalidation {
                budget.cancel();
            }
        })
        .unwrap();
    assert_eq!(
        cancelled.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(cancelled.ranked.is_empty());
    assert_eq!(
        cancelled.coverage.missing(),
        1,
        "the shortfall stays visible in coverage"
    );

    let capped = OracleBounds {
        max_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds(3)
    };
    let bounded = fixture
        .rank(&query, capped, &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(
        bounded.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );

    let moved = fixture
        .rank_with_hook(&query, bounds(3), &EvalBudget::unbounded(), |window| {
            if window == Window::BeforeRevalidation {
                fixture.retire("theta");
            }
        })
        .unwrap();
    assert_eq!(
        moved.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
}

#[test]
fn a_stored_row_that_fails_the_layout_refuses_the_request_before_its_page_is_scored() {
    // The schema's CHECK keeps `vector_dimension * 4 == length(vector)`, so a truncated word cannot be stored; the codec test covers it.
    let cases: [(&str, u32, Vec<u8>, RowRejection); 4] = [
        (
            "dimension",
            7,
            codec::encode(&unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])[..7]),
            RowRejection::Dimension {
                expected: 8,
                actual: 7,
            },
        ),
        (
            "nan",
            8,
            codec::encode(&[0.0, 0.0, f32::NAN, 0.0, 0.0, 0.0, 0.0, 1.0]),
            RowRejection::NonFinite { coordinate: 2 },
        ),
        ("zero", 8, codec::encode(&[0.0; 8]), RowRejection::ZeroNorm),
        (
            "norm",
            8,
            codec::encode(&[0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            RowRejection::Normalization {
                norm: 0.5f64.hypot(0.5),
            },
        ),
    ];
    for (name, dimension, bytes, expected) in cases {
        let fixture = Fixture::all_admitted();
        // The last visited row is the corrupt one, so the walk reaches the final page before refusing.
        let victim = fixture.dense_ids().into_iter().next_back().unwrap();
        let mut windows: Vec<String> = Vec::new();
        fixture
            .raw()
            .execute(
                "UPDATE occurrence_vectors SET vector=?2, vector_dimension=?3 WHERE occurrence_id=?1",
                rusqlite::params![victim, bytes, dimension],
            )
            .unwrap();
        let outcome = fixture.rank_with_hook(
            &axis(0),
            OracleBounds {
                page_rows: NonZeroUsize::new(3).unwrap(),
                ..bounds(3)
            },
            &EvalBudget::unbounded(),
            |window| windows.push(format!("{window:?}")),
        );
        assert_eq!(
            outcome,
            Err(OracleRefusal::StoredRow {
                occurrence_id: victim.clone(),
                rejection: expected
            }),
            "{name}"
        );
        assert_eq!(
            windows.last().unwrap(),
            &format!("Visited({victim:?})"),
            "{name}"
        );
        assert_eq!(
            windows
                .iter()
                .filter(|w| w.starts_with("AfterPage"))
                .count(),
            2,
            "{name}: two full pages were scored before the third refused: {windows:?}"
        );
    }
}

#[test]
fn an_infinite_stored_coordinate_is_refused() {
    let fixture = Fixture::all_admitted();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
            rusqlite::params![
                fixture.id("zeta"),
                codec::encode(&[f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            ],
        )
        .unwrap();
    assert_eq!(
        fixture.rank(&axis(0), bounds(3), &EvalBudget::unbounded()),
        Err(OracleRefusal::StoredRow {
            occurrence_id: fixture.id("zeta"),
            rejection: RowRejection::NonFinite { coordinate: 0 }
        })
    );
}

#[test]
fn an_invalid_query_or_layout_is_refused_before_the_projection_is_read() {
    let fixture = Fixture::all_admitted();
    let generation = generation();
    for (query, expected) in [
        (
            vec![1.0f32; 7],
            RowRejection::Dimension {
                expected: 8,
                actual: 7,
            },
        ),
        (vec![f32::NAN; 8], RowRejection::NonFinite { coordinate: 0 }),
        (vec![0.0f32; 8], RowRejection::ZeroNorm),
        (
            vec![1.0f32; 8],
            RowRejection::Normalization {
                norm: 8.0f64.sqrt(),
            },
        ),
    ] {
        assert_eq!(
            fixture.rank(&query, bounds(3), &EvalBudget::unbounded()),
            Err(OracleRefusal::Query(expected))
        );
    }
    let query = axis(0);
    for tolerance in [f64::NAN, f64::INFINITY, -1e-3] {
        let refused = fixture
            .store
            .with_conn(|conn| {
                let mut request = fixture.query(&query, &generation, bounds(3));
                request.unit_norm_tolerance = tolerance;
                Ok(exhaustive(
                    conn,
                    &fixture.kernel,
                    &request,
                    &EvalBudget::unbounded(),
                ))
            })
            .unwrap();
        assert!(
            matches!(
                refused,
                Err(OracleRefusal::Query(RowRejection::Tolerance { .. }))
            ),
            "{tolerance}: {refused:?}"
        );
    }

    let foreign = VectorGeneration {
        generation_id: "gen-9".to_string(),
        ..generation.clone()
    };
    let unknown = fixture
        .store
        .with_conn(|conn| {
            Ok(exhaustive(
                conn,
                &fixture.kernel,
                &fixture.query(&axis(0), &foreign, bounds(3)),
                &EvalBudget::unbounded(),
            ))
        })
        .unwrap();
    assert!(matches!(
        unknown,
        Err(OracleRefusal::Projection(
            ProjectionError::UnknownGeneration { .. }
        ))
    ));

    for (bound, over) in [
        (
            "k",
            OracleBounds {
                k: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES + 1).unwrap(),
                ..bounds(3)
            },
        ),
        (
            "page_rows",
            OracleBounds {
                page_rows: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES + 1).unwrap(),
                ..bounds(3)
            },
        ),
    ] {
        assert_eq!(
            fixture.rank(&axis(0), over, &EvalBudget::unbounded()),
            Err(OracleRefusal::BatchOverBound {
                bound,
                value: MAX_ELIGIBILITY_CANDIDATES + 1
            })
        );
    }
}

#[test]
fn a_snapshot_that_moves_between_pages_stops_the_walk_without_a_complete_result() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let mut visited: Vec<String> = Vec::new();
    let ranking = fixture
        .rank_with_hook(
            &query,
            bounds(8),
            &EvalBudget::unbounded(),
            |window| match window {
                Window::Visited(id) => visited.push(id.to_string()),
                Window::AfterPage(1) => fixture.retire("theta"),
                _ => {}
            },
        )
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    // Only the first page was admitted; its two rows are re-judged and returned in rank order.
    let mut first_page: Vec<(String, f64)> = fixture
        .reference(&query, &OBJECTS)
        .into_iter()
        .filter(|(id, _)| visited[..2].contains(id))
        .collect();
    first_page.sort_by(|(a_id, a), (b_id, b)| b.total_cmp(a).then_with(|| a_id.cmp(b_id)));
    assert_eq!(keyed(&ranking), first_page);
    assert_eq!(
        ranking.consumed.batches, 3,
        "two pages judged plus one re-judgment"
    );
}

#[test]
fn a_kernel_restore_between_pages_stops_the_walk_with_the_incarnation_change() {
    let fixture = Fixture::all_admitted();
    let manifest = fixture.backup();
    let ranking = fixture
        .rank_with_hook(&axis(0), bounds(8), &EvalBudget::unbounded(), |window| {
            if window == Window::AfterPage(1) {
                fixture.kernel.restore(&manifest).unwrap();
            }
        })
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::KernelIncarnationChanged)
    );
    assert_eq!(ranking.consumed.batches, 3);
    assert_eq!(
        ranking.coverage.required, 4,
        "the second page was visited before its verdicts were discarded"
    );
}

#[test]
fn authority_moved_reports_an_incarnation_change_before_a_snapshot_change() {
    let fixture = Fixture::all_admitted();
    let candidates = fixture
        .store
        .with_conn(|conn| {
            Ok(
                retrieval::eligibility::live_candidates(conn, None, NonZeroUsize::new(64).unwrap())
                    .unwrap(),
            )
        })
        .unwrap();
    let judge = |kernel: &KernelStore| -> EligibilityReport {
        judge_occurrences(
            kernel,
            &fixture.project,
            ArtifactDestination::Local,
            &candidates,
        )
        .unwrap()
    };
    let first = judge(&fixture.kernel);
    assert_eq!(authority_moved(None, None, &first), None);
    assert_eq!(
        authority_moved(Some(&first.snapshot), Some(&first.incarnation), &first),
        None
    );

    fixture.retire("theta");
    let after_commit = judge(&fixture.kernel);
    assert_ne!(after_commit.snapshot, first.snapshot);
    assert_eq!(after_commit.incarnation, first.incarnation);
    assert_eq!(
        authority_moved(
            Some(&first.snapshot),
            Some(&first.incarnation),
            &after_commit
        ),
        Some(AuthorityMoved::Snapshot)
    );

    let manifest = fixture.backup();
    fixture.retire("delta");
    fixture.kernel.restore(&manifest).unwrap();
    let after_restore = judge(&fixture.kernel);
    assert_ne!(after_restore.incarnation, first.incarnation);
    assert_eq!(
        authority_moved(
            Some(&first.snapshot),
            Some(&first.incarnation),
            &after_restore
        ),
        Some(AuthorityMoved::Incarnation),
        "both moved; the incarnation wins"
    );
    assert_eq!(
        authority_moved(
            Some(&after_restore.snapshot),
            Some(&first.incarnation),
            &after_restore
        ),
        Some(AuthorityMoved::Incarnation)
    );
}

#[test]
fn revalidation_drops_a_row_retired_after_admission_and_marks_the_snapshot_change() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let reference = fixture.reference(&query, &OBJECTS);
    let ranking = fixture
        .rank_with_hook(&query, bounds(3), &EvalBudget::unbounded(), |window| {
            if window == Window::BeforeRevalidation {
                fixture.retire("alpha");
            }
        })
        .unwrap();
    assert_eq!(reference[0].0, fixture.id("alpha"));
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(keyed(&ranking), reference[1..3]);
    assert_eq!(
        ranking.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 1)]
    );
}

#[test]
fn a_kernel_restore_before_revalidation_marks_the_incarnation_change() {
    let fixture = Fixture::all_admitted();
    let manifest = fixture.backup();
    let ranking = fixture
        .rank_with_hook(&axis(0), bounds(3), &EvalBudget::unbounded(), |window| {
            if window == Window::BeforeRevalidation {
                fixture.kernel.restore(&manifest).unwrap();
            }
        })
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::KernelIncarnationChanged)
    );
    assert_eq!(ranking.ranked.len(), 3);
}

#[test]
fn a_budget_that_ends_before_the_first_page_refuses_and_one_that_ends_later_is_incomplete_with_no_rows()
 {
    let fixture = Fixture::all_admitted();
    let query = axis(0);

    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        fixture.rank(&query, bounds(3), &cancelled),
        Err(OracleRefusal::BudgetExhausted)
    );

    let budget = EvalBudget::unbounded();
    let after_page = fixture
        .rank_with_hook(&query, bounds(3), &budget, |window| {
            if window == Window::AfterPage(1) {
                budget.cancel();
            }
        })
        .unwrap();
    assert_eq!(
        after_page.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(after_page.ranked.is_empty());
    assert_eq!(after_page.consumed.pages, 1);

    let budget = EvalBudget::unbounded();
    let before_revalidation = fixture
        .rank_with_hook(&query, bounds(3), &budget, |window| {
            if window == Window::BeforeRevalidation {
                budget.cancel();
            }
        })
        .unwrap();
    assert_eq!(
        before_revalidation.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(before_revalidation.ranked.is_empty());
    assert_eq!(before_revalidation.coverage.required, 8);
}

#[test]
fn a_deadline_that_expires_mid_walk_never_yields_a_complete_result() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let started = Instant::now();
    fixture
        .rank(&query, bounds(3), &EvalBudget::unbounded())
        .unwrap();
    let attempt = started.elapsed().max(Duration::from_micros(64));
    let mut outcomes = std::collections::BTreeSet::new();
    for step in 0..32u32 {
        let deadline = Instant::now() + attempt.mul_f64(f64::from(step) / 16.0);
        let budget = EvalBudget::new(Some(deadline), Arc::new(AtomicBool::new(false)));
        let outcome = fixture.rank(&query, bounds(3), &budget);
        let label = match &outcome {
            Err(OracleRefusal::BudgetExhausted) => "refused",
            Ok(ranking) if ranking.completion == Completion::Complete => {
                assert_eq!(keyed(ranking), fixture.reference(&query, &OBJECTS)[..3]);
                "complete"
            }
            Ok(ranking) => {
                assert_eq!(
                    ranking.completion,
                    Completion::Incomplete(IncompleteReason::BudgetExhausted),
                    "{ranking:?}"
                );
                assert!(ranking.ranked.is_empty(), "{ranking:?}");
                "incomplete"
            }
            Err(other) => panic!("{other:?}"),
        };
        outcomes.insert(label);
    }
    assert!(outcomes.contains("refused"), "{outcomes:?}");
    assert!(outcomes.contains("complete"), "{outcomes:?}");
}

#[test]
fn the_row_bound_stops_the_walk_as_incomplete_and_a_bound_at_the_population_stays_complete() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let capped = OracleBounds {
        max_rows: NonZeroUsize::new(5).unwrap(),
        ..bounds(8)
    };
    let (ranking, visited) = fixture.rank_recording(&query, capped);
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_eq!(visited.len(), 5);
    assert_eq!(ranking.coverage.required, 5);
    assert!(ranking.ranked.len() <= 5);

    let exact = OracleBounds {
        max_rows: NonZeroUsize::new(8).unwrap(),
        ..bounds(8)
    };
    let (ranking, visited) = fixture.rank_recording(&query, exact);
    assert_eq!(ranking.completion, Completion::Complete);
    assert_visited_once(&fixture, &visited);

    // A bound that fills exactly at a page edge with rows remaining is still incomplete: the
    // remainder probe reads one more row without visiting it.
    let at_page_edge = OracleBounds {
        page_rows: NonZeroUsize::new(4).unwrap(),
        max_rows: NonZeroUsize::new(4).unwrap(),
        ..bounds(8)
    };
    let (ranking, visited) = fixture.rank_recording(&query, at_page_edge);
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_eq!(visited.len(), 4);
    assert_eq!(ranking.consumed.pages, 1);
}

#[test]
fn page_size_changes_the_batch_count_but_not_the_ranking() {
    let fixture = Fixture::all_admitted();
    let query = unit([0.3, -0.2, 0.5, 0.1, 0.4, -0.6, 0.2, 0.1]);
    let reference = fixture.reference(&query, &OBJECTS);
    let mut previous: Option<Vec<(String, f64)>> = None;
    for page_rows in [1usize, 2, 3, 8, MAX_ELIGIBILITY_CANDIDATES] {
        let bounds = OracleBounds {
            page_rows: NonZeroUsize::new(page_rows).unwrap(),
            ..bounds(4)
        };
        let (ranking, visited) = fixture.rank_recording(&query, bounds);
        assert_eq!(
            ranking.completion,
            Completion::Complete,
            "page_rows={page_rows}"
        );
        assert_eq!(keyed(&ranking), reference[..4], "page_rows={page_rows}");
        assert_visited_once(&fixture, &visited);
        assert_eq!(ranking.consumed.batches, ranking.consumed.pages + 1);
        if let Some(previous) = &previous {
            assert_eq!(&keyed(&ranking), previous);
        }
        previous = Some(keyed(&ranking));
    }
}

#[test]
fn ranking_reads_no_payload_bytes() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let before = fixture
        .rank(&query, bounds(4), &EvalBudget::unbounded())
        .unwrap();
    let changed = fixture
        .raw()
        .execute("UPDATE payloads SET bytes=zeroblob(byte_length)", [])
        .unwrap();
    assert_eq!(changed, fixture.rows.len());
    let after = fixture
        .rank(&query, bounds(4), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn rescore_over_retained_rows_agrees_with_the_exhaustive_ranking() {
    let fixture = Fixture::all_admitted();
    let query = unit([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
    let ranking = fixture
        .rank(&query, bounds(8), &EvalBudget::unbounded())
        .unwrap();
    let rows: Vec<(String, OccurrenceClass, Vec<f32>)> = fixture
        .rows
        .iter()
        .filter_map(|row| Some((row.occurrence_id(), row.class, row.vector.clone()?)))
        .collect();
    let rescored: Vec<Ranked> = rescore(
        &layout(),
        &query,
        rows.iter()
            .map(|(id, class, vector)| (id.as_str(), *class, vector.as_slice())),
    )
    .unwrap();
    assert_eq!(rescored, ranking.ranked);

    let short = [1.0f32; 7];
    assert_eq!(
        rescore(
            &layout(),
            &query,
            [("short", OccurrenceClass::Messages, short.as_slice())]
        ),
        Err(RowRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
    assert_eq!(
        rescore(&layout(), &short, std::iter::empty()),
        Err(RowRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
}

#[test]
fn products_accumulate_in_f64_in_increasing_coordinate_order() {
    // 1e16 is exact in f32. Forward: (1e16 + 1) rounds to 1e16, then -1e16 cancels to 0, then +1 = 1.
    // Reverse: 1 - 1e16 = -1e16 exactly, +1 stays -1e16, +1e16 = 0. Only the increasing order yields 1.0.
    let query = [1.0e16f32, 1.0, -1.0e16, 1.0, 0.0, 0.0, 0.0, 0.0];
    let row = [1.0f32; 8];
    assert_eq!(inner_product(&query, &row), 1.0);
    let reversed: f64 = query
        .iter()
        .zip(row)
        .rev()
        .fold(0.0f64, |sum, (q, r)| sum + f64::from(*q) * f64::from(r));
    assert_eq!(reversed, 0.0);
    // Summing in f32 loses the trailing ones entirely.
    let query32 = [1.0e8f32, 1.0, -1.0e8, 1.0, 0.0, 0.0, 0.0, 0.0];
    let narrow: f32 = query32.iter().zip(row).map(|(q, r)| q * r).sum();
    assert_ne!(f64::from(narrow), inner_product(&query32, &row));
    assert_eq!(inner_product(&query32, &row), 2.0);
}

#[test]
#[should_panic(expected = "rows of one layout have one length")]
fn inner_product_refuses_unequal_lengths_instead_of_truncating() {
    let _ = inner_product(&[1.0; 8], &[1.0; 7]);
}

#[test]
fn rank_order_is_score_descending_then_identifier_bytes_ascending_with_no_epsilon() {
    use std::cmp::Ordering;
    assert_eq!(rank_order((0.5, "b"), (0.4, "a")), Ordering::Less);
    assert_eq!(rank_order((0.4, "a"), (0.5, "b")), Ordering::Greater);
    assert_eq!(rank_order((0.5, "a"), (0.5, "b")), Ordering::Less);
    assert_eq!(rank_order((0.5, "b"), (0.5, "a")), Ordering::Greater);
    assert_eq!(rank_order((0.5, "a"), (0.5, "a")), Ordering::Equal);
    let nudge = 0.5f64.next_up();
    assert_eq!(
        rank_order((nudge, "z"), (0.5, "a")),
        Ordering::Less,
        "one ulp is a real difference"
    );
    assert_eq!(
        rank_order((0.0, "z"), (-0.0, "a")),
        Ordering::Less,
        "signed zero is ordered by total_cmp"
    );
}

#[test]
fn rows_round_trip_every_bit_including_signed_zero() {
    let row = [
        -0.0f32,
        0.0,
        f32::from_bits(0x0000_0001),
        f32::from_bits(0x807F_FFFF),
        1.0,
        -1.0,
        f32::from_bits(0x3F80_0001),
        f32::MIN_POSITIVE,
    ];
    let bytes = codec::encode(&row);
    assert_eq!(bytes.len(), 32);
    assert_eq!(
        &bytes[..8],
        &[0, 0, 0, 0x80, 0, 0, 0, 0],
        "little-endian signed zero then zero"
    );
    let decoded = codec::decode_shape(&bytes, 8).unwrap();
    let bits: Vec<u32> = decoded.iter().map(|value| value.to_bits()).collect();
    assert_eq!(
        bits,
        row.iter().map(|value| value.to_bits()).collect::<Vec<_>>()
    );
    assert!(decoded[0].is_sign_negative() && decoded[0] == 0.0);

    let unit_row = unit([-0.0, 0.6, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0]);
    let decoded = codec::decode(&codec::encode(&unit_row), &layout()).unwrap();
    assert_eq!(
        decoded.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        unit_row.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert!(decoded[0].is_sign_negative());
}

#[test]
fn row_rejections_name_shape_and_magnitude_only() {
    let layout = layout();
    assert_eq!(
        codec::decode(&[0u8; 30], &layout),
        Err(RowRejection::TruncatedWord { bytes: 30 })
    );
    assert_eq!(
        codec::decode(&[0u8; 28], &layout),
        Err(RowRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
    assert_eq!(
        codec::decode(&[0u8; 32], &layout),
        Err(RowRejection::ZeroNorm)
    );
    assert_eq!(
        codec::validate(
            &[0.0, f32::NEG_INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            &layout
        ),
        Err(RowRejection::NonFinite { coordinate: 1 })
    );
    assert_eq!(
        codec::validate(&[2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &layout),
        Err(RowRejection::Normalization { norm: 2.0 })
    );
    assert_eq!(codec::validate(&axis(3), &layout), Ok(()));
    let loose = RowLayout {
        unit_norm_tolerance: 1.5,
        ..layout
    };
    assert_eq!(
        codec::validate(&[2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &loose),
        Ok(())
    );
    for rejection in [
        RowRejection::TruncatedWord { bytes: 1 },
        RowRejection::Dimension {
            expected: 8,
            actual: 7,
        },
        RowRejection::NonFinite { coordinate: 0 },
        RowRejection::ZeroNorm,
        RowRejection::Normalization { norm: 2.0 },
    ] {
        let message = rejection.to_string();
        assert!(!message.is_empty());
        assert!(!rejection.reason().is_empty());
    }
    let boundary = RowLayout {
        unit_norm_tolerance: 0.5,
        ..layout
    };
    assert_eq!(
        codec::validate(&[1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &boundary),
        Ok(()),
        "the tolerance is inclusive"
    );
    assert_eq!(
        codec::validate(
            &[1.5f32.next_up(), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            &boundary
        ),
        Err(RowRejection::Normalization {
            norm: f64::from(1.5f32.next_up())
        })
    );
    for tolerance in [f64::NAN, f64::NEG_INFINITY, -0.5] {
        let checked = RowLayout {
            unit_norm_tolerance: tolerance,
            ..layout
        }
        .check();
        assert!(
            matches!(checked, Err(RowRejection::Tolerance { tolerance: seen }) if seen.to_bits() == tolerance.to_bits()),
            "{checked:?}"
        );
    }
    assert_eq!(
        RowLayout {
            unit_norm_tolerance: 0.0,
            ..layout
        }
        .check(),
        Ok(())
    );
}

#[test]
fn the_original_row_artifact_round_trips_and_binds_dimension_and_metric() {
    let rows = vec![
        axis(0),
        unit([-0.0, 0.6, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0]),
        axis(7),
    ];
    let bytes = codec::encode_rows(&layout(), rows.iter().map(Vec::as_slice)).unwrap();
    assert_eq!(bytes.len(), ARTIFACT_HEADER_BYTES + 3 * 32);
    assert_eq!(&bytes[..8], &ARTIFACT_MAGIC);
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), ARTIFACT_VERSION);
    assert_eq!(bytes[10], Metric::InnerProduct.code());
    assert_eq!(bytes[11], 0);
    assert_eq!(
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        DIMENSION
    );
    assert_eq!(u64::from_le_bytes(bytes[16..24].try_into().unwrap()), 3);
    let decoded = codec::decode_rows(&bytes, &layout()).unwrap();
    assert_eq!(decoded.layout, layout());
    assert_eq!(decoded.rows, rows);
    assert_eq!(
        codec::encode_rows(&layout(), rows.iter().map(Vec::as_slice)).unwrap(),
        bytes,
        "byte-identical inputs yield byte-identical artifacts"
    );
    assert!(
        !format!("{decoded:?}").contains("0.6"),
        "Debug reports counts, not coordinates"
    );

    let empty = codec::encode_rows(&layout(), std::iter::empty()).unwrap();
    assert_eq!(empty.len(), ARTIFACT_HEADER_BYTES);
    assert!(
        codec::decode_rows(&empty, &layout())
            .unwrap()
            .rows
            .is_empty()
    );

    let other_dimension = RowLayout {
        dimension: 4,
        ..layout()
    };
    assert_eq!(
        codec::decode_rows(&bytes, &other_dimension),
        Err(ArtifactRejection::Dimension {
            declared: 8,
            expected: 4
        })
    );

    let mut bad_metric = bytes.clone();
    bad_metric[10] = 7;
    assert_eq!(
        codec::decode_rows(&bad_metric, &layout()),
        Err(ArtifactRejection::Metric {
            code: 7,
            expected: Metric::InnerProduct
        })
    );
    let mut bad_version = bytes.clone();
    bad_version[8] = 2;
    assert_eq!(
        codec::decode_rows(&bad_version, &layout()),
        Err(ArtifactRejection::Version { version: 2 })
    );
    let mut bad_reserved = bytes.clone();
    bad_reserved[11] = 1;
    assert_eq!(
        codec::decode_rows(&bad_reserved, &layout()),
        Err(ArtifactRejection::Reserved { reserved: 1 })
    );
    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert_eq!(
        codec::decode_rows(&bad_magic, &layout()),
        Err(ArtifactRejection::Magic)
    );
    assert_eq!(
        codec::decode_rows(&bytes[..ARTIFACT_HEADER_BYTES - 1], &layout()),
        Err(ArtifactRejection::ShortHeader {
            bytes: ARTIFACT_HEADER_BYTES - 1
        })
    );
    assert_eq!(
        codec::decode_rows(&bytes[..bytes.len() - 1], &layout()),
        Err(ArtifactRejection::RowBytes {
            declared: 3,
            bytes: 3 * 32 - 1
        })
    );
    let mut extra = bytes.clone();
    extra.extend_from_slice(&[0; 32]);
    assert_eq!(
        codec::decode_rows(&extra, &layout()),
        Err(ArtifactRejection::RowBytes {
            declared: 3,
            bytes: 4 * 32
        })
    );

    let mut nan_row = bytes.clone();
    nan_row[ARTIFACT_HEADER_BYTES + 32..ARTIFACT_HEADER_BYTES + 36]
        .copy_from_slice(&f32::NAN.to_le_bytes());
    assert_eq!(
        codec::decode_rows(&nan_row, &layout()),
        Err(ArtifactRejection::Row {
            index: 1,
            rejection: RowRejection::NonFinite { coordinate: 0 }
        })
    );
    let zero = vec![0.0f32; 8];
    assert_eq!(
        codec::encode_rows(&layout(), [axis(0).as_slice(), zero.as_slice()]),
        Err(ArtifactRejection::Row {
            index: 1,
            rejection: RowRejection::ZeroNorm
        })
    );
    assert_eq!(
        codec::encode_rows(
            &RowLayout {
                dimension: 0,
                ..layout()
            },
            std::iter::empty()
        ),
        Err(ArtifactRejection::ZeroDimension)
    );
}

#[test]
fn live_rows_exports_every_live_vector_in_identifier_order_and_refuses_over_bound_and_invalid_rows()
{
    let fixture = Fixture::all_admitted();
    let generation = generation();
    let export =
        |fixture: &Fixture, max: usize, layout: RowLayout| -> Result<LiveRows, ExportRefusal> {
            fixture
                .store
                .with_conn(|conn| {
                    Ok(live_rows(
                        conn,
                        &generation,
                        &fixture.incarnation,
                        &layout,
                        NonZeroUsize::new(max).unwrap(),
                    ))
                })
                .unwrap()
        };
    let exported = export(&fixture, 64, layout()).unwrap();
    let ids: Vec<&str> = exported
        .rows
        .iter()
        .map(|row| row.occurrence_id.as_str())
        .collect();
    let mut expected: Vec<String> = fixture.dense_ids().into_iter().collect();
    expected.sort();
    assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
    for row in &exported.rows {
        let source = fixture
            .rows
            .iter()
            .find(|r| r.occurrence_id() == row.occurrence_id)
            .unwrap();
        assert_eq!(&row.vector, source.vector.as_ref().unwrap());
    }
    assert_eq!(exported.checkpoint.hold_id, HOLD);
    assert!(exported.checkpoint.checkpoint_commit_seq >= exported.checkpoint.snapshot_commit_seq);
    assert!(
        !format!("{:?}", exported.rows).contains("0.9"),
        "Debug hides coordinates"
    );

    assert_eq!(
        export(&fixture, 7, layout()),
        Err(ExportRefusal::OverBound { max: 7 })
    );
    assert_eq!(export(&fixture, 8, layout()).unwrap().rows.len(), 8);

    // A missing vector and a tombstoned row leave the export; the export names only rows that carry a vector.
    fixture.drop_vector("beta");
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id, invalidated_commit_seq, reason, recorded_at) VALUES (?1, 99, 'retired', 0)",
            [fixture.id("gamma")],
        )
        .unwrap();
    let rows = export(&fixture, 64, layout()).unwrap().rows;
    assert_eq!(rows.len(), 6);
    assert!(
        !rows
            .iter()
            .any(|row| row.occurrence_id == fixture.id("beta"))
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.occurrence_id == fixture.id("gamma"))
    );

    assert_eq!(
        export(
            &fixture,
            64,
            RowLayout {
                dimension: 4,
                ..layout()
            }
        ),
        Err(ExportRefusal::LayoutMismatch {
            layout: 4,
            generation: 8
        })
    );
    assert!(matches!(
        export(
            &fixture,
            64,
            RowLayout {
                unit_norm_tolerance: f64::NAN,
                ..layout()
            }
        ),
        Err(ExportRefusal::Layout(RowRejection::Tolerance { .. }))
    ));
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
            rusqlite::params![
                fixture.id("delta"),
                codec::encode(&[0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            ],
        )
        .unwrap();
    assert!(matches!(
        export(&fixture, 64, layout()),
        Err(ExportRefusal::StoredRow { occurrence_id, rejection: RowRejection::Normalization { .. } }) if occurrence_id == fixture.id("delta")
    ));
    let foreign = VectorGeneration {
        generation_id: "gen-9".to_string(),
        ..generation.clone()
    };
    let unknown = fixture
        .store
        .with_conn(|conn| {
            Ok(live_rows(
                conn,
                &foreign,
                &fixture.incarnation,
                &layout(),
                NonZeroUsize::new(8).unwrap(),
            ))
        })
        .unwrap();
    assert!(matches!(
        unknown,
        Err(ExportRefusal::Projection(
            ProjectionError::UnknownGeneration { .. }
        ))
    ));
    let other_kernel = fixture
        .store
        .with_conn(|conn| {
            Ok(live_rows(
                conn,
                &generation,
                "other-kernel",
                &layout(),
                NonZeroUsize::new(8).unwrap(),
            ))
        })
        .unwrap();
    assert!(matches!(
        other_kernel,
        Err(ExportRefusal::Projection(ProjectionError::IdentityMismatch))
    ));
    fixture
        .raw()
        .execute("DELETE FROM projection_checkpoint", [])
        .unwrap();
    assert!(matches!(
        export(&fixture, 64, layout()),
        Err(ExportRefusal::NoCheckpoint { .. })
    ));
}
