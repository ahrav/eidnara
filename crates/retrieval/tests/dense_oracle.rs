mod support;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{ArtifactDestination, EligibilityVerdict, KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::ProjectionError;
use retrieval::batch::VectorGeneration;
use retrieval::dense::codec::{
    self, ARTIFACT_HEADER_BYTES, ARTIFACT_MAGIC, ARTIFACT_VERSION, ArtifactRejection, Metric,
    RowLayout, RowRejection,
};
use retrieval::dense::export::{ExportRefusal, LiveRows, live_rows};
use retrieval::dense::{
    Completion, DenseCoverage, IncompleteReason, OracleBounds, OracleRefusal, Ranked, Window,
    exhaustive, inner_product, rank_order, rescore,
};
use retrieval::eligibility::{
    AuthorityMoved, EligibilityReport, authority_moved, judge_occurrences,
};

use support::dense::*;

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
    // Every row is scored, but only a row that could enter the top-3 when visited is judged, then the 3 held rows are re-judged.
    assert!(
        (3 + 3..8 + 3).contains(&ranking.consumed.judged),
        "{:?}",
        ranking.consumed
    );
    assert!(ranking.consumed.batches <= ranking.consumed.pages + 1);
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
fn a_row_that_cannot_enter_the_top_k_when_visited_is_scored_but_never_judged() {
    // `theta` scores lowest against the axis; the kernel hides it.
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "theta")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    let query = axis(0);
    let reference = fixture.reference(&query, &admitted);
    let theta = fixture.id("theta");
    let before_theta = fixture
        .dense_ids()
        .iter()
        .take_while(|id| **id != theta)
        .count();
    assert!(
        before_theta >= 1,
        "the visit order must reach theta after another row"
    );

    // One row per page: the top-K is full of better rows before theta is visited, so theta is not judged.
    let lazy = fixture
        .rank(
            &query,
            OracleBounds {
                page_rows: NonZeroUsize::new(1).unwrap(),
                ..bounds(before_theta)
            },
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(lazy.completion, Completion::Complete);
    assert_eq!(keyed(&lazy), reference[..before_theta]);
    assert_eq!(
        lazy.coverage.with_vector, 8,
        "theta is still visited and scored"
    );
    assert!(
        lazy.consumed.excluded.is_empty(),
        "a row that could not enter the top-K is not judged: {:?}",
        lazy.consumed
    );
    assert!(
        lazy.consumed.judged < 8 + before_theta,
        "{:?}",
        lazy.consumed
    );

    // One page holding every row: theta is selected against an empty top-K and judged.
    let eager = fixture
        .rank(
            &query,
            OracleBounds {
                page_rows: NonZeroUsize::new(8).unwrap(),
                ..bounds(before_theta)
            },
            &EvalBudget::unbounded(),
        )
        .unwrap();
    assert_eq!(keyed(&eager), keyed(&lazy));
    assert_eq!(
        eager.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)]
    );
    assert_eq!(eager.consumed.judged, 8 + before_theta);
}

