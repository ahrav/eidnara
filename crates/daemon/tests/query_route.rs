//! `retrieval.query` execution over a projection populated from a live kernel.

mod support;

use std::num::NonZeroUsize;
use std::time::Duration;

use daemon::query_route::{
    DenseLane, LaneStatus, Phase, QueryFailure, QueryRouteLimits, Terminal, execute,
};
use daemon::search_projection::SearchProjection;
use kernel::{ArtifactDestination, ProjectScope};
use retrieval::eligibility::Authority;
use retrieval::fusion::Lane;
use retrieval::install_identity;
use serde_json::json;
use support::query_route::{
    ALL_PHASES, FOREIGN, Fixture, QUERY, entry_ids, limits, projection_identity, request_budget,
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
    fixture.retire("other").await;
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
async fn revalidation_refuses_verdicts_joined_across_a_moved_kernel_snapshot() {
    let fixture = Fixture::build().await;
    let mut one_per_batch = limits();
    one_per_batch.validation_batch = NonZeroUsize::new(1).unwrap();
    let (_token, budget) = request_budget(10_000);
    let mut revalidation_checks = 0;
    let outcome = fixture.run(&one_per_batch, budget.shared(), QUERY, |phase| {
        if phase == Phase::Revalidation {
            revalidation_checks += 1;
            if revalidation_checks == 2 {
                fixture.move_kernel_snapshot();
            }
        }
    });
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Unavailable("snapshot_changed")),
        "verdicts from two kernel snapshots are never joined into one answer"
    );
    assert!(revalidation_checks >= 2, "{revalidation_checks}");
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_lane_positions_and_bounds_count_only_eligible_rows() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
    let both = fixture
        .run(&limits(), budget.shared(), "id:rule id:other", |_| {})
        .unwrap();
    let rule_only = fixture
        .run(&limits(), budget.shared(), "id:rule", |_| {})
        .unwrap();
    let both_rows = entry_ids(&both.body).len();
    let rule_rows = entry_ids(&rule_only.body).len();
    assert!(rule_rows > 0 && rule_rows < both_rows, "{}", both.body);
    fixture.retire("other").await;

    let outcome = fixture
        .run(&limits(), budget.shared(), "id:rule id:other", |_| {})
        .unwrap();
    assert_eq!(entry_ids(&outcome.body), entry_ids(&rule_only.body));
    assert_eq!(outcome.fused.entries().len(), rule_rows);
    for (index, entry) in outcome.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let position = index + 1;
        assert_eq!(entry["position"], position, "{}", outcome.body);
        assert_eq!(
            entry["lanes"]["exact"]["position"], position,
            "a retired row earns no lane position: {}",
            outcome.body
        );
    }
    assert_eq!(outcome.body["degraded"], false);

    let mut union = limits();
    union.fused_union = NonZeroUsize::new(rule_rows).unwrap();
    let outcome = fixture
        .run(&union, budget.shared(), "id:rule id:other", |_| {})
        .unwrap();
    assert_eq!(
        entry_ids(&outcome.body).len(),
        rule_rows,
        "a retired row consumes no union slot: {}",
        outcome.body
    );
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_projection_connection_is_free_while_the_lanes_are_admitted() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
    let mut probed = Vec::new();
    let outcome = fixture.run(&limits(), budget.shared(), QUERY, |phase| {
        if matches!(
            phase,
            Phase::Admission | Phase::Fusion | Phase::Revalidation
        ) {
            let (_token, other) = request_budget(500);
            let read = fixture
                .projection
                .read_under(other.shared(), |conn| retrieval::read_identity(conn));
            probed.push((phase, read.is_ok()));
        }
    });
    outcome.unwrap();
    assert_eq!(
        probed,
        vec![
            (Phase::Admission, true),
            (Phase::Fusion, true),
            (Phase::Revalidation, true)
        ]
    );
    fixture.daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stored_identifier_outside_the_contract_makes_the_lane_unavailable() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
    let healthy = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    let lexical_id = healthy
        .fused
        .entries()
        .iter()
        .find(|entry| entry.lane(Lane::Lexical).is_some())
        .map(|entry| entry.occurrence().to_string())
        .unwrap();
    fixture.corrupt_lexical_copy(&lexical_id);

    let outcome = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    assert_eq!(
        outcome.statuses[1],
        LaneStatus::Unavailable("identity"),
        "{}",
        outcome.body
    );
    assert_eq!(outcome.statuses[0], LaneStatus::Complete);
    assert_eq!(outcome.body["degraded"], true);
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
    assert_eq!(outcome.statuses[0], LaneStatus::incomplete("page_bound"));
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
        LaneStatus::incomplete("accepted_bound"),
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
        LaneStatus::incomplete("scan_bound"),
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
        .write(|conn| {
            install_identity(conn, &projection_identity(&fixture.kernel_incarnation), 1).map(|_| ())
        })
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

    let mut floor = limits();
    floor.response_bytes = QueryRouteLimits::response_floor();
    floor.validate().unwrap();
    let outcome = execute(
        &projection,
        &fixture.store,
        Authority {
            project: &fixture.project,
            destination: ArtifactDestination::Local,
        },
        &floor,
        budget.shared(),
        QUERY,
        DenseLane::Undeclared,
        |_| {},
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Unavailable("response_bytes")),
        "a degraded envelope over the bound is refused, never shipped over it"
    );
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

