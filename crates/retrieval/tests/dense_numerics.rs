//! RP2.6.U1 numerical witnesses: the checked candidate capacity, the quantized query transform, and agreement of the production scorers and order with references written here from the formulas.
//! Every reference loop below is independent of `retrieval::dense`; fixtures are literal f32 bits or literal codes so a failure replays exactly.

use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use retrieval::dense::codec::{Metric, RowLayout, RowRejection};
use retrieval::dense::scalar::{QuantizedQuery, QueryRefusal, Scales, encode};
use retrieval::dense::score::TopK;
use retrieval::dense::{
    CandidateCapacity, CandidatePolicy, CapacityRefusal, Ranked, inner_product, rank_order, rescore,
};

const DIMENSION: u32 = 4;

fn layout() -> RowLayout {
    RowLayout {
        dimension: DIMENSION,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: 1e-3,
    }
}

fn policy(alpha: f64, cap: usize) -> CandidatePolicy {
    CandidatePolicy {
        alpha,
        cap: nz(cap),
    }
}

fn nz(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

// ---- Independent references ----

/// `sum_j (s_j * s_j) * (q_j * d_j)`: weight and integer product widened to f64 before they multiply, summed from `+0.0` in coordinate order.
fn reference_quantized(scales: &[f32], query: &[i8], doc: &[i8]) -> f64 {
    let mut total = 0.0f64;
    for index in 0..scales.len() {
        let scale = scales[index] as f64;
        let weight = scale * scale;
        let product = (query[index] as i32) * (doc[index] as i32);
        total += weight * (product as f64);
    }
    total
}

/// The f32 rows widened to f64 before each multiplication, summed from `+0.0` in coordinate order.
fn reference_f32(query: &[f32], row: &[f32]) -> f64 {
    let mut total = 0.0f64;
    for index in 0..query.len() {
        total += (query[index] as f64) * (row[index] as f64);
    }
    total
}

/// The full sort the production order must match: score descending, then identifier bytes ascending, with no tolerance.
/// Plain comparisons equal `total_cmp` on finite scores other than `-0.0`, which the scorers never produce, so every input is checked for that domain.
fn reference_order(mut scored: Vec<(f64, String)>) -> Vec<(u64, String)> {
    for (score, id) in &scored {
        assert!(score.is_finite(), "{id} scores finite");
        assert_ne!(score.to_bits(), (-0.0f64).to_bits(), "{id} scores no -0.0");
    }
    scored.sort_by(|(a, a_id), (b, b_id)| {
        if a > b {
            std::cmp::Ordering::Less
        } else if a < b {
            std::cmp::Ordering::Greater
        } else {
            a_id.as_bytes().cmp(b_id.as_bytes())
        }
    });
    scored
        .into_iter()
        .map(|(score, id)| (score.to_bits(), id))
        .collect()
}

/// Negative control: the unweighted integer dot every scale-weighted fixture must separate from.
fn unweighted(query: &[i8], doc: &[i8]) -> f64 {
    query
        .iter()
        .zip(doc)
        .map(|(q, d)| f64::from(i32::from(*q) * i32::from(*d)))
        .sum()
}

/// Negative control: the weight squared in f32 and the product rounded to f32 before the f64 sum.
fn f32_first_quantized(scales: &[f32], query: &[i8], doc: &[i8]) -> f64 {
    let mut total = 0.0f64;
    for index in 0..scales.len() {
        let weight = scales[index] * scales[index];
        let term = weight * (i32::from(query[index]) * i32::from(doc[index])) as f32;
        total += f64::from(term);
    }
    total
}

/// Negative control: f32 coordinates multiplied in f32 before the f64 sum.
fn f32_first_original(query: &[f32], row: &[f32]) -> f64 {
    let mut total = 0.0f64;
    for (q, r) in query.iter().zip(row) {
        total += f64::from(q * r);
    }
    total
}

// ---- Fixture ----

fn bits(words: [u32; 4]) -> Vec<f32> {
    words.into_iter().map(f32::from_bits).collect()
}

/// Unequal scales: coordinate 0 is calibrated two hundred times finer than coordinate 1.
fn fixture_scales() -> Scales {
    Scales::from_values(
        bits([0x3a81_0204, 0x3e21_4285, 0x3c81_0204, 0x3d01_0204]),
        DIMENSION,
    )
    .unwrap()
}

/// Literal codes per identifier. `dup-a` and `dup-b` carry equal codes; `neg` scores below zero; `zero` scores exactly zero; `max` carries the code extrema.
fn fixture_codes() -> Vec<(&'static str, [i8; 4])> {
    vec![
        ("fine", [127, 0, 3, 0]),
        ("coarse", [0, 2, 0, 0]),
        ("dup-b", [40, 1, 10, -5]),
        ("dup-a", [40, 1, 10, -5]),
        ("neg", [-90, -1, 0, 7]),
        ("zero", [0, 0, 0, 0]),
        ("max", [127, -127, 127, -127]),
        ("mixed", [10, 1, -20, 30]),
    ]
}

/// A unit query whose first coordinate dominates.
fn fixture_query() -> Vec<f32> {
    let raw = [0.8f32, 0.1, 0.5, -0.3];
    let norm = raw
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    raw.iter().map(|v| (f64::from(*v) / norm) as f32).collect()
}

fn production_quantized_order(
    query: &QuantizedQuery<'_>,
    rows: &[(&str, [i8; 4])],
    k: usize,
) -> Vec<(u64, String)> {
    let mut top = TopK::new(nz(k));
    for (id, codes) in rows {
        top.offer(
            Ranked {
                occurrence_id: (*id).to_owned(),
                class: OccurrenceClass::Messages,
                score: query.score(codes),
            },
            (),
        );
    }
    top.into_ranked()
        .into_iter()
        .map(|(ranked, ())| (ranked.score.to_bits(), ranked.occurrence_id))
        .collect()
}

// ---- Capacity ----

#[test]
fn the_capacity_is_ceil_alpha_times_k_and_never_below_k() {
    let capacity = CandidateCapacity::new(64, policy(4.0, 256))
        .unwrap()
        .unwrap();
    assert_eq!((capacity.k().get(), capacity.candidates().get()), (64, 256));
    let capacity = CandidateCapacity::new(10, policy(1.0, 10))
        .unwrap()
        .unwrap();
    assert_eq!(capacity.candidates().get(), 10, "alpha one pools exactly K");
    let capacity = CandidateCapacity::new(3, policy(1.5, 5)).unwrap().unwrap();
    assert_eq!(capacity.candidates().get(), 5, "ceil(4.5) is 5");
    for alpha in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0] {
        let capacity = CandidateCapacity::new(10, policy(alpha, 500))
            .unwrap()
            .unwrap();
        assert_eq!(capacity.candidates().get(), (alpha as usize) * 10);
    }
}

