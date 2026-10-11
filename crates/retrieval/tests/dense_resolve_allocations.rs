//! Allocator evidence for the layer resolver: the resolver retains winner storage and releases the scratch it sorts superseded and masked rows in before returning.
//!
//! Every bound compares one shape against another on the same host.

/// The daemon's allocation recorder: it records only on the thread that opened a window, so the other tests in this binary run unrecorded.
#[path = "../../daemon/tests/support/alloc_recorder.rs"]
#[allow(dead_code)]
mod alloc_recorder;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

mod support;

use std::num::NonZeroUsize;

use retrieval::dense::{Winner, resolve};
use support::layers::{OwnedLayer, layers};

const WINNERS: usize = 10_000;
const DELTAS: u32 = 4;

fn id(index: usize) -> String {
    format!("occurrence-{index:06}")
}

fn layer(ordinal: u32, rows: impl IntoIterator<Item = usize>, tombstones: &[usize]) -> OwnedLayer {
    OwnedLayer::new(
        1,
        ordinal,
        rows.into_iter()
            .map(|index| (id(index), vec![index as f32]))
            .collect(),
        tombstones.iter().copied().map(id).collect(),
    )
}

/// `WINNERS` base rows and nothing else.
fn base_only() -> Vec<OwnedLayer> {
    vec![layer(0, 0..WINNERS, &[])]
}

/// The same base, with `DELTAS` deltas that together replace every base row once.
fn every_row_superseded() -> Vec<OwnedLayer> {
    let share = WINNERS / DELTAS as usize;
    let mut owned = base_only();
    for ordinal in 1..=DELTAS {
        let start = (ordinal as usize - 1) * share;
        owned.push(layer(ordinal, start..start + share, &[]));
    }
    owned
}

/// A base twice as large, with `DELTAS` deltas whose tombstones mask its second half.
fn half_the_base_masked() -> Vec<OwnedLayer> {
    let share = WINNERS / DELTAS as usize;
    let mut owned = vec![layer(0, 0..2 * WINNERS, &[])];
    for ordinal in 1..=DELTAS {
        let start = WINNERS + (ordinal as usize - 1) * share;
        let tombstones: Vec<usize> = (start..start + share).collect();
        owned.push(layer(ordinal, [], &tombstones));
    }
    owned
}

fn entries(owned: &[OwnedLayer]) -> usize {
    owned
        .iter()
        .map(|layer| layer.ids.len() + layer.tombstones.len())
        .sum()
}

struct Measured {
    entries: usize,
    winners: usize,
    winner_capacity: usize,
    superseded: usize,
    masked: usize,
    ledger: alloc_recorder::Ledger,
}

fn measure(owned: &[OwnedLayer]) -> Measured {
    let views = layers(owned);
    let max = NonZeroUsize::new(1 << 20).unwrap();
    let (resolved, ledger) = alloc_recorder::record_window(|| resolve(&views, max).unwrap());
    assert!(!ledger.overflow, "the window fit the recorder");
    Measured {
        entries: entries(owned),
        winners: resolved.winners.len(),
        winner_capacity: resolved.winners.capacity(),
        superseded: resolved.superseded,
        masked: resolved.masked,
        ledger,
    }
}

#[test]
fn retained_memory_follows_winners_and_hidden_rows_add_only_scratch() {
    let base = measure(&base_only());
    let superseded = measure(&every_row_superseded());
    let masked = measure(&half_the_base_masked());

    for (shape, measured, hidden) in [
        ("base only", &base, (0, 0)),
        ("superseded", &superseded, (WINNERS, 0)),
        ("masked", &masked, (0, WINNERS)),
    ] {
        assert_eq!(measured.winners, WINNERS, "{shape}");
        assert_eq!((measured.superseded, measured.masked), hidden, "{shape}");
        assert_eq!(
            measured.ledger.live_bytes_at_close,
            (measured.winner_capacity * size_of::<Winner>()) as isize,
            "{shape}: the winner list is the only allocation that outlives the call"
        );
        // Equal winner counts retain equal bytes, however many rows the deltas hide.
        assert_eq!(
            measured.ledger.live_bytes_at_close, base.ledger.live_bytes_at_close,
            "{shape}"
        );
    }

    for (shape, measured, factor) in [("superseded", &superseded, 2), ("masked", &masked, 3)] {
        assert_eq!(measured.entries, factor * base.entries, "{shape}");
        // Two or three times the entries cost at most four more allocator calls: hidden rows do not allocate per row.
        assert!(
            measured.ledger.allocation_events <= base.ledger.allocation_events + 4,
            "{shape}: {} allocation events against {} for the base alone",
            measured.ledger.allocation_events,
            base.ledger.allocation_events
        );
        // Peak bytes per entry stay within twice the base-only figure, so scratch is linear in the entries handed in.
        assert!(
            measured.ledger.peak_live_bytes * base.entries
                <= 2 * base.ledger.peak_live_bytes * measured.entries,
            "{shape}: peak {} bytes over {} entries against {} over {}",
            measured.ledger.peak_live_bytes,
            measured.entries,
            base.ledger.peak_live_bytes,
            base.entries
        );
    }
}

#[test]
fn retained_memory_grows_with_the_winner_count() {
    let small = measure(&base_only());
    let large = measure(&[layer(0, 0..4 * WINNERS, &[])]);
    assert_eq!(large.winners, 4 * WINNERS);
    for measured in [&small, &large] {
        let per_winner = measured.ledger.live_bytes_at_close as f64
            / (measured.winners * size_of::<Winner>()) as f64;
        assert!((1.0..2.0).contains(&per_winner), "{per_winner}");
    }
    assert!(large.ledger.live_bytes_at_close > 2 * small.ledger.live_bytes_at_close);
}
