mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daemon::projection_gates::ProjectionHook;
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, Transition,
};
use daemon::query_route::{
    CompressedLimits, DenseLimits, DenseVectors, EmbedResult, QueryEmbedder, QueryRouteLimits,
};
use daemon::vector_admission::{Census, ResourceClass};
use daemon::vector_reader::RescoreEvent;
use host_runtime::{
    BindOutcome, Client, ClientRoute, CompositeComponent, HealthReport, HostConfig, HostHandler,
    HostInit, InitError, ManifestSnapshot, RequestCtx, RequestOptions, RequestOutcome,
    ResourceDeclaration, RouteHandle, RouteIdentity, RouteTarget, TargetKind,
};
use kernel::KernelStore;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{CandidatePolicy, ScanBounds, StorageBounds, Window};
use retrieval::fusion::{FusionParameters, LaneWeights};
use serde_json::{Value, json};
use support::embedding_fixtures::{generation, identity, kernel_incarnation_id};
use support::kernel_daemon::{KernelDaemon, SESSION};
use support::projection_gate::{campaign_json, manifest_json_with, open_gate, write_records};
use support::vector_reads::acquire_view;
use support::vector_store::Fixture as Vectors;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn route_limits() -> QueryRouteLimits {
    QueryRouteLimits {
        query_bytes: NonZeroUsize::new(256).unwrap(),
        probes: NonZeroUsize::new(16).unwrap(),
        lexical_scan_rows: NonZeroUsize::new(256).unwrap(),
        lexical_accepted: NonZeroUsize::new(64).unwrap(),
        lexical_qualifying_matches: NonZeroUsize::new(20_000).unwrap(),
        lexical_rank_budget: NonZeroUsize::new(30_000).unwrap(),
        validation_batch: NonZeroUsize::new(16).unwrap(),
        exact_page_rows: NonZeroUsize::new(16).unwrap(),
        exact_pages: NonZeroUsize::new(4).unwrap(),
        fused_union: NonZeroUsize::new(64).unwrap(),
        result_rows: NonZeroUsize::new(32).unwrap(),
        response_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        deadline_ceiling: Duration::from_secs(20),
        fusion: FusionParameters::new(
            LaneWeights {
                exact: 1.0,
                lexical: 1.0,
                dense: 1.0,
            },
            60.0,
        )
        .unwrap(),
        dense: Some(DenseLimits {
            k: NonZeroUsize::new(4).unwrap(),
            page_rows: NonZeroUsize::new(4).unwrap(),
            max_rows: NonZeroUsize::new(64).unwrap(),
            unit_norm_tolerance: 1e-3,
        }),
    }
}

fn compressed_limits() -> CompressedLimits {
    CompressedLimits {
        candidates: CandidatePolicy {
            alpha: 2.0,
            cap: NonZeroUsize::new(64).unwrap(),
        },
        scan: ScanBounds {
            page_rows: NonZeroUsize::new(1).unwrap(),
            max_rows: NonZeroUsize::new(64).unwrap(),
            storage: StorageBounds {
                batch_bytes: NonZeroUsize::new(1 << 20).unwrap(),
                heap_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            },
        },
        max_entries: NonZeroUsize::new(64).unwrap(),
        max_layers: NonZeroUsize::new(4).unwrap(),
        max_pinned_bytes: u64::MAX,
        max_read_bytes: u64::MAX,
    }
}

fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    raw.iter().map(|x| (f64::from(*x) / norm) as f32).collect()
}

fn query_vector() -> Vec<f32> {
    unit([0.9, 0.1, 0.6, 0.2, 0.3, 0.7, 0.1, 0.4])
}

fn vector_for(occurrence_id: &str) -> Vec<f32> {
    let seed = occurrence_id.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    let raw: [f32; 8] = std::array::from_fn(|i| 0.2 + ((seed >> (i * 3)) & 0b111) as f32 * 0.1);
    unit(raw)
}

struct FixedEmbedder;

impl QueryEmbedder for FixedEmbedder {
    fn embed<'a>(
        &'a self,
        _text: &'a str,
        _deadline: tokio::time::Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = EmbedResult> + Send + 'a>> {
        Box::pin(async { Ok(query_vector()) })
    }
}

fn request(project: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "method": "retrieval.query",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "query": "id:rule explicit contract",
        "remaining_ms": 10_000,
        "destination": "local",
    }))
    .unwrap()
}

