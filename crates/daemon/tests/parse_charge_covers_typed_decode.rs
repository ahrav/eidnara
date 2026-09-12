//! The decode's footprint must cover the typed decode's heap peak, not only the retained request.

#![cfg(feature = "test-support")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use daemon::metered_decode::{ResidentMeter, ResidentReserve, decode_metered, footprint_of};

/// A reserve with room for anything; the test reads what the meter counted, not what it held.
struct Unmetered;

impl ResidentReserve for Unmetered {
    fn try_reserve(&self, _bytes: usize) -> Option<host_runtime::wire::ByteCharge> {
        Some(host_runtime::wire::ByteCharge::none())
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}
use daemon::transform::TransformRequest;

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

fn reset_peak() -> usize {
    let live = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(live, Ordering::Relaxed);
    live
}

fn peak_since(base: usize) -> usize {
    PEAK_BYTES.load(Ordering::Relaxed) - base
}

fn dense_native_body(element_count: usize) -> Vec<u8> {
    let mut body = Vec::from(
        br#"{"kind":"transform","session_id":"s","render_config":"r","native_messages":[0"#
            .as_slice(),
    );
    for _ in 1..element_count {
        body.extend_from_slice(b",0");
    }
    body.extend_from_slice(b"]}");
    body
}

#[test]
fn parse_charge_covers_dense_native_typed_decode_peak() {
    // Element counts on both sides of a power of two exercise both vector capacity states.
    for element_count in [1usize << 16, (1 << 16) + 1, 1 << 18] {
        let body = dense_native_body(element_count);
        let charge = footprint_of(&body);

        let base = reset_peak();
        let tree: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        let request: TransformRequest = serde_json::from_value(tree).expect("typed decode");
        let peak = peak_since(base);
        assert_eq!(
            request.native_messages.as_ref().map(Vec::len),
            Some(element_count)
        );
        drop(request);

        assert!(
            peak <= charge,
            "{element_count} native elements peaked at {peak} bytes during typed decode but the \
             parse charge reserved only {charge}"
        );

        // The direct decode of an unpaged transform body builds no tree; the same charge covers it.
        let base = reset_peak();
        let request: TransformRequest = serde_json::from_slice(&body).expect("direct decode");
        let direct_peak = peak_since(base);
        assert_eq!(
            request.native_messages.as_ref().map(Vec::len),
            Some(element_count)
        );
        drop(request);
        assert!(
            direct_peak <= peak,
            "{element_count} native elements peaked at {direct_peak} bytes during the direct \
             decode, above the tree decode's {peak}"
        );

        // The charge the direct lane takes covers its own peak, with the dense values under
        // an ignored field the derive skips.
        let mut ignored = Vec::from(
            br#"{"kind":"transform","session_id":"s","render_config":"r","junk":[0"#.as_slice(),
        );
        for _ in 1..element_count {
            ignored.extend_from_slice(b",0");
        }
        ignored.extend_from_slice(b"]}");
        let meter = ResidentMeter::new(&Unmetered);
        let base = reset_peak();
        let request: TransformRequest = decode_metered(&ignored, &meter).expect("metered decode");
        let metered_peak = peak_since(base);
        drop(request);
        assert!(
            meter.needed() >= metered_peak,
            "{element_count} ignored elements peaked at {metered_peak} bytes but the direct lane \
             counted only {}",
            meter.needed()
        );
        assert_eq!(meter.needed(), footprint_of(&ignored));
    }
}