/// The daemon installs #825 D23's exact limit set, and the set passes the route's own validator.
#[test]
fn production_limits_are_the_d23_set() {
    let limits = QueryRouteLimits::production();
    limits.validate().unwrap();
    let sizes = [
        limits.query_bytes.get(),
        limits.probes.get(),
        limits.lexical_scan_rows.get(),
        limits.lexical_accepted.get(),
        limits.validation_batch.get(),
        limits.exact_page_rows.get(),
        limits.exact_pages.get(),
        limits.fused_union.get(),
        limits.result_rows.get(),
        limits.response_bytes.get(),
    ];
    assert_eq!(sizes, [4096, 16, 4096, 128, 128, 32, 4, 320, 32, 65_536]);
    assert_eq!(
        (
            limits.lexical_qualifying_matches.get(),
            limits.lexical_rank_budget.get()
        ),
        (20_000, 30_000)
    );
    // `lexical_scan_p99_at_one_million_occurrences` in the retrieval crate measures these bounds.
    let lexical = limits.lexical_retrieval_bounds();
    assert_eq!(
        [
            lexical.max_probes.get(),
            lexical.scan_rows.get(),
            lexical.max_accepted.get(),
            lexical.batch_rows.get(),
            lexical.qualifying_matches.get(),
            lexical.rank_budget.get(),
        ],
        [16, 4096, 128, 128, 20_000, 30_000]
    );
    assert_eq!(limits.deadline_ceiling, Duration::from_secs(5));
    let dense = limits.dense.unwrap();
    assert_eq!(
        (dense.k.get(), dense.page_rows.get(), dense.max_rows.get()),
        (64, 256, 1_500_000)
    );
    assert_eq!(dense.unit_norm_tolerance, 1e-3);
    let expected = retrieval::fusion::FusionParameters::new(
        retrieval::fusion::LaneWeights {
            exact: 1.0,
            lexical: 1.0,
            dense: 1.0,
        },
        60.0,
    )
    .unwrap();
    assert_eq!(format!("{:?}", limits.fusion), format!("{expected:?}"));
}

/// Every served claim occurrence carries the canonical decision and source revision it validates to now; occurrence and descriptor ids are never offered as memory ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn served_claims_carry_validated_canonical_decision_references() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
    let outcome = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    let entries = outcome.body["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    let decisions: std::collections::BTreeSet<&str> = entries
        .iter()
        .map(|entry| {
            let canonical = &entry["canonical"];
            assert_eq!(canonical["source_revision"], 1, "{entry}");
            assert!(
                ["visible", "labeled"].contains(&canonical["visibility"].as_str().unwrap()),
                "{entry}"
            );
            let decision = canonical["decision_object_id"].as_str().unwrap();
            assert_ne!(decision, entry["occurrence_id"].as_str().unwrap());
            assert!(
                ["rule", "other", "third"].contains(&decision),
                "a served reference names a seeded decision: {entry}"
            );
            decision
        })
        .collect();
    assert!(decisions.contains("rule"), "{decisions:?}");
    fixture.daemon.shutdown().await;
}

