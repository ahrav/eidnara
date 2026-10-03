mod support;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::Handler;
use daemon::query_route::{
    CompressedLimits, CompressedProducer, DenseLane, DenseLimits, DenseVectors, LaneStatus,
    LimitsRefusal, QueryFailure, QueryOutcome, QueryRouteLimits, Terminal, execute,
};
use daemon::request_budget::{Exhaustion, RequestBudget};
use daemon::vector_admission::ResourceClass;
use daemon::vector_generation::ROWS_FILE;
use daemon::vector_reader::{PinnedVectors, RescoreEvent};
use kernel::ArtifactDestination;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::codec::ARTIFACT_HEADER_BYTES;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{CandidatePolicy, CapacityRefusal, ScanBounds, StorageBounds, Window};
use retrieval::eligibility::Authority;
use retrieval::fusion::{Lane, RawScore};
use support::query_route::{
    Fixture, GENERATION, QUERY, dense_limits, dense_reference, generation, limits,
    projection_identity, query_vector, request_budget, vector_for,
};
use support::vector_reads::{acquire_view, held};
use support::vector_store::Fixture as Vectors;

fn compressed_limits() -> CompressedLimits {
    CompressedLimits {
        candidates: CandidatePolicy {
            alpha: 2.0,
            cap: NonZeroUsize::new(64).unwrap(),
        },
        scan: ScanBounds {
            page_rows: NonZeroUsize::new(2).unwrap(),
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

struct Composition {
    store: Vectors,
    view: Arc<PinnedVectors>,
    rows: Vec<(String, Vec<f32>)>,
}

impl Composition {
    fn of(fixture: &Fixture) -> Self {
        let mut store = Vectors::for_projection(
            projection_identity(&fixture.kernel_incarnation),
            generation(),
        );
        let mut rows: Vec<(String, Vec<f32>)> = fixture
            .live_candidates()
            .into_iter()
            .map(|candidate| {
                let vector = vector_for(&candidate.occurrence_id);
                (candidate.occurrence_id, vector)
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(rows.len() >= 3);
        let export = LiveRows {
            generation: generation(),
            kernel_incarnation_id: fixture.kernel_incarnation.clone(),
            checkpoint: ProjectionCheckpoint {
                snapshot_commit_seq: 1,
                checkpoint_commit_seq: 2,
                hold_id: "hold".to_owned(),
            },
            rows: rows
                .iter()
                .map(|(occurrence_id, vector)| ExportedRow {
                    occurrence_id: occurrence_id.clone(),
                    vector: vector.clone(),
                })
                .collect(),
            tombstones: Vec::new(),
        };
        let base = store.layer_from(&export);
        store
            .publish(&store.compose(1, &base, &[]).unwrap())
            .unwrap();
        let view = acquire_view(&mut store, &mut |_| {}).unwrap();
        Self { store, view, rows }
    }

    fn vectors(&self) -> DenseVectors {
        DenseVectors::new(
            Arc::clone(&self.view),
            self.store.admission.clone(),
            compressed_limits(),
        )
    }

    fn observed(&self, observe: impl Fn(RescoreEvent<'_>) + Send + Sync + 'static) -> DenseVectors {
        self.vectors().with_observer_for_test(Arc::new(observe))
    }

    fn charged(&self, class: ResourceClass) -> u64 {
        held(&self.store.ledger, class)
    }
}

fn with_dense(k: usize) -> QueryRouteLimits {
    let mut limits = limits();
    limits.dense = Some(dense_limits(k));
    limits
}

fn run(
    fixture: &Fixture,
    k: usize,
    vectors: DenseVectors,
    budget: &RequestBudget,
) -> Result<QueryOutcome, QueryFailure> {
    let producer = CompressedProducer {
        limits: dense_limits(k),
        vectors,
    };
    let query = query_vector();
    execute(
        &fixture.projection,
        &fixture.store,
        Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
        },
        &with_dense(k),
        budget.shared(),
        QUERY,
        DenseLane::Ready {
            query: &query,
            generation_id: GENERATION,
            producer: &producer,
        },
        |_| {},
    )
}

fn dense_positions(outcome: &QueryOutcome) -> Vec<(String, usize, u64)> {
    let mut positions: Vec<(String, usize, u64)> = outcome
        .fused
        .entries()
        .iter()
        .filter_map(|entry| {
            entry.lane(Lane::Dense).map(|contribution| {
                let RawScore::Dense(raw) = contribution.raw_score() else {
                    panic!("a dense contribution carries a dense score");
                };
                (
                    entry.occurrence().to_string(),
                    contribution.position().get(),
                    raw.to_bits(),
                )
            })
        })
        .collect();
    positions.sort_by_key(|(_, position, _)| *position);
    positions
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_compressed_producer_serves_the_dense_lane_with_exact_original_scores() {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);
    let (_token, budget) = request_budget(10_000);
    let mut wide = compressed_limits();
    wide.candidates.alpha = 32.0;
    let vectors = DenseVectors::new(
        Arc::clone(&composition.view),
        composition.store.admission.clone(),
        wide,
    );
    let outcome = run(&fixture, 2, vectors, &budget).unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Complete);
    assert_eq!(outcome.body["lanes"]["dense"]["status"], "complete");
    assert_eq!(outcome.body["degraded"], false, "{}", outcome.body);
    assert!(
        composition.rows.len() <= 64,
        "the pool of sixty-four holds every live row"
    );
    let mut expected = dense_reference(&query_vector(), &composition.rows);
    expected.truncate(2);
    let positions = dense_positions(&outcome);
    assert_eq!(positions.len(), 2);
    for ((id, position, raw), (expected_id, expected_score)) in positions.iter().zip(&expected) {
        assert_eq!(id, expected_id, "dense position {position}");
        assert_eq!(*raw, expected_score.to_bits());
    }
    assert_eq!(composition.charged(ResourceClass::Scratch), 0);
    assert_eq!(composition.charged(ResourceClass::RowBuffers), 0);
    fixture.daemon.shutdown().await;
}

#[derive(Clone, Copy, Debug)]
enum Stage {
    Scan,
    Validation,
    OriginalRead,
}

impl Stage {
    fn matches(self, event: &RescoreEvent<'_>) -> bool {
        matches!(
            (self, event),
            (Self::Scan, RescoreEvent::Scan(Window::Visited(_)))
                | (Self::Validation, RescoreEvent::Scan(Window::AfterJudgment))
                | (Self::OriginalRead, RescoreEvent::ReadOriginal { .. })
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_at_each_stage_ends_the_request_on_the_original_budget_and_releases_its_charges()
 {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);
    for stage in [Stage::Scan, Stage::Validation, Stage::OriginalRead] {
        let (token, budget) = request_budget(10_000);
        let deadline = budget.deadline();
        let fired = Arc::new(Mutex::new(None));
        let vectors = {
            let fired = Arc::clone(&fired);
            let token = token.clone();
            let ledger = Arc::clone(&composition.store.ledger);
            composition.observed(move |event| {
                let mut fired = fired.lock().unwrap();
                if fired.is_none() && stage.matches(&event) {
                    token.cancel();
                    *fired = Some(ledger.census());
                }
            })
        };
        let outcome = run(&fixture, 2, vectors, &budget);
        assert!(
            matches!(outcome, Err(QueryFailure::Terminal(Terminal::Cancelled))),
            "{stage:?}: {:?}",
            outcome.err()
        );
        assert_eq!(budget.shared().exhaustion(), Some(Exhaustion::Cancelled));
        assert_eq!(budget.deadline(), deadline, "the deadline derived first");
        let at_cancel = fired.lock().unwrap().take().expect("the stage was reached");
        assert!(at_cancel.held[&ResourceClass::Scratch] > 0, "{stage:?}");
        assert!(at_cancel.held[&ResourceClass::RowBuffers] > 0, "{stage:?}");
        assert!(at_cancel.pinned > 0);
        assert_eq!(composition.charged(ResourceClass::Scratch), 0, "{stage:?}");
        assert_eq!(
            composition.charged(ResourceClass::RowBuffers),
            0,
            "{stage:?}"
        );
        assert!(!composition.view.is_quarantined(), "{stage:?}");
    }
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_that_lapses_inside_the_rescore_is_the_original_deadline() {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);
    let (_token, budget) = request_budget(300);
    let vectors = composition.observed(|event| {
        if matches!(event, RescoreEvent::AfterSelection) {
            std::thread::sleep(Duration::from_millis(400));
        }
    });
    let deadline = budget.deadline();
    let outcome = run(&fixture, 2, vectors, &budget);
    assert!(
        matches!(outcome, Err(QueryFailure::Terminal(Terminal::Deadline))),
        "{:?}",
        outcome.err()
    );
    assert_eq!(budget.shared().exhaustion(), Some(Exhaustion::Deadline));
    assert_eq!(budget.deadline(), deadline);
    assert_eq!(composition.charged(ResourceClass::Scratch), 0);
    assert_eq!(composition.charged(ResourceClass::RowBuffers), 0);
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_cancellation_of_one_request_leaves_the_next_on_the_reused_connection_complete() {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);
    let (token_a, budget_a) = request_budget(10_000);
    let vectors = {
        let token = token_a.clone();
        composition.observed(move |event| {
            if matches!(event, RescoreEvent::Scan(Window::Visited(_))) {
                token.cancel();
            }
        })
    };
    assert!(matches!(
        run(&fixture, 2, vectors, &budget_a),
        Err(QueryFailure::Terminal(Terminal::Cancelled))
    ));

    let (_token_b, budget_b) = request_budget(10_000);
    let vectors = {
        let token = token_a.clone();
        composition.observed(move |event| {
            if matches!(event, RescoreEvent::ReadOriginal { .. }) {
                token.cancel();
            }
        })
    };
    let outcome = run(&fixture, 2, vectors, &budget_b).unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Complete);
    assert!(!budget_b.shared().is_exhausted());
    assert_eq!(dense_positions(&outcome).len(), 2);

    let (token_c, budget_c) = request_budget(10_000);
    let served = run(&fixture, 2, composition.vectors(), &budget_c).unwrap();
    assert_eq!(served.statuses[2], LaneStatus::Complete);
    let (_token_d, budget_d) = request_budget(10_000);
    let vectors = {
        let token = token_c.clone();
        composition.observed(move |event| {
            if matches!(event, RescoreEvent::ReadOriginal { .. }) {
                token.cancel();
            }
        })
    };
    let outcome = run(&fixture, 2, vectors, &budget_d).unwrap();
    assert!(token_c.is_cancelled());
    assert_eq!(outcome.statuses[2], LaneStatus::Complete);
    assert!(!budget_d.shared().is_exhausted());
    assert_eq!(
        served.statuses[2],
        LaneStatus::Complete,
        "C's answer stands"
    );
    assert_eq!(budget_c.shared().exhaustion(), Some(Exhaustion::Cancelled));
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_view_and_scan_bound_degrades_the_lane_with_its_own_reason() {
    let fixture = Fixture::build().await;
    let mut composition = Composition::of(&fixture);
    let degraded = |composition: &Composition, limits: CompressedLimits| {
        let vectors = DenseVectors::new(
            Arc::clone(&composition.view),
            composition.store.admission.clone(),
            limits,
        );
        let (_token, budget) = request_budget(10_000);
        run(&fixture, 2, vectors, &budget).unwrap().statuses[2].clone()
    };
    let mut pinned = compressed_limits();
    pinned.max_pinned_bytes = 1;
    assert_eq!(
        degraded(&composition, pinned),
        LaneStatus::Unavailable("view_bound")
    );
    let mut rows = compressed_limits();
    rows.scan.max_rows = NonZeroUsize::new(1).unwrap();
    assert_eq!(
        degraded(&composition, rows),
        LaneStatus::Unavailable("row_bound")
    );
    let mut heap = compressed_limits();
    heap.scan.storage.heap_bytes = NonZeroUsize::new(1).unwrap();
    assert_eq!(
        degraded(&composition, heap),
        LaneStatus::Unavailable("heap_over_bound")
    );
    let mut batch = compressed_limits();
    batch.scan.storage.batch_bytes = NonZeroUsize::new(1).unwrap();
    assert_eq!(
        degraded(&composition, batch),
        LaneStatus::Unavailable("batch_bytes")
    );
    let tables = composition.store.ledger.census().resident;
    composition
        .store
        .set_limit(daemon::vector_admission::RESIDENT_LIMIT, tables);
    let vectors = DenseVectors::new(
        Arc::clone(&composition.view),
        composition.store.admission.clone(),
        compressed_limits(),
    );
    let (_token, budget) = request_budget(10_000);
    assert_eq!(
        run(&fixture, 2, vectors, &budget).unwrap().statuses[2].clone(),
        LaneStatus::Unavailable("reservation")
    );
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_of_another_kernel_incarnation_degrades_the_lane() {
    let fixture = Fixture::build().await;
    let mut store = Vectors::for_projection(projection_identity("another"), generation());
    let rows: Vec<ExportedRow> = {
        let mut ids: Vec<String> = fixture
            .live_candidates()
            .into_iter()
            .map(|candidate| candidate.occurrence_id)
            .collect();
        ids.sort();
        ids.into_iter()
            .map(|id| ExportedRow {
                vector: vector_for(&id),
                occurrence_id: id,
            })
            .collect()
    };
    let base = store.layer_from(&LiveRows {
        generation: generation(),
        kernel_incarnation_id: "another".to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 1,
            checkpoint_commit_seq: 2,
            hold_id: "hold".to_owned(),
        },
        rows,
        tombstones: Vec::new(),
    });
    store
        .publish(&store.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut store, &mut |_| {}).unwrap();
    let vectors = DenseVectors::new(view, store.admission.clone(), compressed_limits());
    let (_token, budget) = request_budget(10_000);
    let outcome = run(&fixture, 2, vectors, &budget).unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Unavailable("identity"));
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn view_bounds_degrade_the_lane_and_a_missing_original_ends_the_request_and_quarantines() {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);

    let mut tight = compressed_limits();
    tight.max_read_bytes = 1;
    let vectors = DenseVectors::new(
        Arc::clone(&composition.view),
        composition.store.admission.clone(),
        tight,
    );
    let (_token, budget) = request_budget(10_000);
    let outcome = run(&fixture, 2, vectors, &budget).unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Unavailable("view_bound"));
    assert_eq!(outcome.body["degraded"], true);

    let rows = composition
        .store
        .generation_dir(&composition.view.members()[0])
        .join(ROWS_FILE);
    let vectors = composition.observed(move |event| {
        if matches!(event, RescoreEvent::AfterSelection) {
            let bytes = std::fs::read(&rows).unwrap();
            std::fs::write(&rows, &bytes[..ARTIFACT_HEADER_BYTES]).unwrap();
        }
    });
    let (_token, budget) = request_budget(10_000);
    assert!(matches!(
        run(&fixture, 2, vectors, &budget),
        Err(QueryFailure::Unavailable("dense_corruption"))
    ));
    assert!(composition.view.is_quarantined());
    let (_token, budget) = request_budget(10_000);
    let outcome = run(&fixture, 2, composition.vectors(), &budget).unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Unavailable("quarantined"));
    fixture.daemon.shutdown().await;
}

#[test]
fn the_pool_the_limits_give_is_checked_before_a_view_is_installed() {
    let dense = DenseLimits {
        k: NonZeroUsize::new(64).unwrap(),
        ..dense_limits(64)
    };
    let mut limits = compressed_limits();
    limits.candidates = CandidatePolicy {
        alpha: 4.0,
        cap: NonZeroUsize::new(256).unwrap(),
    };
    assert_eq!(limits.capacity(&dense).unwrap().candidates().get(), 256);
    limits.candidates.alpha = 0.5;
    assert_eq!(
        limits.capacity(&dense),
        Err(LimitsRefusal::DenseCapacity(CapacityRefusal::Alpha {
            alpha: 0.5
        }))
    );
    limits.candidates = CandidatePolicy {
        alpha: 32.0,
        cap: NonZeroUsize::new(4096).unwrap(),
    };
    assert_eq!(
        limits.capacity(&dense),
        Err(LimitsRefusal::Dense {
            bound: "candidates",
            value: 2048
        }),
        "the kernel re-judges the pool in one batch"
    );
    let mut limits = compressed_limits();
    limits.candidates.cap = NonZeroUsize::new(128).unwrap();
    assert!(limits.capacity(&dense).is_ok());
    limits.scan.page_rows = NonZeroUsize::new(2000).unwrap();
    assert!(matches!(
        limits.capacity(&dense),
        Err(LimitsRefusal::Dense {
            bound: "scan_page_rows",
            ..
        })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vectors_install_only_under_declared_dense_limits() {
    let fixture = Fixture::build().await;
    let composition = Composition::of(&fixture);
    let handler = Handler::new();
    assert!(matches!(
        handler.set_dense_vectors(Some(composition.vectors())),
        Err(LimitsRefusal::DenseUndeclared)
    ));
    handler.set_query_route_limits(Some(with_dense(2))).unwrap();
    handler
        .set_dense_vectors(Some(composition.vectors()))
        .unwrap();
    let mut wide = compressed_limits();
    wide.candidates.alpha = 4096.0;
    wide.candidates.cap = NonZeroUsize::new(1 << 20).unwrap();
    assert!(matches!(
        handler.set_dense_vectors(Some(DenseVectors::new(
            Arc::clone(&composition.view),
            composition.store.admission.clone(),
            wide
        ))),
        Err(LimitsRefusal::Dense { .. })
    ));
    let mut larger = with_dense(2);
    larger.dense.as_mut().unwrap().k = NonZeroUsize::new(1000).unwrap();
    assert!(
        matches!(
            handler.set_query_route_limits(Some(larger)),
            Err(LimitsRefusal::DenseCapacity(
                CapacityRefusal::OverCap { .. }
            ))
        ),
        "route limits are checked against the installed vectors"
    );
    assert!(matches!(
        handler.set_query_route_limits(Some(limits())),
        Err(LimitsRefusal::DenseUndeclared)
    ));
    handler.set_dense_vectors(None).unwrap();
    handler.set_query_route_limits(Some(limits())).unwrap();
    fixture.daemon.shutdown().await;
}
