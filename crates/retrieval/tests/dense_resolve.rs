//! The resolver against hand-written layer maps: every expectation below is the map itself, never a resolver trace.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::{Layer, Precedence, ResolveRefusal, Resolved, Winner, resolve};

const MAX: usize = 4096;

fn checkpoint(snapshot: i64, checkpoint: i64) -> ProjectionCheckpoint {
    ProjectionCheckpoint {
        snapshot_commit_seq: snapshot,
        checkpoint_commit_seq: checkpoint,
        hold_id: "hold".to_owned(),
    }
}

/// A layer whose rows are one coordinate each, so a test can read a row's value as its identity.
struct Owned {
    precedence: Precedence,
    checkpoint: ProjectionCheckpoint,
    ids: Vec<String>,
    rows: Vec<Vec<f32>>,
    tombstones: Vec<String>,
}

impl Owned {
    fn new(ordinal: u32, rows: &[(&str, f32)], tombstones: &[&str]) -> Self {
        Self::at(1, ordinal, 10 + i64::from(ordinal) * 10, rows, tombstones)
    }

    fn at(epoch: u64, ordinal: u32, seq: i64, rows: &[(&str, f32)], tombstones: &[&str]) -> Self {
        Self {
            precedence: Precedence {
                base_epoch: epoch,
                delta_ordinal: ordinal,
            },
            checkpoint: checkpoint(seq, seq),
            ids: rows.iter().map(|(id, _)| (*id).to_owned()).collect(),
            rows: rows.iter().map(|(_, value)| vec![*value]).collect(),
            tombstones: tombstones.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn layer(&self) -> Layer<'_> {
        Layer {
            precedence: self.precedence,
            checkpoint: &self.checkpoint,
            occurrence_ids: &self.ids,
            rows: &self.rows,
            tombstones: &self.tombstones,
        }
    }
}

fn layers(owned: &[Owned]) -> Vec<Layer<'_>> {
    owned.iter().map(Owned::layer).collect()
}

fn run(owned: &[Owned]) -> Result<Resolved, ResolveRefusal> {
    resolve(&layers(owned), NonZeroUsize::new(MAX).unwrap())
}

/// The winners as `(occurrence, value of the winning row)`, so a test names the expected map without knowing layer or row numbers.
fn values(owned: &[Owned], resolved: &Resolved) -> Vec<(String, f32)> {
    resolved
        .winners
        .iter()
        .map(|winner| {
            (
                winner.occurrence_id.clone(),
                owned[winner.layer].rows[winner.row][0],
            )
        })
        .collect()
}

fn expect(pairs: &[(&str, f32)]) -> Vec<(String, f32)> {
    pairs.iter().map(|(id, v)| ((*id).to_owned(), *v)).collect()
}

#[test]
fn a_base_alone_resolves_to_its_own_rows_in_identifier_order() {
    let base = Owned::new(0, &[("a", 1.0), ("b", 2.0), ("c", 3.0)], &[]);
    let resolved = run(&[base]).unwrap();
    assert_eq!(
        resolved.winners,
        vec![
            Winner {
                occurrence_id: "a".to_owned(),
                layer: 0,
                row: 0
            },
            Winner {
                occurrence_id: "b".to_owned(),
                layer: 0,
                row: 1
            },
            Winner {
                occurrence_id: "c".to_owned(),
                layer: 0,
                row: 2
            },
        ]
    );
    assert_eq!((resolved.superseded, resolved.masked), (0, 0));
    let empty = Owned::new(0, &[], &[]);
    assert_eq!(run(&[empty]).unwrap().winners, Vec::<Winner>::new());
}

#[test]
fn a_newer_row_replaces_an_older_one_whatever_its_value_and_a_newer_tombstone_masks_every_older_row()
 {
    let base = Owned::new(0, &[("a", 9.0), ("b", 9.0), ("c", 9.0), ("d", 9.0)], &[]);
    // The delta's replacement for `a` is worse by value; value decides nothing.
    let first = Owned::new(1, &[("a", 1.0), ("e", 5.0)], &["b"]);
    // `b` comes back in a later delta after the tombstone: the newer row wins over the older tombstone.
    let second = Owned::new(2, &[("b", 2.0)], &["c", "e"]);
    let owned = [base, first, second];
    let resolved = run(&owned).unwrap();
    assert_eq!(
        values(&owned, &resolved),
        expect(&[("a", 1.0), ("b", 2.0), ("d", 9.0)])
    );
    // `a` once, `b` once by the second delta over the base row; `b`'s base row is superseded (the newer row, not the tombstone, is what hides it).
    assert_eq!(resolved.superseded, 2);
    // `c` in the base and `e` in the first delta are masked; `b`'s tombstone in the first delta hides nothing the second delta did not already win.
    assert_eq!(resolved.masked, 2);
}

