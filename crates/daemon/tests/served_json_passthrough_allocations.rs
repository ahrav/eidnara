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
use std::sync::atomic::{AtomicBool, Ordering};

use alloc_recorder::{Event, Ledger, record_window};
use memory_store::WireMessage;
use served_output_fixtures::{Population, ReturnProvenance, populations};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

/// Independent byte oracle: sorted `Value` maps with unique keys give canonical bytes.
fn reference_bytes(message: &WireMessage) -> Vec<u8> {
    serde_json::to_vec(&serde_json::to_value(message).unwrap()).unwrap()
}

/// Independent canonicality oracle: declaration-order serialization already equals the
/// canonical bytes exactly when no object needs reordering.
fn is_canonical_miss(message: &WireMessage, canonical: &[u8]) -> bool {
    serde_json::to_vec(message).unwrap() == canonical
}

fn canonicalize_recorded(message: &WireMessage) -> (Vec<u8>, Ledger) {
    record_window(|| daemon::served_json::canonical_served_bytes_for_test(message))
}

/// Each block has 12 keys; a cap below 12 rejects one allocation per decoded key.
const MAX_EVENTS_PER_BLOCK: usize = 8;

#[test]
fn passthrough_shell_canonicalization_allocates_independently_of_key_count() {
    let small = served_output_fixtures::retained_ascii_message(1);
    let large = served_output_fixtures::retained_ascii_message(65);
    let (small_bytes, small_ledger) = canonicalize_recorded(&small);
    let (large_bytes, large_ledger) = canonicalize_recorded(&large);
    assert_eq!(small_bytes, reference_bytes(&small));
    assert_eq!(large_bytes, reference_bytes(&large));
    assert!(!small_ledger.overflow && !large_ledger.overflow);
    let per_block = (large_ledger.allocation_events - small_ledger.allocation_events) / 64;
    assert!(
        per_block <= MAX_EVENTS_PER_BLOCK,
        "{per_block} allocation events per passthrough block (small {}, large {})",
        small_ledger.allocation_events,
        large_ledger.allocation_events
    );
}

fn classify_return(ledger: &Ledger, returned: &[u8], capacity: usize) -> ReturnProvenance {
    let ptr = returned.as_ptr() as usize;
    let chain = ledger.growth_chain(ptr);
    assert!(
        !chain.is_empty(),
        "the returned buffer must be produced inside the recording window"
    );
    assert!(
        !ledger.was_released(ptr),
        "the returned buffer must stay live through encoder return"
    );
    match chain.as_slice() {
        [Event::Alloc { size, .. }] if *size == returned.len() && capacity == returned.len() => {
            ReturnProvenance::FreshExactSizeAllocation
        }
        _ => ReturnProvenance::SerializationGrowthChain,
    }
}

#[test]
fn canonical_miss_return_buffer_provenance_is_classified() {
    for population in populations() {
        let message = population.build();
        let reference = reference_bytes(&message);
        let canonical_miss = is_canonical_miss(&message, &reference);
        assert_eq!(
            canonical_miss,
            population.expects_canonical_miss(),
            "{}: canonicality oracle disagrees with the declared population",
            population.label()
        );
        let (bytes, ledger) = canonicalize_recorded(&message);
        assert_eq!(bytes, reference, "{}", population.label());
        assert!(!ledger.overflow, "{}: ledger overflow", population.label());
        let capacity = bytes.capacity();
        let provenance = classify_return(&ledger, &bytes, capacity);
        assert_eq!(
            provenance,
            population.expected_return_provenance(),
            "{}: return-buffer provenance",
            population.label()
        );
        let output_sized = ledger.allocations_of_size(bytes.len());
        match provenance {
            ReturnProvenance::FreshExactSizeAllocation => {
                assert!(
                    output_sized
                        .iter()
                        .any(|event| matches!(event, Event::Alloc { ptr, .. } if *ptr == bytes.as_ptr() as usize)),
                    "{}: B must be an exact output-sized allocation",
                    population.label()
                );
                assert!(
                    ledger.dealloc_events >= 1,
                    "{}: A must be released after copying into B",
                    population.label()
                );
            }
            ReturnProvenance::SerializationGrowthChain => {
                let fresh_exact = output_sized.iter().filter(|event| {
                    matches!(event, Event::Alloc { ptr, .. } if ledger.growth_chain(*ptr).len() == 1 && *ptr != bytes.as_ptr() as usize)
                });
                assert_eq!(
                    fresh_exact.count(),
                    0,
                    "{}: no replacement output-sized scratch may exist",
                    population.label()
                );
            }
        }
        drop(bytes);
    }
}

#[test]
fn full_constructor_observation_covers_receipts_hashing_and_arc_conversion() {
    for population in populations().filter(Population::observes_full_constructor) {
        let message = population.build();
        let reference = reference_bytes(&message);
        let (served, ledger) =
            record_window(|| daemon::transform::served_message_for_test(message));
        assert!(!ledger.overflow, "{}: ledger overflow", population.label());
        assert_eq!(
            served.canonical_bytes_for_test(),
            reference.as_slice(),
            "{}",
            population.label()
        );
        assert!(
            ledger.peak_live_bytes >= reference.len(),
            "{}: peak {} below output length {}",
            population.label(),
            ledger.peak_live_bytes,
            reference.len()
        );
        assert!(
            ledger
                .allocations_of_size(reference.len())
                .iter()
                .any(|event| matches!(event, Event::Alloc { .. })),
            "{}: the Arc conversion allocates an exact output-sized payload",
            population.label()
        );
        drop(served);
    }
}

#[test]
fn recording_excludes_other_threads_and_tracks_growth_chains() {
    let stop = Arc::new(AtomicBool::new(false));
    let noisy = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                kept.push(vec![0u8; 4096]);
                if kept.len() > 64 {
                    kept.clear();
                }
            }
        })
    };
    let (grown, ledger) = record_window(|| {
        let mut grown: Vec<u8> = Vec::with_capacity(8);
        grown.extend(std::iter::repeat_n(7u8, 100));
        let scratch = vec![1u8; 17];
        drop(scratch);
        grown
    });
    stop.store(true, Ordering::Relaxed);
    noisy.join().unwrap();
    assert!(!ledger.overflow);
    let chain = ledger.growth_chain(grown.as_ptr() as usize);
    assert!(matches!(chain.first(), Some(Event::Alloc { size: 8, .. })));
    assert!(chain.len() >= 2, "growing past capacity must realloc");
    assert!(!ledger.was_released(grown.as_ptr() as usize));
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
}
