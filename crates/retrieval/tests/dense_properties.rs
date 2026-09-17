//! Property tests cover ranking laws over arbitrary offer orders and dense ties; `dense_oracle.rs` covers exact examples through the projection.
//! The seed is fixed so a failing case reproduces.

use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::scalar::{Scales, encode, weighted_dot};
use retrieval::dense::score::TopK;
use retrieval::dense::{BLOCK_ROWS, Ranked, inner_product, inner_product_block};

const SEED: [u8; 32] = *b"dense-ranking-laws-seed-00000001";

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

/// Few distinct scores and identifiers so ties at the cut are common.
fn scored_rows() -> impl Strategy<Value = Vec<(f64, String)>> {
    let score = prop::sample::select(vec![-1.0f64, -0.0, 0.0, 0.5, 0.5f64.next_up(), 1.0]);
    let id = prop::sample::select(vec!["a", "b", "c", "d", "e", "f", "g", "h"]);
    prop::collection::vec((score, id), 0..12).prop_map(|rows| {
        let mut seen = std::collections::BTreeSet::new();
        rows.into_iter()
            .filter(|(_, id)| seen.insert(*id))
            .map(|(score, id)| (score, id.to_string()))
            .collect()
    })
}

/// The model is written inline, not through `rank_order`, so it is an independent statement of the order.
fn model(mut rows: Vec<(f64, String)>, k: usize) -> Vec<(u64, String)> {
    rows.sort_by(|(a, a_id), (b, b_id)| {
        b.total_cmp(a)
            .then_with(|| a_id.as_bytes().cmp(b_id.as_bytes()))
    });
    rows.truncate(k);
    rows.into_iter()
        .map(|(score, id)| (score.to_bits(), id))
        .collect()
}

