//! The batched kernels must agree bit for bit with the single-row reference functions, including on exact half-integer quotients, values one ulp either side of them, and scales whose reciprocals are not normal.
//! The seed is fixed so a failing case reproduces.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::dense::codec::{Metric, RowLayout};
use retrieval::dense::scalar::{Encoder, Scales, encode_validated_into};

const SEED: [u8; 32] = *b"dense-kernel-agreement-seed-0001";

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: 2048,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    )
}

fn layout(dimension: usize) -> RowLayout {
    RowLayout {
        dimension: dimension as u32,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: 1e12,
    }
}

/// Every positive finite f32: subnormals, powers of two, and the largest values, whose reciprocals are subnormal.
fn scale() -> impl Strategy<Value = f32> {
    prop_oneof![
        (1u32..0x7f80_0000).prop_map(f32::from_bits),
        prop::sample::select(vec![
            f32::from_bits(1),
            f32::MIN_POSITIVE,
            f32::MAX,
            1.0 / 127.0,
            0.5,
            2.0f32.powi(-7),
            2.0f32.powi(-12),
        ]),
        (1.0e-4f32..1.0e-1),
    ]
}

/// A value whose quotient by `scale` is a half-integer, an integer, or one ulp either side of one, or any finite f32.
fn value_for(scale: f32) -> impl Strategy<Value = f32> {
    let near = (-260i32..=260, -2i32..=2, any::<bool>()).prop_map(move |(half, step, exact)| {
        let target = (half as f32 * 0.5) * scale;
        let target = if exact { target } else { target + f32::EPSILON * target * 0.5 };
        let mut value = target;
        for _ in 0..step.unsigned_abs() {
            value = if step > 0 { value.next_up() } else { value.next_down() };
        }
        value
    });
    let any_finite = any::<u32>().prop_filter_map("finite", |bits| {
        let value = f32::from_bits(bits);
        value.is_finite().then_some(value)
    });
    prop_oneof![4 => near, 1 => any_finite].prop_filter("finite", |value| value.is_finite())
}

fn row_and_scales() -> impl Strategy<Value = (Vec<f32>, Vec<f32>)> {
    prop::collection::vec(scale(), 1..100).prop_flat_map(|scales| {
        let values: Vec<_> = scales.iter().map(|scale| value_for(*scale)).collect();
        (values, Just(scales))
    })
}

#[test]
fn the_encoder_codes_every_row_as_the_quotient_does() {
    runner()
        .run(&row_and_scales(), |(row, scales)| {
            let layout = layout(row.len());
            let scales = Scales::from_values(scales, layout.dimension).unwrap();
            let mut expected = Vec::new();
            let expected_clipped = encode_validated_into(&layout, &scales, &row, &mut expected);
            let mut codes = vec![i8::MIN; 3];
            let clipped = Encoder::new(&scales).encode_validated_into(&layout, &row, &mut codes);
            prop_assert_eq!((codes, clipped), (expected, expected_clipped));
            Ok(())
        })
        .unwrap();
}

/// Every block of a production-width row takes the product path except those holding a tie, which the quotient settles.
#[test]
fn ties_and_their_neighbours_code_as_the_quotient_does_in_wide_rows() {
    let dimension = 768;
    let layout = layout(dimension);
    for scale in [1.0f32 / 127.0, 2.0f32.powi(-7), 3.3e-3, 0.0123] {
        let scales = Scales::from_values(vec![scale; dimension], dimension as u32).unwrap();
        let encoder = Encoder::new(&scales);
        for offset in -3i32..=3 {
            let row: Vec<f32> = (0..dimension)
                .map(|i| {
                    let mut value = ((i as f32 - 384.0) * 0.5) * scale;
                    for _ in 0..offset.unsigned_abs() {
                        value = if offset > 0 { value.next_up() } else { value.next_down() };
                    }
                    value
                })
                .collect();
            let mut expected = Vec::new();
            let expected_clipped = encode_validated_into(&layout, &scales, &row, &mut expected);
            let mut codes = Vec::new();
            let clipped = encoder.encode_validated_into(&layout, &row, &mut codes);
            assert_eq!((codes, clipped), (expected, expected_clipped), "scale {scale}, offset {offset}");
        }
    }
}
