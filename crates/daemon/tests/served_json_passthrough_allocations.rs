//! Canonical served serialization of a passthrough shell must not pay per-key heap work.

#![cfg(feature = "test-support")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use memory_store::WireMessage;

static EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Counts allocation and reallocation events; sizes are irrelevant to this oracle.
struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        EVENTS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        EVENTS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const KEYS_PER_EXTRA_OBJECT: usize = 8;

/// Every key is unescaped ASCII, so no object needs the escape-aware decode path.
fn passthrough_message(block_count: usize) -> WireMessage {
    let mut body = String::from(r#"{"role":"user","content":["#);
    for block in 0..block_count {
        if block != 0 {
            body.push(',');
        }
        body.push_str(r#"{"extra":{"#);
        for key in 0..KEYS_PER_EXTRA_OBJECT {
            if key != 0 {
                body.push(',');
            }
            body.push_str(&format!(r#""k{key:02}":{key}"#));
        }
        body.push_str(r#"},"kind":{"text":"t","type":"text"}}"#);
    }
    body.push_str("]}");
    let message: WireMessage = serde_json::from_str(&body).unwrap();
    assert!(message.original().is_some());
    message
}

fn allocation_events(message: &WireMessage) -> (usize, Vec<u8>) {
    let before = EVENTS.load(Ordering::Relaxed);
    let bytes = daemon::served_json::canonical_served_bytes_for_test(message);
    (EVENTS.load(Ordering::Relaxed) - before, bytes)
}

/// Each block has 12 keys; a cap below 12 rejects one allocation per decoded key.
const MAX_EVENTS_PER_BLOCK: usize = 8;

#[test]
fn passthrough_shell_canonicalization_allocates_independently_of_key_count() {
    let small = passthrough_message(1);
    let large = passthrough_message(65);
    let (small_events, small_bytes) = allocation_events(&small);
    let (large_events, large_bytes) = allocation_events(&large);
    assert_eq!(
        small_bytes,
        serde_json::to_vec(&serde_json::to_value(&small).unwrap()).unwrap()
    );
    assert_eq!(
        large_bytes,
        serde_json::to_vec(&serde_json::to_value(&large).unwrap()).unwrap()
    );
    let per_block = (large_events - small_events) / 64;
    assert!(
        per_block <= MAX_EVENTS_PER_BLOCK,
        "{per_block} allocation events per passthrough block (small {small_events}, large {large_events})"
    );
}
