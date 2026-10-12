//! Each probe here is `#[ignore]`d and prints figures that characterize this host; its assertions check only completion and result counts.
//! Run in release: `cargo test --release --locked -p retrieval --test rp25_host_measurements -- --ignored --nocapture --test-threads=1`.

mod support;

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use retrieval::dense::codec::{Metric, RowLayout};
use retrieval::dense::scalar::{calibrate, encode_into, weighted_dot};
use retrieval::dense::{Completion, OracleBounds, inner_product, resolve};
use support::dense::{Fixture, Row};
use support::layers::{OwnedLayer, layers};

/// At least one run, so every median has a sample.
fn runs() -> usize {
    std::env::var("EIDNARA_RP25_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(11)
        .max(1)
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// SplitMix64; equal seeds produce equal `next()` sequences.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }

    fn unit(&mut self, dimension: usize) -> Vec<f32> {
        let raw: Vec<f64> = (0..dimension).map(|_| self.next()).collect();
        let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
        raw.iter().map(|value| (value / norm) as f32).collect()
    }
}

fn layout(dimension: u32) -> RowLayout {
    RowLayout {
        dimension,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: support::dense::TOLERANCE,
    }
}

fn top10(scores: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b)));
    order.truncate(10);
    order
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn exhaustive_f32_oracle_wall_time_and_rows_visited() {
    const DIMENSION: u32 = 384;
    for rows in [10_000usize, 100_000] {
        let mut stream = Stream(u64::try_from(rows).unwrap());
        let objects: Vec<String> = (0..rows).map(|i| format!("m-{i:06}")).collect();
        let names: Vec<&str> = objects.iter().map(String::as_str).collect();
        let corpus: Vec<Row> = names
            .iter()
            .map(|object| Row::claim(object, stream.unit(DIMENSION as usize)))
            .collect();
        let built = Instant::now();
        let fixture = Fixture::with_dimension(&names, &names, corpus, DIMENSION);
        let build = built.elapsed();
        let bounds = OracleBounds {
            k: NonZeroUsize::new(10).unwrap(),
            page_rows: NonZeroUsize::new(256).unwrap(),
            max_rows: NonZeroUsize::new(rows).unwrap(),
        };
        let query = stream.unit(DIMENSION as usize);
        let mut samples = Vec::new();
        let mut last = None;
        for _ in 0..runs() {
            let started = Instant::now();
            let ranking = fixture
                .rank(&query, bounds, &EvalBudget::unbounded())
                .unwrap();
            samples.push(started.elapsed());
            last = Some(ranking);
        }
        let ranking = last.unwrap();
        assert_eq!(ranking.completion, Completion::Complete);
        assert_eq!(ranking.ranked.len(), 10);
        let (min, max) = (
            *samples.iter().min().unwrap(),
            *samples.iter().max().unwrap(),
        );
        println!(
            "oracle rows={rows} dim={DIMENSION} k=10 page_rows=256 runs={} median={:?} min={min:?} max={max:?} rows_visited={} with_vector={} pages={} judged={} batches={} fixture_build={build:?}",
            samples.len(),
            median(samples),
            ranking.coverage.required,
            ranking.coverage.with_vector,
            ranking.consumed.pages,
            ranking.consumed.judged,
            ranking.consumed.batches,
        );
    }
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn int8_kernel_cost_and_fidelity_against_the_f32_oracle() {
    const ROWS: usize = 4096;
    const PASSES: usize = 64;
    for dimension in [128usize, 384, 1024] {
        let mut stream = Stream(dimension as u64);
        let rows: Vec<Vec<f32>> = (0..ROWS).map(|_| stream.unit(dimension)).collect();
        let layout = layout(dimension as u32);
        let calibration = calibrate(&layout, rows.iter().map(Vec::as_slice)).unwrap();
        let scales = &calibration.scales;
        let mut codes = vec![Vec::new(); ROWS];
        let mut encode = Vec::new();
        for _ in 0..runs() {
            let started = Instant::now();
            for (row, code) in rows.iter().zip(&mut codes) {
                encode_into(&layout, scales, black_box(row), code).unwrap();
            }
            encode.push(started.elapsed() / ROWS as u32);
        }
        let query = &rows[0];
        let query_codes = &codes[0];
        let (mut dot, mut f32_dot) = (Vec::new(), Vec::new());
        for _ in 0..runs() {
            let started = Instant::now();
            for _ in 0..PASSES {
                for code in &codes {
                    black_box(weighted_dot(scales, black_box(query_codes), code));
                }
            }
            dot.push(started.elapsed() / (ROWS * PASSES) as u32);
            let started = Instant::now();
            for _ in 0..PASSES {
                for row in &rows {
                    black_box(inner_product(black_box(query), row));
                }
            }
            f32_dot.push(started.elapsed() / (ROWS * PASSES) as u32);
        }
        println!(
            "kernels dim={dimension} rows={ROWS} runs={} weighted_dot_ns={} inner_product_f32_ns={} encode_ns_per_row={} (medians)",
            runs(),
            median(dot).as_nanos(),
            median(f32_dot).as_nanos(),
            median(encode).as_nanos(),
        );
    }

    const CORPUS: usize = 10_000;
    const QUERIES: usize = 32;
    const DIMENSION: usize = 384;
    let mut stream = Stream(0x5eed);
    let rows: Vec<Vec<f32>> = (0..CORPUS).map(|_| stream.unit(DIMENSION)).collect();
    let layout = layout(DIMENSION as u32);
    let calibration = calibrate(&layout, rows.iter().map(Vec::as_slice)).unwrap();
    let scales = &calibration.scales;
    let mut codes = vec![Vec::new(); CORPUS];
    for (row, code) in rows.iter().zip(&mut codes) {
        encode_into(&layout, scales, row, code).unwrap();
    }
    let (mut max_error, mut overlaps, mut query_clipped) = (0.0f64, Vec::new(), 0u32);
    for _ in 0..QUERIES {
        let query = stream.unit(DIMENSION);
        let mut query_codes = Vec::new();
        query_clipped += encode_into(&layout, scales, &query, &mut query_codes).unwrap();
        let exact: Vec<f64> = rows.iter().map(|row| inner_product(&query, row)).collect();
        let weighted: Vec<f64> = codes
            .iter()
            .map(|code| weighted_dot(scales, &query_codes, code))
            .collect();
        for (a, b) in exact.iter().zip(&weighted) {
            max_error = max_error.max((a - b).abs());
        }
        let oracle = top10(&exact);
        overlaps.push(
            top10(&weighted)
                .iter()
                .filter(|i| oracle.contains(i))
                .count(),
        );
    }
    let mean = overlaps.iter().sum::<usize>() as f64 / QUERIES as f64;
    println!(
        "fidelity corpus={CORPUS} dim={DIMENSION} queries={QUERIES} max_abs_weighted_minus_f32={max_error:.3e} top10_overlap_mean={mean:.2} top10_overlap_min={} query_coordinates_clipped={query_clipped}",
        overlaps.iter().min().unwrap(),
    );
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn resolver_wall_time_over_a_base_and_four_deltas() {
    const BASE: usize = 10_000;
    const DIMENSION: usize = 384;
    let id = |i: usize| format!("{i:064x}");
    let mut stream = Stream(0xba5e);
    let mut owned = vec![OwnedLayer::new(
        1,
        0,
        (0..BASE).map(|i| (id(i), stream.unit(DIMENSION))).collect(),
        Vec::new(),
    )];
    for d in 1..=4usize {
        let replaced = (d * 2000..d * 2000 + 500).map(id);
        let added = (BASE + d * 500..BASE + d * 500 + 500).map(id);
        owned.push(OwnedLayer::new(
            1,
            d as u32,
            replaced
                .chain(added)
                .map(|occurrence| (occurrence, stream.unit(DIMENSION)))
                .collect(),
            (d * 2000 + 500..d * 2000 + 600).map(id).collect(),
        ));
    }
    let layers = layers(&owned);
    let max = NonZeroUsize::new(100_000).unwrap();
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..runs() {
        let started = Instant::now();
        let resolved = resolve(black_box(&layers), max).unwrap();
        samples.push(started.elapsed());
        last = Some((resolved.winners.len(), resolved.superseded, resolved.masked));
    }
    let (winners, superseded, masked) = last.unwrap();
    assert_eq!((winners, superseded, masked), (11_600, 2_000, 400));
    let (min, max) = (
        *samples.iter().min().unwrap(),
        *samples.iter().max().unwrap(),
    );
    println!(
        "resolver base={BASE} deltas=4x(500 replace + 500 add + 100 tombstone) runs={} median={:?} min={min:?} max={max:?} winners={winners} superseded={superseded} masked={masked}",
        samples.len(),
        median(samples),
    );
}
