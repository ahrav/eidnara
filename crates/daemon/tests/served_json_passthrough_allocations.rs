//! Canonical served serialization of a passthrough shell must not pay per-key heap work,
//! and every declared served-output population must have an absolute allocation ledger.
//!
//! The recorder is thread-owned, so harness threads never enter the ledger.

#![cfg(feature = "test-support")]

#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;
#[path = "support/served_output_fixtures.rs"]
mod served_output_fixtures;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc_recorder::{BufferProvenance, Event, Ledger, record_window};
use memory_store::WireMessage;
use served_output_fixtures::{
    BLOCK_COUNTS, KEYS_PER_ASCII_BLOCK, declaration_order_equals_canonical, populations,
    reference_bytes, serialization_buffer_root_bytes,
};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

fn canonicalize_recorded(message: &WireMessage) -> (Vec<u8>, Ledger) {
    record_window(|| daemon::served_json::canonical_served_bytes_for_test(message))
}

/// A cap below the per-block key count rejects one allocation per decoded key.
const MAX_EVENTS_PER_BLOCK: usize = KEYS_PER_ASCII_BLOCK - 4;
const _: () = assert!(MAX_EVENTS_PER_BLOCK < KEYS_PER_ASCII_BLOCK);
const ARC_HEADER_BYTES: usize = 2 * std::mem::size_of::<usize>();
/// Lowercase hex of a SHA-256 digest.
const HEX_DIGEST_BYTES: usize = 64;

#[test]
fn passthrough_shell_canonicalization_allocates_independently_of_key_count() {
    let [small_blocks, large_blocks] = BLOCK_COUNTS;
    let small = served_output_fixtures::retained_ascii_message(small_blocks);
    let large = served_output_fixtures::retained_ascii_message(large_blocks);
    let (small_bytes, small_ledger) = canonicalize_recorded(&small);
    let (large_bytes, large_ledger) = canonicalize_recorded(&large);
    assert_eq!(small_bytes, reference_bytes(&small));
    assert_eq!(large_bytes, reference_bytes(&large));
    assert!(!small_ledger.overflow && !large_ledger.overflow);
    let per_block = large_ledger
        .allocation_events
        .saturating_sub(small_ledger.allocation_events)
        / (large_blocks - small_blocks);
    assert!(
        per_block <= MAX_EVENTS_PER_BLOCK,
        "{per_block} allocation events per passthrough block (small {}, large {})",
        small_ledger.allocation_events,
        large_ledger.allocation_events
    );
}

#[test]
fn canonical_miss_return_buffer_provenance_is_classified() {
    for population in populations() {
        let label = population.label();
        let message = population.build();
        let reference = reference_bytes(&message);
        assert_eq!(
            declaration_order_equals_canonical(&message, &reference),
            population.expects_canonical_miss(),
            "{label}: canonicality oracle disagrees with the declared population"
        );
        let (bytes, ledger) = canonicalize_recorded(&message);
        assert_eq!(bytes, reference, "{label}");
        assert!(!ledger.overflow, "{label}: ledger overflow");
        let ptr = bytes.as_ptr() as usize;
        assert_eq!(
            ledger.live_bytes_at_close,
            bytes.capacity() as isize,
            "{label}: only the returned buffer survives the canonicalizer"
        );
        let provenance = ledger.buffer_provenance(ptr, bytes.len(), bytes.capacity());
        let expected = if population.expects_serialization_buffer_return() {
            BufferProvenance::GrowthChain
        } else {
            BufferProvenance::FreshExactSizeAllocation
        };
        assert_eq!(provenance, expected, "{label}: return-buffer provenance");
        match provenance {
            BufferProvenance::FreshExactSizeAllocation => {
                assert_eq!(
                    ledger
                        .allocations_of_size(bytes.len())
                        .iter()
                        .filter(|event| matches!(event, Event::Alloc { .. }))
                        .count(),
                    1,
                    "{label}: exactly one exact output-sized allocation"
                );
                assert!(
                    ledger.was_released(serialization_buffer(&ledger, bytes.len(), ptr)),
                    "{label}: the serialization buffer is released after the copy"
                );
            }
            BufferProvenance::GrowthChain => {
                let chain = ledger.growth_chain(ptr);
                assert!(
                    ledger
                        .grown_from_root_outside(
                            serialization_buffer_root_bytes(),
                            bytes.len(),
                            ptr
                        )
                        .is_empty(),
                    "{label}: no second serialization buffer reaches N outside the returned chain"
                );
                assert!(
                    ledger
                        .allocations_of_size(bytes.len())
                        .iter()
                        .all(|event| chain.contains(event)),
                    "{label}: no exact-size reorder buffer beside the returned chain"
                );
            }
            BufferProvenance::Unattributed => unreachable!("asserted above"),
        }
        drop(bytes);
    }
}