/// A decision retired after its descriptors were admitted is denied at final use: its occurrences leave the answer, and the survivors keep the fused positions and scores they had before the retirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claim_changed_after_admission_is_dropped_without_renumbering_the_survivors() {
    let fixture = Fixture::build().await;
    let (_token, budget) = request_budget(10_000);
    let before = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    // The kernel retires the decision while its descriptors stay live in the projection and kernel.
    let retired = fixture
        .daemon
        .commit(
            "retire-without-sources",
            vec![serde_json::json!({"op": "retire_decision", "object_id": "rule"})],
        )
        .await;
    assert_eq!(retired["state"]["kind"], "available", "{retired}");
    let after = fixture
        .run(&limits(), budget.shared(), QUERY, |_| {})
        .unwrap();
    let retired: Vec<&str> = before.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["canonical"]["decision_object_id"] == "rule")
        .map(|entry| entry["occurrence_id"].as_str().unwrap())
        .collect();
    assert!(!retired.is_empty(), "{}", before.body);
    let survivors: Vec<&serde_json::Value> =
        after.body["entries"].as_array().unwrap().iter().collect();
    assert!(
        survivors.iter().all(|entry| {
            !retired.contains(&entry["occurrence_id"].as_str().unwrap())
                && entry["canonical"]["decision_object_id"].is_string()
        }),
        "{}",
        after.body
    );
    let earlier: std::collections::BTreeMap<&str, (&serde_json::Value, &serde_json::Value)> =
        before.body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["occurrence_id"].as_str().unwrap(),
                    (&entry["position"], &entry["score"]),
                )
            })
            .collect();
    assert!(!survivors.is_empty());
    for entry in survivors {
        let (position, score) = earlier[entry["occurrence_id"].as_str().unwrap()];
        assert_eq!((&entry["position"], &entry["score"]), (position, score));
    }
    fixture.daemon.shutdown().await;
}

/// In the fixture, `explicit` matches two occurrences, `because` three, and `contract` four.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lexical_ranking_bounds_report_every_scope_they_skip() {
    let fixture = Fixture::build().await;
    let run = |qualifying: usize, rank_budget: usize, query: &str| {
        let mut limits = limits();
        limits.lexical_qualifying_matches = NonZeroUsize::new(qualifying).unwrap();
        limits.lexical_rank_budget = NonZeroUsize::new(rank_budget).unwrap();
        let (_token, budget) = request_budget(10_000);
        let outcome = fixture
            .run(&limits, budget.shared(), query, |_| {})
            .unwrap();
        let lexical = outcome
            .fused
            .entries()
            .iter()
            .filter(|entry| entry.lane(Lane::Lexical).is_some())
            .count();
        (outcome.body["lanes"]["lexical"].clone(), lexical)
    };
    let query = "id:rule because contract explicit";
    assert_eq!(run(100, 100, query), (json!({"status": "complete"}), 7));

    // Every probe matches more than one row, so each is read unranked.
    let (lane, lexical) = run(1, 100, QUERY);
    assert_eq!(
        lane,
        json!({"status": "incomplete", "reason": "common_terms"})
    );
    assert!(lexical > 0);

    // The smallest qualifying probe is already over the budget, so nothing is ranked.
    let (lane, lexical) = run(100, 1, QUERY);
    assert_eq!(
        lane,
        json!({"status": "incomplete", "reason": "rank_budget"})
    );
    assert_eq!(lexical, 0);

    // `contract` is common and `because` does not fit the budget behind `explicit`; both skips are reported.
    let (lane, lexical) = run(3, 3, query);
    assert_eq!(
        lane,
        json!({"status": "incomplete", "reason": "common_terms", "also": ["rank_budget"]})
    );
    assert_eq!(lexical, 2);
    fixture.daemon.shutdown().await;
}
