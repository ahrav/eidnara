mod support;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use daemon::query_route::{
    DenseLane, DenseLimits, DenseProducer, DenseRanking, DenseRefusal, DenseRequest,
    ExhaustiveProducer, LaneStatus, LimitsRefusal, Phase, QueryFailure, QueryOutcome,
    QueryRouteLimits, Terminal, execute,
};
use daemon::request_budget::SharedBudget;
use kernel::{ArtifactDestination, KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::dense::codec;
use retrieval::eligibility::Authority;
use retrieval::fusion::{Lane, RawScore};
use storage::GuardedConn;
use support::query_route::{
    DIMENSION, Fixture, GENERATION, QUERY, dense_reference, entry_ids, limits, request_budget, unit,
};
use tokio_util::sync::CancellationToken;

fn dense_limits() -> DenseLimits {
    DenseLimits {
        k: NonZeroUsize::new(8).unwrap(),
        page_rows: NonZeroUsize::new(4).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        unit_norm_tolerance: 1e-3,
    }
}

fn with_dense() -> QueryRouteLimits {
    let mut limits = limits();
    limits.dense = Some(dense_limits());
    limits
}

fn vector_for(occurrence_id: &str) -> Vec<f32> {
    let seed = occurrence_id.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    let mut raw = [0.0f32; DIMENSION as usize];
    for (i, slot) in raw.iter_mut().enumerate() {
        let bit = (seed >> (i * 3)) & 0b111;
        *slot = 0.2 + bit as f32 * 0.1;
    }
    unit(raw)
}

fn query_vector() -> Vec<f32> {
    unit([0.9, 0.1, 0.6, 0.2, 0.3, 0.7, 0.1, 0.4])
}

fn run(
    fixture: &Fixture,
    limits: &QueryRouteLimits,
    query_vector: &[f32],
    producer: &dyn DenseProducer,
    shared: &SharedBudget,
    before_phase: impl FnMut(Phase),
) -> Result<QueryOutcome, QueryFailure> {
    execute(
        &fixture.projection,
        &fixture.store,
        Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
        },
        limits,
        shared,
        QUERY,
        DenseLane::Ready {
            query: query_vector,
            generation_id: GENERATION,
            producer,
        },
        before_phase,
    )
}

fn degraded(fixture: &Fixture, shared: &SharedBudget, reason: &'static str) -> QueryOutcome {
    execute(
        &fixture.projection,
        &fixture.store,
        Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
        },
        &with_dense(),
        shared,
        QUERY,
        DenseLane::Unavailable(reason),
        |_| {},
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dense_positions_follow_the_producer_and_the_fused_order_follows_the_oracle() {
    let fixture = Fixture::build().await;
    let rows = fixture.store_vectors(vector_for);
    assert!(rows.len() >= 3);
    let query = query_vector();
    let mut reference = dense_reference(&query, &rows);
    reference.truncate(dense_limits().k.get());
    let producer = ExhaustiveProducer {
        limits: dense_limits(),
    };
    let (_token, budget) = request_budget(10_000);
    let outcome = run(
        &fixture,
        &with_dense(),
        &query,
        &producer,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.body["degraded"], false, "{}", outcome.body);
    assert_eq!(outcome.statuses[2], LaneStatus::Complete);
    assert_eq!(outcome.body["lanes"]["dense"]["status"], "complete");
    let dense_positions: Vec<(String, usize, f64)> = outcome
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
                    raw,
                )
            })
        })
        .collect();
    assert_eq!(dense_positions.len(), reference.len());
    for (id, position, raw) in &dense_positions {
        let (expected_id, expected_score) = &reference[*position - 1];
        assert_eq!(id, expected_id, "dense position {position}");
        assert_eq!(raw.to_bits(), expected_score.to_bits());
    }
    assert!(
        entry_ids(&outcome.body).len() >= reference.len(),
        "{}",
        outcome.body
    );
    let first = &outcome.body["entries"][0];
    assert_eq!(
        first["lanes"]["dense"]["raw"].as_f64().is_some(),
        outcome.fused.entries()[0].lane(Lane::Dense).is_some()
    );
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_embedding_lane_degrades_to_a_nonempty_exact_and_lexical_answer() {
    let fixture = Fixture::build().await;
    fixture.store_vectors(vector_for);
    let (_token, budget) = request_budget(10_000);
    let started = Instant::now();
    for reason in ["busy", "starting", "disabled", "failing", "input"] {
        let outcome = degraded(&fixture, budget.shared(), reason);
        assert_eq!(outcome.body["kind"], "fused");
        assert_eq!(outcome.body["degraded"], true);
        assert_eq!(outcome.statuses[2], LaneStatus::Unavailable(reason));
        assert_eq!(outcome.body["lanes"]["dense"]["status"], "unavailable");
        assert_eq!(outcome.body["lanes"]["dense"]["reason"], reason);
        assert!(!entry_ids(&outcome.body).is_empty(), "{}", outcome.body);
        assert!(
            outcome
                .fused
                .entries()
                .iter()
                .all(|entry| entry.lane(Lane::Dense).is_none())
        );
        assert!(
            outcome
                .fused
                .entries()
                .iter()
                .any(|entry| entry.lane(Lane::Exact).is_some())
        );
    }
    assert!(
        Instant::now() < budget.deadline(),
        "{:?}",
        started.elapsed()
    );
    assert!(!budget.is_exhausted());
    fixture.daemon.shutdown().await;
}

