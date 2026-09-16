//! The seed is fixed so a failing case reproduces.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::fusion::{
    DeclaredLanes, Fused, FusionParameters, Lane, LaneHit, LaneRanking, LaneWeights, OccurrenceId,
    ParameterRefusal, RawScore, UnionExceeded, fuse,
};

const SEED: [u8; 32] = *b"fusion-rrf-laws-seed-00000000001";
const VERSION: u8 = OCCURRENCE_ENCODING_VERSION;
const UNBOUNDED: NonZeroUsize = NonZeroUsize::MAX;

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: 512,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    )
}

fn id(byte: u8) -> OccurrenceId {
    OccurrenceId::parse(&format!("{byte:02x}").repeat(32)).unwrap()
}

fn hit(occurrence: OccurrenceId, raw_score: RawScore) -> LaneHit {
    LaneHit {
        occurrence,
        raw_score,
    }
}

fn lane(lane: Lane, hits: impl IntoIterator<Item = LaneHit>) -> LaneRanking {
    LaneRanking::consolidate(lane, VERSION, hits).unwrap()
}

fn exact(ids: impl IntoIterator<Item = OccurrenceId>) -> LaneRanking {
    lane(
        Lane::Exact,
        ids.into_iter()
            .map(|occurrence| hit(occurrence, RawScore::Exact)),
    )
}

fn lexical(ranked: impl IntoIterator<Item = (OccurrenceId, f64)>) -> LaneRanking {
    lane(
        Lane::Lexical,
        ranked
            .into_iter()
            .map(|(occurrence, rank)| hit(occurrence, RawScore::Lexical(rank))),
    )
}

fn dense(ranked: impl IntoIterator<Item = (OccurrenceId, f64)>) -> LaneRanking {
    lane(
        Lane::Dense,
        ranked
            .into_iter()
            .map(|(occurrence, score)| hit(occurrence, RawScore::Dense(score))),
    )
}

fn params(exact: f64, lexical: f64, dense: f64, k: f64) -> FusionParameters {
    FusionParameters::new(
        LaneWeights {
            exact,
            lexical,
            dense,
        },
        k,
    )
    .unwrap()
}

fn weights_of(weights: [f64; 3]) -> LaneWeights {
    LaneWeights {
        exact: weights[0],
        lexical: weights[1],
        dense: weights[2],
    }
}

fn order(fused: &Fused) -> Vec<(OccurrenceId, usize)> {
    fused
        .entries()
        .iter()
        .map(|entry| (*entry.occurrence(), entry.position().get()))
        .collect()
}

/// The independent oracle: one term per lane, summed left to right in `Lane::ORDER`, never a closed fraction.
fn oracle_score(parameters: &FusionParameters, positions: [Option<usize>; 3]) -> f64 {
    let mut sum = 0.0f64;
    for (lane, position) in Lane::ORDER.into_iter().zip(positions) {
        if let Some(position) = position {
            sum += parameters.weight(lane) / (parameters.k() + position as f64);
        }
    }
    sum
}

