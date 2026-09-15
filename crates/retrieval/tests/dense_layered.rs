//! The layered ranking over the shared projection fixture. Layers are built by hand from the corpus, and every expected ranking is the independent f64 reference over the rows the hand-written map says win.

mod support;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use kernel::EligibilityVerdict;
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use retrieval::dense::layered::rank_layers_with_hook_for_test;
use retrieval::dense::{
    Completion, DenseCoverage, IncompleteReason, Layer, LayerAccount, LayeredQuery, LayeredRanking,
    LayeredRefusal, Metric, OracleBounds, OracleRefusal, ResolveRefusal, RowRejection, Window,
};

use support::dense::*;
use support::layers::OwnedLayer;

/// `rows` are `(object, vector)`; identifiers come from the fixture's occurrence identity so the layer names what the projection names.
fn owned(
    fixture: &Fixture,
    ordinal: u32,
    rows: &[(&str, Vec<f32>)],
    tombstones: &[&str],
) -> OwnedLayer {
    OwnedLayer::new(
        generation().generation_epoch,
        ordinal,
        rows.iter()
            .map(|(object, vector)| (fixture.id(object), vector.clone()))
            .collect(),
        tombstones.iter().map(|object| fixture.id(object)).collect(),
    )
}

/// Every dense-required corpus row with its stored vector, as the base layer.
fn full_base(fixture: &Fixture) -> OwnedLayer {
    let rows: Vec<(&str, Vec<f32>)> = fixture
        .rows
        .iter()
        .filter_map(|row| Some((row.object.as_str(), row.vector.clone()?)))
        .collect();
    owned(fixture, 0, &rows, &[])
}

