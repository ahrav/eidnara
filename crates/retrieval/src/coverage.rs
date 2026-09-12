//! One coherent coverage observation over the projection: for every source class, the live occurrences the lexical index holds, the subset that requires dense coverage, the durable vectors valid for the observed generation, the missing set `M = R − V`, its pending subset `P`, and `M − P`. Every count comes from one read transaction keyed by the projection identity, its checkpoint, and one vector generation, so no cell mixes denominators; when any of those is absent the observation is unavailable rather than partial. Counts describe coverage; they grant nothing and satisfy no required witness.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use rusqlite::params;
use storage::GuardedConn;

use crate::batch::{
    ProjectionCheckpoint, VectorGeneration, dense_eligible, read_checkpoint, registered_generation,
};
use crate::{ProjectionError, ProjectionIdentity, read_identity};

/// Why no coherent observation exists. Nothing partial is reported in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageUnavailable {
    /// No projection identity is installed.
    NoIdentity,
    /// The identity names another kernel than the caller's.
    ForeignKernel { kernel_incarnation_id: String },
    /// No batch has ever committed, so there is no prefix to describe.
    NoCheckpoint,
    /// The generation the report was asked for is not registered, or its identity disagrees with the projection's.
    GenerationMismatch,
    /// The generation is retired: completion refuses its vectors and no new work queues for it, so its open jobs stand for nothing.
    RetiredGeneration,
    /// A class holds more live occurrences than the observation may count.
    OverBound { class: OccurrenceClass, max: usize },
    /// Tombstones are never reclaimed for most classes, so a long-lived projection can exceed this bound with few live rows.
    TombstonedOverBound { class: OccurrenceClass, max: usize },
}

/// What the dense columns of one class mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenseDisposition {
    /// The class requires dense coverage; `missing`, `pending`, and `missing_without_pending` describe its state.
    Required,
    /// The class is indexed lexically only; no vector is required or counted.
    LexicalOnly,
}

/// One class's coverage at the observation's checkpoint and generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassCoverage {
    pub class: OccurrenceClass,
    pub dense: DenseDisposition,
    /// Live occurrences: exact lexical presence.
    pub lexical: usize,
    /// `R`: live occurrences that require a vector; zero for a lexical-only class.
    pub dense_required: usize,
    /// `V`: members of `R` with a durable vector of the observed generation at the generation's dimension. The payload an occurrence names is immutable and completion refuses a vector whose input is not that payload, so a vector row is a vector of the exact input.
    pub valid_vectors: usize,
    /// `M = R − V`.
    pub missing: usize,
    /// `P`: members of `M` with a current durable pending or admitted job for the observed generation.
    pub pending: usize,
    /// `M − P`: missing without any durable work behind it.
    pub missing_without_pending: usize,
    /// Tombstoned occurrences still present, which count nowhere above.
    pub tombstoned: usize,
}

impl ClassCoverage {
    /// A class with no live occurrence is a known-empty inventory, distinct from an unavailable one.
    pub fn is_known_empty(&self) -> bool {
        self.lexical == 0
    }
}

/// The observation every class was counted under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageReport {
    pub identity: ProjectionIdentity,
    pub checkpoint: ProjectionCheckpoint,
    pub generation: VectorGeneration,
    /// One entry per class in [`OccurrenceClass::ALL`] order.
    pub classes: Vec<ClassCoverage>,
}

impl CoverageReport {
    pub fn class(&self, class: OccurrenceClass) -> &ClassCoverage {
        self.classes
            .iter()
            .find(|coverage| coverage.class == class)
            .expect("every class is reported")
    }
}

/// Rows one class may hold before the observation refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageBounds {
    pub max_live_per_class: NonZeroUsize,
    pub max_tombstoned_per_class: NonZeroUsize,
}

impl CoverageBounds {
    /// Live occurrences across every class the bound admits.
    pub fn max_live(self) -> usize {
        self.max_live_per_class
            .get()
            .saturating_mul(OccurrenceClass::ALL.len())
    }
}

/// Observes coverage for every class at the projection's checkpoint under `generation`, inside the caller's read transaction.
///
/// # Errors
///
/// Returns [`CoverageUnavailable`] when the identity, checkpoint, or generation is absent or foreign, or a class exceeds `bounds`; [`ProjectionError`] when a statement fails.
pub fn observe(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    generation: &VectorGeneration,
    bounds: CoverageBounds,
) -> Result<Result<CoverageReport, CoverageUnavailable>, ProjectionError> {
    let Some(identity) = read_identity(conn)? else {
        return Ok(Err(CoverageUnavailable::NoIdentity));
    };
    if identity.kernel_incarnation_id != kernel_incarnation_id {
        return Ok(Err(CoverageUnavailable::ForeignKernel {
            kernel_incarnation_id: identity.kernel_incarnation_id,
        }));
    }
    let Some(checkpoint) = read_checkpoint(conn, kernel_incarnation_id)? else {
        return Ok(Err(CoverageUnavailable::NoCheckpoint));
    };
    match registered_generation(conn, generation)? {
        Some(registered) if !registered.identity_matches => {
            return Ok(Err(CoverageUnavailable::GenerationMismatch));
        }
        Some(registered) if registered.is_retired() => {
            return Ok(Err(CoverageUnavailable::RetiredGeneration));
        }
        Some(_) => {}
        None => return Ok(Err(CoverageUnavailable::GenerationMismatch)),
    }
    let mut classes = Vec::with_capacity(OccurrenceClass::ALL.len());
    for class in OccurrenceClass::ALL {
        match class_coverage(conn, class, generation, bounds)? {
            Ok(coverage) => classes.push(coverage),
            Err(unavailable) => return Ok(Err(unavailable)),
        }
    }
    Ok(Ok(CoverageReport {
        identity,
        checkpoint,
        generation: generation.clone(),
        classes,
    }))
}

