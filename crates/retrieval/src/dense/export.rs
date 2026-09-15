//! Reads the original rows one generation holds for its live dense-required occurrences, in occurrence identifier order, together with the checkpoint the same transaction sees, so two builds over the same projection state see the same rows and the same provenance.

use std::num::NonZeroUsize;

use rusqlite::params;
use storage::GuardedConn;

use super::codec::{self, RowLayout, RowRejection};
use crate::ProjectionError;
use crate::batch::{ProjectionCheckpoint, VectorGeneration, dense_eligible, read_checkpoint};
use kernel::source_identity::OccurrenceClass;

/// One live occurrence and its validated original row.
#[derive(Clone, PartialEq)]
pub struct ExportedRow {
    pub occurrence_id: String,
    pub vector: Vec<f32>,
}

impl std::fmt::Debug for ExportedRow {
    /// Rows are embedding content and stay out of diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportedRow")
            .field("occurrence_id", &self.occurrence_id)
            .finish()
    }
}

/// The rows and the checkpoint one read transaction observed together.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRows {
    pub checkpoint: ProjectionCheckpoint,
    pub rows: Vec<ExportedRow>,
    /// Occurrences the layer masks in every older layer, in identifier order; a full export of the live population masks none.
    pub tombstones: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExportRefusal {
    #[error("the layout's dimension {layout} is not the generation's {generation}")]
    LayoutMismatch { layout: u32, generation: u32 },
    #[error("the layout admits no generation: {0}")]
    Layout(RowRejection),
    #[error("no batch has committed under kernel {kernel_incarnation_id}")]
    NoCheckpoint { kernel_incarnation_id: String },
    #[error("more than {max} live rows carry a vector of the generation")]
    OverBound { max: usize },
    #[error("the stored vector of occurrence {occurrence_id}: {rejection}")]
    StoredRow {
        occurrence_id: String,
        rejection: RowRejection,
    },
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

/// Every live dense-required occurrence with a vector of `generation`, validated against `layout`, in identifier order, with the checkpoint under `kernel_incarnation_id`; a population above `max_rows` is refused whole rather than truncated.
///
/// # Errors
///
/// A stored row outside `layout` refuses the export, as the oracle refuses a request, because a generation with an invalid member is not the generation the caller asked about.
pub fn live_rows(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
    kernel_incarnation_id: &str,
    layout: &RowLayout,
    max_rows: NonZeroUsize,
) -> Result<LiveRows, ExportRefusal> {
    if layout.dimension != generation.vector_dimension {
        return Err(ExportRefusal::LayoutMismatch {
            layout: layout.dimension,
            generation: generation.vector_dimension,
        });
    }
    layout.check().map_err(ExportRefusal::Layout)?;
    crate::vectors::check_generation(conn, generation)?;
    let checkpoint = read_checkpoint(conn, kernel_incarnation_id)?.ok_or_else(|| {
        ExportRefusal::NoCheckpoint {
            kernel_incarnation_id: kernel_incarnation_id.to_owned(),
        }
    })?;
    let classes: Vec<String> = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect();
    let sql = format!(
        "SELECT o.occurrence_id,v.vector
         FROM occurrence_vectors v
         JOIN occurrences o ON o.occurrence_id=v.occurrence_id
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE v.generation_id=?1 AND t.occurrence_id IS NULL AND o.class IN ({})
         ORDER BY o.occurrence_id
         LIMIT ?2",
        classes.join(",")
    );
    let limit = i64::try_from(max_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(&sql).map_err(ProjectionError::from)?;
    let mut rows = statement
        .query(params![generation.generation_id, limit])
        .map_err(ProjectionError::from)?;
    let mut exported = Vec::new();
    while let Some(row) = rows.next().map_err(ProjectionError::from)? {
        if exported.len() == max_rows.get() {
            return Err(ExportRefusal::OverBound {
                max: max_rows.get(),
            });
        }
        let occurrence_id: String = row.get(0).map_err(ProjectionError::from)?;
        let bytes: Vec<u8> = row.get(1).map_err(ProjectionError::from)?;
        let vector =
            codec::decode(&bytes, layout).map_err(|rejection| ExportRefusal::StoredRow {
                occurrence_id: occurrence_id.clone(),
                rejection,
            })?;
        exported.push(ExportedRow {
            occurrence_id,
            vector,
        });
    }
    Ok(LiveRows {
        checkpoint,
        rows: exported,
        tombstones: Vec::new(),
    })
}
