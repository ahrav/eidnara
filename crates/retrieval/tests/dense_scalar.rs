use kernel::source_identity::OccurrenceClass;
use retrieval::dense::codec::{self, Metric, RowLayout, RowRejection};
use retrieval::dense::scalar::{
    CODE_MAX, CODE_MIN, CalibrationRejection, ScalarBytesRejection, ScalarRecipe, Scales,
    calibrate, decode_codes, encode, encode_codes, weighted_dot,
};
use retrieval::dense::{inner_product, rank_order, rescore};

const DIMENSION: u32 = 8;

fn unit_layout() -> RowLayout {
    RowLayout {
        dimension: DIMENSION,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: 1e-3,
    }
}

/// A layout that admits any finite nonzero row, for exercising rounding and clipping on chosen values.
fn loose_layout() -> RowLayout {
    RowLayout {
        dimension: DIMENSION,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: 1e12,
    }
}

fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

fn scales_of(values: [f32; 8]) -> Scales {
    Scales::from_values(values.to_vec(), DIMENSION).unwrap()
}

fn corpus() -> Vec<Vec<f32>> {
    vec![
        unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1]),
        unit([0.7, 0.3, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0]),
        unit([0.5, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0]),
        unit([0.3, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0]),
        unit([-0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0]),
        unit([0.1, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0]),
    ]
}

#[test]
fn calibration_stores_max_abs_over_127_per_coordinate_in_f32_and_scale_one_for_a_zero_coordinate() {
    let rows = corpus();
    let calibration = calibrate(&unit_layout(), rows.iter().map(Vec::as_slice)).unwrap();
    // Coordinate 0 peaks in the first row, computed as one f32 division.
    assert_eq!(calibration.scales.as_slice()[0], rows[0][0] / 127.0f32);
    // Coordinate 3 is nonzero in exactly one row, so its scale is that value over 127.
    assert_eq!(calibration.scales.as_slice()[3], rows[2][3] / 127.0);
    // Coordinates 5 through 7 are zero in every row except one each; none is all-zero here.
    assert!(
        calibration
            .scales
            .as_slice()
            .iter()
            .all(|s| *s > 0.0 && s.is_finite())
    );
    assert_eq!(calibration.identity.recipe, ScalarRecipe::SymmetricInt8V1);
    assert_eq!(calibration.identity.calibrated_rows, 6);
    assert_eq!(
        calibration.identity.scales_digest,
        calibration.scales.digest()
    );

    let axis_only = [unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])];
    let one = calibrate(&unit_layout(), axis_only.iter().map(Vec::as_slice)).unwrap();
    assert_eq!(one.scales.as_slice()[0], 1.0 / 127.0);
    assert!(
        one.scales.as_slice()[1..].iter().all(|s| *s == 1.0),
        "all-zero coordinates take scale 1"
    );
    assert!(
        !format!("{:?}", one.scales).contains("0.0078"),
        "Debug hides scale values"
    );
}