async fn served_daemon() -> (KernelDaemon, Vec<String>) {
    let data = tempfile::tempdir().unwrap();
    let home = data.path().to_owned();
    let kernel_root = home.join("eidnara").join("context");
    drop(KernelStore::open(kernel_root.join("kernel")).unwrap());
    let incarnation = kernel_incarnation_id(&kernel_root);
    let identity = identity(&incarnation);
    write_records(
        &home,
        &manifest_json_with(
            &identity,
            &ProjectionHook::ALL,
            &[("catchup_lag_commits", 1_000)],
        ),
        &campaign_json(&identity),
    );
    let lifecycle = ProjectionLifecycle::open(&home).unwrap();
    lifecycle
        .record(
            &open_gate(),
            &LifecycleRequest {
                transition: Transition::Rebuilding,
                selected_generation: "unregistered".to_owned(),
                kernel_incarnation_id: incarnation,
                consumer: ConsumerBinding {
                    consumer_id: "search-lifecycle".to_owned(),
                    generation_id: support::embedding_fixtures::GENERATION.to_owned(),
                },
                cause: Cause::DeletedAfterPruning,
                attempt_id: "rebuild-attempt".to_owned(),
                recovery_target: None,
                allowance: 3,
                deadline: now() + 60_000,
                authorization_ref: None,
            },
            now(),
        )
        .unwrap();
    let daemon = KernelDaemon::start_in(data, None).await;
    let mut lexical = route_limits();
    lexical.dense = None;
    daemon
        .handler()
        .set_query_route_limits(Some(lexical))
        .unwrap();
    let started = Instant::now();
    while !matches!(lifecycle.read(), ControlState::Current(_)) {
        assert!(started.elapsed() < Duration::from_secs(30));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let created = daemon
        .commit(
            "create",
            vec![json!({"op": "insert_decision", "spec": {
                "decision_id": "rule-decision",
                "object_id": "rule",
                "domain_id": "memory",
                "decision_kind": "PROJECT_RULES",
                "payload": {"summary": "Keep the public contract explicit.", "rationale": "because rule"},
                "source_id": "rule-lineage",
                "source_revision": 1,
            }})],
        )
        .await;
    assert_eq!(created["state"]["kind"], "available", "{created}");
    daemon.handler().resume_claim_sources_for_test();
    let project = daemon.project().to_owned();
    let started = Instant::now();
    let ids = loop {
        let outcome = daemon
            .outcome(serde_json::from_slice(&request(&project)).unwrap())
            .await;
        let daemon::dispatch::PreparedOutcome::Response(output) = outcome else {
            panic!("the route answers with a response");
        };
        let fused = output.json_for_test().unwrap().clone();
        if fused["kind"] == "fused" && !fused["entries"].as_array().unwrap().is_empty() {
            break fused["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["occurrence_id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{fused}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    (daemon, ids)
}

fn composition(
    incarnation: &str,
    ids: &[String],
) -> (Vectors, Arc<daemon::vector_reader::PinnedVectors>) {
    let mut store = Vectors::for_projection(identity(incarnation), generation());
    let mut rows: Vec<ExportedRow> = ids
        .iter()
        .map(|id| ExportedRow {
            occurrence_id: id.clone(),
            vector: vector_for(id),
        })
        .collect();
    rows.sort_by(|a, b| a.occurrence_id.cmp(&b.occurrence_id));
    rows.dedup_by(|a, b| a.occurrence_id == b.occurrence_id);
    let export = LiveRows {
        generation: generation(),
        kernel_incarnation_id: incarnation.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 1,
            checkpoint_commit_seq: 2,
            hold_id: "hold".to_owned(),
        },
        rows,
        tombstones: Vec::new(),
    };
    let base = store.layer_from(&export);
    store
        .publish(&store.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut store, &mut |_| {}).unwrap();
    (store, view)
}

struct Hosted {
    daemon: Arc<KernelDaemon>,
    activated: Mutex<Option<oneshot::Sender<()>>>,
}

impl HostHandler for Hosted {
    fn manifests(&self) -> Vec<ManifestSnapshot> {
        vec![CompositeComponent::manifest(self.daemon.handler())]
    }

    fn resource_declarations(&self) -> Vec<ResourceDeclaration> {
        vec![CompositeComponent::resources(self.daemon.handler())]
    }

    async fn initialize(&self, _init: HostInit) -> Result<(), InitError> {
        Ok(())
    }

    async fn activate(&self) -> Result<(), InitError> {
        if let Some(sender) = self.activated.lock().unwrap().take() {
            let _ = sender.send(());
        }
        Ok(())
    }

    async fn bind(
        &self,
        route: RouteHandle,
        _target: RouteTarget,
        identity: RouteIdentity,
    ) -> BindOutcome {
        CompositeComponent::bind(self.daemon.handler(), route, identity).await
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        CompositeComponent::handle(self.daemon.handler(), ctx).await
    }

    async fn route_gone(&self, route: RouteHandle) {
        CompositeComponent::route_gone(self.daemon.handler(), route).await;
    }

    async fn health(&self) -> HealthReport {
        CompositeComponent::health(self.daemon.handler()).await
    }

    async fn shutdown(&self) {}
}

struct Host {
    client: Client,
    route: ClientRoute,
    published: Arc<Mutex<Vec<(Instant, Census)>>>,
    join: tokio::task::JoinHandle<Result<(), host_runtime::HostError>>,
    stop: CancellationToken,
    _dir: tempfile::TempDir,
}

async fn watchdog<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("the operation finishes before the watchdog")
}

impl Host {
    async fn start(
        daemon: Arc<KernelDaemon>,
        ledger: Arc<daemon::vector_admission::Ledger>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (activated, ready) = oneshot::channel();
        let project = daemon.project().to_owned();
        let module_id = CompositeComponent::manifest(daemon.handler()).module_id;
        let hosted = Hosted {
            daemon,
            activated: Mutex::new(Some(activated)),
        };
        let mut config = HostConfig {
            data_dir: Some(dir.path().join("host")),
            ..HostConfig::default()
        };
        config.limits.max_resident_bytes = u64::from(u32::MAX);
        config.timing.route_close_budget = Duration::from_secs(5);
        config.timing.shutdown_deadline = Duration::from_secs(10);
        let publication = host_runtime::runtime_dir_path(config.data_dir.as_deref())
            .unwrap()
            .join(host_runtime::CONNECTION_FILE_NAME);
        let published = Arc::new(Mutex::new(Vec::new()));
        let hook = {
            let published = Arc::clone(&published);
            Arc::new(move |kind, channel| {
                if kind == host_runtime::wire::FrameType::Error && channel != 0 {
                    published
                        .lock()
                        .unwrap()
                        .push((Instant::now(), ledger.census()));
                }
            })
        };
        let stop = CancellationToken::new();
        let mut join = tokio::spawn(host_runtime::run_with_publish_hook(
            hosted,
            config,
            stop.clone(),
            Some(hook),
        ));
        watchdog(async {
            tokio::select! {
                result = ready => {
                    if result.is_err() {
                        panic!("the host stopped before activation: {:?}", (&mut join).await);
                    }
                }
                result = &mut join => panic!("the host stopped before activation: {result:?}"),
            }
        })
        .await;
        let client = watchdog(Client::connect(publication)).await.unwrap();
        let route = watchdog(client.open_route(
            RouteTarget {
                module_id,
                kind: TargetKind::ToolProvider,
            },
            RouteIdentity {
                project_root: project,
                harness: "test".to_owned(),
                session: SESSION.to_owned(),
                consumer_module_id: None,
                consumer_launch_nonce: None,
                consumer_capabilities: Vec::new(),
                admission_facts: None,
                credential_fingerprints: Default::default(),
            },
        ))
        .await
        .unwrap();
        Self {
            client,
            route,
            published,
            join,
            stop,
            _dir: dir,
        }
    }

    async fn stop(self) {
        watchdog(self.client.close()).await.unwrap();
        self.stop.cancel();
        watchdog(self.join)
            .await
            .unwrap()
            .expect("the host stops cleanly");
    }
}

type Stage = fn(&RescoreEvent<'_>) -> bool;

struct Hold {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    census: Arc<Mutex<Option<(Census, Instant)>>>,
    after_release: Arc<std::sync::atomic::AtomicUsize>,
}

fn hold_at(
    vectors: DenseVectors,
    ledger: Arc<daemon::vector_admission::Ledger>,
    stage: Stage,
) -> (DenseVectors, Hold) {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(Some(release_rx));
    let census = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&census);
    let after_release = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&after_release);
    let observed = vectors.with_observer_for_test(Arc::new(move |event| {
        if recorded.lock().unwrap().is_some() {
            if matches!(
                event,
                RescoreEvent::Scan(Window::Visited(_) | Window::AfterJudgment)
                    | RescoreEvent::AfterSelection
                    | RescoreEvent::ReadOriginal { .. }
            ) {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            return;
        }
        if !stage(&event) {
            return;
        }
        let Some(release) = release_rx.lock().unwrap().take() else {
            return;
        };
        let _ = entered_tx.send(());
        let _ = release.recv();
        *recorded.lock().unwrap() = Some((ledger.census(), Instant::now()));
    }));
    (
        observed,
        Hold {
            entered,
            release,
            census,
            after_release,
        },
    )
}

fn json_of(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_cancellation_reaches_the_dense_scan_validation_and_original_reads_and_the_work_joins_before_the_request_settles()
 {
    let (daemon, ids) = served_daemon().await;
    let incarnation = daemon
        .store()
        .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::unbounded())
        .unwrap();
    let (store, view) = composition(&incarnation, &ids);
    daemon
        .handler()
        .set_query_route_limits(Some(route_limits()))
        .unwrap();
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::new(FixedEmbedder)));
    let vectors = DenseVectors::new(
        Arc::clone(&view),
        store.admission.clone(),
        compressed_limits(),
    );
    daemon
        .handler()
        .set_dense_vectors(Some(vectors.clone()))
        .unwrap();
    let daemon = Arc::new(daemon);
    let host = Host::start(Arc::clone(&daemon), Arc::clone(&store.ledger)).await;
    let project = daemon.project().to_owned();

    let served = watchdog(host.client.request(
        host.route,
        request(&project),
        RequestOptions::default(),
    ))
    .await
    .unwrap();
    let served = json_of(&served.body);
    assert_eq!(served["kind"], "fused", "{served}");
    assert_ne!(
        served["lanes"]["dense"]["status"], "unavailable",
        "{served}"
    );
    assert!(
        served["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| !entry["lanes"]["dense"].is_null()),
        "a dense contribution reaches the answer: {served}"
    );
    assert_eq!(
        store.ledger.census().held.get(&ResourceClass::Scratch),
        None
    );

    let stages: [(&str, Stage); 3] = [
        ("scan", |event| {
            matches!(event, RescoreEvent::Scan(Window::Visited(_)))
        }),
        ("validation", |event| {
            matches!(event, RescoreEvent::Scan(Window::AfterJudgment))
        }),
        ("original read", |event| {
            matches!(event, RescoreEvent::ReadOriginal { .. })
        }),
    ];
    for (name, stage) in stages {
        let (held, hold) = hold_at(vectors.clone(), Arc::clone(&store.ledger), stage);
        daemon.handler().set_dense_vectors(Some(held)).unwrap();
        let errors_before = host.published.lock().unwrap().len();
        let token = CancellationToken::new();
        let Hold {
            entered,
            release,
            census,
            after_release,
        } = hold;
        let control = async {
            watchdog(tokio::task::spawn_blocking(move || {
                entered.recv_timeout(Duration::from_secs(20)).unwrap()
            }))
            .await
            .unwrap();
            token.cancel();
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                host.published.lock().unwrap().len(),
                errors_before,
                "{name}: the request settles only after its work ends"
            );
            release.send(()).unwrap();
        };
        let (outcome, ()) = watchdog(async {
            tokio::join!(
                host.client.request(
                    host.route,
                    request(&project),
                    RequestOptions {
                        cancellation: Some(token.clone()),
                        ..RequestOptions::default()
                    },
                ),
                control
            )
        })
        .await;
        assert!(
            outcome.is_err(),
            "{name}: a cancelled request answers no value"
        );
        let started = Instant::now();
        let (census, released_at) = loop {
            if let Some(recorded) = census.lock().unwrap().take() {
                break recorded;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "{name}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(census.held[&ResourceClass::Scratch] > 0, "{name}");
        assert!(census.held[&ResourceClass::RowBuffers] > 0, "{name}");
        assert!(census.pinned > 0, "{name}");
        let started = Instant::now();
        while store
            .ledger
            .census()
            .held
            .contains_key(&ResourceClass::Scratch)
        {
            assert!(started.elapsed() < Duration::from_secs(10), "{name}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !store
                .ledger
                .census()
                .held
                .contains_key(&ResourceClass::RowBuffers)
        );
        let started = Instant::now();
        let (published, census_at_publication) = loop {
            if let Some(published) = host.published.lock().unwrap().get(errors_before) {
                break published.clone();
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{name}: the cancelled request publishes its error"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(
            published >= released_at,
            "{name}: settled before the work ended"
        );
        for class in [ResourceClass::Scratch, ResourceClass::RowBuffers] {
            assert!(
                !census_at_publication.held.contains_key(&class),
                "{name}: {class:?} was still charged when the request settled"
            );
        }
        assert_eq!(
            after_release.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{name}: the work stopped at its next checkpoint"
        );
        assert!(!view.is_quarantined(), "{name}");
    }

    daemon
        .handler()
        .set_dense_vectors(Some(vectors.clone()))
        .unwrap();
    let again = watchdog(host.client.request(
        host.route,
        request(&project),
        RequestOptions::default(),
    ))
    .await
    .unwrap();
    assert_eq!(json_of(&again.body)["kind"], "fused");
    host.stop().await;

    let (held, hold) = hold_at(vectors, Arc::clone(&store.ledger), |event| {
        matches!(event, RescoreEvent::ReadOriginal { .. })
    });
    daemon.handler().set_dense_vectors(Some(held)).unwrap();
    let aborted = {
        let daemon = Arc::clone(&daemon);
        let body: Value = serde_json::from_slice(&request(&project)).unwrap();
        tokio::spawn(async move { daemon.outcome(body).await })
    };
    let Hold {
        entered,
        release,
        census,
        ..
    } = hold;
    watchdog(tokio::task::spawn_blocking(move || {
        entered.recv_timeout(Duration::from_secs(20)).unwrap()
    }))
    .await
    .unwrap();
    aborted.abort();
    assert!(aborted.await.unwrap_err().is_cancelled());
    daemon.handler().set_dense_vectors(None).unwrap();
    drop(view);
    let while_held = store.ledger.census();
    assert!(
        while_held.held[&ResourceClass::Scratch] > 0,
        "the aborted handler's work keeps its charges"
    );
    assert!(
        while_held.pinned > 0,
        "the unit's clone is the last one, and it still pins the view"
    );
    release.send(()).unwrap();
    let started = Instant::now();
    while census.lock().unwrap().is_none() || store.ledger.census().pinned > 0 {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the unit's exit releases the view"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(store.ledger.census().held.is_empty());
    let daemon = Arc::try_unwrap(daemon)
        .ok()
        .expect("the host released the daemon");
    daemon.shutdown().await;
}

/// A request that embeds no prose ranks no dense lane, so its unit holds no clone of the installed view: uninstalling the view while that unit is pending leaves the test's `Arc` as the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_that_ranks_no_dense_lane_holds_no_view() {
    let (daemon, ids) = served_daemon().await;
    let incarnation = daemon
        .store()
        .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::unbounded())
        .unwrap();
    let (store, view) = composition(&incarnation, &ids);
    daemon
        .handler()
        .set_query_route_limits(Some(route_limits()))
        .unwrap();
    daemon
        .handler()
        .set_query_embedder_for_test(Some(Arc::new(FixedEmbedder)));
    daemon
        .handler()
        .set_dense_vectors(Some(DenseVectors::new(
            Arc::clone(&view),
            store.admission.clone(),
            compressed_limits(),
        )))
        .unwrap();
    let project = daemon.project().to_owned();
    let daemon = Arc::new(daemon);

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let submitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pending = {
        let daemon = Arc::clone(&daemon);
        let gate = Arc::clone(&gate);
        let submitted = Arc::clone(&submitted);
        let mut body: Value = serde_json::from_slice(&request(&project)).unwrap();
        body["query"] = json!("id:rule");
        tokio::spawn(async move { daemon.outcome_gated(body, gate, submitted).await })
    };
    let started = Instant::now();
    while submitted.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the unit is submitted"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    daemon.handler().set_dense_vectors(None).unwrap();
    assert_eq!(
        Arc::strong_count(&view),
        1,
        "a request that ranks no dense lane holds no clone of the view"
    );
    gate.add_permits(1);
    let daemon::dispatch::PreparedOutcome::Response(output) = watchdog(pending).await.unwrap()
    else {
        panic!("the selector-only request answers");
    };
    let answer = output.json_for_test().unwrap().clone();
    assert_eq!(answer["kind"], "fused", "{answer}");
    assert_eq!(answer["lanes"]["dense"]["status"], "undeclared", "{answer}");
    let daemon = Arc::try_unwrap(daemon)
        .ok()
        .expect("the request released the daemon");
    daemon.shutdown().await;
}
