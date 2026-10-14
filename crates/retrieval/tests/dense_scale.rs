//! The dense quality driver compares production retrieval with a brute-force f64 oracle over seeded unit vectors: the candidate selection keeps one global pool of 256 by scale-weighted int8 score, the rescore returns the top 64 from the original rows, and each query's ordered result is compared with the oracle written here.
//!
//! Ignored by default. `EIDNARA_DENSE_SCALE_ROWS` (default 1,000,000), `EIDNARA_DENSE_SCALE_DIMENSION` (768), and `EIDNARA_DENSE_SCALE_QUERIES` (1,000) size the run, and `EIDNARA_DENSE_SCALE_POOL` (256) sets the candidate pool, so a pool of 64 runs the negative control; `EIDNARA_DENSE_SCALE_SAMPLES` names a file, kept outside the repository, for the raw per-query microseconds.
//!
//! The corpus is a seeded synthetic workload, not the frozen model corpus the production gate names. The timed span is in-process candidate selection plus rescore over rows held in memory, under test scan bounds; it is a local figure, not the reference-host dense-lane latency. One copy of the f32 rows stays resident, in the layer the scan reads, so a 1M by 768 run holds about 3 GB of rows plus 0.8 GB of codes.

mod support;

use std::io::Write;
use std::num::NonZeroUsize;
use std::time::Instant;

use kernel::applicability::EvalBudget;
use retrieval::dense::candidates::select_candidates;
use retrieval::dense::codec::{Metric, RowLayout};
use retrieval::dense::scalar::{calibrate, encode};
use retrieval::dense::{
    CandidateCapacity, CandidatePolicy, CandidateQuery, Completion, LayerCodes, ScanBounds,
    StorageBounds, rescore_pool,
};

use support::dense::{Fixture, Row, TOLERANCE, generation_of};
use support::layers::OwnedLayer;

const K: usize = 64;

fn setting(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{name}={value} is not a count")),
        Err(_) => default,
    }
}

/// `Mix` uses SplitMix64 to reproduce the corpus and queries from their seeds.
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn centered(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }
}

fn normalized(raw: &[f64]) -> Vec<f32> {
    let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
    raw.iter().map(|value| (value / norm) as f32).collect()
}

/// Rows around 1,024 seeded centers, so each query has near neighbors as embeddings of related text do.
fn corpus(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut mix = Mix(0x5eed_0001);
    let centers: Vec<Vec<f64>> = (0..1024)
        .map(|_| (0..dimension).map(|_| mix.centered()).collect())
        .collect();
    (0..rows)
        .map(|index| {
            let center = &centers[index % centers.len()];
            let raw: Vec<f64> = center
                .iter()
                .map(|value| value + 0.35 * mix.centered())
                .collect();
            normalized(&raw)
        })
        .collect()
}

/// A perturbed copy of a seeded corpus row for each query.
fn queries(corpus: &[Vec<f32>], count: usize) -> Vec<Vec<f32>> {
    let mut mix = Mix(0x5eed_0002);
    (0..count)
        .map(|_| {
            let row = &corpus[(mix.next() % corpus.len() as u64) as usize];
            let raw: Vec<f64> = row
                .iter()
                .map(|value| f64::from(*value) + 0.05 * mix.centered())
                .collect();
            normalized(&raw)
        })
        .collect()
}

fn score(query: &[f32], row: &[f32]) -> f64 {
    let mut total = 0.0f64;
    for (q, r) in query.iter().zip(row) {
        total += f64::from(*q) * f64::from(*r);
    }
    total
}

/// The oracle scores every row against `query`, sorts by score descending then identifier bytes ascending, and keeps `K`.
fn oracle(query: &[f32], ids: &[String], rows: &[Vec<f32>]) -> Vec<(String, u64)> {
    let mut scored: Vec<(f64, usize)> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (score(query, row), index))
        .collect();
    // A partial selection keeps the oracle linear in the corpus per query, which a full sort over a million rows would not.
    let order = |a: &(f64, usize), b: &(f64, usize)| {
        b.0.total_cmp(&a.0)
            .then_with(|| ids[a.1].as_bytes().cmp(ids[b.1].as_bytes()))
    };
    scored.select_nth_unstable_by(K - 1, order);
    scored.truncate(K);
    scored.sort_by(order);
    scored
        .into_iter()
        .map(|(score, index)| (ids[index].clone(), score.to_bits()))
        .collect()
}

