//! `retrieval.query` execution over a projection populated from a live kernel.

mod support;

use std::num::NonZeroUsize;
use std::time::Duration;

use daemon::claim_sources::ClaimMaterializer;
use daemon::query_route::{
    DenseLane, LaneStatus, Phase, QueryFailure, QueryRouteLimits, Terminal, execute,
};
use daemon::search_projection::SearchProjection;
use kernel::{ArtifactDestination, ProjectScope, ProviderEgress};
use retrieval::eligibility::Authority;
use retrieval::fusion::Lane;
use retrieval::install_identity;
use serde_json::json;
use support::query_route::{
    ALL_PHASES, FOREIGN, Fixture, NOW, QUERY, commit_bounds, entry_ids, limits,
    projection_identity, request_budget,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_healthy_query_completes_fused_in_the_oracles_order() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
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
        let (token, budget) = request_budget(10_000);
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
        let (_token, budget) = request_budget(600);
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
    let (_token, budget) = request_budget(10_000);
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
        let (_token, budget) = request_budget(10_000);
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
    let (_token, budget) = request_budget(10_000);
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
        DenseLane::Undeclared,
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
    let (_token, budget) = request_budget(10_000);
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