/// `1.1` is stored as `0x3FF199999999999A`, slightly above eleven tenths, so its exact product with ten exceeds eleven.
#[test]
fn the_capacity_takes_the_ceiling_of_the_exact_binary_product() {
    assert_eq!(1.1f64.to_bits(), 0x3FF1_9999_9999_999A);
    let capacity = CandidateCapacity::new(10, policy(1.1, 12))
        .unwrap()
        .unwrap();
    assert_eq!(capacity.candidates().get(), 12);
    assert_eq!(
        CandidateCapacity::new(10, policy(1.1, 11)),
        Err(CapacityRefusal::OverCap {
            candidates: 12,
            cap: 11
        })
    );
}

#[test]
fn a_malformed_alpha_refuses_before_zero_k_is_considered() {
    for alpha in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.999,
        0.0,
        -0.0,
        -1.0,
    ] {
        for k in [0, 1, 64] {
            match CandidateCapacity::new(k, policy(alpha, 256)) {
                Err(CapacityRefusal::Alpha { alpha: refused }) => {
                    assert_eq!(refused.to_bits(), alpha.to_bits());
                }
                other => panic!("alpha {alpha} with k {k}: {other:?}"),
            }
        }
    }
}

#[test]
fn zero_k_under_a_valid_alpha_is_an_empty_ranking_with_no_capacity() {
    assert_eq!(CandidateCapacity::new(0, policy(4.0, 256)), Ok(None));
    assert_eq!(
        CandidateCapacity::new(0, policy(f64::MAX, 1)),
        Ok(None),
        "no product is formed for zero K, so neither overflow nor cap applies"
    );
}

