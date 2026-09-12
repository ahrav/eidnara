//! The daemon's coverage report: the projection's per-class observation joined with the kernel's policy-exclusion dispositions for the same live occurrences. The projection is read once, in one transaction, for the live candidates, the counts, and the candidates' covered subset; the kernel judges the candidates in one batch at one snapshot, and both identities are reported. The counts describe the whole projection store; the exclusions are judged for the caller's project, since only the kernel knows which rows that project may serve. A kernel judgement that cannot be reused, a kernel tip behind the projection's checkpoint, or a live set beyond one kernel batch makes the whole report unavailable; no cell is filled from another observation.

use std::num::NonZeroUsize;

use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, EgressSnapshot, EligibilityVerdict, KernelError, KernelStore,
    MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
};
use retrieval::ProjectionError;
use retrieval::batch::{VectorGeneration, dense_eligible};
use retrieval::coverage::{ClassCoverage, CoverageBounds, CoverageReport, CoverageUnavailable};
use retrieval::eligibility::{Disposition, live_candidates};

use crate::search_projection::{SearchProjection, SearchProjectionError};

/// Why no report was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportUnavailable {
    Projection(CoverageUnavailable),
    /// A classification merge overlapped the kernel read, so its verdicts are not one snapshot's.
    KernelSnapshotIncoherent,
    /// The kernel's tip lies behind the projection's checkpoint, so the two describe different histories.
    KernelBehindProjection {
        tip: i64,
        checkpoint: i64,
    },
    /// More live occurrences than the bounds admit or one kernel batch judges at one snapshot.
    TooManyLiveOccurrences {
        count: usize,
    },
}

/// Live occurrences of one class the kernel excludes under one verdict, recorded in the coverage cell where they were counted, so an eligible ratio uses the same observation. A dense class splits its exclusions between `covered` and `missing`; a lexical-only class holds no vector, so its exclusions sit in `lexical_only`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassExclusion {
    pub class: OccurrenceClass,
    pub verdict: EligibilityVerdict,
    /// Excluded occurrences counted in the class's `valid_vectors`; zero for a lexical-only class.
    pub covered: usize,
    /// Excluded occurrences counted in the class's `missing`; zero for a lexical-only class.
    pub missing: usize,
    /// Excluded occurrences of a lexical-only class, counted in its `lexical` alone; zero for a dense class.
    pub lexical_only: usize,
}

impl ClassExclusion {
    pub fn count(&self) -> usize {
        self.covered + self.missing + self.lexical_only
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionCoverage {
    pub report: CoverageReport,
    /// The kernel snapshot every exclusion was judged at.
    pub kernel_snapshot: EgressSnapshot,
    /// In class order, then first-seen verdict order.
    pub exclusions: Vec<ClassExclusion>,
}

impl ProjectionCoverage {
    pub fn class(&self, class: OccurrenceClass) -> &ClassCoverage {
        self.report.class(class)
    }

    /// Live occurrences of `class` the kernel excludes under any verdict.
    pub fn excluded(&self, class: OccurrenceClass) -> usize {
        self.exclusions
            .iter()
            .filter(|exclusion| exclusion.class == class)
            .map(ClassExclusion::count)
            .sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error(transparent)]
    Projection(#[from] SearchProjectionError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

/// Reads the projection's observation, its live candidates, and the candidates' covered subset in one read transaction, then judges the candidates in one kernel batch. `kernel_incarnation_id` names the incarnation of `kernel`.
///
/// # Errors
///
/// Returns the projection's or the kernel's error when a read fails; an incoherent or absent observation is reported as [`ReportUnavailable`].
pub fn observe_coverage(
    projection: &SearchProjection,
    kernel: &KernelStore,
    kernel_incarnation_id: &str,
    project: &ProjectScope,
    destination: ArtifactDestination,
    generation: &VectorGeneration,
    bounds: CoverageBounds,
) -> Result<Result<ProjectionCoverage, ReportUnavailable>, CoverageError> {
    let max_live = NonZeroUsize::new(bounds.max_live().min(MAX_ELIGIBILITY_CANDIDATES))
        .expect("a nonzero bound stays nonzero");
    let observed = projection.read(|conn| {
        // The observation walks at most its bound per class, so it refuses an
        // oversized class before the candidate read sorts that class.
        let report =
            match retrieval::coverage::observe(conn, kernel_incarnation_id, generation, bounds)? {
                Ok(report) => report,
                Err(unavailable) => return Ok(Err(ReportUnavailable::Projection(unavailable))),
            };
        let candidates = match live_candidates(conn, None, max_live) {
            Ok(candidates) => candidates,
            Err(ProjectionError::TooManyRecords { count }) => {
                return Ok(Err(ReportUnavailable::TooManyLiveOccurrences { count }));
            }
            Err(error) => return Err(error),
        };
        let covered = retrieval::coverage::covered_among(
            conn,
            generation,
            candidates
                .iter()
                .filter(|candidate| dense_eligible(candidate.class))
                .map(|candidate| candidate.occurrence_id.as_str()),
        )?;
        Ok(Ok((report, candidates, covered)))
    })?;
    let (report, candidates, covered) = match observed {
        Ok(observed) => observed,
        Err(unavailable) => return Ok(Err(unavailable)),
    };
    let judged =
        retrieval::eligibility::judge_occurrences(kernel, project, destination, &candidates)?;
    if !judged.is_reusable() {
        return Ok(Err(ReportUnavailable::KernelSnapshotIncoherent));
    }
    if judged.snapshot.tip < report.checkpoint.checkpoint_commit_seq {
        return Ok(Err(ReportUnavailable::KernelBehindProjection {
            tip: judged.snapshot.tip,
            checkpoint: report.checkpoint.checkpoint_commit_seq,
        }));
    }
    let mut exclusions: Vec<ClassExclusion> = Vec::new();
    for judged in &judged.occurrences {
        let Disposition::PolicyExcluded(verdict) = judged.disposition else {
            continue;
        };
        let entry = match exclusions
            .iter_mut()
            .find(|entry| entry.class == judged.class && entry.verdict == verdict)
        {
            Some(entry) => entry,
            None => {
                exclusions.push(ClassExclusion {
                    class: judged.class,
                    verdict,
                    covered: 0,
                    missing: 0,
                    lexical_only: 0,
                });
                exclusions.last_mut().expect("just pushed")
            }
        };
        if !dense_eligible(judged.class) {
            entry.lexical_only += 1;
        } else if covered.contains(&judged.occurrence_id) {
            entry.covered += 1;
        } else {
            entry.missing += 1;
        }
    }
    exclusions.sort_by_key(|entry| {
        OccurrenceClass::ALL
            .iter()
            .position(|class| *class == entry.class)
    });
    Ok(Ok(ProjectionCoverage {
        report,
        kernel_snapshot: judged.snapshot,
        exclusions,
    }))
}
