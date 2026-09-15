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
    // The same four fields `bind_lane` compares.
    let built_for = (
        identity.embedding_model.as_str(),
        identity.tokenizer_fingerprint.as_str(),
        identity.vector_dimension,
        identity.generation_epoch,
    );
    let requested = (
        generation.embedding_model.as_str(),
        generation.tokenizer_fingerprint.as_str(),
        generation.vector_dimension,
        generation.generation_epoch,
    );
    if built_for != requested {
        return Ok(Err(CoverageUnavailable::GenerationMismatch));
    }
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

/// Durable work the dispatcher would still hand out for the observed generation.
pub(crate) const CURRENT_PENDING: &str = "EXISTS(SELECT 1 FROM embedding_jobs j
    WHERE j.occurrence_id=o.occurrence_id AND j.generation_id=?1 AND j.stop_reason IS NULL
      AND (j.state='pending' OR (j.state='admitted' AND j.host_job_id IS NOT NULL)))";

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

/// Supply the complete inventory; matching counts alone do not prove completeness.
///
/// # Errors
///
/// A failed check returns no certificate. Collision and database errors are not downgraded to absence.
pub fn verify_construction(
    conn: &GuardedConn<'_>,
    batch: &crate::batch::ProjectionBatch<'_>,
    generation: &VectorGeneration,
    bounds: CoverageBounds,
) -> Result<CoverageReport, ProjectionError> {
    use crate::batch::{BatchStatus, batch_status};
    let report = observe(
        conn,
        &batch.identity.kernel_incarnation_id,
        generation,
        bounds,
    )?
    .map_err(|_| ProjectionError::CorruptRow)?;
    let mut unique = std::collections::HashSet::new();
    let mut payloads = std::collections::HashSet::new();
    for record in &batch.records {
        let (encoded, selected) = crate::encode_record(record)?;
        let (occurrence_id, payload_id) = crate::canonical_digests(&encoded, selected);
        unique.insert(occurrence_id);
        payloads.insert(payload_id);
    }
    let invalidated: std::collections::HashSet<_> = batch
        .invalidations
        .iter()
        .map(|row| &row.occurrence_id)
        .collect();
    let (rows, payload_rows, tombstones, vectors, jobs, fresh_pending, obsolete, history): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn.query_row(
        "SELECT (SELECT count(*) FROM occurrences),
         (SELECT count(*) FROM payloads),
         (SELECT count(*) FROM occurrence_tombstones),
         (SELECT count(*) FROM occurrence_vectors),
         (SELECT count(*) FROM embedding_jobs),
         (SELECT count(*) FROM embedding_jobs WHERE state='pending' AND attempts=0
            AND episode_allowance=0 AND episode_id IS NULL AND episode_deadline IS NULL
            AND next_attempt_at IS NULL AND admitted_epoch IS NULL AND last_failure_kind IS NULL
            AND host_job_id IS NULL AND host_incarnation IS NULL AND stop_reason IS NULL
            AND authorization_ref IS NULL),
         (SELECT count(*) FROM embedding_jobs WHERE state='obsolete'),
         (SELECT count(*) FROM embedding_recovery_authorizations)
           + (SELECT count(*) FROM retirement_receipts)",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        },
    )?;
    let excluded_jobs = excluded_class_jobs(conn)?;
    let only_generation: bool = conn.query_row(
        "SELECT count(*)=1 AND min(generation_id)=?1 AND min(state)='building' FROM vector_generations",
        [&generation.generation_id], |row| row.get(0),
    )?;
    if invalidated.len() != batch.invalidations.len()
        || !only_generation
        || unique.len() != batch.records.len()
        || usize::try_from(rows).ok() != Some(batch.records.len())
        || usize::try_from(payload_rows).ok() != Some(payloads.len())
        || usize::try_from(tombstones).ok() != Some(batch.invalidations.len())
        || vectors != 0
        || jobs != fresh_pending + obsolete
        || history != 0
        || excluded_jobs
        || batch.generation_id != Some(generation.generation_id.as_str())
        || report.checkpoint.checkpoint_commit_seq != batch.identity.through_commit_seq
        || report
            .classes
            .iter()
            .any(|class| class.missing_without_pending != 0)
        || usize::try_from(fresh_pending).ok()
            != Some(
                report
                    .classes
                    .iter()
                    .map(|class| class.pending)
                    .sum::<usize>(),
            )
    {
        return Err(ProjectionError::CorruptRow);
    }
    if batch_status(conn, batch)? != BatchStatus::Applied {
        return Err(ProjectionError::CorruptRow);
    }
    crate::lexical::verify_rows(conn)?;
    Ok(report)
}

