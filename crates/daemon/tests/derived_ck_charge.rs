//! A native window's CK derivation is charged before it allocates; the charge must cover the
//! derivation's heap peak, the derived CK included, for both native codecs.

#![cfg(feature = "test-support")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use daemon::codec::derived_ck_charge;
use serde_json::{Value, json};

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Counts layout sizes; allocator rounding is not included, so the peak is a floor.
struct PeakAlloc;

impl PeakAlloc {
    fn grow(bytes: usize) {
        let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }

    fn shrink(bytes: usize) {
        LIVE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            Self::grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::shrink(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size > layout.size() {
                Self::grow(new_size - layout.size());
            } else {
                Self::shrink(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: PeakAlloc = PeakAlloc;

/// The peak counter is process-wide, so the measurements run one at a time.
fn measure() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The derivation's heap peak over `native`, with the derived window still held.
fn derivation_peak(profile: &str, native: &[Arc<Value>]) -> usize {
    let base = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(base, Ordering::Relaxed);
    let derived = daemon::derive_native_window_for_test(profile, native).unwrap();
    let peak = PEAK_BYTES.load(Ordering::Relaxed) - base;
    drop(derived);
    peak
}

fn shared(values: Vec<Value>) -> Vec<Arc<Value>> {
    values.into_iter().map(Arc::new).collect()
}

fn opencode(id: usize, role: &str, parts: Vec<Value>) -> Value {
    json!({
        "info": {
            "id": format!("msg_{id:06}"), "sessionID": "ses", "role": role,
            "time": { "created": 1_777_654_348_000u64 + id as u64 },
            "providerID": "anthropic", "modelID": "claude"
        },
        "parts": parts,
    })
}

fn pi(id: usize, message: Value) -> Value {
    json!({ "id": format!("{id:08x}"), "message": message })
}

/// Windows that stress what the derivation clones: long text, long tool inputs and outputs,
/// provider extras, escaped strings, many small messages, and reasoning with signatures.
fn opencode_windows() -> Vec<(&'static str, Vec<Value>)> {
    let long = "x".repeat(256 * 1024);
    let escaped = "\"\\\n\u{1F600}".repeat(16 * 1024);
    let extras = json!({ "anthropic": { "signature": "s".repeat(4096), "cache": [1, 2, 3] } });
    vec![
        ("text", (0..32).map(|i| opencode(i, "user", vec![json!({ "type": "text", "text": long })])).collect()),
        ("escaped", (0..32).map(|i| opencode(i, "user", vec![json!({ "type": "text", "text": escaped })])).collect()),
        ("tool", (0..64).map(|i| opencode(i, "assistant", vec![json!({
            "type": "tool", "callID": format!("call_{i}"), "tool": "bash",
            "state": { "status": "completed", "input": { "command": long, "args": [long, long] }, "output": long }
        })])).collect()),
        ("extras", (0..256).map(|i| opencode(i, "assistant", vec![
            json!({ "type": "reasoning", "text": "think", "metadata": extras, "cache_control": { "type": "ephemeral" } }),
            json!({ "type": "text", "text": "answer", "metadata": extras, "cache_control": { "type": "ephemeral" } }),
        ])).collect()),
        ("small", (0..20_000).map(|i| opencode(i, "user", vec![json!({ "type": "text", "text": "hi" })])).collect()),
        ("idless", (0..2_000).map(|i| json!({ "role": "assistant", "parts": [
            { "type": "tool", "tool": "read", "state": { "status": "pending", "input": { "path": format!("/p/{i}") } } }
        ] })).collect()),
        // An id-less message is named by a hash of its encoding, whose escapes run six bytes
        // to each control character.
        (
            "idless_escaped",
            (0..4)
                .map(|_| json!({ "role": "user", "parts": [{ "type": "text", "text": "\u{1}".repeat(1 << 20) }] }))
                .collect(),
        ),
        ("scalar_parts", vec![json!({ "info": { "id": "m" }, "parts": vec![json!(0); 50_000] })]),
        ("bare_tools", vec![json!({ "info": { "id": "m" }, "parts": vec![json!({ "type": "tool" }); 50_000] })]),
        ("bare_tools_completed", vec![json!({ "parts": vec![json!({ "type": "tool", "state": { "status": "error" } }); 50_000] })]),
        ("empty_messages", vec![json!({}); 50_000]),
        ("long_ids", (0..8).map(|i| json!({ "info": { "id": format!("{i}{}", "i".repeat(1 << 20)) }, "parts": [] })).collect()),
        ("scalar_messages", vec![json!(0); 50_000]),
        ("unknown_parts", vec![json!({ "info": { "id": "m" }, "parts": vec![json!({ "type": "x", "approvalId": "a" }); 50_000] })]),
        ("attachments", vec![json!({ "info": { "id": "m" }, "parts": [{ "type": "tool", "callID": "c", "state": { "status": "completed", "attachments": vec![json!({}); 50_000] } }] })]),
    ]
}

fn pi_windows() -> Vec<(&'static str, Vec<Value>)> {
    let long = "y".repeat(256 * 1024);
    vec![
        ("text", (0..32).map(|i| pi(i, json!({ "role": "user", "content": [{ "type": "text", "text": long }], "timestamp": 1 }))).collect()),
        ("tool", (0..64).map(|i| pi(i, json!({
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": format!("call_{i}"), "name": "bash", "arguments": { "command": long } }],
            "usage": {}, "stopReason": "toolUse", "timestamp": 2
        }))).collect()),
        ("result", (0..64).map(|i| pi(i, json!({
            "role": "toolResult", "toolCallId": format!("call_{i}"), "toolName": "bash",
            "content": [{ "type": "text", "text": long }], "isError": false, "timestamp": 3
        }))).collect()),
        ("small", (0..20_000).map(|i| pi(i, json!({ "role": "user", "content": "hi", "timestamp": 4 }))).collect()),
        ("empty_blocks", vec![pi(1, json!({ "role": "user", "content": vec![json!({ "type": "text", "text": "" }); 50_000], "timestamp": 1 }))]),
        ("scalar_blocks", vec![pi(1, json!({ "role": "user", "content": vec![json!(0); 50_000], "timestamp": 1 }))]),
        ("bare_calls", vec![pi(1, json!({ "role": "assistant", "content": vec![json!({ "type": "toolCall" }); 50_000], "timestamp": 1 }))]),
        ("long_ids", (0..8).map(|i| pi(0, json!({ "role": "user", "content": "x", "timestamp": 1 })).as_object().cloned().map(|mut row| { row.insert("id".into(), json!(format!("{i}{}", "a".repeat(1 << 20)))); Value::Object(row) }).unwrap()).collect()),
        ("bash_output", vec![pi(1, json!({ "role": "bashExecution", "command": "ls", "output": long, "exitCode": 2, "timestamp": 1 }))]),
        ("bash_command_and_path", vec![pi(1, json!({
            "role": "bashExecution", "command": long, "output": long, "truncated": true,
            "fullOutputPath": long, "cancelled": true, "timestamp": 1
        }))]),
        ("long_summaries", vec![
            json!({ "id": "eidnara:branchSummary:00000001", "message": { "role": "branchSummary", "summary": long, "timestamp": 1 } }),
            json!({ "id": "eidnara:compactionSummary:00000002", "message": { "role": "compactionSummary", "summary": long, "tokensBefore": 1, "timestamp": 1 } }),
        ]),
        ("bash", (0..20_000).map(|i| pi(i, json!({ "role": "bashExecution", "command": "", "timestamp": 1 }))).collect()),
        ("summaries", (0..20_000).map(|i| json!({ "id": format!("eidnara:branchSummary:{i:08x}"), "message": { "role": "branchSummary", "summary": "", "timestamp": 1 } })).collect()),
    ]
}

/// The charge covers the derivation's peak for every window, and the measured peak per charged
/// native byte stays under the factor the charge applies.
#[test]
fn the_derived_ck_charge_covers_the_derivation_peak() {
    let _serial = measure();
    let mut worst = 0.0f64;
    // The measured peak over its charge; the evidence for the bound's constants.
    for (profile, windows) in [("opencode-aisdk", opencode_windows()), ("pi", pi_windows())] {
        for (name, window) in windows {
            let native = shared(window);
            let charge = derived_ck_charge(&native);
            let peak = derivation_peak(profile, &native);
            let used = peak as f64 / charge as f64;
            println!("{profile} {name}: peak {peak} charge {charge} used {used:.3}");
            worst = worst.max(used);
            assert!(
                peak <= charge,
                "{profile} {name}: peak {peak} over charge {charge}"
            );
        }
    }
    println!("largest share of the charge used {worst:.3}");
}