fn rank(
    fixture: &Fixture,
    layers: &[Layer<'_>],
    query: &[f32],
    bounds: OracleBounds,
    budget: &EvalBudget,
    hook: impl FnMut(Window<'_>),
) -> Result<LayeredRanking, LayeredRefusal> {
    let generation = generation();
    let request = LayeredQuery {
        generation: &generation,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
        query,
        authority: fixture.authority(),
        bounds,
        layers,
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    fixture
        .store
        .with_conn(|conn| {
            Ok(rank_layers_with_hook_for_test(
                conn,
                &fixture.kernel,
                &request,
                budget,
                hook,
            ))
        })
        .unwrap()
}

fn rank_plain(fixture: &Fixture, layers: &[Layer<'_>], query: &[f32], k: usize) -> LayeredRanking {
    let ranking = rank(
        fixture,
        layers,
        query,
        bounds(k),
        &EvalBudget::unbounded(),
        |_| {},
    )
    .unwrap();
    assert_accounted(&ranking);
    ranking
}

/// Every winner ends exactly one way: scored, revoked, or never reached.
fn assert_accounted(ranking: &LayeredRanking) {
    assert_eq!(
        ranking.layers.winners,
        ranking.ranking.coverage.with_vector + ranking.layers.revoked + ranking.layers.unvisited,
        "{:?}",
        ranking.layers
    );
}

/// The independent expectation over a hand-written `object -> vector` map.
fn reference(
    fixture: &Fixture,
    query: &[f32],
    map: &BTreeMap<&str, Vec<f32>>,
) -> Vec<(String, f64)> {
    reference_over(
        query,
        map.iter()
            .map(|(object, vector)| (fixture.id(object), vector.as_slice())),
    )
}

fn corpus_map(fixture: &Fixture) -> BTreeMap<&str, Vec<f32>> {
    fixture
        .rows
        .iter()
        .filter_map(|row| Some((row.object.as_str(), row.vector.clone()?)))
        .collect()
}

#[test]
fn a_base_alone_ranks_like_the_oracle_over_the_same_rows() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let mut visited = Vec::new();
    let layered = rank(
        &fixture,
        &[base.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |window| {
            if let Window::Visited(id) = window {
                visited.push(id.to_owned());
            }
        },
    )
    .unwrap();
    assert_visited_once(&fixture, &visited);
    let oracle = fixture
        .rank(&query, bounds(8), &EvalBudget::unbounded())
        .unwrap();
    assert_eq!(layered.ranking, oracle);
    assert_eq!(layered.ranking.completion, Completion::Complete);
    assert_eq!(
        keyed(&layered.ranking),
        reference(&fixture, &query, &corpus_map(&fixture))
    );
    assert_eq!(
        layered.layers,
        LayerAccount {
            winners: 8,
            superseded: 0,
            masked: 0,
            revoked: 0,
            unvisited: 0
        }
    );
}

#[test]
fn a_newer_lower_scoring_row_replaces_an_older_higher_one_and_enumeration_order_decides_nothing() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    // `alpha` leads the reference against `axis(0)`; the delta's row for it scores lowest of all.
    let low = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let delta = owned(&fixture, 1, &[("alpha", low.clone())], &[]);
    let mut map = corpus_map(&fixture);
    map.insert("alpha", low);
    let expected = reference(&fixture, &query, &map);
    // Against `axis(0)` the new row scores zero: below every positive row, above only `theta`'s negative one.
    assert_eq!(expected[6], (fixture.id("alpha"), 0.0));

    let forward = rank_plain(&fixture, &[base.layer(), delta.layer()], &query, 8);
    assert_eq!(keyed(&forward.ranking), expected);
    assert_eq!(forward.ranking.completion, Completion::Complete);
    assert_eq!(forward.layers.superseded, 1);
    let backward = rank_plain(&fixture, &[delta.layer(), base.layer()], &query, 8);
    assert_eq!(backward.ranking, forward.ranking);
    assert_eq!(backward.layers, forward.layers);
    // Cut at three, the base's `alpha` row would have led; it must not appear.
    let top = rank_plain(&fixture, &[base.layer(), delta.layer()], &query, 3);
    assert_eq!(keyed(&top.ranking), expected[..3]);
}

#[test]
fn a_tombstoned_winner_is_a_coverage_shortfall_and_a_later_row_over_a_tombstone_wins() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let first = owned(&fixture, 1, &[], &["alpha", "beta"]);
    let restored = unit([0.6, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let second = owned(&fixture, 2, &[("beta", restored.clone())], &[]);
    let mut map = corpus_map(&fixture);
    map.remove("alpha");
    map.insert("beta", restored);

    let ranking = rank_plain(
        &fixture,
        &[base.layer(), first.layer(), second.layer()],
        &query,
        8,
    );
    assert_eq!(keyed(&ranking.ranking), reference(&fixture, &query, &map));
    // `alpha` is live and required in the projection but no layer holds a vector for it: a shortfall, not an exclusion, and no older row stands in.
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
    assert_eq!(
        ranking.ranking.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 7,
            missing_pending: 0,
            missing_without_pending: 1
        }
    );
    assert!(ranking.ranking.consumed.excluded.is_empty());
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 7,
            superseded: 1,
            masked: 1,
            revoked: 0,
            unvisited: 0
        }
    );
}

#[test]
fn a_winner_the_projection_no_longer_lists_is_revoked_and_never_replaced_by_an_older_row() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let delta = owned(
        &fixture,
        1,
        &[("gamma", unit([0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))],
        &[],
    );
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at) VALUES (?1,7,'retired',1)",
            [fixture.id("gamma")],
        )
        .unwrap();
    let mut map = corpus_map(&fixture);
    map.remove("gamma");

    let ranking = rank_plain(&fixture, &[base.layer(), delta.layer()], &query, 8);
    assert_eq!(keyed(&ranking.ranking), reference(&fixture, &query, &map));
    assert_eq!(ranking.ranking.completion, Completion::Complete);
    assert_eq!(ranking.ranking.coverage.required, 7);
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 8,
            superseded: 1,
            masked: 0,
            revoked: 1,
            unvisited: 0
        }
    );
    // A winner past the last live row is revoked too: `theta`'s identifier sorts last among the corpus here or not, the count is the same.
    let late = owned(&fixture, 2, &[("zeta", axis(5))], &[]);
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at) VALUES (?1,8,'retired',1)",
            [fixture.id("zeta")],
        )
        .unwrap();
    let ranking = rank_plain(
        &fixture,
        &[base.layer(), delta.layer(), late.layer()],
        &query,
        8,
    );
    map.remove("zeta");
    assert_eq!(keyed(&ranking.ranking), reference(&fixture, &query, &map));
    assert_eq!(ranking.layers.revoked, 2);
    assert_eq!(ranking.layers.unvisited, 0);
}

