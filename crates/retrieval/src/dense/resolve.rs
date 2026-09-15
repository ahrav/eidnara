//! Resolves one base layer and its ordered deltas into the current occurrence set before anything is judged or scored.
//! Precedence is lexicographic `(base_epoch, delta_ordinal)`, newest first: a later layer's row or tombstone for an occurrence hides every older layer's row for it, so a tombstone masks and a replacement supersedes.
//! The order layers are handed in and the order rows sit in a layer decide nothing; only precedence does.
//! An occurrence a layer both lists and tombstones, or two layers of equal precedence, is a conflict the owners have not decided; the resolver refuses it rather than choosing.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use crate::batch::ProjectionCheckpoint;

/// Position of a layer in the newest-first order; the base is ordinal zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    pub rows: &'a [Vec<f32>],
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
    #[error("layer {index}'s checkpoint moves backwards from the layer before it")]
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
    let mut seen: BTreeMap<&str, Option<Winner>> = BTreeMap::new();
    let mut resolved = Resolved {
        winners: Vec::new(),
        superseded: 0,
        masked: 0,
    };
    for index in order.into_iter().rev() {
        let layer = &layers[index];
        for occurrence_id in layer.tombstones {
            seen.entry(occurrence_id).or_insert(None);
        }
        for (row, occurrence_id) in layer.occurrence_ids.iter().enumerate() {
            match seen.get(occurrence_id.as_str()) {
                Some(None) => resolved.masked += 1,
                Some(Some(_)) => resolved.superseded += 1,
                None => {
                    seen.insert(
                        occurrence_id,
                        Some(Winner {
                            occurrence_id: occurrence_id.clone(),
                            layer: index,
                            row,
                        }),
                    );
                }
            }
        }
    }
    resolved.winners = seen.into_values().flatten().collect();
    Ok(resolved)
}

/// Layer indexes oldest first: the base, then deltas by ascending ordinal.
fn check_topology(
    layers: &[Layer<'_>],
    max_entries: NonZeroUsize,
) -> Result<Vec<usize>, ResolveRefusal> {
    let mut entries = 0usize;
    for (index, layer) in layers.iter().enumerate() {
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
            if let Some(entry) =
                (1..ids.len()).find(|i| ids[*i - 1].as_bytes() >= ids[*i].as_bytes())
            {
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
        entries = entries.saturating_add(layer.occurrence_ids.len() + layer.tombstones.len());
        if entries > max_entries.get() {
            return Err(ResolveRefusal::OverBound {
                max: max_entries.get(),
            });
        }
    }
    let mut order: Vec<usize> = (0..layers.len()).collect();
    order.sort_by_key(|index| layers[*index].precedence);
    let mut bases = order
        .iter()
        .copied()
        .filter(|index| layers[*index].precedence.delta_ordinal == 0);
    let base = bases.next().ok_or(ResolveRefusal::NoBase)?;
    if let Some(second) = bases.next() {
        return Err(ResolveRefusal::MultipleBases {
            first: base,
            second,
        });
    }
    let base_epoch = layers[base].precedence.base_epoch;
    for pair in order.windows(2) {
        let (previous, index) = (pair[0], pair[1]);
        let precedence = layers[index].precedence;
        if precedence == layers[previous].precedence {
            return Err(ResolveRefusal::EqualPrecedence {
                first: previous,
                second: index,
            });
        }
        if precedence.base_epoch != base_epoch {
            return Err(ResolveRefusal::EpochMismatch {
                index,
                epoch: precedence.base_epoch,
                base_epoch,
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