#[test]
fn an_unrepresentable_product_refuses_before_the_cap_is_consulted() {
    assert_eq!(
        CandidateCapacity::new(usize::MAX, policy(2.0, usize::MAX)),
        Err(CapacityRefusal::Unrepresentable {
            alpha: 2.0,
            k: usize::MAX
        })
    );
    assert_eq!(
        CandidateCapacity::new(1, policy(f64::MAX, 1)),
        Err(CapacityRefusal::Unrepresentable {
            alpha: f64::MAX,
            k: 1
        })
    );
    let largest = CandidateCapacity::new(usize::MAX, policy(1.0, usize::MAX))
        .unwrap()
        .unwrap();
    assert_eq!(largest.candidates().get(), usize::MAX);
}

#[test]
fn a_pool_over_the_cap_refuses_and_the_cap_itself_is_admitted() {
    assert_eq!(
        CandidateCapacity::new(64, policy(4.0, 255)),
        Err(CapacityRefusal::OverCap {
            candidates: 256,
            cap: 255
        })
    );
    assert!(
        CandidateCapacity::new(64, policy(4.0, 256))
            .unwrap()
            .is_some()
    );
}

/// Alpha of `2^53` and above takes the left-shift path, whose bits must be checked before they shift out.
#[test]
fn a_large_alpha_shifts_exactly_or_refuses() {
    let two_53 = 2f64.powi(53);
    let pool = |alpha: f64, k: usize| {
        CandidateCapacity::new(k, policy(alpha, usize::MAX)).map(|c| c.unwrap().candidates().get())
    };
    assert_eq!(pool(two_53, 1), Ok(1 << 53));
    assert_eq!(pool(two_53, 1 << 10), Ok(1 << 63));
    assert_eq!(
        pool(two_53, 1 << 11),
        Err(CapacityRefusal::Unrepresentable {
            alpha: two_53,
            k: 1 << 11
        })
    );
    let wide = 2f64.powi(100) * (1.0 + f64::EPSILON);
    assert_eq!(
        pool(wide, 1 << 40),
        Err(CapacityRefusal::Unrepresentable {
            alpha: wide,
            k: 1 << 40
        }),
        "the shifted product exceeds a u128"
    );
    assert_eq!(pool(1.0f64.next_up(), 1), Ok(2), "just above one rounds up");
}

/// Integer alphas and dyadic fractions `a / 2^s` have exact products computable in integers, independent of the f64 decomposition.
#[test]
fn the_capacity_equals_the_integer_ceiling_for_integer_and_dyadic_alphas() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 2048,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, b"dense-candidate-capacity-seed-01"),
    );
    let k = prop_oneof![1usize..=4096, any::<usize>().prop_map(|k| k.max(1))];
    runner
        .run(&(1u64..=(1 << 53), k.clone()), |(a, k)| {
            let expected = k.checked_mul(usize::try_from(a).unwrap());
            let actual = CandidateCapacity::new(k, policy(a as f64, usize::MAX))
                .ok()
                .map(|c| c.unwrap().candidates().get());
            prop_assert_eq!(actual, expected);
            Ok(())
        })
        .unwrap();
    runner
        .run(&(0u64..(1 << 19), 0u32..=20, k), |(half, s, k)| {
            let numerator = 2 * half + 1;
            let alpha = numerator as f64 / 2f64.powi(s as i32);
            prop_assume!(alpha >= 1.0);
            let exact = (u128::from(numerator) * k as u128).div_ceil(1u128 << s);
            let expected = usize::try_from(exact).ok();
            let actual = CandidateCapacity::new(k, policy(alpha, usize::MAX))
                .ok()
                .map(|c| c.unwrap().candidates().get());
            prop_assert_eq!(actual, expected);
            if let Some(pool) = actual {
                prop_assert!(pool >= k);
            }
            Ok(())
        })
        .unwrap();
}