#[test]
fn a_hand_computed_two_lane_example_matches_term_for_term() {
    let (a, b, c, d) = (id(0x0a), id(0x0b), id(0x0c), id(0x0d));
    let lanes = DeclaredLanes::admit([
        lexical([(b, -9.0), (a, -5.0), (c, -1.0)]),
        dense([(a, 0.9), (c, 0.5), (d, 0.1)]),
    ])
    .unwrap();
    let parameters = params(1.0, 0.7, 1.3, 20.0);
    let fused = fuse(lanes, &parameters, UNBOUNDED).unwrap();

    // By hand: a = 0.7/22 + 1.3/21, c = 0.7/23 + 1.3/22, d = 1.3/23, b = 0.7/21.
    let expected = [
        (a, [None, Some(2), Some(1)]),
        (c, [None, Some(3), Some(2)]),
        (d, [None, None, Some(3)]),
        (b, [None, Some(1), None]),
    ];
    assert_eq!(
        order(&fused),
        expected
            .iter()
            .zip(1usize..)
            .map(|((occurrence, _), position)| (*occurrence, position))
            .collect::<Vec<_>>()
    );
    for (entry, (_, positions)) in fused.entries().iter().zip(expected) {
        let by_hand = 0.0f64
            + positions[1].map_or(0.0, |p| 0.7 / (20.0 + p as f64))
            + positions[2].map_or(0.0, |p| 1.3 / (20.0 + p as f64));
        assert_eq!(entry.score().to_bits(), by_hand.to_bits());
        assert_eq!(
            entry.score().to_bits(),
            oracle_score(&parameters, positions).to_bits()
        );
    }
    assert_eq!(
        fused.undeclared_lanes().collect::<Vec<_>>(),
        vec![Lane::Exact]
    );
    assert_eq!(
        fused.entries()[0].lane(Lane::Dense).unwrap().raw_score(),
        RawScore::Dense(0.9)
    );
    assert_eq!(
        fused.entries()[0].lane(Lane::Lexical).unwrap().raw_score(),
        RawScore::Lexical(-5.0)
    );
    assert!(fused.entries()[0].lane(Lane::Exact).is_none());
}

#[test]
fn closed_fractions_and_another_summation_order_are_detected_as_different_arithmetic() {
    let mut closed_differs = false;
    let mut reordered_differs = false;
    for k in [7.0f64, 13.0, 31.0, 60.0, 97.0] {
        for (we, wl, wd) in [(1.0, 1.0, 1.0), (0.3, 0.7, 1.1), (2.5, 0.1, 0.9)] {
            let parameters = params(we, wl, wd, k);
            for (re, rl, rd) in [(1usize, 2usize, 3usize), (5, 1, 9), (2, 2, 2), (11, 4, 1)] {
                // The exact fillers sort below `a`, so `a` lands at exact rank `re`.
                let a = id(0xff);
                let (mut ex, mut lx, mut dn) = (Vec::new(), Vec::new(), Vec::new());
                for filler in 1..re {
                    ex.push(id(0x10 + filler as u8));
                }
                ex.push(a);
                for filler in 1..rl {
                    lx.push((id(0x40 + filler as u8), -100.0 + filler as f64));
                }
                lx.push((a, 0.0));
                for filler in 1..rd {
                    dn.push((id(0x80 + filler as u8), 100.0 - filler as f64));
                }
                dn.push((a, 0.0));
                let fused = fuse(
                    DeclaredLanes::admit([exact(ex.clone()), lexical(lx), dense(dn)]).unwrap(),
                    &parameters,
                    UNBOUNDED,
                )
                .unwrap();
                let entry = fused
                    .entries()
                    .iter()
                    .find(|entry| *entry.occurrence() == a)
                    .unwrap();
                let exact_position = entry.lane(Lane::Exact).unwrap().position().get();
                assert_eq!(exact_position, re);
                let implementation = entry.score();
                let positions = [Some(exact_position), Some(rl), Some(rd)];
                assert_eq!(
                    implementation.to_bits(),
                    oracle_score(&parameters, positions).to_bits()
                );
                let (de, dl, dd) = (k + exact_position as f64, k + rl as f64, k + rd as f64);
                let closed = (we * dl * dd + wl * de * dd + wd * de * dl) / (de * dl * dd);
                let reordered = (wd / dd + wl / dl) + we / de;
                closed_differs |= closed.to_bits() != implementation.to_bits();
                reordered_differs |= reordered.to_bits() != implementation.to_bits();
            }
        }
    }
    // Three hand-written terms (1/61, 1/62, 0.3/8) whose left-to-right sum and (exact + dense) + lexical grouping differ in bits.
    let (exact_term, lexical_term, dense_term) = (1.0f64 / 61.0, 1.0f64 / 62.0, 0.3f64 / 8.0);
    let declared = (exact_term + lexical_term) + dense_term;
    let regrouped = (exact_term + dense_term) + lexical_term;
    assert_ne!(declared.to_bits(), regrouped.to_bits());
    assert!(
        closed_differs,
        "a closed fraction must be distinguishable from the term sum"
    );
    assert!(
        reordered_differs,
        "another summation order must be distinguishable from the declared order"
    );
}

