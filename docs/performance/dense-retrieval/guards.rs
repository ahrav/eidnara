use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};

use kernel::KernelError;
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use retrieval::dense::codec::{self, RowRejection};
use retrieval::dense::score::Ranked;
use retrieval::dense::{Completion, IncompleteReason, OracleBounds, OracleRefusal};
use rusqlite::params;
use serde_json::{Value, json};

use super::candidates;
use super::support::dense::{axis, generation, keyed, reference_over, unit};
use super::{Config, build_fixture, seeded_unit};

fn assert_ranked(actual: &[Ranked], expected: &[(String, f64)]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, (id, score)) in actual.iter().zip(expected) {
        assert_eq!(&actual.occurrence_id, id);
        assert_eq!(actual.class, OccurrenceClass::CanonicalClaims);
        assert_eq!(actual.score.to_bits(), score.to_bits(), "{id}");
    }
}

fn expect_refusal(case: &str, message: &str, run: impl FnOnce()) -> Value {
    let payload = catch_unwind(AssertUnwindSafe(run)).expect_err(case);
    let actual = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .expect("refusal must carry a string panic payload");
    assert!(
        actual.contains(message),
        "{case}: unexpected panic: {actual}"
    );
    json!({"case": case, "outcome": "expected_fixture_panic", "message_checked": message})
}

