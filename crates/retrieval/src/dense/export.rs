//! Reads the original rows one generation holds for its live dense-required occurrences, in occurrence identifier order, together with the checkpoint the same transaction sees, so two builds over the same projection state see the same rows and the same provenance.

use std::num::NonZeroUsize;
use std::sync::LazyLock;

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

/// One read transaction observes `generation`, `kernel_incarnation_id`, `checkpoint`, and `rows` together.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRows {
    pub generation: VectorGeneration,
    pub kernel_incarnation_id: String,
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
    #[error("no batch has committed a checkpoint with a hold under kernel {kernel_incarnation_id}")]
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

/// `ORDER BY` names `v.occurrence_id`, the second column of the index the `generation_id` search drives, because the planner does not carry the join equality to `o.occurrence_id` and adds a sort for that spelling.
static LIVE_ROWS_SQL: LazyLock<String> = LazyLock::new(|| {
    let classes: Vec<String> = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect();
    format!(
        "SELECT o.occurrence_id,v.vector
         FROM occurrence_vectors v
         JOIN occurrences o ON o.occurrence_id=v.occurrence_id
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE v.generation_id=?1 AND t.occurrence_id IS NULL AND o.class IN ({})
         ORDER BY v.occurrence_id
         LIMIT ?2",
        classes.join(",")
    )
});

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
    // The schema admits a NULL hold, read back as empty; `apply_batch` refuses an empty hold, so no batch committed that checkpoint and it is no provenance.
    let checkpoint = read_checkpoint(conn, kernel_incarnation_id)?
        .filter(|checkpoint| !checkpoint.hold_id.is_empty())
        .ok_or_else(|| ExportRefusal::NoCheckpoint {
            kernel_incarnation_id: kernel_incarnation_id.to_owned(),
        })?;
    let limit = i64::try_from(max_rows.get().saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn
        .prepare(&LIVE_ROWS_SQL)
        .map_err(ProjectionError::from)?;
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
        generation: generation.clone(),
        kernel_incarnation_id: kernel_incarnation_id.to_owned(),
        checkpoint,
        rows: exported,
        tombstones: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::LIVE_ROWS_SQL;
    use rusqlite::Connection;

    /// The export must walk the generation's index in identifier order; a sort would copy every vector into a temporary tree and read the whole generation before the bound could refuse it.
    #[test]
    fn the_export_query_walks_the_generation_index_without_sorting() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::BASELINE).unwrap();
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", *LIVE_ROWS_SQL))
            .unwrap()
            .query_map(rusqlite::params!["gen", 3], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|detail| {
                detail.contains(
                    "SEARCH v USING INDEX idx_occurrence_vectors_generation (generation_id=?)",
                )
            }),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|detail| detail.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }
}
