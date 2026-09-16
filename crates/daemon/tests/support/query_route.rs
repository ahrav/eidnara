//! A `KernelDaemon` whose kernel holds three decisions, a projection built from its snapshot, and the route limits the query-route tests share.

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use daemon::claim_sources::{ClaimMaterializer, MaterializationEnd};
use daemon::query_route::{
    DenseLane, Phase, QueryFailure, QueryOutcome, QueryRouteLimits, execute,
};
use daemon::request_budget::{RequestBudget, SharedBudget};
use daemon::search_projection::SearchProjection;
use host_runtime::CancelSignal;
use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;
use kernel::{
    ArtifactDestination, CommitIntent, CommitPageBounds, KernelStore, ProjectScope, ProviderEgress,
    SourceHold, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds, SourcePageBounds,
    SourceRow,
};
use retrieval::batch::{
    BatchBounds, MutationIdentity, VectorGeneration, batch_from_rows, register_generation,
    row_identities,
};
use retrieval::dense::codec;
use retrieval::eligibility::{Authority, OccurrenceCandidate, live_candidates};
use retrieval::exact::{
    ExactQuery, Intent, LookupContext, SelectorBounds, SelectorValue, classify, page,
};
use retrieval::fusion::{
    FusionParameters, Lane, LaneHit, LaneRanking, LaneWeights, OccurrenceId, RawScore,
};
use retrieval::lexical::{LexicalBounds, RetrievalBounds, analyze_segments, compile, retrieve};
use retrieval::{PersistBounds, ProjectionIdentity, install_identity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::kernel_daemon::KernelDaemon;

pub const GENERATION: &str = "gen-1";
pub const DIMENSION: u32 = 8;
pub const CONSUMER: &str = "search";
pub const POLICY: &str = "source-policy.v1";
pub const FOREIGN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
pub const MEMORY: &str = "memory";
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub const MODEL: &str = "tiny-test-model";
pub const NOW: i64 = 1_700_000_000_000;
pub const KERNEL: &str = "route-kernel";
pub const QUERY: &str = "id:rule explicit contract";
pub const WEIGHTS: LaneWeights = LaneWeights {
    exact: 2.0,
    lexical: 1.0,
    dense: 1.0,
};
pub const K: f64 = 7.0;
pub const ALL_PHASES: [Phase; 8] = [
    Phase::Probes,
    Phase::Exact,
    Phase::Lexical,
    Phase::Dense,
    Phase::Fusion,
    Phase::Revalidation,
    Phase::Materialization,
    Phase::Response,
];

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "query-route-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

pub fn commit_bounds() -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

pub fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

pub fn page_bounds() -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

pub fn projection_identity() -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: KERNEL.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v3".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        analysis_identity: retrieval::lexical::AnalysisIdentity::current()
            .as_str()
            .to_string(),
        vector_dimension: DIMENSION,
        generation_epoch: 1,
    }
}

pub fn limits() -> QueryRouteLimits {
    QueryRouteLimits {
        query_bytes: NonZeroUsize::new(512).unwrap(),
        probes: NonZeroUsize::new(16).unwrap(),
        lexical_scan_rows: NonZeroUsize::new(256).unwrap(),
        lexical_accepted: NonZeroUsize::new(64).unwrap(),
        validation_batch: NonZeroUsize::new(16).unwrap(),
        exact_page_rows: NonZeroUsize::new(16).unwrap(),
        exact_pages: NonZeroUsize::new(4).unwrap(),
        fused_union: NonZeroUsize::new(64).unwrap(),
        result_rows: NonZeroUsize::new(32).unwrap(),
        response_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        deadline_ceiling: Duration::from_secs(20),
        fusion: FusionParameters::new(WEIGHTS, K).unwrap(),
        dense: None,
    }
}

pub fn request_budget(remaining_ms: u64) -> (CancellationToken, RequestBudget) {
    let token = CancellationToken::new();
    let budget = RequestBudget::derive(
        CancelSignal::observing(token.clone()),
        Some(remaining_ms),
        Some(Duration::from_secs(20)),
    )
    .unwrap();
    (token, budget)
}

pub struct Fixture {
    pub daemon: KernelDaemon,
    pub store: Arc<KernelStore>,
    pub project: ProjectScope,
    pub projection: SearchProjection,
    _dir: tempfile::TempDir,
}

