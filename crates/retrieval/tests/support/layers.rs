//! A layer a test owns, so a `Layer` can borrow from it.

use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::{Layer, Precedence};

pub struct OwnedLayer {
    pub precedence: Precedence,
    pub checkpoint: ProjectionCheckpoint,
    pub ids: Vec<String>,
    pub rows: Vec<Vec<f32>>,
    pub tombstones: Vec<String>,
}

impl OwnedLayer {
    /// `rows` are `(identifier, vector)` and are sorted here, as are the tombstones; the checkpoint advances ten per ordinal so deltas follow their predecessors.
    pub fn new(
        epoch: u64,
        ordinal: u32,
        rows: Vec<(String, Vec<f32>)>,
        tombstones: Vec<String>,
    ) -> Self {
        let mut rows = rows;
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut tombstones = tombstones;
        tombstones.sort();
        let seq = 10 + i64::from(ordinal) * 10;
        Self {
            precedence: Precedence {
                base_epoch: epoch,
                delta_ordinal: ordinal,
            },
            checkpoint: ProjectionCheckpoint {
                snapshot_commit_seq: seq,
                checkpoint_commit_seq: seq,
                hold_id: "hold".to_owned(),
            },
            ids: rows.iter().map(|(id, _)| id.clone()).collect(),
            rows: rows.into_iter().map(|(_, vector)| vector).collect(),
            tombstones,
        }
    }

    pub fn layer(&self) -> Layer<'_> {
        Layer {
            precedence: self.precedence,
            checkpoint: &self.checkpoint,
            occurrence_ids: &self.ids,
            rows: &self.rows,
            tombstones: &self.tombstones,
        }
    }
}

pub fn layers(owned: &[OwnedLayer]) -> Vec<Layer<'_>> {
    owned.iter().map(OwnedLayer::layer).collect()
}