/// A leaked serialization buffer remains unreleased when a later allocation of
/// at least `N` bytes is freed.
#[test]
fn serialization_buffer_oracle_sees_a_leaked_serialization_buffer() {
    const N: usize = 200;
    let (returned, ledger) = record_window(|| {
        let mut objects: Vec<[usize; 5]> = Vec::with_capacity(4);
        objects.push([0; 5]);
        let mut a: Vec<u8> = Vec::new();
        a.extend_from_slice(b"{");
        a.extend(std::iter::repeat_n(b'x', N - 1));
        objects.extend(std::iter::repeat_n([0; 5], 8));
        drop(objects);
        let b = a.clone();
        std::mem::forget(a);
        b
    });
    let ptr = returned.as_ptr() as usize;
    assert_eq!(
        ledger.buffer_provenance(ptr, returned.len(), returned.capacity()),
        BufferProvenance::FreshExactSizeAllocation
    );
    let serialization_buffer = serialization_buffer(&ledger, returned.len(), ptr);
    assert!(
        !ledger.was_released(serialization_buffer),
        "the leaked serialization buffer must not read as released"
    );
    drop(returned);
}

fn serialization_buffer(ledger: &Ledger, output_len: usize, returned_ptr: usize) -> usize {
    let root = serialization_buffer_root_bytes();
    match ledger.grown_from_root_outside(root, output_len, returned_ptr)[..] {
        [buffer] => buffer,
        ref found => panic!(
            "expected one serialization buffer rooted at {root} bytes reaching {output_len}, found {found:?}"
        ),
    }
}

fn largest_receipt_peak(population: &served_output_fixtures::Population) -> usize {
    population
        .build()
        .content()
        .iter()
        .map(|block| {
            let (serialized, ledger) =
                record_window(|| daemon::served_json::canonical_block_bytes_for_test(block));
            assert!(!ledger.overflow, "receipt ledger overflow");
            assert_eq!(
                ledger.live_bytes_at_close,
                serialized.capacity() as isize,
                "only the receipt string remains live"
            );
            ledger.peak_live_bytes
        })
        .max()
        .unwrap_or(0)
}

fn message_blocks(population: &served_output_fixtures::Population) -> usize {
    population.build().content().len()
}

/// `Arc<[T]>` and `Arc<str>` store strong and weak counts before the payload
/// and pad the layout to word alignment.
fn arc_layout(payload_bytes: usize) -> usize {
    (payload_bytes + ARC_HEADER_BYTES).next_multiple_of(ARC_HEADER_BYTES / 2)
}