#[test]
#[ignore = "builds a large dense corpus; run in release with --ignored"]
fn compressed_top64_matches_the_f64_oracle_for_at_least_99_percent_of_queries() {
    let rows = setting("EIDNARA_DENSE_SCALE_ROWS", 1_000_000);
    let dimension = setting("EIDNARA_DENSE_SCALE_DIMENSION", 768);
    let query_count = setting("EIDNARA_DENSE_SCALE_QUERIES", 1_000);
    let pool_size = setting("EIDNARA_DENSE_SCALE_POOL", 256);
    assert!(query_count > 0, "the run needs queries");
    assert!(
        (K..=kernel::MAX_ELIGIBILITY_CANDIDATES).contains(&pool_size) && rows >= pool_size,
        "the pool holds at least {K} and at most one kernel batch, and the corpus fills it"
    );
    assert!(dimension > 0, "rows need at least one coordinate");
    let started = Instant::now();
    let vectors = corpus(rows, dimension);
    let queries = queries(&vectors, query_count);
    let objects: Vec<String> = (0..rows).map(|index| format!("dense-{index:07}")).collect();
    let object_refs: Vec<&str> = objects.iter().map(String::as_str).collect();
    let projected: Vec<Row> = objects
        .iter()
        .map(|object| Row {
            vector: None,
            ..Row::claim(object, Vec::new())
        })
        .collect();
    let width = u32::try_from(dimension).unwrap();
    let fixture = Fixture::with_dimension(&object_refs, &object_refs, projected, width);
    let ids: Vec<String> = fixture.rows.iter().map(Row::occurrence_id).collect();
    let generation = generation_of(width);
    let layer = OwnedLayer::new(
        generation.generation_epoch,
        0,
        ids.into_iter().zip(vectors).collect(),
        vec![],
    );
    let layout = RowLayout {
        dimension: width,
        metric: Metric::InnerProduct,
        unit_norm_tolerance: TOLERANCE,
    };
    let scales = calibrate(&layout, layer.rows.iter().map(Vec::as_slice))
        .unwrap()
        .scales;
    let codes: Vec<Vec<i8>> = layer
        .rows
        .iter()
        .map(|row| encode(&layout, &scales, row).unwrap().codes)
        .collect();
    let built = started.elapsed();
    let layers = [layer.layer()];
    let layer_codes = [LayerCodes {
        scales: &scales,
        codes: &codes,
    }];
    let capacity = CandidateCapacity::new(
        K,
        CandidatePolicy {
            alpha: pool_size as f64 / K as f64,
            cap: NonZeroUsize::new(pool_size).unwrap(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(capacity.candidates().get(), pool_size);
    let bounds = ScanBounds {
        page_rows: NonZeroUsize::new(256).unwrap(),
        max_rows: NonZeroUsize::new(rows + 1).unwrap(),
        storage: StorageBounds {
            batch_bytes: NonZeroUsize::new(1 << 24).unwrap(),
            heap_bytes: NonZeroUsize::new(1 << 24).unwrap(),
        },
    };
    let (mut exact, mut covered) = (0usize, 0usize);
    let mut samples = Vec::with_capacity(query_count);
    for query in &queries {
        let request = CandidateQuery {
            generation: &generation,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            query,
            authority: fixture.authority(),
            capacity,
            bounds,
            layers: &layers,
            codes: &layer_codes,
            max_entries: NonZeroUsize::new(rows).unwrap(),
        };
        let start = Instant::now();
        let (pool, rescored) = fixture
            .store
            .with_conn(|conn| {
                let pool =
                    select_candidates(conn, &fixture.kernel, &request, &EvalBudget::unbounded())
                        .unwrap();
                assert_eq!(pool.ranking.completion, Completion::Complete);
                let rescored = rescore_pool(&pool, &layout, query, capacity.k(), |winner, row| {
                    row.clone_from(&layer.rows[winner.row]);
                    Ok::<_, ()>(())
                })
                .unwrap();
                Ok((pool, rescored))
            })
            .unwrap();
        samples.push(start.elapsed().as_micros());
        let returned: Vec<(String, u64)> = rescored
            .ranked
            .iter()
            .map(|row| (row.occurrence_id.clone(), row.score.to_bits()))
            .collect();
        let expected = oracle(query, &layer.ids, &layer.rows);
        if returned == expected {
            exact += 1;
        }
        let pool_ids: std::collections::BTreeSet<&str> = pool
            .ranking
            .ranked
            .iter()
            .map(|row| row.occurrence_id.as_str())
            .collect();
        covered += expected
            .iter()
            .filter(|(id, _)| pool_ids.contains(id.as_str()))
            .count();
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let percentile = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    println!(
        "dense scale (synthetic corpus, in-process, not the reference host): rows {rows}, dimension {dimension}, queries {query_count}, pool {pool_size}, built in {:.1}s; exact top-{K} {exact}/{query_count}; candidate coverage {:.4}; select+rescore p50 {} us, p99 {} us, max {} us",
        built.as_secs_f64(),
        covered as f64 / (query_count * K) as f64,
        percentile(0.5),
        percentile(0.99),
        sorted[sorted.len() - 1],
    );
    if let Ok(path) = std::env::var("EIDNARA_DENSE_SCALE_SAMPLES") {
        let mut file = std::fs::File::create(path).unwrap();
        for sample in &samples {
            writeln!(file, "{sample}").unwrap();
        }
    }
    assert!(
        exact * 100 >= query_count * 99,
        "{exact} of {query_count} queries returned the oracle's exact ordered top {K}"
    );
}