/// A vector is valid when it belongs to the observed generation at that generation's dimension.
const VALID_VECTOR: &str = "EXISTS(SELECT 1 FROM occurrence_vectors v
    WHERE v.occurrence_id=o.occurrence_id AND v.generation_id=?1 AND v.vector_dimension=?2)";

/// Durable work that still stands for the observed generation.
const CURRENT_PENDING: &str = "EXISTS(SELECT 1 FROM embedding_jobs j
    WHERE j.occurrence_id=o.occurrence_id AND j.generation_id=?1 AND j.state IN ('pending','admitted'))";

fn class_coverage(
    conn: &GuardedConn<'_>,
    class: OccurrenceClass,
    generation: &VectorGeneration,
    bounds: CoverageBounds,
) -> Result<Result<ClassCoverage, CoverageUnavailable>, ProjectionError> {
    let dense = if dense_eligible(class) {
        DenseDisposition::Required
    } else {
        DenseDisposition::LexicalOnly
    };
    let max_live = bounds.max_live_per_class.get();
    let max_tombstoned = bounds.max_tombstoned_per_class.get();
    // The query reads at most `max_live + max_tombstoned + 1` rows. Reaching
    // that limit proves one bound is exceeded, and the exceeded bound within
    // the prefix names the refusal; a shorter walk covers the whole class.
    let walk_limit = max_live.saturating_add(max_tombstoned).saturating_add(1);
    let (walked, tombstoned): (i64, i64) = conn.query_row(
        "SELECT COUNT(*),SUM(walked.tombstoned) FROM (
             SELECT (t.occurrence_id IS NOT NULL) AS tombstoned
             FROM occurrences o LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
             WHERE o.class=?1 LIMIT ?2) walked",
        params![
            class.code(),
            i64::try_from(walk_limit).map_err(|_| ProjectionError::CorruptRow)?
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            ))
        },
    )?;
    let walked = usize::try_from(walked).map_err(|_| ProjectionError::CorruptRow)?;
    let tombstoned = usize::try_from(tombstoned).map_err(|_| ProjectionError::CorruptRow)?;
    let lexical = walked
        .checked_sub(tombstoned)
        .ok_or(ProjectionError::CorruptRow)?;
    if lexical > max_live {
        return Ok(Err(CoverageUnavailable::OverBound {
            class,
            max: max_live,
        }));
    }
    if tombstoned > max_tombstoned {
        return Ok(Err(CoverageUnavailable::TombstonedOverBound {
            class,
            max: max_tombstoned,
        }));
    }
    let mut coverage = ClassCoverage {
        class,
        dense,
        lexical,
        dense_required: 0,
        valid_vectors: 0,
        missing: 0,
        pending: 0,
        missing_without_pending: 0,
        tombstoned,
    };
    if dense == DenseDisposition::LexicalOnly {
        return Ok(Ok(coverage));
    }
    let (valid, pending_missing): (i64, i64) = conn.query_row(
        &format!(
            "SELECT SUM({VALID_VECTOR}),SUM(NOT {VALID_VECTOR} AND {CURRENT_PENDING})
             FROM occurrences o LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
             WHERE o.class=?3 AND t.occurrence_id IS NULL"
        ),
        params![
            generation.generation_id,
            i64::from(generation.vector_dimension),
            class.code()
        ],
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            ))
        },
    )?;
    coverage.dense_required = lexical;
    coverage.valid_vectors = usize::try_from(valid).map_err(|_| ProjectionError::CorruptRow)?;
    coverage.missing = lexical - coverage.valid_vectors;
    coverage.pending = usize::try_from(pending_missing).map_err(|_| ProjectionError::CorruptRow)?;
    coverage.missing_without_pending = coverage.missing - coverage.pending;
    Ok(Ok(coverage))
}

/// Returns the ids in `occurrence_ids` with a valid vector for `generation`. One point read per id, so a caller can split an observed live set without a class-wide scan.
///
/// # Errors
///
/// Returns the SQLite error when a probe fails.
pub fn covered_among<'a>(
    conn: &GuardedConn<'_>,
    generation: &VectorGeneration,
    occurrence_ids: impl IntoIterator<Item = &'a str>,
) -> Result<HashSet<String>, ProjectionError> {
    // A one-row `o` carrying the probed id lets `VALID_VECTOR` evaluate it unchanged.
    let mut probe = conn.prepare_cached(&format!(
        "SELECT {VALID_VECTOR} FROM (SELECT ?3 AS occurrence_id) o"
    ))?;
    let mut covered = HashSet::new();
    for occurrence_id in occurrence_ids {
        let has_vector: bool = probe.query_row(
            params![
                generation.generation_id,
                i64::from(generation.vector_dimension),
                occurrence_id
            ],
            |row| row.get(0),
        )?;
        if has_vector {
            covered.insert(occurrence_id.to_string());
        }
    }
    Ok(covered)
}
