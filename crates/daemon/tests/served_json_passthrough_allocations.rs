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
    reference_bytes,
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
                // The serialization buffer is the last buffer outside the returned
                // chain that grew to at least N; it must be released before return.
                let serialization_buffer = ledger
                    .allocations_at_least_outside(bytes.len(), ptr)
                    .last()
                    .map(|event| match event {
                        Event::Alloc { ptr, .. } | Event::Realloc { new_ptr: ptr, .. } => *ptr,
                        Event::Dealloc { .. } => unreachable!(),
                    })
                    .expect("the copy path grows a serialization buffer to N");
                assert!(
                    ledger.was_released(serialization_buffer),
                    "{label}: the serialization buffer is released after the copy"
                );
            }
            BufferProvenance::GrowthChain => {
                assert!(
                    ledger
                        .allocations_at_least_outside(bytes.len(), ptr)
                        .is_empty(),
                    "{label}: no output-sized storage outside the returned chain"
                );
                assert!(
                    ledger.allocations_of_size(bytes.len()).is_empty()
                        || bytes.capacity() == bytes.len(),
                    "{label}: no exact-size reorder buffer beside the returned chain"
                );
            }
            BufferProvenance::Unattributed => unreachable!("asserted above"),
        }
        drop(bytes);
    }
}

/// Bytes of the per-block receipt strings the no-projection constructor serializes.
fn message_receipt_bytes(population: &served_output_fixtures::Population) -> usize {
    population
        .build()
        .content()
        .iter()
        .map(|block| {
            serde_json::to_string(block)
                .expect("block serializes")
                .len()
        })
        .sum()
}

#[test]
fn full_constructor_observation_covers_receipts_hashing_and_arc_conversion() {
    for population in populations() {
        let label = population.label();
        let message = population.build();
        let reference = reference_bytes(&message);
        let arc_size = (reference.len() + ARC_HEADER_BYTES).next_multiple_of(ARC_HEADER_BYTES / 2);
        let (_, canonicalizer) = canonicalize_recorded(&message);
        let canonicalizer_peak = canonicalizer.peak_live_bytes;
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
        // Beyond the canonicalizer's own peak, the constructor can only add the Arc
        // payload, one serialized receipt string per block, and the identity string.
        let receipts: usize = message_receipt_bytes(&population);
        let bound = canonicalizer_peak + arc_size + receipts + 64 + 1024;
        assert!(
            ledger.peak_live_bytes <= bound,
            "{label}: constructor peak {} exceeds canonicalizer peak {} plus receipts {} and Arc {}",
            ledger.peak_live_bytes,
            canonicalizer_peak,
            receipts,
            arc_size
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
        std::hint::spin_loop();
    }
    let (grown, foreign_during_window, ledger) = {
        let before = foreign_allocations.load(Ordering::Relaxed);
        let ((grown, seen), ledger) = record_window(|| {
            let mut grown: Vec<u8> = Vec::with_capacity(8);
            grown.extend(std::iter::repeat_n(7u8, 100));
            let scratch = vec![1u8; 17];
            drop(scratch);
            // Spin until the other thread has allocated at least 16 times inside
            // this window, so the exclusion below is exercised rather than assumed.
            while foreign_allocations.load(Ordering::Relaxed) < before + 16 {
                std::hint::spin_loop();
            }
            (grown, foreign_allocations.load(Ordering::Relaxed))
        });
        (grown, seen - before, ledger)
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
    let (boxes, ledger) = record_window(|| (0..70_000).map(|_| Box::new(1u8)).collect::<Vec<_>>());
    assert!(ledger.overflow);
    assert!(ledger.requested_bytes >= 70_000);
    assert!(ledger.peak_live_bytes >= 70_000);
    assert!(ledger.events.len() < 70_000);
    drop(boxes);
}