struct Cancelling {
    inner: ExhaustiveProducer,
    token: CancellationToken,
}

impl DenseProducer for Cancelling {
    fn rank(
        &self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: DenseRequest<'_>,
    ) -> Result<DenseRanking, DenseRefusal> {
        self.token.cancel();
        self.inner.rank(conn, kernel, request)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_deadline_during_the_dense_scan_end_typed_without_a_ranking() {
    let fixture = Fixture::build().await;
    fixture.store_vectors(vector_for);
    let query = query_vector();
    let producer = ExhaustiveProducer {
        limits: dense_limits(),
    };
    let (token, budget) = request_budget(10_000);
    let mut reached = Vec::new();
    let outcome = run(
        &fixture,
        &with_dense(),
        &query,
        &producer,
        budget.shared(),
        |phase| {
            reached.push(phase);
            if phase == Phase::Dense {
                token.cancel();
            }
        },
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Terminal(Terminal::Cancelled))
    );
    assert_eq!(reached.last(), Some(&Phase::Dense));

    let (_token, budget) = request_budget(600);
    let mut reached = Vec::new();
    let outcome = run(
        &fixture,
        &with_dense(),
        &query,
        &producer,
        budget.shared(),
        |phase| {
            reached.push(phase);
            if phase == Phase::Dense {
                std::thread::sleep(Duration::from_millis(700));
            }
        },
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Terminal(Terminal::Deadline))
    );
    assert_eq!(reached.last(), Some(&Phase::Dense));

    let (token, budget) = request_budget(10_000);
    let cancelling = Cancelling {
        inner: ExhaustiveProducer {
            limits: dense_limits(),
        },
        token,
    };
    let outcome = run(
        &fixture,
        &with_dense(),
        &query,
        &cancelling,
        budget.shared(),
        |_| {},
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Terminal(Terminal::Cancelled)),
        "a cancellation raised inside the producer is the budget's verdict"
    );
    fixture.daemon.shutdown().await;
}

struct Counting {
    inner: ExhaustiveProducer,
    calls: Arc<AtomicUsize>,
}