#[test]
fn a_corrupt_identity_field_is_refused_whether_or_not_its_row_could_enter_the_top_k() {
    // `theta` scores lowest against the axis, so with one row per page it is visited after the top-K is full.
    let fixture = Fixture::all_admitted();
    let theta = fixture.id("theta");
    let before_theta = fixture
        .dense_ids()
        .iter()
        .take_while(|id| **id != theta)
        .count();
    assert!(before_theta >= 1);
    fixture
        .raw()
        .execute(
            "UPDATE occurrences SET source_object_id='' WHERE occurrence_id=?1",
            rusqlite::params![theta],
        )
        .unwrap();
    for page_rows in [1, 8] {
        let outcome = fixture.rank(
            &axis(0),
            OracleBounds {
                page_rows: NonZeroUsize::new(page_rows).unwrap(),
                ..bounds(before_theta)
            },
            &EvalBudget::unbounded(),
        );
        assert_eq!(
            outcome,
            Err(OracleRefusal::Kernel(kernel::KernelError::InvalidInput)),
            "page_rows={page_rows}: paging must not decide whether a corrupt identity field is detected"
        );
    }
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

/// A page wide enough for every dense row fills one scoring block, so every lane carries a real row.
fn one_page_bounds(k: usize) -> OracleBounds {
    OracleBounds {
        page_rows: NonZeroUsize::new(9).unwrap(),
        ..bounds(k)
    }
}

#[test]
fn a_page_that_fills_a_scoring_block_yields_the_reference_prefix() {
    let fixture = Fixture::all_admitted();
    for query in [
        axis(0),
        axis(6),
        unit([0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3]),
    ] {
        let reference = fixture.reference(&query, &OBJECTS);
        let ranking = fixture
            .rank(&query, one_page_bounds(8), &EvalBudget::unbounded())
            .unwrap();
        assert_eq!(ranking.completion, Completion::Complete);
        assert_eq!(ranking.consumed.pages, 1);
        assert_eq!(keyed(&ranking), reference);
        for (row, (_, score)) in ranking.ranked.iter().zip(&reference) {
            assert_eq!(row.score.to_bits(), score.to_bits());
        }
        let narrow = fixture
            .rank(&query, bounds(8), &EvalBudget::unbounded())
            .unwrap();
        assert_eq!(
            keyed(&narrow),
            keyed(&ranking),
            "page width never changes a score"
        );
    }
}

#[test]
fn a_corrupt_row_inside_a_scoring_block_refuses_with_its_own_rejection() {
    let ids: Vec<String> = Fixture::all_admitted().dense_ids().into_iter().collect();
    let cases: [(usize, Vec<f32>, RowRejection); 3] = [
        (
            2,
            vec![0.0, 0.0, f32::NAN, 0.0, 0.0, 0.0, 0.0, 1.0],
            RowRejection::NonFinite { coordinate: 2 },
        ),
        (4, vec![0.0; 8], RowRejection::ZeroNorm),
        (
            6,
            vec![0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            RowRejection::Normalization {
                norm: 0.5f64.hypot(0.5),
            },
        ),
    ];
    for (position, vector, expected) in cases {
        let fixture = Fixture::all_admitted();
        let victim = ids[position].clone();
        fixture
            .raw()
            .execute(
                "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
                rusqlite::params![victim, codec::encode(&vector)],
            )
            .unwrap();
        assert_eq!(
            fixture.rank(&axis(0), one_page_bounds(3), &EvalBudget::unbounded()),
            Err(OracleRefusal::StoredRow {
                occurrence_id: victim,
                rejection: expected
            }),
            "row {position}"
        );
    }
}

#[test]
fn a_corrupt_row_visited_before_the_budget_ends_still_refuses() {
    let fixture = Fixture::all_admitted();
    let ids: Vec<String> = fixture.dense_ids().into_iter().collect();
    let victim = ids[1].clone();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
            rusqlite::params![victim, codec::encode(&[f32::NAN; 8])],
        )
        .unwrap();
    let budget = EvalBudget::unbounded();
    let mut visited = 0;
    let outcome = fixture.rank_with_hook(&axis(0), one_page_bounds(3), &budget, |window| {
        if let Window::Visited(_) = window {
            visited += 1;
            if visited == 5 {
                budget.cancel();
            }
        }
    });
    assert_eq!(
        outcome,
        Err(OracleRefusal::StoredRow {
            occurrence_id: victim,
            rejection: RowRejection::NonFinite { coordinate: 0 }
        }),
        "the second row was visited before the fifth ended the budget, so its rejection stands"
    );
}

#[test]
fn an_earlier_row_that_fails_at_scoring_outranks_a_later_row_that_fails_at_decoding() {
    let ids: Vec<String> = Fixture::all_admitted().dense_ids().into_iter().collect();
    let short = codec::encode(&unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])[..7]);

    // A non-finite coordinate is found at scoring; the dimension of the next row is found at decoding.
    let fixture = Fixture::all_admitted();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
            rusqlite::params![ids[1], codec::encode(&[f32::NAN; 8])],
        )
        .unwrap();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2, vector_dimension=7 WHERE occurrence_id=?1",
            rusqlite::params![ids[2], short],
        )
        .unwrap();
    assert_eq!(
        fixture.rank(&axis(0), one_page_bounds(3), &EvalBudget::unbounded()),
        Err(OracleRefusal::StoredRow {
            occurrence_id: ids[1].clone(),
            rejection: RowRejection::NonFinite { coordinate: 0 }
        }),
        "the second row fails the layout first, whatever check finds it"
    );

    // The kernel refuses a corrupt identity field at scoring.
    let fixture = Fixture::all_admitted();
    fixture
        .raw()
        .execute(
            "UPDATE occurrences SET source_object_id='' WHERE occurrence_id=?1",
            rusqlite::params![ids[1]],
        )
        .unwrap();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2, vector_dimension=7 WHERE occurrence_id=?1",
            rusqlite::params![ids[2], short],
        )
        .unwrap();
    assert_eq!(
        fixture.rank(&axis(0), one_page_bounds(3), &EvalBudget::unbounded()),
        Err(OracleRefusal::Kernel(kernel::KernelError::InvalidInput)),
        "the second row's identity fails before the third row's dimension"
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

    // An unknown classification generation is a moved snapshot even against nothing or against itself.
    let mut unknown = first.clone();
    unknown.snapshot.classification_generation = None;
    assert_eq!(
        authority_moved(None, None, &unknown),
        Some(AuthorityMoved::Snapshot)
    );
    assert_eq!(
        authority_moved(
            Some(&unknown.snapshot),
            Some(&unknown.incarnation),
            &unknown
        ),
        Some(AuthorityMoved::Snapshot)
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
fn cancellation_stops_decoding_at_each_row_in_a_page() {
    let fixture = Fixture::all_admitted();
    for stop_at in 1..=8 {
        let budget = EvalBudget::unbounded();
        let mut visited = 0;
        let ranking = fixture
            .rank_with_hook(
                &axis(0),
                OracleBounds {
                    page_rows: NonZeroUsize::new(8).unwrap(),
                    ..bounds(3)
                },
                &budget,
                |window| {
                    if let Window::Visited(_) = window {
                        visited += 1;
                        if visited == stop_at {
                            budget.cancel();
                        }
                    }
                },
            )
            .unwrap();
        assert_eq!(
            ranking.completion,
            Completion::Incomplete(IncompleteReason::BudgetExhausted)
        );
        assert!(ranking.ranked.is_empty());
        assert_eq!(visited, stop_at);
        assert_eq!(ranking.coverage.required, stop_at - 1);
        assert_eq!(ranking.coverage.with_vector, stop_at - 1);
        assert_eq!(ranking.consumed.pages, 1);
        assert_eq!(ranking.consumed.batches, 0);
    }
}

#[test]
fn cancellation_after_a_page_is_judged_keeps_every_exclusion_of_that_page() {
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "alpha")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    let budget = EvalBudget::unbounded();
    let ranking = fixture
        .rank_with_hook(
            &axis(0),
            OracleBounds {
                page_rows: NonZeroUsize::new(8).unwrap(),
                ..bounds(3)
            },
            &budget,
            |window| {
                if window == Window::AfterJudgment {
                    budget.cancel();
                }
            },
        )
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(ranking.ranked.is_empty());
    assert_eq!(ranking.consumed.judged, 8);
    assert_eq!(
        ranking.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)],
        "a judged exclusion counts whether or not its page was scored"
    );
}