/// Both acceptance gates read this one predicate so the classes that queue work cannot drift
/// between construction and reopen.
fn excluded_class_jobs(conn: &GuardedConn<'_>) -> Result<bool, ProjectionError> {
    let mut statement = conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM embedding_jobs j JOIN occurrences o USING(occurrence_id) WHERE o.class=?1)",
    )?;
    for class in OccurrenceClass::ALL
        .into_iter()
        .filter(|class| !dense_eligible(*class))
    {
        if statement.query_row([class.code()], |row| row.get::<_, bool>(0))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `PRAGMA integrity_check` scans the whole database and re-tokenizes every lexical row to verify the inverted index, so its cost grows with file size and payload bytes.
/// `CoverageBounds` does not bound database-wide integrity checks.
pub fn verify_pages(conn: &GuardedConn<'_>) -> Result<(), ProjectionError> {
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok"
        || conn
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some()
    {
        return Err(ProjectionError::CorruptRow);
    }
    crate::lexical::verify_rows(conn)
}

/// Checks mutable state without comparing it to immutable seed bytes. Required dense rows
/// need valid vectors or current durable work; completed jobs need their vectors.
pub fn verify_active(
    conn: &GuardedConn<'_>,
    expected: &ProjectionIdentity,
    generation: &VectorGeneration,
    bounds: CoverageBounds,
    now: i64,
) -> Result<CoverageReport, ProjectionError> {
    read_identity(conn)?
        .ok_or(ProjectionError::IdentityMismatch)?
        .require_compatible(expected)?;
    let report = observe(conn, &expected.kernel_incarnation_id, generation, bounds)?
        .map_err(|_| ProjectionError::CorruptRow)?;
    let (all_vectors, foreign_jobs, vectorless, generations): (i64, i64, i64, i64) = conn.query_row(
        "SELECT (SELECT count(*) FROM occurrence_vectors),
                (SELECT count(*) FROM embedding_jobs WHERE generation_id<>?1),
                (SELECT count(*) FROM embedding_jobs j WHERE state IN ('embedded','published')
                 AND NOT EXISTS(SELECT 1 FROM occurrence_vectors v WHERE v.occurrence_id=j.occurrence_id AND v.generation_id=j.generation_id)),
                (SELECT count(*) FROM vector_generations)",
        [&generation.generation_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
    )?;
    if foreign_jobs != 0 || vectorless != 0 || generations != 1 || excluded_class_jobs(conn)? {
        return Err(ProjectionError::CorruptRow);
    }
    if report.checkpoint.hold_id.is_empty()
        || report
            .classes
            .iter()
            .any(|class| class.missing_without_pending != 0)
    {
        return Err(ProjectionError::CorruptRow);
    }
    let mut statement = conn.prepare("SELECT occurrence_id FROM occurrences")?;
    for id in statement.query_map([], |row| row.get::<_, String>(0))? {
        let row = crate::read_occurrence(conn, &id?)?.ok_or(ProjectionError::CorruptRow)?;
        if row.created_commit_seq > report.checkpoint.checkpoint_commit_seq
            || row
                .tombstone
                .is_some_and(|t| t.invalidated_commit_seq > report.checkpoint.checkpoint_commit_seq)
        {
            return Err(ProjectionError::CorruptRow);
        }
    }
    let mut vectors = conn.prepare(
        "SELECT v.vector_dimension,CASE WHEN length(v.vector)=4*v.vector_dimension THEN v.vector END
         FROM occurrence_vectors v JOIN vector_generations g USING(generation_id)
         WHERE v.vector_dimension=g.vector_dimension AND v.generation_id=?1"
    )?;
    let mut jobs = conn
        .prepare("SELECT job_id,occurrence_id,generation_id,next_attempt_at FROM embedding_jobs")?;
    for row in jobs.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })? {
        let (id, occurrence, generation, next_attempt_at) = row?;
        if id != crate::batch::job_id(&occurrence, &generation) {
            return Err(ProjectionError::CorruptRow);
        }
        let ledger = crate::dispatch::job_ledger(conn, &id)?.ok_or(ProjectionError::CorruptRow)?;
        if invalid_episode_state(&ledger, &id, next_attempt_at, now)? {
            return Err(ProjectionError::CorruptRow);
        }
        if let Some(reference) = ledger.authorization_ref.as_deref() {
            let recorded: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM embedding_recovery_authorizations WHERE job_id=?1 AND authorization_ref=?2)",
                params![id, reference],
                |row| row.get(0),
            )?;
            if !recorded {
                return Err(ProjectionError::CorruptRow);
            }
        }
    }
    let mut seen = 0i64;
    for row in vectors.query_map([&generation.generation_id], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, Vec<u8>>(1)?))
    })? {
        let (dimension, bytes) = row?;
        crate::dense::codec::decode_shape(&bytes, dimension).map_err(|rejection| {
            ProjectionError::InvalidVector {
                reason: rejection.reason(),
            }
        })?;
        seen += 1;
    }
    if seen != all_vectors {
        return Err(ProjectionError::CorruptRow);
    }
    Ok(report)
}

