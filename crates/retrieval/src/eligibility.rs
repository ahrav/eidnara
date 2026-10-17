//! Maps live projection occurrences to [`EligibilityCandidate`]s and carries the kernel's verdicts back as [`Disposition`]s. A verdict is snapshot-bound; the kernel revalidates canonical state at use.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, CommitReadIncarnation, EgressSnapshot, EligibilityBatch,
    EligibilityCandidate, EligibilityVerdict, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES,
    ProjectScope,
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

impl OccurrenceCandidate {
    pub fn new(
        occurrence_id: String,
        class: OccurrenceClass,
        object_id: String,
        source_revision: i64,
        artifact_digest: String,
    ) -> Self {
        Self {
            occurrence_id,
            class,
            candidate: EligibilityCandidate {
                object_id,
                source_revision,
                artifact_digest: Some(artifact_digest),
            },
        }
    }
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
/// Returns [`ProjectionError::TooManyRecords`] when more than `max` live occurrences match, [`ProjectionError::CorruptRow`] for a stored class outside the contract, and the SQLite error otherwise. A `max` at or past `i64::MAX` reads every live row: no set can exceed it.
pub fn live_candidates(
    conn: &GuardedConn<'_>,
    class: Option<OccurrenceClass>,
    max: NonZeroUsize,
) -> Result<Vec<OccurrenceCandidate>, ProjectionError> {
    // One row past `max` proves the set is too large; the probe saturates rather than overflowing.
    let probe = i64::try_from(max.get().saturating_add(1)).unwrap_or(i64::MAX);
    let rows: Vec<LiveRow> = match class {
        Some(class) => conn
            .prepare_cached(LIVE_CANDIDATES_BY_CLASS_SQL)?
            .query_map(params![class.code(), probe], live_row)?
            .collect::<rusqlite::Result<_>>()?,
        None => conn
            .prepare_cached(LIVE_CANDIDATES_SQL)?
            .query_map(params![probe], live_row)?
            .collect::<rusqlite::Result<_>>()?,
    };
    if rows.len() > max.get() {
        return Err(ProjectionError::TooManyRecords { count: rows.len() });
    }
    rows.into_iter().map(candidate).collect()
}

fn candidate(
    (occurrence_id, class, source_object_id, revision, digest): LiveRow,
) -> Result<OccurrenceCandidate, ProjectionError> {
    Ok(OccurrenceCandidate::new(
        occurrence_id,
        OccurrenceClass::from_code(&class).ok_or(ProjectionError::CorruptRow)?,
        source_object_id,
        revision,
        digest,
    ))
}

/// `PolicyExcluded` carries the kernel's verdict itself, never a paraphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Eligible,
    PolicyExcluded(EligibilityVerdict),
}

/// The scope and destination every candidate of one request is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authority<'a> {
    pub project: &'a ProjectScope,
    pub destination: ArtifactDestination,
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
    pub incarnation: CommitReadIncarnation,
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
/// Returns the kernel's [`KernelError::InvalidInput`] when the batch exceeds [`kernel::MAX_ELIGIBILITY_CANDIDATES`] or a candidate is malformed; the caller pages rather than joining verdicts from two snapshots. Also returns [`KernelError::InvalidInput`] when the kernel answers with a verdict count other than the candidate count; that batch is refused outright, and paging does not repair it. Other kernel errors are the kernel's.
pub fn judge_occurrences(
    kernel: &KernelStore,
    project: &ProjectScope,
    destination: ArtifactDestination,
    candidates: &[OccurrenceCandidate],
) -> Result<EligibilityReport, KernelError> {
    let batch = kernel.judge_eligibility(project, destination, &kernel_candidates(candidates))?;
    report(candidates, batch)
}

/// [`judge_occurrences`] whose reader wait and read stop at `budget` with [`KernelError::Deadline`].
///
/// # Errors
///
/// As [`judge_occurrences`], plus [`KernelError::Deadline`] once `budget` ends.
pub fn judge_occurrences_within_budget(
    kernel: &KernelStore,
    project: &ProjectScope,
    destination: ArtifactDestination,
    candidates: &[OccurrenceCandidate],
    budget: &EvalBudget,
) -> Result<EligibilityReport, KernelError> {
    let batch = kernel.judge_eligibility_within_budget(
        project,
        destination,
        &kernel_candidates(candidates),
        budget,
    )?;
    report(candidates, batch)
}