#[test]
fn authority_that_moves_before_final_handoff_rejects_the_winner_without_falling_back() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    // The delta's `alpha` still leads; the base's `alpha` would too, and neither may survive retirement.
    let delta = owned(
        &fixture,
        1,
        &[("alpha", unit([0.95, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))],
        &[],
    );
    let full = reference(&fixture, &query, &corpus_map(&fixture));
    let ranking = rank(
        &fixture,
        &[base.layer(), delta.layer()],
        &query,
        bounds(3),
        &EvalBudget::unbounded(),
        |window| {
            if window == Window::BeforeRevalidation {
                fixture.retire("alpha");
            }
        },
    )
    .unwrap();
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(keyed(&ranking.ranking), full[1..3]);
    assert!(
        !ids_of(&ranking.ranking).contains(&fixture.id("alpha")),
        "no row of a rejected winner's occurrence is returned"
    );
    assert_eq!(
        ranking.ranking.consumed.excluded,
        vec![(EligibilityVerdict::Retracted, 1)]
    );
}

#[test]
fn a_walk_that_stops_early_reports_the_winners_it_never_reached() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let ranking = rank(
        &fixture,
        &[base.layer()],
        &query,
        OracleBounds {
            k: NonZeroUsize::new(8).unwrap(),
            page_rows: NonZeroUsize::new(2).unwrap(),
            max_rows: NonZeroUsize::new(3).unwrap(),
        },
        &EvalBudget::unbounded(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::RowBound)
    );
    assert_eq!(ranking.ranking.coverage.required, 3);
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 8,
            superseded: 0,
            masked: 0,
            revoked: 0,
            unvisited: 5
        }
    );
}

#[test]
fn layers_that_do_not_resolve_or_carry_an_invalid_row_refuse_before_any_row_is_ranked() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let second_base = owned(&fixture, 0, &[("beta", axis(1))], &[]);
    let mut visited = 0usize;
    let refusal = rank(
        &fixture,
        &[base.layer(), second_base.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |_| visited += 1,
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        LayeredRefusal::Resolve(ResolveRefusal::MultipleBases {
            first: 0,
            second: 1
        })
    );
    assert_eq!(visited, 0, "the projection is not walked");

    let unnormalized = owned(&fixture, 1, &[("beta", vec![1.0; 8])], &[]);
    let refusal = rank(
        &fixture,
        &[base.layer(), unnormalized.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(
        refusal,
        LayeredRefusal::Oracle(OracleRefusal::StoredRow {
            rejection: RowRejection::Normalization { .. },
            ..
        })
    ));

    // Layers of another epoch never reach the projection either, even when they agree among themselves.
    let mut foreign = owned(&fixture, 0, &[("beta", axis(1))], &[]);
    foreign.precedence.base_epoch = generation().generation_epoch + 1;
    let refusal = rank(
        &fixture,
        &[foreign.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |_| visited += 1,
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        LayeredRefusal::Epoch {
            layers: generation().generation_epoch + 1,
            generation: generation().generation_epoch
        }
    );
    assert_eq!(visited, 0);
}

#[test]
fn a_class_that_requires_no_vector_is_never_a_winner_the_walk_visits() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    // The tool span is live but not dense-required; a layer row for it is revoked as far as the walk is concerned, never scored.
    let stray = owned(&fixture, 1, &[("tool", axis(2))], &[]);
    assert_eq!(fixture.row("tool").class, OccurrenceClass::RawToolSpans);
    let ranking = rank_plain(&fixture, &[base.layer(), stray.layer()], &query, 8);
    assert_eq!(
        keyed(&ranking.ranking),
        reference(&fixture, &query, &corpus_map(&fixture))
    );
    assert_eq!(ranking.layers.revoked, 1);
}

#[test]
fn a_winner_the_kernel_excludes_at_preselection_never_reaches_the_top_k_and_no_older_row_stands_in()
{
    let admitted: Vec<&str> = OBJECTS
        .iter()
        .copied()
        .filter(|object| *object != "alpha")
        .collect();
    let fixture = Fixture::new(&admitted, corpus());
    let query = axis(0);
    let base = full_base(&fixture);
    // The delta's `alpha` would lead the ranking, as the base's would; the kernel hides both.
    let delta = owned(
        &fixture,
        1,
        &[("alpha", unit([0.95, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))],
        &[],
    );
    let mut map = corpus_map(&fixture);
    map.remove("alpha");
    let ranking = rank_plain(&fixture, &[base.layer(), delta.layer()], &query, 3);
    assert_eq!(ranking.ranking.completion, Completion::Complete);
    assert_eq!(
        keyed(&ranking.ranking),
        reference(&fixture, &query, &map)[..3]
    );
    assert!(!ids_of(&ranking.ranking).contains(&fixture.id("alpha")));
    assert_eq!(
        ranking.ranking.consumed.excluded,
        vec![(EligibilityVerdict::Hidden, 1)],
        "the winner is judged once at its page and once at re-judgment only if admitted; it was not"
    );
    assert_eq!(ranking.ranking.coverage.with_vector, 8);
}

#[test]
fn a_masked_row_with_open_work_is_a_pending_shortfall() {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let delta = owned(&fixture, 1, &[], &["alpha"]);
    fixture.job_state("alpha", "pending");
    let ranking = rank_plain(&fixture, &[base.layer(), delta.layer()], &query, 8);
    assert_eq!(
        ranking.ranking.coverage,
        DenseCoverage {
            required: 8,
            with_vector: 7,
            missing_pending: 1,
            missing_without_pending: 0
        }
    );
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::DenseCoverageShortfall)
    );
}