// ---- Query transform ----

#[test]
fn the_query_transform_refuses_invalid_queries_before_any_code_exists() {
    let scales = fixture_scales();
    let cases: Vec<(Vec<f32>, QueryRefusal)> = vec![
        (
            vec![1.0, 0.0, 0.0],
            QueryRefusal::Row(RowRejection::Dimension {
                expected: 4,
                actual: 3,
            }),
        ),
        (
            vec![f32::NAN, 0.0, 0.0, 0.0],
            QueryRefusal::Row(RowRejection::NonFinite { coordinate: 0 }),
        ),
        (
            vec![0.0, f32::INFINITY, 0.0, 0.0],
            QueryRefusal::Row(RowRejection::NonFinite { coordinate: 1 }),
        ),
        (
            vec![0.0, -0.0, 0.0, 0.0],
            QueryRefusal::Row(RowRejection::ZeroNorm),
        ),
        (
            vec![2.0, 0.0, 0.0, 0.0],
            QueryRefusal::Row(RowRejection::Normalization { norm: 2.0 }),
        ),
    ];
    for (query, expected) in cases {
        assert_eq!(
            QuantizedQuery::new(&layout(), &scales, &query).unwrap_err(),
            expected
        );
    }
    let mut bad_layout = layout();
    bad_layout.unit_norm_tolerance = f64::NAN;
    assert!(matches!(
        QuantizedQuery::new(&bad_layout, &scales, &fixture_query()),
        Err(QueryRefusal::Row(RowRejection::Tolerance { .. }))
    ));
    let wide = Scales::from_values(vec![1.0; 5], 5).unwrap();
    assert_eq!(
        QuantizedQuery::new(&layout(), &wide, &fixture_query()).unwrap_err(),
        QueryRefusal::ScalesDimension {
            expected: 4,
            actual: 5
        }
    );
}

/// Scale one on every coordinate maps a unit query of four equal coordinates (one half each) to round-half-even zero.
#[test]
fn a_query_whose_every_code_is_zero_is_refused() {
    let ones = Scales::from_values(vec![1.0; 4], DIMENSION).unwrap();
    let query = vec![0.5f32, -0.5, 0.5, -0.5];
    assert_eq!(
        QuantizedQuery::new(&layout(), &ones, &query).unwrap_err(),
        QueryRefusal::ZeroCodes
    );
    let one_hot = vec![1.0f32, 0.0, 0.0, 0.0];
    let accepted = QuantizedQuery::new(&layout(), &ones, &one_hot).unwrap();
    assert_eq!(accepted.codes(), &[1, 0, 0, 0]);
}

#[test]
fn the_query_codes_are_the_stored_row_encoding_under_the_same_scales() {
    let scales = fixture_scales();
    let query = fixture_query();
    let quantized = QuantizedQuery::new(&layout(), &scales, &query).unwrap();
    assert_eq!(
        quantized.codes(),
        &encode(&layout(), &scales, &query).unwrap().codes[..]
    );
}

// ---- Quantized scoring and order ----