#[test]
#[ignore = "fixture-only candidate guards; expected refusals print panic messages"]
fn dense_candidate_guards() {
    let mut cases = Vec::new();
    let mut refusals = Vec::new();
    let generation = generation();
    let budget = EvalBudget::unbounded();

    for (name, n, selectivity) in [
        ("excluded_prefix_and_boundary_tie", 160, 10),
        ("zero_eligible", 100, 0),
    ] {
        let config = Config {
            n,
            dimension: 8,
            reps: 0,
            k: 20,
            page: 17,
            selectivity,
            query_seed: 1,
        };
        let (mut fixture, _) = build_fixture(&config, &generation);
        let query = if selectivity == 0 {
            seeded_unit(b"dense-guard-query", config.query_seed, config.dimension)
        } else {
            axis(0)
        };
        let live: Vec<_> = fixture
            .rows
            .iter()
            .enumerate()
            .filter(|(index, _)| index % 20 != 19)
            .map(|(index, row)| (index, row.occurrence_id()))
            .collect();
        let admitted: BTreeSet<_> = live
            .iter()
            .filter(|(index, _)| *index < config.admitted())
            .map(|(_, id)| id.clone())
            .collect();
        let mut boundary = Value::Null;
        if selectivity != 0 {
            assert_eq!(admitted.len(), 16);
            assert!(admitted.len() < config.k);
            let excluded_tie = live
                .iter()
                .filter(|(index, _)| *index >= config.admitted())
                .min_by(|left, right| left.1.cmp(&right.1))
                .unwrap();
            let eligible_tie = live
                .iter()
                .find(|(index, id)| *index < config.admitted() && id > &excluded_tie.1)
                .unwrap();
            let highest: BTreeSet<_> = live
                .iter()
                .filter(|(index, _)| *index >= config.admitted() && *index != excluded_tie.0)
                .take(63)
                .map(|(index, _)| *index)
                .collect();
            assert_eq!(highest.len(), 63);
            let mut raw = fixture.raw();
            let tx = raw.transaction().unwrap();
            for (index, row) in fixture.rows.iter_mut().enumerate() {
                let vector = if index == excluded_tie.0 || index == eligible_tie.0 {
                    unit([3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
                } else if highest.contains(&index) {
                    unit([4.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
                } else if index < config.admitted() {
                    unit([-3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
                } else {
                    axis(1)
                };
                assert_eq!(tx.execute(
                    "UPDATE occurrence_vectors SET vector=?1 WHERE occurrence_id=?2 AND generation_id=?3",
                    params![codec::encode(&vector), row.occurrence_id(), generation.generation_id],
                ).unwrap(), 1);
                row.vector = Some(vector);
            }
            tx.commit().unwrap();
            let full = reference_over(
                &query,
                live.iter().map(|(index, id)| {
                    (id.clone(), fixture.rows[*index].vector.as_deref().unwrap())
                }),
            );
            assert!(full[..64].iter().all(|(id, _)| !admitted.contains(id)));
            assert_eq!(full[63].0, excluded_tie.1);
            assert_eq!(full[64].0, eligible_tie.1);
            assert_eq!(full[63].1.to_bits(), full[64].1.to_bits());
            boundary = json!({
                "rank_64_excluded_id": full[63].0,
                "rank_65_eligible_id": full[64].0,
                "score_bits": full[63].1.to_bits(),
                "first_64_all_excluded": true,
            });
        }
        let mut expected = reference_over(
            &query,
            live.iter()
                .filter(|(_, id)| admitted.contains(id))
                .map(|(index, id)| (id.clone(), fixture.rows[*index].vector.as_deref().unwrap())),
        );
        expected.truncate(config.k);
        let bounds = OracleBounds {
            k: NonZeroUsize::new(config.k).unwrap(),
            page_rows: NonZeroUsize::new(config.page).unwrap(),
            max_rows: NonZeroUsize::new(config.n).unwrap(),
        };
        let request = fixture.query(&query, &generation, bounds);
        let oracle = fixture.rank(&query, bounds, &budget).unwrap();
        assert_eq!(oracle.completion, Completion::Complete);
        assert_eq!(oracle.coverage.required, live.len());
        assert_eq!(oracle.coverage.with_vector, live.len());
        assert_eq!(keyed(&oracle), expected);
        assert_ranked(&oracle.ranked, &expected);
        let resident = candidates::build_resident(&fixture.store, &fixture.kernel, &request);
        let mut paths = Vec::new();
        for (path, result) in [
            (
                "page_score_first",
                candidates::page_score_first(&fixture.store, &fixture.kernel, &request),
            ),
            (
                "shortlist_sql",
                candidates::shortlist_sql(&fixture.store, &fixture.kernel, &request),
            ),
            (
                "resident_shortlist",
                candidates::resident_shortlist(&resident, &fixture.kernel, &request),
            ),
        ] {
            assert_ranked(&result.ranked, &expected);
            let work = &result.measurements;
            assert_eq!(
                work["judged"].as_u64().unwrap(),
                (live.len() + expected.len()) as u64
            );
            if path == "page_score_first" {
                assert_eq!(work["passes"], 1);
                assert_eq!(work["scanned"].as_u64().unwrap(), live.len() as u64);
            } else {
                let passes = work["passes"].as_u64().unwrap();
                assert!(passes >= if selectivity == 0 { 2 } else { 3 });
                assert_eq!(
                    work["scanned"].as_u64().unwrap(),
                    live.len() as u64 * passes
                );
                assert_eq!(work["scored"], work["scanned"]);
                assert_eq!(
                    work["metadata_lookups"].as_u64().unwrap(),
                    live.len() as u64
                );
            }
            if path == "resident_shortlist" {
                assert_eq!(work["decoded"], 0);
                assert_eq!(work["page_queries"], 0);
            }
            paths.push(json!({"path": path, "passes": work["passes"],
                "scanned": work["scanned"], "judged": work["judged"]}));
        }
        cases.push(json!({"case": name, "outcome": "exact_ids_and_score_bits",
            "population": live.len(), "eligible": expected.len(), "k": config.k,
            "page_heap_underfilled_throughout": true, "tie_boundary": boundary, "paths": paths,
            "work_limit": "bounded shortlist memory does not bound repeated scan work"}));
    }

    let config = Config {
        n: 100,
        dimension: 8,
        reps: 0,
        k: 20,
        page: 17,
        selectivity: 100,
        query_seed: 7,
    };
    let query = seeded_unit(b"dense-guard-query", config.query_seed, config.dimension);
    let bounds = OracleBounds {
        k: NonZeroUsize::new(config.k).unwrap(),
        page_rows: NonZeroUsize::new(config.page).unwrap(),
        max_rows: NonZeroUsize::new(config.n).unwrap(),
    };
    for fault in [
        "missing_vector",
        "nonfinite_vector",
        "malformed_digest",
        "kernel_retirement",
    ] {
        let (fixture, _) = build_fixture(&config, &generation);
        let full = reference_over(
            &query,
            fixture
                .rows
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 20 != 19)
                .map(|(_, row)| (row.occurrence_id(), row.vector.as_deref().unwrap())),
        );
        assert!(full.len() > 64);
        let losing_id = &full.last().unwrap().0;
        let losing = fixture
            .rows
            .iter()
            .find(|row| row.occurrence_id() == *losing_id)
            .unwrap();
        let expected = &full[..config.k];
        let request = fixture.query(&query, &generation, bounds);
        let oracle = fixture.rank(&query, bounds, &budget).unwrap();
        assert_eq!(oracle.completion, Completion::Complete);
        assert_ranked(&oracle.ranked, expected);
        let resident = candidates::build_resident(&fixture.store, &fixture.kernel, &request);
        assert_ranked(
            &candidates::resident_shortlist(&resident, &fixture.kernel, &request).ranked,
            expected,
        );

        match fault {
            "missing_vector" | "nonfinite_vector" => {
                let panic_message = if fault == "missing_vector" {
                    fixture.drop_vector(&losing.object);
                    let stock = fixture.rank(&query, bounds, &budget).unwrap();
                    assert_eq!(
                        stock.completion,
                        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
                    );
                    assert_eq!(stock.coverage.missing(), 1);
                    "missing vector"
                } else {
                    let mut invalid = losing.vector.clone().unwrap();
                    invalid[0] = f32::NAN;
                    assert_eq!(fixture.raw().execute(
                        "UPDATE occurrence_vectors SET vector=?1 WHERE occurrence_id=?2 AND generation_id=?3",
                        params![codec::encode(&invalid), losing_id, generation.generation_id],
                    ).unwrap(), 1);
                    let stock = fixture.rank(&query, bounds, &budget).unwrap_err();
                    assert!(matches!(stock, OracleRefusal::StoredRow {
                        occurrence_id, rejection: RowRejection::NonFinite { coordinate: 0 }
                    } if occurrence_id == *losing_id));
                    "corrupt stored vector"
                };
                refusals.push(expect_refusal(
                    &format!("{fault}/page_score_first"),
                    panic_message,
                    || {
                        drop(candidates::page_score_first(
                            &fixture.store,
                            &fixture.kernel,
                            &request,
                        ));
                    },
                ));
                refusals.push(expect_refusal(
                    &format!("{fault}/shortlist_sql"),
                    panic_message,
                    || {
                        drop(candidates::shortlist_sql(
                            &fixture.store,
                            &fixture.kernel,
                            &request,
                        ));
                    },
                ));
                cases.push(json!({"case": fault, "outcome": "all_candidates_refuse",
                    "losing_id": losing_id, "losing_rank": full.len(),
                    "stock": if fault == "missing_vector" { "DenseCoverageShortfall" } else { "StoredRow::NonFinite" }}));
            }
            "malformed_digest" => {
                assert_eq!(fixture.raw().execute(
                    "UPDATE occurrences SET source_artifact_digest='not-a-digest' WHERE occurrence_id=?1",
                    [losing_id],
                ).unwrap(), 1);
                let stock = fixture.rank(&query, bounds, &budget).unwrap_err();
                assert!(matches!(
                    stock,
                    OracleRefusal::Kernel(KernelError::InvalidInput)
                ));
                let skipped = candidates::shortlist_sql(&fixture.store, &fixture.kernel, &request);
                assert_ranked(&skipped.ranked, expected);
                assert_eq!(skipped.measurements["passes"], 1);
                assert_eq!(skipped.measurements["metadata_lookups"], 64);
                cases.push(json!({"case": fault, "outcome": "semantic_gap_demonstrated",
                    "losing_id": losing_id, "losing_rank": full.len(),
                    "stock": "Kernel::InvalidInput", "shortlist_sql": "succeeded_with_exact_ranked_results",
                    "full_semantic_equivalence": false,
                    "resident": "existing_owner_refuses_external_projection_change",
                    "limit": "resident is not rebuilt over malformed metadata; page strategy is not asserted"}));
            }
            "kernel_retirement" => {
                fixture.retire(&losing.object);
                cases.push(
                    json!({"case": fault, "outcome": "resident_refuses_stale_kernel",
                    "mutation": "legitimate_retirement_after_resident_build",
                    "scope": "sequential fixture mutation, not a concurrent snapshot proof"}),
                );
            }
            _ => unreachable!(),
        }
        let message = if fault == "kernel_retirement" {
            "projection is not caught up"
        } else {
            "resident projection is stale"
        };
        refusals.push(expect_refusal(
            &format!("{fault}/resident_shortlist"),
            message,
            || {
                drop(candidates::resident_shortlist(
                    &resident,
                    &fixture.kernel,
                    &request,
                ));
            },
        ));
    }
    assert_eq!(refusals.len(), 8);
    println!(
        "{}",
        json!({"kind": "guard", "test": "dense_candidate_guards", "success": true,
        "cases": cases, "expected_fixture_panics": refusals.len(), "refusals": refusals,
        "panic_output": "expected refusal hooks may print to stderr; success requires every assertion",
        "scope": "small fixture correctness only; no benchmark or universal snapshot guarantee"})
    );
}