impl DenseProducer for Counting {
    fn rank(
        &self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: DenseRequest<'_>,
    ) -> Result<DenseRanking, DenseRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.rank(conn, kernel, request)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn producer_corruption_is_typed_while_a_foreign_query_shape_degrades_the_lane() {
    let fixture = Fixture::build().await;
    let rows = fixture.store_vectors(vector_for);
    let producer = ExhaustiveProducer {
        limits: dense_limits(),
    };
    let (_token, budget) = request_budget(10_000);

    let wrong_shape = vec![1.0f32; DIMENSION as usize + 1];
    let outcome = run(
        &fixture,
        &with_dense(),
        &wrong_shape,
        &producer,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.statuses[2], LaneStatus::Unavailable("query_shape"));
    assert_eq!(outcome.body["degraded"], true);
    assert!(!entry_ids(&outcome.body).is_empty());

    let calls = Arc::new(AtomicUsize::new(0));
    let counting = Counting {
        inner: ExhaustiveProducer {
            limits: dense_limits(),
        },
        calls: Arc::clone(&calls),
    };
    let healthy = run(
        &fixture,
        &with_dense(),
        &query_vector(),
        &counting,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(healthy.body["degraded"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let outcome = degraded(&fixture, budget.shared(), "busy");
    assert_eq!(outcome.body["degraded"], true);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no producer runs without a query vector"
    );

    fixture.store_raw_vector(&rows[0].0, &codec::encode(&[0.0; DIMENSION as usize]));
    let outcome = run(
        &fixture,
        &with_dense(),
        &query_vector(),
        &producer,
        budget.shared(),
        |_| {},
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Unavailable("dense_corruption")),
        "a corrupt stored vector ends the request typed"
    );

    let mut over = with_dense();
    over.dense = Some(DenseLimits {
        k: NonZeroUsize::new(MAX_ELIGIBILITY_CANDIDATES + 1).unwrap(),
        ..dense_limits()
    });
    assert_eq!(
        over.validate(),
        Err(LimitsRefusal::Dense {
            bound: "k",
            value: MAX_ELIGIBILITY_CANDIDATES + 1
        })
    );
    fixture.daemon.shutdown().await;
}

/// The dense lane embeds prose; a request that is one selector, or selectors and whitespace, has none, so a ready lane stays undeclared and no producer runs, as the lexical lane already does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_without_prose_leaves_a_ready_dense_lane_undeclared_and_runs_no_producer() {
    let fixture = Fixture::build().await;
    fixture.store_vectors(vector_for);
    let calls = Arc::new(AtomicUsize::new(0));
    let counting = Counting {
        inner: ExhaustiveProducer {
            limits: dense_limits(),
        },
        calls: Arc::clone(&calls),
    };
    let (_token, budget) = request_budget(10_000);
    for query in ["id:rule", "id:rule  id:other"] {
        let outcome = execute(
            &fixture.projection,
            &fixture.store,
            Authority {
                project: &fixture.project,
                destination: ArtifactDestination::Local,
            },
            &with_dense(),
            budget.shared(),
            query,
            DenseLane::Ready {
                query: &query_vector(),
                generation_id: GENERATION,
                producer: &counting,
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(
            outcome.statuses[1..],
            [LaneStatus::Undeclared, LaneStatus::Undeclared],
            "{query}: {}",
            outcome.body
        );
        assert_eq!(outcome.body["lanes"]["dense"]["status"], "undeclared");
        assert_eq!(outcome.body["degraded"], false);
        assert!(!entry_ids(&outcome.body).is_empty(), "{}", outcome.body);
        assert!(
            outcome
                .fused
                .entries()
                .iter()
                .all(|entry| entry.lane(Lane::Dense).is_none())
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no producer runs for a request without prose"
    );

    let prose = run(
        &fixture,
        &with_dense(),
        &query_vector(),
        &counting,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(prose.statuses[2], LaneStatus::Complete);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_coverage_shortfall_and_a_row_bound_leave_the_dense_lane_incomplete() {
    let fixture = Fixture::build().await;
    let candidates = fixture.live_candidates();
    let withheld = candidates[0].occurrence_id.clone();
    let rows = fixture.store_vectors(vector_for);
    fixture
        .projection
        .write(|conn| {
            conn.execute(
                "DELETE FROM occurrence_vectors WHERE occurrence_id=?1",
                [withheld.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
    let producer = ExhaustiveProducer {
        limits: dense_limits(),
    };
    let (_token, budget) = request_budget(10_000);
    let outcome = run(
        &fixture,
        &with_dense(),
        &query_vector(),
        &producer,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        outcome.statuses[2],
        LaneStatus::Incomplete("coverage_shortfall"),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.body["degraded"], true);
    assert!(
        outcome
            .fused
            .entries()
            .iter()
            .filter(|entry| entry.lane(Lane::Dense).is_some())
            .count()
            < rows.len()
    );

    fixture.store_vectors(vector_for);
    let mut bounded = with_dense();
    let narrow = DenseLimits {
        max_rows: NonZeroUsize::new(1).unwrap(),
        ..dense_limits()
    };
    bounded.dense = Some(narrow);
    let producer = ExhaustiveProducer { limits: narrow };
    let outcome = run(
        &fixture,
        &bounded,
        &query_vector(),
        &producer,
        budget.shared(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        outcome.statuses[2],
        LaneStatus::Incomplete("row_bound"),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.body["degraded"], true);

    let mut bad = with_dense();
    bad.dense = Some(DenseLimits {
        unit_norm_tolerance: f64::NAN,
        ..dense_limits()
    });
    assert_eq!(bad.validate(), Err(LimitsRefusal::DenseTolerance));
    fixture.daemon.shutdown().await;
}