#[test]
fn quantized_scores_and_order_match_the_independent_reference_and_full_sort() {
    let scales = fixture_scales();
    let query = QuantizedQuery::new(&layout(), &scales, &fixture_query()).unwrap();
    let rows = fixture_codes();
    let scored: Vec<(f64, String)> = rows
        .iter()
        .map(|(id, codes)| {
            let reference = reference_quantized(scales.as_slice(), query.codes(), codes);
            assert_eq!(
                query.score(codes).to_bits(),
                reference.to_bits(),
                "{id} score bits"
            );
            (reference, (*id).to_owned())
        })
        .collect();
    let expected = reference_order(scored);
    for k in 1..=rows.len() {
        assert_eq!(
            production_quantized_order(&query, &rows, k),
            expected[..k].to_vec(),
            "k {k}"
        );
    }
    // Equal codes keep both identities, smaller bytes first, and a cut inside the tie keeps the smaller.
    let ids: Vec<&str> = expected.iter().map(|(_, id)| id.as_str()).collect();
    let first = ids.iter().position(|id| *id == "dup-a").unwrap();
    assert_eq!(ids[first + 1], "dup-b");
    assert_eq!(
        production_quantized_order(&query, &rows, first + 1)
            .last()
            .unwrap()
            .1,
        "dup-a"
    );
}

/// Pinned so a change to the scorer, the scales, or the order is a visible diff; the list was produced by the references above.
#[test]
fn the_fixture_order_is_pinned() {
    let scales = fixture_scales();
    let query = QuantizedQuery::new(&layout(), &scales, &fixture_query()).unwrap();
    assert_eq!(query.codes(), &[127, 1, 32, -10]);
    let ids: Vec<String> = production_quantized_order(&query, &fixture_codes(), 8)
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert_eq!(
        ids,
        [
            "dup-a", "dup-b", "coarse", "fine", "zero", "neg", "mixed", "max"
        ]
    );
}

#[test]
fn negative_and_zero_quantized_scores_order_numerically_and_zero_is_positive() {
    let scales = fixture_scales();
    let query = QuantizedQuery::new(&layout(), &scales, &fixture_query()).unwrap();
    let zero = query.score(&[0, 0, 0, 0]);
    assert_eq!(zero.to_bits(), 0.0f64.to_bits(), "+0.0, not -0.0");
    let negative = query.score(&[-90, -1, 0, 7]);
    assert!(negative < 0.0);
    assert_eq!(
        rank_order((zero, "z"), (negative, "a")),
        std::cmp::Ordering::Less,
        "zero outranks a negative score whatever the identifiers"
    );
}

#[test]
fn extreme_scales_and_codes_score_finite_and_match_the_reference() {
    let extremes = [
        vec![f32::MAX; 4],
        vec![f32::from_bits(1); 4],
        vec![f32::MIN_POSITIVE, f32::MAX, 1.0, f32::from_bits(1)],
    ];
    let codes = [[127i8, -127, 127, -127], [-127, 127, -127, 127], [127; 4]];
    for scale_values in extremes {
        let scales = Scales::from_values(scale_values.clone(), DIMENSION).unwrap();
        for query in &codes {
            for doc in &codes {
                let reference = reference_quantized(&scale_values, query, doc);
                let production = retrieval::dense::scalar::weighted_dot(&scales, query, doc);
                assert!(production.is_finite());
                assert_eq!(production.to_bits(), reference.to_bits());
            }
        }
    }
}

/// The weighted fixture ranks `coarse`, one step on the coarse coordinate, above `fine`, the largest code on the fine coordinate; the unweighted dot reverses them, so the oracle comparison rejects it.
#[test]
fn the_fixture_rejects_unweighted_and_f32_first_quantized_scoring() {
    let scales = fixture_scales();
    let query = QuantizedQuery::new(&layout(), &scales, &fixture_query()).unwrap();
    let rows = fixture_codes();
    let order_of = |score: &dyn Fn(&[i8]) -> f64| {
        reference_order(
            rows.iter()
                .map(|(id, codes)| (score(codes), (*id).to_owned()))
                .collect(),
        )
    };
    let weighted = order_of(&|codes| reference_quantized(scales.as_slice(), query.codes(), codes));
    let raw = order_of(&|codes| unweighted(query.codes(), codes));
    let position = |order: &[(u64, String)], id: &str| order.iter().position(|(_, row)| row == id);
    assert!(position(&weighted, "coarse") < position(&weighted, "fine"));
    assert!(
        position(&raw, "coarse") > position(&raw, "fine"),
        "the unweighted dot reverses them"
    );
    let mut bits_differ = false;
    for (_, codes) in &rows {
        let f32_first = f32_first_quantized(scales.as_slice(), query.codes(), codes);
        bits_differ |= f32_first.to_bits() != query.score(codes).to_bits();
    }
    assert!(
        bits_differ,
        "f32-first multiplication changes a score's bits"
    );
}

