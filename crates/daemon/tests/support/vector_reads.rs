//! Shared shape for reader and compactor tests: a fixed five-object corpus, exports whose identifiers are the projection's, a view acquired under the reader's own shared protection, and rankings against the independent f64 reference.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use daemon::vector_admission::{Ledger, ResourceClass};
use daemon::vector_generation::{ExpectedVectors, VerifiedVectors};
use daemon::vector_reader::{
    AcquireEvent, AcquireRefusal, PinnedVectors, RankRefusal, RankRequest, ReaderBounds, acquire,
    rank,
};
use host_runtime::lifecycle::{TRANSACTION_LOCK_NAME, coordination_dir_path};
use kernel::applicability::EvalBudget;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::{LayeredRanking, OracleBounds};

use super::dense_projection::{Projection, occurrence_id};
use super::vector_store::{Fixture, KERNEL, generation, unit};

pub const OBJECTS: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];

pub fn corpus() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("alpha", unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1])),
        ("beta", unit([0.7, 0.3, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0])),
        ("gamma", unit([0.5, 0.5, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0])),
        ("delta", unit([0.3, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0])),
        ("epsilon", unit([0.1, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0])),
    ]
}

pub fn axis(index: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[index] = 1.0;
    unit(raw)
}

/// An export whose identifiers are the projection's, so the layer names what the walk visits.
pub fn export(rows: &[(&str, Vec<f32>)], tombstones: &[&str], checkpoint: i64) -> LiveRows {
    let mut rows: Vec<ExportedRow> = rows
        .iter()
        .map(|(object, vector)| ExportedRow {
            occurrence_id: occurrence_id(object),
            vector: vector.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.occurrence_id.cmp(&b.occurrence_id));
    let mut tombstones: Vec<String> = tombstones
        .iter()
        .map(|object| occurrence_id(object))
        .collect();
    tombstones.sort();
    LiveRows {
        generation: generation(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows,
        tombstones,
    }
}

pub fn bounds() -> ReaderBounds {
    ReaderBounds {
        max_deltas: NonZeroUsize::new(4).unwrap(),
        recovery_bound: NonZeroUsize::new(8).unwrap(),
    }
}

pub fn oracle_bounds(k: usize) -> OracleBounds {
    OracleBounds {
        k: NonZeroUsize::new(k).unwrap(),
        page_rows: NonZeroUsize::new(2).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
    }
}

/// One page of two rows plus one raw row, eight coordinates wide.
pub const PAGE_SCRATCH: u64 = (2 + 1) * 8 * 4;

pub fn projection(fixture: &Fixture, admitted: &[&str]) -> Projection {
    Projection::new(
        fixture.root.path(),
        &fixture.identity,
        &fixture.generation,
        &OBJECTS,
        admitted,
    )
}

pub fn transaction_lock(fixture: &Fixture) -> PathBuf {
    coordination_dir_path(Some(fixture.root.path()))
        .unwrap()
        .join(TRANSACTION_LOCK_NAME)
}

/// Acquires under the reader's own shared protection, giving up the fixture's exclusive transaction for the duration.
pub fn acquire_view(
    fixture: &mut Fixture,
    observe: &mut dyn FnMut(AcquireEvent),
) -> Result<Arc<PinnedVectors>, AcquireRefusal> {
    fixture.release_transaction();
    let view = acquire(
        Some(fixture.root.path()),
        &fixture.expected(),
        bounds(),
        &fixture.ledger,
        &fixture.admission,
        observe,
    );
    fixture.reacquire_transaction();
    view
}

/// What the ledger holds for `class`, or zero.
pub fn held(ledger: &Ledger, class: ResourceClass) -> u64 {
    ledger.census().held.get(&class).copied().unwrap_or(0)
}

/// The corpus with `alpha` replaced and `epsilon` gone, published as a new base over the old composition.
pub fn publish_replacement(
    fixture: &Fixture,
    corpus: &[(&str, Vec<f32>)],
    sequence: u64,
) -> (Vec<(&'static str, Vec<f32>)>, VerifiedVectors) {
    let mut replaced: Vec<(&'static str, Vec<f32>)> = corpus
        .iter()
        .map(|(object, vector)| {
            let object: &'static str = OBJECTS.iter().copied().find(|o| o == object).unwrap();
            (object, vector.clone())
        })
        .collect();
    replaced[0].1 = axis(7);
    replaced.retain(|(object, _)| *object != "epsilon");
    let new_base = fixture.layer_from(&export(&replaced, &[], 10 * sequence as i64));
    fixture
        .publish(&fixture.compose(sequence, &new_base, &[]).unwrap())
        .unwrap();
    (replaced, new_base)
}

pub fn rank_view(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    query: &[f32],
    k: usize,
) -> Result<LayeredRanking, RankRefusal> {
    let expected = fixture.expected();
    rank_expecting(fixture, projection, view, &expected, query, k)
}

pub fn rank_expecting(
    fixture: &Fixture,
    projection: &Projection,
    view: &PinnedVectors,
    expected: &ExpectedVectors<'_>,
    query: &[f32],
    k: usize,
) -> Result<LayeredRanking, RankRefusal> {
    let request = RankRequest {
        expected,
        query,
        authority: projection.authority(),
        bounds: oracle_bounds(k),
        max_entries: NonZeroUsize::new(64).unwrap(),
    };
    projection
        .store
        .with_conn(|conn| {
            Ok(rank(
                view,
                conn,
                &projection.kernel,
                &request,
                &EvalBudget::unbounded(),
                &fixture.admission,
            ))
        })
        .unwrap()
}

pub fn keyed(ranking: &LayeredRanking) -> Vec<(String, f64)> {
    ranking
        .ranking
        .ranked
        .iter()
        .map(|row| (row.occurrence_id.clone(), row.score))
        .collect()
}

pub fn map(rows: &[(&str, Vec<f32>)]) -> Vec<(String, Vec<f32>)> {
    rows.iter()
        .map(|(object, vector)| (occurrence_id(object), vector.clone()))
        .collect()
}

/// The bytes the view should keep resident, from the store's manifests rather than the reader.
pub fn resident_bytes(fixture: &Fixture, members: &[String]) -> u64 {
    members
        .iter()
        .map(|digest| {
            daemon::vector_generation::resident_bytes(&fixture.store.manifest(digest).unwrap())
        })
        .sum()
}

/// Every manifest byte of `digests`, from the store rather than the reader.
pub fn manifest_bytes(fixture: &Fixture, digests: &[String]) -> u64 {
    digests
        .iter()
        .flat_map(|digest| fixture.store.manifest(digest).unwrap().files)
        .map(|file| file.size)
        .sum()
}
