//! The compactor's resident reservation is an upper bound on the heap it uses.

#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;
mod support;

use std::num::NonZeroUsize;

use alloc_recorder::record_window;
use daemon::projection_gates::Denial;
use daemon::vector_admission::{RESIDENT_LIMIT, Refusal};
use daemon::vector_compaction::{CompactionRefusal, compact};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use support::vector_reads::acquire_view;
use support::vector_store::Fixture;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const DIMENSION: u32 = 4096;
/// `ROWS` is not a power of two, so a doubling buffer overshoots its row count.
const ROWS: usize = 33;

fn axis(index: usize) -> Vec<f32> {
    let mut raw = vec![0.0f32; DIMENSION as usize];
    raw[index] = 1.0;
    raw
}

/// Publishes one base of `rows` under `hold_id` with a delta that replaces the first row, then returns what compaction reserves for the pair and the heap it peaks at.
fn reserved_and_peak(
    fixture: &mut Fixture,
    rows: Vec<ExportedRow>,
    hold_id: &str,
    max_entries: usize,
) -> (u64, u64) {
    let checkpoint = |commit_seq: i64| ProjectionCheckpoint {
        snapshot_commit_seq: commit_seq - 1,
        checkpoint_commit_seq: commit_seq,
        hold_id: hold_id.to_owned(),
    };
    let replaced = ExportedRow {
        occurrence_id: rows[0].occurrence_id.clone(),
        vector: rows[0].vector.iter().rev().copied().collect(),
    };
    let base = fixture.layer_from(&LiveRows {
        checkpoint: checkpoint(10),
        rows,
        tombstones: Vec::new(),
    });
    let delta = fixture.layer_from(&LiveRows {
        checkpoint: checkpoint(12),
        rows: vec![replaced],
        tombstones: Vec::new(),
    });
    fixture
        .publish(&fixture.compose(1, &base, &[delta]).unwrap())
        .unwrap();
    let view = acquire_view(fixture, &mut |_| {}).unwrap();
    let max_entries = NonZeroUsize::new(max_entries).unwrap();

    // The reservation is what a limit exactly at the view's own bytes refuses.
    let resident_before = fixture.ledger.census().resident;
    fixture.set_limit(RESIDENT_LIMIT, resident_before);
    let refusal = compact(
        &view,
        &fixture.expected(),
        &fixture.staging(),
        max_entries,
        &fixture.work_dir(),
    )
    .unwrap_err();
    let CompactionRefusal::Reservation(Refusal::Denied(Denial::LimitExceeded { observed, .. })) =
        refusal
    else {
        panic!("{refusal:?}");
    };
    let reserved = observed - resident_before;
    fixture.set_limit(RESIDENT_LIMIT, u64::MAX);

    let work_dir = fixture.work_dir();
    let expected = fixture.expected();
    let staging = fixture.staging();
    let (compacted, ledger) =
        record_window(|| compact(&view, &expected, &staging, max_entries, &work_dir));
    drop(compacted.unwrap());
    (reserved, ledger.peak_live_bytes as u64)
}

#[test]
fn the_resident_reservation_bounds_the_compactors_heap_peak() {
    let mut fixture = Fixture::with_dimension(DIMENSION);
    let rows = (0..ROWS)
        .map(|i| ExportedRow {
            occurrence_id: format!("{i:064x}"),
            vector: axis(i),
        })
        .collect();
    let (reserved, peak) = reserved_and_peak(&mut fixture, rows, "hold-7", 64);
    let row_bytes = ROWS as u64 * u64::from(DIMENSION) * 4;
    assert!(
        peak <= reserved,
        "compaction peaked at {peak} heap bytes against a {reserved}-byte reservation ({} row bytes)",
        row_bytes
    );
    assert!(
        reserved < 4 * row_bytes,
        "the reservation is a bound, not a blank cheque: {reserved} for {row_bytes} row bytes"
    );
}

#[test]
fn a_long_checkpoint_in_the_sidecar_stays_within_the_reservation() {
    // The sidecar's strings dwarf the rows: the build holds them in the sidecar and again in its serialized bytes.
    let mut fixture = Fixture::new();
    let hold_id = "h".repeat(1 << 20);
    let rows = vec![ExportedRow {
        occurrence_id: "alpha".to_owned(),
        vector: support::vector_store::unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    }];
    let (reserved, peak) = reserved_and_peak(&mut fixture, rows, &hold_id, 64);
    assert!(
        peak <= reserved,
        "compaction peaked at {peak} heap bytes against a {reserved}-byte reservation"
    );
    assert!(
        reserved < 8 * hold_id.len() as u64,
        "the reservation is a bound, not a blank cheque: {reserved} for a {}-byte hold id",
        hold_id.len()
    );
}

/// One eight-wide unit row along `axis`.
fn narrow(axis: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[axis] = 1.0;
    support::vector_store::unit(raw)
}

#[test]
fn a_winner_count_past_a_power_of_two_stays_within_the_reservation() {
    // 4097 narrow rows: the resolver's winner vector doubles to 8192 slots, and the rows are too small to hide that.
    let mut fixture = Fixture::new();
    let rows = (0..4097)
        .map(|i| ExportedRow {
            occurrence_id: format!("{i:04}"),
            vector: narrow(i % 8),
        })
        .collect();
    let (reserved, peak) = reserved_and_peak(&mut fixture, rows, "hold-7", 8192);
    assert!(
        peak <= reserved,
        "compaction peaked at {peak} heap bytes against a {reserved}-byte reservation"
    );
}

#[test]
fn a_long_model_name_stays_within_the_reservation() {
    // The model name is in the sidecar and in the compatibility identity, so every path that serializes either holds another copy.
    let mut fixture = Fixture::new();
    fixture.generation.embedding_model = "m".repeat(1 << 20);
    let rows = vec![ExportedRow {
        occurrence_id: "alpha".to_owned(),
        vector: narrow(0),
    }];
    let (reserved, peak) = reserved_and_peak(&mut fixture, rows, "hold-7", 64);
    assert!(
        peak <= reserved,
        "compaction peaked at {peak} heap bytes against a {reserved}-byte reservation"
    );
    assert!(
        reserved < 8 << 20,
        "the reservation is a bound, not a blank cheque: {reserved} for a 1 MiB model name"
    );
}
