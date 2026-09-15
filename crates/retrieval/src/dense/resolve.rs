//! Resolves one base layer and its ordered deltas into the current occurrence set before anything is judged or scored.
//! Precedence is lexicographic `(base_epoch, delta_ordinal)`, newest first: a later layer's row or tombstone for an occurrence hides every older layer's row for it, so a tombstone masks and a replacement supersedes.
//! The order layers are handed in and the order rows sit in a layer decide nothing; only precedence does.
//! An occurrence a layer both lists and tombstones, or two layers of equal precedence, is a conflict the owners have not decided; the resolver refuses it rather than choosing.

use std::num::NonZeroUsize;

use super::codec::RowRejection;
use crate::batch::ProjectionCheckpoint;

/// Why a row source could not produce a row it names.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RowFault {
    #[error("the row is not a member of the generation: {0}")]
    Rejected(#[from] RowRejection),
    #[error("the row could not be read: {0}")]
    Unavailable(String),
}

/// A layer's rows by index. Resident rows answer from memory; a file-backed layer reads one row's bytes at its offset and decodes them, so only winners are ever read.
pub trait RowAccess {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The row at `index`, validated against the generation's layout by the caller.
    ///
    /// # Errors
    ///
    /// A row the source cannot produce, whether it fails the codec or cannot be read.
    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault>;
}

impl RowAccess for [Vec<f32>] {
    fn len(&self) -> usize {
        <[Vec<f32>]>::len(self)
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        self.get(index).cloned().ok_or_else(|| {
            RowFault::Unavailable(format!(
                "row {index} is past the {} resident rows",
                self.len()
            ))
        })
    }
}

impl RowAccess for Vec<Vec<f32>> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn row(&self, index: usize) -> Result<Vec<f32>, RowFault> {
        self.as_slice().row(index)
    }
}

/// Where a layer stands: every layer of one composition shares the base epoch, the base is ordinal zero, and a higher ordinal is newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Precedence {
    pub base_epoch: u64,
    pub delta_ordinal: u32,
}

/// One verified layer's contents, borrowed from whoever holds the files.
/// `occurrence_ids` and `rows` are parallel and in strictly increasing identifier byte order; `tombstones` is strictly increasing too.
pub struct Layer<'a> {
    pub precedence: Precedence,
    pub checkpoint: &'a ProjectionCheckpoint,
    pub occurrence_ids: &'a [String],
    pub rows: &'a dyn RowAccess,
    pub tombstones: &'a [String],
}

impl std::fmt::Debug for Layer<'_> {
    /// Rows are embedding content and stay out of diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("precedence", &self.precedence)
            .field("checkpoint", self.checkpoint)
            .field("rows", &self.occurrence_ids.len())
            .field("tombstones", &self.tombstones.len())
            .finish()
    }
}

/// `layer` indexes the caller's slice as handed in, not the precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Winner {
    pub occurrence_id: String,
    pub layer: usize,
    pub row: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// In occurrence identifier byte order.
    pub winners: Vec<Winner>,
    /// Older rows hidden by a newer row of the same occurrence.
    pub superseded: usize,
    /// Older rows hidden by a newer tombstone; a tombstone with nothing under it is not counted.
    pub masked: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveRefusal {
    #[error("no layer is the base")]
    NoBase,
    #[error("layers {first} and {second} are both bases")]
    MultipleBases { first: usize, second: usize },
    #[error("layer {index} carries base epoch {epoch}, not the base's {base_epoch}")]
    EpochMismatch {
        index: usize,
        epoch: u64,
        base_epoch: u64,
    },
    #[error("layers {first} and {second} share a precedence")]
    EqualPrecedence { first: usize, second: usize },
    #[error("layer {index} both lists and tombstones occurrence {occurrence_id}")]
    ListedAndTombstoned { index: usize, occurrence_id: String },
    #[error(
        "layer {index}'s checkpoint moves backwards, from the layer before it or within itself"
    )]
    CheckpointOrder { index: usize },
    #[error("layer {index} names {ids} occurrences but holds {rows} rows")]
    IncompleteView {
        index: usize,
        ids: usize,
        rows: usize,
    },
    #[error(
        "layer {index}'s {list} are not in strictly increasing identifier order at entry {entry}"
    )]
    Order {
        index: usize,
        list: &'static str,
        entry: usize,
    },
    #[error("the layers carry more than {max} rows and tombstones together")]
    OverBound { max: usize },
}