#[test]
fn cancellation_does_not_decode_a_corrupt_tail_after_the_budget_ends() {
    let fixture = Fixture::all_admitted();
    let last = fixture.dense_ids().into_iter().next_back().unwrap();
    fixture
        .raw()
        .execute(
            "UPDATE occurrence_vectors SET vector=?2 WHERE occurrence_id=?1",
            rusqlite::params![last, codec::encode(&[f32::NAN; 8])],
        )
        .unwrap();
    let budget = EvalBudget::unbounded();
    let ranking = fixture
        .rank_with_hook(
            &axis(0),
            OracleBounds {
                page_rows: NonZeroUsize::new(8).unwrap(),
                ..bounds(3)
            },
            &budget,
            |window| {
                if let Window::Visited(_) = window {
                    budget.cancel();
                }
            },
        )
        .unwrap();
    assert_eq!(
        ranking.completion,
        Completion::Incomplete(IncompleteReason::BudgetExhausted)
    );
    assert!(ranking.ranked.is_empty());
    assert_eq!(ranking.coverage.required, 0);
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
        assert!(
            ranking.consumed.batches <= ranking.consumed.pages + 1,
            "a page with no row that could enter the top-K runs no batch: {:?}",
            ranking.consumed
        );
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
fn codec_entry_points_refuse_invalid_tolerances_before_reading_rows() {
    for tolerance in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
        let layout = RowLayout {
            unit_norm_tolerance: tolerance,
            ..layout()
        };
        for row in [axis(0), vec![2.0; 8], Vec::new()] {
            let bytes = codec::encode(&row);
            for result in [
                codec::validate(&row, &layout),
                codec::decode(&bytes, &layout).map(|_| ()),
                codec::decode(&[0], &layout).map(|_| ()),
            ] {
                assert!(
                    matches!(result, Err(RowRejection::Tolerance { tolerance: seen })
                        if seen.to_bits() == tolerance.to_bits()),
                    "{tolerance}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn tolerated_norm_error_does_not_turn_inner_product_into_cosine() {
    let query = axis(0);
    let mut longer = query.clone();
    longer[0] = 1.0005;
    let ranked = rescore(
        &layout(),
        &query,
        [
            ("a", OccurrenceClass::CanonicalClaims, query.as_slice()),
            ("b", OccurrenceClass::CanonicalClaims, longer.as_slice()),
        ],
    )
    .unwrap();
    // Both rows have cosine 1; stored-value inner product ranks the longer row first.
    assert_eq!(ranked[0].occurrence_id, "b");
    assert_eq!(ranked[0].score, f64::from(longer[0]));
    assert_eq!(ranked[1].score, 1.0);
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
    let filtered = codec::encode_rows(
        &layout(),
        rows.iter().map(Vec::as_slice).filter(|row| row[0] == 0.0),
    )
    .unwrap();
    assert_eq!(u64::from_le_bytes(filtered[16..24].try_into().unwrap()), 2);
    assert_eq!(
        &filtered[ARTIFACT_HEADER_BYTES..],
        &bytes[ARTIFACT_HEADER_BYTES + 32..]
    );
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
    assert_eq!(exported.generation, generation);
    assert_eq!(exported.kernel_incarnation_id, fixture.incarnation);
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
    // The schema admits a NULL hold, read back as empty; no batch could have committed it, so it is no provenance.
    fixture
        .raw()
        .execute("UPDATE projection_checkpoint SET hold_id=NULL", [])
        .unwrap();
    assert!(matches!(
        export(&fixture, 64, layout()),
        Err(ExportRefusal::NoCheckpoint { .. })
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
