//! Times the layer resolver over a base and a bounded set of deltas so its cost is one number per shape: the resolver touches identifiers and tombstones only, never row bytes, so the row payload is one coordinate here.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::{Layer, Precedence, resolve};

/// `(base rows, deltas, rows per delta, tombstones per delta)`.
const SHAPES: [(usize, usize, usize, usize); 3] = [
    (10_000, 0, 0, 0),
    (10_000, 4, 1_000, 100),
    (100_000, 8, 5_000, 500),
];

struct Owned {
    precedence: Precedence,
    checkpoint: ProjectionCheckpoint,
    ids: Vec<String>,
    rows: Vec<Vec<f32>>,
    tombstones: Vec<String>,
}

fn id(n: u64) -> String {
    format!("{n:064x}")
}

/// Every delta touches a fixed stride of the base's identifiers so winners and masked rows both occur, and the same stream every run.
fn build(shape: (usize, usize, usize, usize)) -> Vec<Owned> {
    let (base_rows, deltas, delta_rows, delta_tombstones) = shape;
    let mut owned = vec![Owned {
        precedence: Precedence {
            base_epoch: 1,
            delta_ordinal: 0,
        },
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 10,
            checkpoint_commit_seq: 10,
            hold_id: "hold".to_owned(),
        },
        ids: (0..base_rows as u64).map(id).collect(),
        rows: vec![vec![0.0]; base_rows],
        tombstones: Vec::new(),
    }];
    for ordinal in 1..=deltas {
        let stride = (ordinal as u64) * 7 + 3;
        let ids: Vec<String> = (0..delta_rows as u64)
            .map(|i| id((i * stride) % (base_rows as u64 + delta_rows as u64)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let tombstones: Vec<String> = (0..delta_tombstones as u64)
            .map(|i| id((i * stride + 1) % base_rows as u64))
            .filter(|t| ids.binary_search(t).is_err())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let seq = 10 + ordinal as i64 * 10;
        owned.push(Owned {
            precedence: Precedence {
                base_epoch: 1,
                delta_ordinal: ordinal as u32,
            },
            checkpoint: ProjectionCheckpoint {
                snapshot_commit_seq: seq,
                checkpoint_commit_seq: seq,
                hold_id: "hold".to_owned(),
            },
            rows: vec![vec![0.0]; ids.len()],
            ids,
            tombstones,
        });
    }
    owned
}

fn bench_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("dense_resolve");
    group.measurement_time(Duration::from_secs(5));
    for shape in SHAPES {
        let owned = build(shape);
        let layers: Vec<Layer<'_>> = owned
            .iter()
            .map(|o| Layer {
                precedence: o.precedence,
                checkpoint: &o.checkpoint,
                occurrence_ids: &o.ids,
                rows: &o.rows,
                tombstones: &o.tombstones,
            })
            .collect();
        let entries: usize = layers
            .iter()
            .map(|l| l.occurrence_ids.len() + l.tombstones.len())
            .sum();
        let bound = NonZeroUsize::new(entries).unwrap();
        group.throughput(criterion::Throughput::Elements(entries as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!(
                "base{}_deltas{}x{}+{}",
                shape.0, shape.1, shape.2, shape.3
            )),
            &layers,
            |b, layers| {
                b.iter(|| {
                    let resolved = resolve(black_box(layers), bound).unwrap();
                    black_box(resolved.winners.len())
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_resolve);
criterion_main!(benches);