#[test]
fn non_calibration_parameters_change_the_order_the_calibration_point_gives() {
    let (a, b, c, d) = (id(0x0a), id(0x0b), id(0x0c), id(0x0d));
    // Lexical ranks a then b; dense ranks c, b, d, a. Only k differs between the two runs.
    let lanes = || {
        DeclaredLanes::admit([
            lexical([(a, -9.0), (b, -1.0)]),
            dense([(c, 0.9), (b, 0.7), (d, 0.5), (a, 0.1)]),
        ])
        .unwrap()
    };
    let small_k = fuse(lanes(), &params(0.0, 1.2, 0.8, 5.0), UNBOUNDED).unwrap();
    assert_eq!(order(&small_k)[..2], [(a, 1), (b, 2)]);
    let calibration_k = fuse(lanes(), &params(0.0, 1.2, 0.8, 60.0), UNBOUNDED).unwrap();
    assert_eq!(order(&calibration_k)[..2], [(b, 1), (a, 2)]);

    // A dense-heavy weight set reverses what equal weights give.
    let mirrored = || {
        DeclaredLanes::admit([lexical([(a, -9.0), (b, -1.0)]), dense([(b, 0.9), (a, 0.1)])])
            .unwrap()
    };
    let equal = fuse(mirrored(), &params(1.0, 1.0, 1.0, 60.0), UNBOUNDED).unwrap();
    assert_eq!(order(&equal), vec![(a, 1), (b, 2)]);
    assert_eq!(
        equal.entries()[0].score().to_bits(),
        equal.entries()[1].score().to_bits(),
        "mirrored positions under equal weights tie and fall to identifier order"
    );
    let dense_heavy = fuse(mirrored(), &params(1.0, 0.5, 2.0, 17.0), UNBOUNDED).unwrap();
    assert_eq!(order(&dense_heavy), vec![(b, 1), (a, 2)]);
}

#[test]
fn ties_across_lanes_and_large_tied_sets_fall_to_identifier_order() {
    let (x, y) = (id(0x0e), id(0x0f));
    let across = fuse(
        DeclaredLanes::admit([exact([y]), dense([(x, 0.5)])]).unwrap(),
        &params(1.0, 1.0, 1.0, 60.0),
        UNBOUNDED,
    )
    .unwrap();
    assert_eq!(order(&across), vec![(x, 1), (y, 2)]);
    assert_eq!(
        across.entries()[0].score().to_bits(),
        across.entries()[1].score().to_bits()
    );

    let mut ids: Vec<OccurrenceId> = (0u8..48).map(id).collect();
    ids.reverse();
    let tied = fuse(
        DeclaredLanes::admit([exact(ids.clone())]).unwrap(),
        &params(1.0, 0.0, 0.0, 60.0),
        UNBOUNDED,
    )
    .unwrap();
    ids.sort();
    assert_eq!(
        order(&tied),
        ids.iter().copied().zip(1usize..).collect::<Vec<_>>()
    );
    let zero_weights = fuse(
        DeclaredLanes::admit([
            lexical((0u8..24).rev().map(|n| (id(n), -f64::from(n)))),
            dense((24u8..48).rev().map(|n| (id(n), f64::from(n)))),
        ])
        .unwrap(),
        &params(0.0, 0.0, 0.0, 60.0),
        UNBOUNDED,
    )
    .unwrap();
    assert_eq!(
        order(&zero_weights),
        (0u8..48).map(id).zip(1usize..).collect::<Vec<_>>()
    );
}

