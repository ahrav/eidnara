//! Local, warm-input API and scoring-kernel costs; not a retrieval-latency or memory-bandwidth benchmark.
//! Calibration includes validation and the scale digest; encoding/decoding include allocation and drop.
//! Scoring uses predecoded Vec-backed rows, with no eligibility checks, I/O, or ranking.
//! Throughput counts coordinates, not rows or bytes. Synthetic inputs and short runs are exploratory.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::inner_product;
use retrieval::dense::scalar::{
    Scales, calibrate, decode_codes, encode, encode_codes, weighted_dot,
};

const DIMENSIONS: [u32; 3] = [128, 384, 1024];
const ROWS: usize = 256;

/// A fixed linear congruential stream keeps every run on identical rows.
fn rows(dimension: u32) -> Vec<Vec<f32>> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..ROWS)
        .map(|_| {
            let raw: Vec<f64> = (0..dimension)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
                })
                .collect();
            let norm = raw.iter().map(|v| v * v).sum::<f64>().sqrt();
            raw.iter().map(|v| (v / norm) as f32).collect()
        })
        .collect()
}

fn layout(dimension: u32) -> RowLayout {
    RowLayout {
        dimension,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: 1e-3,
    }
}

fn scalar_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("dense_scalar");
    for dimension in DIMENSIONS {
        let layout = layout(dimension);
        let rows = rows(dimension);
        let calibration = calibrate(&layout, rows.iter().map(Vec::as_slice)).unwrap();
        let scales = calibration.scales;
        let codes: Vec<Vec<i8>> = rows
            .iter()
            .map(|row| encode(&layout, &scales, row).unwrap().codes)
            .collect();
        for count in [1, rows.len()] {
            group.throughput(Throughput::Elements(count as u64 * u64::from(dimension)));
            group.bench_with_input(
                BenchmarkId::new(format!("calibrate_{count}_rows"), dimension),
                &rows[..count],
                |b, rows| {
                    b.iter(|| {
                        calibrate(
                            black_box(&layout),
                            black_box(rows).iter().map(Vec::as_slice),
                        )
                        .unwrap()
                    })
                },
            );
        }
        group.throughput(Throughput::Elements(u64::from(dimension)));
        group.bench_with_input(
            BenchmarkId::new("validate_row", dimension),
            &rows[0],
            |b, row| {
                b.iter(|| {
                    black_box(&layout).check().unwrap();
                    codec::validate(black_box(row), black_box(&layout)).unwrap();
                })
            },
        );
        let mut clipped_query = vec![0.0; dimension as usize];
        clipped_query[0] = 1.0;
        assert_eq!(encode(&layout, &scales, &rows[0]).unwrap().clipped, 0);
        assert_eq!(encode(&layout, &scales, &clipped_query).unwrap().clipped, 1);
        for (name, row) in [
            ("encode_row", &rows[0]),
            ("encode_clipped_query", &clipped_query),
        ] {
            group.bench_with_input(BenchmarkId::new(name, dimension), row, |b, row| {
                b.iter(|| encode(black_box(&layout), black_box(&scales), black_box(row)).unwrap())
            });
        }
        let code_bytes = encode_codes(&codes[0]);
        group.bench_with_input(
            BenchmarkId::new("decode_codes_row", dimension),
            &code_bytes,
            |b, bytes| b.iter(|| decode_codes(black_box(bytes), black_box(dimension)).unwrap()),
        );
        let scale_bytes = scales.encode();
        group.bench_with_input(
            BenchmarkId::new("decode_scales", dimension),
            &scale_bytes,
            |b, bytes| b.iter(|| Scales::decode(black_box(bytes), black_box(dimension)).unwrap()),
        );
        group.throughput(Throughput::Elements(
            rows.len() as u64 * u64::from(dimension),
        ));
        group.bench_with_input(
            BenchmarkId::new(format!("weighted_dot_{}_docs", codes.len()), dimension),
            &codes,
            |b, codes| {
                b.iter(|| {
                    let mut total = 0.0f64;
                    let query = black_box(&codes[0]);
                    let scales = black_box(&scales);
                    for doc in black_box(codes) {
                        total += weighted_dot(scales, query, doc);
                    }
                    total
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new(format!("f32_inner_product_{}_docs", rows.len()), dimension),
            &rows,
            |b, rows| {
                b.iter(|| {
                    let mut total = 0.0f64;
                    let query = black_box(&rows[0]);
                    for row in black_box(rows) {
                        total += inner_product(query, row);
                    }
                    total
                })
            },
        );
    }
    group.finish();
}

fn configure() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
}

criterion_group! {
    name = benches;
    config = configure();
    targets = scalar_benches
}
criterion_main!(benches);
