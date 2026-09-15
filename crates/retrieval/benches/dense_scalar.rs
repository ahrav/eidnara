//! Times the scalar recipe at fixed dimensions so a change to the arithmetic shows up as a change in one number, not in a ranking.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use retrieval::dense::codec::{Metric, RowLayout};
use retrieval::dense::inner_product;
use retrieval::dense::scalar::{Scales, calibrate, encode, weighted_dot};

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
        let scales: Scales = calibration.scales;
        let codes: Vec<Vec<i8>> = rows
            .iter()
            .map(|row| encode(&layout, &scales, row).unwrap().codes)
            .collect();
        group.bench_with_input(
            BenchmarkId::new("calibrate_256_rows", dimension),
            &rows,
            |b, rows| b.iter(|| calibrate(&layout, rows.iter().map(Vec::as_slice)).unwrap()),
        );
        group.bench_with_input(
            BenchmarkId::new("encode_row", dimension),
            &rows[0],
            |b, row| b.iter(|| encode(&layout, &scales, black_box(row)).unwrap()),
        );
        group.bench_with_input(
            BenchmarkId::new("weighted_dot_256_docs", dimension),
            &codes,
            |b, codes| {
                b.iter(|| {
                    let mut total = 0.0f64;
                    for doc in codes {
                        total += weighted_dot(&scales, black_box(&codes[0]), doc);
                    }
                    total
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("f32_inner_product_256_docs", dimension),
            &rows,
            |b, rows| {
                b.iter(|| {
                    let mut total = 0.0f64;
                    for row in rows {
                        total += inner_product(black_box(&rows[0]), row);
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
