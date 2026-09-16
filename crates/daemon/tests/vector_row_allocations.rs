#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;

use alloc_recorder::{RecordingAlloc, record_window};
use retrieval::dense::codec::{self, ARTIFACT_HEADER_BYTES, Metric, RowLayout};

#[global_allocator]
static ALLOC: RecordingAlloc = RecordingAlloc;

#[test]
fn original_row_encoding_preserves_bytes_with_bounded_scratch() {
    for dimension in [128, 384, 1024] {
        let layout = RowLayout {
            dimension,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: 0.0,
        };
        let mut row = vec![-0.0; dimension as usize];
        row[0] = 1.0;
        let rows = vec![row; 256];
        let (bytes, ledger) =
            record_window(|| codec::encode_rows(&layout, rows.iter().map(Vec::as_slice)).unwrap());
        assert!(!ledger.overflow);
        assert_eq!(
            ledger.dealloc_events, 0,
            "only the output buffer is allocated"
        );
        assert_eq!(ledger.peak_live_bytes, bytes.capacity());
        assert!(ledger.allocation_events <= 16);
        assert_eq!(
            bytes.len(),
            ARTIFACT_HEADER_BYTES + 256 * dimension as usize * 4
        );
        for (chunk, row) in bytes[ARTIFACT_HEADER_BYTES..]
            .chunks_exact(dimension as usize * 4)
            .zip(&rows)
        {
            assert_eq!(chunk, codec::encode(row));
        }
        println!(
            "dimension={dimension} allocations={} reallocations={} requested_bytes={} peak_live_bytes={} output_bytes={}",
            ledger.allocation_events,
            ledger.realloc_events,
            ledger.requested_bytes,
            ledger.peak_live_bytes,
            bytes.len(),
        );
    }
}