#[test]
fn a_tombstone_over_nothing_and_a_tombstone_repeated_by_a_later_layer_change_no_winner() {
    let base = Owned::new(0, &[("a", 1.0)], &[]);
    let first = Owned::new(1, &[], &["a", "zz"]);
    let second = Owned::new(2, &[], &["a"]);
    let owned = [base, first, second];
    let resolved = run(&owned).unwrap();
    assert_eq!(resolved.winners, Vec::<Winner>::new());
    assert_eq!((resolved.superseded, resolved.masked), (0, 1));
}

#[test]
fn the_order_layers_are_handed_in_decides_nothing() {
    let base = Owned::new(0, &[("a", 9.0), ("b", 9.0), ("c", 9.0)], &[]);
    let first = Owned::new(1, &[("a", 1.0)], &["c"]);
    let second = Owned::new(2, &[("c", 2.0)], &["a"]);
    let forward = [base, first, second];
    let reference = values(&forward, &run(&forward).unwrap());
    assert_eq!(reference, expect(&[("b", 9.0), ("c", 2.0)]));
    let [base, first, second] = forward;
    for permutation in [
        [&second, &first, &base],
        [&first, &base, &second],
        [&second, &base, &first],
    ] {
        let owned: Vec<Layer<'_>> = permutation.iter().map(|owned| owned.layer()).collect();
        let resolved = resolve(&owned, NonZeroUsize::new(MAX).unwrap()).unwrap();
        let got: Vec<(String, f32)> = resolved
            .winners
            .iter()
            .map(|w| (w.occurrence_id.clone(), permutation[w.layer].rows[w.row][0]))
            .collect();
        assert_eq!(got, reference);
        // `c`'s base row is superseded by the second delta's row; both older rows of `a` are masked by its tombstone.
        assert_eq!((resolved.superseded, resolved.masked), (1, 2));
    }
}

#[test]
fn malformed_layer_sets_are_refused_whole() {
    let base = || Owned::new(0, &[("a", 1.0)], &[]);
    let delta = || Owned::new(1, &[("b", 1.0)], &[]);

    assert_eq!(run(&[]).unwrap_err(), ResolveRefusal::NoBase);
    assert_eq!(run(&[delta()]).unwrap_err(), ResolveRefusal::NoBase);
    let mut second_base = base();
    second_base.checkpoint = checkpoint(5, 5);
    second_base.precedence.base_epoch = 2;
    assert_eq!(
        run(&[delta(), base(), second_base]).unwrap_err(),
        ResolveRefusal::MultipleBases {
            first: 1,
            second: 2
        }
    );
    assert_eq!(
        run(&[base(), delta(), delta()]).unwrap_err(),
        ResolveRefusal::EqualPrecedence {
            first: 1,
            second: 2
        }
    );
    let mut other_epoch = delta();
    other_epoch.precedence.base_epoch = 2;
    assert_eq!(
        run(&[base(), other_epoch]).unwrap_err(),
        ResolveRefusal::EpochMismatch {
            index: 1,
            epoch: 2,
            base_epoch: 1
        }
    );
    let both = Owned::new(1, &[("b", 1.0), ("c", 1.0)], &["c"]);
    assert_eq!(
        run(&[base(), both]).unwrap_err(),
        ResolveRefusal::ListedAndTombstoned {
            index: 1,
            occurrence_id: "c".to_owned()
        }
    );
    let mut earlier = delta();
    earlier.checkpoint = checkpoint(0, 0);
    assert_eq!(
        run(&[base(), earlier]).unwrap_err(),
        ResolveRefusal::CheckpointOrder { index: 1 }
    );
    let mut snapshot_back = delta();
    snapshot_back.checkpoint = checkpoint(0, 10);
    assert_eq!(
        run(&[base(), snapshot_back]).unwrap_err(),
        ResolveRefusal::CheckpointOrder { index: 1 },
        "a delta's snapshot before the previous checkpoint is refused even when its own checkpoint is after it"
    );
    let mut inverted = base();
    inverted.checkpoint = checkpoint(3, 2);
    assert_eq!(
        run(&[inverted]).unwrap_err(),
        ResolveRefusal::CheckpointOrder { index: 0 }
    );
    let mut short = base();
    short.rows.pop();
    assert_eq!(
        run(&[short]).unwrap_err(),
        ResolveRefusal::IncompleteView {
            index: 0,
            ids: 1,
            rows: 0
        }
    );
    let unsorted = Owned::new(0, &[("b", 1.0), ("a", 1.0)], &[]);
    assert_eq!(
        run(&[unsorted]).unwrap_err(),
        ResolveRefusal::Order {
            index: 0,
            list: "occurrence identifiers",
            entry: 1
        }
    );
    let repeated = Owned::new(0, &[("a", 1.0), ("a", 2.0)], &[]);
    assert!(matches!(
        run(&[repeated]).unwrap_err(),
        ResolveRefusal::Order { index: 0, .. }
    ));
    let tombstones = Owned::new(1, &[], &["b", "b"]);
    assert_eq!(
        run(&[base(), tombstones]).unwrap_err(),
        ResolveRefusal::Order {
            index: 1,
            list: "tombstones",
            entry: 1
        }
    );
    // Three entries under a bound of three pass; a fourth is over it, counting rows and tombstones together.
    let wide = || Owned::new(1, &[("b", 1.0), ("c", 1.0)], &["d"]);
    assert_eq!(
        resolve(&layers(&[base(), wide()]), NonZeroUsize::new(3).unwrap()).unwrap_err(),
        ResolveRefusal::OverBound { max: 3 }
    );
    assert!(resolve(&layers(&[base(), wide()]), NonZeroUsize::new(4).unwrap()).is_ok());
}