#[test]
fn top_k_equals_sort_then_truncate_for_every_offer_order() {
    runner()
        .run(
            &(scored_rows(), 1usize..=8).prop_flat_map(|(rows, k)| {
                let n = rows.len();
                (
                    Just(rows),
                    Just(k),
                    Just(n).prop_perturb(|n, mut rng| {
                        let mut order: Vec<usize> = (0..n).collect();
                        for i in (1..n).rev() {
                            let j = rng.random_range(0..=i);
                            order.swap(i, j);
                        }
                        order
                    }),
                )
            }),
            |(rows, k, order)| {
                let mut top: TopK<()> = TopK::new(NonZeroUsize::new(k).unwrap());
                let mut offered: Vec<(f64, String)> = Vec::new();
                for index in order {
                    let (score, id) = &rows[index];
                    let ranked = Ranked {
                        occurrence_id: id.clone(),
                        class: OccurrenceClass::Messages,
                        score: *score,
                    };
                    // `admits` must say exactly whether the row belongs to the top-K of everything offered so far plus itself.
                    let mut with_row = offered.clone();
                    with_row.push((*score, id.clone()));
                    let belongs = model(with_row, k).iter().any(|(_, member)| member == id);
                    prop_assert_eq!(top.admits(*score, id), belongs, "row {} at k={}", id, k);
                    top.offer(ranked, ());
                    offered.push((*score, id.clone()));
                }
                let ranked: Vec<(u64, String)> = top
                    .into_ranked()
                    .into_iter()
                    .map(|(row, ())| (row.score.to_bits(), row.occurrence_id))
                    .collect();
                prop_assert_eq!(ranked, model(rows, k));
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn finite_rows_round_trip_through_bytes_and_back_bit_for_bit() {
    runner()
        .run(
            &prop::collection::vec(any::<u32>(), 1..16).prop_map(|bits| {
                bits.into_iter()
                    .map(f32::from_bits)
                    .filter(|value| value.is_finite())
                    .collect::<Vec<f32>>()
            }),
            |row| {
                let bytes = codec::encode(&row);
                let decoded = codec::decode_shape(&bytes, row.len() as u32).unwrap();
                prop_assert_eq!(
                    decoded.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    row.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
                );
                prop_assert_eq!(codec::encode(&decoded), bytes);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn the_first_non_finite_coordinate_is_the_one_named() {
    runner()
        .run(
            &(
                prop::collection::vec(-1.0f32..1.0, 2..12),
                prop::collection::vec(
                    prop::sample::select(vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
                    1..4,
                ),
                any::<usize>(),
            ),
            |(mut row, poison, seed)| {
                let first = seed % row.len();
                for (offset, value) in poison.into_iter().enumerate() {
                    if first + offset < row.len() {
                        row[first + offset] = value;
                    }
                }
                prop_assert_eq!(
                    codec::validate_shape(&row, row.len() as u32),
                    Err(codec::RowRejection::NonFinite { coordinate: first })
                );
                Ok(())
            },
        )
        .unwrap();
}

fn positive_scales() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(
        prop::sample::select(vec![1.0f32, 0.5, 0.25, 1.0 / 127.0, 3.0e-3, 2.0, 1.0e-6]),
        1..12,
    )
}

/// The independently written in-order model must match bit for bit over the full code range `[-127, 127]`, so any reassociation, fusion, or order change in `weighted_dot` fails here.
#[test]
fn weighted_dot_matches_the_in_order_model_over_the_full_code_range() {
    runner()
        .run(
            &positive_scales().prop_flat_map(|scales| {
                let n = scales.len();
                (
                    Just(scales),
                    prop::collection::vec(-127i8..=127, n),
                    prop::collection::vec(-127i8..=127, n),
                )
            }),
            |(scales, query, doc)| {
                let dimension = scales.len() as u32;
                let scales = Scales::from_values(scales, dimension).unwrap();
                let mut model = 0.0f64;
                for j in 0..query.len() {
                    let s = f64::from(scales.as_slice()[j]);
                    let product = i32::from(query[j]) * i32::from(doc[j]);
                    model += (s * s) * f64::from(product);
                }
                prop_assert_eq!(
                    weighted_dot(&scales, &query, &doc).to_bits(),
                    model.to_bits()
                );
                Ok(())
            },
        )
        .unwrap();
}

/// Codes stay in `[-127, 127]`, `clipped` counts exactly the quotients outside that range, and an in-range quotient rounds ties to even.
#[test]
fn encoding_codes_are_clamped_counted_and_rounded_to_even() {
    runner()
        .run(
            &positive_scales().prop_flat_map(|scales| {
                let n = scales.len();
                (Just(scales), prop::collection::vec(-2.0f32..2.0, n))
            }),
            |(scales, row)| {
                let dimension = scales.len() as u32;
                let layout = RowLayout {
                    dimension,
                    metric: Metric::InnerProduct,
                    unit_norm_tolerance: 1e12,
                };
                let scales = Scales::from_values(scales, dimension).unwrap();
                let Ok(encoded) = encode(&layout, &scales, &row) else {
                    // The only rejection a finite row can draw here is a zero norm.
                    prop_assert!(row.iter().all(|v| *v == 0.0));
                    return Ok(());
                };
                let mut clipped = 0u32;
                for ((value, scale), code) in row.iter().zip(scales.as_slice()).zip(&encoded.codes)
                {
                    let quotient = (f64::from(*value) / f64::from(*scale)).round_ties_even();
                    let code = *code;
                    prop_assert!((-127..=127).contains(&code));
                    if quotient > 127.0 || quotient < -127.0 {
                        clipped += 1;
                        prop_assert_eq!(code, if quotient > 0.0 { 127 } else { -127 });
                    } else {
                        prop_assert_eq!(f64::from(code), quotient);
                    }
                }
                prop_assert_eq!(encoded.clipped, clipped);
                Ok(())
            },
        )
        .unwrap();
}

/// Any f32 bit pattern except the non-finite ones: both signs of zero, subnormals, and every exponent.
fn finite_f32() -> impl Strategy<Value = f32> {
    any::<u32>().prop_filter_map("finite", |bits| {
        let value = f32::from_bits(bits);
        value.is_finite().then_some(value)
    })
}

/// Blocks of eight rows against one query, all with the same length.
fn block_rows() -> impl Strategy<Value = (Vec<f32>, Vec<Vec<f32>>)> {
    (1usize..=40).prop_flat_map(|dimension| {
        (
            prop::collection::vec(finite_f32(), dimension),
            prop::collection::vec(prop::collection::vec(finite_f32(), dimension), BLOCK_ROWS),
        )
    })
}

/// Each lane of the block must equal the single-row functions bit for bit, over every f32 exponent and both signed zeros, so any lane crossing, reassociation, or fusion in the tiled loop fails here.
#[test]
fn inner_product_block_matches_the_single_row_functions_bit_for_bit() {
    runner()
        .run(&block_rows(), |(query, rows)| {
            let lanes: [&[f32]; BLOCK_ROWS] = std::array::from_fn(|lane| rows[lane].as_slice());
            let sums = inner_product_block(&query, &lanes);
            for (lane, row) in rows.iter().enumerate() {
                prop_assert_eq!(
                    sums.scores[lane].to_bits(),
                    inner_product(&query, row).to_bits(),
                    "score of lane {}",
                    lane
                );
                let mut squares = 0.0f64;
                for value in row {
                    let widened = f64::from(*value);
                    squares += widened * widened;
                }
                prop_assert_eq!(
                    sums.sums_of_squares[lane].to_bits(),
                    squares.to_bits(),
                    "sum of squares of lane {}",
                    lane
                );
            }
            Ok(())
        })
        .unwrap();
}

/// Validation from a block's sum of squares decides and names rejections exactly as the scalar validator does, including a non-finite coordinate anywhere in the row, a zero norm, and a norm outside the tolerance.
#[test]
fn validate_from_sum_agrees_with_validate_on_every_row() {
    runner()
        .run(
            &(
                block_rows(),
                0usize..BLOCK_ROWS,
                any::<usize>(),
                0u8..4,
                0.0f64..2.0,
            ),
            |((query, mut rows), lane, seed, poison, tolerance)| {
                let dimension = query.len();
                match poison {
                    1 => rows[lane][seed % dimension] = f32::NAN,
                    2 => rows[lane][seed % dimension] = f32::NEG_INFINITY,
                    3 => rows[lane].iter_mut().for_each(|value| *value = 0.0),
                    _ => {}
                }
                let layout = RowLayout {
                    dimension: dimension as u32,
                    metric: Metric::InnerProduct,
                    unit_norm_tolerance: tolerance,
                };
                let lanes: [&[f32]; BLOCK_ROWS] = std::array::from_fn(|lane| rows[lane].as_slice());
                let sums = inner_product_block(&query, &lanes);
                for (lane, row) in rows.iter().enumerate() {
                    prop_assert_eq!(
                        codec::validate_from_sum(row, &layout, sums.sums_of_squares[lane]),
                        codec::validate(row, &layout),
                        "lane {}",
                        lane
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}