fn kernel_candidates(candidates: &[OccurrenceCandidate]) -> Vec<EligibilityCandidate> {
    candidates
        .iter()
        .map(|candidate| candidate.candidate.clone())
        .collect()
}

/// The kernel judges a row by these terms alone.
pub trait CandidateTerms {
    fn object_id(&self) -> &str;
    fn source_revision(&self) -> i64;
    fn artifact_digest(&self) -> Option<&str>;
}

impl CandidateTerms for OccurrenceCandidate {
    fn object_id(&self) -> &str {
        &self.candidate.object_id
    }

    fn source_revision(&self) -> i64 {
        self.candidate.source_revision
    }

    fn artifact_digest(&self) -> Option<&str> {
        self.candidate.artifact_digest.as_deref()
    }
}

/// `dispositions` preserves input order; `snapshot` and `incarnation` identify the state read by the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispositions {
    pub snapshot: EgressSnapshot,
    pub incarnation: CommitReadIncarnation,
    pub dispositions: Vec<Disposition>,
}

/// `judge_dispositions_within_budget` follows [`judge_occurrences_within_budget`]'s contract and returns dispositions in input order. Rows with equal object, revision, and artifact digest are judged once and share the verdict.
///
/// # Errors
///
/// As [`judge_occurrences_within_budget`]; the candidate bound counts every row, duplicates included, and the verdict count must equal the distinct candidate count.
pub fn judge_dispositions_within_budget<T: CandidateTerms>(
    kernel: &KernelStore,
    project: &ProjectScope,
    destination: ArtifactDestination,
    rows: &[T],
    budget: &EvalBudget,
) -> Result<Dispositions, KernelError> {
    let distinct = Distinct::of(rows)?;
    let batch = kernel.judge_eligibility_within_budget(
        project,
        destination,
        &distinct.candidates,
        budget,
    )?;
    aligned(distinct.candidates.len(), batch.verdicts.len())?;
    Ok(Dispositions {
        snapshot: batch.snapshot,
        incarnation: batch.incarnation,
        dispositions: distinct
            .positions
            .iter()
            .map(|&position| batch.verdicts[position].into())
            .collect(),
    })
}

/// The batch's distinct kernel candidates in first-seen order, and the index of each input's candidate among them. One kernel batch judges every distinct candidate under one project, destination, and snapshot, so inputs with equal candidates share that verdict.
struct Distinct {
    candidates: Vec<EligibilityCandidate>,
    positions: Vec<usize>,
}

/// The object, revision, and artifact digest a kernel verdict is keyed by.
type Terms<'a> = (&'a str, i64, Option<&'a str>);

impl Distinct {
    /// `Distinct::of` applies [`MAX_ELIGIBILITY_CANDIDATES`] to the input count, so every duplicate occurrence consumes the kernel's per-call allowance.
    fn of<T: CandidateTerms>(rows: &[T]) -> Result<Self, KernelError> {
        if rows.len() > MAX_ELIGIBILITY_CANDIDATES {
            return Err(KernelError::InvalidInput);
        }
        let mut seen: HashMap<Terms<'_>, usize> = HashMap::with_capacity(rows.len());
        let mut distinct = Self {
            candidates: Vec::new(),
            positions: Vec::with_capacity(rows.len()),
        };
        let mut previous: Option<(Terms<'_>, usize)> = None;
        for row in rows {
            let key: Terms<'_> = (
                row.object_id(),
                row.source_revision(),
                row.artifact_digest(),
            );
            let position = match previous {
                Some((last, position)) if last == key => position,
                _ => *seen.entry(key).or_insert_with(|| {
                    distinct.candidates.push(EligibilityCandidate {
                        object_id: key.0.to_owned(),
                        source_revision: key.1,
                        artifact_digest: key.2.map(str::to_owned),
                    });
                    distinct.candidates.len() - 1
                }),
            };
            previous = Some((key, position));
            distinct.positions.push(position);
        }
        Ok(distinct)
    }
}

