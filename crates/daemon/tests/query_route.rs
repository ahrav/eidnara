//! `retrieval.query` execution over a projection populated from a live kernel.

mod support;

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use daemon::claim_sources::{ClaimMaterializer, MaterializationEnd};
use daemon::query_route::{
    LaneStatus, Phase, QueryFailure, QueryOutcome, QueryRouteLimits, Terminal, execute,
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
use retrieval::batch::{BatchBounds, MutationIdentity, batch_from_rows, row_identities};
use retrieval::eligibility::Authority;
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
use support::kernel_daemon::KernelDaemon;
use tokio_util::sync::CancellationToken;

const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const FOREIGN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const MEMORY: &str = "memory";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const NOW: i64 = 1_700_000_000_000;
const KERNEL: &str = "route-kernel";
const QUERY: &str = "id:rule explicit contract";
const WEIGHTS: LaneWeights = LaneWeights {
    exact: 2.0,
    lexical: 1.0,
    dense: 1.0,
};
const K: f64 = 7.0;
const ALL_PHASES: [Phase; 7] = [
    Phase::Probes,
    Phase::Exact,
    Phase::Lexical,
    Phase::Fusion,
    Phase::Revalidation,
    Phase::Materialization,
    Phase::Response,
];

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "query-route-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn commit_bounds() -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn batch_bounds() -> BatchBounds {
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

fn page_bounds() -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

fn projection_identity() -> ProjectionIdentity {
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
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn limits() -> QueryRouteLimits {
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
    }
}

fn budget(remaining_ms: u64) -> (CancellationToken, RequestBudget) {
    let token = CancellationToken::new();
    let budget = RequestBudget::derive(
        CancelSignal::observing(token.clone()),
        Some(remaining_ms),
        Some(Duration::from_secs(20)),
    )
    .unwrap();
    (token, budget)
}

struct Fixture {
    daemon: KernelDaemon,
    store: Arc<KernelStore>,
    project: ProjectScope,
    projection: SearchProjection,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn build() -> Self {
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
            .write(|conn| install_identity(conn, &projection_identity(), 1).map(|_| ()))
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

    fn project_snapshot(&self, now: i64) {
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
            None,
        )
        .unwrap();
        self.projection
            .apply_batch(&batch, batch_bounds(), now)
            .unwrap();
        self.store
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
    }

    fn run(
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
            before_phase,
        )
    }

    fn run_with_scope(
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
            |_| {},
        )
    }

    fn oracle(&self, shared: &SharedBudget) -> Vec<String> {
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

fn entry_ids(body: &Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["occurrence_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_healthy_query_completes_fused_in_the_oracles_order() {
    let fixture = Fixture::build().await;
    let (_token, budget) = budget(10_000);
    let mut phases = Vec::new();
    let outcome = fixture
        .run(&limits(), budget.shared(), QUERY, |phase| {
            phases.push(phase)
        })
        .unwrap();
    assert_eq!(phases, ALL_PHASES.to_vec());
    assert_eq!(outcome.body["kind"], "fused");
    assert_eq!(outcome.body["degraded"], false);
    assert_eq!(outcome.body["truncated"], false);
    assert_eq!(
        outcome.statuses,
        [
            LaneStatus::Complete,
            LaneStatus::Complete,
            LaneStatus::Undeclared
        ]
    );
    let ids = entry_ids(&outcome.body);
    assert!(!ids.is_empty(), "{}", outcome.body);
    assert_eq!(ids, fixture.oracle(budget.shared()));
    assert!(
        ids.iter()
            .zip(outcome.fused.entries())
            .all(|(id, entry)| *id == entry.occurrence().to_string())
    );
    let first = &outcome.body["entries"][0];
    assert_eq!(first["position"], 1);
    assert!(
        outcome
            .fused
            .entries()
            .iter()
            .any(|entry| entry.lane(Lane::Exact).is_some()),
        "the id mention feeds the exact lane"
    );
    assert!(
        outcome
            .fused
            .entries()
            .iter()
            .any(|entry| entry.lane(Lane::Lexical).is_some()),
        "the prose feeds the lexical lane"
    );
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_deadline_are_observed_in_every_phase() {
    let fixture = Fixture::build().await;
    for target in ALL_PHASES {
        let (token, budget) = budget(10_000);
        let mut reached = Vec::new();
        let outcome = fixture.run(&limits(), budget.shared(), QUERY, |phase| {
            reached.push(phase);
            if phase == target {
                token.cancel();
            }
        });
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Cancelled)),
            "cancelled at {target:?}"
        );
        assert_eq!(reached.last(), Some(&target), "{reached:?}");
    }
    for target in ALL_PHASES {
        let (_token, budget) = budget(600);
        let mut reached = Vec::new();
        let outcome = fixture.run(&limits(), budget.shared(), QUERY, |phase| {
            reached.push(phase);
            if phase == target {
                std::thread::sleep(Duration::from_millis(700));
            }
        });
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Deadline)),
            "deadline at {target:?}"
        );
        assert_eq!(reached.last(), Some(&target), "{reached:?}");
    }
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revalidation_excludes_retired_and_foreign_occurrences() {
    let fixture = Fixture::build().await;
    let (_token, budget) = budget(10_000);
    let before = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    let before_ids = entry_ids(&before.body);
    let retired = fixture
        .daemon
        .commit(
            "retire",
            vec![json!({"op": "retire_decision", "object_id": "other"})],
        )
        .await;
    assert_eq!(retired["state"]["kind"], "available", "{retired}");
    let report = ClaimMaterializer::new(&fixture.store, ProviderEgress::LocalOnly)
        .run_episode(commit_bounds(), NOW)
        .unwrap();
    assert!(report.retired > 0, "{report:?}");
    let after = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    let after_ids = entry_ids(&after.body);
    assert!(
        after_ids.len() < before_ids.len(),
        "{before_ids:?} vs {after_ids:?}"
    );
    assert!(after_ids.iter().all(|id| before_ids.contains(id)));
    assert_eq!(after.body["degraded"], false);

    let foreign = ProjectScope::new(FOREIGN).unwrap();
    let outcome = fixture.run_with_scope(&foreign, budget.shared()).unwrap();
    assert_eq!(outcome.body["kind"], "fused");
    assert!(entry_ids(&outcome.body).is_empty(), "{}", outcome.body);
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_bound_saturates_before_its_protected_work() {
    let fixture = Fixture::build().await;
    let run = |limits: &QueryRouteLimits, query: &str| {
        let (_token, budget) = budget(10_000);
        let mut reached = Vec::new();
        let outcome = fixture.run(limits, budget.shared(), query, |phase| reached.push(phase));
        (outcome, reached)
    };
    let (full, _) = run(&limits(), QUERY);
    let full = full.unwrap();
    let total = entry_ids(&full.body).len();
    assert!(total >= 2, "{}", full.body);
    let lexical_hits = full
        .fused
        .entries()
        .iter()
        .filter(|entry| entry.lane(Lane::Lexical).is_some())
        .count();
    assert!(lexical_hits >= 2, "{}", full.body);

    let mut rows = limits();
    rows.result_rows = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&rows, QUERY);
    let outcome = outcome.unwrap();
    assert_eq!(entry_ids(&outcome.body).len(), 1);
    assert!(outcome.truncated);
    assert_eq!(outcome.body["truncated"], true);
    assert_eq!(outcome.fused.entries().len(), total);

    let mut bytes = limits();
    bytes.response_bytes = NonZeroUsize::new(200).unwrap();
    let (outcome, _) = run(&bytes, QUERY);
    let outcome = outcome.unwrap();
    assert!(entry_ids(&outcome.body).len() < total);
    assert!(outcome.truncated);
    assert!(serde_json::to_vec(&outcome.body).unwrap().len() <= 200);

    let mut union = limits();
    union.fused_union = NonZeroUsize::new(1).unwrap();
    let (outcome, reached) = run(&union, QUERY);
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Unavailable("fused_union"))
    );
    assert_eq!(reached.last(), Some(&Phase::Fusion), "{reached:?}");

    let mut pages = limits();
    pages.exact_page_rows = NonZeroUsize::new(1).unwrap();
    pages.exact_pages = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&pages, QUERY);
    let outcome = outcome.unwrap();
    assert_eq!(outcome.statuses[0], LaneStatus::Incomplete("page_bound"));
    assert_eq!(outcome.body["degraded"], true);
    assert_eq!(outcome.body["lanes"]["exact"]["status"], "incomplete");

    let mut batches = limits();
    batches.validation_batch = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&batches, QUERY);
    assert_eq!(entry_ids(&outcome.unwrap().body), entry_ids(&full.body));

    let mut selectors = limits();
    selectors.probes = NonZeroUsize::new(1).unwrap();
    let (outcome, reached) = run(&selectors, "id:rule id:other");
    assert!(matches!(outcome, Err(QueryFailure::InvalidQuery(_))));
    assert_eq!(reached.last(), Some(&Phase::Exact), "{reached:?}");

    let mut accepted = limits();
    accepted.lexical_accepted = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&accepted, QUERY);
    let outcome = outcome.unwrap();
    assert_eq!(
        outcome.statuses[1],
        LaneStatus::Incomplete("accepted_bound"),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.body["degraded"], true);

    let mut scan = limits();
    scan.lexical_scan_rows = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&scan, QUERY);
    let outcome = outcome.unwrap();
    assert_eq!(
        outcome.statuses[1],
        LaneStatus::Incomplete("scan_bound"),
        "{}",
        outcome.body
    );

    let mut probes = limits();
    probes.probes = NonZeroUsize::new(1).unwrap();
    let (outcome, _) = run(&probes, "id:rule explicit contract public");
    let outcome = outcome.unwrap();
    assert!(
        matches!(outcome.statuses[1], LaneStatus::Unavailable(_)),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.body["degraded"], true);
    assert!(
        outcome
            .fused
            .entries()
            .iter()
            .all(|entry| entry.lane(Lane::Exact).is_some()),
        "the exact lane still serves"
    );

    let long = "x".repeat(600);
    let (outcome, _) = run(&limits(), &long);
    assert!(matches!(outcome, Err(QueryFailure::InvalidQuery(_))));
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lane_that_cannot_run_degrades_the_answer_while_the_other_serves() {
    let fixture = Fixture::build().await;
    let dir = tempfile::tempdir().unwrap();
    let projection = SearchProjection::open(dir.path()).unwrap();
    projection
        .write(|conn| install_identity(conn, &projection_identity(), 1).map(|_| ()))
        .unwrap();
    let (_token, budget) = budget(10_000);
    let outcome = execute(
        &projection,
        &fixture.store,
        Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
        },
        &limits(),
        budget.shared(),
        QUERY,
        |_| {},
    )
    .unwrap();
    assert!(
        matches!(outcome.statuses[0], LaneStatus::Unavailable(_)),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.statuses[1], LaneStatus::Complete);
    assert_eq!(outcome.body["kind"], "fused");
    assert_eq!(outcome.body["degraded"], true);
    assert_eq!(outcome.body["lanes"]["exact"]["status"], "unavailable");
    assert_eq!(outcome.body["lanes"]["exact"]["reason"], "no_checkpoint");
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_declared_lane_serves_and_no_lane_is_refused() {
    let fixture = Fixture::build().await;
    let (_token, budget) = budget(10_000);
    let direct = fixture
        .run(&limits(), budget.shared(), "id:rule", |_| {})
        .unwrap();
    assert_eq!(
        direct.statuses,
        [
            LaneStatus::Complete,
            LaneStatus::Undeclared,
            LaneStatus::Undeclared
        ]
    );
    assert!(!entry_ids(&direct.body).is_empty());
    assert_eq!(direct.body["degraded"], false);
    let prose = fixture
        .run(&limits(), budget.shared(), "explicit contract", |_| {})
        .unwrap();
    assert_eq!(prose.statuses[0], LaneStatus::Undeclared);
    assert_eq!(prose.statuses[1], LaneStatus::Complete);
    assert!(!entry_ids(&prose.body).is_empty());
    let none = fixture.run(&limits(), budget.shared(), "   ", |_| {});
    assert!(
        matches!(none, Err(QueryFailure::InvalidQuery(_))),
        "{:?}",
        none.err()
    );
    fixture.daemon.shutdown().await;
}
