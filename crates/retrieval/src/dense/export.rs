//! Reads the original rows one generation holds for its live dense-required occurrences, in occurrence identifier order, so two builds over the same projection state see the same rows in the same order.

use std::num::NonZeroUsize;

use rusqlite::params;
use storage::GuardedConn;

use super::codec::{self, RowLayout, RowRejection};
use crate::ProjectionError;
use crate::batch::{VectorGeneration, dense_eligible};
use kernel::source_identity::OccurrenceClass;

/// One live occurrence and its validated original row.
#[derive(Clone, PartialEq)]
pub struct ExportedRow {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    pub vector: Vec<f32>,
}

impl std::fmt::Debug for ExportedRow {
    /// Rows are embedding content and stay out of diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportedRow")
            .field("occurrence_id", &self.occurrence_id)
            .field("class", &self.class)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExportRefusal {
    #[error("the layout's dimension {layout} is not the generation's {generation}")]
    LayoutMismatch { layout: u32, generation: u32 },
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

/// Every live dense-required occurrence with a vector of `generation`, validated against `layout`, in identifier order; a population above `max_rows` is refused whole rather than truncated.
///
/// # Errors
///
/// A stored row outside `layout` refuses the export, as the oracle refuses a request, because a generation with an invalid member is not the generation the caller asked about.
pub fn live_rows(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
    layout: &RowLayout,
    max_rows: NonZeroUsize,
) -> Result<Vec<ExportedRow>, ExportRefusal> {
    if layout.dimension != generation.vector_dimension {
        return Err(ExportRefusal::LayoutMismatch {
            layout: layout.dimension,
            generation: generation.vector_dimension,
        });
    }
    layout
        .check()
        .map_err(|rejection| ExportRefusal::StoredRow {
            occurrence_id: String::new(),
            rejection,
        })?;
    crate::vectors::check_generation(conn, generation)?;
    let classes: Vec<String> = OccurrenceClass::ALL
        .into_iter()
        .filter(|class| dense_eligible(*class))
        .map(|class| format!("'{}'", class.code()))
        .collect();
    let sql = format!(
        "SELECT o.occurrence_id,o.class,v.vector
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
        let class =
            OccurrenceClass::from_code(&row.get::<_, String>(1).map_err(ProjectionError::from)?)
                .ok_or(ProjectionError::CorruptRow)?;
        let bytes: Vec<u8> = row.get(2).map_err(ProjectionError::from)?;
        let vector =
            codec::decode(&bytes, layout).map_err(|rejection| ExportRefusal::StoredRow {
                occurrence_id: occurrence_id.clone(),
                rejection,
            })?;
        exported.push(ExportedRow {
            occurrence_id,
            class,
            vector,
        });
    }
    Ok(exported)
}