/// `None` is a tombstone; otherwise the `(layer, row)` of a row.
type Entry = Option<(usize, usize)>;

/// The independent model: for every occurrence, the entry of highest precedence decides, a row winning and a tombstone masking.
fn model(
    entries: &[(u32, Vec<String>, Vec<String>)],
) -> (BTreeMap<String, (usize, usize)>, usize, usize) {
    let mut best: BTreeMap<&str, (u32, Entry)> = BTreeMap::new();
    for (index, (ordinal, ids, tombstones)) in entries.iter().enumerate() {
        for (row, id) in ids.iter().enumerate() {
            let entry = best.entry(id).or_insert((*ordinal, Some((index, row))));
            if *ordinal >= entry.0 {
                *entry = (*ordinal, Some((index, row)));
            }
        }
        for id in tombstones {
            let entry = best.entry(id).or_insert((*ordinal, None));
            if *ordinal >= entry.0 {
                *entry = (*ordinal, None);
            }
        }
    }
    let winners: BTreeMap<String, (usize, usize)> = best
        .iter()
        .filter_map(|(id, (_, w))| w.map(|w| ((*id).to_owned(), w)))
        .collect();
    let mut superseded = 0;
    let mut masked = 0;
    for (index, (_, ids, _)) in entries.iter().enumerate() {
        for (row, id) in ids.iter().enumerate() {
            match best[id.as_str()].1 {
                Some(winner) if winner == (index, row) => {}
                Some(_) => superseded += 1,
                None => masked += 1,
            }
        }
    }
    (winners, superseded, masked)
}

fn layer_strategy() -> impl Strategy<Value = (BTreeSet<u8>, BTreeSet<u8>)> {
    (
        prop::collection::btree_set(0u8..12, 0..6),
        prop::collection::btree_set(0u8..12, 0..4),
    )
        .prop_map(|(ids, tombstones)| {
            let tombstones: BTreeSet<u8> = tombstones.difference(&ids).copied().collect();
            (ids, tombstones)
        })
}

#[test]
fn any_layer_set_resolves_to_the_model_in_every_enumeration_order() {
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 256,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    );
    let strategy = (
        prop::collection::vec(layer_strategy(), 1..5),
        prop::collection::vec(any::<u8>(), 0..8),
    );
    runner
        .run(&strategy, |(specs, swaps)| {
            let entries: Vec<(u32, Vec<String>, Vec<String>)> = specs
                .iter()
                .enumerate()
                .map(|(ordinal, (ids, tombstones))| {
                    let name = |v: &u8| format!("{v:02}");
                    (
                        ordinal as u32,
                        ids.iter().map(name).collect(),
                        if ordinal == 0 {
                            Vec::new()
                        } else {
                            tombstones.iter().map(name).collect()
                        },
                    )
                })
                .collect();
            let (expected, superseded, masked) = model(&entries);
            let owned: Vec<Owned> = entries
                .iter()
                .map(|(ordinal, ids, tombstones)| {
                    let rows: Vec<(&str, f32)> = ids
                        .iter()
                        .map(|id| (id.as_str(), f32::from(*ordinal as u8)))
                        .collect();
                    let tombstones: Vec<&str> = tombstones.iter().map(String::as_str).collect();
                    Owned::new(*ordinal, &rows, &tombstones)
                })
                .collect();
            // Any enumeration order of the same layers.
            let mut order: Vec<usize> = (0..owned.len()).collect();
            for (i, swap) in swaps.iter().enumerate() {
                let len = order.len();
                order.swap(i % len, usize::from(*swap) % len);
            }
            let permuted: Vec<Layer<'_>> = order.iter().map(|i| owned[*i].layer()).collect();
            let resolved = resolve(&permuted, NonZeroUsize::new(MAX).unwrap()).unwrap();
            let got: BTreeMap<String, (usize, usize)> = resolved
                .winners
                .iter()
                .map(|w| (w.occurrence_id.clone(), (order[w.layer], w.row)))
                .collect();
            prop_assert_eq!(got, expected);
            prop_assert_eq!(resolved.superseded, superseded);
            prop_assert_eq!(resolved.masked, masked);
            let ids: Vec<&str> = resolved
                .winners
                .iter()
                .map(|w| w.occurrence_id.as_str())
                .collect();
            prop_assert!(ids.windows(2).all(|p| p[0] < p[1]));
            Ok(())
        })
        .unwrap();
}