#[test]
fn calibration_refuses_no_rows_invalid_rows_and_a_nonzero_scale_that_underflows() {
    assert_eq!(
        calibrate(&unit_layout(), std::iter::empty()),
        Err(CalibrationRejection::NoRows)
    );
    let unchecked = RowLayout {
        unit_norm_tolerance: f64::NAN,
        ..unit_layout()
    };
    let rows = [vec![1.0f32; 8]];
    assert!(matches!(
        calibrate(&unchecked, rows.iter().map(Vec::as_slice)),
        Err(CalibrationRejection::Layout(RowRejection::Tolerance { .. }))
    ));
    assert!(matches!(
        encode(&unchecked, &scales_of([1.0; 8]), &rows[0]),
        Err(RowRejection::Tolerance { .. })
    ));
    let bad = [unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), vec![0.0; 8]];
    assert_eq!(
        calibrate(&unit_layout(), bad.iter().map(Vec::as_slice)),
        Err(CalibrationRejection::Row {
            index: 1,
            rejection: RowRejection::ZeroNorm
        })
    );
    let short = [vec![1.0f32; 7]];
    assert_eq!(
        calibrate(&unit_layout(), short.iter().map(Vec::as_slice)),
        Err(CalibrationRejection::Row {
            index: 0,
            rejection: RowRejection::Dimension {
                expected: 8,
                actual: 7
            }
        })
    );
    // The smallest subnormal is nonzero, still within the unit-norm tolerance, and divides to zero.
    let tiny = f32::from_bits(1);
    assert!(tiny > 0.0 && tiny / 127.0 == 0.0);
    let underflow = [vec![1.0, tiny, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
    assert_eq!(
        calibrate(&unit_layout(), underflow.iter().map(Vec::as_slice)),
        Err(CalibrationRejection::ScaleUnderflow { coordinate: 1 })
    );
}

#[test]
fn encoding_rounds_ties_to_even_on_both_signs_and_separates_adjacent_values() {
    let scales = scales_of([1.0; 8]);
    let row = [0.5f32, 1.5, 2.5, -0.5, -1.5, -2.5, 3.5, -3.5];
    let encoded = encode(&loose_layout(), &scales, &row).unwrap();
    assert_eq!(encoded.codes, vec![0, 2, 2, 0, -2, -2, 4, -4]);
    assert_eq!(encoded.clipped, 0);

    let adjacent = [
        2.5f32.next_up(),
        2.5f32.next_down(),
        (-2.5f32).next_up(),
        (-2.5f32).next_down(),
        0.5f32.next_up(),
        0.5f32.next_down(),
        1.0,
        -1.0,
    ];
    let encoded = encode(&loose_layout(), &scales, &adjacent).unwrap();
    assert_eq!(encoded.codes, vec![3, 2, -2, -3, 1, 0, 1, -1]);

    // The quotient is formed in f64: with s = 1 − 2⁻²⁴ and x = 3.5 − 2⁻²², the exact quotient is
    // 3.49999997, so the code is 3; an f32 quotient would round to exactly 3.5 and tie to 4.
    let near_one = scales_of([f32::from_bits(0x3f7f_ffff); 8]);
    let x = f32::from_bits(0x405f_ffff);
    assert_eq!(
        (x / f32::from_bits(0x3f7f_ffff)),
        3.5,
        "the f32 quotient lands on the tie"
    );
    let encoded = encode(
        &loose_layout(),
        &near_one,
        &[x, -x, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    )
    .unwrap();
    assert_eq!(&encoded.codes[..2], &[3, -3]);

    // A scale other than one moves the rounding point with the quotient, not the value.
    let halves = scales_of([0.5; 8]);
    let row = [0.25f32, 0.75, 1.25, -0.25, -0.75, -1.25, 0.0, -0.0];
    let encoded = encode(&loose_layout(), &halves, &row).unwrap();
    assert_eq!(encoded.codes, vec![0, 2, 2, 0, -2, -2, 0, 0]);
}

#[test]
fn encoding_clips_to_plus_minus_127_counts_every_clip_and_never_yields_minus_128() {
    let scales = scales_of([1.0; 8]);
    let row = [
        200.0f32, -200.0, 127.4, -127.4, 127.5, -127.5, 126.5, -126.5,
    ];
    let encoded = encode(&loose_layout(), &scales, &row).unwrap();
    assert_eq!(
        encoded.codes,
        vec![127, -127, 127, -127, 127, -127, 126, -126]
    );
    assert_eq!(
        encoded.clipped, 4,
        "127.5 rounds to 128 and -127.5 to -128 before the clamp"
    );
    assert!(encoded.codes.iter().all(|c| *c >= CODE_MIN));
    assert_eq!(CODE_MAX, i8::MAX);
    assert!(!encoded.codes.contains(&i8::MIN));

    // A quotient far beyond i64 clamps before the cast instead of wrapping.
    let tiny = scales_of([1.0e-6; 8]);
    let any_norm = RowLayout {
        unit_norm_tolerance: f64::MAX,
        ..loose_layout()
    };
    let huge = encode(
        &any_norm,
        &tiny,
        &[1.0e12, -1.0e12, 3.0e38, -3.0e38, 0.0, 0.0, 0.0, 1.0],
    )
    .unwrap();
    assert_eq!(&huge.codes[..4], &[127, -127, 127, -127]);
    assert_eq!(huge.clipped, 5);

    // A calibrated corpus never clips its own rows: every value is at most 127 scales.
    let rows = corpus();
    let calibration = calibrate(&unit_layout(), rows.iter().map(Vec::as_slice)).unwrap();
    for row in &rows {
        let encoded = encode(&unit_layout(), &calibration.scales, row).unwrap();
        assert_eq!(encoded.clipped, 0);
        assert_eq!(encoded.codes.len(), 8);
    }
    // A query outside the calibrated range clips and the scales are unchanged.
    let before = calibration.scales.encode();
    let far = unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let encoded = encode(&unit_layout(), &calibration.scales, &far).unwrap();
    assert_eq!(encoded.codes[7], 127);
    assert!(encoded.clipped >= 1);
    assert_eq!(calibration.scales.encode(), before);
}

#[test]
fn encoding_refuses_rows_outside_the_layout_and_scales_of_another_dimension() {
    let scales = scales_of([1.0; 8]);
    assert_eq!(
        encode(&unit_layout(), &scales, &[1.0; 8]),
        Err(RowRejection::Normalization {
            norm: 8.0f64.sqrt()
        })
    );
    assert_eq!(
        encode(&loose_layout(), &scales, &[1.0; 7]),
        Err(RowRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
    assert_eq!(
        encode(&loose_layout(), &scales, &[f32::NAN; 8]),
        Err(RowRejection::NonFinite { coordinate: 0 })
    );
}

#[test]
#[should_panic(expected = "scales of one calibration match the layout")]
fn encoding_with_scales_of_another_dimension_is_a_wiring_error() {
    let narrow = RowLayout {
        dimension: 4,
        ..loose_layout()
    };
    let _ = encode(&narrow, &scales_of([1.0; 8]), &[1.0; 4]);
}

#[test]
fn weighted_scoring_forms_i32_products_in_range_and_weights_each_by_its_squared_scale() {
    let scales = scales_of([1.0, 0.5, 0.25, 2.0, 1.0, 1.0, 1.0, 1.0]);
    let query = [127i8, -127, 127, 1, 0, 3, -5, 7];
    let doc = [127i8, 127, -127, 1, 127, 3, 5, -7];
    let products: Vec<i32> = query
        .iter()
        .zip(doc)
        .map(|(q, d)| i32::from(*q) * i32::from(d))
        .collect();
    assert_eq!(products[0], 16129);
    assert_eq!(products[1], -16129);
    assert!(products.iter().all(|p| (-16129..=16129).contains(p)));
    let mut expected = 0.0f64;
    for (j, product) in products.iter().enumerate() {
        let s = f64::from(scales.as_slice()[j]);
        expected += (s * s) * f64::from(*product);
    }
    assert_eq!(weighted_dot(&scales, &query, &doc), expected);
    assert_eq!(
        weighted_dot(&scales, &query, &doc),
        16129.0 + 0.25 * -16129.0 + 0.0625 * -16129.0 + 4.0 + 0.0 + 9.0 - 25.0 - 49.0
    );
}

#[test]
fn weighted_scoring_accumulates_in_f64_in_increasing_coordinate_order() {
    // Terms 1e16, 1e-8, -1e16, 1e-8: forward yields 1e-8; reverse yields 0.
    let scales = scales_of([1.0e8, 1.0e-4, 1.0e8, 1.0e-4, 1.0, 1.0, 1.0, 1.0]);
    let query = [1i8, 1, -1, 1, 0, 0, 0, 0];
    let doc = [1i8; 8];
    let forward = weighted_dot(&scales, &query, &doc);
    let reversed =
        scales
            .as_slice()
            .iter()
            .zip(query)
            .zip(doc)
            .rev()
            .fold(0.0f64, |sum, ((s, q), d)| {
                sum + (f64::from(*s) * f64::from(*s)) * f64::from(i32::from(q) * i32::from(d))
            });
    assert_ne!(forward, reversed);
    assert_eq!(forward, f64::from(1.0e-4f32) * f64::from(1.0e-4f32));
    assert_eq!(reversed, 0.0);
}

#[test]
fn weighted_scoring_never_fuses_the_multiply_into_the_add() {
    // Term 0 is exactly 1.0; term 1 is (1/127)² · (−16129), which is −1 up to the rounding of 1/127 in f32.
    // A fused multiply-add would carry the exact product into the sum and land on a different f64.
    let scales = scales_of([1.0, 1.0 / 127.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    let query = [1i8, -127, 0, 0, 0, 0, 0, 0];
    let doc = [1i8, 127, 0, 0, 0, 0, 0, 0];
    let unfused = weighted_dot(&scales, &query, &doc);
    let s = f64::from(1.0f32 / 127.0);
    let fused = (s * s).mul_add(-16129.0, 1.0);
    assert_eq!(unfused, 7.450580596923828e-9);
    assert_ne!(unfused, fused);
}

#[test]
#[should_panic(expected = "codes of one calibration have one length")]
fn weighted_scoring_refuses_unequal_lengths_instead_of_truncating() {
    let _ = weighted_dot(&scales_of([1.0; 8]), &[1; 8], &[1; 7]);
}

#[test]
#[should_panic(expected = "the reserved code -128 never reaches scoring")]
fn weighted_scoring_refuses_a_reserved_query_code() {
    // A -128 · 127 product is -16256, outside the recipe's [-16129, 16129].
    let mut query = [0i8; 8];
    query[3] = i8::MIN;
    let _ = weighted_dot(&scales_of([1.0; 8]), &query, &[127; 8]);
}

#[test]
#[should_panic(expected = "the reserved code -128 never reaches scoring")]
fn weighted_scoring_refuses_a_reserved_document_code() {
    let mut doc = [0i8; 8];
    doc[7] = i8::MIN;
    let _ = weighted_dot(&scales_of([1.0; 8]), &[127; 8], &doc);
}

#[test]
fn nonuniform_scales_rank_differently_from_a_raw_integer_dot() {
    // Coordinate 0 carries small values (small scale); coordinate 1 carries large ones.
    let scales = scales_of([0.01, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    let query = [100i8, 10, 0, 0, 0, 0, 0, 0];
    let a = [100i8, 0, 0, 0, 0, 0, 0, 0];
    let b = [0i8, 20, 0, 0, 0, 0, 0, 0];
    let raw = |x: &[i8]| -> i64 {
        query
            .iter()
            .zip(x)
            .map(|(q, d)| i64::from(*q) * i64::from(*d))
            .sum()
    };
    assert!(raw(&a) > raw(&b), "the raw dot prefers `a`");
    assert!(
        weighted_dot(&scales, &query, &a) < weighted_dot(&scales, &query, &b),
        "the weighted dot prefers `b`"
    );
}

#[test]
fn a_query_encoded_under_another_generations_scales_carries_other_codes_and_scores() {
    let first = calibrate(&unit_layout(), corpus().iter().map(Vec::as_slice)).unwrap();
    let mut wider = corpus();
    wider.push(unit([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
    let second = calibrate(&unit_layout(), wider.iter().map(Vec::as_slice)).unwrap();
    assert_ne!(first.identity.scales_digest, second.identity.scales_digest);
    assert_ne!(
        first.identity.calibrated_rows,
        second.identity.calibrated_rows
    );

    let query = unit([0.1, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5]);
    let under_first = encode(&unit_layout(), &first.scales, &query).unwrap();
    let under_second = encode(&unit_layout(), &second.scales, &query).unwrap();
    assert_ne!(under_first.codes, under_second.codes);
    assert!(
        under_first.clipped > 0,
        "the first generation never saw coordinate 7 this large"
    );
    assert_eq!(under_second.clipped, 0);

    let doc = &wider[0];
    let doc_first = encode(&unit_layout(), &first.scales, doc).unwrap();
    let doc_second = encode(&unit_layout(), &second.scales, doc).unwrap();
    assert_ne!(
        weighted_dot(&first.scales, &under_first.codes, &doc_first.codes),
        weighted_dot(&second.scales, &under_second.codes, &doc_second.codes)
    );
}

#[test]
fn weighted_int8_scores_stay_within_the_quantization_bound_and_preserve_separated_orders() {
    let layout = unit_layout();
    let rows = corpus();
    let calibration = calibrate(&layout, rows.iter().map(Vec::as_slice)).unwrap();
    let ids: Vec<String> = (0..rows.len()).map(|i| format!("row-{i}")).collect();
    let mut separated_pairs = 0usize;
    let mut clipped_queries = 0usize;
    for query in [
        unit([0.8, 0.2, 0.1, 0.0, 0.1, 0.0, 0.0, 0.0]),
        unit([0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.2, 0.0]),
        unit([0.1, 0.1, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1]),
    ] {
        let exact = rescore(
            &layout,
            &query,
            rows.iter()
                .zip(&ids)
                .map(|(row, id)| (id.as_str(), OccurrenceClass::Messages, row.as_slice())),
        )
        .unwrap();
        let q = encode(&layout, &calibration.scales, &query).unwrap();
        clipped_queries += usize::from(q.clipped > 0);
        // |Σ q·d − Σ s²·cq·cd| ≤ Σ_j |q_j|·|d_j − s_j·cd_j| + |s_j·cd_j|·|q_j − s_j·cq_j|; an unclipped
        // document residual is at most s_j/2, and the query residual is taken exactly so a clipped query still bounds.
        let scored: Vec<(String, f64, f64, f64)> = rows
            .iter()
            .zip(&ids)
            .map(|(row, id)| {
                let d = encode(&layout, &calibration.scales, row).unwrap();
                assert_eq!(d.clipped, 0);
                let approx = weighted_dot(&calibration.scales, &q.codes, &d.codes);
                let bound: f64 = (0..8)
                    .map(|j| {
                        let s = f64::from(calibration.scales.as_slice()[j]);
                        let query_residual =
                            (f64::from(query[j]) - s * f64::from(q.codes[j])).abs();
                        f64::from(query[j]).abs() * s / 2.0
                            + (s * f64::from(d.codes[j])).abs() * query_residual
                    })
                    .sum();
                (id.clone(), inner_product(&query, row), approx, bound)
            })
            .collect();
        for (id, exact_score, approx, bound) in &scored {
            assert!(
                (approx - exact_score).abs() <= *bound,
                "{id}: |{approx} - {exact_score}| > {bound}"
            );
            let oracle = exact.iter().find(|r| r.occurrence_id == *id).unwrap();
            assert_eq!(
                oracle.score, *exact_score,
                "rescore is the same f64 arithmetic"
            );
        }
        // Rows whose exact scores differ by more than both error bounds keep their relative order.
        for (i, a) in scored.iter().enumerate() {
            for b in &scored[i + 1..] {
                if (a.1 - b.1).abs() > a.3 + b.3 {
                    separated_pairs += 1;
                    assert_eq!(
                        rank_order((a.2, &a.0), (b.2, &b.0)),
                        rank_order((a.1, &a.0), (b.1, &b.0)),
                        "{} vs {}: exact {} / {}, approximate {} / {}",
                        a.0,
                        b.0,
                        a.1,
                        b.1,
                        a.2,
                        b.2
                    );
                }
            }
        }
    }
    assert!(
        separated_pairs >= 20,
        "{separated_pairs} separated pairs were checked"
    );
    assert!(
        clipped_queries >= 1,
        "one query exercises the clipped-query residual"
    );
}

#[test]
fn scales_and_codes_round_trip_deterministically_and_refuse_malformed_bytes() {
    let rows = corpus();
    let first = calibrate(&unit_layout(), rows.iter().map(Vec::as_slice)).unwrap();
    let second = calibrate(&unit_layout(), rows.iter().map(Vec::as_slice)).unwrap();
    assert_eq!(first.scales.encode(), second.scales.encode());
    assert_eq!(first.identity, second.identity);
    let bytes = first.scales.encode();
    assert_eq!(bytes.len(), 32);
    assert_eq!(Scales::decode(&bytes, DIMENSION).unwrap(), first.scales);

    assert_eq!(
        Scales::decode(&bytes[..31], DIMENSION),
        Err(ScalarBytesRejection::TruncatedWord { bytes: 31 })
    );
    assert_eq!(
        Scales::decode(&bytes[..28], DIMENSION),
        Err(ScalarBytesRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
    // Shape errors take precedence over invalid coordinates for both word decoders.
    for actual in [0, 7, 9] {
        let malformed = codec::encode(&vec![f32::NAN; actual]);
        assert_eq!(
            codec::decode_shape(&malformed, DIMENSION),
            Err(RowRejection::Dimension {
                expected: DIMENSION,
                actual
            })
        );
        assert_eq!(
            Scales::decode(&malformed, DIMENSION),
            Err(ScalarBytesRejection::Dimension {
                expected: DIMENSION,
                actual
            })
        );
        let mut truncated = malformed;
        truncated.push(0);
        assert_eq!(
            codec::decode_shape(&truncated, DIMENSION),
            Err(RowRejection::TruncatedWord {
                bytes: truncated.len()
            })
        );
        assert_eq!(
            Scales::decode(&truncated, DIMENSION),
            Err(ScalarBytesRejection::TruncatedWord {
                bytes: truncated.len()
            })
        );
    }
    for bad in [0.0f32, -0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut values = [1.0f32; 8];
        values[5] = bad;
        assert_eq!(
            Scales::decode(&codec::encode(&values), DIMENSION),
            Err(ScalarBytesRejection::NotPositiveFinite { coordinate: 5 })
        );
    }
    let mut subnormal = [1.0f32; 8];
    subnormal[2] = f32::from_bits(1);
    assert!(Scales::decode(&codec::encode(&subnormal), DIMENSION).is_ok());

    let codes = vec![127i8, -127, 0, 1, -1, 64, -64, 3];
    let encoded = encode_codes(&codes);
    assert_eq!(encoded, vec![0x7f, 0x81, 0, 1, 0xff, 64, 0xc0, 3]);
    assert_eq!(decode_codes(&encoded, DIMENSION).unwrap(), codes);
    assert_eq!(
        decode_codes(&encoded[..7], DIMENSION),
        Err(ScalarBytesRejection::Dimension {
            expected: 8,
            actual: 7
        })
    );
    let mut reserved = encoded.clone();
    reserved[2] = 0x80;
    assert_eq!(
        decode_codes(&reserved, DIMENSION),
        Err(ScalarBytesRejection::ReservedCode { coordinate: 2 })
    );

    assert_eq!(
        ScalarRecipe::from_id(ScalarRecipe::SymmetricInt8V1.id()),
        Some(ScalarRecipe::SymmetricInt8V1)
    );
    assert_eq!(ScalarRecipe::from_id("scalar-int8-symmetric.v2"), None);
    assert_eq!(ScalarRecipe::from_id(""), None);
}

/// Pinned bytes for the fixed corpus. A change here is a change to the recipe: it needs a new `ScalarRecipe`
/// variant and the matching edit to `docs/dense-vector-contract.md`, never a reinterpretation of stored codes.
#[test]
fn the_fixed_corpus_produces_pinned_scale_and_code_bytes() {
    let rows = corpus();
    let calibration = calibrate(&unit_layout(), rows.iter().map(Vec::as_slice)).unwrap();
    let digest: String = calibration
        .identity
        .scales_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        digest,
        "70138153acd0ca6ac5466dc434e8d56c80c44f75e515298c4d795f9a5f418533"
    );
    assert_eq!(
        calibration.scales.encode(),
        [
            95, 227, 254, 59, 53, 244, 167, 59, 119, 18, 3, 59, 116, 139, 73, 59, 114, 150, 241,
            59, 5, 56, 0, 60, 35, 223, 251, 59, 56, 145, 98, 58
        ]
    );
    let codes: Vec<Vec<u8>> = rows
        .iter()
        .map(|row| {
            encode_codes(
                &encode(&unit_layout(), &calibration.scales, row)
                    .unwrap()
                    .codes,
            )
        })
        .collect();
    assert_eq!(
        codes,
        [
            [127, 21, 0, 0, 0, 0, 0, 127],
            [114, 74, 127, 0, 0, 0, 0, 0],
            [84, 127, 0, 127, 0, 0, 0, 0],
            [45, 0, 0, 0, 127, 0, 0, 0],
            [228, 0, 0, 0, 0, 0, 127, 0],
            [14, 0, 0, 0, 0, 127, 0, 0],
        ]
    );
}