fn invalid_episode_state(
    ledger: &crate::dispatch::JobLedger,
    job_id: &str,
    next_attempt_at: Option<i64>,
    now: i64,
) -> Result<bool, ProjectionError> {
    let has_episode = ledger.episode()?.is_some();
    let expected_episode = match ledger.authorization_ref.as_deref() {
        Some(reference) if crate::dispatch::valid_authorization_ref(reference) => {
            crate::dispatch::authorized_episode_id(job_id, reference)
        }
        Some(_) => return Ok(true),
        None => crate::dispatch::first_episode_id(job_id),
    };
    let wrong_episode = match ledger.authorization_ref.as_deref() {
        Some(_) => ledger.episode_id.as_deref() != Some(expected_episode.as_str()),
        None => has_episode && ledger.episode_id.as_deref() != Some(expected_episode.as_str()),
    };
    let expired = matches!(ledger.state.as_str(), "pending" | "admitted")
        && has_episode
        && ledger
            .episode_deadline
            .is_some_and(|deadline| deadline < now);
    let retry_after_expiry = matches!(ledger.state.as_str(), "pending" | "admitted")
        && has_episode
        && next_attempt_at.is_some_and(|retry| {
            ledger
                .episode_deadline
                .is_some_and(|deadline| retry > deadline)
        });
    Ok(wrong_episode
        || expired
        || retry_after_expiry
        || (ledger.state == "pending"
            && ((has_episode && ledger.attempts >= ledger.episode_allowance)
                || (!has_episode && ledger.attempts != 0)))
        || (ledger.state == "admitted"
            && (!has_episode
                || ledger.host_job_id.is_none()
                || ledger.attempts == 0
                || ledger.attempts > ledger.episode_allowance)))
}

#[cfg(test)]
mod tests {
    use super::invalid_episode_state;
    use crate::dispatch::JobLedger;

    fn pending(deadline: i64) -> JobLedger {
        JobLedger {
            state: "pending".to_owned(),
            attempts: 0,
            episode_id: Some(crate::dispatch::first_episode_id("job")),
            episode_allowance: 1,
            episode_deadline: Some(deadline),
            host_job_id: None,
            host_incarnation: None,
            last_failure_kind: None,
            stop_reason: None,
            authorization_ref: None,
        }
    }

    #[test]
    fn active_verification_uses_the_dispatchers_inclusive_episode_deadline() {
        let ledger = pending(100);
        assert!(!invalid_episode_state(&ledger, "job", None, 100).unwrap());
        assert!(invalid_episode_state(&ledger, "job", None, 101).unwrap());
        assert!(invalid_episode_state(&ledger, "job", Some(101), 50).unwrap());
        let mut admitted = pending(100);
        admitted.state = "admitted".to_owned();
        admitted.attempts = 1;
        admitted.host_job_id = Some("host".to_owned());
        assert!(!invalid_episode_state(&admitted, "job", None, 100).unwrap());
        assert!(invalid_episode_state(&admitted, "job", None, 101).unwrap());
        admitted.host_job_id = None;
        assert!(invalid_episode_state(&admitted, "job", None, 100).unwrap());
        admitted.state = "embedded".to_owned();
        admitted.host_job_id = None;
        assert!(!invalid_episode_state(&admitted, "job", None, 101).unwrap());
        admitted.state = "obsolete".to_owned();
        assert!(!invalid_episode_state(&admitted, "job", Some(101), 50).unwrap());
        admitted.episode_id = None;
        admitted.episode_allowance = 0;
        admitted.episode_deadline = None;
        admitted.authorization_ref = Some("operator:recovery".to_owned());
        assert!(invalid_episode_state(&admitted, "job", None, 101).unwrap());
    }
}