/// Resolves `layers` into the winners for at most `max_entries` rows and tombstones in total.
///
/// # Errors
///
/// Every structural refusal is found before any winner is chosen; an empty result is a valid resolution of a base whose rows are all masked.
pub fn resolve(
    layers: &[Layer<'_>],
    max_entries: NonZeroUsize,
) -> Result<Resolved, ResolveRefusal> {
    let order = check_topology(layers, max_entries)?;
    // Every entry of every layer, keyed by identifier then by age, so one pass over the sorted entries sees each occurrence's entries together with the newest last.
    let mut entries: Vec<Entry<'_>> = Vec::new();
    for (age, index) in order.into_iter().enumerate() {
        let layer = &layers[index];
        entries.extend(
            layer
                .occurrence_ids
                .iter()
                .enumerate()
                .map(|(row, id)| Entry {
                    occurrence_id: id,
                    age,
                    layer: index,
                    row: Some(row),
                }),
        );
        entries.extend(layer.tombstones.iter().map(|id| Entry {
            occurrence_id: id,
            age,
            layer: index,
            row: None,
        }));
    }
    entries.sort_unstable_by(|a, b| {
        a.occurrence_id
            .as_bytes()
            .cmp(b.occurrence_id.as_bytes())
            .then(a.age.cmp(&b.age))
    });
    let mut resolved = Resolved {
        winners: Vec::new(),
        superseded: 0,
        masked: 0,
    };
    for group in entries.chunk_by(|a, b| a.occurrence_id == b.occurrence_id) {
        let newest = group.last().expect("a group has at least one entry");
        let older_rows = group[..group.len() - 1]
            .iter()
            .filter(|entry| entry.row.is_some())
            .count();
        match newest.row {
            Some(row) => {
                resolved.superseded += older_rows;
                resolved.winners.push(Winner {
                    occurrence_id: newest.occurrence_id.to_owned(),
                    layer: newest.layer,
                    row,
                });
            }
            None => resolved.masked += older_rows,
        }
    }
    Ok(resolved)
}

/// One row or tombstone of one layer; `age` is the layer's position oldest first.
struct Entry<'a> {
    occurrence_id: &'a str,
    age: usize,
    layer: usize,
    /// `None` for a tombstone.
    row: Option<usize>,
}

/// Layer indexes oldest first: the base, then deltas by ascending ordinal.
fn check_topology(
    layers: &[Layer<'_>],
    max_entries: NonZeroUsize,
) -> Result<Vec<usize>, ResolveRefusal> {
    let mut entries = 0usize;
    for (index, layer) in layers.iter().enumerate() {
        entries = entries.saturating_add(check_layer(index, layer)?);
        if entries > max_entries.get() {
            return Err(ResolveRefusal::OverBound {
                max: max_entries.get(),
            });
        }
    }
    order_layers(layers)
}

/// One layer's own consistency; returns the rows and tombstones it carries.
fn check_layer(index: usize, layer: &Layer<'_>) -> Result<usize, ResolveRefusal> {
    if layer.checkpoint.snapshot_commit_seq > layer.checkpoint.checkpoint_commit_seq {
        return Err(ResolveRefusal::CheckpointOrder { index });
    }
    if layer.occurrence_ids.len() != layer.rows.len() {
        return Err(ResolveRefusal::IncompleteView {
            index,
            ids: layer.occurrence_ids.len(),
            rows: layer.rows.len(),
        });
    }
    for (list, ids) in [
        ("occurrence identifiers", layer.occurrence_ids),
        ("tombstones", layer.tombstones),
    ] {
        if let Some(entry) = (1..ids.len()).find(|i| ids[*i - 1].as_bytes() >= ids[*i].as_bytes()) {
            return Err(ResolveRefusal::Order { index, list, entry });
        }
    }
    if let Some(occurrence_id) = layer
        .occurrence_ids
        .iter()
        .find(|id| layer.tombstones.binary_search(id).is_ok())
    {
        return Err(ResolveRefusal::ListedAndTombstoned {
            index,
            occurrence_id: occurrence_id.clone(),
        });
    }
    Ok(layer.occurrence_ids.len() + layer.tombstones.len())
}

/// The base is found and every epoch is checked against it before anything is ordered, so no layer's position can hide a foreign epoch; only then are the deltas ordered by ordinal.
fn order_layers(layers: &[Layer<'_>]) -> Result<Vec<usize>, ResolveRefusal> {
    let mut bases = (0..layers.len()).filter(|index| layers[*index].precedence.delta_ordinal == 0);
    let base = bases.next().ok_or(ResolveRefusal::NoBase)?;
    if let Some(second) = bases.next() {
        return Err(ResolveRefusal::MultipleBases {
            first: base,
            second,
        });
    }
    let base_epoch = layers[base].precedence.base_epoch;
    for (index, layer) in layers.iter().enumerate() {
        if layer.precedence.base_epoch != base_epoch {
            return Err(ResolveRefusal::EpochMismatch {
                index,
                epoch: layer.precedence.base_epoch,
                base_epoch,
            });
        }
    }
    let mut order: Vec<usize> = (0..layers.len()).collect();
    order.sort_by_key(|index| layers[*index].precedence.delta_ordinal);
    for pair in order.windows(2) {
        let (previous, index) = (pair[0], pair[1]);
        if layers[index].precedence.delta_ordinal == layers[previous].precedence.delta_ordinal {
            return Err(ResolveRefusal::EqualPrecedence {
                first: previous,
                second: index,
            });
        }
        let before = layers[previous].checkpoint;
        let own = layers[index].checkpoint;
        if own.snapshot_commit_seq < before.checkpoint_commit_seq
            || own.checkpoint_commit_seq < before.checkpoint_commit_seq
        {
            return Err(ResolveRefusal::CheckpointOrder { index });
        }
    }
    Ok(order)
}
