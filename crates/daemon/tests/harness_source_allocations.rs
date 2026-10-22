#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;

use alloc_recorder::record_window;
use daemon::harness_sources::{Harness, SessionIdentity, opencode_units_within, pi_units_within};
use serde_json::{Value, json};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const ID_BYTES: usize = 1 << 20;
const UNITS: usize = 64;
const ALLOWANCE: usize = 4 * ID_BYTES;

fn session(harness: Harness) -> SessionIdentity {
    SessionIdentity {
        project_id: "a".repeat(64),
        harness,
        session_id: "ses".to_owned(),
    }
}

fn assert_adapter_stops_at_its_allowance(
    adapt: impl FnOnce(&mut dyn FnMut(usize) -> bool) -> Option<usize>,
) {
    let mut granted = 0usize;
    let mut asked = 0usize;
    let (built, ledger) = record_window(|| {
        adapt(&mut |bytes| {
            asked += 1;
            granted += bytes;
            granted <= ALLOWANCE
        })
    });
    assert_eq!(built, None, "the adapter stops once the allowance refuses");
    assert!(asked < UNITS, "asked for {asked} units");
    assert!(!ledger.overflow, "allocation ledger overflowed");
    assert!(
        ledger.peak_live_bytes < ALLOWANCE + 4 * ID_BYTES,
        "adapting peaked at {} bytes",
        ledger.peak_live_bytes
    );
}

#[test]
fn opencode_adaptation_stops_allocating_at_its_allowance() {
    let parts: Vec<Value> = (0..UNITS)
        .map(|_| json!({ "type": "text", "text": "" }))
        .collect();
    let record = json!({
        "info": { "id": "m".repeat(ID_BYTES), "sessionID": "ses", "role": "user",
                  "time": { "created": 1_700_000_000_000u64 } },
        "parts": parts,
    });
    let session = session(Harness::OpenCode);
    assert_adapter_stops_at_its_allowance(|admit| {
        opencode_units_within(&session, &record, admit)
            .unwrap()
            .map(|units| units.len())
    });
}

#[test]
fn pi_adaptation_stops_allocating_at_its_allowance() {
    let content: Vec<Value> = (0..UNITS)
        .map(|_| json!({ "type": "text", "text": "" }))
        .collect();
    let record = json!({
        "type": "message", "id": "m".repeat(ID_BYTES), "parentId": null,
        "message": { "role": "user", "timestamp": 1_700_000_000_000u64, "content": content },
    });
    let session = session(Harness::Pi);
    assert_adapter_stops_at_its_allowance(|admit| {
        pi_units_within(&session, &record, admit)
            .unwrap()
            .map(|units| units.len())
    });
}