// ---- Retained-f32 scoring ----

/// Two rows whose scores differ by less than one f32 ulp: the f64 products separate them, the f32 products tie them.
#[test]
fn original_scores_separate_rows_an_f32_product_would_tie() {
    let query = bits([0x3f35_04f3, 0x3f35_04f3, 0, 0]);
    let upper = bits([0x3f35_04f3, 0x3f35_04f3, 0, 0]);
    let lower = bits([0x3f35_04f6, 0x3f35_04ef, 0, 0]);
    let (u, l) = (inner_product(&query, &upper), inner_product(&query, &lower));
    assert_eq!(u.to_bits(), reference_f32(&query, &upper).to_bits());
    assert_eq!(l.to_bits(), reference_f32(&query, &lower).to_bits());
    assert!(u > l, "f64 products separate the rows");
    assert_eq!(
        f32_first_original(&query, &upper).to_bits(),
        f32_first_original(&query, &lower).to_bits(),
        "f32 products tie them, so an f32-first scorer would order by identifier"
    );
}

#[test]
fn signed_zero_coordinates_score_positive_zero() {
    let query = bits([0x3f80_0000, 0x8000_0000, 0x8000_0000, 0]);
    let orthogonal = bits([0x8000_0000, 0x3f80_0000, 0, 0x8000_0000]);
    let score = inner_product(&query, &orthogonal);
    assert_eq!(score.to_bits(), 0.0f64.to_bits());
    assert_eq!(
        score.to_bits(),
        reference_f32(&query, &orthogonal).to_bits()
    );
}

#[test]
fn rescore_matches_the_independent_reference_and_full_sort_with_negatives_and_ties() {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let rows: Vec<(&str, Vec<f32>)> = vec![
        ("b-same", vec![s, s, 0.0, 0.0]),
        ("a-same", vec![s, s, 0.0, 0.0]),
        ("opposite", vec![-s, -s, 0.0, 0.0]),
        ("orthogonal", vec![0.0, 0.0, 1.0, 0.0]),
        ("half", vec![0.5, 0.5, 0.5, 0.5]),
        ("negative-zero", vec![-0.0, 0.0, -0.0, 1.0]),
    ];
    let query = vec![s, s, 0.0, 0.0];
    let ranked = rescore(
        &layout(),
        &query,
        rows.iter()
            .map(|(id, row)| (*id, OccurrenceClass::Messages, row.as_slice())),
    )
    .unwrap();
    let expected = reference_order(
        rows.iter()
            .map(|(id, row)| (reference_f32(&query, row), (*id).to_owned()))
            .collect(),
    );
    let actual: Vec<(u64, String)> = ranked
        .into_iter()
        .map(|row| (row.score.to_bits(), row.occurrence_id))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(actual[0].1, "a-same");
    assert_eq!(actual[1].1, "b-same");
    assert_eq!(actual.last().unwrap().1, "opposite");
}

#[test]
fn rescore_refuses_an_invalid_query_or_row_before_scoring() {
    let good = vec![1.0f32, 0.0, 0.0, 0.0];
    let rows = [("a", good.clone()), ("b", vec![1.0, f32::NAN, 0.0, 0.0])];
    assert_eq!(
        rescore(
            &layout(),
            &good,
            rows.iter()
                .map(|(id, row)| (*id, OccurrenceClass::Messages, row.as_slice()))
        ),
        Err(RowRejection::NonFinite { coordinate: 1 })
    );
    assert_eq!(
        rescore(
            &layout(),
            &[0.0, 0.0, 0.0, 0.0],
            std::iter::once(("a", OccurrenceClass::Messages, good.as_slice()))
        ),
        Err(RowRejection::ZeroNorm)
    );
}
