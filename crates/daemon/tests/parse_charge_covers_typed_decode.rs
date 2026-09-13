//! The decode's footprint must cover the typed decode's heap peak, not only the retained request.

#![cfg(feature = "test-support")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use daemon::metered_decode::{
    ResidentMeter, ResidentReserve, count_only_reserve, decode_metered, footprint_of,
};

use daemon::transform::TransformRequest;

/// A reserve with room for anything; the tests read what the meter counted, not what it held.
struct Unmetered;

impl ResidentReserve for Unmetered {
    fn try_reserve(&self, _bytes: usize) -> Option<host_runtime::wire::ByteCharge> {
        Some(host_runtime::wire::ByteCharge::none())
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

const UNMETERED: Unmetered = Unmetered;

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
        let meter = ResidentMeter::new(&UNMETERED);
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
    let outcome = daemon::direct_lane_admission_for_test(&body, &Drained);
    let peak = peak_since(base);
    assert!(
        peak < 64 * 1024,
        "the refusal allocated {peak} bytes against a held pool; the 4 MiB text block was \
         unescaped before any charge (outcome: {outcome:?})"
    );
    assert!(
        matches!(&outcome, Err(daemon::dispatch::PreparedOutcome::Error { code, .. }) if code == "queue_full"),
        "the held pool refuses the body as transient: {outcome:?}"
    );
}

/// A reserve that grants charges until `limit` bytes are held, then refuses every one.
struct Granting {
    limit: usize,
    held: std::sync::atomic::AtomicUsize,
}

impl ResidentReserve for Granting {
    fn try_reserve(&self, bytes: usize) -> Option<host_runtime::wire::ByteCharge> {
        let held = self.held.load(Ordering::Relaxed);
        if held + bytes > self.limit {
            return None;
        }
        self.held.store(held + bytes, Ordering::Relaxed);
        Some(host_runtime::wire::ByteCharge::none())
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

#[test]
fn a_pool_with_room_for_the_prefix_only_refuses_before_the_large_string_is_unescaped() {
    let _serial = measure();
    // The pool admits the body's leading values but not the 4 MiB text block. The charge for
    // the unescape buffer that block needs is taken from the bytes before any decode, so the
    // refusal arrives before serde_json has grown that buffer.
    let body = text_body(1 << 22, true);
    let reserve = Granting {
        limit: 64 * 1024,
        held: std::sync::atomic::AtomicUsize::new(0),
    };

    let base = reset_peak();
    let outcome = daemon::direct_lane_admission_for_test(&body, &reserve);
    let peak = peak_since(base);
    assert!(
        peak < 64 * 1024,
        "the refusal allocated {peak} bytes with 64 KiB of pool; the 4 MiB text block was \
         unescaped before its charge (outcome: {outcome:?})"
    );
    assert!(
        matches!(&outcome, Err(daemon::dispatch::PreparedOutcome::Error { code, .. }) if code == "queue_full"),
        "the short pool refuses the body as transient: {outcome:?}"
    );
}

#[test]
fn a_held_pool_refuses_before_the_lane_probe_unescapes_a_long_key() {
    let _serial = measure();
    // The lane probe reads a sub-cap body's top-level keys, unescaping an escaped one into
    // serde_json's buffer, so the admission charge runs before it: a pool with nothing free
    // refuses the body before the probe reads a 900 KiB escaped key.
    let key_bytes = 900 * 1024;
    let mut body = Vec::from(br#"{"\n"#.as_slice());
    body.resize(body.len() + key_bytes, b'k');
    body.extend_from_slice(br#"":1,"kind":"transform"}"#);
    assert!(body.len() <= 1024 * 1024);

    let base = reset_peak();
    let outcome = daemon::handle_entry_for_test(&body, &Drained);
    let peak = peak_since(base);
    assert!(
        peak < 64 * 1024,
        "the entry allocated {peak} bytes against a held pool before any charge (outcome: {outcome:?})"
    );
    assert!(
        matches!(&outcome, Err(daemon::dispatch::PreparedOutcome::Error { code, .. }) if code == "queue_full"),
        "the held pool refuses the body before the probe: {outcome:?}"
    );
}

/// The production tree lane: the metered `Value` parse, then the consuming typed conversion.
/// The peak is read across both steps, with the tree and the request live at the handoff.
fn tree_lane_peak(body: &[u8], meter: &ResidentMeter<'_>) -> (TransformRequest, usize) {
    let base = reset_peak();
    let tree: serde_json::Value = decode_metered(body, meter).expect("tree decode");
    let request: TransformRequest = serde_json::from_value(tree).expect("typed conversion");
    (request, peak_since(base))
}

/// The production direct lane: the metered typed decode from the bytes.
fn direct_lane_peak(body: &[u8], meter: &ResidentMeter<'_>) -> (TransformRequest, usize) {
    let base = reset_peak();
    let request: TransformRequest = decode_metered(body, meter).expect("direct decode");
    (request, peak_since(base))
}

#[test]
fn parse_charge_covers_text_heavy_peaks_on_both_lanes() {
    let _serial = measure();
    for (text_bytes, escaped) in [(1usize << 22, true), (1 << 22, false), (1 << 16, true)] {
        let body = text_body(text_bytes, escaped);
        for lane in [direct_lane_peak, tree_lane_peak] {
            let meter = ResidentMeter::new(&UNMETERED);
            meter
                .reserve_unescape_scratch(&body)
                .expect("the scratch charge is granted");
            let (request, peak) = lane(&body, &meter);
            assert_eq!(request.messages.len(), 1);
            drop(request);
            assert!(
                peak <= meter.needed(),
                "{text_bytes} text bytes (escaped: {escaped}) peaked at {peak} bytes but the lane \
                 charged only {}",
                meter.needed()
            );
        }
    }
}

/// A body whose typed decode fails at a duplicate key inside its last message, after a 4 MiB
/// text block, so the direct lane falls back to the tree from the same bytes.
fn late_duplicate_body(escaped: bool) -> Vec<u8> {
    let mut body = text_body(1 << 22, escaped);
    let tail = br#"]}"#;
    assert!(body.ends_with(tail));
    body.truncate(body.len() - tail.len());
    body.extend_from_slice(
        br#",{"mid":"dup-first","mid":"dup-last","ordinal":1,"ck":{"role":"user","content":[]}}]}"#,
    );
    body
}

#[test]
fn parse_charge_covers_a_failed_typed_prefix_and_its_tree_fallback() {
    let _serial = measure();
    for escaped in [false, true] {
        let body = late_duplicate_body(escaped);
        assert!(
            serde_json::from_slice::<TransformRequest>(&body).is_err(),
            "the typed decode refuses the duplicate key"
        );
        let meter = ResidentMeter::new(&UNMETERED);
        let base = reset_peak();
        let walked = daemon::direct_lane_admission_for_test(&body, &UNMETERED)
            .expect("the walk is admitted");
        assert!(walked, "the tree walk accepts the duplicate key");
        meter
            .reserve_unescape_scratch(&body)
            .expect("the scratch charge is granted");
        let failure = decode_metered::<TransformRequest>(&body, &meter).unwrap_err();
        assert!(
            matches!(failure, daemon::metered_decode::DecodeFailure::Invalid(_)),
            "{failure:?}"
        );
        let typed_needed = meter.needed();
        meter.restart();
        let tree: serde_json::Value = decode_metered(&body, &meter).expect("tree decode");
        let request: TransformRequest = serde_json::from_value(tree).expect("typed conversion");
        let peak = peak_since(base);
        assert_eq!(request.messages.len(), 2);
        assert_eq!(
            request.messages[1].mid, "dup-last",
            "the tree fallback keeps the last value of the duplicate key"
        );
        drop(request);
        let largest_needed = footprint_of(&body).max(typed_needed).max(meter.needed());
        assert!(
            peak <= largest_needed,
            "escaped {escaped}: the walk, the failed typed prefix, and the tree fallback peaked at \
             {peak} bytes but the largest count reached was {largest_needed}"
        );
    }
}

/// Many small retained payload `Value`s: tool inputs and provider extras, not text.
fn payload_heavy_body(block_count: usize) -> Vec<u8> {
    let mut body = Vec::from(
        br#"{"kind":"transform","session_id":"s","render_config":"r","messages":[{"mid":"m","ordinal":0,"ck":{"role":"assistant","content":["#
            .as_slice(),
    );
    for index in 0..block_count {
        if index != 0 {
            body.push(b',');
        }
        body.extend_from_slice(
            format!(
                r#"{{"kind":{{"type":"tool_call","id":"c{index}","name":"t","input":{{"a":[1,2,{{"b":null}}],"c":"v{index}"}}}},"provider_extras":{{"p":{{"k":{{"n":{index}}}}}}}}}"#
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(br#"]}}]}"#);
    body
}

/// The tree lane builds each small object as a `BTreeMap` leaf node of eleven slots before conversion, which the per-value node charge does not cover; that gap is container storage, independent of the string coefficient, and `TREE_LANE_CONTAINER_SLACK_DENOMINATOR` pins its measured size so it cannot widen unnoticed.
const TREE_LANE_CONTAINER_SLACK_DENOMINATOR: usize = 3;

#[test]
fn parse_charge_covers_payload_heavy_direct_decode_peak_and_pins_the_tree_lane_gap() {
    let _serial = measure();
    for block_count in [64usize, 4096] {
        let body = payload_heavy_body(block_count);
        let meter = ResidentMeter::new(&UNMETERED);
        meter
            .reserve_unescape_scratch(&body)
            .expect("the scratch charge is granted");
        let (request, peak) = direct_lane_peak(&body, &meter);
        assert_eq!(request.messages[0].ck.content().len(), block_count);
        drop(request);
        assert!(
            peak <= meter.needed(),
            "{block_count} payload blocks peaked at {peak} bytes but the direct lane charged only {}",
            meter.needed()
        );
        let tree_meter = ResidentMeter::new(&UNMETERED);
        tree_meter
            .reserve_unescape_scratch(&body)
            .expect("the scratch charge is granted");
        let (request, tree_peak) = tree_lane_peak(&body, &tree_meter);
        drop(request);
        let charged = tree_meter.needed();
        assert!(
            tree_peak > charged,
            "{block_count} payload blocks: the tree lane peaked at {tree_peak} bytes within its charge of {charged}; the container gap closed and this pin can be removed"
        );
        assert!(
            tree_peak <= charged + charged / TREE_LANE_CONTAINER_SLACK_DENOMINATOR,
            "{block_count} payload blocks: the tree lane peaked at {tree_peak} bytes against a charge of {charged}, above the pinned container gap"
        );
    }
}

/// The text-heavy admission ceiling under one numeric capacity.
/// An unescaped text block is charged once, so the largest admissible body approaches the capacity itself.
/// One escape charges the unescape scratch buffer at twice the block, so that ceiling is a third of the capacity.
#[test]
fn text_heavy_admission_ceiling_witnesses() {
    let _serial = measure();
    const CAPACITY: usize = 8 << 20;
    let reserve = count_only_reserve(CAPACITY);
    let margin = 64 * 1024;
    for (text_bytes, escaped, admitted) in [
        (CAPACITY - margin, false, true),
        (CAPACITY + margin, false, false),
        (CAPACITY / 3 - margin, true, true),
        (CAPACITY / 3 + margin, true, false),
        (CAPACITY / 3 + margin, false, true),
    ] {
        let body = text_body(text_bytes, escaped);
        let outcome = daemon::direct_lane_admission_for_test(&body, &reserve);
        assert_eq!(
            outcome.is_ok(),
            admitted,
            "{text_bytes} text bytes (escaped: {escaped}) against {CAPACITY}: {outcome:?}"
        );
        assert_eq!(footprint_of(&body) <= CAPACITY, admitted);
        if admitted {
            let meter = ResidentMeter::new(&reserve);
            meter
                .reserve_unescape_scratch(&body)
                .expect("the scratch charge is granted");
            let (request, peak) = direct_lane_peak(&body, &meter);
            drop(request);
            assert!(
                meter.needed() <= CAPACITY,
                "{text_bytes} text bytes (escaped: {escaped}) charged {} against {CAPACITY}",
                meter.needed()
            );
            assert!(
                peak <= meter.needed(),
                "{text_bytes} text bytes (escaped: {escaped}) peaked at {peak} bytes against a charge of {}",
                meter.needed()
            );
        } else {
            assert!(matches!(
                outcome,
                Err(daemon::dispatch::PreparedOutcome::Error { code, .. }) if code == "invalid_params"
            ));
        }
    }
}