impl Fixture {
    pub async fn build() -> Self {
        let daemon = KernelDaemon::start().await;
        let store = daemon.store();
        ClaimMaterializer::register(&store, NOW).unwrap();
        let spec = |object: &str, revision: i64, summary: &str| {
            json!({
                "decision_id": format!("{object}-decision"),
                "object_id": object,
                "domain_id": MEMORY,
                "decision_kind": "PROJECT_RULES",
                "payload": {"summary": summary, "rationale": format!("because {object}")},
                "source_id": format!("{object}-lineage"),
                "source_revision": revision,
            })
        };
        let created = daemon
            .commit(
                "create",
                vec![
                    json!({"op": "insert_decision", "spec": spec("rule", 1, "Keep the public contract explicit.")}),
                    json!({"op": "insert_decision", "spec": spec("other", 1, "Name things after the contract.")}),
                    json!({"op": "insert_decision", "spec": spec("third", 1, "Short names win.")}),
                ],
            )
            .await;
        assert_eq!(created["state"]["kind"], "available", "{created}");
        let scope_id = daemon.read("explicit_search", None, None).await["rows"][0]["scope_id"]
            .as_str()
            .unwrap()
            .to_string();
        let project = ProjectScope::new(scope_id.strip_prefix("project:").unwrap()).unwrap();
        store
            .commit(intent("search"), |envelope| {
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
        let mut materializer = ClaimMaterializer::new(&store, ProviderEgress::LocalOnly);
        let report = materializer.run_episode(commit_bounds(), NOW).unwrap();
        assert!(
            matches!(report.end, MaterializationEnd::ReachedTarget),
            "{report:?}"
        );
        let dir = tempfile::tempdir().unwrap();
        let projection = SearchProjection::open(dir.path()).unwrap();
        projection
            .write(|conn| {
                install_identity(conn, &projection_identity(), 1)?;
                register_generation(conn, &generation(), 1).map(|_| ())
            })
            .unwrap();
        let fixture = Self {
            daemon,
            store,
            project,
            projection,
            _dir: dir,
        };
        fixture.project_snapshot(2);
        fixture
    }

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.store.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    fn capture(&self) -> SourceHold {
        self.store
            .capture_source_hold(
                &self.binding(),
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: SourceHoldAdmission {
                        max_references: NonZeroUsize::new(64).unwrap(),
                        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                    },
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap()
    }

    fn export(&self, hold: &SourceHold) -> Vec<SourceRow> {
        let binding = self.binding();
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .export_source_page(
                    &binding,
                    &hold.hold_id,
                    hold.captured_at,
                    kernel::ExportWindow::Snapshot,
                    cursor.as_ref(),
                    page_bounds(),
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return rows,
            }
        }
    }

    pub fn project_snapshot(&self, now: i64) {
        let hold = self.capture();
        let rows = self.export(&hold);
        let identities = row_identities(&rows);
        let batch = batch_from_rows(
            &rows,
            &identities,
            MutationIdentity {
                kernel_incarnation_id: KERNEL.to_string(),
                hold_id: hold.hold_id.clone(),
                snapshot_commit_seq: hold.snapshot,
                through_commit_seq: hold.snapshot,
            },
            Some(GENERATION),
        )
        .unwrap();
        self.projection
            .apply_batch(&batch, batch_bounds(), now)
            .unwrap();
        self.store
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
    }

    pub fn run(
        &self,
        limits: &QueryRouteLimits,
        shared: &SharedBudget,
        query: &str,
        before_phase: impl FnMut(Phase),
    ) -> Result<QueryOutcome, QueryFailure> {
        execute(
            &self.projection,
            &self.store,
            Authority {
                project: &self.project,
                destination: ArtifactDestination::Local,
            },
            limits,
            shared,
            query,
            DenseLane::Undeclared,
            before_phase,
        )
    }

    pub fn run_with_scope(
        &self,
        project: &ProjectScope,
        shared: &SharedBudget,
    ) -> Result<QueryOutcome, QueryFailure> {
        execute(
            &self.projection,
            &self.store,
            Authority {
                project,
                destination: ArtifactDestination::Local,
            },
            &limits(),
            shared,
            QUERY,
            DenseLane::Undeclared,
            |_| {},
        )
    }

    pub fn oracle(&self, shared: &SharedBudget) -> Vec<String> {
        let limits = limits();
        let intent = classify(
            QUERY,
            SelectorBounds {
                max_input_bytes: limits.query_bytes,
                max_value_bytes: limits.query_bytes,
            },
        )
        .unwrap();
        let Intent::Hybrid(mentions) = &intent else {
            panic!("the fixture query mixes a selector with prose");
        };
        let SelectorValue::Text(object_id) = &mentions[0].selector.value else {
            panic!("the fixture selector names an object");
        };
        self.projection
            .read(|conn| {
                let context = LookupContext {
                    kernel_incarnation_id: KERNEL,
                    page_rows: limits.exact_page_rows,
                    budget: shared.eval(),
                };
                let mut exact = Vec::new();
                let mut cursor = None;
                loop {
                    let page = page(
                        conn,
                        &context,
                        &ExactQuery::CanonicalObject(object_id.as_bytes()),
                        cursor.as_ref(),
                    )
                    .unwrap();
                    exact.extend(page.rows.iter().map(|row| LaneHit {
                        occurrence: OccurrenceId::parse(&row.occurrence_id).unwrap(),
                        raw_score: RawScore::Exact,
                    }));
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
                let analysis = analyze_segments(
                    &intent.lexical_segments(QUERY),
                    LexicalBounds {
                        max_input_bytes: limits.query_bytes,
                        max_atoms: limits.probes,
                    },
                )
                .unwrap();
                let retrieval = retrieve(
                    conn,
                    &self.store,
                    &compile(&analysis),
                    Authority {
                        project: &self.project,
                        destination: ArtifactDestination::Local,
                    },
                    RetrievalBounds {
                        max_probes: limits.probes,
                        scan_rows: limits.lexical_scan_rows,
                        max_accepted: limits.lexical_accepted,
                        batch_rows: limits.validation_batch,
                    },
                    shared.eval(),
                )
                .unwrap();
                let lexical = retrieval.contributions.iter().map(|c| LaneHit {
                    occurrence: OccurrenceId::parse(&c.occurrence_id).unwrap(),
                    raw_score: RawScore::Lexical(c.rank),
                });
                let exact =
                    LaneRanking::consolidate(Lane::Exact, OCCURRENCE_ENCODING_VERSION, exact)
                        .unwrap();
                let lexical =
                    LaneRanking::consolidate(Lane::Lexical, OCCURRENCE_ENCODING_VERSION, lexical)
                        .unwrap();
                assert!(!exact.entries().is_empty() && !lexical.entries().is_empty());
                let mut scores: BTreeMap<OccurrenceId, f64> = BTreeMap::new();
                for (ranking, weight) in [(&exact, WEIGHTS.exact), (&lexical, WEIGHTS.lexical)] {
                    for entry in ranking.entries() {
                        *scores.entry(*entry.occurrence()).or_insert(0.0) +=
                            weight / (K + entry.position().get() as f64);
                    }
                }
                let mut ordered: Vec<(OccurrenceId, f64)> = scores.into_iter().collect();
                ordered.sort_by(|(left_id, left), (right_id, right)| {
                    right
                        .partial_cmp(left)
                        .unwrap()
                        .then_with(|| left_id.cmp(right_id))
                });
                Ok(ordered.into_iter().map(|(id, _)| id.to_string()).collect())
            })
            .unwrap()
    }
}

pub fn entry_ids(body: &Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["occurrence_id"].as_str().unwrap().to_string())
        .collect()
}

pub fn generation() -> VectorGeneration {
    VectorGeneration {
        generation_id: GENERATION.to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: DIMENSION,
        generation_epoch: 1,
    }
}

pub fn unit(raw: [f32; DIMENSION as usize]) -> Vec<f32> {
    let norm = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
    raw.iter().map(|x| x / norm).collect()
}

impl Fixture {
    pub fn live_candidates(&self) -> Vec<OccurrenceCandidate> {
        self.projection
            .read(|conn| live_candidates(conn, None, NonZeroUsize::new(64).unwrap()))
            .unwrap()
    }

    /// Stores `vector_of(occurrence_id)` for every live occurrence under the fixture generation and marks its job embedded.
    pub fn store_vectors(&self, vector_of: impl Fn(&str) -> Vec<f32>) -> Vec<(String, Vec<f32>)> {
        let rows: Vec<(String, Vec<f32>)> = self
            .live_candidates()
            .into_iter()
            .map(|candidate| {
                let vector = vector_of(&candidate.occurrence_id);
                (candidate.occurrence_id, vector)
            })
            .collect();
        self.projection
            .write(|conn| {
                for (occurrence_id, vector) in &rows {
                    conn.execute(
                        "INSERT OR REPLACE INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,?4,1,1,1)",
                        rusqlite::params![occurrence_id, GENERATION, codec::encode(vector), DIMENSION],
                    )?;
                    conn.execute(
                        "UPDATE embedding_jobs SET state='embedded' WHERE occurrence_id=?1",
                        [occurrence_id],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        rows
    }

    pub fn store_raw_vector(&self, occurrence_id: &str, bytes: &[u8]) {
        self.projection
            .write(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES (?1,?2,?3,?4,1,1,1)",
                    rusqlite::params![occurrence_id, GENERATION, bytes, DIMENSION],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

/// Inner-product order over `rows`, best first, ties by identifier bytes.
pub fn dense_reference(query: &[f32], rows: &[(String, Vec<f32>)]) -> Vec<(String, f64)> {
    let mut scored: Vec<(String, f64)> = rows
        .iter()
        .map(|(id, vector)| {
            (
                id.clone(),
                vector
                    .iter()
                    .zip(query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum(),
            )
        })
        .collect();
    scored.sort_by(|(left_id, left), (right_id, right)| {
        right
            .partial_cmp(left)
            .unwrap()
            .then_with(|| left_id.cmp(right_id))
    });
    scored
}