#[test]
fn full_constructor_observation_covers_receipts_hashing_and_arc_conversion() {
    for population in populations() {
        let label = population.label();
        let message = population.build();
        let reference = reference_bytes(&message);
        let arc_size = arc_layout(reference.len());
        let (canonical, canonicalizer) = canonicalize_recorded(&message);
        let canonicalizer_peak = canonicalizer.peak_live_bytes;
        let returned_capacity = canonical.capacity();
        drop(canonical);
        let (served, ledger) =
            record_window(|| daemon::transform::served_message_for_test(message));
        assert!(!ledger.overflow, "{label}: ledger overflow");
        assert_eq!(
            served.canonical_bytes_for_test(),
            reference.as_slice(),
            "{label}"
        );
        assert!(
            ledger.peak_live_bytes >= reference.len(),
            "{label}: peak {} below output length {}",
            ledger.peak_live_bytes,
            reference.len()
        );
        let blocks = message_blocks(&population);
        let message_arc = arc_layout(std::mem::size_of::<WireMessage>());
        let fingerprint_vec = blocks * std::mem::size_of::<(String, usize)>();
        let fingerprint_arc = arc_layout(fingerprint_vec);
        let digests = blocks * HEX_DIGEST_BYTES;
        let identity_arc = arc_layout(HEX_DIGEST_BYTES);
        let retained = message_arc + fingerprint_arc + digests + identity_arc + arc_size;
        assert_eq!(
            ledger.live_bytes_at_close, retained as isize,
            "{label}: the constructor retains the message, fingerprints, identity, and payload"
        );
        // The constructor converts the returned buffer to its exact-size `Arc`
        // before serializing receipts, so the buffer's spare capacity and the
        // `Arc` overlap only during that conversion.
        let conversion_peak = returned_capacity + arc_size;
        let live_after_conversion = arc_size + fingerprint_vec + digests;
        let hashing_peak = live_after_conversion + largest_receipt_peak(&population);
        // Ownership transfer: the identity string and its `Arc` overlap, then the
        // fingerprint `Vec` and its `Arc` overlap while the identity `Arc` is live.
        let ownership_peak = live_after_conversion
            + message_arc
            + identity_arc
            + HEX_DIGEST_BYTES.max(fingerprint_arc);
        let bound = canonicalizer_peak
            .max(conversion_peak)
            .max(hashing_peak)
            .max(ownership_peak);
        assert!(
            ledger.peak_live_bytes <= bound,
            "{label}: constructor peak {} exceeds max(canonicalizer {}, conversion {}, hashing {}, ownership {})",
            ledger.peak_live_bytes,
            canonicalizer_peak,
            conversion_peak,
            hashing_peak,
            ownership_peak
        );
        // `Arc<[u8]>` stores the strong and weak counts ahead of the bytes, so the
        // allocation starts two words before the payload pointer and its layout is
        // padded to word alignment.
        let arc_ptr = served.canonical_bytes_for_test().as_ptr() as usize - ARC_HEADER_BYTES;
        let arc_chain = ledger.growth_chain(arc_ptr);
        assert!(
            matches!(arc_chain.as_slice(), [Event::Alloc { size, .. }] if *size == arc_size),
            "{label}: the Arc payload is one exact allocation inside the window"
        );
        assert!(
            !ledger.was_released(arc_ptr),
            "{label}: the Arc payload stays live"
        );
        assert!(
            ledger.allocation_events > 1,
            "{label}: receipts and identity allocate beyond the payload"
        );
        drop(served);
    }
}

