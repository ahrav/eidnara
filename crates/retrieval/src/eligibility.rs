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

const LIVE_CANDIDATES_SQL: &str =
    "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest
     FROM occurrences o
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     WHERE t.occurrence_id IS NULL
     ORDER BY o.class,o.occurrence_id
     LIMIT ?1";

const LIVE_CANDIDATES_BY_CLASS_SQL: &str =
    "SELECT o.occurrence_id,o.class,o.source_object_id,o.revision,o.source_artifact_digest
     FROM occurrences o
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     WHERE t.occurrence_id IS NULL AND o.class=?1
     ORDER BY o.class,o.occurrence_id
     LIMIT ?2";

type LiveRow = (String, String, String, i64, String);

fn live_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LiveRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
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
    let rows: Vec<LiveRow> = match class {
        Some(class) => conn
            .prepare_cached(LIVE_CANDIDATES_BY_CLASS_SQL)?
            .query_map(params![class.code(), limit + 1], live_row)?
            .collect::<rusqlite::Result<_>>()?,
        None => conn
            .prepare_cached(LIVE_CANDIDATES_SQL)?
            .query_map(params![limit + 1], live_row)?
            .collect::<rusqlite::Result<_>>()?,
    };
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

#[cfg(test)]
mod tests {
    use super::LIVE_CANDIDATES_BY_CLASS_SQL;
    use rusqlite::{Connection, StatementStatus, params};

    /// A class-scoped read must search `idx_occurrences_source` by class and remain bounded as an unrelated class grows.
    #[test]
    fn class_scoped_live_candidates_query_does_not_scan_other_classes() {
        let mut measured = Vec::new();
        for count in [1_024, 16_384] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(crate::BASELINE).unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            conn.execute_batch("INSERT INTO payloads VALUES ('p',x'61',1,0);")
                .unwrap();
            conn.execute(
                "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<?1)
                 INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                     payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                     source_artifact_digest,created_commit_seq,persisted_at)
                 SELECT printf('m%05d',n),x'00','lineage','messages',1,'text','p','domain','normal',
                     'source','evidence','digest',1,0 FROM ids",
                [count],
            )
            .unwrap();
            conn.execute_batch(
                "INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                     payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                     source_artifact_digest,created_commit_seq,persisted_at)
                 VALUES ('pm1',x'00','l','promoted_memory',1,'text','p','d','normal','s','e','digest',1,0),
                        ('pm2',x'00','l','promoted_memory',1,'text','p','d','normal','s','e','digest',1,0),
                        ('pm3',x'00','l','promoted_memory',1,'text','p','d','normal','s','e','digest',1,0);",
            )
            .unwrap();
            let plan = conn
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {LIVE_CANDIDATES_BY_CLASS_SQL}"
                ))
                .unwrap()
                .query_map(params!["promoted_memory", 65], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plan.iter().any(|detail| detail
                    .contains("SEARCH o USING INDEX idx_occurrences_source (class=?)")),
                "class filter must search the class index range: {plan:?}"
            );
            let mut statement = conn.prepare(LIVE_CANDIDATES_BY_CLASS_SQL).unwrap();
            let ids = statement
                .query_map(params!["promoted_memory", 65], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids, ["pm1", "pm2", "pm3"]);
            measured.push((count, statement.get_status(StatementStatus::VmStep)));
        }
        assert!(
            measured.iter().all(|(_, steps)| *steps < 256),
            "class-scoped read must not walk the messages backlog: {measured:?}"
        );
        assert!(
            measured[1].1 <= measured[0].1 * 2,
            "step count must not grow with an unrelated class: {measured:?}"
        );
    }
}
