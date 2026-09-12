//! Maps live projection occurrences to [`EligibilityCandidate`]s and carries the kernel's verdicts back as [`Disposition`]s. A verdict is snapshot-bound; the kernel revalidates canonical state at use.

use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, EgressSnapshot, EligibilityCandidate, EligibilityVerdict, KernelError,
    KernelStore, ProjectScope,
};
use rusqlite::params;
use storage::GuardedConn;

use crate::ProjectionError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceCandidate {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    pub candidate: EligibilityCandidate,
}

/// Live occurrences of `class`, or of every class, in `(class, occurrence_id)` order. A set larger than `max` is refused whole rather than truncated.
///
/// # Errors
///
/// Returns [`ProjectionError::TooManyRecords`] when more than `max` live occurrences match, [`ProjectionError::CorruptRow`] for a stored class outside the contract, and the SQLite error otherwise.
pub fn live_candidates(
    conn: &GuardedConn<'_>,
    class: Option<OccurrenceClass>,
    max: NonZeroUsize,
) -> Result<Vec<OccurrenceCandidate>, ProjectionError> {
    let limit = i64::try_from(max.get()).map_err(|_| ProjectionError::CorruptRow)?;
    let mut statement = conn.prepare_cached(
        "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest
         FROM occurrences o
         LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
         WHERE t.occurrence_id IS NULL AND (?1 IS NULL OR o.class=?1)
         ORDER BY o.class,o.occurrence_id
         LIMIT ?2",
    )?;
    let rows = statement
        .query_map(
            params![class.map(OccurrenceClass::code), limit + 1],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > max.get() {
        return Err(ProjectionError::TooManyRecords { count: rows.len() });
    }
    rows.into_iter()
        .map(
            |(occurrence_id, class, source_object_id, revision, digest)| {
                Ok(OccurrenceCandidate {
                    occurrence_id,
                    class: OccurrenceClass::from_code(&class).ok_or(ProjectionError::CorruptRow)?,
                    candidate: EligibilityCandidate {
                        object_id: source_object_id,
                        source_revision: revision,
                        artifact_digest: Some(digest),
                    },
                })
            },
        )
        .collect()
}

/// `PolicyExcluded` carries the kernel's verdict itself, never a paraphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Eligible,
    PolicyExcluded(EligibilityVerdict),
}

impl From<EligibilityVerdict> for Disposition {
    fn from(verdict: EligibilityVerdict) -> Self {
        match verdict {
            EligibilityVerdict::Ok => Self::Eligible,
            excluded => Self::PolicyExcluded(excluded),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgedOccurrence {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    pub disposition: Disposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassExclusion {
    pub class: OccurrenceClass,
    pub verdict: EligibilityVerdict,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibilityReport {
    pub snapshot: EgressSnapshot,
    pub occurrences: Vec<JudgedOccurrence>,
}

impl EligibilityReport {
    /// An unknown classification generation means a classification merge overlapped the read, so no cache may reuse the verdicts.
    pub fn is_reusable(&self) -> bool {
        self.snapshot.classification_generation.is_some()
    }

    /// Exclusions grouped by class and verdict, in class order and then first-seen verdict order.
    pub fn exclusions(&self) -> Vec<ClassExclusion> {
        let mut out: Vec<ClassExclusion> = Vec::new();
        for class in OccurrenceClass::ALL {
            for judged in self
                .occurrences
                .iter()
                .filter(|judged| judged.class == class)
            {
                let Disposition::PolicyExcluded(verdict) = judged.disposition else {
                    continue;
                };
                match out
                    .iter_mut()
                    .find(|entry| entry.class == class && entry.verdict == verdict)
                {
                    Some(entry) => entry.count += 1,
                    None => out.push(ClassExclusion {
                        class,
                        verdict,
                        count: 1,
                    }),
                }
            }
        }
        out
    }
}

/// Judges every candidate in one kernel batch at one snapshot.
///
/// # Errors
///
/// Returns the kernel's [`KernelError::InvalidInput`] when the batch exceeds [`kernel::MAX_ELIGIBILITY_CANDIDATES`] or a candidate is malformed; the caller pages rather than joining verdicts from two snapshots. Other kernel errors are the kernel's.
pub fn judge_occurrences(
    kernel: &KernelStore,
    project: &ProjectScope,
    destination: ArtifactDestination,
    candidates: &[OccurrenceCandidate],
) -> Result<EligibilityReport, KernelError> {
    let kernel_candidates: Vec<EligibilityCandidate> = candidates
        .iter()
        .map(|candidate| candidate.candidate.clone())
        .collect();
    let batch = kernel.judge_eligibility(project, destination, &kernel_candidates)?;
    debug_assert_eq!(batch.verdicts.len(), candidates.len());
    Ok(EligibilityReport {
        snapshot: batch.snapshot,
        occurrences: candidates
            .iter()
            .zip(batch.verdicts)
            .map(|(candidate, verdict)| JudgedOccurrence {
                occurrence_id: candidate.occurrence_id.clone(),
                class: candidate.class,
                disposition: verdict.into(),
            })
            .collect(),
    })
}
