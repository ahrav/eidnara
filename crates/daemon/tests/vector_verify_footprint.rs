//! `verify`'s accepted byte bound covers peak live heap usage during verification.

#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;
mod support;

use alloc_recorder::record_window;
use daemon::vector_generation::{ROWS_FILE, VectorRefusal, verify};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use support::vector_store::{Fixture, unit};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

/// One eight-wide unit row that differs between neighbouring indices.
fn narrow(index: usize) -> Vec<f32> {
    let mut raw = [0.0f32; 8];
    raw[index % 8] = 1.0;
    raw[(index / 8) % 8] += 0.5;
    unit(raw)
}

fn export(
    fixture: &Fixture,
    rows: Vec<ExportedRow>,
    tombstones: Vec<String>,
    hold_id: &str,
) -> LiveRows {
    LiveRows {
        generation: fixture.generation.clone(),
        kernel_incarnation_id: fixture.identity.kernel_incarnation_id.clone(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 9,
            checkpoint_commit_seq: 10,
            hold_id: hold_id.to_owned(),
        },
        rows,
        tombstones,
    }
}

/// Raises the bound to each refusal's reported bytes until `verify` accepts.
fn accepted_bound(fixture: &Fixture, digest: &str) -> u64 {
    let mut max = 0;
    loop {
        match verify(&fixture.store, digest, &fixture.expected(), max) {
            Ok(_) => return max,
            Err(VectorRefusal::OverBound { bytes, .. }) => {
                assert!(
                    bytes > max,
                    "a refusal reports more than the bound it refused"
                );
                max = bytes;
            }
            Err(other) => panic!("{other:?}"),
        }
    }
}

/// Stages `export` and returns the accepted bound, the heap peak of one verification under it, and the row artifact's size.
fn bound_and_peak(fixture: &Fixture, export: &LiveRows) -> (u64, u64, u64) {
    let digest = fixture.layer_from(export).digest;
    let rows_bytes = std::fs::metadata(fixture.generation_dir(&digest).join(ROWS_FILE))
        .unwrap()
        .len();
    let bound = accepted_bound(fixture, &digest);
    let expected = fixture.expected();
    let (verified, ledger) = record_window(|| verify(&fixture.store, &digest, &expected, bound));
    drop(verified.unwrap());
    (bound, ledger.peak_live_bytes as u64, rows_bytes)
}

#[test]
fn many_identifiers_stay_within_the_verification_bound() {
    // 4097 identifiers: the decoded vector's capacity passes a power of two, and each identifier is held as JSON and as a string together.
    let fixture = Fixture::new();
    let rows = (0..4097)
        .map(|i| ExportedRow {
            occurrence_id: format!("{i:064x}"),
            vector: narrow(i),
        })
        .collect();
    let (bound, peak, _) = bound_and_peak(&fixture, &export(&fixture, rows, Vec::new(), "hold-7"));
    assert!(
        peak <= bound,
        "verification peaked at {peak} heap bytes against a {bound}-byte bound"
    );
}

#[test]
fn many_tombstones_stay_within_the_verification_bound() {
    // Tombstones decode while the identifiers they are checked against stay decoded.
    let fixture = Fixture::new();
    let rows = (0..64)
        .map(|i| ExportedRow {
            occurrence_id: format!("{i:064x}"),
            vector: narrow(i),
        })
        .collect();
    let tombstones = (0..4097).map(|i| format!("t{i:063x}")).collect();
    let (bound, peak, _) = bound_and_peak(&fixture, &export(&fixture, rows, tombstones, "hold-7"));
    assert!(
        peak <= bound,
        "verification peaked at {peak} heap bytes against a {bound}-byte bound"
    );
}

#[test]
fn a_long_checkpoint_in_the_sidecar_stays_within_the_verification_bound() {
    // The sidecar's strings dwarf every other table, so each copy of the sidecar that verification holds shows.
    let fixture = Fixture::new();
    let rows = vec![ExportedRow {
        occurrence_id: "alpha".to_owned(),
        vector: narrow(0),
    }];
    let hold_id = "h".repeat(1 << 20);
    let (bound, peak, _) = bound_and_peak(&fixture, &export(&fixture, rows, Vec::new(), &hold_id));
    assert!(
        peak <= bound,
        "verification peaked at {peak} heap bytes against a {bound}-byte bound"
    );
}

#[test]
fn a_long_model_name_stays_within_the_verification_bound() {
    // The large model name magnifies the heap cost of any copy of it made during verification.
    let fixture = Fixture::with_embedding_model(&"m".repeat(1 << 19));
    let rows = vec![ExportedRow {
        occurrence_id: "alpha".to_owned(),
        vector: narrow(0),
    }];
    let (bound, peak, _) = bound_and_peak(&fixture, &export(&fixture, rows, Vec::new(), "hold-7"));
    assert!(
        peak <= bound,
        "verification peaked at {peak} heap bytes against a {bound}-byte bound"
    );
}

#[test]
fn wide_rows_stream_within_a_bound_below_the_row_payload() {
    // Each 4096-coordinate row is 16 KiB, so the 33 rows stream through three chunks of whole rows.
    const DIMENSION: u32 = 4096;
    let fixture = Fixture::with_dimension(DIMENSION);
    let rows = (0..33)
        .map(|i| {
            let mut vector = vec![0.0f32; DIMENSION as usize];
            vector[i] = 1.0;
            ExportedRow {
                occurrence_id: format!("{i:064x}"),
                vector,
            }
        })
        .collect();
    let (bound, peak, rows_bytes) =
        bound_and_peak(&fixture, &export(&fixture, rows, Vec::new(), "hold-7"));
    assert!(
        peak <= bound,
        "verification peaked at {peak} heap bytes against a {bound}-byte bound"
    );
    assert!(
        bound < rows_bytes,
        "the bound streams the rows rather than holding them: {bound} for {rows_bytes} row bytes"
    );
}
