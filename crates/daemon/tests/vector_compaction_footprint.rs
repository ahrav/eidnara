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

#[test]
fn the_resident_reservation_bounds_the_compactors_heap_peak() {
    let mut fixture = Fixture::with_dimension(DIMENSION);
    let rows = (0..ROWS)
        .map(|i| ExportedRow {
            occurrence_id: format!("{i:064x}"),
            vector: axis(i),
        })
        .collect();
    let base = fixture.layer_from(&LiveRows {
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 9,
            checkpoint_commit_seq: 10,
            hold_id: "hold-7".to_owned(),
        },
        rows,
        tombstones: Vec::new(),
    });
    fixture
        .publish(&fixture.compose(1, &base, &[]).unwrap())
        .unwrap();
    let view = acquire_view(&mut fixture, &mut |_| {}).unwrap();
    let max_entries = NonZeroUsize::new(64).unwrap();

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
    let peak = ledger.peak_live_bytes as u64;
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