#[test]
fn recording_excludes_other_threads_and_tracks_growth_chains() {
    let stop = Arc::new(AtomicBool::new(false));
    let foreign_allocations = Arc::new(AtomicUsize::new(0));
    let noisy = {
        let stop = Arc::clone(&stop);
        let foreign_allocations = Arc::clone(&foreign_allocations);
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                kept.push(vec![0u8; 4096]);
                foreign_allocations.fetch_add(1, Ordering::Relaxed);
                if kept.len() > 64 {
                    kept.clear();
                }
            }
        })
    };
    while foreign_allocations.load(Ordering::Relaxed) == 0 {
        std::thread::yield_now();
    }
    let (grown, foreign_during_window, ledger) = {
        let ((grown, foreign_during_window), ledger) = record_window(|| {
            let before = foreign_allocations.load(Ordering::Relaxed);
            let mut grown: Vec<u8> = Vec::with_capacity(8);
            grown.extend(std::iter::repeat_n(7u8, 100));
            let scratch = vec![1u8; 17];
            drop(scratch);
            while foreign_allocations.load(Ordering::Relaxed) < before + 16 {
                std::thread::yield_now();
            }
            (grown, foreign_allocations.load(Ordering::Relaxed) - before)
        });
        (grown, foreign_during_window, ledger)
    };
    stop.store(true, Ordering::Relaxed);
    noisy.join().unwrap();
    assert!(foreign_during_window >= 16);
    assert!(!ledger.overflow);
    let ptr = grown.as_ptr() as usize;
    let chain = ledger.growth_chain(ptr);
    assert!(matches!(chain.first(), Some(Event::Alloc { size: 8, .. })));
    assert!(chain.len() >= 2, "growing past capacity must realloc");
    assert_eq!(
        ledger.buffer_provenance(ptr, grown.len(), grown.capacity()),
        BufferProvenance::GrowthChain
    );
    assert_eq!(ledger.allocations_of_size(17).len(), 1);
    assert_eq!(ledger.dealloc_events, 1);
    assert!(
        ledger.allocations_of_size(4096).is_empty(),
        "the other thread's 4 KiB allocations must not enter the ledger"
    );
    assert_eq!(
        ledger.allocation_events,
        chain.len() + 1,
        "only the owner thread's allocations are recorded"
    );
    drop(grown);

    // Positive control: a 4 KiB allocation on the owner thread is recorded.
    let (owned, ledger) = record_window(|| vec![0u8; 4096]);
    assert_eq!(ledger.allocations_of_size(4096).len(), 1);
    drop(owned);

    // A shrunk buffer is not a growth chain.
    let (shrunk, ledger) = record_window(|| {
        let mut shrunk: Vec<u8> = Vec::with_capacity(64);
        shrunk.extend(std::iter::repeat_n(1u8, 10));
        shrunk.shrink_to_fit();
        shrunk
    });
    assert_eq!(
        ledger.buffer_provenance(shrunk.as_ptr() as usize, shrunk.len(), shrunk.capacity()),
        BufferProvenance::Unattributed,
        "a shrunk buffer is not a growth chain"
    );
    drop(shrunk);

    // A one-shot over-allocation never grew, so it is neither fresh-exact nor a chain.
    let (slack, ledger) = record_window(|| {
        let mut slack: Vec<u8> = Vec::with_capacity(64);
        slack.extend(std::iter::repeat_n(1u8, 10));
        slack
    });
    assert_eq!(
        ledger.buffer_provenance(slack.as_ptr() as usize, slack.len(), slack.capacity()),
        BufferProvenance::Unattributed,
        "a single over-allocated alloc is not a growth chain"
    );
    drop(slack);
}

#[test]
fn recorder_aggregates_follow_a_scripted_sequence_and_report_overflow() {
    let ((first, third), ledger) = record_window(|| {
        let first: Vec<u8> = Vec::with_capacity(100);
        let second: Vec<u8> = Vec::with_capacity(50);
        let mut third: Vec<u8> = Vec::with_capacity(100);
        third.reserve_exact(200);
        drop(second);
        (first, third)
    });
    assert_eq!(ledger.allocation_events, 4, "three allocs and one realloc");
    assert_eq!(ledger.realloc_events, 1);
    assert_eq!(ledger.dealloc_events, 1);
    assert_eq!(ledger.requested_bytes, 100 + 50 + 100 + 200);
    assert_eq!(ledger.peak_live_bytes, 100 + 50 + 200);
    assert_eq!(ledger.live_bytes_at_close, 300);
    assert!(!ledger.overflow);
    drop((first, third));

    // Freeing memory that predates the window drives the live delta negative.
    let before: Vec<u8> = Vec::with_capacity(4096);
    let ((), ledger) = record_window(|| drop(before));
    assert_eq!(ledger.live_bytes_at_close, -4096);
    assert_eq!(ledger.peak_live_bytes, 0);

    // More events than the ledger holds: aggregates stay exact, events truncate.
    // `boxes` is preallocated outside the window, so only its `Box<u8>` allocations are recorded.
    let mut boxes: Vec<Box<u8>> = Vec::with_capacity(70_000);
    let ((), ledger) = record_window(|| boxes.extend((0..70_000).map(|_| Box::new(1u8))));
    assert!(ledger.overflow);
    assert_eq!(ledger.requested_bytes, 70_000);
    assert_eq!(ledger.peak_live_bytes, 70_000);
    assert_eq!(ledger.live_bytes_at_close, 70_000);
    assert!(ledger.events.len() < 70_000);
    drop(boxes);
}