#[test]
fn invalid_parameters_are_refused_before_scoring() {
    for (weights, k, refusal) in [
        (
            [f64::NAN, 1.0, 1.0],
            60.0,
            ParameterRefusal::Weight(Lane::Exact),
        ),
        (
            [1.0, f64::INFINITY, 1.0],
            60.0,
            ParameterRefusal::Weight(Lane::Lexical),
        ),
        (
            [1.0, 1.0, -0.5],
            60.0,
            ParameterRefusal::Weight(Lane::Dense),
        ),
        ([1.0, 1.0, 1.0], 0.0, ParameterRefusal::K),
        ([1.0, 1.0, 1.0], -3.0, ParameterRefusal::K),
        ([1.0, 1.0, 1.0], f64::NAN, ParameterRefusal::K),
        ([1.0, 1.0, 1.0], f64::INFINITY, ParameterRefusal::K),
        (
            [f64::MAX, f64::MAX, f64::MAX],
            0.5,
            ParameterRefusal::SumOverflow,
        ),
    ] {
        assert_eq!(
            FusionParameters::new(weights_of(weights), k).unwrap_err(),
            refusal,
            "{weights:?} k={k}"
        );
    }
    let negative_zero = FusionParameters::new(weights_of([-0.0, 1.0, 1.0]), 60.0).unwrap();
    assert_eq!(
        negative_zero.weight(Lane::Exact).to_bits(),
        0.0f64.to_bits()
    );
}

#[test]
fn empty_lanes_all_zero_weights_and_equal_scores_return_the_declared_result() {
    let (a, b, c) = (id(0x0a), id(0x0b), id(0x0c));
    let parameters = params(1.0, 1.0, 1.0, 60.0);

    let empty = fuse(DeclaredLanes::admit([]).unwrap(), &parameters, UNBOUNDED).unwrap();
    assert!(empty.entries().is_empty());
    assert_eq!(
        empty.undeclared_lanes().collect::<Vec<_>>(),
        Lane::ORDER.to_vec()
    );

    let declared_empty = fuse(
        DeclaredLanes::admit([exact([]), lexical([])]).unwrap(),
        &parameters,
        UNBOUNDED,
    )
    .unwrap();
    assert!(declared_empty.entries().is_empty());
    assert_eq!(
        declared_empty.undeclared_lanes().collect::<Vec<_>>(),
        vec![Lane::Dense]
    );

    let zero = fuse(
        DeclaredLanes::admit([lexical([(c, -9.0), (a, -5.0)]), dense([(b, 0.9)])]).unwrap(),
        &params(0.0, 0.0, 0.0, 60.0),
        UNBOUNDED,
    )
    .unwrap();
    assert_eq!(order(&zero), vec![(a, 1), (b, 2), (c, 3)]);
    assert!(
        zero.entries()
            .iter()
            .all(|entry| entry.score().to_bits() == 0.0f64.to_bits())
    );

    let equal = fuse(
        DeclaredLanes::admit([exact([c, b, a])]).unwrap(),
        &parameters,
        UNBOUNDED,
    )
    .unwrap();
    assert_eq!(order(&equal), vec![(a, 1), (b, 2), (c, 3)]);
    let expected: f64 = 1.0 / (60.0 + 1.0);
    assert_eq!(
        equal.entries()[0].score().to_bits(),
        expected.to_bits(),
        "an exact set ranks its members in identifier order at positions one to n"
    );
    assert!(equal.entries()[1].score() < equal.entries()[0].score());
}

fn synthetic_id() -> impl Strategy<Value = OccurrenceId> {
    (0u8..48).prop_map(id)
}

fn lexical_hits() -> impl Strategy<Value = Vec<LaneHit>> {
    prop::collection::vec(
        (synthetic_id(), -50i16..0)
            .prop_map(|(occurrence, rank)| hit(occurrence, RawScore::Lexical(f64::from(rank)))),
        0..20,
    )
}

fn dense_hits() -> impl Strategy<Value = Vec<LaneHit>> {
    prop::collection::vec(
        (synthetic_id(), 0u16..1000).prop_map(|(occurrence, score)| {
            hit(occurrence, RawScore::Dense(f64::from(score) / 1000.0))
        }),
        0..20,
    )
}

