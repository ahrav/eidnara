//! The decode's footprint must cover the typed decode's heap peak, not only the retained request.

#![cfg(feature = "test-support")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

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

/// The peak counter is process-wide, so tests that read it run one at a time.
fn measure() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    let _serial = measure();
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

fn text_body(text_bytes: usize, escaped: bool) -> Vec<u8> {
    let mut body = Vec::from(
        br#"{"kind":"transform","session_id":"s","render_config":"r","messages":[{"mid":"m","ordinal":0,"ck":{"role":"user","content":[{"kind":{"type":"text","text":""#
            .as_slice(),
    );
    if escaped {
        body.extend_from_slice(br"\n");
    }
    body.resize(body.len() + text_bytes, b'x');
    body.extend_from_slice(br#""}}]}}]}"#);
    body
}

#[test]
fn parse_charge_covers_escaped_text_direct_decode_peak() {
    let _serial = measure();
    // A string with one escape is unescaped into the deserializer's scratch buffer, which the
    // direct decode holds beside the retained copies; a string without one is borrowed.
    for (text_bytes, escaped) in [(1usize << 22, true), (1 << 22, false), (1 << 16, true)] {
        let body = text_body(text_bytes, escaped);
        let charge = footprint_of(&body);

        let base = reset_peak();
        let request: TransformRequest = serde_json::from_slice(&body).expect("direct decode");
        let direct_peak = peak_since(base);
        assert_eq!(request.messages.len(), 1);
        drop(request);

        assert!(
            direct_peak <= charge,
            "{text_bytes} text bytes (escaped: {escaped}) peaked at {direct_peak} bytes during \
             the direct decode but the parse charge reserved only {charge}"
        );
    }
}

#[test]
fn byte_cap_admits_a_facade_sized_body_without_body_proportional_allocation() {
    let _serial = measure();
    // The cap runs before the resident reservation, so a body at or under the facade cap must
    // reach the reservation without a parse: an escaped key that long would otherwise be
    // unescaped into a body-sized buffer outside admission control.
    let key_bytes = 900 * 1024;
    let mut body = Vec::from(br#"{"\n"#.as_slice());
    body.resize(body.len() + key_bytes, b'k');
    body.extend_from_slice(br#"":1,"kind":"transform"}"#);
    assert!(body.len() <= 1024 * 1024);

    let base = reset_peak();
    let admitted = daemon::request_byte_cap_admits_for_test(&body);
    let peak = peak_since(base);
    assert!(admitted);
    assert!(
        peak < key_bytes / 2,
        "the byte cap allocated {peak} bytes before the resident reservation for a {} byte body",
        body.len()
    );
}

/// A reserve that is already drained: every charge is refused as transient.
struct Drained;

impl ResidentReserve for Drained {
    fn try_reserve(&self, _bytes: usize) -> Option<host_runtime::wire::ByteCharge> {
        None
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

#[test]
fn a_held_pool_stops_the_direct_lane_walk_before_it_unescapes_a_large_string() {
    let _serial = measure();
    // The walk that gates the direct lane is charged like the decodes, so a pool that has no
    // bytes free refuses it at its first value and serde_json never grows an unescape buffer
    // for the 4 MiB text block further into the body.
    let body = text_body(1 << 22, true);
    assert!(
        footprint_of(&body) > 1 << 22,
        "the body's footprint covers its text"
    );

    let base = reset_peak();
    let gate = daemon::direct_lane_gate_for_test(&body, &Drained);
    let peak = peak_since(base);
    assert!(
        peak < 64 * 1024,
        "the walk allocated {peak} bytes against a held pool; the 4 MiB text block was \
         unescaped before any charge (gate: {gate:?})"
    );
    assert!(
        matches!(gate, Err(daemon::metered_decode::Refusal::Transient)),
        "the held pool refuses the walk: {gate:?}"
    );
}