/// Pairs each candidate with its verdict by position. A batch whose verdict count differs from the candidate count is refused, so a short batch cannot drop candidates from a report that still reads as a judgment of every one.
fn report(
    candidates: &[OccurrenceCandidate],
    batch: EligibilityBatch,
) -> Result<EligibilityReport, KernelError> {
    aligned(candidates.len(), batch.verdicts.len())?;
    Ok(EligibilityReport {
        snapshot: batch.snapshot,
        incarnation: batch.incarnation,
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

/// `KernelError::InvalidInput` unless the kernel returned one verdict per candidate.
fn aligned(candidates: usize, verdicts: usize) -> Result<(), KernelError> {
    if candidates == verdicts {
        Ok(())
    } else {
        Err(KernelError::InvalidInput)
    }
}

/// How a later batch's kernel state differs from the first batch's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityMoved {
    /// The kernel database was replaced, so no verdict of the first batch describes the same facts.
    Incarnation,
    /// The kernel committed between batches, so later verdicts describe other facts.
    Snapshot,
}

/// Compares `report` with the state a request first judged under. An incarnation change is reported before a snapshot change because a new incarnation makes snapshot equality meaningless.
/// An unknown classification generation counts as a moved snapshot; see [`EgressSnapshot::classification_generation`].
pub fn authority_moved(
    initial_snapshot: Option<&EgressSnapshot>,
    initial_incarnation: Option<&CommitReadIncarnation>,
    report: &EligibilityReport,
) -> Option<AuthorityMoved> {
    snapshot_moved(
        initial_snapshot,
        initial_incarnation,
        &report.snapshot,
        &report.incarnation,
    )
}

pub fn snapshot_moved(
    initial_snapshot: Option<&EgressSnapshot>,
    initial_incarnation: Option<&CommitReadIncarnation>,
    snapshot: &EgressSnapshot,
    incarnation: &CommitReadIncarnation,
) -> Option<AuthorityMoved> {
    if initial_incarnation.is_some_and(|initial| initial != incarnation) {
        Some(AuthorityMoved::Incarnation)
    } else if snapshot.classification_generation.is_none()
        || initial_snapshot.is_some_and(|initial| initial != snapshot)
    {
        Some(AuthorityMoved::Snapshot)
    } else {
        None
    }
}

/// Judges one batch within `budget`, records the first batch's snapshot and incarnation in `snapshot` and `incarnation`, and reports whether this batch's state differs from that first state.
///
/// # Errors
///
/// As [`judge_occurrences_within_budget`]; a [`KernelError::Deadline`] leaves `snapshot` and `incarnation` unchanged.
pub fn judge_tracked(
    kernel: &KernelStore,
    authority: Authority<'_>,
    candidates: &[OccurrenceCandidate],
    budget: &EvalBudget,
    snapshot: &mut Option<EgressSnapshot>,
    incarnation: &mut Option<CommitReadIncarnation>,
) -> Result<(EligibilityReport, Option<AuthorityMoved>), KernelError> {
    let report = judge_occurrences_within_budget(
        kernel,
        authority.project,
        authority.destination,
        candidates,
        budget,
    )?;
    let moved = authority_moved(snapshot.as_ref(), incarnation.as_ref(), &report);
    snapshot.get_or_insert(report.snapshot);
    incarnation.get_or_insert(report.incarnation);
    Ok((report, moved))
}

/// Counts `verdict` in a tally kept in first-seen verdict order.
pub fn tally_exclusion(tally: &mut Vec<(EligibilityVerdict, usize)>, verdict: EligibilityVerdict) {
    match tally.iter_mut().find(|(seen, _)| *seen == verdict) {
        Some((_, count)) => *count += 1,
        None => tally.push((verdict, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::{LIVE_CANDIDATES_BY_CLASS_SQL, aligned};
    use kernel::KernelError;
    use rusqlite::{Connection, StatementStatus, params};

    #[test]
    fn a_batch_must_carry_one_verdict_per_candidate() {
        assert_eq!(aligned(3, 3), Ok(()));
        assert_eq!(aligned(0, 0), Ok(()));
        assert_eq!(aligned(3, 2), Err(KernelError::InvalidInput));
        assert_eq!(aligned(2, 3), Err(KernelError::InvalidInput));
    }

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