fn exact_hits() -> impl Strategy<Value = Vec<LaneHit>> {
    prop::collection::vec(
        synthetic_id().prop_map(|occurrence| hit(occurrence, RawScore::Exact)),
        0..20,
    )
}

fn weights() -> impl Strategy<Value = ([f64; 3], f64)> {
    (
        [0u16..40, 0u16..40, 0u16..40].prop_map(|w| w.map(|v| f64::from(v) / 10.0)),
        (1u32..12_000).prop_map(|n| f64::from(n) / 100.0),
    )
}

fn snapshot(fused: &Fused) -> Vec<(OccurrenceId, usize, u64)> {
    fused
        .entries()
        .iter()
        .map(|e| (*e.occurrence(), e.position().get(), e.score().to_bits()))
        .collect()
}

type Fixture = (Vec<LaneHit>, Vec<LaneHit>, Vec<LaneHit>, [f64; 3], f64);

#[test]
fn lane_order_probe_duplication_and_entry_order_change_nothing() {
    runner()
        .run(
            &(exact_hits(), lexical_hits(), dense_hits(), weights()).prop_flat_map(
                |(exact, lexical, dense, (weights, k))| {
                    let doubled = |hits: &Vec<LaneHit>| {
                        Just(hits.iter().chain(hits).copied().collect::<Vec<_>>()).prop_shuffle()
                    };
                    (
                        Just::<Fixture>((
                            exact.clone(),
                            lexical.clone(),
                            dense.clone(),
                            weights,
                            k,
                        )),
                        doubled(&exact),
                        doubled(&lexical),
                        doubled(&dense),
                        Just(Lane::ORDER.to_vec()).prop_shuffle(),
                    )
                },
            ),
            |((exact_hits, lexical_hits, dense_hits, weights, k), ex2, lx2, dn2, lane_order)| {
                let parameters = FusionParameters::new(weights_of(weights), k).unwrap();
                let reference = fuse(
                    DeclaredLanes::admit([
                        lane(Lane::Exact, exact_hits.clone()),
                        lane(Lane::Lexical, lexical_hits.clone()),
                        lane(Lane::Dense, dense_hits.clone()),
                    ])
                    .unwrap(),
                    &parameters,
                    UNBOUNDED,
                )
                .unwrap();
                let rankings: Vec<LaneRanking> = lane_order
                    .iter()
                    .map(|l| match l {
                        Lane::Exact => lane(Lane::Exact, ex2.clone()),
                        Lane::Lexical => lane(Lane::Lexical, lx2.clone()),
                        Lane::Dense => lane(Lane::Dense, dn2.clone()),
                    })
                    .collect();
                let permuted = fuse(
                    DeclaredLanes::admit(rankings).unwrap(),
                    &parameters,
                    UNBOUNDED,
                )
                .unwrap();
                prop_assert_eq!(snapshot(&permuted), snapshot(&reference));
                prop_assert_eq!(
                    permuted.undeclared_lanes().collect::<Vec<_>>(),
                    reference.undeclared_lanes().collect::<Vec<_>>()
                );

                let mut previous: Option<(f64, OccurrenceId)> = None;
                for (entry, expected) in reference.entries().iter().zip(1usize..) {
                    prop_assert_eq!(entry.position().get(), expected);
                    let positions = Lane::ORDER.map(|l| entry.lane(l).map(|c| c.position().get()));
                    prop_assert_eq!(
                        entry.score().to_bits(),
                        oracle_score(&parameters, positions).to_bits()
                    );
                    if let Some((score, occurrence)) = previous {
                        prop_assert!(
                            score > entry.score()
                                || (score.to_bits() == entry.score().to_bits()
                                    && occurrence < *entry.occurrence())
                        );
                    }
                    previous = Some((entry.score(), *entry.occurrence()));
                }
                let distinct: BTreeSet<OccurrenceId> = exact_hits
                    .iter()
                    .chain(&lexical_hits)
                    .chain(&dense_hits)
                    .map(|h| h.occurrence)
                    .collect();
                prop_assert_eq!(reference.entries().len(), distinct.len());

                // Negative control against the fused output: a per-probe vote would add the lexical term once per duplicate hit.
                if let Some(first) = lexical_hits.first()
                    && weights[1] > 0.0
                {
                    let duplicates = lexical_hits
                        .iter()
                        .filter(|h| h.occurrence == first.occurrence)
                        .count();
                    let entry = reference
                        .entries()
                        .iter()
                        .find(|e| *e.occurrence() == first.occurrence)
                        .unwrap();
                    let positions = Lane::ORDER.map(|l| entry.lane(l).map(|c| c.position().get()));
                    let single = oracle_score(&parameters, positions);
                    let lexical_term = parameters.weight(Lane::Lexical)
                        / (parameters.k() + positions[1].unwrap() as f64);
                    let per_probe = single + (duplicates - 1) as f64 * lexical_term;
                    prop_assert_eq!(entry.score().to_bits(), single.to_bits());
                    prop_assert_eq!(
                        duplicates > 1,
                        per_probe.to_bits() != entry.score().to_bits()
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn the_union_bound_refuses_before_materializing_the_excess() {
    let ids: Vec<OccurrenceId> = (0u8..6).map(id).collect();
    let parameters = params(1.0, 1.0, 1.0, 60.0);
    let lanes = || {
        DeclaredLanes::admit([
            exact(ids[..4].iter().copied()),
            lexical(ids[2..].iter().map(|o| (*o, -1.0))),
        ])
        .unwrap()
    };
    assert_eq!(
        fuse(lanes(), &parameters, NonZeroUsize::new(5).unwrap()).unwrap_err(),
        UnionExceeded { bound: 5 }
    );
    let exact_fit = fuse(lanes(), &parameters, NonZeroUsize::new(6).unwrap()).unwrap();
    assert_eq!(exact_fit.entries().len(), 6);
    let overlapping = DeclaredLanes::admit([
        exact(ids[..3].iter().copied()),
        lexical(ids[..3].iter().map(|o| (*o, -1.0))),
        dense(ids[..3].iter().map(|o| (*o, 0.5))),
    ])
    .unwrap();
    assert_eq!(
        fuse(overlapping, &parameters, NonZeroUsize::new(3).unwrap())
            .unwrap()
            .entries()
            .len(),
        3,
        "occurrences the union already holds do not consume the bound"
    );
}

#[test]
fn raw_scores_survive_and_filtering_keeps_positions_and_scores_without_rescoring() {
    let (a, b, c, d) = (id(0x0a), id(0x0b), id(0x0c), id(0x0d));
    let fused = fuse(
        DeclaredLanes::admit([
            lexical([(b, -9.25), (a, -5.5), (c, -1.75)]),
            dense([(a, 0.9375), (c, 0.5), (d, 0.125)]),
        ])
        .unwrap(),
        &params(1.0, 0.7, 1.3, 20.0),
        UNBOUNDED,
    )
    .unwrap();
    let before = snapshot(&fused);
    assert_eq!(
        fused.entries()[0].lane(Lane::Lexical).unwrap().raw_score(),
        RawScore::Lexical(-5.5)
    );
    assert_eq!(
        fused.entries()[0].lane(Lane::Dense).unwrap().raw_score(),
        RawScore::Dense(0.9375)
    );
    let absent_before: Vec<Lane> = fused.undeclared_lanes().collect();
    let filtered = fused.filter(|entry| *entry.occurrence() != c);
    let after = snapshot(&filtered);
    let expected: Vec<_> = before.iter().copied().filter(|(o, _, _)| *o != c).collect();
    assert_eq!(after, expected);
    assert_eq!(
        after.iter().map(|(_, p, _)| *p).collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
    assert_eq!(
        filtered.undeclared_lanes().collect::<Vec<_>>(),
        absent_before
    );
}