#[test]
fn a_stop_between_pages_leaves_the_tail_unvisited_while_a_stop_after_the_last_page_knows_it_is_revoked()
 {
    let fixture = Fixture::all_admitted();
    let query = axis(0);
    let base = full_base(&fixture);
    let mut visited = Vec::new();
    let ranking = rank(
        &fixture,
        &[base.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |window| match window {
            Window::Visited(id) => visited.push(id.to_owned()),
            Window::AfterPage(1) => fixture.retire("theta"),
            _ => {}
        },
    )
    .unwrap();
    assert_accounted(&ranking);
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(visited.len(), 4, "two pages of two were visited");
    assert_eq!(ranking.ranking.coverage.required, 4);
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 8,
            superseded: 0,
            masked: 0,
            revoked: 0,
            unvisited: 4
        }
    );

    // A winner past every live row, with the authority moving only at the final re-judgment: the walk saw the population end, so the tail is revoked, not unvisited.
    let fixture = Fixture::all_admitted();
    let whole = full_base(&fixture);
    let last = fixture.dense_ids().into_iter().next_back().unwrap();
    let last_object = fixture
        .rows
        .iter()
        .find(|row| row.occurrence_id() == last)
        .unwrap()
        .object
        .clone();
    fixture
        .raw()
        .execute(
            "INSERT INTO occurrence_tombstones(occurrence_id,invalidated_commit_seq,reason,recorded_at) VALUES (?1,7,'retired',1)",
            [&last],
        )
        .unwrap();
    let ranking = rank(
        &fixture,
        &[whole.layer()],
        &query,
        bounds(8),
        &EvalBudget::unbounded(),
        |window| {
            if window == Window::BeforeRevalidation {
                fixture.retire("beta");
            }
        },
    )
    .unwrap();
    assert_accounted(&ranking);
    assert_eq!(
        ranking.ranking.completion,
        Completion::Incomplete(IncompleteReason::SnapshotChanged)
    );
    assert_eq!(ranking.ranking.coverage.required, 7);
    assert!(!ids_of(&ranking.ranking).contains(&fixture.id(&last_object)));
    assert_eq!(
        ranking.layers,
        LayerAccount {
            winners: 8,
            superseded: 0,
            masked: 0,
            revoked: 1,
            unvisited: 0
        }
    );
}
