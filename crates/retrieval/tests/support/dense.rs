//! The projection-plus-kernel fixture the dense ranking tests share: a seeded kernel with admitted decisions, a projected corpus with stored vectors, and the independent f64 reference ranking.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, BackupRequest, CommitIntent,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EventKind, KernelStore, ProjectScope,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, ProjectionBatch, VectorGeneration, apply_batch,
    register_generation,
};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::oracle::exhaustive_with_hook_for_test;
use retrieval::dense::{
    ExhaustiveQuery, ExhaustiveRanking, OracleBounds, OracleRefusal, Window, exhaustive,
};
use retrieval::eligibility::Authority;
use retrieval::{OccurrenceRecord, Payload, PersistBounds, ProjectionIdentity, install_identity};
use sha2::{Digest, Sha256};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

pub const DOMAIN: &str = "domain";
pub const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SCOPE_A: &str = "project:a";
pub const HOLD: &str = "hold-1";
pub const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const COMMIT_OID: &str = "0123456789abcdef0123456789abcdef01234567";
pub const GENERATION: &str = "gen-1";
pub const DIMENSION: u32 = 8;
pub const TOLERANCE: f64 = 1e-3;

pub fn layout() -> RowLayout {
    RowLayout {
        dimension: DIMENSION,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
    }
}

pub fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: DIMENSION,
        generation_epoch: 1,
    }
}

/// f64 normalization limits rounding error before fixture rows narrow to f32.
pub fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

pub fn axis(index: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[index] = 1.0;
    unit(raw)
}

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "dense-oracle-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "retrieval".to_string(),
    }
}

pub fn scope(scope_id: &str, digest: &str) -> ScopeSpec {
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

pub fn decision(object: &str) -> DecisionSpec {
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

pub fn admission(object: &str) -> AdmissionRequest {
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

pub fn open_store(dir: &Path) -> SqliteStore {
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

pub fn batch_bounds() -> BatchBounds {
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

pub fn bounds(k: usize) -> OracleBounds {
    OracleBounds {
        k: NonZeroUsize::new(k).unwrap(),
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub class: OccurrenceClass,
    pub object: String,
    pub vector: Option<Vec<f32>>,
}

impl Row {
    pub fn claim(object: &str, vector: Vec<f32>) -> Self {
        Self {
            class: OccurrenceClass::CanonicalClaims,
            object: object.to_string(),
            vector: Some(vector),
        }
    }

    pub fn identity(&self) -> Vec<(&str, &str)> {
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

    pub fn representation(&self) -> &'static str {
        match self.class {
            OccurrenceClass::GitCommits => "commit_message",
            OccurrenceClass::RawToolSpans => "tool_output",
            _ => "decision_summary",
        }
    }

    pub fn occurrence<'a>(&'a self, identity: &'a [(&'a str, &'a str)]) -> Occurrence<'a> {
        Occurrence {
            class: self.class.code(),
            identity,
            revision: "1",
            representation: self.representation(),
            span: None,
        }
    }

    pub fn occurrence_id(&self) -> String {
        kernel::source_identity::encode_preserving_span(&self.occurrence(&self.identity()))
            .unwrap()
            .occurrence_id
    }
}

/// Against `axis(0)` each score is the row's first coordinate, so the reference order is legible by eye.
pub fn corpus() -> Vec<Row> {
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

pub const OBJECTS: [&str; 9] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "eta", "theta", "zeta", "tool",
];

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub kernel: KernelStore,
    pub store: SqliteStore,
    pub incarnation: String,
    pub project: ProjectScope,
    pub rows: Vec<Row>,
}

impl Fixture {
    pub fn new(admitted: &[&str], rows: Vec<Row>) -> Self {
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

    pub fn all_admitted() -> Self {
        Self::new(&OBJECTS, corpus())
    }

    /// Projects every row with a queued embedding job, then completes the jobs whose rows carry a vector.
    pub fn project(&self) {
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

    pub fn id(&self, object: &str) -> String {
        self.row(object).occurrence_id()
    }

    pub fn row(&self, object: &str) -> &Row {
        self.rows.iter().find(|row| row.object == object).unwrap()
    }

    pub fn retire(&self, object: &str) {
        self.kernel
            .commit(intent(&format!("retire-{object}")), |envelope| {
                envelope.retire_decision(object)?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Snapshots the kernel so a later `restore` replaces its incarnation.
    pub fn backup(&self) -> std::path::PathBuf {
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

    pub fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("search").join("search.sqlite")).unwrap()
    }

    pub fn drop_vector(&self, object: &str) {
        let changed = self
            .raw()
            .execute(
                "DELETE FROM occurrence_vectors WHERE occurrence_id=?1",
                [self.id(object)],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    pub fn job_state(&self, object: &str, state: &str) {
        let changed = self
            .raw()
            .execute(
                "UPDATE embedding_jobs SET state=?2 WHERE occurrence_id=?1",
                rusqlite::params![self.id(object), state],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    pub fn authority(&self) -> Authority<'_> {
        Authority {
            project: &self.project,
            destination: ArtifactDestination::Local,
        }
    }

    pub fn query<'a>(
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

    pub fn rank(
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

    pub fn rank_with_hook(
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

    pub fn rank_recording(
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
    pub fn reference(&self, query: &[f32], objects: &[&str]) -> Vec<(String, f64)> {
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

    pub fn dense_ids(&self) -> BTreeSet<String> {
        self.rows
            .iter()
            .filter(|row| row.class != OccurrenceClass::RawToolSpans)
            .map(Row::occurrence_id)
            .collect()
    }
}

pub fn keyed(ranking: &ExhaustiveRanking) -> Vec<(String, f64)> {
    ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

pub fn ids_of(ranking: &ExhaustiveRanking) -> Vec<String> {
    ranking
        .ranked
        .iter()
        .map(|row| row.occurrence_id.clone())
        .collect()
}

pub fn assert_visited_once(fixture: &Fixture, visited: &[String]) {
    let unique: BTreeSet<String> = visited.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        visited.len(),
        "a row was visited twice: {visited:?}"
    );
    assert_eq!(unique, fixture.dense_ids());
}
