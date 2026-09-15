//! Property tests cover ranking laws over arbitrary offer orders and dense ties; `dense_oracle.rs` covers exact examples through the projection.
//! The seed is fixed so a failing case reproduces.

use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::dense::Ranked;
use retrieval::dense::codec;
use retrieval::dense::score::TopK;

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
                for index in order {
                    let (score, id) = &rows[index];
                    top.offer(
                        Ranked {
                            occurrence_id: id.clone(),
                            class: OccurrenceClass::Messages,
                            score: *score,
                        },
                        (),
                    );
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
